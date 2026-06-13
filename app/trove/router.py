import base64
import json
import logging
import re

import httpx
from fastapi import (
    APIRouter, BackgroundTasks, Body, Depends, File, Form, Query, Request,
    Response, UploadFile,
)
from fastapi.responses import FileResponse

from app.core.config import settings
from app.admin import ingest_log
from app.auth.models import User
from app.core.dependencies import (
    AccessContext,
    TokenContext,
    require_master_ingest,
    public_scope,
    require_scope,
)
from app.core.errors import APIError, ErrorCode
from app.core.ratelimit import check_rate_limit, rate_limit_headers
from app.core.utils import client_ip, utcnow
from app.events import bus as events_bus
from app.trove import (
    btt_releases,
    captures,
    chaos,
    delves,
    feeds,
    misc,
    news,
    rotations,
    server_time,
    stats,
    tmod,
)
from app.trove import calendar as trove_calendar
from app.trove.codexes import read as codexes_read
from app.trove.leaderboards import activity as leaderboards_activity
from app.trove.leaderboards import class_activity as leaderboards_class_activity
from app.trove.leaderboards import detection as leaderboards_detection
from app.trove.leaderboards import service as leaderboards_service
from app.trove.market import service as market_service
from app.trove.codexes.schemas import (
    CodexCategoryInfo,
    CodexCategoryList,
    CodexEntryOut,
    CodexEntryPage,
    CodexSearchPage,
    CodexTypeInfo,
    CodexTypeList,
)
from app.trove.codexes.types import ALL_TYPES as CODEX_TYPES
from app.trove.gems import builds as gem_builds
from app.trove.gems import evaluator as gem_evaluator
from app.trove.gems.model import Gem, gem_lookups
from app.trove.gems.schemas import (
    AugmentRequest,
    BuildConfigRequest,
    BuildOptions,
    BuildResponse,
    BuildResult,
    EvaluateRequest,
    EvaluationResult,
    GemActionResult,
    GemLookups,
    GemStatRange,
    GenerateGemRequest,
    LevelUpRequest,
    SetLevelRequest,
    StatPositionRequest,
)
from app.trove.misc import (
    ModdingSoftware,
    TimeConvertRequest,
    TimeConvertResponse,
    TimeNow,
    TimezoneList,
)
from app.trove.models import TroveEvent, TroveNews
from app.trove.schemas import (
    BiomeRotationFeed,
    BttAsset,
    BttChangelogGroup,
    BttChangelogOut,
    BttCommit,
    BttLatestPerPlatform,
    BttPlatformLatest,
    BttReleaseInfo,
    BttReleaseList,
    BttReleaseMeta,
    BttUpdateCheck,
    CaptureInsertRequest,
    CaptureInsertResponse,
    ChallengeCaptureOut,
    ChallengeCurrentOut,
    ChallengeHistoryPage,
    ChaosChest,
    ChaosChestCaptureOut,
    ActivityResponse,
    ActivityHistoryResponse,
    ActivitySeriesResponse,
    ClassActivityCurrentResponse,
    ClassActivitySeriesResponse,
    TroveStatusResponse,
    TroveStatusHistoryResponse,
    ChaosChestHistoryPage,
    CheatersResponse,
    ClassList,
    Corruxion,
    DailyBuffs,
    DelveRotationOut,
    DelveWeekInfo,
    DelveWeekList,
    EventCategoryList,
    FeedbackAck,
    Fluxion,
    Gardening,
    LeaderboardBoardOut,
    LeaderboardComparison,
    LeaderboardEntriesPage,
    LeaderboardEntryOut,
    LeaderboardInfo,
    LeaderboardInsertResponse,
    LeaderboardListOut,
    LeaderboardPlayerEntry,
    LeaderboardPlayerHistory,
    LeaderboardTimestamps,
    BoardHistoryResponse,
    PlayerHistorySeriesResponse,
    MarketInsertResponse,
    MarketItemList,
    MarketItemSummary,
    MarketListingOut,
    MarketListingsPage,
    ServerTime,
    StatTable,
    TroveClass,
    TroveEventItem,
    TroveEventList,
    TroveNewsHistory,
    TroveNewsItem,
    TroveNewsList,
    TwitchStream,
    TwitchStreams,
    Video,
    Videos,
    WeeklyBuffs,
    YearlyCalendar,
)
from app.trove.tmod import TmodBuildRequest, TmodReadResponse
from app.trove.updates import read as updates_read
from app.trove.updates import compare as updates_compare
from app.trove.updates.cas import ContentStore
from app.trove.updates.cdn import BRANCHES as UPDATE_BRANCHES
from app.trove.updates.schemas import (
    BranchInfo,
    BranchList,
    ChangeEntry,
    ChangeList,
    DiffHunk,
    DiffHunkLine,
    FileCompareResponse,
    FileHistoryEntry,
    FileHistoryList,
    FileMeta,
    FileVersionInfo,
    TreeEntry,
    TreeListing,
    VersionInfo,
    VersionList,
)

# Organized by FUNCTION, not by game - most of the API is Trove, so grouping by
# "trove" would lump everything together. Rotations/timers vs feeds are separate
# categories (OpenAPI tags) with their own scopes.
rotations_router = APIRouter(prefix="/v1/rotations", tags=["rotations"])
feeds_router = APIRouter(prefix="/v1/feeds", tags=["feeds"])
stats_router = APIRouter(prefix="/v1/stats", tags=["stats"])
gems_router = APIRouter(prefix="/v1/gems", tags=["gems"])
misc_router = APIRouter(prefix="/v1/misc", tags=["misc"])
mods_router = APIRouter(prefix="/v1/mods", tags=["mods"])
updates_router = APIRouter(prefix="/v1/updates", tags=["updates"])
codexes_router = APIRouter(prefix="/v1/codexes", tags=["codexes"])
btt_router = APIRouter(prefix="/v1/btt", tags=["btt"])
leaderboards_router = APIRouter(prefix="/v1/leaderboards", tags=["leaderboards"])
market_router = APIRouter(prefix="/v1/market", tags=["market"])
activity_router = APIRouter(prefix="/v1/activity", tags=["activity"])
class_activity_router = APIRouter(prefix="/v1/class-activity", tags=["class-activity"])

# rotations + feeds are PUBLIC: usable without a token at a stricter per-IP rate
# limit; a token carrying the scope lifts the caller to the full per-token limit.
_ROT = Depends(public_scope("rotations:read"))
_FEED = Depends(public_scope("feeds:read"))
# The Bilibili thumbnail proxy is hit once per <img> on a feed render, so it gets
# its own widened bucket (won't starve the shared feeds budget).
_FEED_IMG = Depends(public_scope("feeds:read", rate_multiplier=settings.bilibili_image_rate_limit_multiplier))
_STAT = Depends(require_scope("stats:read"))
_GEM = Depends(require_scope("gems:read"))
_MISC = Depends(require_scope("misc:read"))
# Tokenless slice of misc - the bot's interest-items list is public so dashboards
# / wikis can render it without a key. A token carrying misc:read still earns the
# wider per-token rate budget; anon callers pay the stricter per-IP cap.
_MISC_PUBLIC = Depends(public_scope("misc:read"))
_MODS = Depends(require_scope("mods:read"))
_UPD = Depends(require_scope("updates:read"))
# Raw file DOWNLOADS are tokenless so game files can be hot-linked directly
# (desktop app / wikis / a plain browser link); anon callers still pay the
# per-IP cap, and a token carrying updates:read still earns the wider
# per-token budget. The browse endpoints (branches / versions / changes /
# tree / file/meta) stay token-gated under _UPD.
_UPD_PUBLIC = Depends(public_scope("updates:read"))

# Used by the feedback webhook helper for "best-effort, log on failure".
logger = logging.getLogger("kiwi.trove.router")
# Codexes are PUBLIC too (game reference data): usable without a token, and at a
# wider rate budget (5× by default) on both the anonymous and authenticated paths.
_CODEX = Depends(public_scope("codexes:read", rate_multiplier=settings.codexes_rate_limit_multiplier))
# BTT releases are PUBLIC: the desktop app needs to poll them on every launch
# to drive update notifications, so no token is required.
_BTT = Depends(public_scope("btt:read"))
# Leaderboards read-side is token-gated (the data is bulky + opinionated). The
# write-side has its own dep - see /v1/leaderboards/insert below.
_LB = Depends(require_scope("leaderboards:read"))
# Public, tokenless surface for the cheater-detection endpoint - same
# reasoning as e.g. rotations: it's anti-cheat data the wider community
# benefits from, no reason to gate it behind an API token.
_LB_PUBLIC = Depends(public_scope("leaderboards:read"))
# Player-activity estimates get their OWN public scope (activity:read) so
# they're a first-class free endpoint family, not buried under leaderboards
# or misc. Tokenless; a token carrying the scope just earns the wider budget.
_ACTIVITY_PUBLIC = Depends(public_scope("activity:read"))
_LB_MASTER = Depends(require_master_ingest)
# Same shape: read gated by scope, write gated by superuser API token.
_MKT = Depends(require_scope("market:read"))
_MKT_MASTER = Depends(require_master_ingest)

# The codex serves the primary timeline by default; PTS is opt-in via ?branch=.
_DEFAULT_CODEX_BRANCH = "live-us"


# --- Rotations / timers (scope: rotations:read) ----------------------------

@rotations_router.get("/server-time", response_model=ServerTime)
async def get_server_time(ctx: AccessContext = _ROT) -> ServerTime:
    """Current Trove server time, current in-game day, and the next daily / weekly resets."""
    return ServerTime(**server_time.server_time())


@rotations_router.get("/daily-buffs", response_model=DailyBuffs)
async def get_daily_buffs(ctx: AccessContext = _ROT) -> DailyBuffs:
    """Today's daily buff plus the full Monday→Sunday rotation."""
    return DailyBuffs(**server_time.daily_buffs())


@rotations_router.get("/weekly-buffs", response_model=WeeklyBuffs)
async def get_weekly_buffs(ctx: AccessContext = _ROT) -> WeeklyBuffs:
    """This week's weekly buff plus the full 4-week rotation."""
    return WeeklyBuffs(**server_time.weekly_buffs())


@rotations_router.get("/corruxion", response_model=Corruxion)
async def get_corruxion(ctx: AccessContext = _ROT) -> Corruxion:
    """Corruxion merchant: live timer + upcoming schedule (14-day / 3-day cycle)."""
    return Corruxion(**server_time.corruxion())


@rotations_router.get("/fluxion", response_model=Fluxion)
async def get_fluxion(ctx: AccessContext = _ROT) -> Fluxion:
    """Fluxion merchant: live timer (voting/selling) + upcoming schedule."""
    return Fluxion(**server_time.fluxion())


@rotations_router.get("/gardening", response_model=Gardening)
async def get_gardening(ctx: AccessContext = _ROT) -> Gardening:
    """Gardening harvest windows: current 2-day and 3-day plant windows + what's next."""
    return Gardening(**server_time.gardening())


@rotations_router.get("/chaos-chest", response_model=ChaosChest)
async def get_chaos_chest(ctx: AccessContext = _ROT) -> ChaosChest:
    """The weekly Chaos Chest: current featured item + window + countdown.

    Source preference: the bot-captured item for the current week wins (it's
    read straight from the in-game cfg). Falls back to the Trovesaurus relay
    when the bot hasn't reported this week yet - e.g., immediately after a Tue
    11:00 UTC reset."""
    return ChaosChest(**await chaos.get_chaos_chest())


@rotations_router.post("/chaos-chest/insert", response_model=CaptureInsertResponse,
                       summary="Insert chaos chest data")
async def insert_chaos_chest(
    req: CaptureInsertRequest,
    _auth = Depends(require_master_ingest),
) -> CaptureInsertResponse:
    """Persist the bot-captured chaos-chest item for the current weekly window.

    **Master only**: requires an API token owned by a superuser account. The
    server infers the week anchor (Tue 11:00 UTC) from "now" - the bot just
    sends ``{name}``. Idempotent: re-submitting the same week replaces the row.
    """
    try:
        doc, was_new = await captures.insert_chaos_chest(req.name)
    except ValueError as e:
        await ingest_log.record(
            endpoint="/v1/rotations/chaos-chest/insert",
            user=_auth.user, token=_auth.token,
            summary={"name": req.name}, success=False, error=str(e),
        )
        raise APIError(status_code=400, code=ErrorCode.bad_request, message=str(e))
    await ingest_log.record(
        endpoint="/v1/rotations/chaos-chest/insert",
        user=_auth.user, token=_auth.token,
        summary={
            "name": doc.name, "anchor": doc.week_anchor,
            "refreshed": not was_new,
        },
    )
    # Push to live SSE subscribers immediately (dedup-guarded). Best-effort.
    try:
        await events_bus.publish_chaos()
    except Exception:
        logging.getLogger("kiwi.trove.router").warning(
            "chaos event publish failed", exc_info=True)
    return CaptureInsertResponse(
        anchor=doc.week_anchor, name=doc.name, refreshed=not was_new,
    )


@rotations_router.get("/chaos-chest/history", response_model=ChaosChestHistoryPage)
async def list_chaos_chest_history(
    ctx: AccessContext = _ROT,
    limit: int = Query(default=50, ge=1, le=200),
    offset: int = Query(default=0, ge=0),
) -> ChaosChestHistoryPage:
    """Past chaos-chest captures, newest week first. Public under
    ``rotations:read`` (the read-side is tokenless via the standard public
    rate limit; submitting requires master)."""
    docs, total = await captures.list_chaos_chest_history(limit=limit, offset=offset)
    items = [
        ChaosChestCaptureOut(
            week_anchor=d.week_anchor,
            week_ends_at=d.week_anchor + 7 * 86400,
            name=d.name,
            captured_at=d.captured_at,
        )
        for d in docs
    ]
    return ChaosChestHistoryPage(items=items, count=len(items), total=total)


@rotations_router.get("/challenge/current", response_model=ChallengeCurrentOut)
async def get_current_challenge(ctx: AccessContext = _ROT) -> ChallengeCurrentOut:
    """The hourly challenge active right now (or the most-recent window when
    we're in the gap between challenges).

    Cadence: one challenge per hour on most days; on trove Fridays
    (real-UTC Fri 11:00 → Sat 11:00) it's two per hour - :00 and :30 - and
    ``is_friday_window`` is set. Each window lasts 20 minutes; ``active``
    distinguishes the live window from the gap that follows.
    """
    return ChallengeCurrentOut(**await captures.get_current_challenge())


@rotations_router.post("/challenge/insert", response_model=CaptureInsertResponse,
                       summary="Insert challenge data")
async def insert_challenge(
    req: CaptureInsertRequest,
    _auth = Depends(require_master_ingest),
) -> CaptureInsertResponse:
    """Persist the bot-captured challenge name for the active 20-minute window.

    **Master only**. The server infers the window anchor from "now"; the bot
    just sends ``{name}``. Idempotent at the (anchor) level. ``name`` of
    ``"none"`` (or empty) is rejected - the bot is expected to skip those
    submissions client-side, so seeing one here surfaces as a 400.
    """
    try:
        doc, was_new = await captures.insert_challenge(req.name)
    except ValueError as e:
        await ingest_log.record(
            endpoint="/v1/rotations/challenge/insert",
            user=_auth.user, token=_auth.token,
            summary={"name": req.name}, success=False, error=str(e),
        )
        raise APIError(status_code=400, code=ErrorCode.bad_request, message=str(e))
    await ingest_log.record(
        endpoint="/v1/rotations/challenge/insert",
        user=_auth.user, token=_auth.token,
        summary={
            "name": doc.name, "anchor": doc.window_anchor,
            "refreshed": not was_new,
        },
    )
    # Push the update to live SSE subscribers immediately (dedup-guarded, so a
    # re-POST of the same name is a no-op). Best-effort: never fail the ingest.
    try:
        await events_bus.publish_challenge()
    except Exception:
        logging.getLogger("kiwi.trove.router").warning(
            "challenge event publish failed", exc_info=True)
    return CaptureInsertResponse(
        anchor=doc.window_anchor, name=doc.name, refreshed=not was_new,
    )


@rotations_router.get("/challenge/history", response_model=ChallengeHistoryPage)
async def list_challenge_history(
    ctx: AccessContext = _ROT,
    limit: int = Query(default=50, ge=1, le=500),
    offset: int = Query(default=0, ge=0),
) -> ChallengeHistoryPage:
    """Past challenge captures, newest window first. Public under
    ``rotations:read`` (tokenless via the standard public rate limit)."""
    docs, total = await captures.list_challenge_history(limit=limit, offset=offset)
    items = [
        ChallengeCaptureOut(
            window_anchor=d.window_anchor,
            window_ends_at=d.window_ends_at,
            name=d.name,
            type=captures.classify_challenge(d.name),
            is_friday_window=d.is_friday_window,
            captured_at=d.captured_at,
        )
        for d in docs
    ]
    return ChallengeHistoryPage(items=items, count=len(items), total=total)


@rotations_router.get("/calendar", response_model=YearlyCalendar)
async def get_calendar(ctx: AccessContext = _ROT) -> YearlyCalendar:
    """The full yearly event calendar (±365 days): every recurring rotation -
    weekly buffs, Corruxion/Fluxion, gardening, Wild Mana, Stampy - as one flat,
    start-sorted timeline. (Invasion is excluded.)"""
    return YearlyCalendar(**trove_calendar.yearly_calendar())


@rotations_router.get("/delves/weeks", response_model=DelveWeekList)
async def list_delve_weeks(ctx: AccessContext = _ROT) -> DelveWeekList:
    """The delve weeks available (metadata only), newest first, plus the live week id."""
    weeks = await delves.list_weeks()
    return DelveWeekList(
        current_week=delves.current_week_id(),
        items=[DelveWeekInfo(**w) for w in weeks],
        count=len(weeks),
    )


@rotations_router.get("/delves", response_model=DelveRotationOut)
async def get_delves(
    ctx: AccessContext = _ROT,
    week: int | None = Query(default=None, description="Week id; defaults to the current week"),
) -> DelveRotationOut:
    """A week's delve rotation - its floor records (relayed from an external source).
    Defaults to the current week; pass `?week=` for history (see `/delves/weeks`)."""
    current = delves.current_week_id()
    wk = week if week is not None else current
    doc = await delves.get_week(wk)
    if doc is None:
        if week is not None:
            raise APIError(status_code=404, code=ErrorCode.not_found,
                           message=f"No delve data for week {week}")
        # Current week not captured yet - return an empty rotation for it.
        return DelveRotationOut(week=wk, is_current=True, total=0, count=0,
                                fetched_at=None, depths=[])
    return DelveRotationOut(
        week=doc.week, is_current=(doc.week == current), total=doc.total,
        count=doc.depth_count, fetched_at=doc.fetched_at, depths=doc.depths,
    )


@rotations_router.get("/biomes", response_model=BiomeRotationFeed)
async def get_biomes(ctx: AccessContext = _ROT) -> BiomeRotationFeed:
    """The 3-hour adventure-world biome rotation (d15): current + upcoming."""
    return BiomeRotationFeed(**rotations.biome_rotation())


@rotations_router.get("/wild-mana", response_model=BiomeRotationFeed)
async def get_wild_mana(ctx: AccessContext = _ROT) -> BiomeRotationFeed:
    """The weekly Wild Mana biome rotation: current + upcoming."""
    return BiomeRotationFeed(**rotations.wild_mana())


@rotations_router.get("/stampy", response_model=BiomeRotationFeed)
async def get_stampy(ctx: AccessContext = _ROT) -> BiomeRotationFeed:
    """The weekly Stampy event biome (48-hour window): current + upcoming."""
    return BiomeRotationFeed(**rotations.stampy())


# --- Feeds (scope: feeds:read) ---------------------------------------------

def _news_item(d: TroveNews) -> TroveNewsItem:
    return TroveNewsItem(
        title=d.title, url=d.url, author=d.author, summary=d.summary,
        category=d.category, categories=d.categories, image=d.image,
        published_at=d.published_at,
    )


@feeds_router.get("/news", response_model=TroveNewsList)
async def list_news(
    ctx: AccessContext = _FEED,
    limit: int = Query(default=20, ge=1, le=50),
) -> TroveNewsList:
    """Latest Trove news, relayed from trovegame.com and cached server-side, newest
    first. Small + live; the full archive is at /v1/misc/news-history."""
    docs = await news.latest_news(limit)
    items = [_news_item(d) for d in docs]
    return TroveNewsList(items=items, count=len(items))


@feeds_router.get("/twitch", response_model=TwitchStreams)
async def get_twitch(ctx: AccessContext = _FEED) -> TwitchStreams:
    """Live Twitch streams for Trove (Helix, fetched at source) + cached."""
    items, fetched_at = await feeds.get_feed("twitch")
    return TwitchStreams(
        items=[TwitchStream(**i) for i in items], count=len(items), fetched_at=fetched_at
    )


@feeds_router.get("/youtube", response_model=Videos)
async def get_youtube(ctx: AccessContext = _FEED) -> Videos:
    """Recent Trove YouTube videos (Data API, fetched at source) + cached."""
    items, fetched_at = await feeds.get_feed("youtube")
    return Videos(items=[Video(**i) for i in items], count=len(items), fetched_at=fetched_at)


@feeds_router.get("/bilibili", response_model=Videos)
async def get_bilibili(ctx: AccessContext = _FEED) -> Videos:
    """Recent Trove Bilibili videos (search-page scrape, fetched at source) + cached."""
    items, fetched_at = await feeds.get_feed("bilibili")
    return Videos(items=[Video(**i) for i in items], count=len(items), fetched_at=fetched_at)


@feeds_router.get("/bilibili/image")
async def proxy_bilibili_image(
    url: str = Query(..., description="Absolute https hdslb.com thumbnail URL to proxy"),
    ctx: AccessContext = _FEED_IMG,
) -> Response:
    """Proxy a Bilibili thumbnail so browsers and WebViews can display it.

    Bilibili's CDN blocks hotlinking unless a bilibili.com Referer is sent, which
    an <img> tag can't do - clients point <img src> here and we refetch with the
    Referer. Cross-origin <img> loads aren't subject to CORS, so this serves the
    hosted web build and the Android WebView identically (and replaces the local
    proxy the desktop/web_server builds carry).
    """
    try:
        content, content_type = await feeds.fetch_bilibili_image(url)
    except ValueError as exc:
        raise APIError(status_code=400, code=ErrorCode.bad_request, message=str(exc))
    except httpx.HTTPError as exc:
        raise APIError(status_code=502, code=ErrorCode.internal_error,
                       message=f"upstream thumbnail fetch failed: {exc}")
    return Response(
        content=content,
        media_type=content_type,
        headers={"Cache-Control": "public, max-age=86400"},
    )


# --- Events: Trovesaurus calendar, relayed + stored (scope: feeds:read) -----
# Categories are free-form and discovered dynamically. Times are unix seconds
# (real UTC); status is derived by comparing now to start/end.


def _event_item(ev: TroveEvent, now: int) -> TroveEventItem:
    if now < ev.starts_at:
        status, seconds_until = "upcoming", ev.starts_at - now
    elif now < ev.ends_at:
        status, seconds_until = "ongoing", ev.ends_at - now
    else:
        status, seconds_until = "ended", 0
    return TroveEventItem(
        event_id=ev.event_id, name=ev.name, url=ev.url, category=ev.category,
        image=ev.image, icon=ev.icon, lookup=ev.lookup,
        starts_at=ev.starts_at, ends_at=ev.ends_at, status=status, seconds_until=seconds_until,
    )


@feeds_router.get("/events", response_model=TroveEventList)
async def list_ongoing_events(
    ctx: AccessContext = _FEED,
    category: str | None = Query(default=None, description="Filter by category (see /events/categories)"),
    limit: int = Query(default=100, ge=1, le=500),
) -> TroveEventList:
    """Events happening right now (started, not yet ended), ending soonest first."""
    now = int(utcnow().timestamp())
    query: dict = {"starts_at": {"$lte": now}, "ends_at": {"$gt": now}}
    if category:
        query["category"] = category
    docs = await TroveEvent.find(query).sort("ends_at").limit(limit).to_list()
    items = [_event_item(e, now) for e in docs]
    return TroveEventList(items=items, count=len(items))


@feeds_router.get("/events/categories", response_model=EventCategoryList)
async def list_event_categories(ctx: AccessContext = _FEED) -> EventCategoryList:
    """The distinct event categories currently stored - discovered dynamically, sorted."""
    rows = await TroveEvent.aggregate(
        [{"$group": {"_id": "$category"}}, {"$sort": {"_id": 1}}]
    ).to_list()
    categories = [r["_id"] for r in rows if r.get("_id")]
    return EventCategoryList(categories=categories, count=len(categories))


@feeds_router.get("/events/upcoming", response_model=TroveEventList)
async def list_upcoming_events(
    ctx: AccessContext = _FEED,
    category: str | None = Query(default=None),
    limit: int = Query(default=100, ge=1, le=500),
) -> TroveEventList:
    """Events that haven't started yet, starting soonest first."""
    now = int(utcnow().timestamp())
    query: dict = {"starts_at": {"$gt": now}}
    if category:
        query["category"] = category
    docs = await TroveEvent.find(query).sort("starts_at").limit(limit).to_list()
    items = [_event_item(e, now) for e in docs]
    return TroveEventList(items=items, count=len(items))


@feeds_router.get("/events/history", response_model=TroveEventList)
async def list_event_history(
    ctx: AccessContext = _FEED,
    category: str | None = Query(default=None),
    limit: int = Query(default=100, ge=1, le=500),
) -> TroveEventList:
    """Events that have already ended, most recently ended first."""
    now = int(utcnow().timestamp())
    query: dict = {"ends_at": {"$lte": now}}
    if category:
        query["category"] = category
    docs = await TroveEvent.find(query).sort("-ends_at").limit(limit).to_list()
    items = [_event_item(e, now) for e in docs]
    return TroveEventList(items=items, count=len(items))


# --- Stats: raw game data (scope: stats:read) ------------------------------
# Pure data, no calculation - what each source/field is and how much it gives.


@stats_router.get("/power-rank", response_model=StatTable)
async def get_power_rank_stats(ctx: TokenContext = _STAT) -> StatTable:
    """Power Rank stat table: each source and the PR it contributes."""
    return StatTable(**stats.stat_table("power-rank"))  # type: ignore[arg-type]


@stats_router.get("/magic-find", response_model=StatTable)
async def get_magic_find_stats(ctx: TokenContext = _STAT) -> StatTable:
    """Magic Find stat table: each source and the MF it contributes."""
    return StatTable(**stats.stat_table("magic-find"))  # type: ignore[arg-type]


@stats_router.get("/light", response_model=StatTable)
async def get_light_stats(ctx: TokenContext = _STAT) -> StatTable:
    """Light stat table: each source and the Light it contributes."""
    return StatTable(**stats.stat_table("light"))  # type: ignore[arg-type]


@stats_router.get("/classes", response_model=ClassList)
async def list_classes(ctx: TokenContext = _STAT) -> ClassList:
    """Every Trove class as a full object, each keyed by its `tech_name` token."""
    return ClassList(**stats.all_classes())


@stats_router.get("/classes/{tech_name}", response_model=TroveClass)
async def get_class(tech_name: str, ctx: TokenContext = _STAT) -> TroveClass:
    """A single class by its `tech_name` token (e.g. `knight`, `adventurer`)."""
    cls = stats.class_by_tech_name(tech_name)
    if cls is None:
        raise APIError(
            status_code=404,
            code=ErrorCode.not_found,
            message=f"No class with tech_name '{tech_name}'",
        )
    return TroveClass(**cls)


# --- Gems: simulator + evaluator + builds (scope: gems:read) ----------------
# Stateless compute - gem objects round-trip through the client; nothing stored.


def _bad_request(message: str) -> APIError:
    return APIError(status_code=400, code=ErrorCode.bad_request, message=message)


@gems_router.get("/lookups", response_model=GemLookups)
async def get_gem_lookups(ctx: TokenContext = _GEM) -> GemLookups:
    """Valid field values for gems: tiers, types, elements, stats, augments, abilities."""
    return GemLookups(**gem_lookups())


@gems_router.post("/generate", response_model=Gem)
async def generate_gem(req: GenerateGemRequest, ctx: TokenContext = _GEM) -> Gem:
    """Roll a new gem. Any omitted field (tier/type/element/restriction) is random."""
    try:
        return Gem.create(
            tier=req.tier, type=req.type, element=req.element, restriction=req.restriction,
            augmentation=req.augmentation, level=req.level,
        )
    except (ValueError, KeyError) as e:
        raise _bad_request(f"Invalid gem parameters: {e}") from e


@gems_router.post("/augment", response_model=GemActionResult)
async def augment_gem(req: AugmentRequest, ctx: TokenContext = _GEM) -> GemActionResult:
    """Apply a focus augment (1 Rough / 2 Precise / 3 Superior) to the stat at `stat_position`."""
    try:
        applied = req.gem.augment_stat(req.stat_position, req.augment_type)
    except (IndexError, ValueError) as e:
        raise _bad_request(str(e)) from e
    return GemActionResult(applied=applied, gem=req.gem)


@gems_router.post("/spark", response_model=GemActionResult)
async def spark_gem(req: StatPositionRequest, ctx: TokenContext = _GEM) -> GemActionResult:
    """Reroll the stat type at `stat_position` to another valid, unused type (no-op if locked)."""
    try:
        applied = req.gem.spark_stat(req.stat_position)
    except IndexError as e:
        raise _bad_request(str(e)) from e
    return GemActionResult(applied=applied, gem=req.gem)


@gems_router.post("/flare", response_model=GemActionResult)
async def flare_gem(req: StatPositionRequest, ctx: TokenContext = _GEM) -> GemActionResult:
    """Move one extra container off the stat at `stat_position` to a random other stat."""
    try:
        applied = req.gem.flare_stat(req.stat_position)
    except IndexError as e:
        raise _bad_request(str(e)) from e
    return GemActionResult(applied=applied, gem=req.gem)


@gems_router.post("/level-up", response_model=GemActionResult)
async def level_up_gem(req: LevelUpRequest, ctx: TokenContext = _GEM) -> GemActionResult:
    """Raise the gem's level by one (adds a container at levels 5/10/15). No-op at max level."""
    applied = req.gem.level_up()
    return GemActionResult(applied=applied, gem=req.gem)


@gems_router.post("/set-level", response_model=GemActionResult)
async def set_gem_level(req: SetLevelRequest, ctx: TokenContext = _GEM) -> GemActionResult:
    """Set the gem to an exact level, adding/removing containers to match."""
    applied = req.gem.set_level(req.level)
    return GemActionResult(applied=applied, gem=req.gem)


@gems_router.post("/evaluate", response_model=EvaluationResult)
async def evaluate_gem(req: EvaluateRequest, ctx: TokenContext = _GEM) -> EvaluationResult:
    """Score a typed-in gem (quality %, Power Rank, per-stat progress, cost to perfect)."""
    try:
        out = gem_evaluator.evaluate_gem(
            req.tier, req.type, req.level,
            [s.model_dump() for s in req.stats], req.auto_guess_procs,
        )
    except gem_evaluator.GemEvaluatorError as e:
        raise _bad_request(str(e)) from e
    except (ValueError, KeyError, ZeroDivisionError) as e:
        raise _bad_request(f"Could not evaluate gem: {e}") from e
    return EvaluationResult(
        **out["result"],
        available_extra_containers=out["available_extra_containers"],
        guessed_distribution=out["guessed_distribution"],
    )


@gems_router.get("/stat-range", response_model=GemStatRange)
async def get_gem_stat_range(
    tier: int, type: int, stat_type: int,
    level: int = Query(default=1, ge=1),
    extra_containers: int = Query(default=0, ge=0),
    element: int | None = None,
    ctx: TokenContext = _GEM,
) -> GemStatRange:
    """The plausible (min, max) value a stat can roll at - for inline input hints."""
    try:
        return GemStatRange(**gem_evaluator.gem_stat_range(tier, type, stat_type, level, extra_containers, element))
    except (ValueError, KeyError) as e:
        raise _bad_request(f"Invalid stat-range parameters: {e}") from e


@gems_router.get("/builds/options", response_model=BuildOptions)
async def get_build_options(ctx: TokenContext = _GEM) -> BuildOptions:
    """Valid field values for a build config (classes, allies, foods, flags, …)."""
    return BuildOptions(**gem_builds.build_options())


@gems_router.post("/builds/calculate", response_model=BuildResponse)
async def calculate_builds(req: BuildConfigRequest, ctx: TokenContext = _GEM) -> BuildResponse:
    """Top-200 gem proc layouts for a build, ranked by damage coefficient."""
    try:
        results = gem_builds.calculate_builds(req.model_dump())
    except gem_builds.BuildError as e:
        raise _bad_request(str(e)) from e
    return BuildResponse(results=[BuildResult(**r) for r in results], count=len(results))


# --- Misc: modding software + time converter + news archive (scope: misc:read) --


@misc_router.get("/news-history", response_model=TroveNewsHistory)
async def get_news_history(
    ctx: TokenContext = _MISC,
    limit: int = Query(default=50, ge=1, le=200),
    offset: int = Query(default=0, ge=0),
) -> TroveNewsHistory:
    """The full Trove news archive (never pruned), newest first, paginated. The
    small live view is /v1/feeds/news."""
    docs, total = await news.news_history(limit, offset)
    return TroveNewsHistory(items=[_news_item(d) for d in docs], count=len(docs), total=total)


@misc_router.get("/software", response_model=ModdingSoftware)
async def get_modding_software(ctx: TokenContext = _MISC) -> ModdingSoftware:
    """Third-party Trove modding software, grouped by category (blueprints, vfx, ui, sound, textures)."""
    return ModdingSoftware(**misc.modding_software())


@misc_router.get("/timezones", response_model=TimezoneList)
async def get_timezones(ctx: TokenContext = _MISC) -> TimezoneList:
    """The timezones supported by the converter and the clocks."""
    return TimezoneList(**misc.timezones())


@misc_router.get("/time/now", response_model=TimeNow)
async def get_time_now(ctx: TokenContext = _MISC) -> TimeNow:
    """Current time across every supported zone, including Trove server (reset) time."""
    return TimeNow(**misc.time_now())


@misc_router.post("/time/convert", response_model=TimeConvertResponse)
async def convert_time(req: TimeConvertRequest, ctx: TokenContext = _MISC) -> TimeConvertResponse:
    """Convert a wall-clock time in a timezone (or an absolute unix) to every zone + Discord codes."""
    try:
        out = misc.convert_time(req.datetime, req.timezone, req.unix)
    except misc.MiscError as e:
        raise _bad_request(str(e)) from e
    return TimeConvertResponse(**out)


@misc_router.get("/interest-items", response_model=MarketItemList)
async def get_interest_items(ctx: AccessContext = _MISC_PUBLIC) -> MarketItemList:
    """The full allow-list of item names the market scraper bot tracks.

    Tokenless: anyone (the bot, dashboards, wikis) can read this list. Managed
    via the master admin panel (/admin/market/interest-items). Sorted by name.
    """
    items = await market_service.interest_items_list()
    return MarketItemList(items=items, count=len(items))


# ── Player activity (scope: activity:read) ────────────────────────────────
# Free/public active-player estimates derived from the leaderboard captures,
# in their OWN `activity:read` scope (tokenless; a token carrying the scope
# just earns the wider per-token budget). The showcase /activity page is the
# main consumer via the same-origin /site proxies, but these are documented,
# stable endpoints for third-party dashboards / wikis too. This is the single
# public home - the old /v1/leaderboards/activity* and /v1/misc/activity*
# copies were folded into here.

@activity_router.get(
    "/current", response_model=ActivityResponse,
    summary="Estimated active players via leaderboard score deltas",
)
async def get_activity_current(
    response: Response,
    ctx: AccessContext = _ACTIVITY_PUBLIC,
) -> ActivityResponse:
    """**Tokenless.** Lower-bound count of active players in the most recent
    capture window - distinct top-N leaderboard players whose score increased
    on at least one lifetime board (or who appear in the new cycle of a
    daily/weekly board where a reset crossed the window) between the two
    latest captures. Also carries distinct 24h / 7d rollups (`estimate_24h` /
    `estimate_7d`). `estimate` is null until two captures exist. Cached for
    the cheater-detection TTL; a new hourly capture invalidates it."""
    from app.admin import runtime_config
    payload = await leaderboards_activity.estimate_active_players()
    ttl = int(await runtime_config.get_setting("cheaters_cache_ttl_seconds"))
    response.headers["Cache-Control"] = f"public, max-age={ttl}"
    return ActivityResponse(**payload)


@activity_router.get(
    "/history", response_model=ActivityHistoryResponse,
    summary="Time-series of estimated active players over recent captures",
)
async def get_activity_history(
    response: Response,
    days: int = Query(default=7, ge=1, le=30),
    ctx: AccessContext = _ACTIVITY_PUBLIC,
) -> ActivityHistoryResponse:
    """**Tokenless.** Time-series of active-player estimates over the last
    ``days`` days, one point per consecutive capture pair. ``estimate_per_hour``
    (= estimate / duration_hours) is the value a chart should plot - a missed
    capture makes the next window span 2-3h and inflates the raw count because
    more players had time to score; the per-hour rate normalises that out.
    Points persist on each ingest, so the series survives restarts."""
    from app.admin import runtime_config
    payload = await leaderboards_activity.estimate_active_players_history(days=days)
    ttl = int(await runtime_config.get_setting("cheaters_cache_ttl_seconds"))
    response.headers["Cache-Control"] = f"public, max-age={ttl}"
    return ActivityHistoryResponse(**payload)


@activity_router.get(
    "/series", response_model=ActivitySeriesResponse,
    summary="Bucketed active-player series for a period (1d … all)",
)
async def get_activity_series(
    response: Response,
    period: str = Query(default="7d", description="1d / 7d / 1m / 3m / 6m / 1y / all"),
    ctx: AccessContext = _ACTIVITY_PUBLIC,
) -> ActivitySeriesResponse:
    """**Tokenless.** Downsampled activity-level series for one period - the
    data behind the Player Activity page's charts. Each bucket is the average
    active-players-per-hour over the captures it spans (hourly for 1d up to
    weekly for 1y/all, sized so the line stays readable); also returns the
    period peak / average / latest level for stat cards."""
    from app.admin import runtime_config
    payload = await leaderboards_activity.activity_series(period=period)
    ttl = int(await runtime_config.get_setting("cheaters_cache_ttl_seconds"))
    response.headers["Cache-Control"] = f"public, max-age={ttl}"
    return ActivitySeriesResponse(**payload)


@activity_router.post(
    "/backfill", status_code=202,
    summary="Backfill the activity history from the archive (master)",
)
async def backfill_activity_history(
    background_tasks: BackgroundTasks,
    total_days: int = Query(
        default=400, ge=0, le=1000,
        description="How far back to backfill (capped by how far the data reaches). 0 = ALL stored history.",
    ),
    chunk_days: int = Query(
        default=14, ge=1, le=60,
        description="Slice size - smaller keeps peak memory lower on huge ranges.",
    ),
    force: bool = Query(
        default=False,
        description="Recompute window_ends already stored (default skips them).",
    ),
    reset: bool = Query(
        default=False,
        description=("DESTRUCTIVE: wipe the whole activity_estimate table "
                     "first, then recompute from scratch (implies force). Use to "
                     "discard values from earlier miscalculated runs."),
    ),
    _auth=_LB_MASTER,
) -> dict:
    """**Master only.** Seed the activity history (the `/series` data) from the
    stored Postgres captures so the multi-period charts have history
    predating the hourly forward-fill. Accepted **202** immediately and run in
    the BACKGROUND, walking the range newest-first in memory-bounded
    ``chunk_days`` slices (a year of hourly anchors loaded at once would blow
    the container's RAM cap). Coverage is capped by how far the archive
    reaches; with ``force=false`` already-stored windows are skipped.

    ``reset=true`` is **destructive**: it first DELETES every stored estimate
    and recomputes the whole range from scratch (implies ``force``) - the right
    move to flush miscalculations from older runs. The ``activity_estimate`` table
    is fully derived data, so nothing irreplaceable is lost; the multi-period
    charts just read empty until the rebuild lands. Progress + the final tally
    (including ``reset_deleted``) land in the api logs."""
    background_tasks.add_task(
        leaderboards_activity.backfill_history_chunked,
        total_days=total_days, chunk_days=chunk_days, force=force, reset=reset,
    )
    return {
        "accepted": True,
        "total_days": total_days,
        "chunk_days": chunk_days,
        "force": force or reset,
        "reset": reset,
        "message": (("RESET + backfill" if reset else "Backfill") +
                    " started in the background; watch the api logs for "
                    "'activity backfill (chunked) done' with the tally."),
    }


# --- Class activity (per-class active players from Effort/Paragon boards) ----


@class_activity_router.get(
    "/current", response_model=ClassActivityCurrentResponse,
    summary="Per-class player counts + player-share from the latest snapshot",
)
async def get_class_activity_current(
    response: Response,
    ctx: AccessContext = _ACTIVITY_PUBLIC,
) -> ClassActivityCurrentResponse:
    """**Tokenless.** A direct headcount from the most recent leaderboard snapshot
    (NOT the activity pipeline): the players present on each Trove class's Effort
    board (Paragon is excluded as ambiguous), plus each class's `share` of that
    total. The clean fields apply the established floors (Power Rank + Effort).
    `share` sums to 1 across classes but counts a multi-class player in each, so
    it's share-of-players, not distinct players. (The time-series endpoint stays
    activity-based - score rose between captures.)"""
    from app.admin import runtime_config
    payload = await leaderboards_class_activity.class_activity_current()
    ttl = int(await runtime_config.get_setting("cheaters_cache_ttl_seconds"))
    response.headers["Cache-Control"] = f"public, max-age={ttl}"
    return ClassActivityCurrentResponse(**payload)


@class_activity_router.get(
    "/series", response_model=ClassActivitySeriesResponse,
    summary="Bucketed per-class active-player series for a period (1d … all)",
)
async def get_class_activity_series(
    response: Response,
    period: str = Query(default="7d", description="1d / 7d / 1m / 3m / 6m / 1y / all"),
    ctx: AccessContext = _ACTIVITY_PUBLIC,
) -> ClassActivitySeriesResponse:
    """**Tokenless.** Downsampled per-class activity-level series for one period -
    the data behind the Class Activity page's multi-line chart. `buckets` is the
    shared time axis; each class line's `values` align to it (null where that
    class had no measurable window in a bucket - e.g. across the weekly reset)."""
    from app.admin import runtime_config
    payload = await leaderboards_class_activity.class_activity_series(period=period)
    ttl = int(await runtime_config.get_setting("cheaters_cache_ttl_seconds"))
    response.headers["Cache-Control"] = f"public, max-age={ttl}"
    return ClassActivitySeriesResponse(**payload)


@class_activity_router.post(
    "/backfill", status_code=202,
    summary="Backfill the per-class activity history (master)",
)
async def backfill_class_activity_history(
    background_tasks: BackgroundTasks,
    total_days: int = Query(
        default=400, ge=0, le=1000,
        description="How far back to backfill (capped by stored data). 0 = ALL stored history.",
    ),
    force: bool = Query(
        default=False, description="Recompute window_ends already stored (default skips them).",
    ),
    reset: bool = Query(
        default=False,
        description=("DESTRUCTIVE: wipe the whole class_activity_estimate table "
                     "first, then recompute from scratch (implies force)."),
    ),
    _auth=_LB_MASTER,
) -> dict:
    """**Master only.** Seed the per-class activity history from the stored
    Postgres captures so the Class Activity charts have history. Accepted **202**
    immediately and run in the BACKGROUND (memory-safe streaming; only the 36
    Effort/Paragon boards load per anchor). `reset=true` wipes first."""
    background_tasks.add_task(
        leaderboards_class_activity.backfill_class_history_chunked,
        total_days=total_days, force=force, reset=reset,
    )
    return {
        "accepted": True,
        "total_days": total_days,
        "force": force or reset,
        "reset": reset,
        "message": (("RESET + backfill" if reset else "Backfill") +
                    " started in the background; watch the api logs for "
                    "'class activity backfill (chunked) done' with the tally."),
    }


@misc_router.get(
    "/trove-status", response_model=TroveStatusResponse,
    summary="Live Trove server status (auth + per-env game sockets)",
)
async def misc_trove_status(
    response: Response,
    ctx: AccessContext = _MISC_PUBLIC,
) -> TroveStatusResponse:
    """**Tokenless.** Live Trove server status from the background prober
    (every ~60s). Status is binary: ``online`` / ``down`` (+ ``unknown``
    before the first probe). ``overall`` is online only when every LIVE
    region is online, else down. ``auth`` is the HTTPS liveness of
    ``auth.trionworlds.com``; ``environments.{eu,us,pts}`` each carry a
    ``game`` probe of the glsserver port (6560) - reachable = online,
    unreachable = down. Served from cache; never blocks on a live probe."""
    from app.trove import status as trove_status
    payload = trove_status.get_status()
    response.headers["Cache-Control"] = "public, max-age=30"
    return TroveStatusResponse(**payload)


@misc_router.get(
    "/trove-status/history", response_model=TroveStatusHistoryResponse,
    summary="Trove status timeline + uptime for one environment",
)
async def misc_trove_status_history(
    response: Response,
    env: str = Query(default="live", pattern="^(live|pts)$"),
    days: int = Query(default=30, ge=1, le=90),
    ctx: AccessContext = _MISC_PUBLIC,
) -> TroveStatusHistoryResponse:
    """**Tokenless.** Status-timeline history for ``env`` (live / pts)
    over the last ``days`` days: ``segments`` (continuous status periods,
    open one has ``ended_at=null``), ``outages`` (the non-online subset),
    and an ``uptime`` fraction. Drives the downtime-history graphic on
    the /status page."""
    from app.trove import status as trove_status
    payload = await trove_status.get_history(env, days)
    response.headers["Cache-Control"] = "public, max-age=60"
    return TroveStatusHistoryResponse(**payload)


# ── Feedback ingest ───────────────────────────────────────────────────────
# Public, tokenless, rate-limited. Two stacked buckets + per-attachment
# size/count limits. All caps + the Discord webhook URL are runtime-tunable
# from the master admin panel (see app/admin/runtime_config.py).
#
# Wire format is multipart/form-data so up to 4 image attachments can ride
# along. The bytes are streamed straight to Discord (not persisted on our
# disk or Mongo); we keep only the metadata (filename + type + size) in the
# entry. On webhook failure we still have the message + metadata.


_FEEDBACK_MAX_ATTACHMENTS = 4
_FEEDBACK_MAX_FILE_BYTES = 5 * 1024 * 1024  # 5 MB per file
_FEEDBACK_ALLOWED_MIME = {
    "image/png", "image/jpeg", "image/webp", "image/gif",
}


def _sanitise_filename(name: str | None) -> str:
    """Strip anything that could mean a path. Discord ignores paths
    anyway, but Mongo will see this string in the admin queue and we
    don't want raw user input to ever look like a path or shell glob."""
    if not name:
        return "image"
    base = re.sub(r"[^\w.\-]", "_", name.rsplit("/", 1)[-1].rsplit("\\", 1)[-1])
    return base[:80] or "image"


async def _post_feedback_webhook(
    *,
    doc_id: str,
    category: str,
    message: str,
    contact: str | None,
    app_version: str | None,
    os_label: str | None,
    client_label: str | None,
    files: list[tuple[str, bytes, str]],
) -> None:
    """Send one feedback entry to the configured Discord webhook.

    Reads the URL from runtime config on every call (cheap - cached for 5s
    by ``runtime_config.get_setting``). Empty URL → no-op. Any HTTP error,
    timeout, or DNS failure is logged and swallowed; we never want webhook
    flakiness to cascade into 5xx on the public POST.

    Attachments are rendered INSIDE the embed (not as separate file blocks
    underneath). Discord groups multiple embeds with the same ``url`` field
    into a single visual card, so we use one anchor URL (the feedback id)
    across N embeds, each carrying one image. The first embed also holds
    the title / description / fields; the rest are image-only siblings.
    Files are renamed to ``<index>_<sanitised>`` in the multipart so the
    ``attachment://`` reference is unambiguous even when the user uploaded
    two files with the same original name."""
    from app.admin import runtime_config

    url = (await runtime_config.get_setting("feedback.discord_webhook")) or ""
    if not url.strip():
        return  # webhook disabled

    # Truncate the message for Discord (their hard cap on embed description
    # is 4096 chars; we go shorter so it stays readable). Keep the full
    # original in Mongo regardless.
    msg_display = message if len(message) <= 1500 else (message[:1500] + " …")
    color = {"bug": 0xf85149, "feature": 0xd29922, "general": 0x58a6ff}.get(
        category, 0x58a6ff,
    )

    # Inline fields fit two per row on Discord's mobile layout; we send
    # OS / Client / App version / Contact as inlines so they don't waste
    # vertical space and the user sees them at a glance.
    fields: list[dict] = []
    if os_label:
        fields.append({"name": "OS", "value": os_label, "inline": True})
    if client_label:
        fields.append({"name": "Client", "value": client_label, "inline": True})
    if app_version:
        fields.append({"name": "App version", "value": app_version[:256], "inline": True})
    if contact:
        fields.append({"name": "Contact", "value": contact[:1024], "inline": False})

    # Rename incoming files with a numeric prefix so the attachment://
    # references can never collide (two screenshots called "image.png"
    # would otherwise both resolve to the same one in the embed).
    relabeled = [
        (f"{i}_{fname}", content, ctype)
        for i, (fname, content, ctype) in enumerate(files)
    ]

    # Gallery URL - arbitrary but must be identical across every embed
    # in the message and must look like a real URL. Discord groups embeds
    # that share a `url` into one card; that's how we get N images
    # inside ONE embed instead of N stacked embeds. The URL doesn't need
    # to resolve - it's just the grouping key.
    gallery_url = f"https://api.aallyn.net/feedback/{doc_id}"

    main_embed: dict = {
        "title": f"New {category} feedback",
        "description": msg_display,
        "color": color,
        "fields": fields,
        "footer": {"text": f"id {doc_id}"},
        "timestamp": utcnow().isoformat(),
    }
    if relabeled:
        main_embed["url"] = gallery_url
        main_embed["image"] = {"url": f"attachment://{relabeled[0][0]}"}

    embeds = [main_embed]
    # Additional image-only sibling embeds - each just an image with the
    # same `url`, no title/description (Discord renders them as extra
    # images in the SAME card as the main embed).
    for fname, _content, _ctype in relabeled[1:]:
        embeds.append({
            "url": gallery_url,
            "image": {"url": f"attachment://{fname}"},
        })

    payload = {"embeds": embeds}

    try:
        async with httpx.AsyncClient(timeout=20) as client:
            if relabeled:
                # Multipart with payload_json + the files keyed as
                # files[N]. The (filename, content, content_type) tuple
                # must exactly match what we wrote into the `attachment://`
                # references above - that's why we used the relabeled
                # names everywhere.
                multipart = {
                    f"files[{i}]": (fname, content, ctype)
                    for i, (fname, content, ctype) in enumerate(relabeled)
                }
                r = await client.post(
                    url,
                    data={"payload_json": json.dumps(payload)},
                    files=multipart,
                )
            else:
                r = await client.post(url, json=payload)
            r.raise_for_status()
    except Exception as e:  # noqa: BLE001 - webhook is best-effort
        logger.warning("feedback webhook POST failed: %s", e)


@misc_router.post("/feedback", response_model=FeedbackAck,
                  summary="Submit feedback")
async def post_feedback(
    request: Request,
    response: Response,
    background: BackgroundTasks,
    message: str = Form(..., min_length=5, max_length=2000),
    contact: str | None = Form(default=None, max_length=200),
    category: str = Form(default="general"),
    app_version: str | None = Form(default=None, max_length=64),
    attachments: list[UploadFile] = File(default=[]),
) -> FeedbackAck:
    """Submit a piece of feedback (bug report / feature idea / general note).

    **Tokenless** - anyone can submit. Wire format is ``multipart/form-data``
    so attachments can ride along; a JSON-only client just omits the file
    fields and works the same.

    **Rate limits** (runtime-tunable from the admin panel):
      - ``feedback.per_ip_max`` / ``feedback.per_ip_window_seconds`` -
        burst from one source. Hit returns 429 with ``X-RateLimit-*`` +
        ``Retry-After``.
      - ``feedback.global_max`` / ``feedback.global_window_seconds`` -
        silent global backstop.

    **Body fields**:
      - ``message`` (required, 5-2000 chars)
      - ``contact`` (optional, ≤200 chars) - reply channel, free text
      - ``category`` (``bug`` / ``feature`` / ``general``; default ``general``)
      - ``app_version`` (optional, ≤64 chars) - for desktop / 3rd-party
        clients; rendered alongside parsed-from-UA OS + client name
      - ``attachments`` (optional, up to 4 images; max 5 MB each;
        ``image/png|jpeg|webp|gif``)

    **Privacy**: no IP is persisted (only used as the rate-limit key).
    The User-Agent is kept raw as a forensic fallback but the human-
    readable OS + client name (parsed at submit time) are what surface
    on the Discord webhook embed.

    **Webhook**: on success, the entry + attachments are POSTed to
    ``feedback.discord_webhook`` (if set) as a fire-and-forget
    BackgroundTask. Webhook failures are logged but never fail the
    submission.
    """
    from app.admin import runtime_config

    # ── 1. Validate category (Form() doesn't accept Literal directly) ──
    if category not in {"bug", "feature", "general"}:
        raise _bad_request(
            "category must be one of 'bug', 'feature', 'general'."
        )

    # ── 2. Rate-limit buckets ──────────────────────────────────────────
    per_ip_max     = await runtime_config.get_setting("feedback.per_ip_max")
    per_ip_window  = await runtime_config.get_setting("feedback.per_ip_window_seconds")
    global_max     = await runtime_config.get_setting("feedback.global_max")
    global_window  = await runtime_config.get_setting("feedback.global_window_seconds")

    ip = client_ip(request) or "unknown"
    info = await check_rate_limit(f"feedback_ip:{ip}", per_ip_max, per_ip_window)
    response.headers.update(rate_limit_headers(info))
    await check_rate_limit("feedback_global", global_max, global_window)

    # ── 3. Validate attachments - count, MIME, size - and read bytes ──
    # We read every file into memory because (a) they're capped at 5 MB,
    # so worst case is 20 MB transient and (b) the Discord webhook expects
    # the full bytes in the multipart body anyway.
    if len(attachments) > _FEEDBACK_MAX_ATTACHMENTS:
        raise _bad_request(
            f"At most {_FEEDBACK_MAX_ATTACHMENTS} attachments allowed."
        )
    files_for_webhook: list[tuple[str, bytes, str]] = []
    attachment_metadata: list[dict] = []
    for f in attachments:
        ctype = (f.content_type or "").lower()
        if ctype not in _FEEDBACK_ALLOWED_MIME:
            raise _bad_request(
                f"Attachment '{f.filename}' has unsupported type "
                f"'{ctype or '?'}'. Allowed: PNG, JPEG, WebP, GIF."
            )
        content = await f.read()
        if len(content) == 0:
            continue  # browsers occasionally send empty file slots
        if len(content) > _FEEDBACK_MAX_FILE_BYTES:
            raise _bad_request(
                f"Attachment '{f.filename}' exceeds 5 MB."
            )
        safe_name = _sanitise_filename(f.filename)
        files_for_webhook.append((safe_name, content, ctype))
        attachment_metadata.append({
            "filename": safe_name,
            "content_type": ctype,
            "size": len(content),
        })

    # ── 4. Persist ─────────────────────────────────────────────────────
    ua = request.headers.get("user-agent")
    try:
        doc = await misc.insert_feedback(
            message=message,
            contact=contact,
            category=category,
            app_version=app_version,
            user_agent=ua,
            attachments=attachment_metadata,
        )
    except misc.MiscError as e:
        raise _bad_request(str(e)) from e

    # ── 5. Webhook (fire-and-forget) ───────────────────────────────────
    background.add_task(
        _post_feedback_webhook,
        doc_id=str(doc.id),
        category=doc.category,
        message=doc.message,
        contact=doc.contact,
        app_version=doc.app_version,
        os_label=doc.os,
        client_label=doc.client,
        files=files_for_webhook,
    )

    return FeedbackAck(ok=True, received_at=doc.created_at)


# --- Mods: .tmod decompile + build (scope: mods:read) -----------------------
# Stateless: read parses an uploaded tmod; build serializes one and returns the
# bytes, then discards it. /v1/mods/* gets a 20 MB body cap (security middleware).


def _safe_filename(name: str) -> str:
    cleaned = re.sub(r"[^A-Za-z0-9 ._-]", "", name).strip() or "mod"
    return cleaned[:80]


@mods_router.post("/read", response_model=TmodReadResponse)
async def read_tmod(
    body: bytes = Body(..., media_type="application/octet-stream"),
    metadata_only: bool = Query(
        default=False,
        description="If true, return only paths/hashes/sizes + header data (no file contents).",
    ),
    ctx: TokenContext = _MODS,
) -> TmodReadResponse:
    """Decompile a .tmod (POST the raw file bytes). Returns header properties + file table."""
    if not body:
        raise _bad_request("Empty body - POST the raw .tmod file bytes.")
    try:
        result = tmod.read_tmod(body, metadata_only=metadata_only)
    except tmod.TmodError as e:
        raise _bad_request(str(e)) from e
    return TmodReadResponse(**result)


@mods_router.post(
    "/build",
    responses={200: {"content": {"application/octet-stream": {}}, "description": "The built .tmod file"}},
)
async def build_tmod(req: TmodBuildRequest, ctx: TokenContext = _MODS) -> Response:
    """Build a .tmod from header fields + files (base64) and return the raw bytes.

    `modLoader` is always stamped `KiwiAPI`. Nothing is stored - the file is built
    in memory and discarded once returned.
    """
    files: list[tuple[str, bytes]] = []
    for f in req.files:
        try:
            files.append((f.path, base64.b64decode(f.content_base64, validate=True)))
        except ValueError as e:  # binascii.Error subclasses ValueError
            raise _bad_request(f"Invalid base64 content for '{f.path}': {e}") from e
    try:
        data = tmod.build_tmod(req.version, req.properties, files)
    except tmod.TmodError as e:
        raise _bad_request(str(e)) from e
    filename = _safe_filename(req.properties.get("title", "mod")) + ".tmod"
    return Response(
        content=data,
        media_type="application/octet-stream",
        headers={"Content-Disposition": f'attachment; filename="{filename}"'},
    )


# --- Updates: browse the archived game files (scope: updates:read) -----------
# Latest version only for now. Trees list one level at a time; file downloads
# stream straight from the content-addressed blob store.


def _check_branch(branch: str) -> None:
    if branch not in UPDATE_BRANCHES:
        raise APIError(
            status_code=404, code=ErrorCode.not_found,
            message=f"Unknown branch '{branch}' (known: {', '.join(UPDATE_BRANCHES)})",
        )


@updates_router.get("/branches", response_model=BranchList)
async def list_update_branches(ctx: TokenContext = _UPD) -> BranchList:
    """The tracked branches (live-us / pts) with their current version and file count."""
    items = await updates_read.list_branches()
    return BranchList(items=[BranchInfo(**b) for b in items], count=len(items))


@updates_router.get("/{branch}/versions", response_model=VersionList)
async def list_update_versions(
    branch: str,
    ctx: TokenContext = _UPD,
    limit: int = Query(default=50, ge=1, le=200),
    offset: int = Query(default=0, ge=0),
) -> VersionList:
    """A branch's captured version history, newest first."""
    _check_branch(branch)
    docs, total = await updates_read.list_versions(branch, limit, offset)
    items = [
        VersionInfo(
            branch=d.branch, ordinal=d.ordinal, version_tag=d.version_tag,
            captured_at=d.captured_at, completed_at=d.completed_at,
            files_added=d.files_added, files_modified=d.files_modified,
            files_removed=d.files_removed, bytes_added=d.bytes_added,
        )
        for d in docs
    ]
    return VersionList(items=items, count=len(items), total=total)


_CHANGE_TYPES = ("added", "modified", "removed")


@updates_router.get("/{branch}/changes", response_model=ChangeList)
async def list_update_changes(
    branch: str,
    ctx: TokenContext = _UPD,
    ordinal: int | None = Query(default=None, description="Version ordinal; omit to use the latest"),
    version: str | None = Query(default=None, description="Version tag, e.g. TEST-103-3325-A-336166 (alternative to ordinal)"),
    type: str | None = Query(default=None, description=f"Filter to one change type: {', '.join(_CHANGE_TYPES)}"),
    limit: int = Query(default=100, ge=1, le=1000),
    offset: int = Query(default=0, ge=0),
) -> ChangeList:
    """The per-file changes a version introduced (added / modified / removed paths).

    Identifies the version by `version` tag, by `ordinal`, or - both omitted - the
    branch's latest. The change-log holds every version, so older versions work too.
    """
    _check_branch(branch)
    if type is not None and type not in _CHANGE_TYPES:
        raise APIError(
            status_code=400, code=ErrorCode.bad_request,
            message=f"Invalid type '{type}' (allowed: {', '.join(_CHANGE_TYPES)})",
        )
    ver = await updates_read.resolve_version(branch, ordinal, version)
    if ver is None:
        raise APIError(status_code=404, code=ErrorCode.not_found, message="No matching version for that branch")
    docs, total = await updates_read.list_changes(branch, ver.ordinal, type, limit, offset)
    return ChangeList(
        branch=branch, ordinal=ver.ordinal, version_tag=ver.version_tag,
        entries=[ChangeEntry(path=d.path, type=d.type, content_sha256=d.content_sha256, size=d.size) for d in docs],
        count=len(docs), total=total,
        files_added=ver.files_added, files_modified=ver.files_modified, files_removed=ver.files_removed,
    )


@updates_router.get("/{branch}/tree", response_model=TreeListing)
async def list_update_tree(
    branch: str,
    ctx: TokenContext = _UPD,
    prefix: str = Query(default="", description="Directory to list (e.g. 'prefabs/'); empty = root"),
) -> TreeListing:
    """Immediate children of a directory in the latest tree (ls-style, one level)."""
    _check_branch(branch)
    if prefix and not prefix.endswith("/"):
        prefix += "/"
    entries = await updates_read.list_directory(branch, prefix)
    return TreeListing(
        branch=branch, prefix=prefix,
        entries=[TreeEntry(**e) for e in entries], count=len(entries),
    )


@updates_router.get("/{branch}/file/meta", response_model=FileMeta)
async def get_update_file_meta(
    branch: str, path: str = Query(...), ctx: TokenContext = _UPD,
) -> FileMeta:
    """Metadata for a single file in the latest tree (hash, size, source archive)."""
    _check_branch(branch)
    meta = await updates_read.get_file_meta(branch, path)
    if meta is None:
        raise APIError(status_code=404, code=ErrorCode.not_found, message=f"No file '{path}'")
    return FileMeta(branch=branch, **meta)


@updates_router.get("/{branch}/file")
async def download_update_file(
    branch: str, path: str = Query(...), ctx: AccessContext = _UPD_PUBLIC,
) -> FileResponse:
    """Download a single file's bytes from the latest tree (streamed from the
    blob store). Tokenless (public) so files can be linked directly."""
    _check_branch(branch)
    meta = await updates_read.get_file_meta(branch, path)
    if meta is None:
        raise APIError(status_code=404, code=ErrorCode.not_found, message=f"No file '{path}'")
    blob = ContentStore(settings.trove_update_store_dir).path_for(meta["content_sha256"])
    if not blob.is_file():
        raise APIError(status_code=404, code=ErrorCode.not_found, message="Blob missing from the store")
    filename = path.rsplit("/", 1)[-1] or "file"
    return FileResponse(blob, media_type="application/octet-stream", filename=filename)


@updates_router.get("/{branch}/file/history", response_model=FileHistoryList)
async def get_update_file_history(
    branch: str,
    path: str = Query(..., description="Logical path (no leading slash)"),
    ctx: TokenContext = _UPD,
) -> FileHistoryList:
    """Every version that touched ``path`` on this branch, newest first.

    Drives the per-file timeline on the public ``/updates`` page. Each
    entry carries the change type, the resulting blob sha + size, and
    the version's ``captured_at`` (joined from ``UpdateVersion``) so the
    client doesn't have to issue one fetch per row.
    """
    _check_branch(branch)
    rows = await updates_read.file_history(branch, path)
    if not rows:
        # 404 - file never existed on this branch. Empty history would
        # otherwise be indistinguishable from a typo'd path.
        raise APIError(
            status_code=404, code=ErrorCode.not_found,
            message=f"No history for '{path}' on branch '{branch}'",
        )
    return FileHistoryList(
        branch=branch, path=path,
        items=[FileHistoryEntry(**r) for r in rows],
        count=len(rows),
    )


@updates_router.get("/{branch}/file/compare", response_model=FileCompareResponse)
async def compare_update_file(
    branch: str,
    path: str = Query(..., description="Logical path (no leading slash)"),
    from_: int = Query(..., alias="from", description="'from' ordinal"),
    to: int = Query(..., description="'to' ordinal"),
    ctx: TokenContext = _UPD,
) -> FileCompareResponse:
    """Compare two versions of one file. Returns a structured unified
    diff for text files (rendered server-side, capped at 1 MiB/side) or
    a metadata-only response for binaries / over-budget files.

    The "from" and "to" ordinals refer to the same UpdateVersion ordering
    used elsewhere in this module. They don't have to be adjacent - any
    two complete versions on the branch are valid. Either side may
    return ``None`` content_sha256 (the path didn't exist yet, or had
    already been removed at that ordinal); the diff still computes
    against an empty side."""
    _check_branch(branch)

    # Resolve both side's versions for the metadata header.
    v_from = await updates_read.resolve_version(branch, ordinal=from_)
    v_to = await updates_read.resolve_version(branch, ordinal=to)
    if v_from is None or v_to is None:
        raise APIError(
            status_code=404, code=ErrorCode.not_found,
            message="One of the requested ordinals doesn't exist on this branch",
        )

    # Resolve each side's blob coordinates (sha + size). Missing → empty.
    a = await updates_read.resolve_file_at_version(branch, path, from_)
    b = await updates_read.resolve_file_at_version(branch, path, to)

    a_info = FileVersionInfo(
        ordinal=v_from.ordinal, version_tag=v_from.version_tag,
        captured_at=v_from.captured_at,
        content_sha256=a["content_sha256"] if a else None,
        size=a["size"] if a else 0,
    )
    b_info = FileVersionInfo(
        ordinal=v_to.ordinal, version_tag=v_to.version_tag,
        captured_at=v_to.captured_at,
        content_sha256=b["content_sha256"] if b else None,
        size=b["size"] if b else 0,
    )

    # Trivial paths: identical or both-missing.
    if (a_info.content_sha256 or "") == (b_info.content_sha256 or "") and a_info.content_sha256 is not None:
        return FileCompareResponse(
            branch=branch, path=path,
            **{"from": a_info, "to": b_info},
            identical=True, is_text=True, hunks=[],
        )
    if a is None and b is None:
        return FileCompareResponse(
            branch=branch, path=path,
            **{"from": a_info, "to": b_info},
            identical=True, is_text=True, hunks=[],
            reason="file did not exist at either side",
        )

    # Fetch both blobs (None when the side didn't exist).
    store = ContentStore(settings.trove_update_store_dir)
    a_bytes = store.get(a["content_sha256"]) if a else None
    b_bytes = store.get(b["content_sha256"]) if b else None

    a_dec = updates_compare.decode_blob(a_bytes)
    b_dec = updates_compare.decode_blob(b_bytes)

    if not (a_dec.is_text and b_dec.is_text):
        reason = a_dec.reason or b_dec.reason or "binary"
        return FileCompareResponse(
            branch=branch, path=path,
            **{"from": a_info, "to": b_info},
            identical=False, is_text=False, reason=reason, hunks=[],
        )

    hunks_raw = updates_compare.make_hunks(a_dec.lines, b_dec.lines)
    hunks = [
        DiffHunk(
            left_start=h["left_start"], right_start=h["right_start"],
            lines=[DiffHunkLine(**ln) for ln in h["lines"]],
        )
        for h in hunks_raw
    ]
    return FileCompareResponse(
        branch=branch, path=path,
        **{"from": a_info, "to": b_info},
        identical=False, is_text=True, hunks=hunks,
    )


# --- Codexes: parsed game data from the archive (scope: codexes:read) --------
# Served from the materialized CodexEntry collection (no archive access on the
# hot path). Eight typed datasets; entries are addressed by their source path.


def _check_codex_type(codex_type: str) -> None:
    if codex_type not in CODEX_TYPES:
        raise APIError(
            status_code=404, code=ErrorCode.not_found,
            message=f"Unknown codex type '{codex_type}' (known: {', '.join(CODEX_TYPES)})",
        )


def _codex_out(d) -> CodexEntryOut:
    return CodexEntryOut(
        type=d.codex_type, path=d.path, name=d.name, category=d.category,
        description=d.description, tradable=d.tradable, mastery=d.mastery,
        blueprint=d.blueprint, data=d.data, indexed_at=d.indexed_at,
    )


_SORT_DESC = "Sort order: " + ", ".join(codexes_read.SORTS)


def _check_sort(sort: str) -> None:
    if sort not in codexes_read.SORTS:
        raise APIError(
            status_code=400, code=ErrorCode.bad_request,
            message=f"Invalid sort '{sort}' (allowed: {', '.join(codexes_read.SORTS)})",
        )


@codexes_router.get("/types", response_model=CodexTypeList)
async def list_codex_types(
    ctx: AccessContext = _CODEX,
    branch: str = Query(default=_DEFAULT_CODEX_BRANCH),
) -> CodexTypeList:
    """The codex types present for a branch, each with its entry count."""
    _check_branch(branch)
    rows = await codexes_read.type_counts(branch)
    return CodexTypeList(
        branch=branch, items=[CodexTypeInfo(**r) for r in rows], count=len(rows),
    )


@codexes_router.get("/search", response_model=CodexSearchPage)
async def search_codexes(
    ctx: AccessContext = _CODEX,
    branch: str = Query(default=_DEFAULT_CODEX_BRANCH),
    q: str | None = Query(default=None, description="Case-insensitive name/description substring"),
    type: str | None = Query(default=None, description="Restrict to one codex type"),
    category: str | None = Query(default=None, description="Exact category match"),
    tradable: bool | None = Query(default=None, description="Filter by tradability"),
    sort: str = Query(default="name", description=_SORT_DESC),
    limit: int = Query(default=50, ge=1, le=200),
    offset: int = Query(default=0, ge=0),
) -> CodexSearchPage:
    """Search/filter across ALL codex types at once (the unified search surface).
    Every filter is optional and ANDed; each result carries its own `type`."""
    _check_branch(branch)
    if type is not None:
        _check_codex_type(type)
    _check_sort(sort)
    docs, total = await codexes_read.query_entries(
        branch, codex_type=type, search=q, category=category, tradable=tradable,
        sort=sort, limit=limit, offset=offset,
    )
    return CodexSearchPage(
        branch=branch, type=type, query=q,
        items=[_codex_out(d) for d in docs], count=len(docs), total=total,
    )


@codexes_router.get("/{codex_type}", response_model=CodexEntryPage)
async def list_codex_entries(
    codex_type: str,
    ctx: AccessContext = _CODEX,
    branch: str = Query(default=_DEFAULT_CODEX_BRANCH),
    search: str | None = Query(default=None, description="Case-insensitive name/description substring"),
    category: str | None = Query(default=None, description="Exact category match (see /{type}/categories)"),
    tradable: bool | None = Query(default=None, description="Filter by tradability"),
    sort: str = Query(default="name", description=_SORT_DESC),
    limit: int = Query(default=50, ge=1, le=200),
    offset: int = Query(default=0, ge=0),
) -> CodexEntryPage:
    """Entries of one codex type - filterable (search/category/tradable), sortable, paged."""
    _check_branch(branch)
    _check_codex_type(codex_type)
    _check_sort(sort)
    docs, total = await codexes_read.query_entries(
        branch, codex_type=codex_type, search=search, category=category,
        tradable=tradable, sort=sort, limit=limit, offset=offset,
    )
    return CodexEntryPage(
        branch=branch, type=codex_type,
        items=[_codex_out(d) for d in docs], count=len(docs), total=total,
    )


@codexes_router.get("/{codex_type}/categories", response_model=CodexCategoryList)
async def list_codex_categories(
    codex_type: str,
    ctx: AccessContext = _CODEX,
    branch: str = Query(default=_DEFAULT_CODEX_BRANCH),
) -> CodexCategoryList:
    """Distinct categories (+ counts) within a type - for building filter dropdowns."""
    _check_branch(branch)
    _check_codex_type(codex_type)
    rows = await codexes_read.list_categories(branch, codex_type)
    return CodexCategoryList(
        branch=branch, type=codex_type,
        items=[CodexCategoryInfo(**r) for r in rows], count=len(rows),
    )


@codexes_router.get("/{codex_type}/entry", response_model=CodexEntryOut)
async def get_codex_entry(
    codex_type: str,
    path: str = Query(..., description="The entry's source prefab path (its stable id)"),
    ctx: AccessContext = _CODEX,
    branch: str = Query(default=_DEFAULT_CODEX_BRANCH),
) -> CodexEntryOut:
    """A single codex entry by its source prefab path."""
    _check_branch(branch)
    _check_codex_type(codex_type)
    doc = await codexes_read.get_entry(branch, codex_type, path)
    if doc is None:
        raise APIError(status_code=404, code=ErrorCode.not_found, message=f"No {codex_type} entry '{path}'")
    return _codex_out(doc)


# --- BTT: BetterTroveTools releases relay (scope: btt:read, public) ---------
# Drives the desktop app's update-check loop: the app polls these endpoints (no
# token required) and emits local push notifications when a newer version ships.


def _release_meta(d) -> BttReleaseMeta:
    return BttReleaseMeta(
        release_id=d.release_id, tag_name=d.tag_name, name=d.name, body=d.body,
        html_url=d.html_url, prerelease=d.prerelease,
        channel="beta" if d.prerelease else "release",
        published_at=d.published_at, fetched_at=d.fetched_at,
    )


def _release_info(d) -> BttReleaseInfo:
    return BttReleaseInfo(**_release_meta(d).model_dump(),
                          assets=[BttAsset(**a) for a in d.assets])


def _check_btt_channel(channel: str) -> None:
    if channel not in btt_releases.CHANNELS:
        raise APIError(
            status_code=400, code=ErrorCode.bad_request,
            message=f"channel must be one of: {', '.join(btt_releases.CHANNELS)}",
        )


@btt_router.get("/releases", response_model=BttReleaseList)
async def list_btt_releases(
    ctx: AccessContext = _BTT,
    channel: str | None = Query(default=None, description="release | beta (default: both)"),
    limit: int = Query(default=50, ge=1, le=200),
    offset: int = Query(default=0, ge=0),
) -> BttReleaseList:
    """Recent BetterTroveTools GitHub releases, newest first. Optional channel filter."""
    if channel is not None:
        _check_btt_channel(channel)
    docs, total = await btt_releases.list_releases(channel, limit, offset)
    return BttReleaseList(
        channel=channel, items=[_release_info(d) for d in docs],
        count=len(docs), total=total,
    )


@btt_router.get("/latest", response_model=BttLatestPerPlatform)
async def get_btt_latest(
    ctx: AccessContext = _BTT,
    channel: str = Query(default="release", description="release | beta"),
) -> BttLatestPerPlatform:
    """The latest BetterTroveTools version per platform (windows/linux/android) on
    a channel. Each platform walks the channel newest-first independently to the
    most recent release that actually ships an asset for it - so a release with
    no Windows build doesn't suppress Windows updates."""
    _check_btt_channel(channel)
    per = await btt_releases.latest_per_platform(channel)
    out: dict[str, BttPlatformLatest | None] = {}
    for platform, found in per.items():
        if found is None:
            out[platform] = None
            continue
        release, matched = found
        out[platform] = BttPlatformLatest(
            platform=platform, release=_release_meta(release),
            assets=[BttAsset(**a) for a in matched],
        )
    return BttLatestPerPlatform(channel=channel, platforms=out)


def _check_btt_platform(platform: str) -> None:
    if platform not in btt_releases.PLATFORMS:
        raise APIError(
            status_code=404, code=ErrorCode.not_found,
            message=f"unknown platform '{platform}' (known: {', '.join(btt_releases.PLATFORMS)})",
        )


@btt_router.get("/check", response_model=BttUpdateCheck)
async def check_btt_update(
    ctx: AccessContext = _BTT,
    installed: str = Query(..., min_length=1, description="The version tag the client has installed (e.g. 'v1.2.3')"),
    platform: str = Query(..., description="windows | linux | android"),
    channel: str = Query(default="release", description="release | beta"),
) -> BttUpdateCheck:
    """Server-side "is there an update?" - saves the client doing version math.
    Compares `installed` against the latest tag the channel has for `platform`
    (the same walk-back logic as `/latest`) and returns a boolean. `comparable`
    flags malformed tags so a client can decide how to treat the result."""
    _check_btt_platform(platform)
    _check_btt_channel(channel)
    per = await btt_releases.latest_per_platform(channel)
    found = per[platform]
    if found is None:
        return BttUpdateCheck(
            installed=installed, channel=channel, platform=platform,
            update_available=False, comparable=False, latest=None,
        )
    release, matched = found
    cmp = btt_releases.compare_versions(installed, release.tag_name)
    return BttUpdateCheck(
        installed=installed, channel=channel, platform=platform,
        update_available=(cmp is not None and cmp < 0),
        comparable=(cmp is not None),
        latest=BttPlatformLatest(
            platform=platform, release=_release_meta(release),
            assets=[BttAsset(**a) for a in matched],
        ),
    )


@btt_router.get("/latest/{platform}", response_model=BttPlatformLatest)
async def get_btt_latest_for_platform(
    platform: str,
    ctx: AccessContext = _BTT,
    channel: str = Query(default="release", description="release | beta"),
) -> BttPlatformLatest:
    """Latest BetterTroveTools version for a single platform on a channel."""
    _check_btt_platform(platform)
    _check_btt_channel(channel)
    per = await btt_releases.latest_per_platform(channel)
    found = per[platform]
    if found is None:
        raise APIError(
            status_code=404, code=ErrorCode.not_found,
            message=f"no {channel} release carries a {platform} asset yet",
        )
    release, matched = found
    return BttPlatformLatest(
        platform=platform, release=_release_meta(release),
        assets=[BttAsset(**a) for a in matched],
    )


@btt_router.get("/changelog", response_model=BttChangelogOut)
async def get_btt_changelog(
    ctx: AccessContext = _BTT,
    limit_groups: int = Query(default=0, ge=0, le=50,
        description="Limit returned groups to the newest N (0 = all)"),
    commits_per_group: int = Query(default=0, ge=0, le=100,
        description="Limit commits per group to the newest N (0 = all)"),
) -> BttChangelogOut:
    """The BetterTroveTools changelog - commits from GitHub grouped by tag,
    newest first; commits since the last tag live under `"Unreleased"`. Cached
    server-side so users don't blow through GitHub's 60/hr unauth rate limit
    when they open the changelog panel in the desktop app. Use `limit_groups`
    / `commits_per_group` to trim down what comes back."""
    from app.core.config import settings as _s
    doc = await btt_releases.get_changelog()
    if doc is None:
        # Refresher hasn't run yet (very early after boot). Return empty rather
        # than 404 so the client UI can render "loading" instead of an error.
        return BttChangelogOut(
            repo=_s.btt_releases_repo, groups=[], rate_limited=False, fetched_at=utcnow(),
        )
    groups = doc.groups
    if limit_groups:
        groups = groups[:limit_groups]
    if commits_per_group:
        groups = [{**g, "commits": g["commits"][:commits_per_group]} for g in groups]
    return BttChangelogOut(
        repo=doc.repo,
        groups=[BttChangelogGroup(version=g["version"],
                                  commits=[BttCommit(**c) for c in g["commits"]])
                for g in groups],
        rate_limited=doc.rate_limited,
        fetched_at=doc.fetched_at,
    )


# --- Leaderboards: in-game leaderboards ingest + read -----------------------
# READ side (scope: leaderboards:read) - anyone with the scope can query.
# WRITE side (insert) - gated by a superuser API token. The bot script POSTs a
# raw LeaderBot.cfg dump as a multipart file, the parser explodes it into the
# Postgres board/player/entry tables (entry is partitioned by anchor), and the
# capture is also gzip-saved to the backlog for replay/cutover.


async def _enforce_lb_archive_limit(
    response: Response, ctx: TokenContext, anchor: int,
) -> None:
    """Tight per-token throttle for anchors older than the archive threshold.

    Applied IN ADDITION to the standard per-token cap from ``_enforce_token_limits``.
    No-op when the queried anchor is within the threshold (normal hot/cold-30-day
    queries pay only the standard limit). Wide-open queries for old data are
    cheap per-row but a malicious caller could trawl the whole archive - this
    bucket caps them at ``settings.leaderboards_archive_rate_limit_max`` per
    window. ``X-RateLimit-Archive-*`` headers expose the bucket state alongside
    the standard ``X-RateLimit-*`` headers (which describe the wider limit)."""
    if not await leaderboards_service.is_archive_query(anchor):
        return
    from app.admin import runtime_config
    lb_max, lb_window = await runtime_config.get_rate_limit("leaderboards_archive_rate_limit")
    info = await check_rate_limit(f"lb_archive:{ctx.token.id}", lb_max, lb_window)
    # Don't clobber the standard X-RateLimit-* headers - surface this bucket
    # under a parallel namespace so clients can tell the two apart.
    for k, v in rate_limit_headers(info).items():
        response.headers[k.replace("X-RateLimit-", "X-RateLimit-Archive-")] = v


def _lb_timestamp(created_at: int | None) -> int:
    """Normalize a query ``created_at``; 404-style 400 if it's not on the 11:00
    UTC anchor (or its midnight alias)."""
    if created_at is None or created_at <= 0:
        raise APIError(
            status_code=400, code=ErrorCode.bad_request,
            message="Missing or invalid 'created_at' (unix seconds at 11:00 UTC).",
        )
    anchor = leaderboards_service.normalize_timestamp(created_at)
    if anchor == -1:
        raise APIError(
            status_code=400, code=ErrorCode.bad_request,
            message="created_at must align to a Trove reset (11:00 UTC; 00:00 UTC also accepted).",
        )
    return anchor


# NOTE: the public player-activity reads (current / history / series) live on
# their own `activity_router` (/v1/activity/*, scope activity:read) - see the
# "Player activity" section earlier in this module. They used to be mirrored
# here under leaderboards:read; that copy was removed when activity got its
# own scope.


@leaderboards_router.get(
    "/cheaters", response_model=CheatersResponse,
    summary="Flag possible cheaters via statistical outlier detection",
)
async def list_possible_cheaters(
    response: Response,
    _ctx: AccessContext = _LB_PUBLIC,
) -> CheatersResponse:
    """**Tokenless.** Flag players with statistically anomalous scores on
    the most-recent captured anchor. Runs three independent checks and
    surfaces the raw evidence per player per board so callers can decide
    how strict to be (a player flagged by multiple checks, or across
    multiple boards, is higher confidence than one with a single flag).

    Checks:

    - **Modified Z-score (MAD-based)** - robust outlier vs the board's
      *median*. Threshold default 3.5 (Iglewicz & Hoaglin 1993, "strong
      outlier"). MAD instead of stddev means a cheater can't pollute
      their own baseline.
    - **Rank-gap ratio** - the score gap from rank N to N+1 compared
      against the typical between-rank gap on the same board. Catches
      lone-wolf patterns at the top.
    - **Velocity** - score-gain rate (Δscore / Δtime) vs the board's
      peer p95 rate, using the player's previous historical capture.
      Degrades gracefully when archive history is thin.

    All thresholds + the cache TTL are runtime-tunable from the master
    panel (``cheaters_*`` keys). The response echoes the active config
    so a flag is reproducible. Cached in-process for
    ``cheaters_cache_ttl_seconds`` (default 30 min) keyed by
    ``(anchor, config)``; a config change invalidates immediately.
    """
    payload = await leaderboards_detection.detect_possible_cheaters()
    # Surface a matching Cache-Control to CDN / browsers. The server-side
    # cache TTL dominates, but a public CDN cache cuts noise even further.
    from app.admin import runtime_config
    ttl = int(await runtime_config.get_setting("cheaters_cache_ttl_seconds"))
    response.headers["Cache-Control"] = f"public, max-age={ttl}"
    return CheatersResponse(**payload)


@leaderboards_router.get("/timestamps", response_model=LeaderboardTimestamps)
async def list_leaderboard_timestamps(
    ctx: TokenContext = _LB,
    limit: int = Query(default=60, ge=1, le=365,
                       description="Number of recent timestamps to return (newest first)"),
) -> LeaderboardTimestamps:
    """The dump anchors that currently have stored entries, newest first.

    Each entry is a unix-seconds value at 11:00 UTC (Trove's daily reset). Pass
    one of these as ``?created_at=`` to the other endpoints to query a specific
    day's dump.
    """
    items = await leaderboards_service.list_timestamps(limit)
    return LeaderboardTimestamps(items=items, count=len(items))


@leaderboards_router.get("", response_model=LeaderboardListOut)
async def list_leaderboards(
    response: Response,
    ctx: TokenContext = _LB,
    created_at: int = Query(..., description="Anchor in unix seconds (11:00 UTC)"),
) -> LeaderboardListOut:
    """Every board that has stored entries at ``created_at``.

    ``contest_type`` is set on a board iff the dump at THIS anchor flagged it as
    a Daily/Weekly contest window; it's ``null`` otherwise. Anchors older than
    ``leaderboards_archive_query_threshold_days`` pay the archive rate-limit
    bucket on top of the standard per-token cap."""
    anchor = _lb_timestamp(created_at)
    await _enforce_lb_archive_limit(response, ctx, anchor)
    rows = await leaderboards_service.list_boards_at(anchor)
    items = [LeaderboardInfo(**r) for r in rows]
    return LeaderboardListOut(created_at=anchor, items=items, count=len(items))


@leaderboards_router.get("/{uuid:int}", response_model=LeaderboardBoardOut)
async def get_leaderboard(uuid: int, ctx: TokenContext = _LB) -> LeaderboardBoardOut:
    """A single board's metadata (no entries) - handy for resolving a uuid to a
    human name. ``contests`` lists every anchor at which the board was observed
    in a contest window."""
    row = await leaderboards_service.get_board(uuid)
    if row is None:
        raise APIError(
            status_code=404, code=ErrorCode.not_found,
            message=f"No leaderboard with uuid {uuid}",
        )
    return LeaderboardBoardOut(**row)


@leaderboards_router.get("/{uuid:int}/entries", response_model=LeaderboardEntriesPage)
async def list_leaderboard_entries(
    uuid: int,
    response: Response,
    ctx: TokenContext = _LB,
    created_at: int = Query(..., description="Anchor in unix seconds (11:00 UTC)"),
    limit: int = Query(default=100, ge=1, le=1000),
    offset: int = Query(default=0, ge=0),
) -> LeaderboardEntriesPage:
    """Ranked entries for one board at one anchor - top-N with pagination.

    Each item also carries day-over-day movement vs the previous trove-day's
    latest snapshot: ``rank_delta`` (positive = climbed), ``score_delta``
    (positive = gained), ``prev_rank``/``prev_score``, and ``is_new`` for a
    player with no prior-day position. These are only populated when the page
    ``comparison.comparable`` is true - the board must not have reset between the
    two snapshots, so daily boards never carry deltas day-over-day, weekly boards
    skip the Monday reset, and lifetime boards always compare.

    Anchors older than ``leaderboards_archive_query_threshold_days`` pay the
    archive rate-limit bucket on top of the standard per-token cap (the data
    lives in the cold collection at that point, and historical trawling is the
    intended target of the tighter throttle)."""
    anchor = _lb_timestamp(created_at)
    await _enforce_lb_archive_limit(response, ctx, anchor)
    items, total, comparison = await leaderboards_service.list_entries_with_deltas(
        uuid, anchor, limit=limit, offset=offset,
    )
    return LeaderboardEntriesPage(
        uuid=uuid, created_at=anchor,
        items=[LeaderboardEntryOut(**i) for i in items],
        count=len(items), total=total,
        comparison=LeaderboardComparison(**comparison),
    )


@leaderboards_router.get("/players/{player_name}/history",
                         response_model=LeaderboardPlayerHistory)
async def get_player_history(
    player_name: str,
    ctx: TokenContext = _LB,
    uuid: int | None = Query(default=None,
                             description="Restrict to a single board (optional)"),
    limit: int = Query(default=50, ge=1, le=500),
) -> LeaderboardPlayerHistory:
    """Recent dump appearances for ``player_name`` (most recent first).

    Useful for a player profile: their last N ranks across whatever boards they
    were on, optionally pinned to a single board with ``?uuid=``.
    """
    rows = await leaderboards_service.player_history(
        player_name, limit=limit, uuid=uuid,
    )
    return LeaderboardPlayerHistory(
        player_name=player_name,
        items=[LeaderboardPlayerEntry(**r) for r in rows],
        count=len(rows),
    )


@leaderboards_router.get("/{uuid:int}/history", response_model=BoardHistoryResponse)
async def get_board_history(
    uuid: int,
    response: Response,
    ctx: TokenContext = _LB,
    days: int = Query(default=7, ge=1, le=30,
                      description="Window size in days (default 7)"),
    top: int = Query(default=5, ge=1, le=20,
                     description="Lines to plot - top N at most-recent anchor"),
) -> BoardHistoryResponse:
    """Score-vs-time trajectories for the current top-``top`` players on
    ``uuid`` over the last ``days`` days of hourly captures.

    Drives the per-board chart on the public leaderboards page; also
    callable directly for clients that want their own visualisation. The
    older end of a wide window pays the archive rate-limit if it crosses
    the archive query threshold. Served from the Redis read-through cache
    (the warmer pre-computes the default 7d/top-5 for every board each
    ingest); the rate-limit is still charged regardless of cache hit."""
    # Archive-window check: if the window's lower bound is in archive
    # territory, charge the archive cap. Detection module + entries
    # endpoint use the same primitive. Charged BEFORE the cache read so a
    # cache hit can't dodge the limit.
    await _enforce_lb_archive_limit(
        response, ctx, int(utcnow().timestamp()) - days * 86400,
    )
    from app.trove.leaderboards import cache as leaderboards_cache
    payload = await leaderboards_cache.get_board_history(uuid, days, top)
    return BoardHistoryResponse(**payload)


@leaderboards_router.get("/players/{player_name}/series",
                         response_model=PlayerHistorySeriesResponse)
async def get_player_history_series(
    player_name: str,
    response: Response,
    ctx: TokenContext = _LB,
    days: int = Query(default=7, ge=1, le=30,
                      description="Window size in days (default 7)"),
) -> PlayerHistorySeriesResponse:
    """Score-vs-time trajectories for ONE player, grouped per board they
    appear on, over the last ``days`` days. Drives the per-player chart
    in the leaderboards page's history side-panel."""
    await _enforce_lb_archive_limit(
        response, ctx, int(utcnow().timestamp()) - days * 86400,
    )
    payload = await leaderboards_service.player_history_series(
        player_name, days=days,
    )
    return PlayerHistorySeriesResponse(**payload)


async def _process_leaderboard_dump(
    text: str, timestamp: int | None, user, token, nbytes: int,
    *, allow_backfill: bool = False, warm: bool = True,
) -> dict | None:
    """Background half of POST /v1/leaderboards/insert.

    Runs AFTER the 202 ack is sent, so the bot's HTTP client isn't held open
    through the multi-second parse + persist (which was timing it out). Both
    success and failure are written to the ingest log - the bot already has
    its ack and never sees an exception raised from here.
    """
    try:
        summary = await leaderboards_service.insert_dump(
            text, timestamp=timestamp, allow_backfill=allow_backfill,
        )
        await ingest_log.record(
            endpoint="/v1/leaderboards/insert",
            user=user, token=token,
            summary={
                "boards": summary.get("boards"),
                "entries": summary.get("entries"),
                "anchor": summary.get("created_at"),
                "cleared_before_insert": summary.get("cleared_before_insert"),
                "archived_old": summary.get("archived_old"),
                "bytes": nbytes,
            },
        )
        # Drop the Redis snapshot for this anchor so a re-insert / back-fill
        # can't serve the pre-insert cached boards/entries; the warmer below
        # re-warms the latest anchor's snapshot.
        if summary.get("created_at"):
            from app.trove.leaderboards import cache as leaderboards_cache
            await leaderboards_cache.invalidate_anchor(summary["created_at"])
            # Save the raw dump to the server-side backlog (keyed by anchor) so the
            # whole history can be re-ingested later from the admin panel with no
            # upload. Best-effort; the re-ingest path calls insert_dump directly so
            # it never re-saves (no loop).
            from app.trove.leaderboards import backlog
            await backlog.save(summary["created_at"], text)
            # Relay a lightweight "new leaderboard data" event to the live channel
            # (SSE subscribers can refetch). Best-effort; never fail the ingest.
            try:
                from app.events import bus as events_bus
                await events_bus.publish(
                    "leaderboard", str(summary["created_at"]),
                    {"anchor": summary["created_at"], "boards": summary.get("boards"),
                     "entries": summary.get("entries")},
                )
            except Exception:
                logger.warning("leaderboard event publish failed", exc_info=True)
        # Warm the new anchor's heavy queries (cheaters detection, activity
        # estimate, boards listing + the Redis snapshot) so the first visitor
        # doesn't pay the multi-second cold-cache tax. Skipped during a bulk
        # back-fill (warm=False) - warming on every file would re-scan an
        # ever-changing "latest"; the caller warms ONCE at the end instead.
        if warm:
            leaderboards_detection.trigger_warmer()
        return summary
    except Exception as exc:  # noqa: BLE001 - log + record; never re-raise (no client left to see it)
        logger.exception("leaderboard ingest processing failed")
        await ingest_log.record(
            endpoint="/v1/leaderboards/insert",
            user=user, token=token, success=False, error=str(exc)[:300],
            summary={"bytes": nbytes, "anchor": timestamp},
        )
        return None


@leaderboards_router.post("/insert", response_model=LeaderboardInsertResponse,
                          status_code=202,
                          summary="Insert leaderboard data")
async def insert_leaderboards(
    background_tasks: BackgroundTasks,
    response: Response,
    file: UploadFile = File(..., description="The raw LeaderBot.cfg dump (text)"),
    timestamp: int | None = Query(
        default=None,
        description=("Override the 'as-of' anchor in unix seconds (11:00 UTC). "
                     "Defaults to the latest 11:00 UTC reset - pass this only for back-fills."),
    ),
    backfill: bool = Query(
        default=False,
        description=("Master bulk re-seed: lift the 14-day anchor limit so a "
                     "historical capture lands at its real (filename) anchor "
                     "instead of falling back to today."),
    ),
    sync: bool = Query(
        default=False,
        description=("Process inline and return the real counts (200) instead of "
                     "202-background. Use for bulk back-fill so the client's await "
                     "paces ingestion - only one dump is in memory at a time."),
    ),
    warm: bool = Query(
        default=True,
        description=("Trigger the cache warmer after this insert. Set false during "
                     "a bulk back-fill and warm once at the end (POST /leaderboards/warm)."),
    ),
    _auth = _LB_MASTER,
) -> LeaderboardInsertResponse:
    """Ingest a leaderboard dump - accepted immediately, processed in the background.

    **Master only**: requires an API token owned by a superuser account. Submit
    the raw cfg text as a multipart file (the bot reads the game's
    ``LeaderBot.cfg`` and POSTs it verbatim). The dump is idempotent for a given
    anchor - re-running the same dump on the same timestamp converges.

    Returns **202 Accepted** as soon as the upload is read; the parse + persist
    (several seconds on a full dump) then runs in the background so the bot's
    client isn't held open and timed out. The resulting boards/entries counts -
    and any parse/DB error - are written to the master ingest log.
    """
    raw = await file.read()
    if not raw:
        raise APIError(
            status_code=400, code=ErrorCode.bad_request,
            message="Empty upload - POST the raw cfg text as a multipart 'file' field.",
        )
    try:
        text = raw.decode("utf-8")
    except UnicodeDecodeError:
        # Some scrapes have stray bytes; replace rather than reject so a single
        # bad row doesn't kill the whole dump.
        text = raw.decode("utf-8", errors="replace")
    # A bulk re-seed (backfill) is a PURE insert: NEVER warm per file - cheaters
    # + activity are recomputed ONCE at the end (POST /leaderboards/warm). Enforced
    # server-side so a stray warm=true can't make every file launch a heavy
    # calculation round (which is what was OOM-ing the box during mass ingest).
    warm = warm and not backfill
    # Hand the heavy parse+persist to a background task and ack now. Auth + the
    # per-token ingest cooldown already ran in the dependency (so an over-eager
    # bot still gets 429 before reaching here), and the dump text is fully
    # buffered in memory - the task doesn't depend on the request staying open.
    if sync:
        # Backpressure for bulk back-fills: process inline so the client's await
        # waits for this dump to finish before sending the next - only one ~18 MB
        # dump is in memory at a time, instead of the client racing ahead and
        # piling up background tasks until the container OOMs.
        summary = await _process_leaderboard_dump(
            text, timestamp, _auth.user, _auth.token, len(raw),
            allow_backfill=backfill, warm=warm,
        )
        response.status_code = 200
        if summary is None:
            return LeaderboardInsertResponse(
                accepted=False, bytes=len(raw),
                message="Ingest failed - see the master ingest log for the error.",
            )
        return LeaderboardInsertResponse(
            accepted=True, bytes=len(raw),
            message=(f"Ingested anchor {summary.get('created_at')}: "
                     f"{summary.get('boards')} boards, {summary.get('entries')} entries."),
        )

    background_tasks.add_task(
        _process_leaderboard_dump, text, timestamp, _auth.user, _auth.token, len(raw),
        allow_backfill=backfill, warm=warm,
    )
    return LeaderboardInsertResponse(
        accepted=True, bytes=len(raw),
        message=("Dump accepted - parsing and persisting in the background; "
                 "boards/entries counts land in the ingest log when done."),
    )


@leaderboards_router.post("/warm", status_code=202,
                          summary="Warm leaderboard caches (master)")
async def warm_leaderboards(_auth = _LB_MASTER) -> dict:
    """**Master only.** Wake the cache warmer to recompute the latest anchor's
    cheaters + activity + page snapshots. Call once after a bulk back-fill (which
    uploads with ``warm=false`` to avoid re-warming on every file)."""
    leaderboards_detection.trigger_warmer()
    return {"warming": True}


@leaderboards_router.post("/cheaters/recompute", status_code=200,
                          summary="Reset + recompute cheater detection (master)")
async def recompute_cheaters(_auth = _LB_MASTER) -> dict:
    """**Master only.** Drop the cached cheater-detection results (in-process +
    the persisted Redis snapshots) and recompute the latest anchor from scratch -
    independent of the activity / class-activity histories, so a cheater-config
    tweak can be re-evaluated without rerunning the long backfills. Cheap
    (latest-anchor only); runs inline and returns the new flag count."""
    from app.trove.leaderboards import cache as leaderboards_cache
    cleared = await leaderboards_cache.invalidate_cheaters()
    leaderboards_detection.reset()
    result = await leaderboards_detection.detect_possible_cheaters(force=True)
    return {
        "recomputed": True,
        "redis_snapshots_cleared": cleared,
        "anchor": result.get("anchor") if isinstance(result, dict) else None,
        "total_flagged": result.get("total_flagged") if isinstance(result, dict) else None,
        "boards_analyzed": result.get("boards_analyzed") if isinstance(result, dict) else None,
    }


@leaderboards_router.post("/views/recompute", status_code=200,
                          summary="Reset + re-warm the page view caches (master)")
async def recompute_views(_auth = _LB_MASTER) -> dict:
    """**Master only.** Drop the leaderboards page-VIEW caches (anchor list, board
    lists, entry pages, per-board history charts, ready pointer) and re-warm the
    latest captures + every board's default chart - independent of the cheaters /
    activity data. Use after a cache hiccup or to force the page snapshots to
    rebuild. Runs inline (a few seconds); returns how many keys were cleared +
    charts re-warmed."""
    from app.trove.leaderboards import cache as leaderboards_cache
    cleared = await leaderboards_cache.invalidate_views()
    await leaderboards_cache.warm()
    ts = await leaderboards_service.list_timestamps(limit=1, include_archive=False)
    warmed = 0
    anchor = ts[0] if ts else None
    if anchor is not None:
        warmed = await leaderboards_cache.warm_board_histories(anchor)
        await leaderboards_cache.set_ready_anchor(anchor)
    return {
        "recomputed": True,
        "keys_cleared": cleared,
        "anchor": anchor,
        "board_charts_warmed": warmed,
    }


@leaderboards_router.post("/reingest-backlog", status_code=202,
                          summary="Re-ingest the leaderboard backlog (master)")
async def reingest_backlog(
    background_tasks: BackgroundTasks,
    clear_first: bool = Query(
        default=False,
        description="Reset ALL leaderboard data first, then re-ingest from scratch.",
    ),
    _auth = _LB_MASTER,
) -> dict:
    """**Master only.** Replay every file in the server-side backlog folder
    (``{backlog_dir}/leaderboards/<anchor>.cfg[.gz]``) oldest-first - a pure
    server-side ingest (NO upload), one dump at a time, with the heavy
    cheaters/activity compute run ONCE at the end. Returns immediately; poll
    ``/reingest-status`` for live progress."""
    from app.trove.leaderboards import backlog
    files = backlog.list_files()
    if not files:
        return {"started": False, "files": 0,
                "message": ("Backlog is empty - let the bot populate it, or drop "
                            "<unix>.cfg files into the backlog folder.")}
    background_tasks.add_task(backlog.reingest, clear_first=clear_first)
    return {"started": True, "files": len(files), "clear_first": clear_first}


@leaderboards_router.get("/reingest-status",
                         summary="Backlog re-ingest progress (master)")
async def reingest_status(_auth = _LB_MASTER) -> dict:
    """**Master only.** Live progress of the backlog re-ingest (poll this) plus
    the current backlog file count."""
    from app.trove.leaderboards import backlog
    status = await backlog.get_status()
    status.update(backlog.info())   # backlog_dir + backlog_dir_exists + backlog_files
    return status


@leaderboards_router.post("/reset", status_code=200,
                          summary="Reset all leaderboard data (master)")
async def reset_leaderboards(
    drop_boards: bool = Query(
        default=False,
        description=("Also delete board metadata + admin reset-cadence overrides. "
                     "Off by default - those are config, re-used on re-ingest."),
    ),
    _auth = _LB_MASTER,
) -> dict:
    """**Master only. Destructive.** Wipe ALL leaderboard data + derived state for
    a clean slate before a full re-ingest: every entry (hot + archive), every
    stored activity estimate, and all cheater / activity / Redis caches + the
    ready pointer. Board metadata (incl. admin reset-cadence overrides) is kept
    unless ``drop_boards``. Returns the deletion tallies; logged at WARNING."""
    summary = await leaderboards_service.reset_all(drop_boards=drop_boards)
    await ingest_log.record(
        endpoint="/v1/leaderboards/reset",
        user=_auth.user, token=_auth.token, summary=summary,
    )
    return {"reset": True, **summary}


# NOTE: the master activity backfill moved to `POST /v1/activity/backfill`
# (on activity_router) so the whole activity family shares one prefix.


# --- Market: in-game marketplace listings ingest + read ---------------------
# READ side (scope: market:read) - anyone with the scope can query.
# WRITE side (insert) - superuser API token only. The bot script POSTs the raw
# GrainusMod.cfg dump as a multipart file; the parser pulls one row per
# listing, the service upserts by UUID (bumping last_seen on re-scrape).


async def _enforce_market_archive_limit(
    response: Response, ctx: TokenContext, hide_expired: bool,
) -> None:
    """Tight per-token throttle when a caller asks for expired market listings.

    Market's "archive surface" is implicit - any listing older than 7 days is
    expired (the in-game lifetime), so ``hide_expired=false`` is the request
    for historical data. Same shape as the leaderboards archive limit:
    additive on top of the standard per-token cap, exposed via
    ``X-RateLimit-Archive-*`` headers."""
    if hide_expired:
        return
    from app.admin import runtime_config
    mk_max, mk_window = await runtime_config.get_rate_limit("market_archive_rate_limit")
    info = await check_rate_limit(f"mkt_archive:{ctx.token.id}", mk_max, mk_window)
    for k, v in rate_limit_headers(info).items():
        response.headers[k.replace("X-RateLimit-", "X-RateLimit-Archive-")] = v


@market_router.get("/listings", response_model=MarketListingsPage)
async def list_market_listings(
    response: Response,
    ctx: TokenContext = _MKT,
    name: str | None = Query(default=None, description="Exact item name match"),
    price_min: float | None = Query(default=None, ge=0,
        description="Minimum price-each (flux)"),
    price_max: float | None = Query(default=None, ge=0,
        description="Maximum price-each (flux)"),
    last_seen_after: int | None = Query(default=None, ge=0,
        description="Only listings re-seen at or after this unix-seconds anchor"),
    hide_expired: bool = Query(default=True,
        description="Drop listings past their 7-day lifetime or stale >3h"),
    sort: str = Query(default="-last_seen",
        description="Beanie sort string: e.g. '+price_each', '-last_seen'"),
    limit: int = Query(default=100, ge=1, le=1000),
    offset: int = Query(default=0, ge=0),
) -> MarketListingsPage:
    """Paginated marketplace listings with the usual filters.

    ``hide_expired=true`` (the default) hides anything past its in-game lifetime
    or that hasn't been re-seen for 3+ hours; pass ``hide_expired=false`` to
    include the historical tail - which pays the archive rate-limit bucket on
    top of the standard per-token cap.
    """
    if price_min is not None and price_max is not None and price_min > price_max:
        raise APIError(
            status_code=400, code=ErrorCode.bad_request,
            message="price_min cannot be greater than price_max",
        )
    await _enforce_market_archive_limit(response, ctx, hide_expired)
    items, total = await market_service.list_listings(
        name=name, price_min=price_min, price_max=price_max,
        last_seen_after=last_seen_after, hide_expired=hide_expired,
        sort=sort, limit=limit, offset=offset,
    )
    return MarketListingsPage(
        items=[MarketListingOut(**i) for i in items],
        count=len(items), total=total,
    )


@market_router.get("/items", response_model=MarketItemList)
async def list_market_items(ctx: TokenContext = _MKT) -> MarketItemList:
    """Item names that currently have at least one stored listing (sorted).

    The output is a strict subset of ``/v1/market/interest_items`` - anything
    the bot tracks but hasn't seen on the market shows up there but not here.
    """
    items = await market_service.list_distinct_items()
    return MarketItemList(items=items, count=len(items))


# NOTE: the interest-items list moved to /v1/misc/interest-items so it's
# tokenless (the bot + dashboards / wikis can fetch it without an API key).
# Look in the misc router below for the new endpoint.


@market_router.get("/items/{name}/summary", response_model=MarketItemSummary)
async def get_market_item_summary(
    name: str, ctx: TokenContext = _MKT,
) -> MarketItemSummary:
    """Aggregate min/max/avg/median price-each + listing count for one item.

    Computed across active listings only (expired ones are excluded).
    """
    summary = await market_service.item_summary(name)
    if summary is None:
        raise APIError(
            status_code=404, code=ErrorCode.not_found,
            message=f"No active market listings for '{name}'",
        )
    return MarketItemSummary(**summary)


@market_router.post("/insert", response_model=MarketInsertResponse,
                    status_code=200,
                    summary="Insert market data")
async def insert_market_listings(
    file: UploadFile = File(..., description="The raw GrainusMod.cfg dump (text)"),
    timestamp: int | None = Query(
        default=None,
        description=("Override the 'last_seen' anchor in unix seconds. Defaults "
                     "to now() - pass this only for back-fills."),
    ),
    _auth = _MKT_MASTER,
) -> MarketInsertResponse:
    """Ingest a marketplace scrape.

    **Master only**: requires an API token owned by a superuser account. Submit
    the raw cfg text as a multipart file (the bot reads the game's
    ``GrainusMod.cfg`` and POSTs it verbatim). Idempotent at the listing level
    - same listing UUID re-scraped just bumps ``last_seen``, never duplicates.
    """
    raw = await file.read()
    if not raw:
        raise APIError(
            status_code=400, code=ErrorCode.bad_request,
            message="Empty upload - POST the raw cfg text as a multipart 'file' field.",
        )
    try:
        text = raw.decode("utf-8")
    except UnicodeDecodeError:
        text = raw.decode("utf-8", errors="replace")
    summary = await market_service.insert_dump(text, timestamp=timestamp)
    await ingest_log.record(
        endpoint="/v1/market/insert",
        user=_auth.user, token=_auth.token,
        summary={
            "parsed": summary.get("parsed"),
            "imported": summary.get("imported"),
            "ignored_not_in_list": summary.get("ignored_not_in_list"),
            "last_seen": summary.get("last_seen"),
            "bytes": len(raw),
        },
    )
    # Relay a lightweight "market refreshed" event to the live channel (SSE
    # subscribers can refetch). Best-effort; never fail the ingest.
    if summary.get("last_seen"):
        try:
            from app.events import bus as events_bus
            await events_bus.publish(
                "market", str(summary["last_seen"]),
                {"last_seen": summary["last_seen"], "imported": summary.get("imported"),
                 "parsed": summary.get("parsed")},
            )
        except Exception:
            logging.getLogger("kiwi.trove.router").warning(
                "market event publish failed", exc_info=True)
    return MarketInsertResponse(**summary)

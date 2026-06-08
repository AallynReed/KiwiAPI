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
from app.trove import (
    btt_releases,
    captures,
    chaos,
    delves,
    misc,
    news,
    relays,
    rotations,
    server_time,
    stats,
    tmod,
)
from app.trove import calendar as trove_calendar
from app.trove.codexes import read as codexes_read
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
    ChaosChestHistoryPage,
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
    LeaderboardEntriesPage,
    LeaderboardEntryOut,
    LeaderboardInfo,
    LeaderboardInsertResponse,
    LeaderboardListOut,
    LeaderboardPlayerEntry,
    LeaderboardPlayerHistory,
    LeaderboardTimestamps,
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
from app.trove.updates.cas import ContentStore
from app.trove.updates.cdn import BRANCHES as UPDATE_BRANCHES
from app.trove.updates.schemas import (
    BranchInfo,
    BranchList,
    FileMeta,
    TreeEntry,
    TreeListing,
    VersionInfo,
    VersionList,
)

# Organized by FUNCTION, not by game — most of the API is Trove, so grouping by
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
# Tokenless slice of misc — the bot's interest-items list is public so dashboards
# / wikis can render it without a key. A token carrying misc:read still earns the
# wider per-token rate budget; anon callers pay the stricter per-IP cap.
_MISC_PUBLIC = Depends(public_scope("misc:read"))
_MODS = Depends(require_scope("mods:read"))
_UPD = Depends(require_scope("updates:read"))

# Used by the feedback webhook helper for "best-effort, log on failure".
logger = logging.getLogger("kiwi.trove.router")
# Codexes are PUBLIC too (game reference data): usable without a token, and at a
# wider rate budget (5× by default) on both the anonymous and authenticated paths.
_CODEX = Depends(public_scope("codexes:read", rate_multiplier=settings.codexes_rate_limit_multiplier))
# BTT releases are PUBLIC: the desktop app needs to poll them on every launch
# to drive update notifications, so no token is required.
_BTT = Depends(public_scope("btt:read"))
# Leaderboards read-side is token-gated (the data is bulky + opinionated). The
# write-side has its own dep — see /v1/leaderboards/insert below.
_LB = Depends(require_scope("leaderboards:read"))
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
    when the bot hasn't reported this week yet — e.g., immediately after a Tue
    11:00 UTC reset."""
    return ChaosChest(**await chaos.get_chaos_chest())


@rotations_router.post("/chaos-chest/insert", response_model=CaptureInsertResponse,
                       summary="Insert chaos chest data")
async def insert_chaos_chest(
    req: CaptureInsertRequest,
    _user: User = Depends(require_master_ingest),
) -> CaptureInsertResponse:
    """Persist the bot-captured chaos-chest item for the current weekly window.

    **Master only**: requires an API token owned by a superuser account. The
    server infers the week anchor (Tue 11:00 UTC) from "now" — the bot just
    sends ``{name}``. Idempotent: re-submitting the same week replaces the row.
    """
    try:
        doc, was_new = await captures.insert_chaos_chest(req.name)
    except ValueError as e:
        raise APIError(status_code=400, code=ErrorCode.bad_request, message=str(e))
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
    (real-UTC Fri 11:00 → Sat 11:00) it's two per hour — :00 and :30 — and
    ``is_friday_window`` is set. Each window lasts 20 minutes; ``active``
    distinguishes the live window from the gap that follows.
    """
    return ChallengeCurrentOut(**await captures.get_current_challenge())


@rotations_router.post("/challenge/insert", response_model=CaptureInsertResponse,
                       summary="Insert challenge data")
async def insert_challenge(
    req: CaptureInsertRequest,
    _user: User = Depends(require_master_ingest),
) -> CaptureInsertResponse:
    """Persist the bot-captured challenge name for the active 20-minute window.

    **Master only**. The server infers the window anchor from "now"; the bot
    just sends ``{name}``. Idempotent at the (anchor) level. ``name`` of
    ``"none"`` (or empty) is rejected — the bot is expected to skip those
    submissions client-side, so seeing one here surfaces as a 400.
    """
    try:
        doc, was_new = await captures.insert_challenge(req.name)
    except ValueError as e:
        raise APIError(status_code=400, code=ErrorCode.bad_request, message=str(e))
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
    """The full yearly event calendar (±365 days): every recurring rotation —
    weekly buffs, Corruxion/Fluxion, gardening, Wild Mana, Stampy — as one flat,
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
    """A week's delve rotation — its floor records (relayed from an external source).
    Defaults to the current week; pass `?week=` for history (see `/delves/weeks`)."""
    current = delves.current_week_id()
    wk = week if week is not None else current
    doc = await delves.get_week(wk)
    if doc is None:
        if week is not None:
            raise APIError(status_code=404, code=ErrorCode.not_found,
                           message=f"No delve data for week {week}")
        # Current week not captured yet — return an empty rotation for it.
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
    """Live Twitch streams for Trove, relayed + cached from the trovesaurus bot."""
    items, fetched_at = await relays.get_feed("twitch")
    return TwitchStreams(
        items=[TwitchStream(**i) for i in items], count=len(items), fetched_at=fetched_at
    )


@feeds_router.get("/youtube", response_model=Videos)
async def get_youtube(ctx: AccessContext = _FEED) -> Videos:
    """Recent Trove YouTube videos, relayed + cached from the trovesaurus bot."""
    items, fetched_at = await relays.get_feed("youtube")
    return Videos(items=[Video(**i) for i in items], count=len(items), fetched_at=fetched_at)


@feeds_router.get("/bilibili", response_model=Videos)
async def get_bilibili(ctx: AccessContext = _FEED) -> Videos:
    """Recent Trove Bilibili videos, relayed + cached from the trovesaurus bot."""
    items, fetched_at = await relays.get_feed("bilibili")
    return Videos(items=[Video(**i) for i in items], count=len(items), fetched_at=fetched_at)


@feeds_router.get("/bilibili/image")
async def proxy_bilibili_image(
    url: str = Query(..., description="Absolute https hdslb.com thumbnail URL to proxy"),
    ctx: AccessContext = _FEED_IMG,
) -> Response:
    """Proxy a Bilibili thumbnail so browsers and WebViews can display it.

    Bilibili's CDN blocks hotlinking unless a bilibili.com Referer is sent, which
    an <img> tag can't do — clients point <img src> here and we refetch with the
    Referer. Cross-origin <img> loads aren't subject to CORS, so this serves the
    hosted web build and the Android WebView identically (and replaces the local
    proxy the desktop/web_server builds carry).
    """
    try:
        content, content_type = await relays.fetch_bilibili_image(url)
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
    """The distinct event categories currently stored — discovered dynamically, sorted."""
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
# Pure data, no calculation — what each source/field is and how much it gives.


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
# Stateless compute — gem objects round-trip through the client; nothing stored.


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
    """The plausible (min, max) value a stat can roll at — for inline input hints."""
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

    Reads the URL from runtime config on every call (cheap — cached for 5s
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

    # Gallery URL — arbitrary but must be identical across every embed
    # in the message and must look like a real URL. Discord groups embeds
    # that share a `url` into one card; that's how we get N images
    # inside ONE embed instead of N stacked embeds. The URL doesn't need
    # to resolve — it's just the grouping key.
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
    # Additional image-only sibling embeds — each just an image with the
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
                # references above — that's why we used the relabeled
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
    except Exception as e:  # noqa: BLE001 — webhook is best-effort
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

    **Tokenless** — anyone can submit. Wire format is ``multipart/form-data``
    so attachments can ride along; a JSON-only client just omits the file
    fields and works the same.

    **Rate limits** (runtime-tunable from the admin panel):
      - ``feedback.per_ip_max`` / ``feedback.per_ip_window_seconds`` —
        burst from one source. Hit returns 429 with ``X-RateLimit-*`` +
        ``Retry-After``.
      - ``feedback.global_max`` / ``feedback.global_window_seconds`` —
        silent global backstop.

    **Body fields**:
      - ``message`` (required, 5-2000 chars)
      - ``contact`` (optional, ≤200 chars) — reply channel, free text
      - ``category`` (``bug`` / ``feature`` / ``general``; default ``general``)
      - ``app_version`` (optional, ≤64 chars) — for desktop / 3rd-party
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

    # ── 3. Validate attachments — count, MIME, size — and read bytes ──
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
        raise _bad_request("Empty body — POST the raw .tmod file bytes.")
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

    `modLoader` is always stamped `KiwiAPI`. Nothing is stored — the file is built
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
    branch: str, path: str = Query(...), ctx: TokenContext = _UPD,
) -> FileResponse:
    """Download a single file's bytes from the latest tree (streamed from the blob store)."""
    _check_branch(branch)
    meta = await updates_read.get_file_meta(branch, path)
    if meta is None:
        raise APIError(status_code=404, code=ErrorCode.not_found, message=f"No file '{path}'")
    blob = ContentStore(settings.trove_update_store_dir).path_for(meta["content_sha256"])
    if not blob.is_file():
        raise APIError(status_code=404, code=ErrorCode.not_found, message="Blob missing from the store")
    filename = path.rsplit("/", 1)[-1] or "file"
    return FileResponse(blob, media_type="application/octet-stream", filename=filename)


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
    """Entries of one codex type — filterable (search/category/tradable), sortable, paged."""
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
    """Distinct categories (+ counts) within a type — for building filter dropdowns."""
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
    most recent release that actually ships an asset for it — so a release with
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
    """Server-side "is there an update?" — saves the client doing version math.
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
    """The BetterTroveTools changelog — commits from GitHub grouped by tag,
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
# READ side (scope: leaderboards:read) — anyone with the scope can query.
# WRITE side (insert) — gated by a superuser API token. The bot script POSTs a
# raw LeaderBot.cfg dump as a multipart file, the parser explodes it into the
# Leaderboard / LeaderboardEntry collections, then prunes anything older than
# the retention window.


async def _enforce_lb_archive_limit(
    response: Response, ctx: TokenContext, anchor: int,
) -> None:
    """Tight per-token throttle for anchors older than the archive threshold.

    Applied IN ADDITION to the standard per-token cap from ``_enforce_token_limits``.
    No-op when the queried anchor is within the threshold (normal hot/cold-30-day
    queries pay only the standard limit). Wide-open queries for old data are
    cheap per-row but a malicious caller could trawl the whole archive — this
    bucket caps them at ``settings.leaderboards_archive_rate_limit_max`` per
    window. ``X-RateLimit-Archive-*`` headers expose the bucket state alongside
    the standard ``X-RateLimit-*`` headers (which describe the wider limit)."""
    if not await leaderboards_service.is_archive_query(anchor):
        return
    from app.admin import runtime_config
    lb_max, lb_window = await runtime_config.get_rate_limit("leaderboards_archive_rate_limit")
    info = await check_rate_limit(f"lb_archive:{ctx.token.id}", lb_max, lb_window)
    # Don't clobber the standard X-RateLimit-* headers — surface this bucket
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


@leaderboards_router.get("/{uuid}", response_model=LeaderboardBoardOut)
async def get_leaderboard(uuid: int, ctx: TokenContext = _LB) -> LeaderboardBoardOut:
    """A single board's metadata (no entries) — handy for resolving a uuid to a
    human name. ``contests`` lists every anchor at which the board was observed
    in a contest window."""
    row = await leaderboards_service.get_board(uuid)
    if row is None:
        raise APIError(
            status_code=404, code=ErrorCode.not_found,
            message=f"No leaderboard with uuid {uuid}",
        )
    return LeaderboardBoardOut(**row)


@leaderboards_router.get("/{uuid}/entries", response_model=LeaderboardEntriesPage)
async def list_leaderboard_entries(
    uuid: int,
    response: Response,
    ctx: TokenContext = _LB,
    created_at: int = Query(..., description="Anchor in unix seconds (11:00 UTC)"),
    limit: int = Query(default=100, ge=1, le=1000),
    offset: int = Query(default=0, ge=0),
) -> LeaderboardEntriesPage:
    """Ranked entries for one board at one anchor — top-N with pagination.

    Anchors older than ``leaderboards_archive_query_threshold_days`` pay the
    archive rate-limit bucket on top of the standard per-token cap (the data
    lives in the cold collection at that point, and historical trawling is the
    intended target of the tighter throttle)."""
    anchor = _lb_timestamp(created_at)
    await _enforce_lb_archive_limit(response, ctx, anchor)
    items, total = await leaderboards_service.list_entries(
        uuid, anchor, limit=limit, offset=offset,
    )
    return LeaderboardEntriesPage(
        uuid=uuid, created_at=anchor,
        items=[LeaderboardEntryOut(**i) for i in items],
        count=len(items), total=total,
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


@leaderboards_router.post("/insert", response_model=LeaderboardInsertResponse,
                          status_code=200,
                          summary="Insert leaderboard data")
async def insert_leaderboards(
    file: UploadFile = File(..., description="The raw LeaderBot.cfg dump (text)"),
    timestamp: int | None = Query(
        default=None,
        description=("Override the 'as-of' anchor in unix seconds (11:00 UTC). "
                     "Defaults to the latest 11:00 UTC reset — pass this only for back-fills."),
    ),
    _user: User = _LB_MASTER,
) -> LeaderboardInsertResponse:
    """Ingest a leaderboard dump.

    **Master only**: requires an API token owned by a superuser account. Submit
    the raw cfg text as a multipart file (the bot reads the game's
    ``LeaderBot.cfg`` and POSTs it verbatim). The dump is idempotent for a given
    anchor — re-running the same dump on the same timestamp converges.
    """
    raw = await file.read()
    if not raw:
        raise APIError(
            status_code=400, code=ErrorCode.bad_request,
            message="Empty upload — POST the raw cfg text as a multipart 'file' field.",
        )
    try:
        text = raw.decode("utf-8")
    except UnicodeDecodeError:
        # Some scrapes have stray bytes; replace rather than reject so a single
        # bad row doesn't kill the whole dump.
        text = raw.decode("utf-8", errors="replace")
    summary = await leaderboards_service.insert_dump(text, timestamp=timestamp)
    return LeaderboardInsertResponse(**summary)


# --- Market: in-game marketplace listings ingest + read ---------------------
# READ side (scope: market:read) — anyone with the scope can query.
# WRITE side (insert) — superuser API token only. The bot script POSTs the raw
# GrainusMod.cfg dump as a multipart file; the parser pulls one row per
# listing, the service upserts by UUID (bumping last_seen on re-scrape).


async def _enforce_market_archive_limit(
    response: Response, ctx: TokenContext, hide_expired: bool,
) -> None:
    """Tight per-token throttle when a caller asks for expired market listings.

    Market's "archive surface" is implicit — any listing older than 7 days is
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
    include the historical tail — which pays the archive rate-limit bucket on
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

    The output is a strict subset of ``/v1/market/interest_items`` — anything
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
                     "to now() — pass this only for back-fills."),
    ),
    _user: User = _MKT_MASTER,
) -> MarketInsertResponse:
    """Ingest a marketplace scrape.

    **Master only**: requires an API token owned by a superuser account. Submit
    the raw cfg text as a multipart file (the bot reads the game's
    ``GrainusMod.cfg`` and POSTs it verbatim). Idempotent at the listing level
    — same listing UUID re-scraped just bumps ``last_seen``, never duplicates.
    """
    raw = await file.read()
    if not raw:
        raise APIError(
            status_code=400, code=ErrorCode.bad_request,
            message="Empty upload — POST the raw cfg text as a multipart 'file' field.",
        )
    try:
        text = raw.decode("utf-8")
    except UnicodeDecodeError:
        text = raw.decode("utf-8", errors="replace")
    summary = await market_service.insert_dump(text, timestamp=timestamp)
    return MarketInsertResponse(**summary)

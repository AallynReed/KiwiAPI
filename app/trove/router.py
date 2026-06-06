import base64
import re

import httpx
from fastapi import APIRouter, Body, Depends, Query, Response
from fastapi.responses import FileResponse

from app.core.config import settings
from app.core.dependencies import AccessContext, TokenContext, public_scope, require_scope
from app.core.errors import APIError, ErrorCode
from app.core.utils import utcnow
from app.trove import (
    btt_releases,
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
    BttLatestPerPlatform,
    BttPlatformLatest,
    BttReleaseInfo,
    BttReleaseList,
    BttReleaseMeta,
    BttUpdateCheck,
    ChaosChest,
    ClassList,
    Corruxion,
    DailyBuffs,
    DelveRotationOut,
    DelveWeekInfo,
    DelveWeekList,
    EventCategoryList,
    Fluxion,
    Gardening,
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
_MODS = Depends(require_scope("mods:read"))
_UPD = Depends(require_scope("updates:read"))
# Codexes are PUBLIC too (game reference data): usable without a token, and at a
# wider rate budget (5× by default) on both the anonymous and authenticated paths.
_CODEX = Depends(public_scope("codexes:read", rate_multiplier=settings.codexes_rate_limit_multiplier))
# BTT releases are PUBLIC: the desktop app needs to poll them on every launch
# to drive update notifications, so no token is required.
_BTT = Depends(public_scope("btt:read"))

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
    """The weekly Chaos Chest: current featured item (relayed) + window + countdown."""
    return ChaosChest(**await chaos.get_chaos_chest())


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

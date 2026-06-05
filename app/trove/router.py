import base64
import re

from fastapi import APIRouter, Body, Depends, Query, Response
from fastapi.responses import FileResponse

from app.core.config import settings
from app.core.dependencies import TokenContext, require_scope
from app.core.errors import APIError, ErrorCode
from app.core.utils import utcnow
from app.trove import misc, relays, rotations, server_time, stats, tmod
from app.trove.codexes import read as codexes_read
from app.trove.codexes.schemas import (
    CodexEntryOut,
    CodexEntryPage,
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
    ClassList,
    Corruxion,
    DailyBuffs,
    EventCategoryList,
    Fluxion,
    Gardening,
    ServerTime,
    StatTable,
    TroveClass,
    TroveEventItem,
    TroveEventList,
    TroveNewsItem,
    TroveNewsList,
    TwitchStream,
    TwitchStreams,
    Video,
    Videos,
    WeeklyBuffs,
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

_ROT = Depends(require_scope("rotations:read"))
_FEED = Depends(require_scope("feeds:read"))
_STAT = Depends(require_scope("stats:read"))
_GEM = Depends(require_scope("gems:read"))
_MISC = Depends(require_scope("misc:read"))
_MODS = Depends(require_scope("mods:read"))
_UPD = Depends(require_scope("updates:read"))
_CODEX = Depends(require_scope("codexes:read"))

# The codex serves the primary timeline by default; PTS is opt-in via ?branch=.
_DEFAULT_CODEX_BRANCH = "live-us"


# --- Rotations / timers (scope: rotations:read) ----------------------------

@rotations_router.get("/server-time", response_model=ServerTime)
async def get_server_time(ctx: TokenContext = _ROT) -> ServerTime:
    """Current Trove server time, current in-game day, and the next daily / weekly resets."""
    return ServerTime(**server_time.server_time())


@rotations_router.get("/daily-buffs", response_model=DailyBuffs)
async def get_daily_buffs(ctx: TokenContext = _ROT) -> DailyBuffs:
    """Today's daily buff plus the full Monday→Sunday rotation."""
    return DailyBuffs(**server_time.daily_buffs())


@rotations_router.get("/weekly-buffs", response_model=WeeklyBuffs)
async def get_weekly_buffs(ctx: TokenContext = _ROT) -> WeeklyBuffs:
    """This week's weekly buff plus the full 4-week rotation."""
    return WeeklyBuffs(**server_time.weekly_buffs())


@rotations_router.get("/corruxion", response_model=Corruxion)
async def get_corruxion(ctx: TokenContext = _ROT) -> Corruxion:
    """Corruxion merchant: live timer + upcoming schedule (14-day / 3-day cycle)."""
    return Corruxion(**server_time.corruxion())


@rotations_router.get("/fluxion", response_model=Fluxion)
async def get_fluxion(ctx: TokenContext = _ROT) -> Fluxion:
    """Fluxion merchant: live timer (voting/selling) + upcoming schedule."""
    return Fluxion(**server_time.fluxion())


@rotations_router.get("/gardening", response_model=Gardening)
async def get_gardening(ctx: TokenContext = _ROT) -> Gardening:
    """Gardening harvest windows: current 2-day and 3-day plant windows + what's next."""
    return Gardening(**server_time.gardening())


@rotations_router.get("/biomes", response_model=BiomeRotationFeed)
async def get_biomes(ctx: TokenContext = _ROT) -> BiomeRotationFeed:
    """The 3-hour adventure-world biome rotation (d15): current + upcoming."""
    return BiomeRotationFeed(**rotations.biome_rotation())


@rotations_router.get("/wild-mana", response_model=BiomeRotationFeed)
async def get_wild_mana(ctx: TokenContext = _ROT) -> BiomeRotationFeed:
    """The weekly Wild Mana biome rotation: current + upcoming."""
    return BiomeRotationFeed(**rotations.wild_mana())


@rotations_router.get("/stampy", response_model=BiomeRotationFeed)
async def get_stampy(ctx: TokenContext = _ROT) -> BiomeRotationFeed:
    """The weekly Stampy event biome (48-hour window): current + upcoming."""
    return BiomeRotationFeed(**rotations.stampy())


# --- Feeds (scope: feeds:read) ---------------------------------------------

@feeds_router.get("/news", response_model=TroveNewsList)
async def list_news(
    ctx: TokenContext = _FEED,
    limit: int = Query(default=20, ge=1, le=50),
) -> TroveNewsList:
    """Latest Trove news, relayed from trovegame.com and cached server-side, newest first."""
    docs = await TroveNews.find().sort("-published_at").limit(limit).to_list()
    items = [
        TroveNewsItem(
            title=d.title,
            url=d.url,
            author=d.author,
            summary=d.summary,
            category=d.category,
            categories=d.categories,
            image=d.image,
            published_at=d.published_at,
        )
        for d in docs
    ]
    return TroveNewsList(items=items, count=len(items))


@feeds_router.get("/twitch", response_model=TwitchStreams)
async def get_twitch(ctx: TokenContext = _FEED) -> TwitchStreams:
    """Live Twitch streams for Trove, relayed + cached from the trovesaurus bot."""
    items, fetched_at = await relays.get_feed("twitch")
    return TwitchStreams(
        items=[TwitchStream(**i) for i in items], count=len(items), fetched_at=fetched_at
    )


@feeds_router.get("/youtube", response_model=Videos)
async def get_youtube(ctx: TokenContext = _FEED) -> Videos:
    """Recent Trove YouTube videos, relayed + cached from the trovesaurus bot."""
    items, fetched_at = await relays.get_feed("youtube")
    return Videos(items=[Video(**i) for i in items], count=len(items), fetched_at=fetched_at)


@feeds_router.get("/bilibili", response_model=Videos)
async def get_bilibili(ctx: TokenContext = _FEED) -> Videos:
    """Recent Trove Bilibili videos, relayed + cached from the trovesaurus bot."""
    items, fetched_at = await relays.get_feed("bilibili")
    return Videos(items=[Video(**i) for i in items], count=len(items), fetched_at=fetched_at)


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
    ctx: TokenContext = _FEED,
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
async def list_event_categories(ctx: TokenContext = _FEED) -> EventCategoryList:
    """The distinct event categories currently stored — discovered dynamically, sorted."""
    rows = await TroveEvent.aggregate(
        [{"$group": {"_id": "$category"}}, {"$sort": {"_id": 1}}]
    ).to_list()
    categories = [r["_id"] for r in rows if r.get("_id")]
    return EventCategoryList(categories=categories, count=len(categories))


@feeds_router.get("/events/upcoming", response_model=TroveEventList)
async def list_upcoming_events(
    ctx: TokenContext = _FEED,
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
    ctx: TokenContext = _FEED,
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


# --- Misc: modding software + time converter (scope: misc:read) -------------


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
        description=d.description, tradable=d.tradable, blueprint=d.blueprint,
        data=d.data, indexed_at=d.indexed_at,
    )


@codexes_router.get("/types", response_model=CodexTypeList)
async def list_codex_types(
    ctx: TokenContext = _CODEX,
    branch: str = Query(default=_DEFAULT_CODEX_BRANCH),
) -> CodexTypeList:
    """The codex types present for a branch, each with its entry count."""
    _check_branch(branch)
    rows = await codexes_read.type_counts(branch)
    return CodexTypeList(
        branch=branch, items=[CodexTypeInfo(**r) for r in rows], count=len(rows),
    )


@codexes_router.get("/{codex_type}", response_model=CodexEntryPage)
async def list_codex_entries(
    codex_type: str,
    ctx: TokenContext = _CODEX,
    branch: str = Query(default=_DEFAULT_CODEX_BRANCH),
    search: str | None = Query(default=None, description="Case-insensitive name substring"),
    limit: int = Query(default=50, ge=1, le=200),
    offset: int = Query(default=0, ge=0),
) -> CodexEntryPage:
    """Entries of one codex type, name-sorted; searchable by name and paginated."""
    _check_branch(branch)
    _check_codex_type(codex_type)
    docs, total = await codexes_read.list_entries(
        branch, codex_type, search=search, limit=limit, offset=offset,
    )
    return CodexEntryPage(
        branch=branch, type=codex_type,
        items=[_codex_out(d) for d in docs], count=len(docs), total=total,
    )


@codexes_router.get("/{codex_type}/entry", response_model=CodexEntryOut)
async def get_codex_entry(
    codex_type: str,
    path: str = Query(..., description="The entry's source prefab path (its stable id)"),
    ctx: TokenContext = _CODEX,
    branch: str = Query(default=_DEFAULT_CODEX_BRANCH),
) -> CodexEntryOut:
    """A single codex entry by its source prefab path."""
    _check_branch(branch)
    _check_codex_type(codex_type)
    doc = await codexes_read.get_entry(branch, codex_type, path)
    if doc is None:
        raise APIError(status_code=404, code=ErrorCode.not_found, message=f"No {codex_type} entry '{path}'")
    return _codex_out(doc)

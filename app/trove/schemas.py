from datetime import datetime
from typing import Literal

from pydantic import BaseModel, Field

# Category a captured challenge falls into. Computed from the raw display name
# by ``app.trove.captures.classify_challenge`` so consumers don't have to
# memorise which strings are special-cased; see that helper for the rules.
# Field name ``type`` shadows the Python builtin at attribute level but
# Pydantic handles it fine - chose ``type`` because the old API used
# ``challenge_type`` and consumers reading the renamed `kind` would have
# broken. ``ChallengeType`` (the alias) is the public name.
ChallengeType = Literal["collection", "rampage", "racing", "target", "dungeon"]

# --- Server time -----------------------------------------------------------


class ServerTime(BaseModel):
    now_unix: int                 # real UTC, unix seconds
    now_iso: str                  # real UTC, ISO 8601
    trove_day: str                # current in-game day, e.g. "Friday"
    daily_reset_at: int           # next daily reset (11:00 UTC), unix seconds
    weekly_reset_at: int          # next weekly-buff rotation, unix seconds


# --- Buffs -----------------------------------------------------------------


class DailyBuff(BaseModel):
    name: str
    color: str
    weekday: str
    emoji: str
    normal_buffs: list[str]
    premium_buffs: list[str]
    icon: str | None = None
    banner: str | None = None


class DailyBuffs(BaseModel):
    current: DailyBuff
    week: list[DailyBuff]         # all 7, Monday..Sunday


class WeeklyBuff(BaseModel):
    name: str
    color: str
    weekday: str
    emoji: str
    buffs: list[str]
    banner: str | None = None


class WeeklyBuffs(BaseModel):
    current: WeeklyBuff
    rotation: list[WeeklyBuff]    # the full 4-week rotation


# --- Merchants (Corruxion, Fluxion) ----------------------------------------


class MerchantWindow(BaseModel):
    starts_at: int                # unix seconds (real UTC)
    ends_at: int
    state: str | None = None      # Fluxion only: "voting" | "selling"


class Corruxion(BaseModel):
    active: bool
    starts_at: int                # current window (if active) else next window
    ends_at: int
    seconds_remaining: int        # to leaving (if active) else to arriving
    schedule: list[MerchantWindow]  # upcoming occurrences, soonest first


class Fluxion(BaseModel):
    active: bool
    state: str                    # "voting" | "selling" | "away"
    starts_at: int
    ends_at: int
    seconds_remaining: int
    schedule: list[MerchantWindow]


# --- Chaos Chest (weekly featured-item rotation) ---------------------------


class ChaosChestItem(BaseModel):
    name: str                     # featured item display name
    identifier: str | None = None  # game path identifier (forward-slashed)
    blueprint: str | None = None   # blueprint path (lowercased)


class ChaosChest(BaseModel):
    active: bool                  # within the current weekly window
    starts_at: int                # window start, unix seconds
    ends_at: int                  # window end, unix seconds
    seconds_remaining: int        # to the window end
    item: ChaosChestItem | None   # featured item (None if upstream unavailable)
    fetched_at: datetime | None   # when the item was last relayed


# --- Yearly calendar (all rotations as one ±365-day timeline) --------------


class CalendarBiome(BaseModel):
    name: str
    icon: str


class CalendarEvent(BaseModel):
    type: str                     # weekly_buff | corruxion | fluxion | gardening_2/3 | stampy | mana
    name: str
    starts_at: int
    ends_at: int
    color: str | None = None      # hex (no #) for buff / fluxion / gardening entries
    state: str | None = None      # fluxion only: "voting" | "selling"
    biomes: list[CalendarBiome] | None = None  # stampy / mana entries


class YearlyCalendar(BaseModel):
    starts_at: int                # window start (now − 365d), unix seconds
    ends_at: int                  # window end (now + 365d), unix seconds
    generated_at: int             # the "now" the window was centered on
    count: int
    events: list[CalendarEvent]   # flat, sorted by starts_at


# --- Delve rotations (weekly community delve data, relayed from an external source) ---


class DelveWeekInfo(BaseModel):
    week: int
    total: int                    # source-reported total
    count: int                    # stored floor records
    fetched_at: datetime


class DelveWeekList(BaseModel):
    current_week: int             # the live week id right now
    items: list[DelveWeekInfo]    # available weeks, newest first
    count: int


class DelveRotationOut(BaseModel):
    week: int
    is_current: bool              # whether this is the live week
    total: int
    count: int
    fetched_at: datetime | None
    depths: list[dict]            # floor records, passed through from the source


# --- Gardening (plant harvest windows) -------------------------------------


class HarvestWindow(BaseModel):
    name: str                     # "2-day plants" | "3-day plants"
    active: bool
    starts_at: int
    ends_at: int


class Gardening(BaseModel):
    two_day: HarvestWindow
    three_day: HarvestWindow
    upcoming: list[HarvestWindow]  # next harvest windows, soonest first


# --- Biome rotations (d15 / wild mana / stampy) ----------------------------


class Biome(BaseModel):
    name: str                     # sub-biome name
    final_name: str               # parent/display biome
    icon: str


class BiomeRotation(BaseModel):
    starts_at: int
    ends_at: int
    biomes: list[Biome]


class BiomeRotationFeed(BaseModel):
    current: BiomeRotation | None
    upcoming: list[BiomeRotation]


# --- News ------------------------------------------------------------------


class TroveNewsItem(BaseModel):
    title: str
    url: str
    author: str
    summary: str
    category: str
    categories: list[str]
    image: str | None = None
    published_at: datetime | None = None


class TroveNewsList(BaseModel):
    items: list[TroveNewsItem]
    count: int


class TroveNewsHistory(BaseModel):
    items: list[TroveNewsItem]
    count: int                    # returned this page
    total: int                    # all archived articles (for paging)


# --- BetterTroveTools releases (drives in-app update checks) ----------------


class BttAsset(BaseModel):
    name: str                     # the asset filename
    url: str                      # the raw download URL (browser_download_url)
    size: int                     # bytes
    content_type: str | None = None
    download_count: int = 0


class BttReleaseMeta(BaseModel):
    """Release-level metadata WITHOUT the asset list - used inside per-platform views."""
    release_id: int
    tag_name: str                 # e.g. "v1.2.3"
    name: str                     # release title
    body: str                     # release notes (markdown)
    html_url: str                 # the GitHub release page
    prerelease: bool
    channel: str                  # "release" | "beta" (derived from prerelease)
    published_at: datetime
    fetched_at: datetime


class BttReleaseInfo(BttReleaseMeta):
    """Full release - meta + every asset on the release. Used in the listing endpoint."""
    assets: list[BttAsset]


class BttReleaseList(BaseModel):
    channel: str | None = None    # the channel filter, if any
    items: list[BttReleaseInfo]
    count: int                    # returned this page
    total: int                    # all matching releases (for paging)


class BttPlatformLatest(BaseModel):
    """The latest release that ships an asset for a given platform on a channel."""
    platform: str                 # windows | linux | android
    release: BttReleaseMeta
    assets: list[BttAsset]        # platform-matching assets only, sorted by priority


class BttLatestPerPlatform(BaseModel):
    channel: str                  # "release" | "beta"
    platforms: dict[str, BttPlatformLatest | None]  # one per known platform; null if none


class BttUpdateCheck(BaseModel):
    """Server-side "is there an update?" check - the client just reads `update_available`."""
    installed: str                # echoed back from the query
    channel: str                  # "release" | "beta"
    platform: str                 # windows | linux | android
    update_available: bool        # True iff `installed` is older than `latest.release.tag_name`
    comparable: bool              # False if either side couldn't be parsed as a version
    latest: BttPlatformLatest | None  # the latest release for this platform/channel, or null


class BttCommit(BaseModel):
    sha: str                      # full commit SHA
    short_sha: str                # first 7 chars (display id)
    message: str                  # first line of the commit message
    type: str | None = None       # conventional-commit prefix (feat/fix/docs/...) or null
    url: str                      # the GitHub commit page


class BttChangelogGroup(BaseModel):
    version: str                  # tag name (e.g. "v1.2.3") or "Unreleased"
    commits: list[BttCommit]


class BttChangelogOut(BaseModel):
    repo: str                     # "AallynReed/BetterTroveTools"
    groups: list[BttChangelogGroup]  # newest first; "Unreleased" leads when present
    rate_limited: bool            # True if the last fetch hit GitHub's rate limit
    fetched_at: datetime          # when the changelog was last (successfully) refreshed


# --- Events (Trovesaurus calendar) -----------------------------------------


class TroveEventItem(BaseModel):
    event_id: str
    name: str
    url: str
    category: str
    image: str | None = None
    icon: str | None = None
    lookup: str | None = None
    starts_at: int                # unix seconds (real UTC)
    ends_at: int
    status: str                   # "upcoming" | "ongoing" | "ended"
    seconds_until: int            # to start (upcoming), to end (ongoing), else 0


class TroveEventList(BaseModel):
    items: list[TroveEventItem]
    count: int


class EventCategoryList(BaseModel):
    categories: list[str]         # distinct categories currently in the store
    count: int


# --- Relayed feeds: Twitch / YouTube / Bilibili ----------------------------


class TwitchStream(BaseModel):
    channel: str                  # display name
    login: str                    # twitch.tv/<login>
    url: str
    title: str
    viewers: int
    game: str | None = None
    started_at: str | None = None
    thumbnail: str | None = None


class TwitchStreams(BaseModel):
    items: list[TwitchStream]
    count: int
    fetched_at: datetime | None = None


class Video(BaseModel):
    title: str
    url: str
    channel: str
    video_id: str | None = None
    published_at: str | None = None
    thumbnail_url: str | None = None


class Videos(BaseModel):
    items: list[Video]
    count: int
    fetched_at: datetime | None = None


# --- Stats: calculator stat tables -----------------------------------------


class StatSource(BaseModel):
    name: str                     # the source, e.g. "Dragons", "Hat", "Gems"
    value: float                  # how much this source contributes
    type: str                     # "slider" | "switch" (input kind in the calculator)
    percentage: bool = False      # value is a % bonus rather than a flat amount
    step: float | None = None     # slider increment, when the source is a stepped slider
    permanent: bool | None = None  # light table only: permanent vs temporary buff


class StatTable(BaseModel):
    stat: str                     # "power-rank" | "magic-find" | "light"
    label: str                    # human label, e.g. "Power Rank"
    sources: list[StatSource]
    count: int


# --- Stats: classes --------------------------------------------------------


class ClassStat(BaseModel):
    name: str                     # stat name, e.g. "Physical Damage"
    value: float | None           # base value (null when the class lacks the stat)
    percentage: bool              # whether the stat is a percentage


class AbilityStage(BaseModel):
    name: str
    base: float
    multiplier: float


class Ability(BaseModel):
    name: str
    icon: str
    type: str                     # "Passive" | "Active" | "Upgrade"
    stages: list[AbilityStage]


class Subclass(BaseModel):
    name: str
    description: str
    level: dict[str, list[ClassStat]]  # subclass level ("1".."30") -> stat bonuses
    power: dict[str, str]              # power milestone -> human-readable effect


class TroveClass(BaseModel):
    tech_name: str                # canonical token - reference a class by THIS
    name: str                     # display name
    shorts: list[str]             # abbreviations, e.g. ["BD"]
    damage_type: str              # "Physical" | "Magic"
    weapons: list[str]
    attributes: list[str]
    stats: list[ClassStat]        # base stats
    bonuses: list[ClassStat]      # class-gem bonus stats
    subclass: Subclass
    abilities: list[Ability]      # may be empty (only some classes carry ability data)


class ClassList(BaseModel):
    items: list[TroveClass]
    count: int


# --- Leaderboards ----------------------------------------------------------
# ``created_at`` is unix seconds anchored at 11:00 UTC (Trove's daily reset),
# matching the bot's dump cadence. ``contest_type`` is ``"daily"`` / ``"weekly"``
# / ``None``; ``reset_kind`` describes the BOARD (daily/weekly/default) while
# ``contest_type`` describes whether THIS dump captured a contest window.


class LeaderboardContest(BaseModel):
    time: int     # unix seconds - the anchor this contest was observed at
    type: str     # "daily" | "weekly"


class LeaderboardInfo(BaseModel):
    uuid: int
    name_id: str
    name: str
    category_id: str
    category: str
    contest_type: str | None = None    # contest type at the queried anchor
    reset_kind: str                    # "daily" | "weekly" | "default"
    player_board: bool                 # False for server-tally boards


class LeaderboardBoardOut(LeaderboardInfo):
    contests: list[LeaderboardContest] = []


class LeaderboardListOut(BaseModel):
    created_at: int
    items: list[LeaderboardInfo]
    count: int


class LeaderboardEntryOut(BaseModel):
    rank: int
    player_name: str
    score: float
    # Day-over-day change vs the previous comparable snapshot. All null when the
    # page isn't comparable (see ``LeaderboardEntriesPage.comparison``) or, for a
    # comparable page, when the player had no prior-day position (``is_new``).
    prev_rank: int | None = None
    prev_score: float | None = None
    rank_delta: int | None = None      # prev_rank - rank: positive = climbed
    score_delta: float | None = None   # score - prev_score: positive = gained
    is_new: bool = False


class LeaderboardComparison(BaseModel):
    """Whether this page carries day-over-day deltas, and against what snapshot.

    ``comparable`` is false when the board reset between the previous-day
    snapshot and this one (daily boards always; weekly across the Monday reset)
    or when there's no earlier snapshot stored yet.
    """
    comparable: bool
    prev_anchor: int | None = None     # the snapshot deltas were computed against
    reason: str                        # ok | no_prior_snapshot | crossed_reset


class LeaderboardEntriesPage(BaseModel):
    uuid: int
    created_at: int
    items: list[LeaderboardEntryOut]
    count: int
    total: int
    comparison: LeaderboardComparison


class LeaderboardTimestamps(BaseModel):
    items: list[int]
    count: int


class LeaderboardInsertResponse(BaseModel):
    """Acknowledgement that a dump was accepted for BACKGROUND processing.

    The parse + persist is deferred to a background task so the bot's HTTP
    client isn't held open through a multi-second insert (which was timing it
    out). The real counts (boards / entries / archived) land in the master
    ingest log when processing finishes; a parse/DB failure is recorded there
    too (success=false)."""
    accepted: bool = True
    bytes: int
    message: str


class LeaderboardPlayerEntry(LeaderboardEntryOut):
    leaderboard: int
    created_at: int


class LeaderboardPlayerHistory(BaseModel):
    player_name: str
    items: list[LeaderboardPlayerEntry]
    count: int


# --- History timeseries (charts) ----------------------------------------------
# Two endpoints drive the public leaderboards page charts: per-board (one
# line per top-player) and per-player (one line per board they appear on).
# Same shape both times - list of {created_at, rank, score} points sorted
# ascending - wrapped in a series object that carries the line's label.

class HistoryPoint(BaseModel):
    """One score sample at one anchor.

    ``synthetic`` distinguishes real captures (False - what the bot dumped)
    from server-injected reset boundary markers (True - see
    ``app/trove/leaderboards/service.py::_inject_reset_zeros``). On
    daily/weekly boards the service inserts a pair of synthetic points at
    each reset moment that falls between two real captures: a hold-flat
    point carrying the pre-reset score at R − 1s, then a zero point at R
    itself. This lets the chart show an honest cliff at the reset boundary
    instead of a misleading smooth descent across it. Renderers should
    suppress hover dots / tooltips on synthetic points but DO include them
    in the line path.
    """
    created_at: int
    rank: int
    score: float
    synthetic: bool = False


class BoardHistorySeries(BaseModel):
    """One line on the per-board chart - points are this player's scores
    on the board over the requested window, ascending."""
    player_name: str
    current_rank: int | None  # rank at the most-recent anchor in window
    points: list[HistoryPoint]


class BoardHistoryResponse(BaseModel):
    uuid: int
    days: int
    window_start: int   # unix seconds - inclusive lower bound
    window_end: int     # unix seconds - "now" at request time
    anchors: list[int]  # every distinct created_at in window (ascending)
    series: list[BoardHistorySeries]


class PlayerHistorySeriesItem(BaseModel):
    """One line on the per-player chart - points are this player's
    scores on one board they appear on, over the requested window."""
    uuid: int
    name: str
    best_rank: int
    points: list[HistoryPoint]


class PlayerHistorySeriesResponse(BaseModel):
    player_name: str
    canonical_name: str
    days: int
    window_start: int
    window_end: int
    anchors: list[int]
    series: list[PlayerHistorySeriesItem]


# --- Cheater detection ----------------------------------------------------
# The /v1/leaderboards/cheaters endpoint flags players with statistically
# anomalous scores. The full methodology lives in
# app/trove/leaderboards/detection.py - schemas here document the response
# shape for the OpenAPI surface.

class CheaterEvidence(BaseModel):
    """One piece of evidence - produced by ONE of the three checks."""
    type: Literal["score_outlier", "rank_gap", "velocity_outlier"]
    summary: str           # human-readable interpretation, safe to render verbatim
    measurements: dict     # type-specific numbers: raw value + peer baseline + magnitude
    confidence: float      # per-evidence confidence in [0.5, 0.99]; sigmoid on magnitude/threshold


class CheaterBoardEntry(BaseModel):
    """A board on which a player was flagged. Carries the player's
    current standing on it plus the evidence list."""
    uuid: int
    name: str
    category: str
    contest_type: str | None
    rank: int
    score: float
    evidence: list[CheaterEvidence]
    confidence: float = 0.0  # per-board confidence (max of evidence confidences on this board)


class CheaterPlayer(BaseModel):
    player_name: str
    leaderboards: list[CheaterBoardEntry]
    confidence: float  # overall confidence in [0.0, 1.0] - max-within-board, noisy-OR across boards


class CheatersConfig(BaseModel):
    """The thresholds the analysis was run with. Surfaced so the caller
    can correlate a flag to the active config and reproduce locally."""
    z_threshold: float
    velocity_multiplier: float
    min_board_size: int


class ActivityBoardCount(BaseModel):
    uuid: int
    name: str
    category: str
    active_players: int  # distinct players with score increase on THIS board in the window


class ActivityResponse(BaseModel):
    """Estimated active players via leaderboard score deltas."""
    window_start: int | None  # unix seconds - earlier of the two anchors
    window_end: int | None    # unix seconds - later (more recent) anchor
    duration_hours: float | None
    estimate: int | None      # distinct active players (union across all tracked boards); None = data unavailable
    by_board: list[ActivityBoardCount]
    boards_analyzed: int
    methodology: str
    computed_at: int


class ActivityHistoryPoint(BaseModel):
    """One point on the activity time-series - distinct active players
    in the window ``(window_start, window_end]``. Two numeric metrics:

    * ``estimate`` - raw count; what the per-window pill shows.
    * ``estimate_per_hour`` - count divided by window duration. This is
      the "flattened" metric: a missed-capture gap makes the next
      window span 2-3h instead of 1h, which naturally inflates the
      raw count because more players had time to score. Dividing
      restores the per-hour rate so the chart line stays smooth across
      irregular window sizes.
    """
    window_end: int        # late anchor, unix seconds
    window_start: int      # early anchor
    duration_hours: float
    estimate: int
    estimate_per_hour: float


class ActivityHistoryResponse(BaseModel):
    """Time-series of activity estimates over the last ``days`` days,
    one point per consecutive captured anchor pair. Sorted ascending by
    ``window_end``. Empty ``points`` when fewer than two captures have
    been stored - the consumer should hide the chart in that case."""
    days: int
    window_start: int
    window_end: int
    points: list[ActivityHistoryPoint]
    methodology: str


class TroveStatusResponse(BaseModel):
    """Live Trove server status from the background prober.

    ``overall`` is a rollup of the public Live regions - ``online`` when
    every region is up, ``down`` when all are fully down, ``maintenance``
    for any mixed/partial state (online / maintenance / down / unknown).
    ``auth`` is the shared HTTPS liveness of the account-auth gateway.
    ``environments`` carries a per-env dict (``eu`` / ``us`` / ``pts``),
    each ``{status, online, game}`` where ``game`` is the TCP probe of the
    glsserver port. Free-form dicts keep the wire stable as fields
    evolve."""
    overall: Literal["online", "maintenance", "down", "unknown"]
    auth: dict | None              # {online, http_status, latency_ms, error}
    environments: dict             # {eu: {status, online, game}, us: {...}, pts: {...}}
    checked_at: int | None         # unix seconds of the last completed probe


class TroveStatusHistoryResponse(BaseModel):
    """Status-timeline history for one environment over ``days`` days.

    ``segments`` is the continuous chain of status periods (clamped to the
    window); the current/open one has ``ended_at=null``. ``outages`` is
    the subset that isn't ``online``, for a readable incident log.
    ``uptime`` is the online fraction over the covered span (null when no
    data has been recorded yet)."""
    env: str
    days: int
    window_start: int
    window_end: int
    uptime: float | None
    covered_seconds: int
    segments: list[dict]
    outages: list[dict]


class AnalyzedBoardInfo(BaseModel):
    """One board the detector actually scanned in this run.

    Surfaced so the showcase-site cheaters tab (and any API caller) can
    show *which* boards the analysis covered, not just a count - the user
    can verify the board they care about was in scope and see its effective
    reset cadence + entry count without making a second request."""

    uuid: int
    name: str
    category: str
    reset_kind: str           # effective cadence: "daily" / "weekly" / "default" / "none"
    contest_type: str | None  # contest active on THIS anchor, if any
    entries: int              # how many entries the board had at this anchor


class SkippedBoardInfo(BaseModel):
    """One board the detector touched but did NOT analyze. ``reason``
    explains why so a UI can group / explain the omission."""

    uuid: int
    name: str
    category: str
    reset_kind: str
    contest_type: str | None
    # "admin_excluded" = listed in cheaters_excluded_board_uuids
    # "below_min_size" = entries < cheaters_min_board_size (sample too small)
    reason: Literal["admin_excluded", "below_min_size"]
    entries: int | None = None  # populated for "below_min_size", null otherwise


class CheatersResponse(BaseModel):
    players: list[CheaterPlayer]
    computed_at: int          # unix seconds when the analysis ran
    anchor: int | None        # capture timestamp the analysis is based on
    method: str               # short description of the methodology
    config: CheatersConfig    # active thresholds (echoed for transparency)
    total_flagged: int
    boards_analyzed: int
    boards_excluded: int = 0  # how many boards the operator opted out via cheaters_excluded_board_uuids
    # Detailed coverage - what the analysis actually touched. ``analyzed_boards``
    # carries one entry per scanned board; ``excluded_boards`` covers both
    # operator-excluded and below-min-size skips. Both lists sort by
    # (category, name) for stable rendering. Empty when no anchor available.
    analyzed_boards: list[AnalyzedBoardInfo] = []
    excluded_boards: list[SkippedBoardInfo] = []


# --- Market ---------------------------------------------------------------
# One ``MarketListingOut`` per in-game listing (UUID v1 from the game is the id).
# ``expired`` is True when the listing is past its 7-day lifetime OR hasn't been
# re-scraped for >3h; the read endpoints hide these by default and surface them
# only when the caller opts in.


class MarketListingOut(BaseModel):
    id: str          # the game's UUID v1, stringified
    name: str
    type: str | None
    stack: int
    price: int
    price_each: float
    last_seen: int   # unix seconds (real UTC)
    created_at: int  # unix seconds (real UTC) - decoded from the UUID
    expires_at: int  # created_at + 7d
    expired: bool


class MarketListingsPage(BaseModel):
    items: list[MarketListingOut]
    count: int      # rows returned in this page
    total: int      # rows matching the filter (independent of pagination)


class MarketItemList(BaseModel):
    items: list[str]
    count: int


class MarketItemSummary(BaseModel):
    name: str
    count: int          # number of active listings
    total_price: int    # sum of `price` across active listings
    total_stack: int    # sum of `stack`
    min_each: float
    max_each: float
    avg_each: float
    median_each: float


class MarketInsertResponse(BaseModel):
    parsed: int                # rows the parser recognised
    imported: int              # rows persisted (matched the interest list)
    ignored_not_in_list: int   # parsed but not in the interest list
    last_seen: int | None      # the anchor stamped on every persisted row


# --- Captured rotations (chaos chest + hourly challenge) -------------------
# Both share a "POST a name, GET current + history" shape. The server computes
# the time anchor; the bot just sends the captured display name.


class CaptureInsertRequest(BaseModel):
    name: str  # the captured item / challenge display name (verbatim from cfg)


class ChaosChestCaptureOut(BaseModel):
    week_anchor: int     # unix seconds at Tue 11:00 UTC (the week's start)
    week_ends_at: int    # week_anchor + 7 days
    name: str
    captured_at: datetime


class ChaosChestHistoryPage(BaseModel):
    items: list[ChaosChestCaptureOut]
    count: int
    total: int


class ChallengeWindow(BaseModel):
    """Pure window math (no name) - the shape ``server_time.challenge_window`` returns."""
    starts_at: int
    ends_at: int
    active: bool
    is_friday_window: bool
    seconds_remaining: int


class ChallengeCurrentOut(ChallengeWindow):
    """The current window + the captured challenge for it, if any.

    ``name`` is None when the bot hasn't reported this window yet (or during
    the gap between two windows - though ``starts_at`` / ``ends_at`` still
    describe the most-recent window in that case). ``type`` mirrors that
    nullability since it's derived from ``name``.
    """
    name: str | None = None
    type: ChallengeType | None = None
    captured_at: datetime | None = None


class ChallengeCaptureOut(BaseModel):
    window_anchor: int
    window_ends_at: int
    name: str
    # Server-side classification of ``name`` (collection/rampage/racing/target/dungeon)
    # so consumers don't have to maintain the special-case table themselves.
    type: ChallengeType
    is_friday_window: bool
    captured_at: datetime


class ChallengeHistoryPage(BaseModel):
    items: list[ChallengeCaptureOut]
    count: int
    total: int


class CaptureInsertResponse(BaseModel):
    """Shared response shape for the two master-only ingest endpoints."""
    anchor: int      # the time anchor the server inferred from "now"
    name: str        # the value persisted (post-trim)
    refreshed: bool  # False on first sighting of this anchor, True on re-submit


# --- Feedback (POST /v1/misc/feedback) -------------------------------------
# Endpoint accepts multipart/form-data - Form() + UploadFile validation
# happens at the FastAPI layer (see router.post_feedback). FeedbackCategory
# stays here as the documented enum surface.

FeedbackCategory = Literal["bug", "feature", "general"]


class FeedbackAck(BaseModel):
    """Response to a successful feedback submission - minimal on purpose
    (no echo of the message, no internal id). Empty 200-OK was tempting
    but a client UI usually wants SOMETHING to confirm the write."""
    ok: bool = True
    received_at: datetime

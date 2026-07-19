from datetime import datetime
from typing import Literal

from pydantic import BaseModel, Field

# Category a captured challenge falls into; see app.trove.captures.classify_challenge.
# Field named ``type`` (not ``kind``) to stay compatible with the old API's consumers.
ChallengeType = Literal["collection", "rampage", "racing", "target", "dungeon"]

# --- Server time -----------------------------------------------------------


class ServerTime(BaseModel):
    now_unix: int
    now_iso: str
    trove_day: str
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
    starts_at: int
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
    name: str
    identifier: str | None = None  # game path identifier (forward-slashed)
    blueprint: str | None = None   # blueprint path (lowercased)


class ChaosChest(BaseModel):
    active: bool                  # within the current weekly window
    starts_at: int
    ends_at: int
    seconds_remaining: int        # to the window end
    item: ChaosChestItem | None   # None if upstream unavailable
    fetched_at: datetime | None


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
    starts_at: int                # now − 365d, unix seconds
    ends_at: int                  # now + 365d, unix seconds
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
    current_week: int             # the live week id
    items: list[DelveWeekInfo]    # available weeks, newest first
    count: int


class DelveRotationOut(BaseModel):
    week: int
    is_current: bool
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
    name: str
    url: str                      # browser_download_url
    size: int
    content_type: str | None = None
    download_count: int = 0


class BttReleaseMeta(BaseModel):
    """Release-level metadata WITHOUT the asset list - used inside per-platform views."""
    release_id: int
    tag_name: str
    name: str
    body: str                     # release notes (markdown)
    html_url: str
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
    latest: BttPlatformLatest | None


class BttCommit(BaseModel):
    sha: str
    short_sha: str                # first 7 chars (display id)
    message: str                  # first line of the commit message
    type: str | None = None       # conventional-commit prefix (feat/fix/docs/...) or null
    url: str


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
    categories: list[str]         # distinct categories in the store
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


# --- Stats: Coefficient calculator (POST /v1/stats/coefficient) ------------
# Stateless: the in-game Coefficient (effective damage incl. crit) from a build's
# damage + crit-damage. Same formula the OCR extractor derives with.


class CoefficientRequest(BaseModel):
    physical_damage: float | None = Field(default=None, ge=0, description="Physical Damage stat")
    magic_damage: float | None = Field(default=None, ge=0, description="Magic Damage stat")
    critical_damage: float = Field(..., ge=0,
                                   description="Critical Damage as a percent (e.g. 3438.3 for 3,438.3%)")


class CoefficientResponse(BaseModel):
    coefficient: int = Field(..., description="floor(max(physical, magic) damage * (1 + critical_damage/100))")
    damage_used: Literal["physical", "magic"] = Field(..., description="Which damage stat was higher and used")
    formula: str = Field(..., description="The exact formula applied")


# --- Stats: classes --------------------------------------------------------


class ClassStat(BaseModel):
    name: str                     # e.g. "Physical Damage"
    value: float | None           # base value (null when the class lacks the stat)
    percentage: bool


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


class BoardHealthResponse(BaseModel):
    """Health summary for one board, from its latest snapshot vs the previous
    trove-day. Turnover + score inflation are day-over-day (null when the two
    snapshots aren't comparable, e.g. a daily board resets between them);
    competitiveness is a same-snapshot concentration measure."""
    uuid: int
    name: str | None = None
    category: str | None = None
    reset_kind: str | None = None
    anchor: int                              # latest snapshot analysed
    prev_anchor: int | None = None           # the day-over-day comparison snapshot
    comparable: bool                         # False when a reset crossed / no prior
    comparison_reason: str | None = None
    population: int                          # total entries on the board at ``anchor``
    sample_size: int                         # top-N actually analysed
    # Competitiveness (same-snapshot concentration of the top-N scores):
    leader_share: float | None = None        # #1 score / Σ(top-N)
    p1_pn_ratio: float | None = None         # #1 score / last-in-sample score
    gini: float | None = None                # 0 even … →1 one player dominates
    # Roster turnover + score inflation (day-over-day; null when not comparable):
    turnover_rate: float | None = None       # new top-N entrants / sample
    new_entrants: int | None = None
    median_score_gain: float | None = None
    median_score_gain_pct: float | None = None


class PlayerProfileSummary(BaseModel):
    boards_appeared: int                     # distinct leaderboards ever appeared on
    appearances: int                         # total capture rows (kept for back-compat)
    best_rank: int | None = None
    best_rank_board_uuid: int | None = None
    best_rank_board_name: str | None = None
    top10_count: int | None = None           # boards with a best rank in the top 10
    top100_count: int | None = None          # boards with a best rank in the top 100
    latest_anchor: int | None = None         # most recent capture the player was in


class PlayerBoardSummary(BaseModel):
    """One leaderboard the player has appeared on, aggregated over all history:
    their best rank ever, current rank/score, how many captures they've been in,
    and first/last seen. (Replaces the old per-capture flat list on the page.)"""
    leaderboard: int
    board_name: str | None = None
    best_rank: int
    latest_rank: int | None = None
    latest_score: float | None = None
    appearances: int                         # captures on THIS board
    first_seen: int | None = None
    last_seen: int | None = None


class PlayerProfileEntry(BaseModel):
    player_name: str
    leaderboard: int
    board_name: str | None = None
    rank: int
    score: float
    created_at: int
    prev_rank: int | None = None
    prev_score: float | None = None
    rank_delta: int | None = None
    score_delta: float | None = None
    is_new: bool | None = None


class PlayerProfileResponse(BaseModel):
    """Public player profile: leaderboard appearances + a verified-claim flag.
    Powers the /player/<name> page and the Discord bot's rank link."""
    player_name: str
    verified: bool                           # a site account claimed + was approved
    summary: PlayerProfileSummary
    boards: list[PlayerBoardSummary] = []    # one row per leaderboard, best first
    recent: list[PlayerProfileEntry]         # flat per-capture rows (legacy/back-compat)


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
    """One piece of evidence from a per-player check. ``sustained_velocity`` is
    the week-long throughput check (gain since the weekly reset / hours elapsed
    vs peer p95); the others are single-snapshot / last-hour."""
    type: Literal["score_outlier", "rank_gap", "velocity_outlier", "sustained_velocity"]
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


class CheaterClusterBoard(BaseModel):
    """One board a cluster was detected on. Fields vary by detection method:
    name-stem clusters carry the score-range fields (``score_min`` …
    ``rank_max``); co-movement clusters carry ``matching_hours`` +
    ``avg_hourly_gain`` instead. The other set is ``null``."""
    uuid: int
    name: str
    category: str
    contest_type: str | None = None
    members: int             # accounts from this cluster present on THIS board
    member_names: list[str] = []  # those accounts' names (capped at 60), for display + the chart
    # Name-stem method: the near-score subset's spread on this board.
    score_min: float | None = None
    score_max: float | None = None
    spread: float | None = None   # relative score spread of the subset (0 = identical)
    rank_min: int | None = None
    rank_max: int | None = None
    # Co-movement method: how many hours the group matched here + the avg
    # matched hourly gain.
    matching_hours: int | None = None
    avg_hourly_gain: float | None = None


class CheaterCluster(BaseModel):
    """A coordinated multi-account ('alt army') cluster. Two detection methods,
    distinguished by ``method``:

    * ``"co_movement"`` - the PRIMARY, name-agnostic signal: accounts whose
      hourly score gains move in lockstep across the captures since the weekly
      reset (confidence from matching-hour count + group size).
    * ``"name_stem"`` - similarly-named accounts sitting at near-identical
      scores in one snapshot (confidence from score tightness + size + boards).
    * ``"both"`` - a co-moving group that ALSO shares a name stem (highest
      confidence; the two independent signals corroborate).

    The unit of suspicion is the group, not one account - so this is surfaced
    separately from the per-player ``players`` list."""
    stem: str                # shared name stem ("anana"); "" for pure co-movement
    label: str               # display label ("anana*" or "<name> +N")
    method: str = "name_stem"  # "co_movement" | "name_stem" | "schedule" | "both"
    # Which INDEPENDENT signals agreed on this group (fusion). The more, the
    # higher the confidence: "co_movement", "schedule", "name_stem", "footprint".
    corroborated_by: list[str] = []
    member_count: int        # distinct accounts in the group (union across boards)
    members: list[str]       # member names, capped (see members_truncated)
    members_truncated: int = 0  # accounts omitted from `members` beyond the cap
    board_count: int
    boards: list[CheaterClusterBoard]
    confidence: float        # [0.5, 0.97]
    summary: str             # human-readable interpretation, safe to render verbatim
    measurements: dict       # confidence sub-terms + raw inputs (method-specific)


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
    # Wider rollups: distinct active players over the last 24h / 7d (same
    # late anchor, earlier endpoint). None until enough history exists to
    # reach back that far. ``span_*_hours`` is the window actually covered
    # (shorter than the nominal 24/168 right after a fresh deploy).
    estimate_24h: int | None = None
    estimate_7d: int | None = None
    window_24h_start: int | None = None
    window_7d_start: int | None = None
    span_24h_hours: float | None = None
    span_7d_hours: float | None = None
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


class ActivitySeriesPoint(BaseModel):
    """One bucket on the multi-period activity chart. ``active`` is the
    average active-players-per-hour across the windows that fell in the
    bucket; ``peak`` is the busiest single window in it."""
    t: int            # bucket start, unix seconds
    active: float     # avg active players / hour in the bucket
    peak: float       # busiest hour in the bucket
    samples: int      # number of captures aggregated into the bucket


class ActivitySeriesPeak(BaseModel):
    t: int            # bucket start of the period's busiest bucket
    active: float


class ActivitySeriesResponse(BaseModel):
    """Bucketed activity-level series for one period (1d … all). Buckets
    are sized to the period so the line stays readable; ``peak`` /
    ``average`` / ``latest`` summarise it for stat cards. Empty ``points``
    when the estimate collection has nothing in range."""
    period: str
    bucket_seconds: int
    window_start: int
    window_end: int
    points: list[ActivitySeriesPoint]
    peak: ActivitySeriesPeak | None = None
    average: float | None = None
    latest: float | None = None
    methodology: str


# --- Class activity (per-class active players from Effort/Paragon boards) ----


class ClassActivityItem(BaseModel):
    class_index: int       # classes.json index (= Effort/Paragon board uuid % 1000)
    name: str              # class display name
    icon: str | None       # self-hosted icon URL (/static/class-icons/<qualified_name>.png)
    active_players: int     # RAW: players present on this class's Effort board (Paragon excluded)
    share: float            # 0..1, share of the snapshot's total players (NOT distinct players)
    # CLEAN view: same headcount filtered to players clearing the established floors
    # (Power Rank + Effort). ``null`` when unmeasurable for this class (the Power
    # Rank board is absent in the snapshot).
    active_players_clean: int | None = None
    share_clean: float | None = None
    # Effort ADDED to this class's leaderboard in the latest hour (this capture vs
    # the previous) - Σ positive per-player score gains. RAW = all players; CLEAN =
    # those clearing the established floors. ``null`` when unmeasurable (no previous
    # capture, or the pair crosses the weekly reset).
    effort_added: int | None = None
    effort_added_clean: int | None = None


class ClassActivityCurrentResponse(BaseModel):
    """Per-class player share from the LATEST leaderboard snapshot - a direct
    headcount, NOT the activity pipeline (no score-rose step).

    Two views: RAW (``active_players`` / ``share`` / ``total_active``) counts every
    player present on a class's Effort board at the newest capture (Paragon is
    excluded as ambiguous); CLEAN (``*_clean``), the "established" page default,
    keeps only players who clear ALL three floors - ``power_rank_threshold``
    (1000+i board), ``effort_threshold`` (4000+i) and ``xp_threshold`` (the global
    XP board, 21005) - filtering newbies + alts (a floor of 0 disables that gate).
    ``share`` is a class's count over the total
    across classes: a player on several classes is counted in each, so the total is
    Σ class counts (the share denominator), not a distinct headcount.
    ``window_start``/``window_end`` are both the snapshot anchor; ``duration_hours``
    is null (there's no window). The time-series endpoint stays activity-based."""
    window_start: int | None
    window_end: int | None
    duration_hours: float | None
    total_active: int | None
    total_active_clean: int | None = None
    # Total Effort added across all classes in the latest hour (raw / established);
    # null when unmeasurable (no previous capture, or pair crosses a weekly reset).
    total_effort_added: int | None = None
    total_effort_added_clean: int | None = None
    power_rank_threshold: int = 0
    effort_threshold: int = 0
    xp_threshold: int = 0
    classes: list[ClassActivityItem]
    methodology: str
    computed_at: int


class ClassActivitySeriesLine(BaseModel):
    class_index: int
    name: str
    icon: str | None            # self-hosted icon URL (/static/class-icons/<qualified_name>.png)
    values: list[float | None]  # RAW avg active/hr per bucket, aligned to `buckets`; null = no data
    # CLEAN (Power-Rank-filtered) avg active/hr per bucket, aligned to `buckets`;
    # null where that view had no measurable window in the bucket.
    values_clean: list[float | None] = []


class ClassActivitySeriesResponse(BaseModel):
    """Per-class bucketed series for the Class Activity chart. ``buckets`` is the
    shared x-axis (one timestamp per bucket); each line's ``values`` (raw) and
    ``values_clean`` (the clean/established view) align to it, with ``null`` where
    that class had no measurable window in the bucket. ``power_rank_threshold`` /
    ``effort_threshold`` / ``xp_threshold`` are the current clean-view floors (for
    display; the XP floor reads the global XP board, 21005, 0 = off)."""
    period: str
    bucket_seconds: int
    window_start: int
    window_end: int
    power_rank_threshold: int = 0
    effort_threshold: int = 0
    xp_threshold: int = 0
    buckets: list[int]
    classes: list[ClassActivitySeriesLine]
    methodology: str


class TroveStatusResponse(BaseModel):
    """Live Trove server status from the background prober.

    Status is binary: ``online`` (reachable) or ``down`` (anything
    unreachable). ``overall`` is ``online`` only when every public Live region
    is online, else ``down`` (consumers read full-vs-partial from the per-region
    detail). ``auth`` is the shared HTTPS liveness of the account-auth gateway.
    ``environments`` carries a per-env dict (``eu`` / ``us`` / ``pts``), each
    ``{status, online, game}`` where ``game`` is the glsserver probe. (The
    legacy ``maintenance`` value is still accepted on the wire so a stale cached
    snapshot validates, but the prober no longer emits it.)"""
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
    # Coordinated alt-clusters (group-shaped detection), most-suspicious-first.
    # Separate from `players`: the suspicious unit is the name-family, not one
    # account. Empty when no clusters found or no anchor available yet.
    clusters: list[CheaterCluster] = []
    # How many boards the alt-cluster pass actually scanned (not excluded by
    # cheaters_cluster_excluded_board_uuids, ≥ cheaters_cluster_min_size
    # entries). Distinct from boards_analyzed, which is gated by the per-player
    # cheaters_min_board_size + cheaters_excluded_board_uuids.
    clusters_boards_scanned: int = 0
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


# --- Player renames (reconstructed name changes) --------------------------
# Trove leaderboards carry no stable UID, so a rename is inferred from behaviour:
# a name that vanished between two adjacent captures (within renames_max_gap_seconds)
# while a NEW name appeared carrying the same lifetime-board score fingerprint.


class RenameEvent(BaseModel):
    """One detected rename ``from_name -> to_name`` across an adjacent capture
    pair. ``evidence`` carries the matched lifetime boards (score_from/score_to/
    drift), the confidence sub-terms, and a human-readable summary."""
    id: int
    from_name: str
    to_name: str
    from_anchor: int          # capture the old name was last seen in
    to_anchor: int            # capture the new name appeared in (the "detected at")
    confidence: float         # blended confidence in [0, 0.97]
    matched_boards: int       # lifetime boards the fingerprint matched on
    evidence: dict = {}
    method_version: int = 1
    created_at: int


class RenamesResponse(BaseModel):
    # Detected renames, MOST-RECENT-first (by the capture the new name appeared in).
    renames: list[RenameEvent] = []
    total: int = 0
    limit: int = 50
    offset: int = 0
    # False when the rename-detection feature flag is OFF (the tab hides itself).
    enabled: bool = True
    method_version: int = 1


class RenameHistoryResponse(BaseModel):
    """The full rename chain touching a name - edges walked in both directions so
    an identity that renamed several times (A→B→C) returns its whole timeline."""
    query: str
    current_name: str
    aliases: list[str] = []
    edges: list[RenameEvent] = []
    rename_count: int = 0


# --- Record highs (free "how high can these stats go" endpoint) -----------
# The current ceiling for Trove Mastery / Geode Mastery / Power Rank, read off
# the rank-1 holder of the relevant lifetime board(s). Mastery boards store a
# running POINTS total; ``level`` is that total run through the in-game
# points->level curve.


class MasteryRecord(BaseModel):
    points: int                     # rank-1's raw Mastery points total
    level: int                      # level for that total (capped, when soft-capped)
    points_into_level: int          # how far the total has climbed into `level`
    points_to_next_level: int       # points still owed to reach the next level
    player_name: str                # the current record holder
    anchor: int                     # snapshot the record is from (unix seconds)
    # Present only on soft-capped boards (Geode Mastery). `level` is clamped to
    # `level_cap`; `uncapped_level` is what it would be without the cap.
    level_cap: int | None = None
    uncapped_level: int | None = None
    capped: bool | None = None


class PowerRankRecord(BaseModel):
    value: int                      # highest Power Rank across all class boards
    board_uuid: int                 # which class board (1000-1016) it came from
    player_name: str
    anchor: int


class MasteryRecordsResponse(BaseModel):
    # Any field is null only if that board has never been ingested yet.
    trove_mastery: MasteryRecord | None = None
    geode_mastery: MasteryRecord | None = None
    power_rank: PowerRankRecord | None = None


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


# --- Store (in-game Kiwi Store catalog) -------------------------------------
# One ``StoreProductOut`` per product code. ``active`` = the product appeared
# in the latest scrape; delisted products stay stored (history) but drop out
# of the default read view. Prices are the in-game currencies (TWC credits /
# TWP cubits); real-money SKUs carry a pre-formatted ``price_string`` instead.


class StorePriceOut(BaseModel):
    currency: str            # "TWC" (credits) / "TWP" (cubits) / real-money ISO code
    # In-game currencies (TWC/TWP) are whole units; real-money currencies (EUR,
    # USD, ...) are in MINOR units (cents) exactly as the game reports them -
    # divide by 100 for a display price (149 EUR -> €1.49).
    cost: float
    can_purchase: bool       # engine-side purchasability at capture time
    monthly: int = 0         # patron: per-month price; 0 otherwise
    sale: str = ""           # sale-sticker loc key suffix ("" = no sale)


class StoreTextureOut(BaseModel):
    texture: str
    x: int = 0
    y: int = 0
    text: str = ""           # overlay text ("" = none)
    overlay: bool = False


class StorePricePointOut(BaseModel):
    ts: int                  # unix seconds (the ingest anchor that saw the change)
    prices: list[StorePriceOut]
    price_string: str = ""


class StoreProductOut(BaseModel):
    code: str                # the engine's stable product code
    kind: str                # product|starter|patron|interactable|trial|class
    name: str                # raw loc key or display name as the engine sent it
    image: str
    info: str
    informational: bool
    tradable: bool
    prices: list[StorePriceOut]
    price_string: str | None          # real-money SKUs (pre-formatted)
    price_string_currency: str | None
    price_string_sale: str | None
    promo: str | None
    deal_expires_at: int | None       # unix seconds (limited-time deals)
    interact_label: str | None
    interact_enabled: bool
    trial_limits: str | None
    class_level: int | None           # class tiles only
    class_power_rank: int | None
    class_sub_name: str | None
    class_icon: str | None
    textures: list[StoreTextureOut]
    loot_title: str | None
    loot_body: str | None             # lootbox probability text
    categories: list[int]             # store-tab indices this code appears under
    first_seen: int                   # unix seconds
    last_seen: int                    # unix seconds
    active: bool                      # present in the latest scrape


class StoreRecords(BaseModel):
    times_available: int          # number of distinct continuous runs in-store
    returns: int                  # times it came back after leaving (runs - 1)
    total_days_seen: int          # summed days present across all runs
    longest_run_days: int
    first_seen: int
    last_seen: int
    currently_active: bool
    gap_days: int | None          # whole days since last present (null if active)
    price_low: dict[str, float] | None   # cheapest each currency has ever been
    price_high: dict[str, float] | None  # priciest each currency has ever been
    price_changes: int


class StoreProductDetail(StoreProductOut):
    price_history: list[StorePricePointOut]
    # Availability intervals [[start_anchor, end_anchor], ...] (unix seconds) -
    # one per continuous run the product was present; drives the detail timeline.
    availability: list[list[int]]
    records: StoreRecords


class StoreTimelineItem(BaseModel):
    code: str
    name: str
    kind: str
    image: str | None
    availability: list[list[int]]   # [[start_anchor, end_anchor], ...] unix seconds
    first_seen: int
    last_seen: int
    active: bool


class StoreTimelineResponse(BaseModel):
    anchor: int | None                     # latest ingest anchor (unix seconds)
    span: dict[str, int]                   # {start, end} axis bounds (unix seconds)
    items: list[StoreTimelineItem]
    count: int


class StoreProductsPage(BaseModel):
    items: list[StoreProductOut]
    count: int          # rows returned in this page
    total: int          # rows matching the filter (independent of pagination)
    anchor: int | None  # latest ingest anchor (unix seconds)


class StoreCategoryOut(BaseModel):
    index: int
    label: str           # raw loc key (e.g. "$Store_Tab_Featured")
    icon: str | None
    codes: list[str]     # product codes in display order
    count: int
    active: bool         # present in the latest scrape


class StoreCategoriesResponse(BaseModel):
    items: list[StoreCategoryOut]
    count: int
    anchor: int | None


class StoreInsertResponse(BaseModel):
    products: int        # products the parser recognised
    categories: int      # categories the parser recognised
    created: int         # products seen for the first time
    price_changes: int   # products whose price signature changed
    anchor: int          # unix seconds stamped on every persisted doc
    done_marker: bool    # the bot's `done = true` line was present


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


# --- Character-stat OCR (POST /v1/ocr/character) ---------------------------
# Self-hosted OCR of the in-game character stat sheet. Labels are matched
# against a closed multilingual vocabulary and each value sanity-checked, so a
# garbled or translated label still resolves and an implausible read is flagged
# rather than silently returned. See app/trove/ocr/.


class OcrStatValue(BaseModel):
    """One recognized stat. ``value`` is an integer for count stats and a float
    for percent stats. ``in_range`` / ``type_match`` flag a read that parsed but
    looks implausible for this stat (e.g. a percent value on an integer stat);
    ``confidence`` (0-1) folds label-match quality with those checks."""
    value: int | float = Field(..., description="Numeric value; int for counts, float for percents")
    unit: Literal["count", "percent"]
    raw: str = Field(..., description="The number exactly as OCR read it (pre-normalization); empty when derived")
    confidence: float = Field(..., ge=0.0, le=1.0)
    in_range: bool = Field(..., description="Value sits within the stat's plausible range")
    type_match: bool = Field(..., description="Value's percent/integer kind matches the stat")
    derived: bool = Field(default=False, description="True if computed rather than read - Coefficient = floor(max(physical,magic) damage * (1 + critical_damage/100)) when it isn't on the sheet")


class CharacterStatsOcr(BaseModel):
    """Stats recognized from a character-sheet screenshot, keyed by canonical
    stat key (e.g. ``physical_damage``, ``critical_hit``)."""
    stats: dict[str, OcrStatValue]
    matched: int = Field(..., description="How many known stats were recognized")
    total_known: int = Field(..., description="Size of the known-stat vocabulary")
    lines: list[str] = Field(..., description="Raw text lines the OCR produced (transparency)")

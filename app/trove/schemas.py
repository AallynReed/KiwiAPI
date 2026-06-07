from datetime import datetime
from typing import Literal

from pydantic import BaseModel, Field

# Category a captured challenge falls into. Computed from the raw display name
# by ``app.trove.captures.classify_challenge`` so consumers don't have to
# memorise which strings are special-cased; see that helper for the rules.
# Field name ``type`` shadows the Python builtin at attribute level but
# Pydantic handles it fine — chose ``type`` because the old API used
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
    """Release-level metadata WITHOUT the asset list — used inside per-platform views."""
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
    """Full release — meta + every asset on the release. Used in the listing endpoint."""
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
    """Server-side "is there an update?" check — the client just reads `update_available`."""
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
    tech_name: str                # canonical token — reference a class by THIS
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
    time: int     # unix seconds — the anchor this contest was observed at
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


class LeaderboardEntriesPage(BaseModel):
    uuid: int
    created_at: int
    items: list[LeaderboardEntryOut]
    count: int
    total: int


class LeaderboardTimestamps(BaseModel):
    items: list[int]
    count: int


class LeaderboardInsertResponse(BaseModel):
    boards: int
    entries: int
    cleared_before_insert: int
    archived_old: int   # rows moved hot → archive at the tail of this insert
    created_at: int | None


class LeaderboardPlayerEntry(LeaderboardEntryOut):
    leaderboard: int
    created_at: int


class LeaderboardPlayerHistory(BaseModel):
    player_name: str
    items: list[LeaderboardPlayerEntry]
    count: int


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
    created_at: int  # unix seconds (real UTC) — decoded from the UUID
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
    """Pure window math (no name) — the shape ``server_time.challenge_window`` returns."""
    starts_at: int
    ends_at: int
    active: bool
    is_friday_window: bool
    seconds_remaining: int


class ChallengeCurrentOut(ChallengeWindow):
    """The current window + the captured challenge for it, if any.

    ``name`` is None when the bot hasn't reported this window yet (or during
    the gap between two windows — though ``starts_at`` / ``ends_at`` still
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
# Endpoint accepts multipart/form-data — Form() + UploadFile validation
# happens at the FastAPI layer (see router.post_feedback). FeedbackCategory
# stays here as the documented enum surface.

FeedbackCategory = Literal["bug", "feature", "general"]


class FeedbackAck(BaseModel):
    """Response to a successful feedback submission — minimal on purpose
    (no echo of the message, no internal id). Empty 200-OK was tempting
    but a client UI usually wants SOMETHING to confirm the write."""
    ok: bool = True
    received_at: datetime

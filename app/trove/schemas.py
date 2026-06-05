from datetime import datetime

from pydantic import BaseModel

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

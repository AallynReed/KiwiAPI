"""Request/response models for the gem endpoints.

The `Gem` model itself (model.py) is the round-trip object - clients post back
exactly what `generate` returned. These wrap it with action parameters and shape
the evaluator/builds outputs.
"""

from pydantic import BaseModel, Field

from .model import Gem

# --- Simulator: generate + actions -----------------------------------------


class GenerateGemRequest(BaseModel):
    tier: int | None = None          # 1-4; random if omitted
    type: int | None = None          # 1 Lesser, 2 Empowered; random if omitted
    element: int | None = None       # 1-4; random if omitted
    restriction: int | None = None   # Lesser only: 1 Fierce, 2 Arcane
    level: int = 1
    augmentation: float | None = None  # seed every container's base roll (0-1)


class GemActionResult(BaseModel):
    applied: bool                    # False if the action was a no-op (e.g. stat already maxed)
    gem: Gem


class AugmentRequest(BaseModel):
    gem: Gem
    stat_position: int = Field(ge=0, description="0, 1 or 2 - which stat to augment")
    augment_type: int = 1            # 1 Rough, 2 Precise, 3 Superior


class StatPositionRequest(BaseModel):
    gem: Gem
    stat_position: int = Field(ge=0, description="0, 1 or 2")


class LevelUpRequest(BaseModel):
    gem: Gem


class SetLevelRequest(BaseModel):
    gem: Gem
    level: int = Field(ge=1)


# --- Reference / lookups ---------------------------------------------------


class LookupItem(BaseModel):
    id: int
    name: str


class TierLookup(BaseModel):
    id: int
    name: str
    max_level: int


class AugmentLookup(BaseModel):
    id: int
    name: str
    increase_percent: float


class GemLookups(BaseModel):
    tiers: list[TierLookup]
    types: list[LookupItem]
    elements: list[LookupItem]
    restrictions: list[LookupItem]
    stat_types: list[LookupItem]
    augment_types: list[AugmentLookup]
    abilities: list[LookupItem]
    abilities_by_element: dict[str, list[int]]


# --- Evaluator -------------------------------------------------------------


class EvalStatInput(BaseModel):
    type: int                        # stat type id (1-7)
    value: float                     # the value shown in-game
    extra_containers: int = 0        # procs on this stat (0-3)


class EvaluateRequest(BaseModel):
    tier: int = 4
    type: int = 1
    level: int = 1
    stats: list[EvalStatInput]       # exactly 3
    auto_guess_procs: bool = False   # let the API infer the proc spread


class FocusStrategyCost(BaseModel):
    key: str
    label: str
    superior: int
    precise: int
    rough: int
    total: int
    recipe_totals: dict[str, int]    # material key -> quantity to perfect the gem


class EvalStat(BaseModel):
    type: int
    display_name: str
    entered_value: float
    extra_containers: int
    containers: int
    progress: float                  # 0-1 (clamped)
    quality_percent: float
    estimated_pr_contribution: float
    is_within_range: bool
    raw_progress: float              # unclamped - <0 or >1 flags an impossible value
    threshold_progress: float


class EvaluationResult(BaseModel):
    tier: int
    type: int
    type_name: str
    element: int
    element_name: str
    restriction: int | None
    restriction_name: str
    level: int
    quality: float
    quality_percent: float
    calculated_power_rank: int
    has_issues: bool
    issues: list[str]
    stats: list[EvalStat]
    focus_totals: dict[str, FocusStrategyCost]
    headline_cost: FocusStrategyCost  # cheapest realistic path (precise + rough)
    available_extra_containers: int
    guessed_distribution: list[int]


class SimpleEvaluateRequest(BaseModel):
    tier: int = 4
    type: int = 1
    power_rank: int = 0
    level: int = 1


class SimpleEvaluationResult(BaseModel):
    tier: int
    tier_name: str
    type: int
    type_name: str
    level: int
    power_rank: int
    min_power_rank: int
    max_power_rank: int
    quality: float
    quality_percent: float
    is_within_range: bool           # True when the entered PR sits in the plausible band
    distance: int                   # how far outside the band the PR is (0 when within)
    focus_totals: dict[str, FocusStrategyCost]
    headline_cost: FocusStrategyCost


class GemStatRange(BaseModel):
    stat_type: int
    stat_display_name: str
    element: int
    containers: int
    min_value: float
    max_value: float
    stat_base: float
    thresholds: list[float]


# --- Builds ----------------------------------------------------------------


class BuildConfigRequest(BaseModel):
    build_type: str = "Light"        # "Light" | "Farm" | "Health"
    character: str = "Bard"          # class display name
    subclass: str = "Boomeranger"
    food: str = ""                   # food key (see /builds/options) or ""
    ally: str = "boot_clown"         # ally key, or "boot_clown" for no ally (stats are level 30)
    ally_buff: bool = True           # Blessing of the Lilypad, applied to the ally's stats
    berserker_battler: bool = False
    critical_damage_count: int = Field(default=3, ge=0, le=3)
    no_face: bool = False
    light: int = 0                   # Farm only: target base light (0 disables targeting)
    subclass_active: bool = False
    litany: bool = False
    star_chart: str | None = None    # optional star-chart build code


class BuildResult(BaseModel):
    rank: int
    layout: str                      # proc layout, e.g. "0/9/0/9 + 0/0/3 + 0/0/6"
    base_dmg: float
    crit_dmg: float
    light: int
    bonus_dmg: float
    total_dmg: float
    class_bonus: float | None
    coefficient: float               # the ranking metric (higher = more damage)


class BuildResponse(BaseModel):
    results: list[BuildResult]
    count: int


class OptionEntry(BaseModel):
    key: str
    label: str


class CritRange(BaseModel):
    min: int
    max: int
    default: int


class BuildOptions(BaseModel):
    build_type: list[str]
    character: list[str]             # valid for both character and subclass
    ally: list[OptionEntry]
    food: list[OptionEntry]
    critical_damage_count: CritRange
    flags: list[str]                 # boolean config fields
    notes: dict[str, str]

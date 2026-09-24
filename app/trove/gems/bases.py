"""Gem numbers: stat rolls, the level schedule, level-up odds and costs, focuses and
boosters come from the game files (gem_upgrades.json, app/trove/decode/gem_upgrades.py).
Power Rank is not in the game files and stays as BetterTroveTools had it.

The model keeps its original form, value = base * (threshold * containers + PR
increments): base is a stat's per-level step over its tier's Power Rank per step, so
base * threshold is exactly the game's roll range and base * increments its level gain.
"""

from app.trove.decode import store as gamedata

from .constants import AugmentType, GemElement, GemStatType, GemTier, GemType

GAMEDATA = "gem_upgrades.json"

# Power Rank each stat step adds, and the PR a container's roll spans.
_TIER_PR_BASE = {GemTier.RADIANT: 3, GemTier.STELLAR: 5, GemTier.CRYSTAL: 7, GemTier.MYSTIC: 9}
_LESSER_PR_THRESHOLD = {GemTier.RADIANT: [85, 113], GemTier.STELLAR: [150, 200],
                        GemTier.CRYSTAL: [175, 250], GemTier.MYSTIC: [200, 260]}
_EMPOWERED_PR_THRESHOLD = {GemTier.RADIANT: [113, 150], GemTier.STELLAR: [200, 266],
                           GemTier.CRYSTAL: [220, 280], GemTier.MYSTIC: [240, 300]}

# The t<N> of a tier's gem item files (large gems add 100), and each element's colour.
_FILE_TIER = {GemTier.RADIANT: 9, GemTier.STELLAR: 10, GemTier.CRYSTAL: 11, GemTier.MYSTIC: 12}
_COLOR = {GemElement.WATER: "blue", GemElement.FIRE: "red", GemElement.AIR: "yellow", GemElement.COSMIC: "opal"}
_STAT = {
    GemStatType.PHYSICAL_DAMAGE: ("PhysicalDamage", False), GemStatType.MAGIC_DAMAGE: ("SpellDamage", False),
    GemStatType.CRITICAL_DAMAGE: ("CriticalHitDamage", False), GemStatType.CRITICAL_HIT: ("CriticalHitChance", False),
    GemStatType.MAX_HEALTH: ("MaxHealth", False), GemStatType.MAX_HEALTH_BONUS: ("MaxHealth", True),
    GemStatType.LIGHT: ("Light", False),
}
_FOCUS = {AugmentType.ROUGH: "item/gem/booster/augment1", AugmentType.PRECISE: "item/gem/booster/augment2",
          AugmentType.SUPERIOR: "item/gem/booster/augment3"}


@gamedata.cached(GAMEDATA)
def _data() -> dict:
    data = gamedata.load(GAMEDATA, {}) or {}
    index = {(i["size"], i["color"], i["tier"]): g for g in data.get("gems", []) for i in g["items"]}
    return {**data, "index": index}


def upgrade_data(gem_tier: GemTier, gem_type: GemType, gem_element: GemElement) -> dict:
    """The game's upgrade record for this kind of gem."""
    tier = _FILE_TIER[GemTier(gem_tier)]
    size, tier = ("small", tier) if GemType(gem_type) == GemType.LESSER else ("large", tier + 100)
    found = _data()["index"].get((size, _COLOR[GemElement(gem_element)], tier))
    if found is None:
        raise ValueError(f"no game data for {GemType(gem_type).display_name} {GemTier(gem_tier).display_name} gems")
    return found


def item_name(item: str) -> str:
    return _data().get("names", {}).get(item) or item.rsplit("/", 1)[-1]


def element_materials(gem_element: GemElement) -> dict:
    """The element's dust and amber ("Gem Spark") items."""
    return _data().get("materials", {}).get(_COLOR[GemElement(gem_element)], {})


def boosters() -> list[dict]:
    """Level-up boosters: {id, item, name, chance_multiplier, double_multiplier}."""
    return [{"id": b["item"].rsplit("/", 1)[-1], "name": item_name(b["item"]), **b}
            for b in _data().get("boosters", [])]


def booster(booster_id: str) -> dict:
    found = next((b for b in boosters() if b["id"] == booster_id), None)
    if found is None:
        raise ValueError(f"unknown booster {booster_id!r}")
    return found


def get_gem_max_level(gem_tier: GemTier, gem_type: GemType) -> int:
    return upgrade_data(gem_tier, gem_type, GemElement.WATER)["max_level"]


def level_attempt(gem_tier: GemTier, gem_type: GemType, gem_element: GemElement, level: int) -> dict | None:
    """The game's record for the attempt that reaches ``level`` (None for level 1 or past max)."""
    return next((lv for lv in upgrade_data(gem_tier, gem_type, gem_element)["levels"] if lv["level"] == level), None)


def attempt_odds(attempt: dict, boost: dict | None = None) -> tuple[float, float]:
    """(level-up chance, double level-up chance) of one attempt, a booster applied."""
    chance = attempt["chance"] * (boost["chance_multiplier"] if boost else 1)
    double = attempt["double_chance"] * (boost["double_multiplier"] if boost else 1)
    return min(1.0, chance), min(1.0, double)


def attempt_cost(gem_element: GemElement, attempt: dict, boost: dict | None = None) -> list[dict]:
    """What one attempt spends, landed or not: {item, name, count}."""
    mats = element_materials(gem_element)
    rows = [(c["item"], c["count"]) for c in attempt["cost"]]
    if attempt["dust"] and mats.get("dust"):
        rows.append((mats["dust"], attempt["dust"]))
    if attempt["amber"] and mats.get("amber"):
        rows.append((mats["amber"], attempt["amber"]))
    if boost:
        rows.append((boost["item"], 1))
    return [{"item": item, "name": item_name(item), "count": count} for item, count in rows]


def level_plan(gem_tier: GemTier, gem_type: GemType, gem_element: GemElement,
               booster_id: str | None = None, from_level: int = 1) -> dict:
    """Every attempt from ``from_level`` to max, and the expected total spend.

    Expected attempts at a level are 1 / chance; double level-ups are left out, so
    the total leans slightly high."""
    boost = booster(booster_id) if booster_id else None
    rows, expected = [], {}
    for lv in upgrade_data(gem_tier, gem_type, gem_element)["levels"]:
        if lv["level"] <= from_level:
            continue
        chance, double = attempt_odds(lv, boost)
        cost = attempt_cost(gem_element, lv, boost)
        attempts = 1 / chance if chance > 0 else 0
        for c in cost:
            expected[c["name"]] = expected.get(c["name"], 0) + c["count"] * attempts
        rows.append({"level": lv["level"], "chance": chance, "double_chance": double,
                     "expected_attempts": round(attempts, 2), "cost": cost})
    return {"levels": rows, "expected_total": [{"name": k, "count": round(v)} for k, v in expected.items()]}


def boost_levels(gem_tier: GemTier, gem_type: GemType) -> list[int]:
    """Levels that add a stat boost (a container) - 5, 10 and 15."""
    return [lv["level"] for lv in upgrade_data(gem_tier, gem_type, GemElement.WATER)["levels"] if lv["boost"]]


def boosts_at(gem_tier: GemTier, gem_type: GemType, level: int) -> int:
    return sum(1 for lv in boost_levels(gem_tier, gem_type) if lv <= level)


def _steps(gem_tier: GemTier, level: int) -> int:
    lv = level_attempt(gem_tier, GemType.LESSER, GemElement.WATER, level)
    return lv["stat_steps"] if lv else 0


def get_increment_power_rank_lesser(gem_tier: GemTier, level: int) -> int:
    return _TIER_PR_BASE[GemTier(gem_tier)] * _steps(gem_tier, level)


# Lesser and Empowered share the same per-level schedule.
get_increment_power_rank_empowered = get_increment_power_rank_lesser


def stat_roll(gem_tier: GemTier, gem_type: GemType, gem_element: GemElement,
              gem_stat_type: GemStatType) -> dict:
    """{min, max, step} of one container's roll, in display units."""
    st = GemStatType(gem_stat_type)
    key = _STAT.get(st)
    for row in upgrade_data(gem_tier, gem_type, gem_element)["stats"]:
        if key == (row["stat"], row["percent"]):
            return row
    raise ValueError(f"no stat base for {st.display_name} at {GemTier(gem_tier).display_name}")


def _base(gem_tier, gem_type, gem_element, gem_stat_type) -> float:
    return stat_roll(gem_tier, gem_type, gem_element, gem_stat_type)["step"] / _TIER_PR_BASE[GemTier(gem_tier)]


def _threshold(gem_tier, gem_type, gem_element, gem_stat_type) -> list[float]:
    roll = stat_roll(gem_tier, gem_type, gem_element, gem_stat_type)
    base = _base(gem_tier, gem_type, gem_element, gem_stat_type)
    return [round(roll["min"] / base, 9), round(roll["max"] / base, 9)]


def get_stat_base_lesser(gem_tier: GemTier, gem_element: GemElement, gem_stat_type: GemStatType) -> float:
    return _base(gem_tier, GemType.LESSER, gem_element, gem_stat_type)


def get_stat_base_empowered(gem_tier: GemTier, gem_element: GemElement, gem_stat_type: GemStatType) -> float:
    return _base(gem_tier, GemType.EMPOWERED, gem_element, gem_stat_type)


def get_stat_threshold_lesser(gem_tier, gem_element, gem_stat_type) -> list[float]:
    return _threshold(gem_tier, GemType.LESSER, gem_element, gem_stat_type)


def get_stat_threshold_empowered(gem_tier, gem_element, gem_stat_type) -> list[float]:
    return _threshold(gem_tier, GemType.EMPOWERED, gem_element, gem_stat_type)


def get_lesser_gem_pr_threshold(gem_tier: GemTier, gem_element: GemElement) -> list[int]:
    return _LESSER_PR_THRESHOLD[GemTier(gem_tier)]


def get_empowered_gem_pr_threshold(gem_tier: GemTier, gem_element: GemElement) -> list[int]:
    return _EMPOWERED_PR_THRESHOLD[GemTier(gem_tier)]


def get_augment_base(augment: AugmentType) -> float:
    item = _FOCUS[AugmentType(augment)]
    return next(f["amount"] for f in _data().get("focuses", []) if f["item"] == item)


def stat_weights(gem_tier: GemTier, gem_type: GemType, gem_element: GemElement) -> dict[GemStatType, float]:
    """How likely each stat is to roll on a fresh gem. A cosmic gem that ships no pool
    of its own rolls like the same size and tier of gem of another element."""
    pool = upgrade_data(gem_tier, gem_type, gem_element)["pool"] \
        or upgrade_data(gem_tier, gem_type, GemElement.WATER)["pool"]
    by_key = {(p["stat"], p["percent"]): p["weight"] for p in pool}
    return {st: by_key[key] for st, key in _STAT.items() if key in by_key}


def build_gem_stats() -> dict:
    """Max-roll Mystic gem stats for the build calculator: {type: {stat: [at level 35, per boost]}}."""
    out = {}
    names = {"Damage": GemStatType.PHYSICAL_DAMAGE, "Critical Damage": GemStatType.CRITICAL_DAMAGE,
             "Maximum Health": GemStatType.MAX_HEALTH, "Maximum Health %": GemStatType.MAX_HEALTH_BONUS,
             "Light": GemStatType.LIGHT}
    for gem_type in GemType:
        steps = sum(_steps(GemTier.MYSTIC, lv) for lv in range(1, get_gem_max_level(GemTier.MYSTIC, gem_type) + 1))
        row = {}
        for name, st in names.items():
            element = GemElement.COSMIC if st == GemStatType.LIGHT else GemElement.WATER
            roll = stat_roll(GemTier.MYSTIC, gem_type, element, st)
            row[name] = [roll["max"] + steps * roll["step"], roll["max"]]
        out[gem_type.display_name] = row
    return out

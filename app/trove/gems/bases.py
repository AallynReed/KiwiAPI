"""Gem base values, thresholds, level/PR tables and augment magnitudes.

Ported from BetterTroveTools `models/trove/gem_bases.py`. Element is intentionally
ignored in every lookup (the source matched it with `_`); it's kept in the
signatures for parity with the original call sites. Lookups return concrete
values and raise on genuinely-unmapped input (unreachable for valid gem enums).
"""

from .constants import AugmentType, GemElement, GemStatType, GemTier, GemType

_DMG = (GemStatType.PHYSICAL_DAMAGE, GemStatType.MAGIC_DAMAGE)
_CRIT = (GemStatType.CRITICAL_DAMAGE, GemStatType.CRITICAL_HIT)
_HEALTH = (GemStatType.MAX_HEALTH, GemStatType.MAX_HEALTH_BONUS)

_MAX_LEVEL = {GemTier.RADIANT: 23, GemTier.STELLAR: 25, GemTier.CRYSTAL: 30, GemTier.MYSTIC: 35}
_TIER_PR_BASE = {GemTier.RADIANT: 3, GemTier.STELLAR: 5, GemTier.CRYSTAL: 7, GemTier.MYSTIC: 9}


def get_gem_max_level(gem_tier: GemTier, gem_type: GemType) -> int:
    return _MAX_LEVEL[GemTier(gem_tier)]


def get_level_pr_increment(level: int, gem_base: int) -> int:
    if level in (1, 5, 10, 15):
        return 0
    if level > 15 and level % 5 == 0:
        return gem_base * 5
    if 1 < level < 15:
        return gem_base
    if level > 15:
        return gem_base * 2
    return 0


def get_increment_power_rank_lesser(gem_tier: GemTier, level: int) -> int:
    return get_level_pr_increment(level, _TIER_PR_BASE[GemTier(gem_tier)])


# Lesser and Empowered share the same per-level PR increment schedule.
get_increment_power_rank_empowered = get_increment_power_rank_lesser


# Per-tier stat bases: {stat -> value} plus the damage value (shared by phys/magic).
_LESSER_BASES = {
    GemTier.RADIANT: (14, {GemStatType.CRITICAL_DAMAGE: 0.2, GemStatType.CRITICAL_HIT: 0.02,
                           GemStatType.MAX_HEALTH_BONUS: 0.5, GemStatType.MAX_HEALTH: 50, GemStatType.LIGHT: 1}),
    GemTier.STELLAR: (14, {GemStatType.CRITICAL_DAMAGE: 0.2, GemStatType.CRITICAL_HIT: 0.02,
                           GemStatType.MAX_HEALTH_BONUS: 0.5, GemStatType.MAX_HEALTH: 50, GemStatType.LIGHT: 1}),
    GemTier.CRYSTAL: (16, {GemStatType.CRITICAL_DAMAGE: 3 / 14, GemStatType.CRITICAL_HIT: 0.3 / 14,
                           GemStatType.MAX_HEALTH_BONUS: 0.5, GemStatType.MAX_HEALTH: 50, GemStatType.LIGHT: 5 / 7}),
    GemTier.MYSTIC: (168 / 9, {GemStatType.CRITICAL_DAMAGE: 2.5 / 9, GemStatType.CRITICAL_HIT: 0.25 / 9,
                               GemStatType.MAX_HEALTH_BONUS: 5.25 / 9, GemStatType.MAX_HEALTH: 525 / 9, GemStatType.LIGHT: 5 / 9}),
}


def get_stat_base_lesser(gem_tier: GemTier, gem_element: GemElement, gem_stat_type: GemStatType) -> float:
    dmg, table = _LESSER_BASES[GemTier(gem_tier)]
    st = GemStatType(gem_stat_type)
    if st in _DMG:
        return dmg
    if st in table:
        return table[st]
    raise ValueError(f"no stat base for {st.display_name} at {GemTier(gem_tier).display_name}")


def get_stat_base_empowered(gem_tier: GemTier, gem_element: GemElement, gem_stat_type: GemStatType) -> float:
    # Identical to Lesser except Mystic damage (28 vs 168/9).
    if GemTier(gem_tier) == GemTier.MYSTIC and GemStatType(gem_stat_type) in _DMG:
        return 28
    return get_stat_base_lesser(gem_tier, gem_element, gem_stat_type)


# Stat thresholds [min, max] per tier. Damage/crit/health/light split where they differ.
_LESSER_THRESHOLDS = {
    GemTier.RADIANT: {"all": [85, 113]},
    GemTier.STELLAR: {"all": [150, 200]},
    GemTier.CRYSTAL: {"dmg": [210, 280], "crit": [560 / 3, 770 / 3], "health": [245, 315], "light": [280, 385]},
    GemTier.MYSTIC: {"dmg": [270, 360], "crit": [187.2, 297], "health": [315, 405], "light": [495, 585]},
}
_EMPOWERED_THRESHOLDS = {
    GemTier.RADIANT: {"all": [113, 150]},
    GemTier.STELLAR: {"all": [200, 266]},
    GemTier.CRYSTAL: {"dmg": [245, 350], "crit": [700 / 3, 910 / 3], "health": [315, 385], "light": [350, 420]},
    GemTier.MYSTIC: {"dmg": [210, 300], "crit": [252, 342], "health": [405, 495], "light": [495, 630]},
}


def _threshold(table: dict, gem_tier: GemTier, gem_stat_type: GemStatType) -> list[float]:
    row = table[GemTier(gem_tier)]
    if "all" in row:
        return list(row["all"])
    st = GemStatType(gem_stat_type)
    if st in _DMG:
        return list(row["dmg"])
    if st in _CRIT:
        return list(row["crit"])
    if st in _HEALTH:
        return list(row["health"])
    if st == GemStatType.LIGHT:
        return list(row["light"])
    raise ValueError(f"no threshold for {st.display_name} at {GemTier(gem_tier).display_name}")


def get_stat_threshold_lesser(gem_tier, gem_element, gem_stat_type) -> list[float]:
    return _threshold(_LESSER_THRESHOLDS, gem_tier, gem_stat_type)


def get_stat_threshold_empowered(gem_tier, gem_element, gem_stat_type) -> list[float]:
    return _threshold(_EMPOWERED_THRESHOLDS, gem_tier, gem_stat_type)


_LESSER_PR_THRESHOLD = {GemTier.RADIANT: [85, 113], GemTier.STELLAR: [150, 200],
                        GemTier.CRYSTAL: [175, 250], GemTier.MYSTIC: [200, 260]}
_EMPOWERED_PR_THRESHOLD = {GemTier.RADIANT: [113, 150], GemTier.STELLAR: [200, 266],
                           GemTier.CRYSTAL: [220, 280], GemTier.MYSTIC: [240, 300]}


def get_lesser_gem_pr_threshold(gem_tier: GemTier, gem_element: GemElement) -> list[int]:
    return _LESSER_PR_THRESHOLD[GemTier(gem_tier)]


def get_empowered_gem_pr_threshold(gem_tier: GemTier, gem_element: GemElement) -> list[int]:
    return _EMPOWERED_PR_THRESHOLD[GemTier(gem_tier)]


def get_augment_base(augment: AugmentType) -> float:
    return {AugmentType.ROUGH: 2.5, AugmentType.PRECISE: 5, AugmentType.SUPERIOR: 12.5}[AugmentType(augment)]

"""Gem enums and the static pools/restrictions that drive generation & rerolls.

Ported verbatim from BetterTroveTools `models/trove/gem_constants.py`. IntEnum
values are the canonical wire format — a gem JSON uses the integers, not names.
"""

from enum import IntEnum

GEM_TIER_NAMES = {1: "Radiant", 2: "Stellar", 3: "Crystal", 4: "Mystic"}


class GemTier(IntEnum):
    RADIANT = 1
    STELLAR = 2
    CRYSTAL = 3
    MYSTIC = 4

    @property
    def display_name(self) -> str:
        return GEM_TIER_NAMES[self.value]


GEM_TYPE_NAMES = {1: "Lesser", 2: "Empowered"}


class GemType(IntEnum):
    LESSER = 1
    EMPOWERED = 2

    @property
    def display_name(self) -> str:
        return GEM_TYPE_NAMES[self.value]


GEM_ELEMENT_NAMES = {1: "Water", 2: "Fire", 3: "Air", 4: "Cosmic"}


class GemElement(IntEnum):
    WATER = 1
    FIRE = 2
    AIR = 3
    COSMIC = 4

    @property
    def display_name(self) -> str:
        return GEM_ELEMENT_NAMES[self.value]


GEM_STAT_TYPE_NAMES = {
    1: "Physical Damage",
    2: "Magic Damage",
    3: "Critical Damage",
    4: "Critical Hit",
    5: "Max Health",
    6: "Max Health %",
    7: "Light",
    8: "Movement Speed",
    9: "Jump",
}


class GemStatType(IntEnum):
    PHYSICAL_DAMAGE = 1
    MAGIC_DAMAGE = 2
    CRITICAL_DAMAGE = 3
    CRITICAL_HIT = 4
    MAX_HEALTH = 5
    MAX_HEALTH_BONUS = 6
    LIGHT = 7
    MOVEMENT_SPEED = 8
    JUMP = 9

    @property
    def display_name(self) -> str:
        return GEM_STAT_TYPE_NAMES[self.value]


GEM_RESTRICTION_NAMES = {1: "Fierce", 2: "Arcane"}


class GemRestriction(IntEnum):
    FIERCE = 1
    ARCANE = 2

    @property
    def display_name(self) -> str:
        return GEM_RESTRICTION_NAMES[self.value]


GEM_ABILITY_NAMES = {
    1: "Stinging Curse",
    2: "Volatile Velocity",
    3: "Spirit Surge",
    4: "Mired Mojo",
    5: "Stunburst",
    6: "Pyrodisc",
    7: "Explosive Epilogue",
    8: "Cubic Curtain",
    9: "Berserk Battler",
    10: "Empyrean Barrier",
    11: "Flower Power",
    12: "Vampirian Vanquisher",
}


class GemAbility(IntEnum):
    STINGING_CURSE = 1
    VOLATILE_VELOCITY = 2
    SPIRIT_SURGE = 3
    MIRED_MOJO = 4
    STUNBURST = 5
    PYRODISC = 6
    EXPLOSIVE_EPILOGUE = 7
    CUBIC_CURTAIN = 8
    BERSERK_BATTLER = 9
    EMPYREAN_BARRIER = 10
    FLOWER_POWER = 11
    VAMPIRIAN_VANQUISHER = 12

    @property
    def display_name(self) -> str:
        return GEM_ABILITY_NAMES[self.value]


AUGMENT_TYPE_NAMES = {1: "Rough Focus", 2: "Precise Focus", 3: "Superior Focus"}


class AugmentType(IntEnum):
    ROUGH = 1
    PRECISE = 2
    SUPERIOR = 3

    @property
    def display_name(self) -> str:
        return AUGMENT_TYPE_NAMES[self.value]


# Which stat types each element is allowed to roll (used by rerolls).
GEM_STAT_RESTRICTIONS: dict[GemElement, list[GemStatType]] = {
    GemElement.AIR: [
        GemStatType.PHYSICAL_DAMAGE, GemStatType.MAGIC_DAMAGE, GemStatType.CRITICAL_DAMAGE,
        GemStatType.CRITICAL_HIT, GemStatType.MAX_HEALTH, GemStatType.MAX_HEALTH_BONUS,
    ],
    GemElement.FIRE: [
        GemStatType.PHYSICAL_DAMAGE, GemStatType.MAGIC_DAMAGE, GemStatType.CRITICAL_DAMAGE,
        GemStatType.CRITICAL_HIT, GemStatType.MAX_HEALTH, GemStatType.MAX_HEALTH_BONUS,
    ],
    GemElement.WATER: [
        GemStatType.PHYSICAL_DAMAGE, GemStatType.MAGIC_DAMAGE, GemStatType.CRITICAL_DAMAGE,
        GemStatType.CRITICAL_HIT, GemStatType.MAX_HEALTH, GemStatType.MAX_HEALTH_BONUS,
    ],
    GemElement.COSMIC: [
        GemStatType.PHYSICAL_DAMAGE, GemStatType.MAGIC_DAMAGE, GemStatType.CRITICAL_DAMAGE,
        GemStatType.CRITICAL_HIT, GemStatType.MAX_HEALTH, GemStatType.MAX_HEALTH_BONUS,
        GemStatType.LIGHT,
    ],
}

# Generation pools (the 3 stats a fresh gem can pick from), by element.
_PHYS = [GemStatType.PHYSICAL_DAMAGE, GemStatType.CRITICAL_DAMAGE, GemStatType.CRITICAL_HIT,
         GemStatType.MAX_HEALTH, GemStatType.MAX_HEALTH_BONUS]
_MAGIC = [GemStatType.MAGIC_DAMAGE, GemStatType.CRITICAL_DAMAGE, GemStatType.CRITICAL_HIT,
          GemStatType.MAX_HEALTH, GemStatType.MAX_HEALTH_BONUS]
PHYSICAL_GEM_STAT_POOL: dict[GemElement, list[GemStatType]] = {e: list(_PHYS) for e in GemElement}
MAGIC_GEM_STAT_POOL: dict[GemElement, list[GemStatType]] = {e: list(_MAGIC) for e in GemElement}

# Empowered abilities available per element (Cosmic has its own set).
_ELEMENTAL_ABILITIES = [
    GemAbility.STINGING_CURSE, GemAbility.VOLATILE_VELOCITY, GemAbility.SPIRIT_SURGE,
    GemAbility.MIRED_MOJO, GemAbility.STUNBURST, GemAbility.PYRODISC,
    GemAbility.EXPLOSIVE_EPILOGUE, GemAbility.CUBIC_CURTAIN,
]
GEM_ABILITIES: dict[GemElement, list[GemAbility]] = {
    GemElement.WATER: list(_ELEMENTAL_ABILITIES),
    GemElement.FIRE: list(_ELEMENTAL_ABILITIES),
    GemElement.AIR: list(_ELEMENTAL_ABILITIES),
    GemElement.COSMIC: [
        GemAbility.BERSERK_BATTLER, GemAbility.EMPYREAN_BARRIER,
        GemAbility.FLOWER_POWER, GemAbility.VAMPIRIAN_VANQUISHER,
    ],
}

"""What the numbered fields of the components the decoders read mean.

The client carries no field names, so each label here is tied to evidence in
Trove_x64.exe or the game text; anything not established is left unlabelled
rather than guessed. See docs/binfab-format.md for how each was found.
"""
from __future__ import annotations

from app.trove.codexes.bonuses import STAT_KEYS

# Item/ability identity component: name key, category, description key, icon.
IDENTITY = 49
ID_NAME, ID_CATEGORY, ID_DESCRIPTION, ID_ICON = 1, 2, 5, 6

# Ability action components -> (energy cost field, cooldown field), both on the leaf
# section. Energy is what the class's CanUse compares with the entity's energy
# pool (component 1) or runs through the "useCost" modifier; cooldown is what its
# GetCooldown returns (interface vtable slot 15, or the "cooldown" modifier).
# None = the class has no such cost. An absent field is 0 (factories memset).
ACTIONS: dict[int, tuple[int | None, int | None]] = {
    54: (5, 23),      # melee attack
    57: (11, 16),     # throw (Pretend Pirate 65 / Defused Dummy 45 energy)
    60: (5, 24),      # dash / charge
    62: (4, None),
    69: (8, None),
    93: (3, 19),      # shoot
    100: (8, 16),     # jump
    135: (None, 3),   # timed ability (ultimates: Hot Lead 33s)
    153: (11, None),
    162: (None, 1),   # ability delegate
    178: (11, None),
    194: (23, 17),
    204: (6, None),
    232: (None, 2),
    291: (6, 4),
    380: (11, 16),
    382: (3, 19),
    437: (8, None),
    1008: (3, 19),
    1020: (11, 16),
}

# Every component whose root class is the Effect base (buffs, debuffs, damage over
# time, stuns, polymorphs...). Their first section is that base.
EFFECTS = frozenset({
    45, 46, 64, 131, 132, 136, 141, 142, 150, 151, 155, 156, 157, 158, 165, 171, 189,
    213, 218, 228, 231, 233, 234, 235, 240, 242, 243, 250, 252, 260, 261, 262, 272, 274,
    275, 278, 280, 281, 282, 283, 284, 287, 288, 303, 304, 309, 315, 316, 319, 321, 324,
    330, 331, 332, 336, 337, 339, 344, 349, 350, 354, 365, 373, 379, 383, 384, 389, 392,
    396, 397, 401, 406, 433, 447, 450, 452, 453, 458, 461, 466, 473, 475, 483, 485, 488,
    492, 499, 1004, 1005, 1013, 1016, 1017, 1021, 1031, 1032, 1034, 1037, 1038, 1050, 1051,
})
# Effect base, section 0: seconds the effect lasts (Freeze Blast 3.5, Dragonman 15;
# -FLT_MAX = until removed), and the effect's own name / description locale keys.
EFFECT_DURATION, EFFECT_DESCRIPTION, EFFECT_NAME = 4, 2, 23

# Combat-event effect spawner (ally and equipment procs): section 2, field 4 is the
# proc's own cooldown ("Triggers Chaos on damage dealt with a 10 second cooldown").
PROC_SPAWNER = 150
PROC_SECTION, PROC_COOLDOWN = 2, 4

# DamageParameters (leaf section), named from the client's own editor text.
DAMAGE = {
    32: "damage",               # "The amount of damage applied to entities."
    12: "multiplier",           # "A multiplier on the attacker's damage stat."
    5: "max_health_percent",    # "...as a percent of their maximum health."
    28: "crit_chance_bonus",    # "A bonus added to the attacker's CriticialHitChance stat."
    25: "crit_damage_multiplier",
    19: "energy_drain",         # "The amount of energy drained from the target..."
    31: "block_damage",         # "The amount of damage applied to blocks."
    2: "max_block_hardness",
    4: "max_blocks",
    9: "knockback_multiplier",
    10: "knockback",
    14: "min_knockback",
    23: "knockup",
    33: "stability_ignore",
    16: "target_effect",        # an effect prefab spawned on the target
    20: "attacker_effect",
    11: "damage_type",
    26: "damage_stat",
}
DAMAGE_SIGNATURE = frozenset({4, 9, 12, 14, 32})
DAMAGE_INTS = (2, 4, 31, 32)
DAMAGE_FLOATS = (5, 9, 10, 12, 14, 33)

# HealingParameters (leaf section).
HEALING = {
    7: "health",
    1: "max_health_percent",
    2: "energy",
    3: "max_energy_percent",
    5: "target_effect",
    6: "attacker_effect",
}
HEALING_SIGNATURE = frozenset({1, 2, 3, 4, 5, 6, 7})
HEALING_KEYS = frozenset(range(9))

# Stat modifier record: {0 stat, 1 op, 2 value, 3 label, 5 KModFlags, 6 scale factor,
# 7 scaling mode, 8 extra flat}. Stat and op are the KStatType / KModType ordinals.
MOD_STAT, MOD_OP, MOD_VALUE, MOD_LABEL, MOD_FLAGS = 0, 1, 2, 3, 5
MOD_OPS = ("MultiplySum", "Add", "Set", "Nullify", "Multiply", "Minimum", "Maximum")
MOD_FLAG_NAMES = {1: "PVE", 2: "PVP", 4: "Reproportion", 8: "OwnedClass", 16: "Discovery",
                  32: "Delve", 64: "PostRange", 128: "ExcludePVP"}


def stat_key(ordinal: int) -> str:
    """KStatType ordinal -> the `$Stat_*` key the locale tables name."""
    extra = {45: "$Stat_HealDoneMultiplier", 46: "$Stat_HealReceiveMultiplier"}
    return STAT_KEYS.get(ordinal) or extra.get(ordinal, f"$Stat_{ordinal}")

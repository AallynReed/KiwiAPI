"""delve_modifiers.json - the Kiwi Wiki's /delve-modifiers page.

The client carries each modifier's name (`$DelveCreatureMod_*`, `$DelveLairMod_*`,
`$DelvePathMod_*` in languages/en/delve.binfab) and the effect prefabs under
prefabs/abilities/delve/, but not the server's table joining the two. MODIFIERS
below is that join, written by hand and kept to prefabs whose names match the
modifier unambiguously; the numbers are always read from the prefabs.

Also decodes the unnamed per-tier player effects and names "Under Pressure".
"""

from __future__ import annotations

import re
import struct

from app.trove.codexes.binfab import extract_localization_map, unzig
from app.trove.codexes.bonuses import OPERATIONS, STAT_KEYS
from app.trove.decode.tree import GameTree

TITLE = "Delve modifiers"
OUTPUT = "delve_modifiers.json"
PREFIXES = ("prefabs/abilities/delve/", "languages/en/")
INDENT, FINAL_NEWLINE = 1, True

CATEGORIES = {"DelveCreatureMod_": "creature", "DelveLairMod_": "lair", "DelvePathMod_": "path"}

# key -> summary: what the prefab chain does; effects: (who, prefab).
MODIFIERS: dict[str, dict] = {
    "DelveCreatureMod_berserker": {
        "summary": "Gets stronger each time one of its allies is killed.",
        "effects": [("Per ally killed", "mutators/berserker_onallykilled_stats")],
    },
    "DelveCreatureMod_berserkerHard": {
        "summary": "Gets stronger and heals each time one of its allies is killed. The bonus stacks.",
        "effects": [("Per ally killed", "mutators/berserkerstacking_onallykilled_stats")],
    },
    "DelveCreatureMod_slowPlayersOnHit": {
        "summary": "Its hits snare you and knock you off your mount.",
        "effects": [("Players it hits", "mutators/daze_onoutgoingdamage_movementspeed")],
    },
    "DelveCreatureMod_debuffOnRangedDamage": {
        "summary": "Hitting it from range debuffs you.",
        "effects": [("Ranged attackers", "mutators/debuffonrangeddamage_shield_debuff")],
    },
    "DelveCreatureMod_ShadowChickens": {
        "summary": "Summons shadow chickens when it spots you. They vanish after 30 seconds and give no kill credit.",
        "effects": [("Each chicken", "mutators/spawnchicken_onaggro_stats")],
    },
    "DelveCreatureMod_SpawnClone": {
        "summary": "Splits off a clone of itself when damaged. Clones vanish after 30 seconds and give no kill credit.",
        "effects": [("Each clone", "mutators/spawnclone_ondamage_stats")],
    },
    "DelveCreatureMod_SpawnMushrooms": {
        "summary": "Keeps spawning mushroom men that explode when they die. They vanish after 10 seconds and give no kill credit.",
        "effects": [("Each mushroom man", "mutators/spawnmushroom_periodic_stats")],
    },
    "DelveCreatureMod_SummonTurret": {
        "summary": "Summons a flamethrower turret when it spots you. It vanishes after 30 seconds and gives no kill credit.",
    },
    "DelveCreatureMod_SpellResistant": {"summary": "Takes 50% less magic damage."},
    "DelveCreatureMod_SpellImmune": {"summary": "Takes no magic damage."},
    "DelveCreatureMod_PhysicalResistant": {"summary": "Takes 50% less physical damage."},
    "DelveCreatureMod_PhysicalImmune": {"summary": "Takes no physical damage."},
    "DelveCreatureMod_icyGround": {"summary": "Turns the ground under you to ice when it spots you."},
    "DelveLairMod_bossBerserker": {
        "summary": "The boss gets stronger at set health thresholds.",
        "effects": [("Per threshold", "boss/mutators/berserker_onhealthpercent_stats")],
    },
    "DelveLairMod_floorToLavaShort": {
        "summary": "Keeps turning patches of floor to lava. Anyone caught loses health regen for 10 seconds.",
        "effects": [("Players caught", "mutators/shared_disable_healthregen_10s")],
    },
    "DelveLairMod_floorToLavaLong": {
        "summary": "Keeps turning patches of floor to lava. Anyone caught loses health regen for 15 seconds.",
        "effects": [("Players caught", "mutators/shared_disable_healthregen_15s")],
    },
    "DelveLairMod_floorToIce": {"summary": "Keeps turning patches of floor to ice."},
    "DelveLairMod_Gravity": {"summary": "Keeps turning patches of floor into blocks that pull you down."},
    "DelveLairMod_stopDpsStun": {"summary": "Every so often the boss turns reactive: hit it then and you're stunned."},
    "DelvePathMod_FlaskRefiller": {"summary": "Creatures that die release a burst that refills flasks nearby."},
}

# Vulnerable / Hardened carry their percentage in the key itself; no prefab ships.
_INC_DMG = re.compile(r"^DelveLairMod_(increase|reduce)IncDmg_(\d+)$")

# `$Stat_` key (no prefix) -> (label, scale, how Add reads: "pct" | "mult" | "flat").
STATS = {
    "IncomingDamageMod": ("Damage taken", 1, "mult"),
    "OutgoingDamageMod": ("Damage dealt", 1, "mult"),
    "PhysicalDamage": ("Physical damage", 1, "flat"),
    "SpellDamage": ("Magic damage", 1, "flat"),
    "MaxHealth": ("Maximum health", 1, "flat"),
    "MovementSpeed": ("Movement speed", 1, "flat"),
    "HealthRegen_controller": ("Health regen", 1, "flat"),
    "CriticalHitChance": ("Critical hit", 0.1, "pct"),  # stored in tenths of a percent
    "Jump": ("Jump", 1, "flat"),
    "Glide": ("Glide", 1, "flat"),
    "MaxFlasks": ("Flasks", 1, "flat"),
}

_MOD = re.compile(rb"(.)\x10(.)\x24(.{4})\x38(.)", re.S)


def _num(v: float) -> str:
    v = round(v, 2)
    return f"{v:,.0f}" if v == int(v) else f"{v:,.2f}".rstrip("0")


def stat_mods(data: bytes) -> list[tuple[str, str, float]]:
    out = []
    for m in _MOD.finditer(data):
        stat = STAT_KEYS.get(unzig(m.group(1)[0]), "").removeprefix("$Stat_")
        op = OPERATIONS.get(m.group(2)[0])
        label = data[m.end():m.end() + m.group(4)[0]]
        if stat and op and re.fullmatch(rb"[\x20-\x7e]*", label):
            out.append((stat, op, struct.unpack("<f", m.group(3))[0]))
    return out


def effect_lines(data: bytes) -> list[str]:
    lines = []
    for stat, op, value in stat_mods(data):
        if op == "Nullify":
            continue  # vetoes other modifiers sharing its label; not an effect of its own
        if stat not in STATS:
            raise ValueError(f"unmapped stat {stat}")
        label, scale, kind = STATS[stat]
        value *= scale
        if op == "Multiply" or (op == "Set" and kind == "mult"):
            lines.append(f"{label} ×{_num(value)}")
        elif op == "Set":
            lines.append(f"{label} set to {_num(value)}")
        elif op == "Add" and kind in ("mult", "pct"):
            pct = value * 100 if kind == "mult" else value
            lines.append(f"{label} {'+' if pct >= 0 else '−'}{_num(abs(pct))}%")
        elif op == "Add":
            lines.append(f"{label} {'+' if value >= 0 else '−'}{_num(abs(value))}")
        elif op == "MultiplySum":
            lines.append(f"{label} +{_num(value * 100)}%")
        else:
            raise ValueError(f"unhandled {op} on {stat}")
    return lines


def build(tree: GameTree) -> dict:
    loc = extract_localization_map(tree.read("languages/en/delve.binfab") or b"")
    effect_loc = extract_localization_map(tree.read("languages/en/prefabs_effects_delve.binfab") or b"")

    def prefab(rel: str) -> bytes:
        data = tree.read(f"prefabs/abilities/delve/{rel}.binfab")
        if data is None:
            raise ValueError(f"missing prefab abilities/delve/{rel}")
        return data

    unknown = set(MODIFIERS) - {k[1:] for k in loc}
    if unknown:
        raise ValueError(f"no longer in the locale: {sorted(unknown)}")

    modifiers = []
    for key, game_name in sorted(loc.items()):
        key = key[1:]
        category = next((c for p, c in CATEGORIES.items() if key.startswith(p)), None)
        if category is None:
            continue
        spec = MODIFIERS.get(key, {})
        summary = spec.get("summary", "")
        if m := _INC_DMG.match(key):
            summary = f"Incoming damage {'+' if m.group(1) == 'increase' else '−'}{m.group(2)}%."
        modifiers.append({
            "key": key, "category": category, "name": game_name, "summary": summary,
            "effects": [{"who": who, "lines": effect_lines(prefab(rel))}
                        for who, rel in spec.get("effects", [])],
        })

    tiers = []
    for n in range(1, 100):
        rel = f"mutators/tier_player_set_incomingdamagemod_t{n:02d}"
        if not tree.exists(f"prefabs/abilities/delve/{rel}.binfab"):
            break
        (value,) = (v for stat, op, v in stat_mods(prefab(rel))
                    if stat == "IncomingDamageMod" and op == "Set")
        tiers.append({"tier": n, "damage_taken": _num(value)})

    return {
        "modifiers": sorted(modifiers, key=lambda x: x["name"].lower()),
        "pressure": {
            "name": effect_loc["$prefabs_effects_delve_healthregen_scaling_name"],
            "description": effect_loc["$prefabs_effects_delve_healthregen_scaling_description"],
        },
        "tier_damage": tiers,
        "tier_effects": [
            {"name": "Fewer flasks", "lines": [line for n in (1, 2, 3) for line in
                                                effect_lines(prefab(f"mutators/tier_player_sub_flasks_{n:02d}"))]
             + effect_lines(prefab("mutators/tier_player_set_flasks_0"))},
            {"name": "Slowed", "lines": effect_lines(prefab("mutators/tier_player_set_movementspeed_40"))},
            {"name": "Reduced healing", "lines": effect_lines(prefab("mutators/tier_player_reducehealing_noregen"))
             + ["Healing received is reduced"]},
            {"name": "No mounts", "lines": []},
            {"name": "Death boon", "lines": ["When a player dies, the rest of the party is healed to full"]},
            {"name": "Death curse", "lines": ["When a player dies, the rest of the party is hit by a death curse"]},
        ],
    }


def count(data: dict) -> int:
    return len(data["modifiers"])

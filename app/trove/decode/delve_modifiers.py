"""delve_modifiers.json - the Kiwi Wiki's /delve-modifiers page.

Every delve modifier has a sign in the client - `prefabs/placeable/deco/delve/
<id>_interactable.binfab`, the stone shown when a floor rolls it - carrying a
`$prefabs_placeable_deco_delve_<id>_interactable_sign_title` / `_content` key
pair. Those are the names and descriptions players see, so they are the page.

The numbers sit in effect prefabs under `prefabs/abilities/delve/`, which the
signs don't reference; the server holds that join. SIGNS below is it, written by
hand from the matching internal names (`increaseplayerdamage_05` ->
`tier_player_set_incomingdamagemod_t05`), plus each sign's group.

The older `$DelveCreatureMod_*` / `$DelveLairMod_*` / `$DelvePathMod_*` locale
names are internal labels for the same modifiers. LEGACY maps them onto signs;
one left without a sign has no in-game presence and is listed as retired.
"""
from __future__ import annotations

import re
import struct

from app.trove.codexes.binfab import extract_localization_map, unzig
from app.trove.codexes.bonuses import OPERATIONS, STAT_KEYS
from app.trove.decode.tree import GameTree

TITLE = "Delve modifiers"
OUTPUT = "delve_modifiers.json"
PREFIXES = ("prefabs/placeable/deco/delve/", "prefabs/abilities/delve/", "languages/en/")
INDENT, FINAL_NEWLINE = 1, True

SIGN_DIR = "prefabs/placeable/deco/delve/"
_SIGN_KEY = re.compile(rb"\$prefabs_placeable_deco_delve_([A-Za-z0-9_]+?)_interactable_sign_title")

# sign id -> (group, [(who, effect prefab under abilities/delve/)]); who is blank when it's
# the whole party. Groups:
# creature (rolled onto enemies), lair (the boss), path, player, tier.
SIGNS: dict[str, tuple[str, list[tuple[str, str]]]] = {
    "agile": ("creature", []),
    "antisolo2": ("creature", []),
    "antumbral": ("creature", []),
    "arena": ("creature", []),
    "berserker": ("creature", [("Per ally slain", "mutators/berserker_onallykilled_stats")]),
    "berserkerstacking": ("creature", [("Per ally slain", "mutators/berserkerstacking_onallykilled_stats")]),
    "buffondeath": ("creature", []),
    "debuffonranged": ("creature", [("Ranged attackers", "mutators/debuffonrangeddamage_shield_debuff")]),
    "deltalith_miniBoss": ("creature", []),
    "fragile": ("creature", []),
    "icyground_onaggro": ("creature", []),
    "physicalimmune": ("creature", []),
    "physicalresistant": ("creature", []),
    "rangedimmune": ("creature", []),
    "slowplayersonhit": ("creature", [("Players it hits", "mutators/daze_onoutgoingdamage_movementspeed")]),
    "spawnclone": ("creature", [("Each clone", "mutators/spawnclone_ondamage_stats")]),
    "spawnmushrooms": ("creature", [("Each Sporeling", "mutators/spawnmushroom_periodic_stats")]),
    "spawnshadowchickens": ("creature", [("Each chicken", "mutators/spawnchicken_onaggro_stats")]),
    "spawnturret": ("creature", []),
    "spellimmune": ("creature", []),
    "spellresistant": ("creature", []),
    "tenebrous": ("creature", []),
    "tenebrous2": ("creature", []),
    "tenebrous3": ("creature", []),
    "weak": ("creature", []),
    "berserkerboss": ("lair", [("Per threshold", "boss/mutators/berserker_onhealthpercent_stats")]),
    "deltalith_bossAntiGrav": ("lair", []),
    "deltalith_bossFloorIce": ("lair", []),
    "deltalith_bossFloorLavaLong": ("lair", [("Players caught", "mutators/shared_disable_healthregen_15s")]),
    "deltalith_bossFloorLavaShort": ("lair", [("Players caught", "mutators/shared_disable_healthregen_10s")]),
    "deltalith_bossIncreaseDamage_15": ("lair", []),
    "deltalith_bossIncreaseDamage_30": ("lair", []),
    "deltalith_bossIncreaseDamage_45": ("lair", []),
    "deltalith_bossReduceIncDmg_15": ("lair", []),
    "deltalith_bossReduceIncDmg_30": ("lair", []),
    "deltalith_bossReduceIncDmg_45": ("lair", []),
    "deltalith_bossReduceIncDmg_60": ("lair", []),
    "deltalith_bossReduceIncDmg_75": ("lair", []),
    "deltalith_bossTimeBomb_180": ("lair", []),
    "deltalith_graveyard_creaturAoeRoot": ("lair", []),
    "deltalith_stopDpsStun": ("lair", []),
    "flaskrefiller": ("path", []),
    "pathExplosions": ("path", []),
    "pathNoChevrons": ("path", []),
    "speedblocks": ("path", []),
    **{f"increaseplayerdamage_{n:02d}": (
        "player", [("", f"mutators/tier_player_set_incomingdamagemod_t{n:02d}")]) for n in range(1, 14)},
    "flasks_0": ("player", [("", "mutators/tier_player_set_flasks_0")]),
    **{f"flasks_n{n:02d}": ("player", [("", f"mutators/tier_player_sub_flasks_{n:02d}")]) for n in (1, 2, 3)},
    **{f"jump_{j}": ("player", [("", f"mutators/jump_{j}")]) for j in ("0", "2", "n1", "n3", "n5")},
    "deltalith_movementSpeed_40": ("player", [("", "mutators/tier_player_set_movementspeed_40")]),
    "reducehealing": ("player", [("", "mutators/tier_player_reducehealing_noregen")]),
    "vampirism": ("player", []),
    "deltalith_death_boon": ("player", []),
    "deltalith_death_curse": ("player", []),
    "deltalith_group_death": ("player", []),
    "disabledeathsaveeffect": ("player", []),
    "respawnSlower": ("player", []),
    "lootlevel": ("tier", []),
    "fauxlootlevel": ("tier", []),
}

# Internal locale label -> the sign it names.
LEGACY: dict[str, str] = {
    "DelveCreatureMod_Agile": "agile", "DelveCreatureMod_MiniBoss": "deltalith_miniBoss",
    "DelveCreatureMod_Antumbral": "antumbral", "DelveCreatureMod_Arena": "arena",
    "DelveCreatureMod_Fragile": "fragile", "DelveCreatureMod_icyGround": "icyground_onaggro",
    "DelveCreatureMod_berserker": "berserker", "DelveCreatureMod_berserkerHard": "berserkerstacking",
    "DelveCreatureMod_antiSolo2": "antisolo2", "DelveCreatureMod_slowPlayersOnHit": "slowplayersonhit",
    "DelveCreatureMod_buffOnDeath": "buffondeath", "DelveCreatureMod_RangedProtection": "rangedimmune",
    "DelveCreatureMod_debuffOnRangedDamage": "debuffonranged",
    "DelveCreatureMod_ShadowChickens": "spawnshadowchickens", "DelveCreatureMod_SpawnClone": "spawnclone",
    "DelveCreatureMod_SpawnMushrooms": "spawnmushrooms", "DelveCreatureMod_SummonTurret": "spawnturret",
    "DelveCreatureMod_Tenebrous": "tenebrous", "DelveCreatureMod_Weak": "weak",
    "DelveCreatureMod_SpellResistant": "spellresistant", "DelveCreatureMod_SpellImmune": "spellimmune",
    "DelveCreatureMod_PhysicalResistant": "physicalresistant",
    "DelveCreatureMod_PhysicalImmune": "physicalimmune",
    "DelvePathMod_noChevrons": "pathNoChevrons", "DelvePathMod_FlaskRefiller": "flaskrefiller",
    "DelvePathMod_SpeedBlocks": "speedblocks", "DelvePathMod_explosions": "pathExplosions",
    "DelveLairMod_bossBerserker": "berserkerboss", "DelveLairMod_floorToLavaShort": "deltalith_bossFloorLavaShort",
    "DelveLairMod_floorToLavaLong": "deltalith_bossFloorLavaLong", "DelveLairMod_floorToIce": "deltalith_bossFloorIce",
    "DelveLairMod_Gravity": "deltalith_bossAntiGrav",
    "DelveLairMod_graveyardAoeRoot": "deltalith_graveyard_creaturAoeRoot",
    "DelveLairMod_stopDpsStun": "deltalith_stopDpsStun",
    **{f"DelveLairMod_increaseIncDmg_{n}": f"deltalith_bossIncreaseDamage_{n}" for n in (15, 30, 45)},
    **{f"DelveLairMod_reduceIncDmg_{n}": f"deltalith_bossReduceIncDmg_{n}" for n in (15, 30, 45, 60, 75)},
}
_LEGACY_GROUP = {"DelveCreatureMod_": "creature", "DelveLairMod_": "lair", "DelvePathMod_": "path"}

# The boss's damage-taken modifiers carry their size only in the id.
_BOSS_DMG = re.compile(r"^deltalith_boss(IncreaseDamage|ReduceIncDmg)_(\d+)$")

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


def _sign_id(data: bytes) -> str | None:
    m = _SIGN_KEY.search(data)
    return m.group(1).decode() if m else None


def build(tree: GameTree) -> dict:
    loc: dict[str, str] = {}
    for path in tree.files("languages/en/prefabs_placeable_deco_delve", ".binfab"):
        loc.update(extract_localization_map(tree.read(path) or b""))
    legacy = extract_localization_map(tree.read("languages/en/delve.binfab") or b"")
    effect_loc = extract_localization_map(tree.read("languages/en/prefabs_effects_delve.binfab") or b"")

    def prefab(rel: str) -> bytes:
        data = tree.read(f"prefabs/abilities/delve/{rel}.binfab")
        if data is None:
            raise ValueError(f"missing prefab abilities/delve/{rel}")
        return data

    signs = {}
    for path in tree.files(SIGN_DIR, "_interactable.binfab"):
        sid = _sign_id(tree.read(path) or b"") if path.count("/") == 4 else None
        base = f"$prefabs_placeable_deco_delve_{sid}_interactable_sign"
        if sid and loc.get(base + "_title"):
            signs[sid] = (loc[base + "_title"], loc.get(base + "_content", ""))

    missing = set(SIGNS) - set(signs)
    if missing:
        raise ValueError(f"signs no longer in the game: {sorted(missing)}")

    modifiers = []
    for sid, (name, description) in signs.items():
        group, effects = SIGNS.get(sid, ("new", []))
        rows = [{"who": who, "lines": effect_lines(prefab(rel))} for who, rel in effects]
        if m := _BOSS_DMG.match(sid):
            sign = "+" if m.group(1) == "IncreaseDamage" else "−"
            rows.append({"who": "The boss", "lines": [f"Damage taken {sign}{m.group(2)}%"]})
        modifiers.append({"key": sid, "category": group, "name": name,
                          "description": description, "effects": rows})

    retired = []
    for key, name in legacy.items():
        key = key[1:]
        group = next((g for p, g in _LEGACY_GROUP.items() if key.startswith(p)), None)
        if group is None:
            continue
        if key not in LEGACY:
            retired.append({"key": key, "category": group, "name": name})
        elif LEGACY[key] not in signs:
            raise ValueError(f"{key} maps to a sign that is gone: {LEGACY[key]}")

    order = {g: i for i, g in enumerate(("creature", "lair", "path", "player", "tier", "new"))}
    return {
        "modifiers": sorted(modifiers, key=lambda x: (order[x["category"]], _natural(x["name"]))),
        "retired": sorted(retired, key=lambda x: x["name"].lower()),
        "pressure": {
            "name": effect_loc["$prefabs_effects_delve_healthregen_scaling_name"],
            "description": effect_loc["$prefabs_effects_delve_healthregen_scaling_description"],
        },
    }


_ROMAN = {"I": 1, "II": 2, "III": 3, "IV": 4, "V": 5, "VI": 6, "VII": 7, "VIII": 8, "IX": 9,
          "X": 10, "XI": 11, "XII": 12, "XIII": 13}


def _natural(name: str) -> tuple[str, int]:
    """"Terror IX" sorts before "Terror X"."""
    head, _, tail = name.rpartition(" ")
    return (head.lower(), _ROMAN[tail]) if head and tail in _ROMAN else (name.lower(), 0)


def count(data: dict) -> int:
    return len(data["modifiers"])

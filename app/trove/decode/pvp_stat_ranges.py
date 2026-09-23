"""pvp.json - the PvE -> PvP stat curves behind the /calculators PvP tab.

PvP converts each PvE stat through a `PVPStatRange` record in
`prefabs/pvp/data/stats/<mode>_statranges.binfab`. A record is
`<wt4 key> 00 <zig stat>` followed by these fields until `1e`:

  1 vec2   (not read by the curve)       5 f32  sqrt coefficient
  2 vec2   output range [lo, hi]         6 bool curve on
  3 vec2   (not read by the curve)       7 bool raise values below the knee to it
  4 f32    knee                          8 f32  soft-cap ceiling

Trove_x64.exe applies them in FUN_14081dff0 (`pvp_value` below; calculators.js
`pvpValue`). docs/pvp-stats.md explains the curve; the wiki's /pvp-stats shows it.

Between the floor and the soft cap the game applies class, then role modifiers
(FUN_14081e290). The client's `pvp_classes` / `pvp_roles` ship empty; the server
sends the real ones, so ROLES below is kept by hand from the Trove team's numbers.
"""
from __future__ import annotations

import math
import struct
from collections.abc import Sequence

from app.trove.codexes.binfab import read_uleb, unzig
from app.trove.codexes.bonuses import STAT_KEYS
from app.trove.decode.tree import GameTree

TITLE = "PvP stat curves"
OUTPUT = "pvp.json"
PREFIXES = ("prefabs/pvp/data/stats/",)
INDENT, FINAL_NEWLINE = 1, True

# Mode key (calculators.js PVP_MODES names it) -> file.
MODES = {
    "pvp": "pvp_statranges.binfab",
    "battleroyale": "pvp_battleroyale_statranges.binfab",
    "bloodstone": "pvp_bloodstone_statranges.binfab",
}

# `$Stat_` key -> wire-to-sheet scale, the same as class_levels.py.
# calculators.js PVP_SHEET names and orders them; stats absent here are not on the sheet.
STATS = {
    "PhysicalDamage": 1, "SpellDamage": 1, "MaxHealth": 1, "MaxEnergy": 1,
    "HealthRegen_controller": 1, "EnergyRegen_controller": 1, "MovementSpeed": 1,
    "AttackSpeed": 1, "Jump": 1, "CriticalHitChance": 0.1, "CriticalHitDamage": 1,
}
REQUIRED = (2, 4, 5, 6, 7, 8)

# The two files as they ship today: empty lists.
EMPTY_LIST = bytes.fromhex("3e2e0008be012e00081e")
ROLE_FILES = ("pvp_roles.binfab", "pvp_classes.binfab")

# Modes the roles apply in: PvP only, not Battle Royale or Bloodstone.
ROLE_MODES = ["pvp"]

# KPVPClassRole (Tank, Assassin, Mage, Support, RangedDPS, Skirmisher) -> classes
# (tech name, display name) and modifiers in the order the game applies them.
# `stat` is None where the stat behind the label isn't known.
ROLES = [
    {"key": "Tank", "name": "Tank",
     "classes": [["candybarbarian", "Candy Barbarian"], ["spirittank", "Revenant"], ["knight", "Knight"]],
     "modifiers": [{"stat": "MaxHealth", "label": "Maximum Health", "op": "multiply", "value": 1.15},
                   {"stat": "OutgoingDamageMod", "label": "Outgoing damage", "op": "multiply", "value": 0.9}]},
    {"key": "Assassin", "name": "Assassin",
     "classes": [["shadowhunter", "Shadow Hunter"], ["neonninja", "Neon Ninja"]],
     "modifiers": [{"stat": "MaxHealth", "label": "Maximum Health", "op": "multiply", "value": 0.85},
                   {"stat": "OutgoingDamageMod", "label": "Outgoing damage", "op": "multiply", "value": 1.12}]},
    {"key": "Skirmisher", "name": "Skirmisher",
     "classes": [["lunarlancer", "Lunar Lancer"], ["adventurer", "Boomeranger"], ["crimefighter", "Vanguardian"]],
     "modifiers": [{"stat": "MovementSpeed", "label": "Movement Speed", "op": "multiply", "value": 1.08},
                   {"stat": "MaxHealth", "label": "Maximum Health", "op": "multiply", "value": 0.95}]},
    {"key": "Mage", "name": "Mage",
     "classes": [["icemage", "Ice Sage"], ["dracolyte", "Dracolyte"], ["tombraiser", "Tomb Raiser"],
                 ["faetrickster", "Fae Trickster"]],
     "modifiers": [{"stat": "SpellDamage", "label": "Magic Damage", "op": "multiply", "value": 1.15},
                   {"stat": None, "label": "Cooldown", "op": "multiply", "value": 1.2}]},
    {"key": "Support", "name": "Support",
     "classes": [["chloromancer", "Chloromancer"], ["bard", "Bard"]],
     "modifiers": [{"stat": "HealDoneMultiplier", "label": "Healing done", "op": "multiply", "value": 0.9},
                   {"stat": None, "label": "Cooldown", "op": "multiply", "value": 1.15}]},
    {"key": "RangedDPS", "name": "Ranged DPS",
     "classes": [["gunslinger", "Gunslinger"], ["piratelord", "Pirate Captain"], ["dinotamer", "Dino Tamer"],
                 ["solarion", "Solarion"]],
     "modifiers": [{"stat": "CriticalHitDamage", "label": "Critical Damage", "op": "multiply", "value": 1.15},
                   {"stat": "MaxHealth", "label": "Maximum Health", "op": "multiply", "value": 0.92}]},
]


def parse(data: bytes) -> list[dict]:
    if data[:2] != b"\x3e\xae":
        raise ValueError("not a statranges list")
    count, pos = read_uleb(data, 2)
    pos += 1
    rows = []
    for _ in range(count):
        key, pos = read_uleb(data, pos)
        if key & 0xF != 4 or data[pos] != 0:
            raise ValueError(f"bad record header at {pos}")
        stat, pos = read_uleb(data, pos + 1)
        row: dict = {"stat": unzig(stat)}
        while data[pos] != 0x1E:
            key, pos = read_uleb(data, pos)
            field, wire = key >> 4, key & 0xF
            if wire == 0xC:
                if data[pos] != 0x24:
                    raise ValueError(f"field {field}: not a vec2")
                row[field] = list(struct.unpack_from("<2f", data, pos + 1))
                pos += 9
            elif wire == 4:
                row[field] = struct.unpack_from("<f", data, pos)[0]
                pos += 4
            elif wire == 0:
                value, pos = read_uleb(data, pos)
                row[field] = unzig(value)
            else:
                raise ValueError(f"field {field}: unknown wire type {wire}")
        rows.append(row)
        pos += 1
    return rows


def build(tree: GameTree) -> dict:
    modes = {}
    for key, name in MODES.items():
        data = tree.read(f"prefabs/pvp/data/stats/{name}")
        if data is None:
            raise ValueError(f"missing prefabs/pvp/data/stats/{name}")
        stats = {}
        for row in parse(data):
            stat = STAT_KEYS.get(row["stat"], "").removeprefix("$Stat_")
            if stat not in STATS:
                continue
            missing = [f for f in REQUIRED if f not in row]
            if missing:
                raise ValueError(f"{name} {stat}: missing fields {missing}")
            stats[stat] = {
                "scale": STATS[stat], "out": [round(v, 4) for v in row[2]],
                "knee": round(row[4], 4), "sqrt": round(row[5], 4),
                "curve": bool(row[6]), "floor": bool(row[7]), "cap": round(row[8], 4),
            }
        modes[key] = stats
    for name in ROLE_FILES:
        if tree.read(f"prefabs/pvp/data/stats/{name}") not in (None, EMPTY_LIST):
            raise ValueError(f"{name} now ships data: decode it instead of the hand-kept ROLES")
    return {"modes": modes, "roles": ROLES, "role_modes": ROLE_MODES}


def pvp_value(e: dict, v: float, mods: Sequence[dict] = ()) -> float:
    """One pvp.json entry applied to a PvE value, both in game units (calculators.js `pvpValue`).
    `mods` are the role's modifiers for this stat."""
    if e["curve"]:
        if v < e["knee"]:
            if e["floor"]:
                v = e["knee"]
        else:
            v = e["knee"] + e["sqrt"] * math.sqrt(v - e["knee"])
    lo, hi = e["out"]
    v = max(v, lo)
    for m in mods:
        v = v + m["value"] if m["op"] == "add" else v * m["value"]
    if v > hi:
        cap = e["cap"]
        v = hi + (cap - hi) * (1 - math.exp(-(v - hi) / (cap - hi))) if hi < cap else hi
    return v


def count(data: dict) -> int:
    return sum(len(v) for v in data["modes"].values())

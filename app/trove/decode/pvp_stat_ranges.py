"""pvp.json - the PvE -> PvP stat curves behind the /calculators PvP tab.

PvP converts each PvE stat through a `PVPStatRange` record in
`prefabs/pvp/data/stats/<mode>_statranges.binfab`. A record is
`<wt4 key> 00 <zig stat>` followed by these fields until `1e`:

  1 vec2   (not read by the curve)       5 f32  sqrt coefficient
  2 vec2   output range [lo, hi]         6 bool curve on
  3 vec2   (not read by the curve)       7 bool raise values below the knee to it
  4 f32    knee                          8 f32  soft-cap ceiling

Trove_x64.exe applies them in FUN_14081dff0 (see site/static/calculators.js
`pvpValue`).
"""
from __future__ import annotations

import struct

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
    return {"modes": modes}


def count(data: dict) -> int:
    return sum(len(v) for v in data["modes"].values())

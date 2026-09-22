"""classes.json - each class's base sheet, class bonuses and per-level stats.

Every `prefabs/class/<tech>.binfab` carries a 30-entry level list: record N (wire
field N) is what reaching level N grants. Level 1 holds the class's own modifiers
(MultiplySum -> "Maximum Health %", Multiply -> the class bonuses, Multiply 0 ->
the class has no such stat, Set -> a fixed value); levels 2-30 add flat
stats. A stat record is `<zig stat> 10 <op> 24 <f32> 38 <label>`, the same shape
app/trove/codexes/bonuses.py reads.

The universal starting sheet (BASE) is in no game file - the prefabs and
Trove_x64.exe carry no such table - so it is read off the in-game character sheet.
Every class's old hand-entered sheet minus its prefab level sums gave these same
values, which is what validated them.

Everything else in classes.json is hand-maintained, so each rebuild starts from
the repo copy and replaces only `stats`, `bonuses` and `levels`.
"""
from __future__ import annotations

import re
import struct

from app.trove.codexes.binfab import read_uleb, unzig
from app.trove.codexes.bonuses import OPERATIONS, STAT_KEYS
from app.trove.decode import store
from app.trove.decode.tree import GameTree

TITLE = "Class levels"
OUTPUT = "classes.json"
PREFIXES = ("prefabs/class/",)
INDENT, FINAL_NEWLINE = 4, False
MAX_LEVEL = 30

# `$Stat_` key -> (sheet name, wire-to-sheet scale, shown as a percentage)
STATS = {
    "PhysicalDamage": ("Physical Damage", 1, False),
    "SpellDamage": ("Magic Damage", 1, False),
    "MaxHealth": ("Maximum Health", 1, False),
    "MaxEnergy": ("Energy", 1, False),
    "HealthRegen_controller": ("Health Regen", 1, False),
    "EnergyRegen_controller": ("Energy Regen", 1, False),
    "Stability": ("Stability", 1, False),
    "MovementSpeed": ("Movement Speed", 1, False),
    "AttackSpeed": ("Attack Speed", 1, True),
    "Jump": ("Jump", 1, False),
    "CriticalHitChance": ("Critical Hit", 0.1, True),  # stored in tenths of a percent
    "CriticalHitDamage": ("Critical Damage", 1, True),
    "Light": ("Light", 1, False),
    "JumpSpeedMultiplier": ("Jump Speed", 100, True),
}

# The character sheet before any level: (name, value, percentage), in sheet order.
BASE = [
    ("Physical Damage", 100, False),
    ("Magic Damage", 100, False),
    ("Maximum Health", 150, False),
    ("Maximum Health %", 0, True),
    ("Energy", 100, False),
    ("Health Regen", 25, False),
    ("Energy Regen", 100, False),
    ("Movement Speed", 40, False),
    ("Attack Speed", 100, True),
    ("Jump", 3, False),
    ("Critical Hit", 2, True),
    ("Critical Damage", 50, True),
    ("Light", 0, False),
]
BONUS_NAMES = ("Physical Damage", "Magic Damage", "Health Regen")

_STAT = re.compile(rb"\x10(.)\x24(.{4})\x38", re.S)
_RECORD_BODY = b"\x3e"


def _num(v: float) -> float | int:
    v = round(v, 4)
    return int(v) if v == int(v) else v


def level_records(data: bytes) -> list[tuple[int, int]]:
    """`[(start, end)]` byte span of each level record, index 0 = level 1."""
    found: dict[int, int] = {}
    for m in re.finditer(rb"[\x00\x1e]([\x80-\xff]*[\x00-\x7f])\x38", data):
        key, pos = read_uleb(data, m.start() + 1)
        level = key >> 4
        if (key & 0xF) != 2 or not 1 <= level <= MAX_LEVEL or level in found:
            continue
        if level > 1 and level - 1 not in found:
            continue
        length, body = read_uleb(data, pos + 1)
        if data[body + length:body + length + len(_RECORD_BODY)] == _RECORD_BODY:
            found[level] = m.start()
    if sorted(found) != list(range(1, MAX_LEVEL + 1)):
        raise ValueError(f"level list incomplete: {sorted(found)}")
    end = data.find(b"ui/classes/", found[MAX_LEVEL])
    if end < 0:
        raise ValueError("no class icon after the level list")
    starts = [found[lv] for lv in range(1, MAX_LEVEL + 1)] + [end]
    return list(zip(starts, starts[1:], strict=False))


def stat_records(data: bytes, start: int, end: int) -> list[tuple[str, str, float]]:
    out = []
    for m in _STAT.finditer(data, start, end):
        stat, op = unzig(data[m.start() - 1]), data[m.start() + 1]
        if stat in STAT_KEYS and op in OPERATIONS:
            out.append((STAT_KEYS[stat][len("$Stat_"):], OPERATIONS[op],
                        struct.unpack("<f", m.group(2))[0]))
    return out


def decode(data: bytes) -> dict:
    totals = {name: float(v) for name, v, _ in BASE}
    fixed: dict[str, float | None] = {}
    bonuses = dict.fromkeys(BONUS_NAMES, 0.0)
    levels: dict[str, list[dict]] = {}

    for level, (start, end) in enumerate(level_records(data), 1):
        rows: dict[str, dict] = {}
        for key, op, value in stat_records(data, start, end):
            if key not in STATS:
                raise ValueError(f"level {level}: unmapped stat {key}")
            name, scale, pct = STATS[key]
            if op == "Add":
                row = rows.setdefault(name, {"name": name, "value": 0.0, "percentage": pct})
                row["value"] += value * scale
            elif op == "MultiplySum":
                row = rows.setdefault(f"{name} %", {"name": f"{name} %", "value": 0.0, "percentage": True})
                row["value"] += value * 100
            elif op == "Multiply" and value == 0:
                fixed[name] = None
            elif op == "Multiply":
                if name not in bonuses:
                    raise ValueError(f"level {level}: unexpected multiplier on {name}")
                bonuses[name] += (value - 1) * 100
            elif op == "Set":
                fixed[name] = value * scale
            else:
                raise ValueError(f"level {level}: unexpected {op} on {name}")
        for row in rows.values():
            row["value"] = _num(row["value"])
            if row["name"] in fixed:
                raise ValueError(f"level {level}: {row['name']} grows but is fixed")
            if row["name"] in totals:
                totals[row["name"]] += row["value"]
        if rows:
            levels[str(level)] = list(rows.values())

    stats = []
    for name, _, pct in BASE:
        value = fixed[name] if name in fixed else totals[name]
        stats.append({"name": name, "value": None if value is None else _num(value), "percentage": pct})
    return {
        "stats": stats,
        "bonuses": [{"name": n, "value": _num(v), "percentage": True} for n, v in bonuses.items()],
        "levels": levels,
    }


def build(tree: GameTree) -> list[dict]:
    classes = store.baseline(OUTPUT)
    for c in classes:
        data = tree.read(f"prefabs/class/{c['qualified_name']}.binfab")
        if data is None:
            raise ValueError(f"no class prefab for {c['qualified_name']}")
        c.update(decode(data))
    return classes


def count(data: list) -> int:
    return len(data)

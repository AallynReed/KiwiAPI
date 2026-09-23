"""The wiki's /pvp-stats page, shaped from pvp.json (app/trove/decode/pvp_stat_ranges.py)."""
from __future__ import annotations

import math

from app.trove.decode.pvp_stat_ranges import ROLE_MODES, pvp_value

MODES = (("pvp", "PvP"), ("battleroyale", "Battle Royale"), ("bloodstone", "Bloodstone"))

# (stat, sheet name, shown as %), in character-sheet order; same as calculators.js PVP_SHEET.
SHEET = (
    ("PhysicalDamage", "Physical Damage", False),
    ("SpellDamage", "Magic Damage", False),
    ("MaxHealth", "Maximum Health", False),
    ("MaxEnergy", "Maximum Energy", False),
    ("HealthRegen_controller", "Health Regen", False),
    ("EnergyRegen_controller", "Energy Regen", False),
    ("MovementSpeed", "Movement Speed", False),
    ("AttackSpeed", "Attack Speed", True),
    ("Jump", "Jump", False),
    ("CriticalHitChance", "Critical Hit", True),
    ("CriticalHitDamage", "Critical Damage", True),
)
NAMES = {stat: name for stat, name, _ in SHEET}
PCT = {stat: pct for stat, _, pct in SHEET}

# PvE sheet values for the examples table.
EXAMPLES = (
    ("MaxHealth", (30_000, 100_000, 300_000)),
    ("PhysicalDamage", (1_000, 10_000, 50_000)),
    ("CriticalHitDamage", (200, 500, 1_500)),
    ("CriticalHitChance", (20, 50, 100)),
    ("MovementSpeed", (60, 100, 150)),
    ("MaxEnergy", (100, 200, 400)),
)


def fmt(v: float, pct: bool = False) -> str:
    if abs(v) >= 1e6:
        s = f"{v / 1e6:.1f}".removesuffix(".0") + " million"
    elif abs(v) >= 1000:
        s = f"{v:,.0f}"
    else:
        s = f"{v:,.1f}".removesuffix(".0")
    return s + ("%" if pct else "")


def mods_for(role: dict | None, stat: str) -> list[dict]:
    return [m for m in role["modifiers"] if m["stat"] == stat] if role else []


def convert(modes: dict, mode: str, stat: str, sheet: float, role: dict | None = None) -> float | None:
    """A PvE sheet value in `mode`, in sheet units; None when the mode leaves the stat alone."""
    e = modes.get(mode, {}).get(stat)
    if e is None:
        return None
    mods = mods_for(role, stat) if mode in ROLE_MODES else []
    return pvp_value(e, sheet / e["scale"], mods) * e["scale"]


def mult(v: float) -> str:
    return f"×{v:.2f}"


def _rule(stat: str, e: dict | None) -> dict:
    """One table row, in sheet units."""
    pct = PCT[stat]
    row = {"stat": stat, "name": NAMES[stat]}
    if e is None:
        return row | {"text": "Unchanged from PvE"}
    s = e["scale"]
    lo, hi = e["out"]
    soft = e["cap"] > hi
    past = f"Creeps toward {fmt(e['cap'] * s, pct)}" if soft else "Hard stop"
    if lo == hi:
        return row | {"text": f"Fixed at {fmt(lo * s, pct)} for everyone"}
    if not e["curve"]:
        return row | {"text": f"Carries over as is, up to {fmt(hi * s, pct)}"
                              + (f", then creeps toward {fmt(e['cap'] * s, pct)}" if soft else "")}
    knee = e["knee"]
    needed = knee + ((hi - knee) / e["sqrt"]) ** 2 if e["sqrt"] > 0 and hi > knee else None
    return row | {
        "min": fmt(max(knee, lo) * s, pct) if e["floor"] else None,
        "knee": fmt(knee * s, pct),
        "cap": fmt(hi * s, pct),
        "needed": fmt(math.ceil(needed * s), pct) if needed is not None else None,
        "past": past,
    }


def page(data: dict) -> dict:
    """Template context for /pvp-stats."""
    modes = data["modes"]
    base = modes.get("pvp", {})
    out = []
    for key, label in MODES:
        entries = modes.get(key)
        if entries is None:
            continue
        stats = [stat for stat, _, _ in SHEET]
        changed = [stat for stat in stats if entries.get(stat) != base.get(stat)]
        # A mode mostly like PvP lists only the stats it changes.
        diff_only = key != "pvp" and len(changed) * 2 < len(stats)
        raw = [{"name": NAMES[stat], **entries[stat]} for stat in stats if stat in entries]
        out.append({
            "key": key, "label": label, "diff_only": diff_only,
            "changed": [NAMES[s] for s in changed],
            "rows": [_rule(stat, entries.get(stat)) for stat in (changed if diff_only else stats)],
            "raw": raw,
        })
    examples = [{
        "name": NAMES[stat], "pct": PCT[stat],
        "rows": [{"pve": fmt(v, PCT[stat]),
                  "out": [None if (r := convert(modes, m["key"], stat, v)) is None else fmt(r, PCT[stat])
                          for m in out]}
                 for v in values],
    } for stat, values in EXAMPLES]
    roles = data.get("roles", [])
    by_key = {r["key"]: r for r in roles}
    role_rows = [{
        "name": r["name"],
        "classes": [name for _, name in r["classes"]],
        "modifiers": [{"label": m["label"], "mult": mult(m["value"])} for m in r["modifiers"]],
    } for r in roles]
    # Maximum Health at one PvE value, per role, in PvP.
    hp_sample = 100_000
    role_hp = [{"name": r["name"], "value": fmt(convert(modes, "pvp", "MaxHealth", hp_sample, r) or 0)}
               for r in roles]
    crit = modes.get("bloodstone", {}).get("CriticalHitChance")
    return {
        "modes": out,
        "examples": examples,
        "roles": role_rows,
        "role_hp": role_hp,
        "hp_sample": fmt(hp_sample),
        "ex": lambda mode, stat, v, role=None: fmt(convert(modes, mode, stat, v, by_key.get(role)) or 0,
                                                   PCT[stat]),
        "rule": lambda mode, stat: _rule(stat, modes.get(mode, {}).get(stat)),
        # Bloodstone's crit range reads like percent typed into a tenths-of-a-percent field.
        "bloodstone_crit": crit and crit["out"][1] < base.get("CriticalHitChance", {}).get("knee", 0)
                           and {"cap": fmt(crit["out"][1] * crit["scale"], True),
                                "raw": fmt(crit["out"][1], True)},
    }

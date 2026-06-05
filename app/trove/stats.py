"""Raw game-stat data: the calculator stat tables + class definitions.

This module only *transmits* BetterTroveTools' static JSON in a cleaned-up,
typed shape — it runs none of the calculators. We expose what each source/field
is and how much it contributes, plus full class objects keyed by a stable
``tech_name`` token so a class can be referenced later by that token alone.

Sources (copied into gamedata/):
  - stats/power_rank.json · stats/magic_find.json · stats/light.json
  - classes.json
"""

from __future__ import annotations

import json
from functools import cache, lru_cache
from pathlib import Path

_DATA_DIR = Path(__file__).parent / "gamedata"
_STATS_DIR = _DATA_DIR / "stats"

# Each calculator stat table -> (file, human label).
_STAT_TABLES: dict[str, tuple[str, str]] = {
    "power-rank": ("power_rank.json", "Power Rank"),
    "magic-find": ("magic_find.json", "Magic Find"),
    "light": ("light.json", "Light"),
}

STAT_TABLE_KEYS = tuple(_STAT_TABLES)


@cache
def _read_json(path: str) -> object:
    return json.loads(Path(path).read_text(encoding="utf-8"))


# --- Stat-source tables (power rank / magic find / light) ------------------


def _clean_source(s: dict) -> dict:
    """One row of a stat table: a source and how much it contributes."""
    return {
        "name": s["name"],
        "value": s["value"],
        "type": s.get("type", "switch"),        # "slider" | "switch" (input kind)
        "percentage": s.get("percentage", False),  # value is a % bonus vs a flat amount
        "step": s.get("step"),                   # slider increment, when present
        "permanent": s.get("perm"),              # light: permanent vs temporary buff
    }


def stat_table(table: str) -> dict | None:
    """A calculator stat table by key, or None if the key is unknown."""
    entry = _STAT_TABLES.get(table)
    if entry is None:
        return None
    filename, label = entry
    raw = _read_json(str(_STATS_DIR / filename))
    sources = [_clean_source(s) for s in raw]  # type: ignore[union-attr]
    return {"stat": table, "label": label, "sources": sources, "count": len(sources)}


# --- Classes ---------------------------------------------------------------


def _clean_stat(s: dict) -> dict:
    return {"name": s["name"], "value": s.get("value"), "percentage": s.get("percentage", False)}


def _clean_stage(st: dict) -> dict:
    return {"name": st.get("name", ""), "base": st.get("base", 0), "multiplier": st.get("multiplier", 0)}


def _clean_ability(a: dict) -> dict:
    return {
        "name": a.get("name", ""),
        "icon": a.get("icon", ""),
        "type": a.get("type", ""),
        "stages": [_clean_stage(st) for st in a.get("stages", [])],
    }


def _clean_subclass(sc: dict) -> dict:
    return {
        "name": sc.get("name", ""),
        "description": sc.get("description", ""),
        # level -> the stat bonuses granted at that subclass level
        "level": {lvl: [_clean_stat(x) for x in stats] for lvl, stats in sc.get("level", {}).items()},
        # power milestone -> human-readable effect (often blank in the source)
        "power": sc.get("power", {}),
    }


def _clean_class(c: dict) -> dict:
    return {
        "tech_name": c["qualified_name"],   # the canonical token used to reference this class
        "name": c["name"],                  # display name
        "shorts": c.get("shorts", []),
        "damage_type": c.get("damage_type", ""),
        "weapons": c.get("weapons", []),
        "attributes": c.get("attributes", []),
        "stats": [_clean_stat(s) for s in c.get("stats", [])],
        "bonuses": [_clean_stat(s) for s in c.get("bonuses", [])],
        "subclass": _clean_subclass(c.get("subclass", {})),
        "abilities": [_clean_ability(a) for a in c.get("abilities", [])],
    }


@lru_cache(maxsize=1)
def _classes_index() -> dict[str, dict]:
    """tech_name -> cleaned class object, in source order (preserved by dict)."""
    raw = _read_json(str(_DATA_DIR / "classes.json"))
    return {c["qualified_name"]: _clean_class(c) for c in raw}  # type: ignore[union-attr]


def all_classes() -> dict:
    """Every class as a full object (each carries its ``tech_name``)."""
    items = list(_classes_index().values())
    return {"items": items, "count": len(items)}


def class_by_tech_name(tech_name: str) -> dict | None:
    """A single class looked up by its ``tech_name`` token, or None."""
    return _classes_index().get(tech_name)

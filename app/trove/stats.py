"""Raw game-stat data: the calculator stat tables + class definitions.

This module only *transmits* BetterTroveTools' static JSON in a cleaned-up,
typed shape - it runs none of the calculators. We expose what each source/field
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


# --- Coefficient (derived damage metric) -----------------------------------

# The in-game character "Coefficient": effective damage folding in crit. Shared by
# the OCR derivation (app/trove/ocr/parse.py) and POST /v1/stats/coefficient so the
# two can never drift. The game TRUNCATES (floors) the result.
COEFFICIENT_FORMULA = "floor(max(physical_damage, magic_damage) * (1 + critical_damage / 100))"


def compute_coefficient(
    physical_damage: float | None,
    magic_damage: float | None,
    critical_damage: float | None,
) -> tuple[int, str] | None:
    """Coefficient = ``floor(D * (1 + critical_damage / 100))`` where ``D`` is the
    HIGHER of physical / magic damage. ``critical_damage`` is a percent (e.g.
    ``3438.3`` for 3,438.3%). Returns ``(coefficient, "physical" | "magic")``, or
    ``None`` if there's no damage value or no crit damage to compute from."""
    candidates = [(v, name) for v, name in
                  ((physical_damage, "physical"), (magic_damage, "magic")) if v is not None]
    if not candidates or critical_damage is None:
        return None
    base, which = max(candidates, key=lambda c: c[0])
    return int(round(base * (1 + critical_damage / 100), 6)), which


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


# --- Class ↔ leaderboard-board mapping (for the Class Activity feature) ------
# The class leaderboards live in three parallel ID ranges: board ``1000+i``
# (Power Rank), ``4000+i`` (Effort) and ``5000+i`` (Paragon) all belong to class
# index ``i``, so ``class_index = board_uuid % 1000``. The index order is the
# game's class RELEASE order (0=Knight … 17=Solarion) - NOT classes.json's
# alphabetical order. ``_BOARD_CLASS_ORDER`` below is the authoritative
# board-index → class mapping, verified against the real board names stored in
# the DB (2026-07-17: board 4000 = "KNIGHT", 4016 = "BARD", and 1000+i/5000+i
# name the same class as 4000+i for every i). If a class ever looks mislabelled
# again, re-dump the board names and fix THIS list - never guess.
_EFFORT_BASE = 4000
_PARAGON_BASE = 5000
_POWER_RANK_BASE = 1000

# Board index -> classes.json tech_name, in the boards' release order.
_BOARD_CLASS_ORDER: tuple[str, ...] = (
    "knight",          # 0  - 1000/4000/5000: KNIGHT
    "gunslinger",      # 1  - GUNSLINGER
    "faetrickster",    # 2  - FAE TRICKSTER
    "dracolyte",       # 3  - DRACOLYTE
    "neonninja",       # 4  - NEON NINJA
    "candybarbarian",  # 5  - CANDY BARBARIAN
    "icemage",         # 6  - ICE SAGE
    "shadowhunter",    # 7  - SHADOW HUNTER
    "piratelord",      # 8  - PIRATE CAPTAIN
    "adventurer",      # 9  - BOOMERANGER
    "tombraiser",      # 10 - TOMB RAISER
    "lunarlancer",     # 11 - LUNAR LANCER
    "spirittank",      # 12 - REVENANT
    "chloromancer",    # 13 - CHLOROMANCER
    "dinotamer",       # 14 - DINO TAMER
    "crimefighter",    # 15 - VANGUARDIAN
    "bard",            # 16 - BARD
    "solarion",        # 17 - SOLARION
)


def _class_for_index(i: int) -> dict | None:
    """The cleaned class object for board/class index ``i`` (release order), or
    None when the index is out of range or its tech_name isn't in classes.json
    (e.g. a brand-new class whose board exists before the static data update)."""
    if 0 <= i < len(_BOARD_CLASS_ORDER):
        return _classes_index().get(_BOARD_CLASS_ORDER[i])
    return None


def class_count() -> int:
    """Number of known classes (drives the board ranges + chart palette)."""
    return len(_BOARD_CLASS_ORDER)


def class_name(i: int) -> str:
    """Display name for class index ``i`` (board release order); falls back to a
    generic label if the index is unknown (e.g. a new class not yet mapped)."""
    c = _class_for_index(i)
    return c["name"] if c is not None else f"Class {i}"


def class_index_for_board(uuid: int) -> int:
    """Class index for a Power Rank (1000+i) / Effort (4000+i) / Paragon (5000+i)
    board uuid."""
    return uuid % 1000


def class_icon(i: int) -> str | None:
    """Self-hosted class-icon URL for class index ``i``. The PNGs were downloaded
    from trovesaurus (``ui_class_<qualified_name>.png``) into
    ``site/static/class-icons/<qualified_name>.png`` so we serve them ourselves."""
    c = _class_for_index(i)
    return f"/static/class-icons/{c['tech_name']}.png" if c is not None else None


def class_board_uuids() -> list[int]:
    """Every Effort + Paragon board uuid for the known classes."""
    n = class_count()
    return [_EFFORT_BASE + i for i in range(n)] + [_PARAGON_BASE + i for i in range(n)]


def class_effort_board_uuids() -> list[int]:
    """Every Effort board uuid - the sole basis for class-activity counts. Paragon
    (5000+i) is intentionally excluded: its scores are ambiguous, so we neither
    count nor filter on it."""
    return [_EFFORT_BASE + i for i in range(class_count())]


def class_effort_board_uuid(i: int) -> int:
    """Effort board uuid for class index ``i`` (= 4000+i)."""
    return _EFFORT_BASE + i


def class_paragon_board_uuid(i: int) -> int:
    """Paragon board uuid for class index ``i`` (= 5000+i)."""
    return _PARAGON_BASE + i


def class_pr_board_uuid(i: int) -> int:
    """Power Rank board uuid for class index ``i`` (= 1000+i)."""
    return _POWER_RANK_BASE + i


def class_pr_board_uuids() -> list[int]:
    """Every Power Rank board uuid for the known classes (the clean-view gate)."""
    return [_POWER_RANK_BASE + i for i in range(class_count())]

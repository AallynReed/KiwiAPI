"""Server-side render model for /allies.

The full ally table: all 1,200 pet prefabs that grant combat stats, decoded by
scripts/decode_ally_abilities.py. /abilities lists only the 127 that also carry
an ability, because that page is about abilities; this one is about the stats,
which is what you are actually shopping for.

Every row is rendered and the client sorts and filters in place - 1,200 rows is
small enough for the DOM and means the table works with JS switched off (sorted
by name, which is the useful default anyway).
"""
import json
import re
from functools import cache
from pathlib import Path
from typing import Any

_DATA = Path(__file__).resolve().parents[1] / "trove" / "gamedata"

_SLUG = re.compile(r"[^a-z0-9]+")


def slug(name: str) -> str:
    """`Maximum Health %` -> `maximum-health`, for the per-stat data attributes."""
    return _SLUG.sub("-", name.lower()).strip("-")


def _fmt(stat: dict) -> str:
    """The value as it reads on a card: `+350`, `+25%`, `x1.5`."""
    value, op = stat.get("value") or 0, stat.get("op", "")
    if op == "Add":
        return f"+{value:g}"
    if op == "Multiply":
        amount = stat.get("amount")
        return f"x{amount:g}" if amount else f"{value:g}%"
    return f"+{value:g}%"


@cache
def _load() -> list[dict]:
    try:
        return json.loads((_DATA / "ally_abilities.json").read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return []


def allies_view(allies: list[dict] | None = None) -> dict[str, Any]:
    """`{rows, stats, count}` - one row per ally, sortable by any stat it grants."""
    allies = _load() if allies is None else allies
    rows = []
    seen: set[str] = set()
    for ally in allies:
        stats = []
        # NOT "values": in a template `row.values` resolves to dict.values,
        # the built-in method, and the row renders no sort attributes at all.
        sortable: dict[str, float] = {}
        for stat in ally.get("stats") or []:
            name = stat.get("name") or ""
            if not name:
                continue
            seen.add(name)
            stats.append({"name": name, "slug": slug(name), "shown": _fmt(stat)})
            # Sorting is on the decoded number, not the formatted string, and the
            # biggest wins where an ally somehow lists a stat twice.
            sortable[slug(name)] = float(
                max(sortable.get(slug(name), 0), abs(stat.get("value") or 0)))
        if not stats:
            continue
        powers = [a["text"] for a in (ally.get("abilities") or []) if a.get("text")]
        name = ally.get("name") or ally.get("slug") or ""
        rows.append({
            "name": name,
            "stats": stats,
            "sort_values": sortable,
            "ability": " ".join(powers).strip(),
            "search": " ".join([name, *(s["name"] for s in stats), *powers]).lower(),
        })

    rows.sort(key=lambda r: r["name"].lower())
    stats = [{"name": n, "slug": slug(n)} for n in sorted(seen)]
    return {"rows": rows, "stats": stats, "count": len(rows)}

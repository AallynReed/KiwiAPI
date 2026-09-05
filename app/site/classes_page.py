"""Server-side render model for /classes.

The class picker + the initially-selected class's full detail are server-rendered
(English) so the page is complete without JS and paints instantly; ``classes.js``
then fetches the full set once to power switching, hydrates the existing nav, and
re-renders the detail only when a different class is chosen (hash deep-link, a
click, or a language switch).

Mirrors ``classes.js`` buildNav/renderDetail so the pre-rendered DOM matches what
the JS would build (same classes, ``data-tech``, section order).
"""
from typing import Any

from app.trove import stats as trove_stats


def _fmt_stat(s: dict) -> str:
    """``{value, percentage}`` -> "131%" / "2,376" - matches classes.js fmtStat
    (thousands separators, up to 2 fraction digits, trailing zeros trimmed)."""
    v = s.get("value")
    if not isinstance(v, (int, float)) or isinstance(v, bool):
        v = 0
    if v == int(v):
        num = f"{int(v):,}"
    else:
        num = f"{v:,.2f}".rstrip("0").rstrip(".")
    return f"{num}%" if s.get("percentage") else num


def _fmt_bonus(s: dict) -> str:
    """Same as ``_fmt_stat`` but signed, for values shown as a bonus - matches
    classes.js fmtBonus. ``absolute`` stats (Fae Trickster's Flying Speed) name a
    resulting value rather than a delta, so they carry no sign."""
    out = _fmt_stat(s)
    if s.get("absolute"):
        return out
    return out if out.startswith("-") else f"+{out}"


def _dmg_class(damage_type: str | None) -> str:
    return "physical" if (damage_type or "").lower() == "physical" else "magic"


def _meaningful(rows: list | None) -> list:
    """A subclass bonus is worth showing only with a real name or a non-zero
    value (some classes carry all-zero placeholder rows)."""
    return [b for b in (rows or []) if b and ((b.get("name") or "").strip() or b.get("value"))]


def _fmt_stages(stages: list) -> list[dict]:
    """An ability's damage rows, ready to print.

    `multiplier` scales the class's damage stat, so it reads as a percentage -
    the wire's 3.5 is 350%. `base` is a flat add, shown only when it is there.
    Mirrors fmtStage() in classes.js so SSR and the client agree.
    """
    out = []
    for s in stages:
        mult, base = s.get("multiplier") or 0, s.get("base") or 0
        if not mult and not base:
            continue
        out.append({
            "name": s.get("name") or "",
            "damage": f"{mult * 100:g}%" if mult else "",
            "base": f"+{base:g}" if base else "",
        })
    return out


def _detail(c: dict) -> dict:
    stats = [{"name": s["name"], "val": _fmt_stat(s)}
             for s in (c.get("stats") or []) if s and s.get("name")]
    bonuses = [{"name": b["name"], "val": _fmt_bonus(b)}
               for b in (c.get("bonuses") or []) if b and b.get("name") and b.get("value")]

    sc = c.get("subclass") or {}
    levels = sc.get("level") or {}
    tiers = []
    for tier in sorted(levels.keys(), key=lambda k: int(k)):
        meaningful = _meaningful(levels[tier])
        if not meaningful:
            continue
        tiers.append({
            "tier": tier,
            "bonuses": [{"name": (b.get("name") or "").strip(), "val": _fmt_bonus(b)}
                        for b in meaningful],
        })
    subclass = None
    if sc.get("name") or sc.get("description") or tiers:
        subclass = {
            "name": sc.get("name") or "",
            "description": sc.get("description") or "",
            "tiers": tiers,
        }

    rows = [{
        "name": a.get("name") or "",
        "description": a.get("description") or "",
        "type": a.get("type") or "",
        "inactive": a.get("active") is False,
        "stages": _fmt_stages(a.get("stages") or []),
    } for a in (c.get("abilities") or []) if a and (a.get("name") or a.get("description"))]
    # Abilities the live class prefab no longer reaches still load in game, but
    # they are not what the class does now - so they sit in their own fold.
    abilities = [a for a in rows if not a["inactive"]]
    legacy = [a for a in rows if a["inactive"]]

    return {
        "tech": c["tech_name"],
        "name": c.get("name") or "",
        "damage_type": c.get("damage_type") or "",
        "dmg_class": _dmg_class(c.get("damage_type")),
        "weapons": c.get("weapons") or [],
        "shorts": " / ".join(c.get("shorts") or []),
        "stats": stats,
        "bonuses": bonuses,
        "subclass": subclass,
        "abilities": abilities,
        "legacy": legacy,
    }


def classes_view() -> dict[str, Any]:
    """Nav entries for every class + the full detail model for the first one
    (the client default; a URL hash overrides it after JS loads)."""
    items = (trove_stats.all_classes() or {}).get("items") or []
    nav = [{
        "tech": c["tech_name"],
        "name": c.get("name") or "",
        "damage_type": c.get("damage_type") or "",
        "dmg_class": _dmg_class(c.get("damage_type")),
    } for c in items]
    return {
        "nav": nav,
        "initial": _detail(items[0]) if items else None,
        "initial_tech": items[0]["tech_name"] if items else "",
        "count": len(items),
    }

"""Render model for a class's detail (``partials/class_detail.html``), used by
the wiki's class pages."""
from app.wiki.ability_view import card


def _fmt_stat(s: dict) -> str:
    """``{value, percentage}`` -> "131%" / "2,376"
    (thousands separators, up to 2 fraction digits, trailing zeros trimmed).
    A null value is a stat the class does not have."""
    v = s.get("value")
    if v is None:
        return "—"
    if not isinstance(v, (int, float)) or isinstance(v, bool):
        v = 0
    if v == int(v):
        num = f"{int(v):,}"
    else:
        num = f"{v:,.2f}".rstrip("0").rstrip(".")
    return f"{num}%" if s.get("percentage") else num


def _fmt_bonus(s: dict) -> str:
    """Same as ``_fmt_stat`` but signed, for values shown as a bonus.
    ``absolute`` stats (Fae Trickster's Flying Speed) name a
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


def _growth(levels: dict) -> dict | None:
    """``{level: [stat]}`` -> a level-by-stat table."""
    order = sorted(levels, key=int)
    cols: list[str] = []
    for lvl in order:
        for s in levels[lvl]:
            if s.get("name") and s["name"] not in cols:
                cols.append(s["name"])
    if not cols:
        return None
    rows = []
    for lvl in order:
        by_name = {s["name"]: s for s in levels[lvl] if s.get("name")}
        rows.append({"level": lvl, "cells": [_fmt_bonus(by_name[n]) if n in by_name else "" for n in cols]})
    return {"cols": cols, "rows": rows}


def _sheet_at(stat: dict, levels: dict, level: int):
    """A level-30 sheet value wound back to ``level``."""
    v = stat.get("value")
    if not isinstance(v, (int, float)) or isinstance(v, bool):
        return v
    for lvl, rows in levels.items():
        if int(lvl) > level:
            v -= sum(r.get("value") or 0 for r in rows if r.get("name") == stat["name"])
    return round(v, 2)


def _totals(stats: list, levels: dict) -> dict | None:
    """The sheet at every level, limited to the stats that change with level."""
    if not levels:
        return None
    grid = [[_sheet_at(s, levels, lv) for s in stats] for lv in range(1, 31)]
    keep = [i for i in range(len(stats)) if grid[0][i] != grid[-1][i]]
    if not keep:
        return None
    return {
        "cols": [stats[i]["name"] for i in keep],
        "rows": [{"level": str(lv), "cells": [_fmt_stat({"value": grid[lv - 1][i], "percentage": stats[i].get("percentage")})
                                              for i in keep]} for lv in range(1, 31)],
    }


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
        **card(a),
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
        "growth": _growth(c.get("levels") or {}),
        "totals": _totals([s for s in (c.get("stats") or []) if s and s.get("name")], c.get("levels") or {}),
        "bonuses": bonuses,
        "subclass": subclass,
        "abilities": abilities,
        "legacy": legacy,
    }


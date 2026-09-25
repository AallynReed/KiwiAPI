"""Display model for an ability, shared by the wiki's class, ally and mount pages.

The decoders (app/trove/decode/*_abilities.py) emit raw numbers - a multiplier of
3.5, a heal of 0.15 of max health, an effect lasting 12.0 - and this turns one
ability into the rows a card prints. Nothing here reads game files.
"""
from __future__ import annotations

from typing import Any
from urllib.parse import quote

from app.core.config import settings
from app.trove.codexes.bonuses import _normalize
from app.trove.codexes.localize import resolve_stat_name

_OPS = ("MultiplySum", "Add", "Set", "Nullify", "Multiply", "Minimum", "Maximum")


def num(v: float | int) -> str:
    """350.0 -> "350", 0.466667 -> "0.47", 12000 -> "12,000"."""
    v = float(v)
    if v == int(v):
        return f"{int(v):,}"
    return f"{v:,.2f}".rstrip("0").rstrip(".")


def seconds(v: float | int) -> str:
    return f"{num(v)}s"


def _is_percent(row: dict) -> bool:
    """Whether a stat row reads as a percentage - the codexes' rule (``bonuses._normalize``),
    for rows that don't carry the flag themselves (ally stats)."""
    if "percent" in row:
        return bool(row["percent"])
    op = row.get("op")
    if op not in _OPS:
        return False
    return _normalize(row.get("stat", ""), _OPS.index(op) * 2, row.get("amount", 0) or 0)[1]


def stat_text(row: dict) -> str:
    """One stat modifier as a player reads it: "+30% Critical Damage", "Movement Speed 90".

    A MultiplySum keeps its "… Bonus" name, as in the codexes: +30% Critical Damage
    (an Add, 30 points) and +30% Critical Damage Bonus (scales the stat) differ.
    """
    name = row.get("name") or resolve_stat_name({}, row.get("stat", ""))
    op, value = row.get("op"), row.get("value", 0) or 0
    amount = row.get("amount")
    # Ally rows store |value|; the raw amount still carries the sign (a Multiply
    # of 0.9 is 10% LESS incoming damage, not more).
    if isinstance(amount, (int, float)):
        if op == "Multiply":
            value = (amount - 1) * 100
        elif op in ("Add", "MultiplySum") and amount < 0:
            value = -abs(value)
    unit = "%" if _is_percent(row) else ""
    if op in ("Add", "MultiplySum", "Multiply"):
        sign = "+" if value >= 0 else "-"
        return f"{sign}{num(abs(value))}{unit} {name}"
    if op == "Set":
        return f"{name} {num(value)}{unit}"
    if op == "Nullify":
        return f"No {name} bonuses"
    if op == "Minimum":
        return f"{name} at least {num(value)}{unit}"
    if op == "Maximum":
        return f"{name} at most {num(value)}{unit}"
    return f"{name} {num(value)}{unit}"


def _damage(stage: dict) -> dict:
    parts, notes = [], []
    if stage.get("multiplier"):
        parts.append(f"{num(stage['multiplier'] * 100)}%")
    if stage.get("base"):
        parts.append(f"+{num(stage['base'])}")
    # 100.0 sits in ~80 records across every ability file, far above the next value
    # (5.0); it isn't a share of max health, and what it is isn't known, so it's left out.
    if stage.get("max_health_percent") and stage["max_health_percent"] < 100:
        parts.append(f"{num(stage['max_health_percent'] * 100)}% max health")
    if stage.get("crit_chance_bonus"):
        notes.append(f"+{num(stage['crit_chance_bonus'])} critical hit")
    if stage.get("energy_drain"):
        notes.append(f"drains {num(stage['energy_drain'])} energy")
    return {"name": stage.get("name") or "", "value": " ".join(parts), "notes": notes}


def _heal(row: dict) -> dict:
    parts = []
    if row.get("max_health_percent") and row["max_health_percent"] < 100:
        parts.append(f"{num(row['max_health_percent'] * 100)}% max health")
    if row.get("health"):
        parts.append(f"{num(row['health'])} health")
    if row.get("max_energy_percent"):
        parts.append(f"{num(row['max_energy_percent'] * 100)}% max energy")
    if row.get("energy"):
        parts.append(f"{num(row['energy'])} energy")
    return {"name": row.get("name") or "", "value": " + ".join(parts)}


def _effect(row: dict) -> dict | None:
    stats = [stat_text(s) for s in row.get("stats") or []]
    duration = seconds(row["duration"]) if row.get("duration") else ""
    if not stats and not duration:
        return None
    # A nameless, statless effect that ends at once is the carrier of a hit, not a
    # buff or debuff anyone would notice.
    if not stats and not row.get("title") and (row.get("duration") or 0) < 0.5:
        return None
    return {"name": row.get("title") or row.get("name") or "", "duration": duration, "stats": stats,
            "description": row.get("description") or ""}


def vfx_links(rows: list[dict]) -> list[dict]:
    """The site's viewer, opened on each visual effect the ability plays."""
    base = settings.app_url.rstrip("/")
    out = []
    for row in rows or []:
        pkfx = row.get("pkfx") or ""
        if not pkfx:
            continue
        name = pkfx.rsplit("/", 1)[-1]
        label = row.get("key", "").removeprefix("$VFX_").replace("_", " ").title() or name
        out.append({"label": label, "url": f"{base}/embed/viewer?game={quote(name)}&mode=vfx"})
    return out


def card(a: dict[str, Any], *, name: str = "", description: str = "", kind: str = "") -> dict:
    """``a`` is one decoded ability (class, ally or mount shape)."""
    meta = []
    if a.get("energy"):
        meta.append({"label": "Energy", "value": num(a["energy"]) if a["energy"] > 0 else f"gains {num(-a['energy'])}"})
    if a.get("cooldown"):
        meta.append({"label": "Cooldown", "value": seconds(a["cooldown"])})
    if a.get("proc_cooldown"):
        meta.append({"label": "Triggers at most every", "value": seconds(a["proc_cooldown"])})
    for act in a.get("actions") or []:
        bits = []
        if act.get("energy"):
            bits.append(f"{num(act['energy'])} energy")
        if act.get("cooldown"):
            bits.append(f"{seconds(act['cooldown'])} cooldown")
        if bits:
            meta.append({"label": act.get("name") or "Replaced ability", "value": ", ".join(bits)})
    effects = [e for e in (_effect(r) for r in a.get("effects") or []) if e]
    return {
        "name": name or a.get("name") or "",
        "kind": kind or a.get("type") or "",
        "description": description or a.get("description") or a.get("text") or "",
        "meta": meta,
        "damage": [d for d in (_damage(s) for s in a.get("stages") or []) if d["value"]],
        "healing": [h for h in (_heal(r) for r in a.get("healing") or []) if h["value"]],
        "effects": effects,
        "vfx": vfx_links(a.get("vfx") or []),
    }

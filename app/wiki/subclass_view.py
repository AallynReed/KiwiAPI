"""The subclass power table on a class page, from subclass_abilities.json.

One row per tier the game unlocks by the subclass class's Power Rank (Fae
Trickster's wing speed by its level instead): when it triggers, then what each
part does. A tier with no parts is shown as having no effect, not hidden.
"""
from __future__ import annotations

import re
from typing import Any

from app.trove.decode import store as gamedata
from app.wiki.ability_view import card, num, seconds

EVENTS = {"damage_dealt": "you deal damage", "damage_received": "you take damage",
          "critical_hit": "you land a critical hit", "enemy_defeated": "you defeat an enemy",
          "health_below": "your health is below {h}", "health_drops_below": "your health drops below {h}",
          "target_health_below": "you hit an enemy below {h} health"}
KINDS = {"subclass_power_rank": "Power Rank", "subclass_level": "Level"}


@gamedata.cached("subclass_abilities.json")
def _by_folder() -> dict[str, dict]:
    return {e["game_folder"]: e for e in gamedata.load("subclass_abilities.json", []) or []}


def _pct(v: float) -> str:
    return f"{num(round(v * 100, 2))}%"


def _trigger(t: dict) -> str:
    health = _pct(t["health"]) if t.get("health") else ""
    when = " or ".join(EVENTS.get(o, o.replace("_", " ")).format(h=health) for o in t.get("on") or [])
    lead = "Whenever" if (t.get("chance") or 0) >= 1 else f"{_pct(t['chance'])} chance when"
    bits = [f"{lead} {when}".strip()]
    if t.get("damage_type"):
        bits.append(f"{t['damage_type']} damage only")
    if t.get("cooldown"):
        bits.append(f"{seconds(t['cooldown'])} cooldown")
    return ", ".join(bits)


def _label(name: str) -> str:
    """"Dracolyte Effect Damage Tier1" -> "Dracolyte Damage"."""
    text = re.sub(r"\s*Tier\s*\d+$", "", name or "")
    return re.sub(r"\s+", " ", re.sub(r"\bEffect\b", "", text)).strip() or name


def _part(p: dict) -> dict:
    c = card(p, name=_label(p.get("name", "")))
    lines = [_trigger(t) for t in p.get("triggers") or []]
    lines += [f"{_label(d['name'])}: {d['value']}" if d["name"] else d["value"] for d in c["damage"]]
    lines += [f"Restores {h['value']}" for h in c["healing"]]
    for e in p.get("effects") or []:
        extra = []
        if e.get("life_leech"):
            extra.append(f"{_pct(e['life_leech'])} life leech")
        if e.get("reflect"):
            extra.append(f"reflects {num(e['reflect'] * 100)}% of damage taken")
        if e.get("absorbs_over_max_health"):
            extra.append(f"absorbs damage above {_pct(e['absorbs_over_max_health'])} of your health")
        if extra:
            lines.append(", ".join(extra) + (f" for {seconds(e['duration'])}" if e.get("duration") else ""))
    for e in c["effects"]:
        if e["stats"]:
            lines.append(", ".join(e["stats"]) + (f" for {e['duration']}" if e["duration"] else ""))
    if p.get("choices"):
        lines.append(f"Picks one of {len(p['choices'])} effects at random")
    if p.get("summons"):
        lines.append(f"Summons one of {len(p['summons'])} minions")
    return {"label": c["name"], "lines": lines}


def table(game_folder: str) -> dict[str, Any] | None:
    e = _by_folder().get(game_folder)
    if not e:
        return None
    rows = []
    for t in e.get("tiers") or []:
        kind = KINDS.get(t.get("threshold_kind", ""), "")
        rows.append({"threshold": f"{kind} {num(t['threshold'])}+" if kind else num(t["threshold"]),
                     "parts": [_part(p) for p in t.get("parts") or []], "overlapping": bool(t.get("overlapping"))})
    return {"name": e.get("name", ""), "description": e.get("description", ""), "rows": rows}

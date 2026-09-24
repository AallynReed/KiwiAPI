"""The wiki's world pages (/worlds, /delve-gateways, /shadow-tower) over worlds.json.

The client holds several separate world lists - portal worlds on a difficulty
ladder, biome challenge zones, Delve gateways, Shadow Tower floors - and no file
joins them (dungeons, spawns and world generation are the server's), so each page
shows its list as the game has it and joins nothing by name.
"""
from __future__ import annotations

import re
from typing import Any

from app.trove.decode import store as gamedata
from app.wiki import entities
from app.wiki.ability_view import num

WORLD_TYPES = ("Adventure Portal", "Super Adventure Portal", "Geode Portal")


def _clean(text: str | None) -> str:
    """Game text with its hotkey tokens made readable."""
    text = entities._text(text)
    return re.sub(r"\[HK:[^\]]*\]", "your interact key", text)


@gamedata.cached("worlds.json")
def _data() -> dict:
    data = gamedata.load("worlds.json", {}) or {}
    return data if isinstance(data, dict) else {}


def _requirement(r: dict, difficulties: dict[int, str]) -> str:
    if r.get("kind") == "power_rank_of_rank" and "rank" in r:
        return f"The Power Rank for {difficulties.get(r['rank'], r['rank'])} ({r['rank']})"
    if r.get("kind") == "stat" and r.get("at_least") is not None:
        return f"{r.get('stat', '')} {num(r['at_least'])} or more"
    return ""


def worlds_page() -> dict[str, Any]:
    data = _data()
    difficulties = {d["rank"]: d["name"] for d in data.get("difficulties") or []}
    groups = []
    for kind in WORLD_TYPES:
        rows = [{"name": w["name"], "description": _clean(w.get("description")),
                 "difficulty": f"{w['difficulty']} ({w['rank']})" if w.get("difficulty") else "",
                 "rank": w.get("rank", 99),
                 "requires": [x for x in (_requirement(r, difficulties) for r in w.get("requires") or []) if x],
                 "item": w.get("item_name", "")}
                for w in data.get("worlds") or [] if w.get("type") == kind]
        if rows:
            groups.append({"name": kind.replace(" Portal", " worlds"), "rows": sorted(rows, key=lambda r: r["rank"])})
    biomes = [{"name": b["name"], "description": _clean(b.get("description")), "zones": len(b.get("zones") or [])}
              for b in data.get("biomes") or []]
    return {"difficulties": data.get("difficulties") or [], "world_groups": groups, "biomes": biomes}


def gateways_page() -> dict[str, Any]:
    sections: dict[str, list[dict]] = {"Stable gateways": [], "Biome gateways": [], "Boss gateways": []}
    for g in _data().get("delve_gateways") or []:
        key = ("Boss gateways" if g.get("delve_bosses") else "Biome gateways" if g.get("delve_biomes")
               else "Stable gateways")
        crafted = [f for f in entities._sources(g.get("prefab") or "") if f["label"] == "Crafted at"]
        sections[key].append({"name": g["name"], "description": _clean(g.get("description")),
                              "depth": g.get("depth"), "crafted": crafted,
                              "search": f"{g['name']} {g.get('description', '')}".lower()})
    for rows in sections.values():
        rows.sort(key=lambda r: (r["depth"] or 0, r["name"].lower()))
    return {"sections": [{"name": k, "rows": v} for k, v in sections.items() if v],
            "count": sum(len(v) for v in sections.values())}


def shadow_tower_page() -> dict[str, Any]:
    floors = []
    for f in sorted(_data().get("shadow_tower") or [], key=lambda x: x.get("floor", 0)):
        floors.append({"floor": f.get("floor"), "name": f["name"],
                       "difficulties": [{"name": d["difficulty"], "flux": d.get("flux"),
                                         "loot": f"{d['loot_difficulty']} ({d['loot_rank']})"
                                         if d.get("loot_difficulty") else ""}
                                        for d in f.get("difficulties") or []]})
    return {"floors": floors}

"""The wiki's upgrade-tree pages over upgrade_trees.json: the Star Chart and the
profession trees (one data page each), the Geode tool modules, and each class's
Paragon powers (shown on its class page)."""
from __future__ import annotations

from typing import Any

from app.trove.decode import store as gamedata
from app.wiki import entities, links

# Data page -> the progression system it shows.
SYSTEMS = {"star-chart": "standard", "depths-of-the-angler": "fishing", "rune-anvil": "runecrafting",
           "gearcrafting-progression": "gearcrafting"}
LEADS = {
    "standard": "Every star on the Star Chart: what it grants and what it costs. A Constellation Key opens a "
                "branch; each star in it costs Celestial Spheres.",
    "fishing": "Every node of Depths of the Angler, the fishing progression, with what it grants and costs.",
    "runecrafting": "Every node of the Rune Anvil, the Runecrafting progression, with what it grants and costs.",
    "gearcrafting": "Every node of the Gearcrafter's progression, with what it unlocks and costs.",
}
MAX_DEPTH = 8


@gamedata.cached("upgrade_trees.json")
def _data() -> dict:
    data = gamedata.load("upgrade_trees.json", {}) or {}
    return data if isinstance(data, dict) else {}


def _costs(rows: list[dict] | None) -> list[dict]:
    return [links.thing(c.get("name") or c["item"], c["item"], c.get("quantity")) for c in rows or []]


def system(slug: str) -> dict | None:
    return next((s for s in _data().get("systems") or [] if s["slug"] == slug), None)


def system_page(page: str) -> dict[str, Any]:
    s = system(SYSTEMS[page]) or {"nodes": []}
    nodes = {n["id"]: n for n in s["nodes"]}
    kids: dict[str, list[str]] = {}
    for n in s["nodes"]:
        kids.setdefault(n.get("parent") or "", []).append(n["id"])
    branches = []
    for root in kids.get("", []):
        rows: list[dict] = []
        stack = [(root, 0)]
        while stack:
            nid, depth = stack.pop()
            n = nodes[nid]
            detail = entities._text(n.get("detail"))
            rows.append({"anchor": links.anchor("n", nid), "depth": min(depth, MAX_DEPTH), "name": n.get("name") or nid,
                         "description": entities._text(n.get("description")),
                         "detail": detail, "stats": [] if detail else [entities.stat_text(x) for x in n.get("stats") or []],
                         "cost": _costs(n.get("cost")), "opens": _costs(n.get("opens_with")),
                         "search": " ".join([n.get("name") or "", detail]).lower()})
            stack += [(k, depth + 1) for k in reversed(kids.get(nid, []))]
        branches.append({"name": nodes[root].get("name") or root, "rows": rows[1:] if len(rows) > 1 else rows,
                         "lead": entities._text(nodes[root].get("detail") or nodes[root].get("description"))})
    return {"lead": LEADS.get(SYSTEMS[page], ""), "system": s, "branches": branches,
            "count": sum(len(b["rows"]) for b in branches), "planner": page == "star-chart"}


def geode_tools_page() -> dict[str, Any]:
    modules = []
    for m in _data().get("geode_tools") or []:
        totals: dict[str, dict] = {}
        for lv in m.get("levels") or []:
            for c in lv.get("cost") or []:
                t = totals.setdefault(c["item"], {**c, "quantity": 0})
                t["quantity"] += c.get("quantity") or 0
        modules.append({"name": m["name"], "anchor": links.anchor("m", m["slug"]),
                        "levels": [{"level": lv["level"], "name": lv.get("name") or "",
                                    "detail": entities._text(lv.get("detail")), "cost": _costs(lv.get("cost"))}
                                   for lv in m.get("levels") or []],
                        "total": _costs(list(totals.values()))})
    return {"modules": modules}


@gamedata.cached("class_rewards.json")
def _class_rewards() -> dict:
    data = gamedata.load("class_rewards.json", {}) or {}
    return (data.get("classes") or {}) if isinstance(data, dict) else {}


def _grants(rows: list[dict] | None) -> list[dict]:
    return [links.thing(g.get("name") or links.name(g["ref"]) or g["ref"].rsplit("/", 1)[-1].replace("_", " "),
                        g["ref"], g.get("count")) for g in rows or []]


def class_rewards(tech: str) -> dict[str, Any]:
    """What a class unlocks by level, and what each Paragon level pays."""
    c = _class_rewards().get(tech) or {}
    levels = []
    for lv in c.get("levels") or []:
        things = [{"text": a["name"], "url": "#abilities"} for a in lv.get("abilities") or [] if a.get("name")]
        if lv.get("costume"):
            things.append({"text": links.name(lv["costume"]) or lv["costume"].rsplit("/", 1)[-1],
                           "url": links.link(lv["costume"])})
        things += [{"text": links.name(s) or "Style", "url": links.link(s)} for s in lv.get("styles") or []]
        levels.append({"level": lv["level"], "text": entities._text(lv.get("text")), "things": things})
    p = c.get("paragon") or {}
    return {"levels": levels, "paragon_levels": p.get("levels") or 0,
            "paragon_every": _grants(p.get("every")), "paragon_prime": _grants(p.get("prime"))}


def paragon(tech: str) -> list[dict]:
    """A class's Paragon powers, for its class page."""
    p = next((x for x in _data().get("paragon") or [] if x["class"] == tech), None)
    return [{"name": x["name"], "description": entities._text(x.get("description")), "cost": _costs(x.get("cost"))}
            for x in (p or {}).get("powers") or []]

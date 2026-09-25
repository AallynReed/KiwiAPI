"""The wiki's /mastery page over mastery.json, and each collectible's mastery value."""
from __future__ import annotations

from typing import Any

from app.trove.decode import store as gamedata
from app.wiki import entities, links
from app.wiki.ability_view import num

LADDER_LEADS = {
    "trove": "Collecting things and levelling classes earns Trove Mastery.",
    "geode": "Geode Mastery comes from Geode collectibles and companions.",
    "pvp": "PvP Mastery comes from playing Battle Arena and other PvP modes.",
}


@gamedata.cached("mastery.json")
def _data() -> dict:
    data = gamedata.load("mastery.json", {}) or {}
    return data if isinstance(data, dict) else {}


@gamedata.cached("mastery.json")
def _values() -> dict[str, dict[str, int]]:
    return {k.lower(): v for k, v in (_data().get("collectibles") or {}).items()}


def value(ref: str | None) -> dict[str, int]:
    """``{"trove": n, "geode": n}`` for a collectible, empty when it gives none."""
    return _values().get(links.norm(ref), {})


def facts(ref: str | None) -> list[dict]:
    v = value(ref)
    out = []
    if v.get("trove"):
        out.append({"label": "Mastery", "value": num(v["trove"]), "url": "/mastery#trove"})
    if v.get("geode"):
        out.append({"label": "Geode Mastery", "value": num(v["geode"]), "url": "/mastery#geode"})
    return out


def level_ref(ladder: str, level: int) -> str:
    return f"mastery:{ladder}:{level}"


def _bands(levels: list[dict]) -> list[dict]:
    """Runs of levels that cost the same, with the running total at the end of each."""
    out: list[dict] = []
    total = 0
    for lv in levels:
        total += lv["points"]
        if out and out[-1]["points"] == lv["points"] and out[-1]["last"] == lv["level"] - 1:
            out[-1].update(last=lv["level"], total=total)
        else:
            out.append({"first": lv["level"], "last": lv["level"], "points": lv["points"], "total": total})
    return [{**b, "range": str(b["first"]) if b["first"] == b["last"] else f"{b['first']}–{b['last']}",
             "points": num(b["points"]), "total": num(b["total"])} for b in out if b["first"] > 1 or b["points"]]


def _reward(rw: dict) -> dict:
    grants = [links.thing(g.get("name") or links.name(g["ref"]) or g["ref"].rsplit("/", 1)[-1], g["ref"],
                          g.get("count")) for g in rw.get("grants") or []]
    stats = [entities.stat_text(s) for s in rw.get("stats") or []]
    text = entities._text(rw.get("text"))
    # A reward that hands out one linked thing shows it by name; others keep the game's text.
    if len(grants) == 1 and grants[0]["url"]:
        return {"text": text or grants[0]["text"], "url": grants[0]["url"], "extra": []}
    return {"text": text or ", ".join(stats) or ", ".join(g["text"] for g in grants), "url": "",
            "extra": [g for g in grants if g["url"]]}


def page() -> dict[str, Any]:
    data = _data()
    names = data.get("source_names") or {}
    ladders = []
    geode = next((x for x in data.get("ladders") or [] if x["slug"] == "geode"), {})
    geode_points = {s["source"]: s["points"] for s in geode.get("sources") or []}
    for lad in data.get("ladders") or []:
        rewards = {r["id"]: r for r in lad.get("rewards") or []}
        rows = []
        for lv in lad.get("levels") or []:
            got = [_reward(rewards[i]) for i in lv.get("rewards") or [] if i in rewards]
            got = [g for g in got if g["text"]]
            if got:
                rows.append({"level": lv["level"], "anchor": links.anchor(f"l-{lad['slug']}", str(lv["level"])),
                             "rewards": got})
        sources = []
        if lad["slug"] == "trove":
            sources = [{"name": names[str(s["source"])], "points": num(s["points"]),
                        "geode": num(geode_points[s["source"]]) if geode_points.get(s["source"]) else ""}
                       for s in lad.get("sources") or [] if str(s["source"]) in names]
            sources.sort(key=lambda r: r["name"].lower())
        ladders.append({"slug": lad["slug"], "name": lad["name"], "lead": LADDER_LEADS.get(lad["slug"], ""),
                        "max": len(lad.get("levels") or []), "cap": lad.get("cap") or 0,
                        "every": [entities.stat_text(s) for s in lad.get("every_level") or []],
                        "past_cap": [entities.stat_text(s) for s in lad.get("past_cap") or []],
                        "bands": _bands(lad.get("levels") or []), "rewards": rows, "sources": sources})
    return {"ladders": ladders}

"""The wiki's /daily-bonuses and /leaderboards pages, over daily_bonuses.json and
leaderboards.json."""
from __future__ import annotations

from typing import Any
from urllib.parse import quote

from app.core.config import settings
from app.trove.decode import store as gamedata
from app.trove.server_time import trove_now
from app.wiki import links
from app.wiki.ability_view import card, num

RESETS = {"never": "Never", "daily": "Daily", "weekly": "Weekly"}
# Board kinds whose score is not a single player metric, in the game's own terms.
KIND_COUNTS = {
    "metaexperience": "Trove Mastery points",
    "geodemetaexperience": "Geode Mastery points",
    "xpallclasses": "Experience on every class",
    "powerrankclass": "The class's Power Rank",
    "clubpowerrank": "Club Power Rank",
    "metriccollection": "Effort points",
}
OTHER_GROUPS = (("tower", "Shadow Tower", "One board per floor boss and difficulty."),
                ("", "Other boards", "Boards the game defines outside its fixed tabs."))


@gamedata.cached("daily_bonuses.json")
def _daily() -> dict:
    data = gamedata.load("daily_bonuses.json", {}) or {}
    return data if isinstance(data, dict) else {}


@gamedata.cached("leaderboards.json")
def _boards() -> dict:
    data = gamedata.load("leaderboards.json", {}) or {}
    return data if isinstance(data, dict) else {}


def _effect_lines(day: dict) -> list[str]:
    lines = [f"+{num(s['value'])} {s['name']}" for s in day.get("stats") or []]
    claims = [m for m in day.get("multipliers") or [] if m.get("kind") == "claim"]
    for m in day.get("multipliers") or []:
        if m.get("kind") == "pvpexperience":
            lines.append(f"Arena win experience ×{num(m['multiplier'])}")
    for mult in sorted({m.get("multiplier") for m in claims if m.get("multiplier") is not None}):
        n = sum(1 for m in claims if m.get("multiplier") == mult)
        lines.append(f"×{num(mult)} on {n} challenge reward table{'s' if n != 1 else ''}")
    return lines


def daily_bonuses_page() -> dict[str, Any]:
    today = trove_now().weekday()
    days = [{**d, "effects": _effect_lines(d), "today": d.get("day") == today}
            for d in _daily().get("days") or []]
    return {"days": days}


def _icon(path: str) -> str:
    return f"{settings.api_url.rstrip('/')}/site/store/texture?path={quote(path)}" if path else ""


def _counts(board: dict, names: dict[int, str]) -> str:
    kind = (board.get("kind") or "").lower()
    if board.get("metric"):
        return board["metric"].get("label") or ""
    if board.get("combines"):
        parts = ", ".join(names.get(i, str(i)) for i in board["combines"])
        how = {"sum": "Total of", "max": "Highest of", "min": "Lowest of"}.get(board.get("composition", ""), "From")
        return f"{how}: {parts}"
    return KIND_COUNTS.get(kind, "")


def _row(board: dict, names: dict[int, str]) -> dict:
    cls = board.get("class") or ""
    return {"id": board["id"], "name": board.get("name") or str(board["id"]), "icon": _icon(board.get("icon", "")),
            "counts": _counts(board, names), "resets": RESETS.get(board.get("resets", ""), ""),
            "anchor": links.anchor("b", str(board["id"])), "class_url": links.class_url(cls) if cls else ""}


def leaderboards_page() -> dict[str, Any]:
    data = _boards()
    boards = {b["id"]: b for b in data.get("boards") or []}
    names = {i: b.get("name") or str(i) for i, b in boards.items()}
    groups, placed = [], set()
    for c in data.get("categories") or []:
        rows = [_row(boards[i], names) for i in c.get("boards") or [] if i in boards]
        placed.update(r["id"] for r in rows)
        groups.append({"name": c.get("name") or "", "lead": "", "rows": rows})
    for kind, name, lead in OTHER_GROUPS:
        rows = [_row(b, names) for i, b in boards.items()
                if i not in placed and (not kind or (b.get("kind") or "").lower() == kind)]
        placed.update(r["id"] for r in rows)
        if rows:
            groups.append({"name": name, "lead": lead, "rows": rows})
    effort = next((b.get("points") for b in boards.values() if b.get("points")), [])
    same = all(b.get("points") == effort for b in boards.values() if b.get("points"))
    points = [{"label": p.get("label") or p.get("name") or str(p.get("id")), "points": f"{p['points']:g}"}
              for p in sorted(effort, key=lambda p: -p["points"])]
    return {"groups": groups, "effort": points if same else [], "count": len(boards)}


@gamedata.cached("lootbox_odds.json")
def lootbox_odds_page() -> dict[str, Any]:
    data = gamedata.load("lootbox_odds.json", {}) or {}
    boxes = []
    for b in (data.get("boxes") or []) if isinstance(data, dict) else []:
        boxes.append({"name": b["name"], "anchor": links.anchor("x", b["name"]), "notes": b.get("notes") or [],
                      "tiers": [{"name": t.get("name") or "", "chance": _pct(t.get("chance")),
                                 "prizes": [{"name": i["name"], "chance": _pct(i["chance"])} for i in t.get("items") or []]}
                                for t in b.get("tiers") or []],
                      "search": " ".join([b["name"], *(i["name"] for t in b.get("tiers") or [] for i in t.get("items") or [])]).lower()})
    return {"boxes": boxes}


def _pct(v: float | None) -> str:
    return f"{num(v)}%" if v is not None else ""


POWERUP_MODES = (("battleroyale", "Bomber Royale", "Weapon, bomb, mobility and terrain swaps picked up during a match, with their "
                  "upgraded tiers."),
                 ("arena", "Battle Arena consumables", "The power-ups that spawn in the Battle Arena."))


@gamedata.cached("pvp_powerups.json")
def pvp_powerups_page() -> dict[str, Any]:
    data = gamedata.load("pvp_powerups.json", {}) or {}
    rows = (data.get("powerups") or []) if isinstance(data, dict) else []
    modes = []
    for mode, name, lead in POWERUP_MODES:
        groups: dict[str, list[dict]] = {}
        for p in (r for r in rows if r["mode"] == mode):
            c = card(p, name=p["name"], description=p.get("description") or "",
                     kind=f"Tier {p['tier']}" if p["tier"] > 1 else "")
            groups.setdefault(p["group"], []).append(c)
        modes.append({"name": name, "lead": lead, "anchor": links.anchor("m", mode),
                      "groups": [{"name": g, "cards": cards} for g, cards in groups.items()]})
    return {"modes": modes, "count": len(rows)}


@gamedata.cached("chat_commands.json")
def chat_commands_page() -> dict[str, Any]:
    data = gamedata.load("chat_commands.json", {}) or {}
    rows = (data.get("commands") or []) if isinstance(data, dict) else []
    return {"commands": [{**c, "usage": "/" + " ".join([c["command"], *(f"<{a}>" for a in c.get("args") or [])]),
                          "search": f"{c['command']} {c.get('description', '')}".lower()} for c in rows]}

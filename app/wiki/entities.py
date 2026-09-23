"""Ally and mount pages: lookups and render models over the decoded game data.

URLs use the game's own prefab name with dashes (``/ally/delve-dragonfly-jadedrake``)
because display names repeat across variants; the write-up is stored at
``ally/<prefab name>`` / ``mount/<prefab name>``.
"""
from __future__ import annotations

from typing import Any

from app.trove.codexes.localize import resolve_stat_name
from app.trove.decode import store as gamedata
from app.wiki.ability_view import card, stat_text

KINDS: dict[str, dict[str, str]] = {
    "ally": {"file": "ally_abilities.json", "plural": "allies", "title": "Allies"},
    "mount": {"file": "mount_abilities.json", "plural": "mounts", "title": "Mounts"},
}

# `$EquipmentSlot_*` -> what the stat group is labelled on a mount page.
SLOT_NAMES = {"unlock": "When unlocked", "$EquipmentSlot_Mount": "As a mount", "$EquipmentSlot_Wings": "As wings",
              "$EquipmentSlot_Boat": "As a boat", "$EquipmentSlot_Cart": "As a cart"}


def url_slug(prefab_slug: str) -> str:
    return prefab_slug.replace("_", "-")


@gamedata.cached("ally_abilities.json", "mount_abilities.json")
def _index() -> dict[str, dict[str, dict]]:
    # An entry whose name is its prefab name has no display name in the game - not
    # something a player can own - so it gets no page.
    return {kind: {url_slug(e["slug"]): e for e in gamedata.load(spec["file"], []) or []
                   if e.get("name") and e["name"] != e["slug"]}
            for kind, spec in KINDS.items()}


def find(kind: str, slug: str) -> dict | None:
    """An ally/mount by URL slug or by prefab name."""
    table = _index().get(kind, {})
    return table.get(slug) or table.get(url_slug(slug))


def entries(kind: str) -> list[dict]:
    return list(_index().get(kind, {}).values())


def storage_slug(kind: str, entry: dict) -> str:
    return f"{kind}/{entry['slug']}"


def _stat_rows(stats: list[dict]) -> list[str]:
    return [stat_text(s) for s in stats or []]


def summary(kind: str, e: dict) -> dict:
    """One row of the index page."""
    return {"name": e["name"], "url": f"/{kind}/{url_slug(e['slug'])}",
            "abilities": len(e.get("abilities") or []),
            "search": f"{e['name']} {e.get('description', '')}".lower()}


def detail(kind: str, e: dict) -> dict[str, Any]:
    abilities = [card(a, name=a.get("name", ""), description=a.get("text", "")) for a in e.get("abilities") or []]
    if kind == "mount":
        groups = [{"label": SLOT_NAMES.get(g["slot"], resolve_stat_name({}, g["slot"])),
                   "stats": _stat_rows(g["stats"])} for g in e.get("stats") or []]
    else:
        groups = [{"label": "Stats granted", "stats": _stat_rows(e.get("stats") or [])}] if e.get("stats") else []
    # Locale text carries line breaks as a literal backslash-n.
    description = (e.get("description") or "").replace(chr(92) + "n", "\n")
    return {"name": e["name"], "description": description, "prefab": e.get("prefab", ""),
            "stat_groups": groups, "abilities": abilities}

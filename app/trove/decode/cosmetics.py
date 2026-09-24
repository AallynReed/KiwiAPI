"""cosmetics.json - costumes and styles (hats, faces, weapons, banners).

Costumes are `prefabs/skins/<id>.binfab`, one bare object each: field 0 the class
(KClass ordinal), 2 the model (rig + parts), 3 which ability visuals it swaps, 4 the
UI blueprint, 5/6 the `$prefabs_skins_<id>_skinset_name`/`_description` keys. The
ones a player can own are the members of `collections/collection_skin` (bare ids,
grouped by class). Bomber Royale bomb skins (`skins/secondary`) are not costumes.

Styles have no prefab: the style catalogue is the gear loot tables
`prefabs/loot/<slot>.binfab`. Field 0 lists groups {0 group id, 1 rows}, and a row
is {0 blueprint, 1 name key, 2 description key, ...}; the group ids are the ones
`collections/collection_equipmentappearance` names ("Prestige" -> "Paragon"), which
is where the collection screen files each style. A banner row may name an
`equipment/...` prefab instead of a blueprint; its model is that prefab's. The ring
tables are empty and `dailybonus` holds no styles. Groups the game only uses while
building content (InProgress, ReadyForGame, Hidden) are left unnamed.
"""
from __future__ import annotations

import re
from typing import Any

from app.trove.decode.ability import Prefabs
from app.trove.decode.common import locale
from app.trove.decode.recipes import CLASSES
from app.trove.decode.tree import GameTree
from app.trove.decode.wire import Obj

TITLE = "Costumes and styles"
OUTPUT = "cosmetics.json"
PREFIXES = ("prefabs/skins/", "prefabs/loot/", "prefabs/collections/", "prefabs/equipment/", "languages/en/")
INDENT, FINAL_NEWLINE = 4, False

SKIN_CLASS, SKIN_BLUEPRINT, SKIN_NAME, SKIN_DESCRIPTION = 0, 4, 5, 6
ROW_BLUEPRINT, ROW_NAME, ROW_DESCRIPTION = 0, 1, 2
BLUEPRINT = 37
# Loot table -> the slot it styles, in the order the wiki lists them.
SLOTS = {"hat": "Hats", "face": "Faces", "weapon_melee": "Melee weapons", "weapon_pistol": "Guns",
         "weapon_staff": "Staffs", "weapon_bow": "Bows", "weapon_spear": "Spears", "weapon_fist": "Fists",
         "pvpbanner": "Banners"}
WORKING_GROUP = re.compile(r"^(InProgress|ReadyForGame|Hidden)")


def _leaf(v: Any) -> dict:
    return v.leaf if isinstance(v, Obj) else {}


def _names(tree: GameTree) -> dict[str, str]:
    out: dict[str, str] = {}
    for path in tree.files("languages/en/", ".binfab"):
        for key, text in locale(tree.read(path)).items():
            out.setdefault(key, text)
    return out


def _group_names(prefabs: Prefabs, collection: str, names: dict[str, str]) -> dict[str, str]:
    """Collection group id -> its display name ("" for the working groups)."""
    pf = prefabs.get(f"collections/collection_{collection}")
    out: dict[str, str] = {}
    for group in _leaf(pf.root if pf else None).get(0) or []:
        g = _leaf(group)
        gid = g.get(0) if isinstance(g.get(0), str) else ""
        if gid and gid not in out:
            label = names.get(g.get(1) or "", "")
            out[gid] = "" if WORKING_GROUP.match(gid) else label or re.sub(r"(?<=[a-z])(?=[A-Z])", " ", gid)
    return out


def _costumes(prefabs: Prefabs, names: dict[str, str]) -> list[dict]:
    pf = prefabs.get("collections/collection_skin")
    out = []
    seen: set[str] = set()
    for group in _leaf(pf.root if pf else None).get(0) or []:
        g = _leaf(group)
        gid = g.get(0) if isinstance(g.get(0), str) else ""
        label = "" if WORKING_GROUP.match(gid) else names.get(g.get(1) or "", "") or gid
        for row in g.get(3) or []:
            sid = _leaf(row).get(0)
            if not isinstance(sid, str) or sid in seen:
                continue
            seen.add(sid)
            skin = prefabs.get(f"skins/{sid}")
            leaf = _leaf(skin.root if skin else None)
            name = names.get(leaf.get(SKIN_NAME) or "", "")
            if not name:
                continue
            entry: dict[str, Any] = {"slug": sid, "name": name,
                                     "description": names.get(leaf.get(SKIN_DESCRIPTION) or "", ""),
                                     "prefab": f"skins/{sid}"}
            cls = leaf.get(SKIN_CLASS)
            if isinstance(cls, int) and 0 <= cls < len(CLASSES):
                entry["class"] = CLASSES[cls]
            if isinstance(leaf.get(SKIN_BLUEPRINT), str) and leaf[SKIN_BLUEPRINT]:
                entry["blueprint"] = leaf[SKIN_BLUEPRINT]
            if label:
                entry["group"] = label
            out.append(entry)
    return sorted(out, key=lambda e: e["name"].lower())


def _model(prefabs: Prefabs, ref: str) -> str:
    """A style row's blueprint: the row's own, or the named equipment prefab's."""
    if ref.lower().endswith(".blueprint"):
        return ref
    if ref.startswith("equipment/"):
        bp = _leaf(prefabs.get(ref).component(BLUEPRINT) if prefabs.get(ref) else None).get(0)
        return bp if isinstance(bp, str) else ""
    return ""


def _styles(prefabs: Prefabs, names: dict[str, str], groups: dict[str, str]) -> list[dict]:
    slots = []
    for table, title in SLOTS.items():
        pf = prefabs.get(f"loot/{table}")
        rows = []
        for group in _leaf(pf.root if pf else None).get(0) or []:
            g = _leaf(group)
            label = groups.get(g.get(0), "") if isinstance(g.get(0), str) else ""
            for row in g.get(1) or []:
                r = _leaf(row)
                name = names.get(r.get(ROW_NAME) or "", "")
                if not name:
                    continue
                style: dict[str, Any] = {"name": name}
                desc = names.get(r.get(ROW_DESCRIPTION) or "", "")
                if desc:
                    style["description"] = desc
                model = _model(prefabs, r.get(ROW_BLUEPRINT) or "") if isinstance(r.get(ROW_BLUEPRINT), str) else ""
                if model:
                    style["blueprint"] = model
                if label:
                    style["group"] = label
                rows.append(style)
        if rows:
            slots.append({"slug": title.lower().replace(" ", "-"), "name": title, "prefab": f"loot/{table}",
                          "styles": rows})
    return slots


def build(tree: GameTree) -> dict[str, list[dict]]:
    prefabs = Prefabs(tree)
    names = _names(tree)
    return {"costumes": _costumes(prefabs, names),
            "style_slots": _styles(prefabs, names, _group_names(prefabs, "equipmentappearance", names))}


def count(data: dict) -> int:
    return len(data.get("costumes") or []) + sum(len(s["styles"]) for s in data.get("style_slots") or [])

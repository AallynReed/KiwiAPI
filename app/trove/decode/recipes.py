"""recipes.json - every recipe, and the crafting stations that offer them, grouped
the way each station's crafting window groups them.

A recipe is a bare object in ``prefabs/recipes/<id>.binfab``. Field indices come from
the class's property enumerator in Trove_x64.exe, meanings from its editor text and
the code that reads each member:

    0   ingredients ("List of ingredients needed for this recipe.")
    1   results ("List of results gained from crafting this recipe.")
    2   "Time required for crafting." (0-10; the unit is not stated)
    10  requirement (15 enumerates the very same member, so the two always match)
    13  limit reset period (NoReset, DailyReset ... SixHoursReset)
    14  crafts allowed per period ("You can craft {0} more of this item {1}."; the
        client shows no limit for NoReset, so neither does this)
    20  season (None, Winter, Spring, Summer, Fall)

The other fields (flags, two strings, completion effects) have no proven meaning yet
and are left out. Ingredients and results are RecipeItems: 0 prefab, 1 amount,
2 claim, 3 collectable ``{0 KCollectionType, 1 ref}``, 4 kind (Prefab, Claim,
Collectable, Title), 6 per-craft quantity factor, 7 its operation (None, Sum,
Substract, Multiply).

A requirement is ``{0 type name, 1 params}``: params section 0 field 0 is an optional
failure-message key, the last section the type's own fields. Labelled here, each from
its evaluator or text builder: the level family (classlevel, powerrank, metalevel,
zonelevel, highestpowerlevel) is ``0 min, 1 max, 2 mode`` (at least min / at most max /
between); profession is ``0 KProfession, 1 min, 2 max, 3 mode``; hastitle and
hascollection are ``0 list, 1 any/all/none``; activeclass lists KClass values; tags
and zonetag list tags; recipe is ``0 player, 1 workbench, 2 nested requirement``;
setall/setany/setnone list child requirements. Other types keep only their name.

A station is a ``placeable/crafting`` prefab with the crafting component (61). Its
field 14 lists the tabs ``{0 name key, 1 recipe ids}``; component 76 section 1 holds
the window title (field 0) and a KProfession (field 2). A profession station has no
tabs of its own: the client shows the tiers of ``prefabs/professions/<profession>``,
``{0 title key, 1 skill it unlocks at, 2 recipe ids}``. The placeable item that
puts the station down names it in its component 50 and carries its description.

Items learn recipes through component 223: field 8 lists collectables (type 1 is a
recipe id), and fields 6/7 name collection_recipe categories and collection types
for the "unlock a random recipe" scrolls. The game loads only the recipes that
``collections/collection_recipe`` lists; its categories are reported per recipe.
"""
from __future__ import annotations

from typing import Any

from app.trove.decode import fields as F
from app.trove.decode.common import locale, stem
from app.trove.decode.tree import GameTree
from app.trove.decode.wire import Obj, WireError, _Reader, parse, read_object, strings

TITLE = "Recipes"
OUTPUT = "recipes.json"
PREFIXES = ("prefabs/", "languages/en/")
INDENT, FINAL_NEWLINE = None, True

STATION_ROOT = "placeable/crafting/"
CRAFTING, STATION, PLACER, BLUEPRINT, UNLOCKER = 61, 76, 50, 37, 223
CRAFT_TABS = 14
TAB_NAME, TAB_RECIPES = 0, 1
STATION_TITLE, STATION_PROFESSION = 0, 2
TIERS, TIER_TITLE, TIER_SKILL, TIER_RECIPES = 0, 0, 1, 2
UNLOCK_COLLECTIONS, UNLOCK_TYPES, UNLOCK_COLLECTABLES = 6, 7, 8

INGREDIENTS, RESULTS, CRAFT_TIME, REQUIREMENT, RESET, LIMIT, SEASON = 0, 1, 2, 10, 13, 14, 20
ITEM_PATH, ITEM_AMOUNT, ITEM_CLAIM, ITEM_COLLECTABLE, ITEM_KIND, ITEM_FACTOR, ITEM_OP = 0, 1, 2, 3, 4, 6, 7
COLLECTABLE_TYPE, COLLECTABLE_REF = 0, 1

# Enum tables read out of Trove_x64.exe (name, value order).
ITEM_KINDS = ("Prefab", "Claim", "Collectable", "Title")
QUANTITY_OPS = ("None", "Sum", "Substract", "Multiply")
RESETS = ("NoReset", "DailyReset", "WeeklyReset", "MonthlyReset", "SeasonReset", "NeverReset",
          "HourlyReset", "ThreeHoursReset", "SixHoursReset")
SEASONS = ("None", "Winter", "Spring", "Summer", "Fall")
PROFESSIONS = ("Gardening", "Ringcrafting", "Runecrafting", "Crystallogy", "Mysticism",
               "MartialArts", "Gearcrafting")
COLLECTION_TYPES = ("EquipmentAppearance", "Recipe", "Mount", "Pet", "Skin", "Cart", "Flask",
                    "Wings", "Tome", "Boat", "Sail", "FishingPole", "Fish", "FlaskEffect", "Badge",
                    "Aura", "GeodeCompanion", "SecondarySkin", "Memento")
CLASSES = ("Knight", "GunSlinger", "FaeTrickster", "Dracolyte", "NeonNinja", "CandyBarbarian",
           "IceMage", "ShadowHunter", "PirateLord", "Adventurer", "TombRaiser", "LunarLancer",
           "SpiritTank", "Chloromancer", "DinoTamer", "CrimeFighter", "Bard", "Solarion")
RECIPE_TYPE = COLLECTION_TYPES.index("Recipe")
MATCH = ("any", "all", "none")

LEVEL_KINDS = frozenset({"classlevel", "powerrank", "metalevel", "zonelevel", "highestpowerlevel"})
SETS = frozenset({"setall", "setany", "setnone"})
# Single-bound text the client builds for these (a range's argument order is unproven).
LEVEL_TEXT = {("metalevel", 0): "$MoreMasteryRequirement", ("metalevel", 1): "$LessMasteryRequirement",
              ("classlevel", 0): "$HigherLevelRequirement", ("classlevel", 1): "$LowerLevelRequirement"}
LITERAL_TEXT = {("highestpowerlevel", 0): "Highest class power rank {0} or more",
                ("highestpowerlevel", 1): "Highest class power rank {0} or less"}


def _enum(table: tuple[str, ...], value: Any) -> str | None:
    return table[value] if isinstance(value, int) and 0 <= value < len(table) else None


def _rel(path: Any) -> str:
    """A prefab reference as a logical path (``item/x``), or '' when it is not one."""
    if not isinstance(path, str) or not path or path.endswith(".blueprint"):
        return ""
    return path.strip().removeprefix("prefabs/").removesuffix(".binfab")


def _components(data: bytes, wanted: set[int]) -> dict[int, Obj]:
    """The wanted components of an entity prefab, parsing only their own bytes."""
    r = _Reader(data, len(data))
    spans: dict[int, tuple[int, int]] = {}
    try:
        _, p = r.zz(0)
        _, p = r.zz(p, 70)
        size, p = r.uvar(p)
        if p + size != len(data):
            return {}
        while p < len(data):
            cid, p = r.zz(p)
            n, p = r.uvar(p)
            if cid in wanted:
                spans.setdefault(cid, (p, p + n))
            p += n
        return {cid: read_object(data, a, b) for cid, (a, b) in spans.items()}
    except WireError:
        return {}


class _Names:
    """Identity name/description keys of referenced prefabs, each file read once."""

    def __init__(self, tree: GameTree):
        self.tree = tree
        self.keys: dict[str, tuple[str, str]] = {}

    def get(self, rel: str) -> tuple[str, str]:
        if rel not in self.keys:
            data = self.tree.read(f"prefabs/{rel}.binfab")
            leaf = _components(data, {F.IDENTITY}).get(F.IDENTITY, Obj()).leaf if data else {}
            name, desc = leaf.get(F.ID_NAME), leaf.get(F.ID_DESCRIPTION)
            self.keys[rel] = (name if isinstance(name, str) else "", desc if isinstance(desc, str) else "")
        return self.keys[rel]


def _collectable(obj: Any) -> dict:
    leaf = obj.leaf if isinstance(obj, Obj) else {}
    ref = leaf.get(COLLECTABLE_REF)
    if not isinstance(ref, str) or not ref:
        return {}
    out = {"ref": ref}
    kind = _enum(COLLECTION_TYPES, leaf.get(COLLECTABLE_TYPE))
    if kind:
        out["type"] = kind
    return out


def _item(obj: Any, names: _Names, keys: set[str], result: bool) -> dict | None:
    leaf = obj.leaf if isinstance(obj, Obj) else {}
    if not leaf:
        return None
    out: dict[str, Any] = {}
    if result:
        out["kind"] = _enum(ITEM_KINDS, leaf.get(ITEM_KIND, 0)) or leaf.get(ITEM_KIND)
    path = _rel(leaf.get(ITEM_PATH))
    if path:
        out["path"] = path
    claim = leaf.get(ITEM_CLAIM)
    if isinstance(claim, str) and claim:
        out["claim"] = claim
    coll = _collectable(leaf.get(ITEM_COLLECTABLE))
    if coll:
        out["collectable"] = coll
    ref = coll.get("ref", "")
    named = path or (_rel(ref) if "/" in ref else "")
    if named:
        key = names.get(named)[0]
        if key:
            out["name"] = key
            keys.add(key)
    amount = leaf.get(ITEM_AMOUNT)
    if isinstance(amount, int):
        out["amount"] = amount
    op = leaf.get(ITEM_OP, 0)
    if op:
        out["per_craft"] = {"op": _enum(QUANTITY_OPS, op) or op, "factor": round(float(leaf.get(ITEM_FACTOR, 0)), 4)}
    return out if out.keys() - {"kind", "amount"} else None


def _bounds(own: dict, lo: int, hi: int, mode_i: int) -> dict:
    mode = own.get(mode_i, 0)
    out: dict[str, Any] = {}
    if mode in (0, 2):
        out["min"] = own.get(lo, 0)
    if mode in (1, 2):
        out["max"] = own.get(hi, 0)
    return out


def _requirement(node: Any, keys: set[str]) -> dict | None:
    leaf = node.leaf if isinstance(node, Obj) else {}
    kind, params = leaf.get(0), leaf.get(1)
    if not isinstance(kind, str) or kind.lower() in ("", "null", "none"):
        return None
    k = kind.lower()
    out: dict[str, Any] = {"kind": k}
    own: dict = {}
    if isinstance(params, Obj) and params:
        msg = params[0].get(0)
        if isinstance(msg, str) and msg.startswith("$"):
            out["message"] = msg
            keys.add(msg)
        own = params[-1] if len(params) > 1 else {}
    if k in LEVEL_KINDS:
        out.update(_bounds(own, 0, 1, 2))
        mode = own.get(2, 0)
        fmt = LEVEL_TEXT.get((k, mode))
        if fmt:
            out["text"] = (fmt, own.get(0 if mode == 0 else 1, 0))
            keys.add(fmt)
        elif (k, mode) in LITERAL_TEXT:
            out["text"] = LITERAL_TEXT[(k, mode)].format(own.get(0 if mode == 0 else 1, 0))
    elif k == "profession":
        prof = _enum(PROFESSIONS, own.get(0))
        if prof:
            out["profession"] = prof
        out.update({b: round(float(v), 4) for b, v in _bounds(own, 1, 2, 3).items()})
    elif k in ("hastitle", "hastitleequipped"):
        out["titles"] = [t for t in own.get(0) or [] if isinstance(t, str)]
        if k == "hastitle":
            out["match"] = _enum(MATCH, own.get(1, 0))
    elif k == "hascollection":
        out["collectables"] = [c for c in map(_collectable, own.get(0) or []) if c]
        out["match"] = _enum(MATCH, own.get(1, 0))
    elif k == "activeclass":
        out["classes"] = [_enum(CLASSES, c) or c for c in own.get(0) or [] if isinstance(c, int)]
    elif k in ("tags", "zonetag"):
        out["tags"] = [t for t in own.get(0) or [] if isinstance(t, str)]
    elif k in SETS:
        out["of"] = [r for r in (_requirement(c, keys) for c in own.get(0) or []) if r]
    elif k == "recipe":
        out["applies_to"] = [who for who, i in (("player", 0), ("workbench", 1)) if own.get(i)]
        child = _requirement(own.get(2), keys)
        if child:
            out["condition"] = child
    return out


def _recipe(root: Obj, names: _Names, keys: set[str]) -> dict:
    leaf = root.leaf
    out: dict[str, Any] = {
        "results": [r for r in (_item(o, names, keys, True) for o in leaf.get(RESULTS) or []) if r],
        "ingredients": [r for r in (_item(o, names, keys, False) for o in leaf.get(INGREDIENTS) or []) if r],
    }
    t = leaf.get(CRAFT_TIME)
    if isinstance(t, float) and t > 0:
        out["craft_time"] = round(t, 4)
    req = _requirement(leaf.get(REQUIREMENT), keys)
    if req:
        out["requirement"] = req
    limit, reset = leaf.get(LIMIT, 0), leaf.get(RESET, 0)
    if isinstance(limit, int) and limit > 0 and reset:
        out["limit"] = {"count": limit, "reset": _enum(RESETS, reset) or reset}
    season = leaf.get(SEASON, 0)
    if season:
        out["season"] = _enum(SEASONS, season) or season
    return out


def _tabs(crafting: Obj) -> list[dict]:
    out = []
    for tab in crafting.get(CRAFT_TABS) or []:
        leaf = tab.leaf if isinstance(tab, Obj) else {}
        ids = [r for r in leaf.get(TAB_RECIPES) or [] if isinstance(r, str) and r]
        name = leaf.get(TAB_NAME)
        out.append({"name": name if isinstance(name, str) else "", "recipes": ids})
    return out


def _tiers(tree: GameTree, profession: str) -> list[dict]:
    try:
        pf = parse(tree.read(f"prefabs/professions/{profession.lower()}.binfab") or b"")
    except WireError:
        return []
    out = []
    for tier in (pf.root.leaf.get(TIERS) if pf.root is not None else None) or []:
        leaf = tier.leaf if isinstance(tier, Obj) else {}
        ids = [r for r in leaf.get(TIER_RECIPES) or [] if isinstance(r, str) and r]
        out.append({"name": leaf.get(TIER_TITLE) or "", "unlock_skill": leaf.get(TIER_SKILL, 0), "recipes": ids})
    return out


def _texts(tree: GameTree, keys: set[str]) -> dict[str, str]:
    out: dict[str, str] = {}
    for path in tree.files("languages/en/", ".binfab"):
        for k, v in locale(tree.read(path)).items():
            if k in keys:
                out.setdefault(k, v)
    return out


def _label(key: Any, text: dict[str, str]) -> str:
    """A tab/title label: a locale key, an ``@literal``, or plain text."""
    if not isinstance(key, str) or not key:
        return ""
    if key.startswith("$"):
        return text.get(key, "")
    return key.removeprefix("@")


def build(tree: GameTree) -> dict:
    names = _Names(tree)
    keys: set[str] = set()
    known = {stem(p).lower(): stem(p) for p in tree.files("prefabs/recipes/", ".binfab")}

    recipes: dict[str, dict] = {}
    for rid in known.values():
        try:
            pf = parse(tree.read(f"prefabs/recipes/{rid}.binfab") or b"")
        except WireError:
            continue
        if pf.root is not None:
            recipes[rid] = _recipe(pf.root, names, keys)

    def resolve(ids: list[str]) -> list[str]:
        return list(dict.fromkeys(known[i.lower()] for i in ids if i.lower() in known and known[i.lower()] in recipes))

    # One pass over every prefab: crafting components, station placers, recipe teachers.
    placer: dict[str, str] = {}
    teaches: dict[str, list[str]] = {}
    collection_unlockers: dict[str, list[str]] = {}
    stations_raw: list[tuple[str, dict[int, Obj]]] = []
    others: list[dict] = []
    for path in tree.files("prefabs/", ".binfab"):
        rel = _rel(path)
        wanted = {CRAFTING, STATION, UNLOCKER} | ({PLACER, BLUEPRINT} if rel.startswith(STATION_ROOT) else set())
        comps = _components(tree.read(path) or b"", wanted)
        if PLACER in comps and rel.startswith(STATION_ROOT):
            for ref in strings(comps[PLACER]):
                if _rel(ref).startswith(STATION_ROOT):
                    placer.setdefault(_rel(ref), rel)
        if UNLOCKER in comps:
            u = comps[UNLOCKER].leaf
            for c in map(_collectable, u.get(UNLOCK_COLLECTABLES) or []):
                if c.get("type") == "Recipe":
                    teaches.setdefault(c["ref"].lower(), []).append(rel)
            if RECIPE_TYPE in (u.get(UNLOCK_TYPES) or []):
                for cat in u.get(UNLOCK_COLLECTIONS) or []:
                    if isinstance(cat, str) and cat:
                        collection_unlockers.setdefault(cat, []).append(rel)
        if CRAFTING in comps:
            if rel.startswith(STATION_ROOT):
                stations_raw.append((rel, comps))
            else:
                ids = resolve([r for t in _tabs(comps[CRAFTING]) for r in t["recipes"]])
                if ids:
                    title = comps[STATION].leaf.get(STATION_TITLE) if STATION in comps else None
                    others.append({"prefab": rel, "name": title or "", "recipes": ids})
                    keys.add(title or "")

    for other, orec in recipes.items():
        for r in orec["results"]:
            c = r.get("collectable", {})
            if c.get("type") == "Recipe":
                teaches.setdefault(c["ref"].lower(), []).append(f"recipes/{other}")
    for rid, rec in recipes.items():
        by = teaches.get(rid.lower())
        if by:
            rec["unlocked_by"] = list(dict.fromkeys(by))

    # The recipe collection: what the game loads, by category.
    collections: dict[str, dict] = {}
    try:
        coll = parse(tree.read("prefabs/collections/collection_recipe.binfab") or b"")
        cats = (coll.root.leaf.get(0) if coll.root is not None else None) or []
    except WireError:
        cats = []
    for cat in cats:
        leaf = cat.leaf if isinstance(cat, Obj) else {}
        cname = leaf.get(0)
        if not isinstance(cname, str):
            continue
        entry: dict[str, Any] = {"name": leaf.get(1) or cname}
        keys.add(entry["name"])
        if collection_unlockers.get(cname):
            entry["unlockers"] = collection_unlockers[cname]
        collections[cname] = entry
        for e in leaf.get(3) or []:
            rid = e.leaf.get(0) if isinstance(e, Obj) else None
            if isinstance(rid, str) and known.get(rid.lower()) in recipes:
                recipes[known[rid.lower()]].setdefault("collection", cname)

    stations = []
    for rel, comps in stations_raw:
        st = comps[STATION].leaf if STATION in comps else {}
        prof = _enum(PROFESSIONS, st.get(STATION_PROFESSION, -1))
        groups = _tiers(tree, prof) if prof else _tabs(comps[CRAFTING])
        for g in groups:
            g["recipes"] = resolve(g["recipes"])
            keys.add(g["name"])
        groups = [g for g in groups if g["recipes"]]
        item = placer.get(rel, "")
        name_key, desc_key = names.get(item) if item else ("", "")
        title = st.get(STATION_TITLE) or name_key
        row: dict[str, Any] = {"slug": rel.removeprefix(STATION_ROOT).replace("/", "_"), "name": title,
                               "description": desc_key, "prefab": rel}
        keys.update((title, desc_key))
        if item:
            row["item"] = item
        bp = comps[BLUEPRINT].leaf.get(0) if BLUEPRINT in comps else None
        if isinstance(bp, str) and bp:
            row["blueprint"] = bp
        if prof:
            row["profession"] = prof
            keys.add(f"$Profession_DisplayName_{prof}")
        row["groups"] = groups
        stations.append(row)

    keys.discard("")
    text = _texts(tree, keys)
    return _finish(recipes, stations, others, collections, text)


def _finish(recipes: dict, stations: list[dict], others: list[dict], collections: dict,
            text: dict[str, str]) -> dict:
    def named(rows: list[dict]) -> None:
        for r in rows:
            if "name" in r:
                label = text.get(r["name"], "")
                if label:
                    r["name"] = label
                else:
                    del r["name"]

    def req_text(req: dict) -> None:
        if "message" in req:
            msg = text.get(req["message"], "")
            if msg:
                req["message"] = msg
            else:
                del req["message"]
        if isinstance(req.get("text"), tuple):
            fmt, value = req["text"]
            if text.get(fmt):
                req["text"] = text[fmt].replace("{0}", str(value))
            else:
                del req["text"]
        for child in req.get("of", []):
            req_text(child)
        if "condition" in req:
            req_text(req["condition"])

    for rec in recipes.values():
        named(rec["results"])
        named(rec["ingredients"])
        if "requirement" in rec:
            req_text(rec["requirement"])
    for c in collections.values():
        c["name"] = _label(c["name"], text) or c["name"].removeprefix("$")

    offered: dict[str, list[str]] = {}
    kept = []
    for st in stations:
        st["name"] = _label(st["name"], text)
        st["description"] = _label(st["description"], text)
        if "profession" in st:
            st["profession_name"] = text.get(f"$Profession_DisplayName_{st['profession']}", st["profession"])
        for g in st["groups"]:
            g["name"] = _label(g["name"], text)
        if not st["name"] or not st["groups"]:
            continue
        if not st["description"]:
            del st["description"]
        kept.append(st)
        for g in st["groups"]:
            for rid in g["recipes"]:
                offered.setdefault(rid, [])
                if st["slug"] not in offered[rid]:
                    offered[rid].append(st["slug"])

    shape: dict[str, list[str]] = {}
    for st in kept:
        sig = repr([(g["name"], g["recipes"]) for g in st["groups"]])
        shape.setdefault(sig, []).append(st["slug"])
    for st in kept:
        twins = [s for s in shape[repr([(g["name"], g["recipes"]) for g in st["groups"]])] if s != st["slug"]]
        if twins:
            st["same_as"] = twins

    elsewhere: set[str] = set()
    for o in others:
        o["name"] = _label(o["name"], text)
        if not o["name"]:
            del o["name"]
        elsewhere.update(o["recipes"])
    for rid, rec in recipes.items():
        if rid in offered:
            rec["stations"] = offered[rid]

    return {
        "stations": sorted(kept, key=lambda s: (s["name"].lower(), s["slug"])),
        "other_offerers": others,
        "collections": collections,
        "recipes": dict(sorted(recipes.items())),
        "not_offered": sorted(r for r in recipes if r not in offered and r not in elsewhere),
    }


def count(data: dict) -> int:
    return len(data.get("recipes", {}))

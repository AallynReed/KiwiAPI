"""items.json - every named item (`prefabs/item/**`) and piece of gear
(`prefabs/equipment/**`: banners, Delve and Geode gear): what it is and does.

Identity (49): 1 name key, 5 description key, 9 rarity (the exe's KItemRarity:
0-6 Common..Resplendent, 7-11 Shadow 1-5, Radiant/Stellar alternating from 12,
22-26 Crystal 1-5, 27-31 Mystic 1-5; 17 and 20 override it when not -1), 10 item
type (KItemType, used here only to pick a deconstruct table), 14 tradable (0 shows
`$Item_Untradable`). The picture is the blueprint in 37 field 0.

What using or owning an item does, each from its component:
- 223 unlocker, second section: 8 lists `{0 collection type, 1 ref}` it grants (a
  costume is a bare `skins/` id, a bomb skin a `skins/secondary/` id, a style a
  blueprint); 5 = 1 with 6 group ids and 7 collection types grants one random member
  of those groups of `collections/collection_<type>`. Recipes it teaches are left to
  recipes.json (`unlocked_by`).
- 65 food: field 0 = 50, field 1 its stat modifier records; 472 decay says how long
  each piece lasts in the food slot.
- 398 (0), 135 (2) and 57 (0) name what using it runs; `ability.describe` reads that
  chain. An `effects/emptyeffect` means the server applies it, so nothing is shown.
- 1033 title ids it grants, 411 (1) adventures it starts, 368 (0) experience.
- 241 lockbox, second section: 0 the ways to open it `{3 cost rows, 4 label}`. The
  loot tables it names are the server's, so a box's contents are not in the client.
- 472 decay: `mementos._decay`.

Loot Collecting and composting read `crafting/deconstruct`: field 2 by item, else
field 0 by item type, else field 1 by collection type (from 223), per station
(2 Loot Collector, 8 Compost); a table row adds its outputs when the item carries
all its tags (106 field 0, suffixed `_ver<n>` when 311 is present, plus the rarity
name).

`$GuideUI_generic_<path>` is the game's own hint text for an item.

Seeds grow by `blocks/sprouts` (root 1 rows `[{0 seed item}, {1 seconds, 2 outcomes
{3 plant or placeable, 4 weight}}]`); a `plant/<x>` outcome is a block template in
`blocks/plants` or `blocks/gardening` whose second section's field 0 is its
blueprint. Plants have no names, so they are shown by their picture.
"""
from __future__ import annotations

from typing import Any

from app.trove.decode.ability import Prefabs, describe, identity, walk_values
from app.trove.decode.common import collection_members, locale, stat_rows
from app.trove.decode.mementos import _decay
from app.trove.decode.recipes import COLLECTION_TYPES
from app.trove.decode.tree import GameTree
from app.trove.decode.wire import Obj, Prefab, WireError, parse

TITLE = "Items"
OUTPUT = "items.json"
PREFIXES = ("prefabs/", "languages/en/")
INDENT, FINAL_NEWLINE = None, True

ROOTS = ("prefabs/item/", "prefabs/equipment/")
BLUEPRINT, TAGS, TAG_VERSION, UNLOCKER, FOOD, DECAY = 37, 106, 311, 223, 65, 472
LOCKBOX, TITLES, ADVENTURES, XP = 241, 1033, 411, 368
USE = {398: 0, 135: 2, 57: 0}
ID_RARITY, ID_TYPE, ID_TRADABLE, ID_RARITY_OVERRIDES = 9, 10, 14, (17, 20)
FOOD_KIND = 50
RARITY = (("Common", "Uncommon", "Rare", "Epic", "Legendary", "Relic", "Resplendent")
          + tuple(f"Shadow {i}" for i in range(1, 6))
          + tuple(x for i in range(1, 6) for x in (f"Radiant {i}", f"Stellar {i}"))
          + tuple(f"Crystal {i}" for i in range(1, 6)) + tuple(f"Mystic {i}" for i in range(1, 6)))
STATIONS = {2: "Loot Collector", 8: "Compost"}
SKIN, SECONDARY_SKIN, STYLE, RECIPE = (COLLECTION_TYPES.index(x) for x in
                                       ("Skin", "SecondarySkin", "EquipmentAppearance", "Recipe"))
EMPTY_EFFECT = "effects/emptyeffect"


def _leaf(v: Any) -> dict:
    return v.leaf if isinstance(v, Obj) else {}


def _rows(v: Any) -> list[dict]:
    return [r.leaf for r in v or () if isinstance(r, Obj)]


def _own(pf: Prefab, cid: int) -> dict:
    """A component's last (own-class) section."""
    comp = pf.component(cid)
    return comp.leaf if isinstance(comp, Obj) and comp else {}


def _rarity_name(ident: dict) -> str:
    r = ident.get(ID_RARITY)
    for i in ID_RARITY_OVERRIDES:
        if isinstance(ident.get(i), int) and ident[i] != -1:
            r = ident[i]
    return RARITY[r] if isinstance(r, int) and 0 <= r < len(RARITY) else ""


def _unlock_ref(kind: int, ref: str) -> str:
    ref = ref.removesuffix(".binfab").removeprefix("prefabs/")
    if kind == SKIN and "/" not in ref:
        return f"skins/{ref}"
    if kind == SECONDARY_SKIN and "/" not in ref:
        return f"skins/secondary/{ref}"
    if kind == STYLE and not ref.startswith("equipment/"):
        return "bp:" + ref.rsplit("/", 1)[-1].lower().removesuffix(".blueprint")
    return ref


class _Deconstruct:
    def __init__(self, prefabs: Prefabs, names: dict[str, str]):
        self.prefabs, self.names = prefabs, names
        master = prefabs.get("crafting/deconstruct")
        root = master.root.leaf if master and master.root else {}
        self.by = [{(r.get(0), r.get(2)): r.get(1) for r in _rows(root.get(i))} for i in (2, 0, 1)]
        self._tables: dict[str, list[tuple[set, list]]] = {}

    def _table(self, ref: Any) -> list[tuple[set, list]]:
        if not isinstance(ref, str):
            return []
        ref = ref.removeprefix("prefabs/").removesuffix(".binfab")
        if ref not in self._tables:
            pf = self.prefabs.get(ref)
            rows = []
            for r in _rows(pf.root.leaf.get(0) if pf and pf.root else None):
                need = {t for t in r.get(0) or () if isinstance(t, str)}
                outs = [(o[0], o.get(1) or 1) for o in _rows(r.get(1)) if isinstance(o.get(0), str)]
                rows.append((need, outs))
            self._tables[ref] = rows
        return self._tables[ref]

    def __call__(self, rel: str, item_type: Any, collections: set[int], keys: set[str]) -> list[dict]:
        out = []
        for station, label in STATIONS.items():
            ref = (self.by[0].get((rel, station)) or self.by[1].get((item_type, station))
                   or next((self.by[2][(c, station)] for c in sorted(collections) if (c, station) in self.by[2]), None))
            totals: dict[str, int] = {}
            for need, outs in self._table(ref):
                if need <= keys:
                    for item, n in outs:
                        item = item.removeprefix("prefabs/").removesuffix(".binfab")
                        totals[item] = totals.get(item, 0) + n
            if totals:
                out.append({"station": label, "yields": [
                    {"item": k, "name": self.names.get(identity(self.prefabs.get(k)).get("name_key", ""), ""),
                     "count": v} for k, v in totals.items()]})
        return out


def _sprouts(prefabs: Prefabs) -> dict[str, dict]:
    templates: dict[str, str] = {}
    for stem in ("blocks/plants", "blocks/gardening"):
        pf = prefabs.get(stem)
        for row in (pf.root.leaf.get(1) if pf and pf.root else None) or []:
            if isinstance(row, Obj) and len(row) > 1 and isinstance(row[0].get(0), str):
                bp = row[1].get(0)
                if isinstance(bp, str) and bp:
                    templates[row[0][0]] = bp
    pf = prefabs.get("blocks/sprouts")
    out: dict[str, dict] = {}
    for row in (pf.root.leaf.get(1) if pf and pf.root else None) or []:
        if not isinstance(row, Obj) or len(row) < 2 or not isinstance(row[0].get(0), str):
            continue
        body = row[1]
        outs: list[tuple[str, float]] = [(o[3], float(o.get(4) or 0)) for o in _rows(body.get(2))
                                         if isinstance(o.get(3), str)]
        total = sum(w for _, w in outs) or 1
        grown = []
        for ref, w in outs:
            entry: dict[str, Any] = {"chance": round(w / total, 4)}
            if ref.startswith("plant/"):
                if ref not in templates:
                    continue
                entry["blueprint"] = templates[ref]
            else:
                entry["ref"] = ref.removesuffix(".binfab")
            grown.append(entry)
        if isinstance(body.get(1), (int, float)) and grown:
            out[row[0][0].removesuffix(".binfab").lower()] = {"seconds": body[1], "outcomes": grown}
    return out


def _use(prefabs: Prefabs, pf: Prefab, names: dict[str, str]) -> dict:
    roots: list[str] = []
    for cid, idx in USE.items():
        v = _own(pf, cid).get(idx)
        roots += [v] if isinstance(v, str) else [x for x in v or () if isinstance(x, str)]
    out: dict[str, Any] = {}
    for root in roots:
        root = root.removesuffix(".binfab")
        if not root or root == EMPTY_EFFECT:
            continue
        info = describe(prefabs, root, prefix=("abilities/", "effects/", "item/"))
        for key in ("stages", "healing"):
            rows = [{k: v for k, v in r.items() if k != "_depth"} for r in info[key]]
            if rows:
                out.setdefault(key, []).extend(rows)
        for eff in info["effects"]:
            row = {k: v for k, v in eff.items() if k not in ("_depth", "name_key", "description_key")}
            title = names.get(eff.get("name_key", ""), "")
            if title:
                row["title"] = title
            out.setdefault("effects", []).append(row)
    return out


def _cost(rows: Any, prefabs: Prefabs, names: dict[str, str]) -> list[dict]:
    out = []
    for r in _rows(rows):
        item, n = r.get(0), r.get(1)
        if isinstance(item, str) and isinstance(n, int) and n > 0:
            item = item.removesuffix(".binfab")
            out.append({"item": item, "name": names.get(identity(prefabs.get(item)).get("name_key", ""), ""),
                        "count": n})
    return out


def build(tree: GameTree) -> dict:
    prefabs = Prefabs(tree)
    names: dict[str, str] = {}
    for path in tree.files("languages/en/", ".binfab"):
        for key, text in locale(tree.read(path)).items():
            names.setdefault(key, text)
    guides = {k.removeprefix("$GuideUI_generic_").lower(): v for k, v in names.items()
              if k.startswith("$GuideUI_generic_")}
    pools: dict[int, dict[str, tuple[str, str]]] = {}

    def pool(ctype: int) -> dict[str, tuple[str, str]]:
        if ctype not in pools:
            name = COLLECTION_TYPES[ctype].lower() if 0 <= ctype < len(COLLECTION_TYPES) else ""
            pools[ctype] = collection_members(prefabs.get(f"collections/collection_{name}"), names) if name else {}
        return pools[ctype]

    decon = _Deconstruct(prefabs, names)
    sprouts = _sprouts(prefabs)
    items = []
    for path in [p for root in ROOTS for p in tree.files(root, ".binfab")]:
        rel = path[len("prefabs/"):-len(".binfab")]
        try:
            pf = parse(tree.read(path) or b"")
        except WireError:
            continue
        ident = _leaf(pf.component(49))
        name = names.get(ident.get(1) or "", "")
        if not name:
            continue
        folder = rel.split("/")[1] if rel.count("/") > 1 else ""
        slug = rel.removeprefix("item/").replace("/", "_")
        if rel.startswith("equipment/"):
            folder, slug = f"gear_{folder}".rstrip("_"), "gear_" + rel.removeprefix("equipment/").replace("/", "_")
        item: dict[str, Any] = {"slug": slug, "name": name,
                                "description": names.get(ident.get(5) or "", ""), "prefab": rel, "folder": folder}
        rarity = _rarity_name(ident)
        if rarity:
            item["rarity"] = rarity
        if ident.get(ID_TRADABLE) == 0:
            item["tradable"] = False
        bp = _leaf(pf.component(BLUEPRINT)).get(0)
        if isinstance(bp, str) and bp:
            item["blueprint"] = bp
        guide = guides.get(rel.replace("/", "_").lower()) or guides.get(rel.removeprefix("item/").replace("/", "_").lower())
        if guide:
            item["guide"] = guide

        unl = _own(pf, UNLOCKER) if pf.component(UNLOCKER) else {}
        ctypes: set[int] = set()
        unlocks = []
        for r in _rows(unl.get(8)):
            kind, ref = r.get(0), r.get(1)
            if isinstance(kind, int) and isinstance(ref, str) and ref:
                ctypes.add(kind)
                if kind != RECIPE:
                    u = _unlock_ref(kind, ref)
                    if u not in unlocks:
                        unlocks.append(u)
        if unlocks:
            item["unlocks"] = unlocks
        if unl.get(5) == 1 and unl.get(6) and unl.get(7):
            groups = {g for g in unl[6] if isinstance(g, str)}
            members = []
            for ctype in (t for t in unl[7] if isinstance(t, int)):
                ctypes.add(ctype)
                members += [_unlock_ref(ctype, ref) for ref, (gid, _) in pool(ctype).items() if gid in groups]
            if members:
                item["random_unlock"] = {"groups": sorted(groups), "members": list(dict.fromkeys(members))}

        food = _own(pf, FOOD) if pf.component(FOOD) else {}
        if food.get(0) == FOOD_KIND:
            stats = stat_rows(walk_values(food.get(1)))
            if stats:
                item["food"] = stats
        use = _use(prefabs, pf, names)
        if use:
            item["use"] = use
        titles = [t for t in _own(pf, TITLES).get(0) or () if isinstance(t, str)] if pf.component(TITLES) else []
        if titles:
            item["titles"] = titles
        advs = [a.get(0) for a in _rows(_own(pf, ADVENTURES).get(1)) if isinstance(a.get(0), str)] \
            if pf.component(ADVENTURES) else []
        if advs:
            item["adventures"] = advs
        xp = _own(pf, XP).get(0) if pf.component(XP) else None
        if isinstance(xp, int) and xp > 0:
            item["experience"] = xp
        if pf.component(LOCKBOX):
            opens = [{"label": o.get(4) if isinstance(o.get(4), str) else "",
                      "cost": _cost(o.get(3), prefabs, names)} for o in _rows(_own(pf, LOCKBOX).get(0))]
            if opens:
                item["opens"] = opens
        decay = _decay(pf)
        if decay:
            item["decay"] = decay
        if rel.lower() in sprouts:
            item["grows"] = sprouts[rel.lower()]

        tags = [t for t in _leaf(pf.component(TAGS)).get(0) or () if isinstance(t, str) and t]
        version = _own(pf, TAG_VERSION).get(2, 0) if pf.component(TAG_VERSION) else None
        keys = {f"{t}_ver{version}" for t in tags} if version is not None else set(tags)
        keys.add(rarity.replace(" ", ""))
        dec = decon(rel, ident.get(ID_TYPE), ctypes, keys)
        if dec:
            item["deconstruct"] = dec
        items.append(item)
    return {"items": items}


def count(data: dict) -> int:
    return len(data["items"])

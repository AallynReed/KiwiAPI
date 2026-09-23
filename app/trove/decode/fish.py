"""fish.json - every fish: weights, trophy sizes, trophies, deconstruction, and the
lure tables that decide what gets caught.

Fish are `prefabs/item/fish/<liquid>/*.binfab`: identity (49; field 9 is the item
rarity, field 10 the item type), tags (106) and blueprint (37). Enchanted fish sit
in their own folder but carry a `pools_<liquid>_<rarity>` tag - the tag the
"Catch Common Fish From Water Pools" adventures count - so that tag names the
liquid whose pools hold them.

`prefabs/fish/fish.binfab` lists a FishData row per fish: {0 fish, 1/2/3 basic,
silver and gold trophy placeables, 4 KFishSize}. Trove_x64.exe (FUN_1407db030)
clamps a caught fish's weight to the `fishweightdata` row of that KFishSize, then
names its size by cutting the range with that file's field-0 fractions (0.95,
0.03, 0.015, 0.005) cumulatively: Average, Trophy, Silver Trophy, Gold Trophy
Size (`$FishSize_*`). A Trophy/Silver/Gold catch is the one that gets that
row's trophy (FUN_1407db620).

Deconstructing (FUN_1405af490 + FUN_1405af160) builds keys from the fish's tags as
`<tag>_ver<version>` plus its rarity word, suffixed `_Basic/_Silver/_Gold` for a
trophy-size catch; every row of the item type's deconstruct table whose keys are
all present pays out. Version 1 is the current fish (anything else shows
`$OldFishVersion`, "Fermented fish").

Lure consumables apply an effect (398 section 1) whose bait component (1017, last
section) holds three enums, in the order its constructor declares them:
KFishingRarityType, KFishTrophyDropType, KFishingSpeedType. They key the
`fishingraritydata` odds (common/uncommon/rare), `fishtrophydata` odds (per size)
and `fishingspeeddata` (min/max wait, before player modifiers). Angler's lures go
through a splitter (46) whose variants are gated on fishing-upgrade tags (239).

What a pool or open water can hand out is decided by the server; the client has
no catch tables per fish, so none are emitted.
"""
from __future__ import annotations

from typing import Any

from app.trove.decode import fields as F
from app.trove.decode.ability import Prefabs, identity
from app.trove.decode.common import locale
from app.trove.decode.tree import GameTree
from app.trove.decode.wire import Obj, Prefab

TITLE = "Fish"
OUTPUT = "fish.json"
PREFIXES = ("prefabs/item/", "prefabs/fish/", "prefabs/meta/fishing/", "prefabs/placeable/",
            "prefabs/collections/", "prefabs/crafting/", "prefabs/effects/fishingbait/",
            "prefabs/upgrade/upgrades/", "languages/en/")
INDENT, FINAL_NEWLINE = 4, False

ID_RARITY, ID_ITEM_TYPE = 9, 10
TAGS, BLUEPRINT, USE_EFFECTS, SPLITTER, BAIT, CONDITIONS = 106, 37, 398, 46, 1017, 239

# Enum ordinals, from their name tables in Trove_x64.exe.
ITEM_RARITY = ("Common", "Uncommon", "Rare", "Epic", "Legendary", "Relic", "Resplendent")
SIZE_KEYS = ("$FishSize_Small", "$FishSize_Medium", "$FishSize_Large", "$FishSize_ExtraLarge")
SIZE_SUFFIX = ("", "_Basic", "_Silver", "_Gold")
RARITY_TYPES = ("Default", "Good", "Great", "Epic", "Legendary", "RarityBait1", "RarityBait2",
                "RarityBait3", "AlwaysRare")
TROPHY_TYPES = ("Default", "Good", "Great", "Epic", "TrophyBait1", "TrophyBait2", "TrophyBait3",
                "AlwaysTrophy")
SPEED_TYPES = ("Default", "Optimal", "Nonoptimal", "DefaultBait", "SpeedBait1", "SpeedBait2",
               "SpeedBait3", "Debug")
FISH_VERSION = 1


def _rows(pf: Prefab | None, idx: int = 0) -> list[dict]:
    """The leaf sections of the objects in a table file's root field ``idx``."""
    root = pf.root if pf else None
    return [r.leaf for r in (root.get(idx) if root is not None else None) or [] if isinstance(r, Obj)]


def _enum(names: tuple[str, ...], value: Any) -> str | None:
    return names[value] if isinstance(value, int) and 0 <= value < len(names) else None


class _Text:
    """Locale lookups. A key sits in the table its own name starts with
    (`$prefabs_item_crafting_runecrafting_ore_deep_name`), else in the one named
    after the prefab's folder (`$CollectionName_Water` -> prefabs_collections), else
    in a sibling of that folder's table (Soggy Homework is in ..._consumable_events)."""

    def __init__(self, tree: GameTree):
        self.tree = tree
        self._tables: dict[str, dict[str, str]] = {}

    def table(self, name: str) -> dict[str, str]:
        if name not in self._tables:
            self._tables[name] = locale(self.tree.read(f"languages/en/{name}.binfab"))
        return self._tables[name]

    def get(self, rel: str, key: str | None) -> str:
        if not key:
            return ""
        if key.startswith("@"):                   # an unlocalised literal
            return key[1:]
        parts, dirs = key.lstrip("$").split("_"), rel.split("/")[:-1]
        names = ["_".join(parts[:n]) for n in range(len(parts) - 1, 0, -1)]
        names += ["prefabs_" + "_".join(dirs[:n]) for n in range(len(dirs), 0, -1)]
        for name in names:
            if self.tree.exists(f"languages/en/{name}.binfab") and self.table(name).get(key):
                return self.table(name)[key]
        for path in self.tree.files("languages/en/prefabs_" + "_".join(dirs[:2]), ".binfab"):
            text = self.table(path[len("languages/en/"):-len(".binfab")]).get(key)
            if text:
                return text
        return ""


def _named(prefabs: Prefabs, text: _Text, rel: str) -> dict:
    ident = identity(prefabs.get(rel))
    return {"name": text.get(rel, ident.get("name_key")), "prefab": rel}


def _deconstruct_rows(prefabs: Prefabs, item_type: int) -> list[tuple[frozenset, list]]:
    rows = []
    for idx in _rows(prefabs.get("crafting/deconstruct")):
        if idx.get(0) != item_type or not isinstance(idx.get(1), str):
            continue
        for row in _rows(prefabs.get(idx[1].removeprefix("prefabs/"))):
            keys, outs = row.get(0) or [], row.get(1) or []
            outs = [(o.leaf.get(0), o.leaf.get(1)) for o in outs if isinstance(o, Obj)]
            rows.append((frozenset(keys), [(p, n) for p, n in outs if isinstance(p, str) and n]))
    return rows


def _yield(rows, keys: set[str], prefabs: Prefabs, text: _Text) -> list[dict]:
    total: dict[str, int] = {}
    for need, outs in rows:
        if need <= keys:
            for rel, n in outs:
                total[rel] = total.get(rel, 0) + n
    return [{**_named(prefabs, text, rel), "count": n} for rel, n in total.items()]


def _conditions(rows, negate: bool, req: set, exc: set) -> bool:
    """Collect the tags a condition list requires / forbids; False if it uses an
    operator other than setall / setnone / tags."""
    for row in rows or []:
        leaf = row.leaf if isinstance(row, Obj) else {}
        op, body = str(leaf.get(0, "")).lower(), leaf.get(1)
        args = body.get(0) if isinstance(body, Obj) else None
        if op == "tags":
            (exc if negate else req).update(a for a in args or [] if isinstance(a, str))
        elif op in ("setall", "setnone"):
            if not _conditions(args, negate ^ (op == "setnone"), req, exc):
                return False
        else:
            return False
    return True


def _upgrade_names(prefabs: Prefabs, text: _Text) -> dict[str, str]:
    """upgrade tag -> the name of the upgrade that grants it."""
    names: dict[str, str] = {}
    progression: dict[str, str] = {}
    for path in text.tree.files("languages/en/prefabs_progression", ".binfab"):
        progression.update(text.table(path.rsplit("/", 1)[-1].removesuffix(".binfab")))
    for path in prefabs.tree.files("prefabs/upgrade/upgrades/", ".binfab"):
        pf = prefabs.get(path.removeprefix("prefabs/"))
        entries = pf.root.get(0) if pf and pf.root is not None else None
        for entry in entries.values() if isinstance(entries, dict) else ():
            data = entry.get(1) if isinstance(entry, Obj) else None
            if not isinstance(data, Obj) or len(data) < 2:
                continue
            title = progression.get(data[0].get(0, ""), "")
            for tag in data.leaf.get(0) or []:
                if isinstance(tag, str) and title:
                    names.setdefault(tag, title)
    return names


def _bait(prefabs: Prefabs, ref: str, upgrades: dict[str, str]) -> list[dict]:
    pf = prefabs.get(ref)
    if pf is None:
        return []
    splitter = pf.component(SPLITTER)
    if splitter is not None and len(splitter) > 1:
        return [row for sub in splitter.leaf.get(0) or [] if isinstance(sub, str)
                for row in _bait(prefabs, sub, upgrades)]
    bait = pf.component(BAIT)
    if bait is None or len(bait) < 3:
        return []
    row: dict[str, Any] = {
        "catch": _enum(RARITY_TYPES, bait.leaf.get(0, 0)),
        "size": _enum(TROPHY_TYPES, bait.leaf.get(1, 0)),
        "bite": _enum(SPEED_TYPES, bait.leaf.get(2, 0)),
        "duration": bait[0].get(F.EFFECT_DURATION),
    }
    cond = pf.component(CONDITIONS)
    req: set[str] = set()
    exc: set[str] = set()
    if cond is not None and _conditions(cond.leaf.get(0), False, req, exc):
        for key, tags in (("requires", req), ("excludes", exc)):
            if tags:
                row[key] = sorted(upgrades.get(t, t) for t in tags)
    return [row]


def _lures(prefabs: Prefabs, text: _Text) -> list[dict]:
    upgrades = _upgrade_names(prefabs, text)
    out = []
    for path in prefabs.tree.files("prefabs/item/consumable/fishingbait/", ".binfab"):
        rel = path.removeprefix("prefabs/").removesuffix(".binfab")
        pf = prefabs.get(rel)
        ident = identity(pf)
        name = text.get(rel, ident.get("name_key"))
        use = pf.component(USE_EFFECTS) if pf else None
        if not name or use is None or len(use) < 2:
            continue
        effects = [row for ref in use.leaf.get(0) or [] if isinstance(ref, str)
                   for row in _bait(prefabs, ref, upgrades)]
        if effects:
            out.append({"name": name, "description": text.get(rel, ident.get("description_key")),
                        "prefab": rel, "effects": effects})
    return out


def _odds(pf: Prefab | None, names: tuple[str, ...], labels: list[str]) -> dict[str, dict]:
    out = {}
    for row in _rows(pf):
        key, values = _enum(names, row.get(0, 0)), row.get(1) or []
        if key and len(values) == len(labels):
            out[key] = dict(zip(labels, values, strict=True))
    return out


def build(tree: GameTree) -> dict:
    prefabs = Prefabs(tree)
    text = _Text(tree)
    sizes = [text.table("ui").get(k, "") for k in SIZE_KEYS]

    weight_pf = prefabs.get("meta/fishing/fishweightdata")
    fractions = weight_pf.root.get(0) if weight_pf and weight_pf.root is not None else None
    weights = {r.get(0, 0): (float(r.get(1, 0)), float(r.get(2, 0))) for r in _rows(weight_pf, 1)}
    fish_data = {r[0]: r for r in _rows(prefabs.get("fish/fish")) if isinstance(r.get(0), str)}

    categories: dict[str, str] = {}
    for cat in _rows(prefabs.get("collections/collection_fish")):
        title = text.get("collections/collection_fish", cat.get(1)) or str(cat.get(0, ""))
        for entry in cat.get(3) or []:
            if isinstance(entry, Obj) and isinstance(entry.get(0), str):
                categories.setdefault(entry.get(0), title)

    decon_cache: dict[int, list] = {}
    fish = []
    for path in tree.files("prefabs/item/fish/", ".binfab"):
        rel = path.removeprefix("prefabs/").removesuffix(".binfab")
        pf = prefabs.get(rel)
        obj = pf.component(F.IDENTITY) if pf else None
        name = text.get(rel, obj.get(F.ID_NAME)) if obj is not None else ""
        if pf is None or obj is None or not name:
            continue
        rarity = _enum(ITEM_RARITY, obj.get(ID_RARITY)) or ""
        tag_obj, bp = pf.component(TAGS), pf.component(BLUEPRINT)
        tags = [t for t in (tag_obj.get(0) if tag_obj is not None else None) or [] if isinstance(t, str)]
        entry: dict[str, Any] = {
            "slug": rel.removeprefix("item/fish/").replace("/", "_"),
            "name": name,
            "description": text.get(rel, obj.get(F.ID_DESCRIPTION)),
            "prefab": rel,
            "blueprint": bp.get(0) if bp is not None else None,
            "liquid": rel.split("/")[2],
            "rarity": rarity,
        }
        if rel in categories:
            entry["collection"] = categories[rel]
        pool = next((t.split("_")[1] for t in tags if t.startswith("pools_") and t.count("_") == 2), None)
        if pool:
            entry["pool"] = pool
        entry["tags"] = tags

        item_type = obj.get(ID_ITEM_TYPE)
        if isinstance(item_type, int) and item_type not in decon_cache:
            decon_cache[item_type] = _deconstruct_rows(prefabs, item_type)
        rows = decon_cache.get(item_type, []) if isinstance(item_type, int) else []
        keys = {f"{t}_ver{FISH_VERSION}" for t in tags}

        data = fish_data.get(rel)
        span = weights.get(data.get(4)) if data else None
        if span:
            entry["weight"] = {"min": span[0], "max": span[1]}
        lo = span[0] if span else 0.0
        bands = []
        for i, label in enumerate(sizes):
            band: dict[str, Any] = {"size": label}
            if span and isinstance(fractions, list) and i < len(fractions):
                hi = span[1] if i == len(sizes) - 1 else min(span[1], lo + fractions[i] * (span[1] - span[0]))
                band["min"], band["max"] = round(lo, 3), round(hi, 3)
                lo = hi
            if i and data and isinstance(data.get(i), str):
                band["trophy"] = _named(prefabs, text, data[i])
            band["deconstruct"] = _yield(rows, keys | {rarity + SIZE_SUFFIX[i]}, prefabs, text)
            bands.append(band)
        entry["sizes"] = bands
        fish.append(entry)

    liquids = {"water": 0, "lava": 1, "chocolate": 2, "plasma": 3}
    fish.sort(key=lambda f: (liquids.get(f["liquid"], 9), f["liquid"],
                             ITEM_RARITY.index(f["rarity"]) if f["rarity"] in ITEM_RARITY else 9,
                             f["name"].lower()))
    return {
        "fish": fish,
        "catch_odds": _odds(prefabs.get("meta/fishing/fishingraritydata"), RARITY_TYPES,
                            [r.lower() for r in ITEM_RARITY[:3]]),
        "size_odds": _odds(prefabs.get("meta/fishing/fishtrophydata"), TROPHY_TYPES, sizes),
        "bite_time": {k: {"min": r.get(1, 0), "max": r.get(2, 0)}
                      for r in _rows(prefabs.get("meta/fishing/fishingspeeddata"))
                      if (k := _enum(SPEED_TYPES, r.get(0, 0)))},
        "lures": _lures(prefabs, text),
    }


def count(data: dict) -> int:
    return len(data["fish"])

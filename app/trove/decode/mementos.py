"""mementos.json - Delve mementos: what each is, where it files, what it gives.

A memento is an unlocker item under `prefabs/item/unlocker/delve/{path,boss,decor}/`
(`events/` below each is event content). Identity (49) field 1 is its name and field
9 its rarity (the exe's rarity enum: 1 Uncommon, 2 Rare, 3 Epic; the Q-mento text
pairs these with Creature, Boss, Biome). Blueprint is 37 field 0; the tags the
deconstruct table matches on are 106 field 0. Unlocker (223) section 1 field 8
lists `{0 collection type, 1 path}`: type 18 (the exe's `$CollectionType_Memento`)
is the collection entry it grants - usually itself, but `boss/diatryma_carys`
grants `boss/saurianSwamp_diatryma` - and type 1 rows are recipes it also teaches.

`collections/collection_memento` files entries into groups (0 id, 1 name key,
2 counted, 3 entries). The exe leaves every entry of a group whose field 2 is 0 out
of mastery; that is the `Hidden` group. `collections/unlocks` adds more collect-time
unlocks per entry: the Memento Gateway recipes.

Mastery is the exe's formula: `meta/meta` field 0 gives the base per mastery source
(source 21 is Memento, its `$MasteryXPMsg_Memento` case) times the multiplier of the
`meta/multipliers` group listing the entry, or of the default group (root field 1).

Decay (472 field 5) is `{0 'quantitydecay', 1 {1 seconds, 3 KDecayableType}}` with
the quantitydecay section's field 0 = how many it destroys (-1: the whole stack, the
`$ItemDecay_QuantityAll` branch). Every memento decays after 3 hours online.

Loot Collecting runs `crafting/deconstruct`: collection type 18 at the Deconstructor
(station 2) reads `crafting/deconstruct_memento`, and every row whose tags the item
all carries adds its outputs - the Deep Kraken memento gets two rows.

A recipe (`prefabs/recipes/`) lists ingredients in root field 0 (`{0 item, 1 count}`)
and results in field 1 (an item and count, or field 3's `{0 collection type, 1 path}`
unlock); ones naming a memento give `crafted_by` and `used_in`. The daily Corrupted
Memories adventure (`meta/activities/npc_adventures`) counts consuming the mementos
it lists - older ones only.

What a memento commemorates is not referenced in either direction (no memento
names its creature or biome, and no creature names its memento), so it is not
emitted: matching on names would be a guess.
"""
from __future__ import annotations

from app.trove.decode.ability import identity
from app.trove.decode.common import locale
from app.trove.decode.fields import IDENTITY
from app.trove.decode.tree import GameTree
from app.trove.decode.wire import Obj, Prefab, WireError, parse

TITLE = "Mementos"
OUTPUT = "mementos.json"
PREFIXES = ("prefabs/item/", "prefabs/collections/", "prefabs/crafting/", "prefabs/meta/",
            "prefabs/recipes/", "prefabs/equipment/", "prefabs/placeable/",
            "languages/en/")
INDENT, FINAL_NEWLINE = 4, False

ROOT = "item/unlocker/delve/"
LANG = "languages/en/"
MEMENTO = 18                    # collection type
RECIPE = 1
MASTERY_SOURCE = 21
DECONSTRUCTOR = 2               # KStationType
ID_RARITY = 9
BLUEPRINT, TAGS, UNLOCKER, DECAY = 37, 106, 223, 472
RARITY = {0: "Common", 1: "Uncommon", 2: "Rare", 3: "Epic", 4: "Legendary"}
DECAY_TRIGGER = {0: "Offline", 1: "Online", 2: "LastLogin", 3: "Equipped",
                 4: "DailyReset", 5: "WeeklyReset", 6: "Event"}
ADVENTURE = "adv_daily_delve_memento"


def _rows(value) -> list[dict]:
    return [r.leaf for r in value or () if isinstance(r, Obj)]


class _Game:
    """Parsed prefabs and locale text, loaded on demand."""

    def __init__(self, tree: GameTree):
        self.tree = tree
        self._pf: dict[str, Prefab | None] = {}
        self._tables: dict[str, dict[str, str]] = {}
        stems = [p[len(LANG):-len(".binfab")] for p in tree.files(LANG, ".binfab")]
        self._stems = sorted(stems, key=len, reverse=True)

    def prefab(self, rel: str) -> Prefab | None:
        rel = rel.removesuffix(".binfab")
        if rel not in self._pf:
            data = self.tree.read(f"prefabs/{rel}.binfab")
            try:
                self._pf[rel] = parse(data) if data else None
            except WireError:
                self._pf[rel] = None
        return self._pf[rel]

    def table(self, stem: str) -> dict[str, str]:
        if stem not in self._tables:
            self._tables[stem] = locale(self.tree.read(f"{LANG}{stem}.binfab"))
        return self._tables[stem]

    def text(self, key, stem: str | None = None, rel: str = "") -> str:
        """A key's text: ``stem``'s table, else the tables its name or ``rel``'s folders
        prefix, else any table (a few keys sit in an unrelated one)."""
        if isinstance(key, str) and key.startswith("@"):         # an unlocalised literal
            return key[1:]
        if not isinstance(key, str) or not key.startswith("$"):
            return ""
        if stem:
            return self.table(stem).get(key, "")
        low = key[1:].lower()
        folder = "prefabs_" + rel.rsplit("/", 1)[0].replace("/", "_").lower() if "/" in rel else ""
        likely = [s for s in self._stems if low.startswith(s.lower() + "_") or folder.startswith(s.lower())]
        for s in likely + [s for s in self._stems if s not in likely]:
            if key in self.table(s):
                return self.table(s)[key]
        return ""

    def name(self, rel: str) -> str:
        return self.text(identity(self.prefab(rel)).get("name_key"), rel=rel)

    def recipe(self, rid: str) -> Prefab | None:
        return self.prefab(f"recipes/{rid}")


def _unlock(row: dict) -> tuple[int, str] | None:
    u = row.get(3)
    leaf = u.leaf if isinstance(u, Obj) else {}
    kind, path = leaf.get(0, -1), leaf.get(1)
    return (kind, path) if isinstance(kind, int) and kind >= 0 and isinstance(path, str) and path else None


def _recipe_results(g: _Game, pf: Prefab | None) -> list[dict]:
    """A recipe's results (root field 1): an item and count, or a collection unlock."""
    out = []
    for row in _rows(pf.root.leaf.get(1) if pf and pf.root else None):
        unlock = _unlock(row)
        if unlock:
            out.append({"unlock": unlock[1], "type": unlock[0], "name": g.name(unlock[1])})
        elif isinstance(row.get(0), str) and row[0]:
            out.append({"item": row[0], "count": row.get(1, 1), "name": g.name(row[0])})
    return out


def _recipe_name(g: _Game, rid: str) -> str:
    return next((r["name"] for r in _recipe_results(g, g.recipe(rid)) if r["name"]), "")


def _ingredients(g: _Game, pf: Prefab) -> list[dict]:
    return [{"item": r[0], "count": r.get(1, 1), "name": g.name(r[0])}
            for r in _rows(pf.root.leaf.get(0) if pf.root else None) if isinstance(r.get(0), str) and r[0]]


def _collection(g: _Game) -> tuple[dict[str, dict], list[dict]]:
    pf = g.prefab("collections/collection_memento")
    entries: dict[str, dict] = {}
    groups = []
    for grp in _rows(pf.root.leaf.get(0) if pf and pf.root else None):
        rows = _rows(grp.get(3))
        if not rows:
            continue
        info = {"id": grp.get(0, ""), "name": g.text(grp.get(1), "prefabs_collections") or grp.get(0, ""),
                "counted": bool(grp.get(2))}
        groups.append({**info, "entries": len(rows)})
        for r in rows:
            if isinstance(r.get(0), str):
                entries.setdefault(r[0], info)
    return entries, groups


def _mastery(g: _Game) -> dict[str, int] | None:
    """Collectible path -> mastery, plus "" for the default; None if unreadable."""
    meta, mult = g.prefab("meta/meta"), g.prefab("meta/multipliers")
    if not (meta and meta.root and mult and mult.root):
        return None
    base = next((r[1] for r in _rows(meta.root.leaf.get(0)) if r.get(0) == MASTERY_SOURCE), None)
    groups = _rows(mult.root.leaf.get(0))
    default = mult.root.leaf.get(1)
    if not isinstance(base, int) or not isinstance(default, int) or not 0 <= default < len(groups):
        return None
    out = {"": round(base * groups[default].get(0, 0))}
    for grp in groups:
        for r in _rows(grp.get(1)):
            if r.get(0) == MEMENTO and isinstance(r.get(1), str):
                out[r[1]] = round(base * grp.get(0, 0))
    return out


def _deconstruct_rows(g: _Game) -> list[dict]:
    master = g.prefab("crafting/deconstruct")
    table = next((r.get(1) for r in _rows(master.root.leaf.get(1) if master and master.root else None)
                  if r.get(0) == MEMENTO and r.get(2) == DECONSTRUCTOR), None)
    pf = g.prefab(table.removeprefix("prefabs/")) if isinstance(table, str) else None
    rows = []
    for r in _rows(pf.root.leaf.get(0) if pf and pf.root else None):
        tags = {t for t in r.get(0) or () if isinstance(t, str)}
        outs = [{"item": o[0], "count": o.get(1, 1), "name": g.name(o[0])}
                for o in _rows(r.get(1)) if isinstance(o.get(0), str)]
        rows.append({"tags": tags, "outputs": outs})
    return rows


def _collect_unlocks(g: _Game) -> dict[str, list[str]]:
    """Collectible path -> recipes collecting it also teaches (`collections/unlocks`)."""
    pf = g.prefab("collections/unlocks")
    out: dict[str, list[str]] = {}
    for r in _rows(pf.root.leaf.get(0) if pf and pf.root else None):
        head = r.get(0)
        src = head.leaf if isinstance(head, Obj) else {}
        if src.get(0) != MEMENTO or not isinstance(src.get(1), str):
            continue
        out.setdefault(src[1], []).extend(
            o[1] for o in _rows(r.get(1)) if o.get(0) == RECIPE and isinstance(o.get(1), str))
    return out


def _adventure(g: _Game) -> tuple[dict, set[str]]:
    """The daily Corrupted Memories adventure and the memento items it counts."""
    pf = g.prefab("meta/activities/npc_adventures")
    for adv in _rows(pf.root.leaf.get(1) if pf and pf.root else None):
        if adv.get(0) != ADVENTURE:
            continue
        items = set()
        stack = [adv.get(3)]
        while stack:
            v = stack.pop()
            if isinstance(v, str) and v.startswith(ROOT):
                items.add(v.lower())
            elif isinstance(v, dict):
                stack.extend(v.values())
            elif isinstance(v, (list, tuple)):
                stack.extend(v)
        info = {"name": g.text(adv.get(1), "adventures"), "description": g.text(adv.get(2), "adventures")}
        return info, items
    return {}, set()


def _decay(pf: Prefab) -> dict | None:
    comp = pf.component(DECAY)
    for row in _rows(comp.get(5) if comp else None):
        timer = row.get(1)
        if row.get(0) != "quantitydecay" or not isinstance(timer, Obj) or not timer:
            continue
        base, own = timer[0], timer.leaf if len(timer) > 1 else {}
        seconds = base.get(1)
        if isinstance(seconds, float) and seconds.is_integer():
            seconds = int(seconds)
        out = {"trigger": DECAY_TRIGGER.get(base.get(3), base.get(3)), "seconds": seconds}
        if isinstance(own.get(0), int):
            out["destroys"] = "all" if own[0] == -1 else own[0]
        return out
    return None


def build(tree: GameTree) -> dict:
    g = _Game(tree)
    entries, groups = _collection(g)
    mastery = _mastery(g)
    deconstruct = _deconstruct_rows(g)
    teaches = _collect_unlocks(g)
    adventure, adventure_items = _adventure(g)

    made_by: dict[str, list[dict]] = {}
    used_in: dict[str, list[dict]] = {}
    for path in tree.files("prefabs/recipes/", ".binfab"):
        data = tree.read(path) or b""
        if ROOT.encode() not in data:
            continue
        rid = path[len("prefabs/recipes/"):-len(".binfab")]
        pf = g.recipe(rid)
        if pf is None or pf.root is None:
            continue
        results = _recipe_results(g, pf)
        ingredients = _ingredients(g, pf)
        for r in results:
            target = r.get("unlock") or r.get("item") or ""
            if target.startswith(ROOT):
                made_by.setdefault(target.lower(), []).append({"recipe": rid, "ingredients": ingredients})
        for i in ingredients:
            if i["item"].startswith(ROOT):
                used_in.setdefault(i["item"].lower(), []).append(
                    {"recipe": rid, "makes": [{k: v for k, v in r.items() if k != "type"} for r in results]})

    out = []
    for path in tree.files(f"prefabs/{ROOT}", ".binfab"):
        rel = path[len("prefabs/"):-len(".binfab")]
        pf = g.prefab(rel)
        unlocker = pf.component(UNLOCKER) if pf else None
        if pf is None or unlocker is None:
            continue
        grants: list[tuple[int, str]] = [(r.get(0, -1), r[1]) for r in _rows(unlocker.get(8))
                                         if isinstance(r.get(1), str)]
        collectible = next((p for k, p in grants if k == MEMENTO), None)
        ident = identity(pf)
        name = g.text(ident.get("name_key"), rel=rel)
        if collectible is None or not name:
            continue
        sub = rel[len(ROOT):]
        ident_comp = pf.component(IDENTITY)
        leaf = ident_comp.leaf if ident_comp else {}
        row: dict = {"slug": sub.replace("/", "_"), "name": name}
        if name.startswith("Memento: "):
            row["title"] = name[len("Memento: "):]
        if description := g.text(ident.get("description_key"), rel=rel):
            row["description"] = description
        row["prefab"] = rel
        if collectible.lower() != rel.lower():
            row["collectible"] = collectible
        bp = pf.component(BLUEPRINT)
        if bp is not None and isinstance(bp.get(0), str) and bp.get(0):
            row["blueprint"] = bp.get(0).removesuffix(".blueprint")
        if leaf.get(ID_RARITY) in RARITY:
            row["rarity"] = RARITY[leaf[ID_RARITY]]
        entry = entries.get(collectible)
        if entry:
            row["category"], row["category_id"] = entry["name"], entry["id"]
        if "/events/" in f"/{sub}":
            row["event"] = True

        if entry and entry["counted"] and mastery is not None:
            row["mastery"] = mastery.get(collectible, mastery[""])

        if decay := _decay(pf):
            row["decay"] = decay

        tag_comp = pf.component(TAGS)
        tags = {t for t in (tag_comp.get(0) if tag_comp else None) or () if isinstance(t, str)}
        loot = [o for r in deconstruct if r["tags"] <= tags for o in r["outputs"]]
        if loot:
            row["deconstruct"] = loot

        recipes = [p for k, p in grants if k == RECIPE] + teaches.get(collectible, [])
        taught = []
        for rid in dict.fromkeys(recipes):
            taught.append({"recipe": rid, "name": _recipe_name(g, rid)})
        if taught:
            row["teaches"] = taught
        if made := made_by.get(rel.lower()) or made_by.get(collectible.lower()):
            row["crafted_by"] = made
        if used := used_in.get(rel.lower()):
            row["used_in"] = used
        if rel.lower() in adventure_items:
            row["daily_adventure"] = True
        out.append(row)

    order = {"path": 0, "boss": 1, "decor": 2}
    out.sort(key=lambda m: (order.get(m["prefab"][len(ROOT):].split("/", 1)[0], 9), m["name"].lower()))
    return {
        "categories": groups,
        "adventure": adventure,
        "mementos": out,
    }


def count(data: dict) -> int:
    return len(data["mementos"])

"""placeables.json - the blocks, decorations and building pieces a player can own,
grouped the way the game offers them.

A placeable is `prefabs/placeable/**/<id>.binfab`. Identity (49): 1 name, 2 the
KItemCategory (Crafting, Items, Building, Decoration, Blocks - the inventory and
market category, shown as `$AuctionItemCategory_*`), 5 description, 9 rarity
(the tooltip reads it), 10 KItemType (26 = Compostable). Component 50 is the block
it places: leaf field 0 = {0 KBlockType, 1 a per-type parameter - the RGB colour on
the colour, glass and glow blocks ("Green Screen" is 0x00ff00), 2 block data whose
base section names the entity it spawns (0, a station or gateway `_interactive`
twin) or its template key (6)}; leaf field 4 is the placement restriction mask the
item tooltip reads (Trove_x64.exe FUN_14039bf40: 1/2/4/8/16, 1|16 together is one
line). KBlockType names are the `$Block_*_name` keys its registration
(FUN_140570660) writes for each id.

`prefabs/blocks/{blocks,torches,signs,trophies,particleinteractables}` are
BlockTemplateLists ("Matches block type with various data particular to that block
type"): root 0 a KBlockType, 1 entries {section 0: 0 the key a block's data names;
section 1: 1 model name, 3 particle effect, 7 light level}. Field 7 is set only on
light sources (torches, lamps, lanterns, glowing gumdrops) and field 6 then holds
an RGBA colour matching their names; the float reader cannot return those bits
exactly, so only the level is kept. A model name is a blueprint basename, resolved
against the client's blueprint list the way the codex extractor does. The small
grass/flower/seaweed types (8, 34, 106) pick models from lists inside the exe, so
those have none here.

What a player can own is not one flag: the client carries no loot tables. An entry
is kept when the client proves an item form - a loaded recipe
(`collections/collection_recipe`) offered by a crafting component (61; tabs in
field 14, or a profession's tiers), a fish's trophy (`fish/fish`: {0 fish, 1/2/3
Trophy/Silver/Gold size trophy}), a claim table reward (`claim/*`), a club fixture
(`meta/inventory` field 2, by club UI subheading), a recipe ingredient, a Loot
Collector entry of its own (`crafting/deconstruct` field 2), or the Compostable
item type. Deconstruction follows the exe (FUN_1405af490): the per-prefab table for
that station, else the item type's; the keys are the item's tags (component 106,
each `<tag>_ver<n>` when the versioned component 311 is present, n = its field 2)
plus its rarity name, and every row whose tags are all present pays out.

Groups: a crafted entry sits with the station that offers it (the one offering the
most placeables when several do) - one group per station, one per tab for the few
stations offering more than 150 (Cube Converter; Highlands, Robotic, Faerie Workbench),
and stations offering fewer than 5 pooled. Biome workbenches are thereby the
biome decoration sets. The rest: fish trophies (311), other Compostables (monster
trophies, prize produce), club fixtures, and one group for what is left.
Named placeables with none of the signals above - world props, NPC and portal
placements, tutorial signs, delve dressing - are left out.
"""
from __future__ import annotations

import re
from typing import Any

from app.trove.codexes.extract import blueprint_stems, model_blueprint
from app.trove.decode.common import locale, stem
from app.trove.decode.recipes import PROFESSIONS, _components, _label, _tabs, _tiers
from app.trove.decode.tree import GameTree
from app.trove.decode.wire import Obj, WireError, parse, strings

TITLE = "Blocks and placeables"
OUTPUT = "placeables.json"
PREFIXES = ("prefabs/placeable/", "prefabs/blocks/", "prefabs/recipes/", "prefabs/collections/",
            "prefabs/crafting/", "prefabs/claim/", "prefabs/fish/", "prefabs/meta/", "prefabs/professions/",
            "prefabs/npc/", "prefabs/item/", "blueprints/", "languages/en/")
INDENT, FINAL_NEWLINE = None, True

ROOT = "placeable/"
IDENTITY, PLACER, BLUEPRINT, CRAFTING, STATION, TAGS, VERSIONED, FIXTURE = 49, 50, 37, 61, 76, 106, 311, 409
ID_NAME, ID_CATEGORY, ID_DESCRIPTION, ID_RARITY, ID_ITEM_TYPE = 1, 2, 5, 9, 10
BLOCK, RESTRICTIONS = 0, 4
BLOCK_TYPE, BLOCK_PARAM, BLOCK_DATA = 0, 1, 2
DATA_ENTITY, DATA_TEMPLATE = 0, 6
TPL_MODEL, TPL_VFX, TPL_LIGHT = 1, 3, 7
STATION_TITLE, STATION_PROFESSION = 0, 2
VERSION = 2
FIXTURE_TIERS, TIER_TEXT = 1, 6
COMPOSTABLE = 26
LOOT_COLLECTOR, COMPOST = 2, 8
SPLIT_STATION, SMALL_STATION = 150, 5
TEMPLATE_TABLES = ("trophies", "blocks", "torches", "signs", "particleinteractables")

# Enum tables read out of Trove_x64.exe.
CATEGORIES = ("Crafting", "Items", "Building", "Decoration", "Blocks")
ITEM_RARITY = ("Common", "Uncommon", "Rare", "Epic", "Legendary", "Relic", "Resplendent")
# Block types whose parameter is an RGB colour (names match: "Laser Blue" 0x10d5ff).
COLOURED = frozenset({18, 24, 25, 29, 55})
RESTRICTION_TEXT = ((0x11, "$PlaceableNotInClubWorldOrGroupAccessLocations"), (8, "$PlaceableNotOnCornerstone"),
                    (2, "$PlaceableNotOnPublicWorlds"), (0x10, "$PlaceableNotInClubWorld"),
                    (4, "$PlaceableNotWhenAtMaxGardening"), (1, "$PlaceableNotInGroupAccessLocations"))
# KBlockType -> the name key its registration gives it (only ids whose key is written by that call).
BLOCK_TYPES = {
    1: "Dirt", 2: "Grass", 3: "Stone", 4: "Prefab", 5: "With_Game_Object", 7: "Sign", 11: "Trophy",
    12: "Player_Trophy", 15: "Bedrock", 20: "Sand", 24: "Player_Color", 27: "Resource", 28: "Reflective",
    30: "Canopy", 38: "Tag", 50: "Hard_Dirt", 51: "Hard_Stone", 57: "Plug", 58: "Socket",
    59: "Prefab_Decoration", 60: "Prefab_Indestructible", 61: "Chest", 62: "Sprout", 63: "Plant", 64: "Plant",
    69: "Flowing_Lava", 70: "Plasma", 71: "Flowing_Plasma", 75: "Spawner", 80: "Chocolate",
    81: "Flowing_Chocolate", 83: "Acid", 84: "Flowing_Acid", 87: "Flowing_Balefire", 89: "ParticlesInteractable",
    90: "Music_Note", 91: "Music_Instrument", 92: "Numbers", 93: "Particles", 94: "LED", 95: "Milk",
    96: "Flowing_Milk", 104: "Glowing_Grass", 105: "Prefab_No_Collide", 107: "Prefab_Intangible",
    120: "Railroad_Track", 121: "Monorail_Track", 122: "Railroad_Track", 151: "Air_Vent", 154: "Vanishing",
    158: "Air_Current_Source", 159: "Air_Current", 160: "Collision_Target", 161: "Transmute",
    162: "Transmute_Passable", 172: "Mimic", 173: "Boat_Booster", 174: "Anti_Gravity", 176: "SpawnOnDestroy",
    177: "Matchmaker", 180: "Sky_Projector", 181: "Jukebox",
}
FISH_SIZES = ("", "$FishSize_Medium", "$FishSize_Large", "$FishSize_ExtraLarge")
WANTED = {IDENTITY, PLACER, BLUEPRINT, CRAFTING, STATION, TAGS, VERSIONED, FIXTURE}


def _leaf(v: Any) -> dict:
    return v.leaf if isinstance(v, Obj) else {}


def _rel(v: Any) -> str:
    if not isinstance(v, str) or not v or v.lower().endswith(".blueprint"):
        return ""
    return v.strip().lower().removeprefix("prefabs/").removesuffix(".binfab")


def _root(tree: GameTree, rel: str) -> dict:
    try:
        pf = parse(tree.read(f"prefabs/{rel}.binfab") or b"")
    except WireError:
        return {}
    return _leaf(pf.root)


def _slug(text: str) -> str:
    return re.sub(r"[^a-z0-9]+", "-", text.lower()).strip("-")


def _text(tree: GameTree) -> dict[str, str]:
    out: dict[str, str] = {}
    for path in tree.files("languages/en/", ".binfab"):
        for key, value in locale(tree.read(path)).items():
            out.setdefault(key, value)
    return out


def _prefabs(tree: GameTree, prefix: str) -> dict[str, dict[int, Obj]]:
    """`rel -> {component id: object}` for the components this decoder reads."""
    out = {}
    for path in tree.files(f"prefabs/{prefix}", ".binfab"):
        comps = _components(tree.read(path) or b"", WANTED)
        if comps:
            out[_rel(path)] = comps
    return out


def _templates(tree: GameTree) -> dict[str, dict]:
    """Template key -> its data section, first table listing it wins."""
    out: dict[str, dict] = {}
    for table in TEMPLATE_TABLES:
        for row in _root(tree, f"blocks/{table}").get(1) or []:
            if isinstance(row, Obj) and len(row) > 1 and isinstance(row[0].get(0), str):
                out.setdefault(row[0][0].lower(), row[-1])
    return out


def _recipes(tree: GameTree) -> tuple[dict[str, str], dict[str, list[str]], dict[str, list[str]]]:
    """Recipe ids by lowercase id, and placeable results / ingredients per recipe."""
    known = {stem(p).lower(): stem(p) for p in tree.files("prefabs/recipes/", ".binfab")}
    results: dict[str, list[str]] = {}
    ingredients: dict[str, list[str]] = {}
    for rid in known.values():
        root = _root(tree, f"recipes/{rid}")
        results[rid] = [r for o in root.get(1) or [] if (r := _rel(_leaf(o).get(0))).startswith(ROOT)]
        ingredients[rid] = [r for o in root.get(0) or [] if (r := _rel(_leaf(o).get(0))).startswith(ROOT)]
    return known, results, ingredients


def _loaded(tree: GameTree, known: dict[str, str]) -> set[str]:
    """Recipe ids `collections/collection_recipe` lists - the only ones the game loads."""
    out = set()
    for group in _root(tree, "collections/collection_recipe").get(0) or []:
        for row in _leaf(group).get(3) or []:
            rid = _leaf(row).get(0)
            if isinstance(rid, str) and rid.lower() in known:
                out.add(known[rid.lower()])
    return out


def _decon_tables(tree: GameTree) -> tuple[dict, dict, dict[str, list]]:
    """(item type, station) -> table, (prefab, station) -> table, and each table's rows."""
    master = _root(tree, "crafting/deconstruct")
    by_type = {(r.get(0), r.get(2)): _rel(r.get(1)) for r in map(_leaf, master.get(0) or [])}
    by_prefab = {(_rel(r.get(0)), r.get(2)): _rel(r.get(1)) for r in map(_leaf, master.get(2) or [])}
    rows: dict[str, list] = {}
    for table in {*by_type.values(), *by_prefab.values()} - {""}:
        rows[table] = []
        for r in map(_leaf, _root(tree, table).get(0) or []):
            outs = [(_rel(_leaf(o).get(0)), _leaf(o).get(1)) for o in r.get(1) or []]
            rows[table].append((frozenset(t for t in r.get(0) or [] if isinstance(t, str)),
                                [(p, n) for p, n in outs if p and isinstance(n, int) and n]))
    return by_type, by_prefab, rows


class _Items:
    """Names of referenced items (deconstruct outputs, fish), each file read once."""

    def __init__(self, tree: GameTree, text: dict[str, str]):
        self.tree, self.text, self.cache = tree, text, {}

    def name(self, rel: str) -> str:
        if rel not in self.cache:
            comps = _components(self.tree.read(f"prefabs/{rel}.binfab") or b"", {IDENTITY})
            self.cache[rel] = self.text.get(_leaf(comps.get(IDENTITY)).get(ID_NAME) or "", "")
        return self.cache[rel]


def build(tree: GameTree) -> dict:
    text = _text(tree)
    items = _Items(tree, text)
    placeables = _prefabs(tree, ROOT)
    npcs = _prefabs(tree, "npc/")
    templates = _templates(tree)
    known, results, ingredients = _recipes(tree)
    loaded = _loaded(tree, known)
    valid = {p.removeprefix("blueprints/").lower() for p in tree.files("blueprints/", ".blueprint")}
    stems = blueprint_stems(valid)

    places: dict[str, str] = {}
    for rel, comps in placeables.items():
        data = _leaf(_leaf(comps.get(PLACER)).get(BLOCK)).get(BLOCK_DATA)
        target = _rel(data[0].get(DATA_ENTITY)) if isinstance(data, Obj) and data else ""
        if target:
            places[rel] = target
    placer = {v: k for k, v in reversed(list(places.items()))}

    def name_of(rel: str) -> str:
        return text.get(_leaf(placeables.get(rel, {}).get(IDENTITY)).get(ID_NAME) or "", "")

    # Every crafting component: its tabs, and which placeables each tab offers.
    stations: dict[str, dict] = {}
    offers: dict[str, list[tuple[str, int]]] = {}
    rank: dict[tuple[str, str], int] = {}
    for rel, comps in (*placeables.items(), *npcs.items()):
        if CRAFTING not in comps:
            continue
        st = _leaf(comps.get(STATION))
        prof = PROFESSIONS[st[STATION_PROFESSION]] if st.get(STATION_PROFESSION) in range(len(PROFESSIONS)) else None
        tabs = _tiers(tree, prof) if prof else _tabs(comps[CRAFTING])
        title = _label(st.get(STATION_TITLE), text) or name_of(placer.get(rel, ""))
        slug = rel.removeprefix(ROOT).removeprefix("crafting/").replace("/", "_")
        stations[rel] = {"slug": slug, "name": title, "tabs": [_label(t["name"], text) for t in tabs], "count": 0}
        for i, tab in enumerate(tabs):
            for rid in dict.fromkeys(known.get(r.lower(), "") for r in tab["recipes"]):
                if rid in loaded:
                    for res in results[rid]:
                        if (rel, i) not in offers.setdefault(res, []):
                            offers[res].append((rel, i))
                        rank.setdefault((rel, res), len(rank))
    for spots in offers.values():
        for st in dict.fromkeys(s for s, _ in spots):
            stations[st]["count"] += 1
    made_by: dict[str, list[str]] = {}
    used_in: dict[str, list[str]] = {}
    for rid in sorted(loaded):
        for res in results[rid]:
            made_by.setdefault(res, []).append(rid)
        for ing in ingredients[rid]:
            used_in.setdefault(ing, []).append(rid)

    claims: dict[str, list[str]] = {}
    for path in tree.files("prefabs/claim/", ".binfab"):
        table = _rel(path)
        if table == "claim/test":
            continue
        for ref in strings(_root(tree, table)):
            if (r := _rel(ref)).startswith(ROOT) and table not in claims.get(r, []):
                claims.setdefault(r, []).append(table)
    fishing: dict[str, dict] = {}
    for row in map(_leaf, _root(tree, "fish/fish").get(0) or []):
        fish = _rel(row.get(0))
        for size in (1, 2, 3):
            trophy = _rel(row.get(size))
            if fish and trophy:
                fishing.setdefault(trophy, {"fish": fish, "fish_name": items.name(fish),
                                            "size": text.get(FISH_SIZES[size], "")})
    fixtures: dict[str, str] = {}
    for section in map(_leaf, _root(tree, "meta/inventory").get(2) or []):
        for ref in section.get(0) or []:
            if _rel(ref):
                fixtures.setdefault(_rel(ref), text.get(section.get(1) or "", ""))
    by_type, by_prefab, decon_rows = _decon_tables(tree)
    collected = {p for p, _ in by_prefab}

    def deconstruct(rel: str, comps: dict, rarity: str, item_type: Any) -> dict[str, list[dict]]:
        tags = [t for t in _leaf(comps.get(TAGS)).get(0) or [] if isinstance(t, str) and t]
        if VERSIONED in comps:
            tags = [f"{t}_ver{_leaf(comps[VERSIONED]).get(VERSION, 0)}" for t in tags]
        keys = {*tags, rarity}
        out = {}
        for station, label in (("loot_collector", LOOT_COLLECTOR), ("compost", COMPOST)):
            table = by_prefab.get((rel, label)) or by_type.get((item_type, label))
            total: dict[str, int] = {}
            for need, outs in decon_rows.get(table or "", []):
                if need <= keys:
                    for item, n in outs:
                        total[item] = total.get(item, 0) + n
            if total:
                out[station] = [{"item": i, "name": items.name(i), "count": n} for i, n in total.items()]
        return out

    def model(rel: str, comps: dict, template: dict) -> str:
        own = _leaf(comps.get(BLUEPRINT)).get(0)
        if isinstance(own, str) and own:
            return own
        name = template.get(TPL_MODEL)
        found = model_blueprint(name, valid, stems) if isinstance(name, str) and name else None
        if found:
            return found
        twin = _leaf(placeables.get(places.get(rel, ""), {}).get(BLUEPRINT)).get(0)
        return twin if isinstance(twin, str) else ""

    groups: dict[tuple, list[tuple[int, dict]]] = {}
    for rel, comps in placeables.items():
        ident = _leaf(comps.get(IDENTITY))
        name = text.get(ident.get(ID_NAME) or "", "")
        if not name:
            continue
        item_type = ident.get(ID_ITEM_TYPE)
        spots = sorted(offers.get(rel, []), key=lambda s: (-stations[s[0]]["count"], stations[s[0]]["slug"], s[1]))
        if not (spots or rel in fishing or rel in claims or rel in fixtures or rel in used_in
                or item_type == COMPOSTABLE or rel in collected):
            continue
        rarity = ITEM_RARITY[ident[ID_RARITY]] if ident.get(ID_RARITY) in range(len(ITEM_RARITY)) else ""
        category = ident.get(ID_CATEGORY) if ident.get(ID_CATEGORY) in CATEGORIES else ""
        block = _leaf(_leaf(comps.get(PLACER)).get(BLOCK))
        data = block.get(BLOCK_DATA)
        key = data[0].get(DATA_TEMPLATE) if isinstance(data, Obj) and data else None
        template = templates.get(key.lower(), {}) if isinstance(key, str) else {}

        entry: dict[str, Any] = {"slug": rel.removeprefix(ROOT).replace("/", "_"), "name": name,
                                 "description": text.get(ident.get(ID_DESCRIPTION) or "", ""), "prefab": rel}
        if category:
            entry["category"] = category
        if rarity:
            entry["rarity"] = rarity
        bp = model(rel, comps, template)
        if bp:
            entry["blueprint"] = bp
        btype = block.get(BLOCK_TYPE)
        if isinstance(btype, int):
            entry["block_type"] = btype
            if btype in COLOURED and isinstance(block.get(BLOCK_PARAM), int):
                entry["color"] = f"#{block[BLOCK_PARAM] & 0xFFFFFF:06x}"
        if isinstance(template.get(TPL_LIGHT), int) and template[TPL_LIGHT] > 0:
            entry["light"] = template[TPL_LIGHT]
        if isinstance(template.get(TPL_VFX), str) and template[TPL_VFX]:
            entry["vfx"] = template[TPL_VFX]
        mask = _leaf(comps.get(PLACER)).get(RESTRICTIONS)
        if isinstance(mask, int) and mask:
            lines, left = [], mask
            for bits, line in RESTRICTION_TEXT:
                if left & bits == bits:
                    lines.append(text.get(line, ""))
                    left &= ~bits
            entry["restrictions"] = [line for line in lines if line]
        if rel in places:
            entry["places"] = places[rel]
            if places[rel] in stations:
                st = stations[places[rel]]
                entry["station"] = {"slug": st["slug"], "name": st["name"]}
        if spots:
            entry["crafted_at"] = [{"station": stations[s]["slug"], "name": stations[s]["name"],
                                    "tab": stations[s]["tabs"][i]} for s, i in spots]
            entry["recipes"] = made_by.get(rel, [])
        if rel in used_in:
            entry["used_in"] = used_in[rel]
        if rel in fishing:
            entry["fishing"] = fishing[rel]
        if rel in claims:
            entry["claims"] = claims[rel]
        if rel in fixtures:
            entry["club_section"] = fixtures[rel]
            tiers = [text.get(_leaf(t).get(TIER_TEXT) or "", "") for t in _leaf(comps.get(FIXTURE)).get(FIXTURE_TIERS) or []]
            if any(tiers):
                entry["tiers"] = tiers
        entry.update(deconstruct(rel, comps, rarity, item_type))

        if spots:
            home = stations[spots[0][0]]["count"]
            gkey: tuple = (("small",) if home < SMALL_STATION else
                           ("crafted", spots[0][0], spots[0][1] if home > SPLIT_STATION else None))
        elif VERSIONED in comps:
            gkey = ("fish_trophies",)
        elif item_type == COMPOSTABLE:
            gkey = ("trophies",)
        elif rel in fixtures:
            gkey = ("club_fixtures",)
        else:
            gkey = ("other",)
        groups.setdefault(gkey, []).append((rank.get((spots[0][0], rel), 0) if spots else 0, entry))

    return {"categories": {c: text.get(f"$AuctionItemCategory_{c}", c) for c in CATEGORIES},
            "block_types": {k: text.get(f"$Block_{v}_name", v.replace("_", " ")) for k, v in BLOCK_TYPES.items()},
            "groups": _finish(groups, stations)}


def _finish(groups: dict[tuple, list[tuple[int, dict]]], stations: dict[str, dict]) -> list[dict]:
    """Name, order and slug the groups; crafted entries keep their station's order."""
    fixed = {"small": "Other crafting stations", "fish_trophies": "Fish trophies",
             "trophies": "Compostable decorations", "club_fixtures": "Club fixtures", "other": "Other placeables"}
    order = ("crafted", "small", "fish_trophies", "trophies", "club_fixtures", "other")
    labels = {}
    for gkey in groups:
        st = stations[gkey[1]] if gkey[0] == "crafted" else {}
        labels[gkey] = ((st["tabs"][gkey[2]] if gkey[2] is not None else "") or st["name"]) if st else fixed[gkey[0]]
    names = list(labels.values())
    rows = []
    for gkey, members in groups.items():
        if gkey[0] == "crafted":
            entries = [e for _, e in sorted(members, key=lambda m: m[0])]
        else:
            entries = sorted((e for _, e in members), key=lambda e: (e["name"].lower(), e["slug"]))
        cats = [e.get("category", "") for e in entries]
        row: dict[str, Any] = {"name": labels[gkey], "category": max(CATEGORIES, key=cats.count)}
        if gkey[0] == "crafted":
            st = stations[gkey[1]]
            if names.count(labels[gkey]) > 1:
                row["name"] = f"{labels[gkey]} ({st['name']})"
            row["station"] = {"slug": st["slug"], "name": st["name"]}
        rows.append((order.index(gkey[0]) > 1, CATEGORIES[::-1].index(row["category"]), row["name"].lower(),
                     row, entries))
    out, slugs = [], set()
    for *_, row, entries in sorted(rows, key=lambda r: r[:3]):
        slug = _slug(row["name"])
        while slug in slugs:
            slug += "-2"
        slugs.add(slug)
        out.append({"slug": slug, **row, "entries": entries})
    return out


def count(data: dict) -> int:
    return sum(len(g["entries"]) for g in data.get("groups") or [])

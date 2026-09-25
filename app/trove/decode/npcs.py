"""npcs.json - enemies, bosses and friendly NPCs, and what each one does.

An NPC is a `prefabs/npc/**` entity of type 44; its root component 44 is the NPC
class (the exe registers both of its factories under id 44). Read structurally:

- 43 field 4 is the nameplate: a `$key`, an `@literal` or a bare literal the client
  shows as written. Field 24 is a description key, 25 a club icon, and 19 the
  ability list: rows `{0 ability refs, 4 requirement {0 type name, ...}}`.
- 103 is the sign a friendly NPC shows (`{0 title keys, 1 text keys}`). Its title
  names the NPC when the nameplate is empty or an `@` working label.
- 7 field 0 holds the base stats, one float per KStatType ordinal (47 = the enum).
  MaxHealth is 0 on 1,486 of 1,620 NPCs and every damage stat is 0 outside a few
  dummies: health, damage and their level scaling are set by the server. What is
  here is emitted as written, minus the neutral 1.0 of the multiplier stats.
- 6 field 1 is the side: 1 on owned pets and class summons, 2 on NPC and Delve
  spawns (0 = none; a spawned prefab inherits its spawner's).
- 111 field 0 names the AI behaviour (`meleeBasic`, `immobile` ...; the exe's own
  behaviour names). 160 field 1 lists effect prefabs the NPC carries on itself
  (Dracocolatl's x5 health); 145 maps animation events to ability prefabs.
- 40 is the model: field 0 the skeleton, field 1 the part blueprints.
- Friendly services: 61 is the crafting component (field 14 tabs `{0 name key,
  1 recipe ids}`, 76 section 1 field 0 the window title), 268 section 1 fields 1-2
  name vendor stores (their stock is server-side), 411 section 1 field 1 offers
  adventures `{0 adventure id}`.

Boss is only what the game's own tags (106 field 0) say, and only where the tag's
meaning is shown by the game: `delveBoss` is what the Star Chart's "After defeating
a delve boss" node triggers on (the boss folder's minions do not carry it),
`worldboss` is the tag of the Geode Leviathans (`npc/worldboss/`, the
GeodeWorldBossesKilled metric) and of the dragons, T-Rexes and Flamotrons that
share it, and `shadowtitan` is excluded together with `delveBoss` where the game
keeps an effect off bosses (the Chloromancer shield). `elite` (the dungeon bosses, and every world and
Shadow Tower boss) is emitted as the tag it is: nothing client-side says what it
means. Loot is server-side too: the drop tags (`raredrop`, `*trophy`) are consumed
by no client file. Spawn locations are server-side; `group` is the NPC's folder.

Adventures come from `meta/activities/*`: rows `{0 id, 1 name key, 2 text key,
3 objective}`, where an `npckilled` objective lists the NPCs it counts. Placeable
items that put an NPC down (`placeable/npc/**`) name it in component 50.

Abilities are listed once, by ref, under "abilities" and described by
``ability.describe``; an NPC's damage is a multiplier on its server-set damage stat.
"""
from __future__ import annotations

from typing import Any

from app.trove.codexes.localize import resolve_stat_name
from app.trove.decode.ability import Prefabs, identity, text_keys, walk
from app.trove.decode.ability import refs as prefab_refs
from app.trove.decode.ally_abilities import _detail
from app.trove.decode.common import locale
from app.trove.decode.fields import stat_key
from app.trove.decode.tree import GameTree
from app.trove.decode.wire import Obj, Prefab, WireError, parse, strings

TITLE = "NPCs and bosses"
OUTPUT = "npcs.json"
PREFIXES = ("prefabs/npc/", "prefabs/placeable/npc/", "prefabs/abilities/", "prefabs/meta/activities/",
            "prefabs/sfx/", "languages/en/")
INDENT, FINAL_NEWLINE = None, True

ROOT = "npc/"
NPC_TYPE = 44
SIDE, STATS, NPC, PLACER, SIGN, TAGS, AI = 6, 7, 43, 50, 103, 106, 111
ANIM_EVENTS, CARRIED, CRAFTING, STATION, SHOP, ADVENTURES = 145, 160, 61, 76, 268, 411
NAME, ABILITY_LIST, DESCRIPTION, ICON = 4, 19, 24, 25
SIDES = {1: "players", 2: "enemies"}
BOSS_TAGS = {"delveBoss": "delve", "worldboss": "world", "shadowtitan": "shadow_tower"}
NEUTRAL_MULTIPLIERS = frozenset({11, 12, 24, 27, 45, 46})
NO_REQUIREMENT = frozenset({"none", "null", ""})


def _leaf(v: Any) -> dict:
    return v.leaf if isinstance(v, Obj) else {}


def _list(v: Any) -> list:
    if isinstance(v, dict):
        return list(v.values())
    return list(v) if isinstance(v, (list, tuple)) else []


def _rel(ref: str) -> str:
    return ref.removeprefix("prefabs/").removesuffix(".binfab")


class _Text:
    def __init__(self, tree: GameTree):
        self.table: dict[str, str] = {}
        for path in tree.files("languages/en/", ".binfab"):
            for k, v in locale(tree.read(path)).items():
                self.table.setdefault(k, v)

    def __call__(self, value: Any) -> str:
        if not isinstance(value, str) or not value:
            return ""
        if value.startswith("$"):
            return self.table.get(value, "")
        return value.removeprefix("@")


def _first(value: Any) -> str:
    return next((v for v in _list(value) if isinstance(v, str) and v), "")


def _adventures(tree: GameTree, text: _Text) -> tuple[dict[str, dict], dict[str, list[str]]]:
    """Adventure id -> {name, description}, and NPC ref -> the adventures counting its kills."""
    info: dict[str, dict] = {}
    counts: dict[str, list[str]] = {}

    def visit(v: Any) -> None:
        if isinstance(v, Obj):
            leaf = v.leaf
            aid, key = leaf.get(0), leaf.get(1)
            if isinstance(aid, str) and isinstance(key, str) and key.startswith("$") and isinstance(leaf.get(3), Obj):
                row = {"id": aid, "name": text(key)}
                if desc := text(leaf.get(2)):
                    row["description"] = desc
                info.setdefault(aid, row)
                objective = leaf[3]
                if objective and objective[0].get(0) == "npckilled":
                    body = objective[0].get(1)
                    own = body[-1] if isinstance(body, Obj) and len(body) > 1 else {}
                    for ref in _list(own.get(0)):
                        if isinstance(ref, str) and ref:
                            counts.setdefault(_rel(ref).lower(), []).append(aid)
            for sec in v:
                for x in sec.values():
                    visit(x)
        elif isinstance(v, (list, tuple, dict)):
            for x in _list(v):
                visit(x)

    for path in tree.files("prefabs/meta/activities/", ".binfab"):
        try:
            pf = parse(tree.read(path) or b"")
        except WireError:
            continue
        if pf.root is not None:
            visit(pf.root)
        for _, obj in pf.components:
            visit(obj)
    return info, counts


def _placers(tree: GameTree, text: _Text) -> dict[str, list[dict]]:
    """NPC ref -> the placeable items that put it down."""
    out: dict[str, list[dict]] = {}
    for path in tree.files("prefabs/placeable/npc/", ".binfab"):
        try:
            pf = parse(tree.read(path) or b"")
        except WireError:
            continue
        placer = pf.component(PLACER)
        rel = _rel(path)
        for ref in strings(placer) if placer is not None else []:
            if _rel(ref).startswith(ROOT):
                row = {"prefab": rel, "name": text(identity(pf).get("name_key"))}
                out.setdefault(_rel(ref).lower(), []).append(row)
    return out


def _stats(pf: Prefab) -> list[dict]:
    rows = []
    for ordinal, value in enumerate(_list(_leaf(pf.component(STATS)).get(0))):
        if not isinstance(value, (int, float)) or isinstance(value, bool) or value == 0:
            continue
        if ordinal in NEUTRAL_MULTIPLIERS and value == 1:
            continue
        key = stat_key(ordinal)
        rows.append({"stat": key, "name": resolve_stat_name({}, key), "value": round(float(value), 4)})
    return rows


def _model(pf: Prefab) -> dict:
    comp = _leaf(pf.component(40))
    skeleton = comp.get(0) if isinstance(comp.get(0), str) else ""
    parts = [p for p in (_leaf(r).get(0) for r in _list(comp.get(1))) if isinstance(p, str) and p]
    out: dict[str, Any] = {}
    if skeleton:
        out["skeleton"] = skeleton.removesuffix(".skeleton.gr2")
    if parts:
        out["blueprints"] = list(dict.fromkeys(parts))
    return out


def _ability_rows(pf: Prefab) -> list[dict]:
    rows: list[dict] = []
    for row in _list(_leaf(pf.component(NPC)).get(ABILITY_LIST)):
        leaf = _leaf(row)
        need = _leaf(_first_obj(leaf.get(4))).get(0)
        for ref in _list(leaf.get(0)):
            if isinstance(ref, str) and ref:
                entry: dict[str, Any] = {"ref": _rel(ref)}
                if isinstance(need, str) and need not in NO_REQUIREMENT:
                    entry["requires"] = need
                rows.append(entry)
    for row in _list(_leaf(pf.component(ANIM_EVENTS)).get(0)):
        leaf = _leaf(row)
        if isinstance(leaf.get(1), str) and leaf[1]:
            rows.append({"ref": _rel(leaf[1]), "event": leaf.get(0) if isinstance(leaf.get(0), str) else ""})
    out: list[dict] = []
    for r in rows:
        if r not in out:
            out.append(r)
    return out


def _first_obj(value: Any) -> Obj | None:
    if isinstance(value, Obj):
        return value
    return next((v for v in _list(value) if isinstance(v, Obj)), None)


def _ability(prefabs: Prefabs, ref: str, text: _Text) -> dict | None:
    pf = prefabs.get(ref)
    if pf is None:
        return None
    out: dict[str, Any] = {}
    keys = text_keys(pf)
    if title := text(keys.get("name_key")):
        out["name"] = title
    if desc := text(keys.get("description_key")):
        out["text"] = desc
    out.update(_detail(prefabs, ref, text.table))
    summons: list[str] = []
    for _, node, _ in walk(prefabs, ref):
        for s in prefab_refs(node, ROOT) + prefab_refs(node, "prefabs/" + ROOT):
            if _rel(s) not in summons:
                summons.append(_rel(s))
    if summons:
        out["summons"] = summons
    return out


def _services(pf: Prefab, text: _Text) -> dict:
    out: dict[str, Any] = {}
    craft = pf.component(CRAFTING)
    if craft is not None:
        tabs = []
        for tab in _list(craft.get(14)):
            leaf = _leaf(tab)
            recipes = [r for r in _list(leaf.get(1)) if isinstance(r, str) and r]
            if recipes:
                tabs.append({"name": text(leaf.get(0)), "recipes": recipes})
        station = pf.component(STATION)
        title = text(station[1].get(0)) if station is not None and len(station) > 1 else ""
        if tabs:
            out["crafting"] = {"title": title, "tabs": tabs} if title else {"tabs": tabs}
    shop = pf.component(SHOP)
    if shop is not None and len(shop) > 1:
        stores = [s for f in (1, 2) for s in _list(shop[1].get(f)) if isinstance(s, str) and s]
        if stores:
            out["stores"] = list(dict.fromkeys(stores))
    return out


def _offered(pf: Prefab) -> list[str]:
    comp = pf.component(ADVENTURES)
    rows = _list(comp[1].get(1)) if comp is not None and len(comp) > 1 else []
    return list(dict.fromkeys(a for a in (_leaf(r).get(0) for r in rows) if isinstance(a, str) and a))


def _entry(rel: str, pf: Prefab, text: _Text) -> dict | None:
    npc = _leaf(pf.component(NPC))
    sign = _leaf(pf.component(SIGN))
    plate = npc.get(NAME) if isinstance(npc.get(NAME), str) else ""
    title, sign_text = text(_first(sign.get(0))), text(_first(sign.get(1)))
    name = text(plate)
    if title and (not plate or plate.startswith("@")):
        name = title
    if not name:
        return None
    tags = {t for t in _list(_leaf(pf.component(TAGS)).get(0)) if isinstance(t, str)}
    folder = rel[len(ROOT):].rsplit("/", 1)[0] if "/" in rel[len(ROOT):] else ""
    entry: dict[str, Any] = {"slug": rel[len(ROOT):].replace("/", "_"), "name": name}
    if desc := text(npc.get(DESCRIPTION)):
        entry["description"] = desc
    entry["prefab"] = rel
    entry["group"] = "/".join(folder.split("/")[:2])
    if boss := next((kind for tag, kind in BOSS_TAGS.items() if tag in tags), None):
        entry["boss"] = boss
    if "elite" in tags:
        entry["elite"] = True
    side = _leaf(pf.component(SIDE)).get(1)
    if side in SIDES:
        entry["side"] = SIDES[side]
    ai = _leaf(_leaf(pf.component(AI)).get(0)).get(0)
    if isinstance(ai, str) and ai:
        entry["behavior"] = ai
    if model := _model(pf):
        entry["model"] = model
    if isinstance(npc.get(ICON), str) and npc[ICON]:
        entry["icon"] = npc[ICON]
    if title or sign_text:
        entry["sign"] = {k: v for k, v in (("title", title), ("text", sign_text)) if v}
    if stats := _stats(pf):
        entry["stats"] = stats
    return entry


def build(tree: GameTree) -> dict:
    prefabs = Prefabs(tree)
    text = _Text(tree)
    adventure_info, adventure_kills = _adventures(tree, text)
    placers = _placers(tree, text)

    npcs: list[dict] = []
    abilities: dict[str, dict] = {}
    skipped = 0
    for path in tree.files(f"prefabs/{ROOT}", ".binfab"):
        # Parsed here rather than through `prefabs`: 1,600 NPCs would sit in its cache.
        try:
            pf = parse(tree.read(path) or b"")
        except WireError:
            continue
        if pf.type_id != NPC_TYPE or pf.component(NPC) is None:
            continue
        rel = _rel(path)
        entry = _entry(rel, pf, text)
        if entry is None:
            skipped += 1
            continue
        rows = _ability_rows(pf)
        carried = [_rel(r) for r in _list(_leaf(pf.component(CARRIED)).get(1)) if isinstance(r, str) and r]
        for ref in [r["ref"] for r in rows] + carried:
            if ref not in abilities:
                abilities[ref] = _ability(prefabs, ref, text) or {}
        if rows := [r for r in rows if abilities.get(r["ref"])]:
            entry["abilities"] = rows
        if carried := [r for r in dict.fromkeys(carried) if abilities.get(r)]:
            entry["effects"] = carried
        entry.update(_services(pf, text))
        if offered := [adventure_info.get(a, {"id": a}) for a in _offered(pf)]:
            entry["offers_adventures"] = offered
        if counted := adventure_kills.get(rel.lower()):
            entry["defeat_adventures"] = [adventure_info[a] for a in dict.fromkeys(counted) if a in adventure_info]
        if placed := placers.get(rel.lower()):
            entry["placed_by"] = placed
        npcs.append(entry)

    npcs.sort(key=lambda e: (e["group"], e["name"].lower(), e["slug"]))
    groups: dict[str, dict[str, int]] = {}
    for e in npcs:
        g = groups.setdefault(e["group"], {"npcs": 0, "bosses": 0})
        g["npcs"] += 1
        g["bosses"] += 1 if "boss" in e else 0
    return {
        "groups": [{"id": k, **v} for k, v in sorted(groups.items())],
        "unnamed": skipped,
        "npcs": npcs,
        "abilities": {k: v for k, v in sorted(abilities.items()) if v},
    }


def count(data: dict) -> int:
    return len(data["npcs"])

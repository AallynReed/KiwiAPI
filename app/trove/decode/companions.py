"""companions.json - the Geode companions, what each level costs and what it grants.

The companions players can own are the members of
`prefabs/collections/collection_geodecompanion.binfab`: a list of groups (field 0
id, 1 `$CollectionName_*` key = the rarity word, 3 member rows whose field 0 is an
`item/companion/<slug>` prefab). The `sample` test companion is in no group.

A companion item prefab carries the identity component (name, description), the
UI blueprint (component 37, field 0), the creature it spawns (444, section 0
field 0: an `item/companion/npc/*` prefab, whose component 40 names its
`<rig>.skeleton.gr2`) and its upgrade tree id (440, field 0). The creature has no
abilities of its own: everything a companion grants comes from its levels.

`prefabs/upgrade/trees/*` map a tree id to its root node: {0 node key, 1 cost rows
({0 item, 1 quantity}), 3 child nodes}. Companion trees are one chain, root first,
so a node's depth is its level; the root costs nothing. Trees are found by id, not
by file name.

`prefabs/upgrade/upgrades/*` map a node key to what it grants: {0 kind, 1 body}.
The body's base section holds the name (0), description (1) and detail (4) keys;
the derived section's field 0 is stat modifier records for kind `stats`, ability
refs for kind `effects`. Nodes are found by key, not by file name (one file is
misspelt `hecbkug_zepperay_common`).

An `effects` ref is a periodic spawner whose chain ends in one of: a stat effect
(modifier records), an ability-value effect (component 453: rows of {0 value, 1 op,
2 amount} applied to the ability named in field 1 - `useCost` Add -5 is "-5
Barrier N-Charge Cost", `cooldown` Multiply 0.9 is "-10% Pathpainter Cooldown"), or
an energy restore (HealingParameters). Component 239 limits where it applies
({0 condition type, 1 its values}). What the chain does is reported beside the
node's own label, since a few labels do not match their chain.
"""
from __future__ import annotations

from typing import Any

from app.trove.decode import fields as F
from app.trove.decode.ability import Prefabs, healing_records, identity, modifiers, walk_values
from app.trove.decode.common import locale, stat_rows, stem
from app.trove.decode.tree import GameTree
from app.trove.decode.wire import Obj, WireError, parse

TITLE = "Companions"
OUTPUT = "companions.json"
PREFIXES = ("prefabs/collections/", "prefabs/item/", "prefabs/upgrade/", "prefabs/abilities/",
            "languages/en/")
INDENT, FINAL_NEWLINE = 4, False

COLLECTION = "collections/collection_geodecompanion"
BLUEPRINT, CREATURE, UPGRADE_TREE, MODEL = 37, 444, 440, 40
NODE_KEY, NODE_COST, NODE_CHILDREN = 0, 1, 3
GRANT_KIND, GRANT_BODY = 0, 1
GRANT_TEXT = {0: "name", 1: "description", 4: "detail"}
VALUE_EFFECT, VALUE_ROWS, VALUE_TARGET = 453, 0, 1
CONDITIONS = 239


def _leaf(v: Any) -> dict:
    return v.leaf if isinstance(v, Obj) else {}


def _keyed(tree: GameTree, prefix: str) -> dict[str, Obj]:
    """`key -> node` over every file under ``prefix`` (each is one stringmap)."""
    out: dict[str, Obj] = {}
    for path in tree.files(prefix, ".binfab"):
        try:
            pf = parse(tree.read(path) or b"")
        except WireError:
            continue
        table = _leaf(pf.root).get(0)
        for key, node in (table or {}).items() if isinstance(table, dict) else ():
            if isinstance(node, Obj):
                out.setdefault(key, node)
    return out


def _chain(root: Obj) -> list[Obj]:
    """A companion tree's nodes, root first; stops where the tree branches."""
    out, node = [root], root
    while True:
        kids = [c for c in node.get(NODE_CHILDREN) or [] if isinstance(c, Obj)]
        if len(kids) != 1:
            return out
        node = kids[0]
        out.append(node)


def _value_changes(obj: Obj, names: dict[str, str], prefabs: Prefabs) -> dict:
    leaf = obj.leaf
    target = leaf.get(VALUE_TARGET)
    rows = []
    for row in leaf.get(VALUE_ROWS) or []:
        r = _leaf(row)
        op = r.get(1)
        if isinstance(r.get(0), str) and isinstance(op, int) and 0 <= op < len(F.MOD_OPS):
            rows.append({"value": r[0], "op": F.MOD_OPS[op], "amount": round(float(r.get(2, 0)), 4)})
    out: dict[str, Any] = {}
    if isinstance(target, str) and target:
        out["ability"] = target.removesuffix(".binfab")
        title = names.get(identity(prefabs.get(target)).get("name_key", ""), "")
        if title:
            out["ability_name"] = title
    if rows:
        out["values"] = rows
    return out


def _conditions(obj: Obj) -> list[dict]:
    out = []
    for row in obj.get(0) or []:
        r = _leaf(row)
        values = _leaf(r.get(1)).get(0)
        if isinstance(r.get(0), str) and r[0] and isinstance(values, list):
            out.append({"type": r[0], "values": [v for v in values if v is not None]})
    return out


def _ability(prefabs: Prefabs, ref: str, names: dict[str, str]) -> dict:
    """What an upgrade's ability chain does, without entering the abilities it modifies."""
    effects: list[dict] = []
    conditions: list[dict] = []
    seen, queue = {ref}, [ref]
    while queue:
        pf = prefabs.get(queue.pop(0))
        if pf is None:
            continue
        targets = set()
        for cid, obj in pf.components:
            if cid == VALUE_EFFECT:
                change = _value_changes(obj, names, prefabs)
                targets.add(change.get("ability"))
                if change.get("values"):
                    effects.append(change)
            elif cid == CONDITIONS:
                conditions += [c for c in _conditions(obj) if c not in conditions]
            elif cid in F.EFFECTS:
                stats = modifiers(walk_values(obj))
                if stats:
                    effects.append({"stats": stats})
        for rec in healing_records(pf):
            effects.append({"restores": {k: v for k, v in rec.items() if k in ("health", "energy")}})
        for v in walk_values([o for _, o in pf.components]):
            if isinstance(v, str) and v.startswith("abilities/"):
                child = v.removesuffix(".binfab")
                if child not in seen and child not in targets:
                    seen.add(child)
                    queue.append(child)
    out: dict[str, Any] = {}
    if effects:
        out["effects"] = effects
    if conditions:
        out["conditions"] = conditions
    return out


def _grant(node: Obj | None, prefabs: Prefabs, names: dict[str, str]) -> dict:
    if node is None:
        return {}
    kind, body = node.get(GRANT_KIND), node.get(GRANT_BODY)
    if not isinstance(body, Obj) or not body:
        return {}
    out: dict[str, Any] = {}
    for idx, key in GRANT_TEXT.items():
        text = names.get(body[0].get(idx) or "", "")
        if text:
            out[key] = text
    payload = body.leaf.get(0) if len(body) > 1 else None
    if kind == "stats":
        stats = stat_rows(walk_values(payload))
        if stats:
            out["stats"] = stats
    elif kind == "effects":
        powers = []
        for ref in payload or []:
            if isinstance(ref, str) and ref.startswith("abilities/"):
                ref = ref.removesuffix(".binfab")
                powers.append({"ref": ref, **_ability(prefabs, ref, names)})
        if powers:
            out["abilities"] = powers
    return out


def _costs(node: Obj, prefabs: Prefabs, names: dict[str, str]) -> list[dict]:
    out = []
    for row in node.get(NODE_COST) or []:
        r = _leaf(row)
        item, qty = r.get(0), r.get(1)
        if isinstance(item, str) and isinstance(qty, int) and qty > 0:
            name = names.get(identity(prefabs.get(item)).get("name_key", ""), "")
            out.append({"item": item, **({"name": name} if name else {}), "quantity": qty})
    return out


def _rig(pf) -> str:
    model = pf.component(MODEL) if pf else None
    skeleton = model.get(0) if model is not None else None
    if isinstance(skeleton, str) and skeleton.endswith(".skeleton.gr2"):
        return skeleton.removesuffix(".skeleton.gr2")
    return ""


def _members(prefabs: Prefabs, names: dict[str, str]) -> list[tuple[str, str]]:
    """`(item/companion/<slug>, rarity word)` in collection order."""
    pf = prefabs.get(COLLECTION)
    out: list[tuple[str, str]] = []
    for group in _leaf(pf.root if pf else None).get(0) or []:
        g = _leaf(group)
        rarity = names.get(g.get(1) or "", "") or g.get(0) or ""
        for row in g.get(3) or []:
            ref = _leaf(row).get(0)
            if isinstance(ref, str) and ref.startswith("item/companion/"):
                out.append((ref.removesuffix(".binfab"), rarity))
    return out


def build(tree: GameTree) -> list[dict]:
    prefabs = Prefabs(tree)
    names: dict[str, str] = {}
    for path in tree.files("languages/en/", ".binfab"):
        for key, text in locale(tree.read(path)).items():
            names.setdefault(key, text)
    trees = _keyed(tree, "prefabs/upgrade/trees/")
    grants = _keyed(tree, "prefabs/upgrade/upgrades/")

    companions = []
    for rel, rarity in _members(prefabs, names):
        pf = prefabs.get(rel)
        if pf is None:
            continue
        ident = identity(pf)
        blueprint = _leaf(pf.component(BLUEPRINT)).get(0)
        creature = pf.component(CREATURE)
        npc = creature[0].get(0) if creature else None
        tree_id = _leaf(pf.component(UPGRADE_TREE)).get(0)
        entry: dict[str, Any] = {
            "slug": stem(rel), "name": names.get(ident.get("name_key", ""), "") or stem(rel),
            "description": names.get(ident.get("description_key", ""), ""),
            "prefab": rel, "rarity": rarity,
        }
        if isinstance(blueprint, str) and blueprint:
            entry["blueprint"] = blueprint
        if isinstance(npc, str) and npc:
            entry["creature"] = npc
            rig = _rig(prefabs.get(npc))
            if rig:
                entry["rig"] = rig
        levels = []
        root = trees.get(tree_id) if isinstance(tree_id, str) else None
        if root is not None:
            entry["upgrade_tree"] = tree_id
            for depth, node in enumerate(_chain(root), 1):
                key = node.get(NODE_KEY)
                level: dict[str, Any] = {"level": depth, "key": key}
                cost = _costs(node, prefabs, names)
                if cost:
                    level["cost"] = cost
                level.update(_grant(grants.get(key) if isinstance(key, str) else None, prefabs, names))
                levels.append(level)
        entry["levels"] = levels
        companions.append(entry)
    return companions


def count(data: list) -> int:
    return len(data)

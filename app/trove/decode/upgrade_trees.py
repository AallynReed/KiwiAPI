"""upgrade_trees.json - the Star Chart and the other upgrade trees players spend on.

Every tree here is built the way `companions.py` describes: `prefabs/upgrade/trees/*`
give each node's cost and children, `prefabs/upgrade/upgrades/*` its name,
description, detail text and what it grants, both keyed by node id.

- **Progression systems** are `prefabs/progression/<id>.binfab` (root): 0 id,
  3 name key, 14 description key, 15 detail key, 2 nodes `{0 node id, 2 cost to
  open (rows {0 item, 1 quantity}), 3 {1: second section 0 system, 1 parent node, 2 tree}, 9 map position [x, y]}`. A node's parent is the tree node listing it as a child, else that link's. The
  Star Chart is `standard`; the others are Depths of
  the Angler (`fishing`), the Rune Anvil (`runecrafting`) and Gearcrafting. A node's
  cost to activate is its tree node's cost.
- **Geode tool modules** are the trees `<module>_upgrade_tree` (barrier, battery,
  hookshot, jump_thruster, ncharge, omni_tool, pathpainter, vacuum, vision): one
  chain from a costless root, named by `$module_<module>_name` (omnitool drops its
  underscore).
- **Paragon** trees are `prestige_<class>`: roots that each offer two branch
  nodes, every branch an ability mod (`abilities/mods_01/<class>/...`) costing the
  class's `item/currency/prestige/<class>`. A branch can sit under two roots, so the
  powers are listed once each, in node-id order.

`upgrades/progression` has no text for any node and nothing names its trees, so it
is left out.
"""
from __future__ import annotations

from typing import Any

from app.trove.decode.ability import Prefabs, identity
from app.trove.decode.common import locale
from app.trove.decode.companions import _chain, _costs, _grant, _keyed, _leaf
from app.trove.decode.tree import GameTree
from app.trove.decode.wire import Obj, WireError, parse

TITLE = "Upgrade trees"
OUTPUT = "upgrade_trees.json"
PREFIXES = ("prefabs/progression/", "prefabs/upgrade/", "prefabs/item/", "prefabs/abilities/",
            "prefabs/class/", "languages/en/")
INDENT, FINAL_NEWLINE = 1, True

SYSTEMS = ("standard", "fishing", "runecrafting", "gearcrafting")
MODULES = ("omni_tool", "hookshot", "jump_thruster", "barrier", "pathpainter", "vision", "battery", "ncharge",
           "vacuum")
S_ID, S_NODES, S_NAME, S_DESC, S_DETAIL = 0, 2, 3, 14, 15
N_ID, N_OPEN, N_LINK, N_POS = 0, 2, 3, 9
NODE_KEY, NODE_CHILDREN = 0, 3


def _text(names: dict[str, str], key: Any) -> str:
    return names.get(key, "") if isinstance(key, str) else ""


def _tree_nodes(root: Obj, parents: dict[str, str] | None = None) -> dict[str, Obj]:
    """Every node under ``root`` by id; ``parents`` collects each child's parent."""
    out: dict[str, Obj] = {}
    queue = [root]
    while queue:
        node = queue.pop()
        key = node.get(NODE_KEY)
        if isinstance(key, str) and key not in out:
            out[key] = node
            for c in node.get(NODE_CHILDREN) or []:
                ck = c.get(NODE_KEY) if isinstance(c, Obj) else None
                if isinstance(ck, str):
                    queue.append(c)
                    if parents is not None:
                        parents.setdefault(ck, key)
    return out


def _node(key: str, grants: dict[str, Obj], prefabs: Prefabs, names: dict[str, str]) -> dict[str, Any]:
    g = _grant(grants.get(key), prefabs, names)
    out: dict[str, Any] = {"id": key}
    for field in ("name", "description", "detail", "stats"):
        if g.get(field):
            out[field] = g[field]
    return out


def _system(sid: str, prefabs: Prefabs, trees: dict[str, Obj], grants: dict[str, Obj],
            names: dict[str, str], tree: GameTree) -> dict | None:
    try:
        root = parse(tree.read(f"prefabs/progression/{sid}.binfab") or b"").root
    except WireError:
        return None
    if root is None:
        return None
    leaf = root.leaf
    costs: dict[str, Obj] = {}
    parents: dict[str, str] = {}
    for n in leaf.get(S_NODES) or []:
        key = _leaf(n).get(N_ID)
        if isinstance(key, str) and key in trees:
            costs.update(_tree_nodes(trees[key], parents))
    nodes, seen = [], set()
    for n in leaf.get(S_NODES) or []:
        nl = _leaf(n)
        key = nl.get(N_ID)
        if not isinstance(key, str) or key in seen:  # the Star Chart lists two stars twice
            continue
        seen.add(key)
        node = _node(key, grants, prefabs, names)
        link = _leaf(nl.get(N_LINK)).get(1)
        parent = parents.get(key) or (link[1].get(1) if isinstance(link, Obj) and len(link) > 1 else None)
        if isinstance(parent, str) and parent:
            node["parent"] = parent
        pos = nl.get(N_POS)
        if isinstance(pos, list) and len(pos) == 2:
            node["position"] = pos
        opens = _cost_rows(nl.get(N_OPEN), prefabs, names)
        if opens:
            node["opens_with"] = opens
        if key in costs:
            cost = _costs(costs[key], prefabs, names)
            if cost:
                node["cost"] = cost
        nodes.append(node)
    return {"slug": sid, "name": _text(names, leaf.get(S_NAME)) or sid.title(),
            "description": _text(names, leaf.get(S_DESC)), "detail": _text(names, leaf.get(S_DETAIL)),
            "nodes": nodes}


def _cost_rows(rows: Any, prefabs: Prefabs, names: dict[str, str]) -> list[dict]:
    out = []
    for row in rows or []:
        r = _leaf(row)
        item, qty = r.get(0), r.get(1)
        if isinstance(item, str) and isinstance(qty, int) and qty > 0:
            name = names.get(identity(prefabs.get(item)).get("name_key", ""), "")
            out.append({"item": item, **({"name": name} if name else {}), "quantity": qty})
    return out


def _module(mid: str, prefabs: Prefabs, trees: dict[str, Obj], grants: dict[str, Obj],
            names: dict[str, str]) -> dict | None:
    root = trees.get(f"{mid}_upgrade_tree")
    name = names.get(f"$module_{mid}_name", "") or names.get(f"$module_{mid.replace('_', '')}_name", "")
    if root is None or not name:
        return None
    levels = []
    for depth, node in enumerate(_chain(root)[1:], 2):
        key = node.get(NODE_KEY)
        if not isinstance(key, str):
            continue
        level = {"level": depth, **_node(key, grants, prefabs, names)}
        cost = _costs(node, prefabs, names)
        if cost:
            level["cost"] = cost
        levels.append(level)
    return {"slug": mid, "name": name, "levels": levels}


def _paragon(cls: str, prefabs: Prefabs, trees: dict[str, Obj], grants: dict[str, Obj],
             names: dict[str, str]) -> dict | None:
    roots = [n for k, n in sorted(trees.items()) if f"_{cls}_root_" in f"_{k}"]
    branches: dict[str, Obj] = {}
    for root in roots:
        for child in root.get(NODE_CHILDREN) or []:
            key = child.get(NODE_KEY) if isinstance(child, Obj) else None
            if isinstance(key, str):
                branches.setdefault(key, child)
    powers = []
    for key in sorted(branches):
        g = _grant(grants.get(key), prefabs, names)
        if not g.get("name"):
            continue
        power: dict[str, Any] = {"id": key, "name": g["name"]}
        if g.get("description"):
            power["description"] = g["description"]
        refs = [a["ref"] for a in g.get("abilities") or []]
        if refs:
            power["abilities"] = refs
        cost = _costs(branches[key], prefabs, names)
        if cost:
            power["cost"] = cost
        powers.append(power)
    return {"class": cls, "powers": powers} if powers else None


def build(tree: GameTree) -> dict:
    prefabs = Prefabs(tree)
    names: dict[str, str] = {}
    for path in tree.files("languages/en/", ".binfab"):
        for key, text in locale(tree.read(path)).items():
            names.setdefault(key, text)
    trees = _keyed(tree, "prefabs/upgrade/trees/")
    grants = _keyed(tree, "prefabs/upgrade/upgrades/")
    systems = [s for s in (_system(sid, prefabs, trees, grants, names, tree) for sid in SYSTEMS) if s]
    modules = [m for m in (_module(mid, prefabs, trees, grants, names) for mid in MODULES) if m]
    classes = sorted({p.rsplit("/", 1)[-1].removesuffix(".binfab").removeprefix("prestige_")
                      for p in tree.files("prefabs/upgrade/trees/", ".binfab") if "/prestige_" in p})
    paragon = [p for p in (_paragon(c, prefabs, trees, grants, names) for c in classes) if p]
    return {"systems": systems, "geode_tools": modules, "paragon": paragon}


def count(data: dict) -> int:
    return (sum(len(s["nodes"]) for s in data["systems"]) + sum(len(m["levels"]) for m in data["geode_tools"])
            + sum(len(p["powers"]) for p in data["paragon"]))

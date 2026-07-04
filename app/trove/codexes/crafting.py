"""Recursive crafting-tree resolution for the Recipe Cost Calculator (/codexes/crafting).

Recipes are indexed flat in ``codex_entry`` (keyed by their source prefab path),
but each *produces* an item at ``data.recipe.output.path`` and lists ingredients
by item path. To turn one recipe into a full dependency tree we invert that: build
an ``output_path -> recipe`` map for the branch once, then walk each ingredient,
expanding any ingredient that is itself the output of another recipe until we
bottom out at base materials. Cycles (Trove has a few) are broken by a per-branch
visited set; depth and node budgets bound pathological trees.

Prices come from the market scope. Each node whose item *name* has active listings
is annotated with the median price-each, and costs roll up from the leaves:

  - buy_cost   = median price-each * quantity needed (if the item is on the market)
  - craft_cost = sum of the children's best costs (only when EVERY descendant is
                 priced - one untracked base material makes it None so we never
                 silently understate; craft_cost_partial keeps the known-so-far sum)
  - best_cost  = min(buy, craft); recommendation = craft | buy | unknown

The market bot only tracks ~319 "interest" items, so most ingredients are
price-unknown - surfaced honestly rather than counted as free.
"""

from __future__ import annotations

import math

from app.core.config import settings
from app.trove.codexes import pg_store, read
from app.trove.market import service as market_service

# Tree bounds. Trove crafting chains are shallow in practice; these are guards
# against cycles the visited-set misses and against a combinatorial blow-up.
MAX_DEPTH = 20
MAX_NODES = 600

# Per-branch ``output_path -> recipe`` map, invalidated when the branch's codex
# meta signature (parser version + last index time) changes.
_MAP_CACHE: dict[str, tuple[tuple, dict]] = {}


async def _signature(branch: str) -> tuple:
    if not settings.postgres_enabled:
        return (0, None)
    return await pg_store.meta_signature(branch)


async def _recipe_map(branch: str) -> dict[str, dict]:
    """``output_path -> {source_path, name, category, output, ingredients}`` for a
    branch. Cached until the branch is re-indexed."""
    sig = await _signature(branch)
    cached = _MAP_CACHE.get(branch)
    if cached and cached[0] == sig:
        return cached[1]

    out_map: dict[str, dict] = {}
    for row in await read.all_recipes(branch):
        rec = (row.get("data") or {}).get("recipe") or {}
        out = rec.get("output")
        if not out or not out.get("path"):
            continue
        out_map[out["path"]] = {
            "source_path": row["path"],
            "name": row["name"],
            "category": row.get("category") or "",
            "output": {
                "path": out["path"],
                "name": out.get("name") or row["name"],
                "amount": int(out.get("amount") or 1),
            },
            "ingredients": [
                {"path": i["path"], "name": i.get("name") or i["path"],
                 "amount": int(i.get("amount") or 1)}
                for i in (rec.get("ingredients") or []) if i.get("path")
            ],
        }
    _MAP_CACHE[branch] = (sig, out_map)
    return out_map


def _raw(path: str, name: str, need: int, recipe_map: dict, depth: int,
         stack: frozenset, counter: list, names: set) -> dict:
    """Build the un-priced structure and collect every item name to price. ``need``
    is the quantity of this item the parent requires; craftable nodes scale their
    ingredients by how many crafts that implies."""
    counter[0] += 1
    names.add(name)
    node: dict = {
        "path": path, "name": name, "need": need,
        "craftable": False, "crafts": None, "output_amount": None, "children": [],
    }
    rec = recipe_map.get(path)
    if rec and depth < MAX_DEPTH and path not in stack and counter[0] < MAX_NODES:
        out_amt = rec["output"]["amount"] or 1
        crafts = max(1, math.ceil(need / out_amt))
        node["craftable"] = True
        node["crafts"] = crafts
        node["output_amount"] = out_amt
        deeper = stack | {path}
        for ing in rec["ingredients"]:
            node["children"].append(
                _raw(ing["path"], ing["name"], ing["amount"] * crafts,
                     recipe_map, depth + 1, deeper, counter, names)
            )
    return node


def _annotate(node: dict, prices: dict) -> dict:
    """Post-order cost annotation over a raw tree (pure - no I/O)."""
    price = prices.get(node["name"])
    unit = price["median_each"] if price else None
    node["market_price_each"] = unit
    node["market_count"] = price["count"] if price else 0
    node["buy_cost"] = round(unit * node["need"], 2) if unit is not None else None

    if node["craftable"] and node["children"]:
        known = 0.0
        all_known = True
        unpriced = 0
        for child in node["children"]:
            _annotate(child, prices)
            if child["best_cost"] is None:
                all_known = False
            else:
                known += child["best_cost"]
            unpriced += child["unpriced_count"]
        node["craft_cost_partial"] = round(known, 2)
        node["craft_cost"] = round(known, 2) if all_known else None
        node["unpriced_count"] = unpriced
    else:
        node["craft_cost_partial"] = None
        node["craft_cost"] = None
        node["unpriced_count"] = 0 if unit is not None else 1

    buy, craft = node["buy_cost"], node["craft_cost"]
    options = [x for x in (buy, craft) if x is not None]
    if options:
        node["best_cost"] = min(options)
        if craft is not None and (buy is None or craft <= buy):
            node["recommendation"] = "craft"
        else:
            node["recommendation"] = "buy"
    else:
        node["best_cost"] = None
        node["recommendation"] = "unknown"
    return node


async def build_tree(branch: str, recipe_path: str) -> dict | None:
    """Full priced crafting tree for the recipe at ``recipe_path``, or ``None`` if
    it isn't a known recipe. The root represents crafting one output batch
    (``output.amount`` units)."""
    root_entry = await read.get_entry(branch, "recipe", recipe_path)
    if not root_entry:
        return None
    rec = (root_entry.get("data") or {}).get("recipe") or {}
    out = rec.get("output")
    if not out or not out.get("path"):
        return None

    recipe_map = await _recipe_map(branch)
    counter = [0]
    names: set[str] = set()
    raw = _raw(out["path"], out.get("name") or root_entry["name"],
               int(out.get("amount") or 1), recipe_map, 0, frozenset(), counter, names)

    prices = await market_service.medians_for_names(sorted(names))
    tree = _annotate(raw, prices)

    return {
        "branch": branch,
        "recipe_path": recipe_path,
        "output": raw and {"path": out["path"],
                           "name": out.get("name") or root_entry["name"],
                           "amount": int(out.get("amount") or 1)},
        "category": root_entry.get("category") or "",
        "root": tree,
        "node_count": counter[0],
        "priced_items": len(prices),
        "truncated": counter[0] >= MAX_NODES,
    }

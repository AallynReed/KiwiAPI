"""gem_upgrades.json - gem stat rolls, level-up odds and costs, boosters and focuses.

`prefabs/gems/meta/gem_upgradedata.binfab` is one object whose field 0 maps an
upgrade-data key to a GemUpgradeData record. A gem item names its key in the gem
component (361, field 0): `small/opal_t12` -> 54, `large/opal_*_t112` -> 123. The
key is not derivable from the file name, so every key lists the items that use it.

GemUpgradeData:
  0  stats: {0 KStatType, 1 op (1 Add, 0 MultiplySum = a percent), 3 {0 min, 1 max, 2 step}}
  2  levels: map level -> {0 cost items, 1 Re-Gemerator count (cracking is gone, not read),
     3 double level-up chance, 4 level-up chance, 5 rewards (KGemLevelRewardType list),
     7 dust, 8 reward text key, 10 KUpgradeType (0 Dust, 1 Amber, 2 DustAndAmber), 11 amber}
  6  reselect-stat cost items
  8  {0 KGemTierUpgradeType (2 IncrementTier), 1 minimum level}, 9 the tier-up item

A level's entry holds two halves of different attempts: its cost and rewards are
paid and earned by the attempt that reaches it, while its chances are the odds of
the attempt made *from* it (checked in game 2026-09-25: a failed attempt's karma
tracks the current level's chance, and the cost shown is the next level's). Each
output row is one attempt, the one reaching `level`, with both halves joined.
Reward 3 is "Increase all stats a small fixed amount" (the `step` of every stat);
1 and 2 are the stat boosts at levels 5, 10 and 15. Boosters (component 367 of
`item/gem/booster/*`) multiply the two chances: field 2 the level-up chance, 1 the
double; a focus (type 3) adds field 5 to one stat's roll.

`gem_upgradematerialslist` pairs each gem colour with its dust and amber items
("Gem Spark" in game). Stat values are in display units: percents x100, Critical
Hit from tenths of a percent (as class_levels.py).
"""
from __future__ import annotations

import re
from collections import Counter

from app.trove.codexes.localize import resolve_stat_name
from app.trove.decode.ability import Prefabs, identity
from app.trove.decode.common import locale
from app.trove.decode.fields import stat_key
from app.trove.decode.tree import GameTree
from app.trove.decode.wire import Obj

TITLE = "Gem upgrades"
OUTPUT = "gem_upgrades.json"
PREFIXES = ("prefabs/gems/meta/", "prefabs/item/gem/", "prefabs/item/crafting/", "languages/en/")
INDENT, FINAL_NEWLINE = 1, True

GEM, STAT_STEP, BOOST_REWARDS = 361, 3, (1, 2)
BOOSTER, BOOSTER_TYPE, FOCUS_TYPE = 367, 2, 3
UPGRADE_DUST, UPGRADE_AMBER, UPGRADE_BOTH = 0, 1, 2
INCREMENT_TIER = 2
SCALE = {"CriticalHitChance": 0.1}
ITEM = re.compile(r"^prefabs/item/gem/(small|large)/(\w+?)_(?:.*_)?t(\d+)(_concat\d+)?\.binfab$")


def _leaf(v) -> dict:
    return v.leaf if isinstance(v, Obj) else {}


def _objs(v) -> list[dict]:
    return [o.leaf for o in (v or []) if isinstance(o, Obj)] if isinstance(v, list) else []


def _items(v) -> list[dict]:
    return [{"item": r[0], "count": int(r.get(1, 0))}
            for r in _objs(v) if isinstance(r.get(0), str) and r.get(1)]


def _stat(sid: int, add: int) -> tuple[str, bool]:
    return stat_key(sid).removeprefix("$Stat_"), add == 0


def _value(stat: str, percent: bool, raw: float) -> float:
    return round(float(raw) * (100 if percent else SCALE.get(stat, 1)), 4)


def _pool(rows) -> list[dict]:
    out = []
    for r in _objs(rows):
        stat, percent = _stat(r.get(0, 0), r.get(1, 1))
        out.append({"stat": stat, "percent": percent, "weight": float(r.get(2, 0))})
    return out


def _gem_items(tree: GameTree, prefabs: Prefabs) -> tuple[dict[int, list], dict[int, list]]:
    """key -> the gem items that use it, and key -> the stat pool of its plain gems.

    A plain gem is a small gem without `_concat` (fixed-stat variants) or a large gem
    that is not a class gem (361 field 2 is the class; -1 for none)."""
    items: dict[int, set] = {}
    pools: dict[int, Counter] = {}
    for path in tree.files("prefabs/item/gem/", ".binfab"):
        m = ITEM.match(path.lower())
        pf = prefabs.get(path.removeprefix("prefabs/")) if m else None
        comp = _leaf(pf.component(GEM)) if pf else {}
        key = comp.get(0)
        if m is None or not isinstance(key, int):
            continue
        size, color, tier, concat = m.groups()
        items.setdefault(key, set()).add((size, color, int(tier)))
        if concat or comp.get(2, -1) != -1:
            continue
        pool = tuple((r["stat"], r["percent"], r["weight"]) for r in _pool(_leaf(comp.get(3)).get(1)))
        pools.setdefault(key, Counter())[pool] += 1
    out_items = {k: [{"size": s, "color": c, "tier": t} for s, c, t in sorted(v)] for k, v in items.items()}
    out_pools = {k: [{"stat": s, "percent": p, "weight": w} for s, p, w in c.most_common(1)[0][0]]
                 for k, c in pools.items()}
    return out_items, out_pools


def _levels(levels: dict) -> list[dict]:
    out = []
    for level in sorted(k for k, v in levels.items() if isinstance(k, int) and isinstance(v, Obj)):
        if level < 2:
            continue
        e = levels[level].leaf
        odds = _leaf(levels.get(level - 1))
        rewards = [r for r in (e.get(5) or []) if isinstance(r, int)]
        kind = e.get(10, UPGRADE_DUST)
        out.append({
            "level": level,
            "chance": round(float(odds.get(4, 0)), 6),
            "double_chance": round(float(odds.get(3, 0)), 6),
            "stat_steps": rewards.count(STAT_STEP),
            "boost": any(r in BOOST_REWARDS for r in rewards),
            "cost": _items(e.get(0)),
            "dust": int(e.get(7, 0)) if kind in (UPGRADE_DUST, UPGRADE_BOTH) else 0,
            "amber": int(e.get(11, 0)) if kind in (UPGRADE_AMBER, UPGRADE_BOTH) else 0,
        })
    return out


def _gem(key: int, rec: dict, items: list, pool: list | None) -> dict:
    stats = []
    for r in _objs(rec.get(0)):
        stat, percent = _stat(r.get(0, 0), r.get(1, 1))
        rng = _leaf(r.get(3))
        stats.append({"stat": stat, "name": resolve_stat_name({}, f"$Stat_{stat}"), "percent": percent,
                      **{k: _value(stat, percent, rng.get(i, 0)) for i, k in enumerate(("min", "max", "step"))}})
    levels = _levels(rec.get(2) or {})
    tier_up = _leaf(rec.get(8))
    up_item = _items(rec.get(9))
    return {
        "key": key,
        "items": items,
        "max_level": max((lv["level"] for lv in levels), default=1),
        "stats": stats,
        "pool": pool or [],
        "levels": levels,
        "reselect_cost": _items(rec.get(6)),
        "tier_upgrade": ({"level": tier_up.get(1), "item": up_item[0]["item"]}
                         if tier_up.get(0) == INCREMENT_TIER and up_item else None),
    }


def _boosters(tree: GameTree, prefabs: Prefabs) -> tuple[list, list]:
    """Boosters the upgrade screen takes (its slot list, `gem_insurancedata`) and focuses."""
    slots = prefabs.get("gems/meta/gem_insurancedata")
    usable = {_leaf(r).get(0) for r in ((slots.root.get(0) if slots and slots.root is not None else None) or [])}
    boosters, focuses = [], []
    for path in tree.files("prefabs/item/gem/booster/", ".binfab"):
        rel = path.removeprefix("prefabs/").removesuffix(".binfab")
        pf = prefabs.get(rel)
        b = _leaf(pf.component(BOOSTER)) if pf else {}
        if b.get(4) == FOCUS_TYPE and b.get(5):
            focuses.append({"item": rel, "amount": round(float(b[5]) * 100, 4)})
        elif b.get(4) == BOOSTER_TYPE and rel in usable:
            boosters.append({"item": rel, "chance_multiplier": float(b.get(2, 1)),
                             "double_multiplier": float(b.get(1, 1))})
    return boosters, focuses


def build(tree: GameTree) -> dict:
    prefabs = Prefabs(tree)
    root = prefabs.get("gems/meta/gem_upgradedata")
    table = (root.root.get(0) if root and root.root is not None else None) or {}
    items, pools = _gem_items(tree, prefabs)
    gems = [_gem(k, v.leaf, items.get(k, []), pools.get(k))
            for k, v in sorted(table.items()) if isinstance(v, Obj)]

    mats = prefabs.get("gems/meta/gem_upgradematerialslist")
    materials = {}
    for row in (mats.root.leaf.values() if mats and mats.root is not None else []):
        dust, amber = _leaf(row).get(0), _leaf(row).get(1)
        m = re.match(r"item/crafting/gemdust_(\w+)$", dust or "")
        if m and amber:
            materials[m.group(1)] = {"dust": dust, "amber": amber}

    boosters, focuses = _boosters(tree, prefabs)

    text: dict[str, str] = {}
    for path in tree.files("languages/en/", ".binfab"):
        for k, v in locale(tree.read(path)).items():
            text.setdefault(k, v)
    names = {}
    wanted = {i["item"] for g in gems for lv in g["levels"] for i in lv["cost"]}
    wanted |= {i["item"] for g in gems for i in g["reselect_cost"]}
    wanted |= {g["tier_upgrade"]["item"] for g in gems if g["tier_upgrade"]}
    wanted |= {v for m in materials.values() for v in m.values()}
    wanted |= {b["item"] for b in boosters} | {f["item"] for f in focuses}
    for item in sorted(wanted):
        key = identity(prefabs.get(item)).get("name_key")
        if key and text.get(key):
            names[item] = text[key]
    return {"gems": gems, "materials": materials, "boosters": boosters,
            "focuses": focuses, "names": names}


def count(data: dict) -> int:
    return len(data["gems"])

"""gear.json - what the client knows about equipment, for the /gear-guide page.

Rarities (names, required level, Power Rank, forge costs), the forge rules (star
levels, Pearls of Wisdom, the stations and what each one costs), the stat lines the
files fix on basic and named items, and every banner and torch with its full stats.

Not here because the client never has it: the min/max of a rolled Hat/Face/Weapon/Ring
line. An item arrives from the server with its rolls already made (StatRollInfo 8/9);
every prefab template leaves them at 0. ``server_side`` says so in the output.

Sources: KItemRarity order (exe), ``$Rarity_*`` locale, ``meta/powerrank`` (keyed by
rarity id + 1), ``crafting/upgrade`` (improve / raise costs), component 146 on
``placeable/crafting/*`` (stations), component 128 (stat-line templates), component 65
field 1 and component 351 (banner stats and tiers). ``EXE`` holds the few rules that
live only in Trove_x64.exe.
"""
from __future__ import annotations

import re
from typing import Any

from app.trove.codexes.bonuses import STAT_KEYS
from app.trove.decode.ability import Prefabs, identity
from app.trove.decode.common import locale
from app.trove.decode.fields import MOD_OPS
from app.trove.decode.tree import GameTree
from app.trove.decode.wire import Obj

TITLE = "Gear"
OUTPUT = "gear.json"
PREFIXES = ("prefabs/equipment/", "prefabs/item/banner/", "prefabs/item/crafting/",
            "prefabs/crafting/", "prefabs/meta/", "prefabs/placeable/crafting/", "languages/en/")
INDENT, FINAL_NEWLINE = 1, True

IDENTITY, SLOT_COMP, TEMPLATE, BANNER_TIERS, STATION = 49, 65, 128, 351, 146

RARITIES = ("Common", "Uncommon", "Rare", "Epic", "Legendary", "Relic", "Resplendent",
            "Shadow1", "Shadow2", "Shadow3", "Shadow4", "Shadow5",
            "Radiant1", "Stellar1", "Radiant2", "Stellar2", "Radiant3", "Stellar3",
            "Radiant4", "Stellar4", "Radiant5", "Stellar5",
            "Crystal1", "Crystal2", "Crystal3", "Crystal4", "Crystal5",
            "Mystic1", "Mystic2", "Mystic3", "Mystic4", "Mystic5")
SLOTS = {0: ("Weapon", "$EquipmentSlot_Weapon"), 4: ("Hat", "$EquipmentSlot_Hat"),
         5: ("Face", "$EquipmentSlot_Face"), 6: ("Ring", "$EquipmentSlot_Ring"),
         33: ("Banner", "$EquipmentSlot_PVPBanner")}
ITEM_TYPES = {4: (0, "Gun", "$ItemType_Gun"), 5: (0, "Bow", "$ItemType_Bow"),
              6: (0, "Staff", "$ItemType_Staff"), 8: (0, "Spear", "$ItemType_Spear"),
              9: (0, "Fist", "$ItemType_Fist"), 14: (0, "Melee", "$ItemType_Melee"),
              12: (4, "Hat", "$ItemType_Hat"), 13: (5, "Face", "$ItemType_Face"),
              15: (6, "Ring_MaxHealth", "$Stat_MaxHealth"), 16: (6, "Ring_Lasermancy", "$Stat_Mining"),
              17: (6, "Ring_PhysicalDamage", "$Stat_PhysicalDamage"), 18: (6, "Ring_MagicDamage", "$Stat_SpellDamage"),
              20: (33, "Banner", "$ItemType_Banner")}
UPGRADE_TYPES = ("AddStat", "AddQuality", "ReduceMinimumLevel", "IncreaseRarity", "PickNewStats",
                 "AddStatBonus", "PickNewParticles", "AddGemQuality", "RepairGem", "AddGemStatBonus",
                 "ReselectGemStats", "MoveGemUpgrade", "UpgradeTreeBonus", "UpgradeGemTier")
UPGRADE_LABEL = {0: "$ItemUpgrade_AddStat", 1: "$ItemUpgrade_AddQuality", 2: "$ItemUpgrade_ReduceMinLevel",
                 3: "$ItemUpgrade_IncreaseRarity", 4: "$ItemUpgrade_NewStats", 5: "$ItemUpgrade_AddStatBonus",
                 6: "$ItemUpgrade_NewParticles"}
GEAR_UPGRADES = frozenset(range(7))   # the rest are gem / companion / upgrade-tree operations
PVE = 1   # KModFlags
EXTRA_STATS = {45: "$Stat_HealDoneMultiplier", 46: "$Stat_HealReceiveMultiplier"}

# Rules compiled into Trove_x64.exe (2026-10-01 build), not in any prefab.
EXE = {
    "star_step": 0.2,                    # value(L) = min + L * 0.2 * (max - min)
    "max_star": 5,
    "max_lines": 3,                      # a Pearl adds a line while an item has fewer
    "max_boosts_per_line": 2,
    "pearl_multipliers": [1.0, 1.1, 1.3],
    "chaos_min_lines": 3,
    "tome_max_reduction": 10,
    "pr_per_stat_boost": 0.0375,
    "pr_per_star": 0.06,
}
_REQUIRED_LEVEL = [0, 0, 4, 6, 8] + [10] * 17 + [20] * 5 + [30] * 5


def _leaf(v: Any) -> dict:
    return v.leaf if isinstance(v, Obj) else {}


def _rows(v: Any) -> list[dict]:
    return [r.leaf for r in v or () if isinstance(r, Obj)]


def _family(key: str) -> tuple[str, int | None]:
    m = re.match(r"([A-Za-z]+?)(\d)$", key)
    return (m.group(1), int(m.group(2))) if m else (key, None)


def _stat_key(sid: int) -> str:
    return STAT_KEYS.get(sid) or EXTRA_STATS.get(sid) or f"$Stat_{sid}"


def _stat_name(sid: int, loc: dict[str, str]) -> str:
    key = _stat_key(sid)
    return loc.get(key) or loc.get(key.removesuffix("_controller")) or key


def power_rank(base: int, star: int, boosts: int) -> int:
    """Gear Power Rank at a star level with ``boosts`` pearl stat boosts in total."""
    p = base * (1 + EXE["pr_per_stat_boost"] * boosts)
    return int(max(p * (1 + EXE["pr_per_star"] * star), p + star) + 0.5)


def _en(tree: GameTree) -> dict[str, str]:
    out: dict[str, str] = {}
    for path in tree.files("languages/en/", ".binfab"):
        for k, v in locale(tree.read(path)).items():
            out.setdefault(k, v)
    return out


def _name(prefabs: Prefabs, ref: str, loc: dict[str, str]) -> str:
    key = identity(prefabs.get(ref)).get("name_key", "")
    return loc.get(key) or ref.rsplit("/", 1)[-1]


def _costs(v: Any, prefabs: Prefabs, loc: dict[str, str]) -> list[dict]:
    return [{"item": c[0], "name": _name(prefabs, c[0], loc), "count": c.get(1, 1)}
            for c in _rows(_leaf(v).get(0)) if isinstance(c.get(0), str)]


def _mods(rows: Any, loc: dict[str, str]) -> list[dict]:
    out = []
    for r in _rows(rows):
        sid, op, val = r.get(0, 0), r.get(1, 0), r.get(2)
        if not isinstance(val, (int, float)) or not 0 <= op < len(MOD_OPS):
            continue
        percent = op in (0, 4)
        row = {"name": _stat_name(sid, loc), "value": round(float(val) * 100 if percent else float(val), 4)}
        if percent:
            row["percent"] = True
        if r.get(5, 0) & PVE:
            row["pve"] = True
        out.append(row)
    return out


def _rarities(prefabs: Prefabs, loc: dict[str, str]) -> tuple[list[dict], str | None]:
    pr_pf = prefabs.get("meta/powerrank")
    pr = {r[0]: r[1] for r in _rows(_leaf(pr_pf.root).get(0) if pr_pf else None)
          if isinstance(r.get(0), int) and isinstance(r.get(1), int)}
    up_pf = prefabs.get("crafting/upgrade")
    up_root = _leaf(up_pf.root) if up_pf else {}
    upgrade = {r[0]: r.get(1) or {} for r in _rows(up_root.get(0))}
    raise_from = up_root.get(1)
    out = []
    for rid, key in enumerate(RARITIES):
        family, level = _family(key)
        base = loc.get(f"$Rarity_{family}", family)
        costs = upgrade.get(rid, {})
        improve = {str(k): _costs(v, prefabs, loc) for k, v in sorted(costs.items()) if k}
        raise_cost = _costs(costs.get(0), prefabs, loc) if isinstance(raise_from, int) and rid == raise_from + 1 else []
        out.append({
            "id": rid, "key": key, "family": family, "level": level,
            "name": f"{base} {level}" if level else base,
            "required_level": _REQUIRED_LEVEL[rid],
            "power_rank": pr.get(rid + 1),
            "aura": {"weapon": rid >= 12, "hat": rid >= 13},
            "improve_cost": improve or None,
            "raise_cost": raise_cost or None,
        })
    return out, RARITIES[raise_from] if isinstance(raise_from, int) else None


def _stations(tree: GameTree, prefabs: Prefabs, loc: dict[str, str]) -> list[dict]:
    out = []
    for path in tree.files("prefabs/placeable/crafting/", "_interactive.binfab"):
        rel = path.removeprefix("prefabs/").removesuffix(".binfab")
        pf = prefabs.get(rel)
        comp = pf.component(STATION) if pf else None
        if comp is None or len(comp) < 2:
            continue
        ops = []
        for i, row in enumerate(_rows(comp[1].get(1))):
            if row.get(2) and i in GEAR_UPGRADES:
                ops.append({"key": UPGRADE_TYPES[i], "label": loc.get(UPGRADE_LABEL[i], UPGRADE_TYPES[i]),
                            "cost": _costs(Obj([row]), prefabs, loc) or None})
        if ops:
            out.append({"prefab": rel, "name": _name(prefabs, rel.removesuffix("_interactive"), loc), "operations": ops})
    return out


def build(tree: GameTree) -> dict:
    loc = _en(tree)
    prefabs = Prefabs(tree)
    rarities, raise_from = _rarities(prefabs, loc)

    named: list[dict] = []
    banners: dict[str, dict] = {}   # one row per stat profile; cosmetic variants share it
    roots = ("prefabs/equipment/", "prefabs/item/banner/")
    for path in [p for r in roots for p in tree.files(r, ".binfab")]:
        rel = path.removeprefix("prefabs/").removesuffix(".binfab")
        pf = prefabs.get(rel)
        if pf is None:
            continue
        ident = _leaf(pf.component(IDENTITY))
        slot_c = _leaf(pf.component(SLOT_COMP))
        slot = slot_c.get(0)
        if slot not in SLOTS:
            continue
        rid = ident.get(9)
        for i in (17, 20):
            if isinstance(ident.get(i), int) and ident[i] != -1:
                rid = ident[i]
        rarity = RARITIES[rid] if isinstance(rid, int) and 0 <= rid < len(RARITIES) else None
        itype = ident.get(10)
        name_key = ident.get(1, "")
        name = loc.get(name_key) or None

        tmpl = pf.component(TEMPLATE)
        if tmpl is not None and itype in ITEM_TYPES:
            stats = [_stat_name(s.get(0, 0), loc) + (" %" if s.get(3) == 0 else "") for s in _rows(tmpl.leaf.get(8))]
            if rarity != "Common" and stats and name:
                named.append({"name": name, "prefab": rel, "slot": SLOTS[slot][0],
                              "item_type": ITEM_TYPES[itype][1], "rarity": rarity, "stats": stats})

        if slot == 33:
            fixed = _mods(slot_c.get(1), loc)
            tiers_c = _leaf(pf.component(BANNER_TIERS))
            tiers = [{"stats": _mods(_leaf(g).get(0), loc)} for _, g in sorted((tiers_c.get(1) or {}).items())]
            tiers = [t for t in tiers if t["stats"]]
            if not (fixed or tiers) or not name:
                continue
            every = fixed + [s for t in tiers for s in t["stats"]]
            pve = [s.pop("pve", False) for s in every]   # pop every row, so no short-circuit
            pvp = not any(pve)
            profile = banners.setdefault(repr((pvp, fixed, tiers)),
                                         {"pvp": pvp, "stats": fixed, "tiers": tiers, "items": []})
            if all(i["name"] != name or i["rarity"] != rarity for i in profile["items"]):
                desc = ident.get(5, "")
                profile["items"].append({"name": name, "rarity": rarity, "prefab": rel,
                                         "description": loc.get(desc) if desc else None})

    slots = [{"key": key, "name": loc.get(label, key),
              "item_types": [{"key": tkey, "name": loc.get(tlabel, tkey)}
                             for tslot, tkey, tlabel in ITEM_TYPES.values() if tslot == sid]}
             for sid, (key, label) in SLOTS.items()]

    named.sort(key=lambda n: (RARITIES.index(n["rarity"]) if n["rarity"] else -1, n["slot"], n["name"]))
    def best(profile: dict) -> int:
        return max((RARITIES.index(i["rarity"]) for i in profile["items"] if i["rarity"]), default=-1)

    profiles = sorted(banners.values(), key=lambda b: (b["pvp"], -best(b), b["items"][0]["name"]))
    for profile in profiles:
        profile["items"].sort(key=lambda i: i["name"])
    return {
        "server_side": [
            "The value range of every rolled Hat, Face, Weapon and Ring stat line",
            "Which bonus stats a dropped or crafted item rolls, and how often",
        ],
        "rarities": rarities,
        "forge": {**{k: v for k, v in EXE.items() if not k.startswith("pr_")},
                  "raise_from": raise_from,
                  "raise_to": RARITIES[RARITIES.index(raise_from) + 1] if raise_from else None,
                  "stations": _stations(tree, prefabs, loc)},
        "power_rank": {"per_stat_boost": EXE["pr_per_stat_boost"], "per_star": EXE["pr_per_star"]},
        "slots": slots,
        "named_items": named,
        "banners": profiles,
    }


def count(data: dict) -> int:
    return len(data.get("rarities", [])) + len(data.get("banners", []))

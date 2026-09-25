"""pvp_powerups.json - Bomber Royale's weapon, bomb and mobility swaps (with their
upgrades) and the PvP arena consumables.

Each is an ability prefab under `prefabs/pvp/battleroyale/` (`Upgrade/` holds the
`_t2`..`_t4` tiers) or `prefabs/pvp/pvp_consumable_*`, with the identity component
(name, description, 6 icon path) and an action ``ability.describe`` walks.
"""
from __future__ import annotations

import re
from typing import Any

from app.trove.decode.ability import Prefabs, describe, identity
from app.trove.decode.common import locale
from app.trove.decode.tree import GameTree

TITLE = "PvP power-ups"
OUTPUT = "pvp_powerups.json"
PREFIXES = ("prefabs/pvp/", "prefabs/abilities/", "prefabs/effects/", "prefabs/sfx/", "languages/en/")
INDENT, FINAL_NEWLINE = 1, True

GROUPS = {"weaponswap": "Weapons", "bombswap": "Bombs", "mobility": "Mobility", "terraswap": "Terrain",
          "consumable": "Consumables"}
TIER = re.compile(r"_t(\d)$")


def _detail(prefabs: Prefabs, rel: str, names: dict[str, str]) -> dict:
    info = describe(prefabs, rel, prefix=("abilities/", "pvp/", "effects/"))
    out: dict[str, Any] = {}
    for key in ("energy", "cooldown", "proc_cooldown"):
        if info.get(key):
            out[key] = info[key]
    for key in ("stages", "healing"):
        rows = [{k: v for k, v in r.items() if k != "_depth"} for r in info[key]]
        if rows:
            out[key] = rows
    effects = []
    for eff in info["effects"]:
        row = {k: v for k, v in eff.items() if k not in ("_depth", "name_key", "description_key")}
        title = names.get(eff.get("name_key", ""), "")
        if title:
            row["title"] = title
        effects.append(row)
    if effects:
        out["effects"] = effects
    return out


def build(tree: GameTree) -> dict:
    prefabs = Prefabs(tree)
    names: dict[str, str] = {}
    for path in tree.files("languages/en/", ".binfab"):
        for key, text in locale(tree.read(path)).items():
            names.setdefault(key, text)
    rows = []
    for path in tree.files("prefabs/pvp/", ".binfab"):
        rel = path[len("prefabs/"):-len(".binfab")]
        stem = rel.rsplit("/", 1)[-1]
        mode = "battleroyale" if "/battleroyale/" in rel else "arena" if stem.startswith("pvp_consumable_") else ""
        if not mode:
            continue
        ident = identity(prefabs.get(rel))
        name = names.get(ident.get("name_key", ""), "")
        if not name:
            continue
        kind = next((g for g in GROUPS if f"_{g}_" in f"_{stem}_"), "consumable")
        tier = TIER.search(stem)
        row: dict[str, Any] = {"slug": stem, "name": name, "description": names.get(ident.get("description_key", ""), ""),
                               "prefab": rel, "mode": mode, "group": GROUPS[kind],
                               "base": TIER.sub("", stem), "tier": int(tier.group(1)) if tier else 1}
        if ident.get("icon"):
            row["icon"] = ident["icon"]
        row.update(_detail(prefabs, rel, names))
        rows.append(row)
    rows.sort(key=lambda r: (r["mode"], r["group"], r["base"], r["tier"]))
    return {"powerups": rows}


def count(data: dict) -> int:
    return len(data["powerups"])

"""gem_abilities.json - empowered gems and the numbers behind their abilities.

Empowered gems are the ones that carry an ability, and the game names them by
file: `prefabs/item/gem/large/<element>_<slug>_t<tier>.binfab`. The item prefab
names the ability prefabs it grants (component 149), and ``ability.describe``
reads each chain structurally: damage, healing, effect durations and stat changes.

Many gem abilities are swaps that point at the class ability they replace as well
as the replacement, so the walk stops at every prefab a class prefab names - a gem
reports what it adds, not the base kit's numbers.

Small gems carry no ability - they are `blue_t1`, `blue_t10_concat1` and so on,
with no slug - which is why only `large/` is walked.

Gem STAT ROLL values (gamedata/mystic.json) are deliberately not touched: they
are not in the prefab tree. `gems/meta/gem_upgradedata.binfab` holds upgrade
costs (flux, gemrepair, warpseed) and the item prefabs hold no stat records, so
the per-tier roll ranges are computed by the game and cannot be read from here.
"""
from __future__ import annotations

import re

from app.trove.decode.ability import Prefabs, describe, refs
from app.trove.decode.common import locale, stem
from app.trove.decode.tree import GameTree
from app.trove.decode.wire import strings

TITLE = "Gem abilities"
OUTPUT = "gem_abilities.json"
PREFIXES = ("prefabs/item/gem/large/", "prefabs/abilities/", "prefabs/class/", "prefabs/sfx/",
            "languages/en/")
INDENT, FINAL_NEWLINE = 4, False

# The file prefix is the gem's colour; these are the names the game's own
# descriptions use ("Empowered Water Gem for the Ice Sage.").
ELEMENTS = {"blue": "Water", "red": "Fire", "yellow": "Air",
            "opal": "Cosmic", "prismatic": "Prismatic"}

_ITEM = re.compile(r"^([a-z]+)_(.+)_t(\d+)$")


def class_kit(prefabs: Prefabs, tree: GameTree) -> set[str]:
    """Every ability a class prefab names directly - the base kit a swap replaces."""
    kit: set[str] = set()
    for path in tree.files("prefabs/class/", ".binfab"):
        pf = prefabs.get(path[len("prefabs/"):-len(".binfab")])
        if pf is not None and pf.root is not None:
            kit.update(s for s in strings(pf.root) if s.startswith("abilities/"))
    return kit


def merge_rows(entry: dict, info: dict) -> None:
    for key in ("stages", "healing", "effects"):
        for row in info[key]:
            row = {k: v for k, v in row.items() if k != "_depth"}
            if row not in entry[key]:
                entry[key].append(row)
    for eff in info["effects"]:
        for stat in eff.get("stats", []):
            row = {**stat, "prefab": eff["prefab"]}
            if row not in entry["stats"]:
                entry["stats"].append(row)
    for v in info["vfx"]:
        if v not in entry["vfx"]:
            entry["vfx"].append(v)
    for act in info["actions"]:
        row = {k: v for k, v in act.items() if k != "_depth"}
        if row not in entry["actions"]:
            entry["actions"].append(row)
    entry["prefabs"] = sorted(set(entry["prefabs"]) | set(info["prefabs"]))
    for key in ("energy", "cooldown", "proc_cooldown"):
        if info.get(key) and key not in entry:
            entry[key] = info[key]


def build(tree: GameTree) -> list[dict]:
    prefabs = Prefabs(tree)
    kit = class_kit(prefabs, tree)
    aloc = locale(tree.read("languages/en/prefabs_item_gem_large.binfab"))

    gems: dict[tuple[str, str], dict] = {}
    for path in tree.files("prefabs/item/gem/large/", ".binfab"):
        name = stem(path)
        match = _ITEM.match(name)
        if path.count("/") != 4 or not match:
            continue
        colour, slug, tier = match.groups()
        entry = gems.setdefault((colour, slug), {
            "element": ELEMENTS.get(colour, colour), "slug": slug,
            "name": "", "description": "", "tiers": [], "prefabs": [],
            "stats": [], "stages": [], "healing": [], "effects": [], "vfx": [], "actions": [], "_refs": [],
        })
        entry["tiers"].append(int(tier))
        item = prefabs.get(path[len("prefabs/"):-len(".binfab")])
        for ref in refs(item) if item else []:
            if ref not in entry["_refs"]:
                entry["_refs"].append(ref)
        # Three ways the text is reached, in order of authority. The prefab names
        # its own key and that key is not always the filename (`empyrean_barrier`
        # is keyed `..._empyrean_barrier_spawner_...`). Failing that, the filename.
        # Failing that, the same slug under another colour: an ability keeps its
        # name across elements and only one colour's entry is always written -
        # Air's Bawk-Bomb has no key of its own, Water's does.
        keys = [t[: -len("_name")] for t in strings(item.components if item else [])
                if t.startswith("$prefabs_item_gem_large") and t.endswith("_name")]
        keys.append(f"$prefabs_item_gem_large_{name}")
        keys += [f"$prefabs_item_gem_large_{c}_{slug}_t{tier}" for c in ELEMENTS if c != colour]
        for base in keys:
            entry["name"] = entry["name"] or aloc.get(base + "_name", "")
            entry["description"] = entry["description"] or aloc.get(base + "_description", "")

    for entry in gems.values():
        entry["tiers"] = sorted(set(entry["tiers"]))
        for ref in entry.pop("_refs"):
            merge_rows(entry, describe(prefabs, ref, kit))
        for key in ("healing", "effects", "vfx", "actions"):
            if not entry[key]:
                del entry[key]

    return sorted(gems.values(), key=lambda g: (g["element"], g["name"] or g["slug"]))


def count(data: list) -> int:
    return len(data)

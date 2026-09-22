"""gem_abilities.json - empowered gems and the numbers behind their abilities.

Empowered gems are the ones that carry an ability, and the game names them by
file: `prefabs/item/gem/large/<element>_<slug>_t<tier>.binfab`. The item prefab
itself is only metadata (name key, description key, blueprint), so the numbers
come from `prefabs/abilities/gems/<slug>*` - the same damage record the class
abilities use (field 12 = weapon-damage multiplier, field 14 = base/100) plus
ordinary stat records.

Small gems carry no ability - they are `blue_t1`, `blue_t10_concat1` and so on,
with no slug - which is why only `large/` is walked.

Gem STAT ROLL values (gamedata/mystic.json) are deliberately not touched: they
are not in the prefab tree. `gems/meta/gem_upgradedata.binfab` holds upgrade
costs (flux, gemrepair, warpseed) and the item prefabs hold no stat records, so
the per-tier roll ranges are computed by the game and cannot be read from here.
"""
from __future__ import annotations

import re

from app.trove.codexes.binfab import harvest_strings
from app.trove.decode.common import damage_blocks, locale, stat_rows, stem
from app.trove.decode.tree import GameTree

TITLE = "Gem abilities"
OUTPUT = "gem_abilities.json"
PREFIXES = ("prefabs/item/gem/large/", "prefabs/abilities/gems/", "languages/en/")
INDENT, FINAL_NEWLINE = 4, False

# The file prefix is the gem's colour; these are the names the game's own
# descriptions use ("Empowered Water Gem for the Ice Sage.").
ELEMENTS = {"blue": "Water", "red": "Fire", "yellow": "Air",
            "opal": "Cosmic", "prismatic": "Prismatic"}

_ITEM = re.compile(r"^([a-z]+)_(.+)_t(\d+)$")


def build(tree: GameTree) -> list[dict]:
    aloc = locale(tree.read("languages/en/prefabs_item_gem_large.binfab"))
    by_stem = {stem(p): p for p in tree.files("prefabs/abilities/gems/", ".binfab")
               if p.count("/") == 3}

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
            "stats": [], "stages": [],
        })
        entry["tiers"].append(int(tier))
        # Three ways the text is reached, in order of authority. The prefab names
        # its own key and that key is not always the filename (`empyrean_barrier`
        # is keyed `..._empyrean_barrier_spawner_...`). Failing that, the filename.
        # Failing that, the same slug under another colour: an ability keeps its
        # name across elements and only one colour's entry is always written -
        # Air's Bawk-Bomb has no key of its own, Water's does.
        keys = [t[: -len("_name")] for t in
                (x[2] for x in harvest_strings(tree.read(path) or b""))
                if t.startswith("$prefabs_item_gem_large") and t.endswith("_name")]
        keys.append(f"$prefabs_item_gem_large_{name}")
        keys += [f"$prefabs_item_gem_large_{c}_{slug}_t{tier}" for c in ELEMENTS if c != colour]
        for base in keys:
            entry["name"] = entry["name"] or aloc.get(base + "_name", "")
            entry["description"] = entry["description"] or aloc.get(base + "_description", "")

    for (_colour, slug), entry in gems.items():
        entry["tiers"] = sorted(set(entry["tiers"]))
        # The slug names the ability; its prefabs are that slug plus its spawners,
        # effects and bullets. Longest-first so a stage keeps its most specific name.
        for ability in sorted(by_stem, key=len, reverse=True):
            if ability != slug and not ability.startswith(slug + "_"):
                continue
            data = tree.read(by_stem[ability]) or b""
            prefab = f"abilities/gems/{ability}"
            rows = stat_rows(data, prefab)
            stages = [{"name": ability.replace("_", " ").title(), "prefab": prefab,
                       "base": base, "multiplier": mult}
                      for mult, base in damage_blocks(data) if mult or base]
            if rows or stages:
                entry["prefabs"].append(prefab)
            for row in rows:
                if row not in entry["stats"]:
                    entry["stats"].append(row)
            for stage in stages:
                if stage not in entry["stages"]:
                    entry["stages"].append(stage)
        entry["prefabs"].sort()

    return sorted(gems.values(), key=lambda g: (g["element"], g["name"] or g["slug"]))


def count(data: list) -> int:
    return len(data)

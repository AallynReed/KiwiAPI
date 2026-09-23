"""ring_abilities.json - class rings and what their implementation chain changes.

Class rings live in `prefabs/abilities/mods_01/<class>/ring_<slug>.binfab`. Unlike
class abilities and gems, a ring names itself with LITERAL text in its identity
component - "Candy Barbarian: Spin-To-Win" and its description - so that is the
bridge here, with the mods locale table preferred where its key matches.

The ring itself carries no numbers; component 149 names the prefabs that
implement it (`ring_spin_to_win` -> `spin_to_win_swap`) and ``ability.describe``
reads that chain: the replacement abilities' energy and cooldown, damage, healing,
and each effect's duration and stat changes. The walk stops at the class's own
kit and at other rings, so a ring reports only what it adds.
"""
from __future__ import annotations

from app.trove.decode.ability import Prefabs, describe, identity, refs
from app.trove.decode.common import locale, stem
from app.trove.decode.gem_abilities import class_kit, merge_rows
from app.trove.decode.tree import GameTree

TITLE = "Ring abilities"
OUTPUT = "ring_abilities.json"
PREFIXES = ("prefabs/abilities/", "prefabs/class/", "prefabs/sfx/", "languages/en/")
INDENT, FINAL_NEWLINE = 4, False
MODS = "prefabs/abilities/mods_01/"


def _locale_names(tree: GameTree) -> dict[str, tuple[str, str]]:
    """`ring slug -> (name, description)` from the mods locale table.

    The key is `<class-ish>_<ring slug>` and the class part does not match the
    prefab folder (`candy_barbarian_` vs `candybarbarian/`), so only an exact
    slug tail is accepted - fuzzy matching pairs Deep Wounds with the wrong ring.
    Where it matches, the localised name is better than the prefab's literal
    title, which hyphenates its word breaks: "Melody-Overload" is "Melody
    Overload", and `ring_tactical_shot` is displayed as "Tactical Seekers".
    """
    table = locale(tree.read("languages/en/prefabs_abilities_mods.binfab"))
    out: dict[str, tuple[str, str]] = {}
    for key, value in table.items():
        if not key.endswith("_name"):
            continue
        slug = key[len("$prefabs_abilities_mods_"):-len("_name")]
        out[slug] = (value, table.get(f"$prefabs_abilities_mods_{slug}_description", ""))
    return out


def build(tree: GameTree) -> list[dict]:
    prefabs = Prefabs(tree)
    names = _locale_names(tree)
    kit = class_kit(prefabs, tree)
    rings = []
    for folder in tree.dirs(MODS):
        paths = [p for p in tree.files(f"{MODS}{folder}/ring_", ".binfab") if p.count("/") == 4]
        others = {f"abilities/mods_01/{folder}/{stem(p)}" for p in paths}
        for path in paths:
            ring = stem(path)
            rel = f"abilities/mods_01/{folder}/{ring}"
            pf = prefabs.get(rel)
            ident = identity(pf)
            # A title opening with "@" (Smashing, Firestorm, ...) is skipped, as the
            # literal-title decoder always did; whether those rings are live is unknown.
            owner, _, name = ident.get("name_key", "").partition(":")
            if not name.strip() or not owner[:1].isalpha():
                continue
            name, description = name.strip(), ident.get("description_key", "").strip()
            slug = ring[len("ring_"):]
            for key, (loc_name, loc_desc) in names.items():
                if key == slug or key.endswith("_" + slug):
                    name = loc_name or name
                    description = loc_desc or description
                    break
            entry = {
                "class": owner.strip(), "game_folder": folder, "name": name,
                "description": description, "slug": slug, "prefab": rel, "prefabs": [],
                "stats": [], "stages": [], "healing": [], "effects": [], "vfx": [], "actions": [],
            }
            for ref in refs(pf) if pf else []:
                info = describe(prefabs, ref, kit | (others - {rel}))
                merge_rows(entry, info)
            for key in ("healing", "effects", "vfx", "actions"):
                if not entry[key]:
                    del entry[key]
            rings.append(entry)
    return sorted(rings, key=lambda r: (r["class"], r["name"]))


def count(data: list) -> int:
    return len(data)

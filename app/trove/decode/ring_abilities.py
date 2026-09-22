"""ring_abilities.json - class rings and the numbers down their implementation chain.

Class rings live in `prefabs/abilities/mods_01/<class>/ring_<slug>.binfab`. Unlike
class abilities and gems, a ring names itself with LITERAL text rather than a
locale key - "Candy Barbarian: Spin-To-Win" and its description sit in the prefab
as plain strings - so that is the bridge here.

The ring itself carries no numbers; it points at the prefab that implements it
(`ring_spin_to_win` -> `spin_to_win_swap`), and the damage and stat records sit
down that chain, same shape as the class abilities: field 12 is the weapon-damage
multiplier, field 14 the base as a fraction of 100.
"""
from __future__ import annotations

import re

from app.trove.codexes.binfab import harvest_strings
from app.trove.decode.common import damage_blocks, locale, refs, stat_rows, stem
from app.trove.decode.tree import GameTree

TITLE = "Ring abilities"
OUTPUT = "ring_abilities.json"
PREFIXES = ("prefabs/abilities/mods_01/", "languages/en/")
INDENT, FINAL_NEWLINE = 4, False
MODS = "prefabs/abilities/mods_01/"

# "Candy Barbarian: Spin-To-Win" - a display title, not a path or a locale key.
_TITLE = re.compile(r"^[A-Z][A-Za-z' ]+:\s*\S.*$")


def _title_and_description(data: bytes) -> tuple[str, str, str]:
    """`(class, name, description)` from the prefab's own literal strings."""
    strings = [s[2] for s in harvest_strings(data)]
    title = next((s for s in strings
                  if _TITLE.match(s) and "/" not in s and not s.endswith(".blueprint")), "")
    if not title:
        return "", "", ""
    owner, _, name = title.partition(":")
    # The description is the longest remaining sentence; paths and one-word tags
    # (Equipment, quantitydecay) are never it.
    body = ""
    for s in strings:
        if s == title or "/" in s or s.endswith(".blueprint") or " " not in s:
            continue
        if len(s) > len(body):
            body = s
    return owner.strip(), name.strip(), body.strip()


def _walk(tree: GameTree, rel: str, folder: str, stop: set[str]) -> tuple[list[dict], list[dict], list[str]]:
    """Damage + stat records down one ring's implementation chain."""
    stats: list[dict] = []
    stages: list[dict] = []
    touched: list[str] = []
    seen = {rel}
    queue = [rel]
    while queue:
        cur = queue.pop(0)
        data = tree.read(f"prefabs/{cur}.binfab")
        if data is None:
            continue
        rows = stat_rows(data, cur)
        found = [{"name": cur.rsplit("/", 1)[-1].replace("_", " ").title(), "prefab": cur,
                  "base": base, "multiplier": mult}
                 for mult, base in damage_blocks(data) if mult or base]
        if rows or found:
            touched.append(cur)
        for row in rows:
            if row not in stats:
                stats.append(row)
        for stage in found:
            if stage not in stages:
                stages.append(stage)
        for child in refs(data, f"abilities/mods_01/{folder}/"):
            if child in seen or child in stop or len(seen) > 60:
                continue
            seen.add(child)
            queue.append(child)
    return stats, stages, touched


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
        desc = table.get(f"$prefabs_abilities_mods_{slug}_description", "")
        out[slug] = (value, desc)
    return out


def build(tree: GameTree) -> list[dict]:
    names = _locale_names(tree)
    rings = []
    for folder in tree.dirs(MODS):
        paths = [p for p in tree.files(f"{MODS}{folder}/ring_", ".binfab") if p.count("/") == 4]
        # Other rings of the same class are boundaries, so one ring cannot claim
        # another's numbers - the same rule the class abilities needed.
        others = {f"abilities/mods_01/{folder}/{stem(p)}" for p in paths}
        for path in paths:
            ring = stem(path)
            rel = f"abilities/mods_01/{folder}/{ring}"
            owner, name, description = _title_and_description(tree.read(path) or b"")
            if not name:
                continue
            slug = ring[len("ring_"):]
            for key, (loc_name, loc_desc) in names.items():
                if key == slug or key.endswith("_" + slug):
                    name = loc_name or name
                    description = loc_desc or description
                    break
            stats, stages, touched = _walk(tree, rel, folder, others - {rel})
            rings.append({
                "class": owner, "game_folder": folder, "name": name,
                "description": description, "slug": slug,
                "prefab": rel, "prefabs": touched, "stats": stats, "stages": stages,
            })
    return sorted(rings, key=lambda r: (r["class"], r["name"]))


def count(data: list) -> int:
    return len(data)

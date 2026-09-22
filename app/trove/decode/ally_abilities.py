"""ally_abilities.json - allies with the stats and abilities they grant.

Allies are the `prefabs/collections/pet/*.binfab` that carry combat stats - about
half the pet prefabs; the rest are cosmetic pets with nothing to grant. Both the
stat records and the ability refs come out of app/trove/codexes/bonuses.py, which
was written for exactly these collection prefabs.

Names and descriptions come from the prefab's own `$prefabs_collections_pet_…`
keys, resolved against languages/en/prefabs_collections_pet.binfab; an ability's
text is resolved the same way from the key its ref carries.
"""
from __future__ import annotations

from app.trove.codexes.binfab import decode_identity
from app.trove.codexes.bonuses import extract_abilities
from app.trove.decode.common import locale, stat_rows, stem
from app.trove.decode.tree import GameTree

TITLE = "Ally abilities"
OUTPUT = "ally_abilities.json"
PREFIXES = ("prefabs/collections/pet/", "languages/en/")
INDENT, FINAL_NEWLINE = 4, False


def build(tree: GameTree) -> list[dict]:
    names = locale(tree.read("languages/en/prefabs_collections_pet.binfab"))
    # An ability's text lives in whichever prefabs_abilities_* table its ref belongs
    # to, so merge them all rather than guessing which file to open per ability.
    abilities_text: dict[str, str] = {}
    for path in tree.files("languages/en/prefabs_abilities", ".binfab"):
        abilities_text.update(locale(tree.read(path)))

    allies = []
    for path in tree.files("prefabs/collections/pet/", ".binfab"):
        if path.count("/") != 3:
            continue
        data = tree.read(path) or b""
        stats = stat_rows(data)
        if not stats:
            continue                       # a cosmetic pet, not an ally
        slug = stem(path)
        identity = decode_identity(data) or {}
        name = names.get(identity.get("name_key") or "", "")
        description = names.get(identity.get("desc_key") or "", "")

        powers = []
        for ability in extract_abilities(data):
            if ability.get("hidden"):
                continue
            text = abilities_text.get(ability.get("key") or "", "")
            row = {"ref": ability.get("ref", ""), "text": text}
            if text and row not in powers:
                powers.append(row)

        allies.append({
            "slug": slug, "name": name or slug, "description": description,
            "prefab": f"collections/pet/{slug}", "stats": stats, "abilities": powers,
        })
    return sorted(allies, key=lambda a: a["name"].lower())


def count(data: list) -> int:
    return len(data)

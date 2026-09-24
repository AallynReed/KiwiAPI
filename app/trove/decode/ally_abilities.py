"""ally_abilities.json - allies with the stats and abilities they grant.

Allies are the `prefabs/collections/pet/*.binfab` players can own: every member of
`collections/collection_pet` (with the collection group it is filed under), plus
any other pet that carries combat stats. Pet prefabs outside both are NPC and test
variants. Everything
is read structurally: the stat modifier records (component 65), the identity
component's name and description keys, and every `abilities/...` prefab the pet
names.

An ability's text comes from its own prefab (``ability.text_keys``), not from a key
built out of its path: that guess missed the text on about 185 allies, and the old
byte scan missed a handful of allies outright. Each ability is then read by
``ability.describe``: the proc's cooldown, what it heals or deals, and each
effect's duration and stat changes.
"""
from __future__ import annotations

from app.trove.codexes.bonuses import _is_hidden
from app.trove.decode.ability import Prefabs, component_values, describe, identity, refs, text_keys
from app.trove.decode.common import collection_members, locale, stat_rows, stem
from app.trove.decode.tree import GameTree
from app.trove.decode.wire import WireError, parse

TITLE = "Ally abilities"
OUTPUT = "ally_abilities.json"
PREFIXES = ("prefabs/collections/", "prefabs/abilities/", "prefabs/sfx/", "languages/en/")
INDENT, FINAL_NEWLINE = 4, False


def _detail(prefabs: Prefabs, ref: str, text: dict[str, str]) -> dict:
    """What an ability ref does, trimmed to what a reader needs."""
    info = describe(prefabs, ref)
    out = {}
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
        title = text.get(eff.get("name_key", ""), "")
        if title:
            row["title"] = title
        effects.append(row)
    if effects:
        out["effects"] = effects
    if info["vfx"]:
        out["vfx"] = info["vfx"]
    return out


def build(tree: GameTree) -> list[dict]:
    prefabs = Prefabs(tree)
    names = locale(tree.read("languages/en/prefabs_collections_pet.binfab"))
    # An ability's text lives in whichever prefabs_abilities_* table its ref belongs
    # to, so merge them all rather than guessing which file to open per ability.
    abilities_text: dict[str, str] = {}
    for path in tree.files("languages/en/prefabs_abilities", ".binfab"):
        abilities_text.update(locale(tree.read(path)))

    owned = collection_members(prefabs.get("collections/collection_pet"),
                               {**locale(tree.read("languages/en/prefabs_collections.binfab")), **names})
    allies = []
    for path in tree.files("prefabs/collections/pet/", ".binfab"):
        if path.count("/") != 3:
            continue
        # Parsed here rather than through `prefabs`: 2,400 pets would sit in its cache.
        try:
            pf = parse(tree.read(path) or b"")
        except WireError:
            continue
        stats = stat_rows(component_values(pf))
        slug = stem(path)
        member = owned.get(f"collections/pet/{slug}")
        if not stats and member is None:
            continue                       # an NPC or test variant, not an ally
        ident = identity(pf)
        name = names.get(ident.get("name_key", ""), "")
        description = names.get(ident.get("description_key", ""), "")

        powers = []
        for ref in refs(pf):
            if _is_hidden(ref) or prefabs.get(ref) is None:
                continue
            text = abilities_text.get(text_keys(prefabs.get(ref)).get("description_key", ""), "")
            if text and not any(p["ref"] == ref and p["text"] == text for p in powers):
                powers.append({"ref": ref, "text": text, **_detail(prefabs, ref, abilities_text)})

        ally = {"slug": slug, "name": name or slug, "description": description,
                "prefab": f"collections/pet/{slug}", "stats": stats, "abilities": powers}
        if member and member[1]:
            ally["group"] = member[1]
        allies.append(ally)
    return sorted(allies, key=lambda a: a["name"].lower())


def count(data: list) -> int:
    return len(data)

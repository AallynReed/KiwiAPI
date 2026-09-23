"""ally_abilities.json - allies with the stats and abilities they grant.

Allies are the `prefabs/collections/pet/*.binfab` that carry combat stats - about
half the pet prefabs; the rest are cosmetic pets with nothing to grant. The stat
records and the ability refs come out of app/trove/codexes/bonuses.py, which was
written for exactly these collection prefabs.

Names and descriptions come from the prefab's own `$prefabs_collections_pet_…`
keys. Each ability is then read structurally (``ability.describe``): the proc's
cooldown, what it heals or deals, and each effect's duration and stat changes.
"""
from __future__ import annotations

from app.trove.codexes.binfab import decode_identity
from app.trove.codexes.bonuses import extract_abilities
from app.trove.decode.ability import Prefabs, describe
from app.trove.decode.common import locale, stat_rows, stem
from app.trove.decode.tree import GameTree

TITLE = "Ally abilities"
OUTPUT = "ally_abilities.json"
PREFIXES = ("prefabs/collections/pet/", "prefabs/abilities/", "prefabs/sfx/", "languages/en/")
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
            ref = ability.get("ref", "")
            if text and not any(p["ref"] == ref and p["text"] == text for p in powers):
                powers.append({"ref": ref, "text": text, **_detail(prefabs, ref, abilities_text)})

        allies.append({
            "slug": slug, "name": name or slug, "description": description,
            "prefab": f"collections/pet/{slug}", "stats": stats, "abilities": powers,
        })
    return sorted(allies, key=lambda a: a["name"].lower())


def count(data: list) -> int:
    return len(data)

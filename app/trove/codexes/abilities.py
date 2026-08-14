"""Ability display text, read from the referenced ability prefab.

A collection bonus references an ability by path (`abilities/equipment/onhit_heal`).
Turning that into something a player can read used to be done by CONSTRUCTING the
locale key from the ref, and by prettifying the ref itself into a title. Both are
wrong in ways that show:

- The description key is not one shape. The archive carries at least
  ``$prefabs_abilities_equipment_<x>_description`` and
  ``$abilities_equipment_<x>_description``, and the constructed form additionally
  guessed a `…_combateventeffectspawner_…` segment. Whenever the guess missed, the
  bonus displayed with no description at all.
- Ability prefabs mostly carry NO name key. Prettifying the ref produced labels like
  "Enemydeath Damagebuff" - an internal id with title case on it, shown where the
  game itself shows no name.

So: open the referenced prefab and read the keys it actually contains. When it names
no display name, this reports none, and the caller shows the description alone rather
than a transformed id.

Pure + stdlib-only.
"""

from __future__ import annotations

from app.trove.codexes.binfab import harvest_strings


def normalize_ref(ref: str) -> str:
    """An ability ref -> its prefab logical path (relative to `prefabs/`, no suffix).

    Refs are written a few ways depending on where they appear; the archive path is
    always under `abilities/`."""
    text = str(ref or "").replace("\\", "/").strip().strip("/")
    text = text.removesuffix(".binfab")
    if not text:
        return ""
    if text.startswith("prefabs/"):
        text = text[len("prefabs/"):]
    if not text.startswith("abilities/"):
        text = "abilities/" + text
    return text.lower()


def extract_keys(content: bytes) -> dict[str, str]:
    """`{"name_key", "desc_key"}` from an ability prefab's own bytes.

    Uses the length-prefixed string scan rather than a raw `$…` regex: a raw match
    runs past the key into whatever byte follows, so `…_description` reads as
    `…_description0` and then matches nothing in the locale map.

    First of each kind wins - a prefab that carries several describes its own primary
    effect first, and later ones belong to sub-components.
    """
    name_key = ""
    desc_key = ""
    for _off, _field, text in harvest_strings(content):
        if not text.startswith("$"):
            continue
        if not desc_key and text.endswith("_description"):
            desc_key = text
        elif not name_key and text.endswith("_name"):
            name_key = text
        if name_key and desc_key:
            break
    return {"name_key": name_key, "desc_key": desc_key}


def display(keys: dict[str, str], loc: dict[str, str]) -> dict[str, str]:
    """Resolve the extracted keys against the locale map.

    An unresolved key yields an empty string - never a prettified fallback, which is
    what produced the internal-id labels this module exists to stop."""
    name_key = keys.get("name_key") or ""
    desc_key = keys.get("desc_key") or ""
    return {
        "name_key": name_key,
        "desc_key": desc_key,
        "name": loc.get(name_key, "") if name_key else "",
        "description": loc.get(desc_key, "") if desc_key else "",
    }

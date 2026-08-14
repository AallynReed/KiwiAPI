"""The unlock graph: `prefabs/collections/unlocks.binfab`.

The file states what owning or consuming one thing grants you: an unlocker item and
the recipe it teaches, a collectible and the badge it awards, a class level and the
starter styles it hands out. It is stored as a flat run of length-prefixed reference
strings in (source, target) order.

Pairing is positional, which is exactly the kind of thing that goes quietly wrong, so
the rule here is deliberately strict: a pair is only emitted when BOTH halves look
like real references (a `<kind>/<name>` path or a bare `recipe_*` id). Anything else
resets the pairing rather than shifting every later pair by one - a half-read run
that silently re-pairs the whole file is worse than a short one.

Pure + stdlib-only.
"""

from __future__ import annotations

import re

from app.trove.codexes.binfab import _real_fields

UNLOCKS_PATH = "prefabs/collections/unlocks.binfab"

_RECIPE_RE = re.compile(r"^recipe_[a-z0-9_]+$")
_PATH_RE = re.compile(r"^[a-z0-9_]+(?:/[A-Za-z0-9_\-.\[\]]+)+$")

# Reference families that can only ever be GRANTED, never do the granting: a blueprint
# is an asset, a bare `recipe_*` token is a taught recipe, and a badge rank is an award.
# Everything else (an unlocker item, a collectible, a costume label) can open a group.
_TARGET_ONLY_PREFIXES = ("collections/badge/",)
_SOURCE_ROOTS = ("collections/", "item/", "equipment/", "recipes/", "placeable/")


def _is_target_only(text: str) -> bool:
    lowered = text.lower()
    return (lowered.endswith(".blueprint")
            or bool(_RECIPE_RE.match(lowered))
            or lowered.startswith(_TARGET_ONLY_PREFIXES))


def _is_source(text: str) -> bool:
    """A reference that can open a group: a known prefab root, or the bare costume
    label the style grants are filed under (`gunslinger_llama`)."""
    lowered = text.lower()
    if lowered.startswith("$") or _is_target_only(lowered):
        return False
    if lowered.startswith(_SOURCE_ROOTS) and _PATH_RE.match(lowered):
        return True
    return "/" not in lowered and "." not in lowered and len(lowered) > 3


def parse_unlocks(data: bytes) -> list[tuple[str, str]]:
    """`[(source, target), …]` - what each source grants, in file order.

    Grants are NOT fixed pairs: a costume grants two or three styles at once, so a
    strict alternating read mis-pairs everything after the first multi-grant. Instead
    a source opens a group and every following target-only reference attaches to it,
    which is decided by what KIND of reference each string is rather than by its
    position - see `_is_target_only`.

    A source with no targets is dropped rather than paired with whatever came next.
    Both halves are returned as written; the caller normalizes them to entry paths.
    """
    pairs: list[tuple[str, str]] = []
    seen: set[tuple[str, str]] = set()
    source: str | None = None
    # `_real_fields` (not `harvest_strings`) because the byte-offset scan also emits
    # phantom matches that start INSIDE a real string; those would read as extra grants.
    for _off, _field, raw in _real_fields(data):
        text = raw.strip().rstrip("./")
        if not text:
            continue
        if _is_target_only(text):
            if source is None:
                continue
            key = (source.lower(), text.lower())
            if key not in seen and source.lower() != text.lower():
                seen.add(key)
                pairs.append((source, text))
        elif _is_source(text):
            source = text
    return pairs


def starter_style_unlocks(pairs: list[tuple[str, str]]) -> set[str]:
    """Targets granted by a class level-1 allocation - the styles you start with.

    These are source-backed zero-mastery styles: the game hands them to you, so they
    are not a collection you can earn. Returned as the raw target references."""
    return {
        target for source, target in pairs
        if source.lower().endswith("_lvl1") or "_lvl1/" in source.lower()
    }

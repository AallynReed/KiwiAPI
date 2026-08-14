"""The 8 codex types and how a prefab path maps to one.

Paths are the archive's logical paths; Trove keeps prefabs under `prefabs/`, with
collectibles in `collections/<kind>/` and items in `item/<kind>/`. Matched most
specific first (item/fish/ before item/). `PREFABS_ROOT`/`LOCALE_ROOT` are
verifiable against the live tree via `/v1/updates/{branch}/tree`.
"""

from __future__ import annotations

PREFABS_ROOT = "prefabs/"
LOCALE_ROOT = "languages/en/"

# (type, prefix-under-prefabs/, excluded-stem-suffixes), specific -> general.
# NOTE: dragons are NOT path-classified - they live under `collections/mount/`
# alongside regular mounts and are split out by their collection category in the
# indexer (a category containing "dragon" => dragon), so there's no entry here.
CODEX_TYPES: list[tuple[str, str, tuple[str, ...]]] = [
    ("fish", "item/fish/", ()),
    ("memento", "item/unlocker/", ()),
    ("ally", "collections/pet/", ("_npc",)),
    ("mount", "collections/mount/", ()),
    ("badge", "collections/badge/", ()),
    # The rest of the collection tree. These were never classified, so the game's
    # wings, auras, boats, sails, flasks, tomes, mag riders and fishing poles - ~660
    # collectibles the mastery table already knew the base for - were parsed by nothing
    # and served by nothing. Directory names are the archive's own (`collections/fishing`
    # holds fishing poles; there is no `collections/fishingpole`).
    ("wings", "collections/wings/", ()),
    ("aura", "collections/aura/", ()),
    ("boat", "collections/boat/", ()),
    ("sail", "collections/sail/", ()),
    ("flask", "collections/flask/", ()),
    ("tome", "collections/tome/", ()),
    ("magrider", "collections/magrider/", ()),
    ("fishingpole", "collections/fishing/", ()),
    # Costumes live in their own root, not under collections/.
    ("skin", "skins/", ()),
    # Styles (hats/faces/hair/weapons/banners) are the `equipment/` appearance prefabs
    # (verified against the live archive; catalogue = collection_equipmentappearance).
    # Must precede `item/` - both are item-ish, but equipment/ is its own root.
    ("style", "equipment/", ()),
    ("recipe", "recipes/", ()),
    ("item", "item/", ()),  # catch-all for remaining item/* prefabs
]

# Types whose entries are collectibles with a mastery base (used by the read layer to
# group the codex browser). `dragon` is derived from mounts, so it inherits mount's.
COLLECTIBLE_TYPES: frozenset[str] = frozenset({
    "ally", "mount", "dragon", "badge", "wings", "aura", "boat", "sail",
    "flask", "tome", "magrider", "fishingpole", "skin", "fish", "memento",
})
# Every codex type the API serves - the path-classified ones plus `dragon`
# (derived from mounts). Used to validate `/v1/codexes/{type}`.
ALL_TYPES: list[str] = [t[0] for t in CODEX_TYPES] + ["dragon"]


def classify(path: str) -> str | None:
    """The codex type for a prefab logical path, or None if it isn't a codex source."""
    if not path.startswith(PREFABS_ROOT) or not path.endswith(".binfab"):
        return None
    rel = path[len(PREFABS_ROOT):]
    stem = rel.rsplit("/", 1)[-1].removesuffix(".binfab")
    for ctype, prefix, excludes in CODEX_TYPES:
        if rel.startswith(prefix):
            if any(stem.endswith(suffix) for suffix in excludes):
                return None
            return ctype
    return None

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
CODEX_TYPES: list[tuple[str, str, tuple[str, ...]]] = [
    ("fish", "item/fish/", ()),
    ("memento", "item/unlocker/", ()),
    ("ally", "collections/pet/", ("_npc",)),
    ("mount", "collections/mount/", ()),
    ("dragon", "collections/dragon/", ()),
    ("badge", "collections/badge/", ()),
    ("recipe", "recipes/", ()),
    ("item", "item/", ()),  # catch-all for remaining item/* prefabs
]
ALL_TYPES: list[str] = [t[0] for t in CODEX_TYPES]


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

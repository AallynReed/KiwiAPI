"""Per-prefab extraction → a codex entry dict. Pure (bytes + locale map in).

v1 is identity-level: name/category/description (locale-resolved), tradability,
and the source keys - which covers the entity prefabs (allies, mounts, dragons,
badges, items, fish, mementos). Recipes and collection tables have no identity
component, so they fall back to a filename-derived name until their typed
extractors land. Rich per-type fields (stats, mastery, model, variants) populate
`data` later.
"""

from __future__ import annotations

from app.trove.codexes import binfab


def _name_from_path(path: str) -> str:
    stem = path.rsplit("/", 1)[-1].removesuffix(".binfab")
    return stem.replace("_", " ").strip().title() or stem


def refine_mount(entry: dict, rel_path: str, mount_categories: dict[str, str]) -> dict:
    """Split dragons out of the mount tree (mutates + returns `entry`).

    Mounts and dragons both live under `collections/mount/`; the `collection_mount`
    table assigns each a category, and a category containing "dragon" marks a
    dragon (matching BTT's `category.includes('dragon')` rule). The table category
    is preferred over the in-prefab one; if absent, the in-prefab category stands.
    """
    category = mount_categories.get(rel_path.lower()) or entry.get("category") or ""
    entry["category"] = category
    if "dragon" in category.lower():
        entry["codex_type"] = "dragon"
    return entry


def extract_entry(codex_type: str, path: str, content: bytes, loc_map: dict[str, str]) -> dict:
    """Identity-level codex entry from a prefab's bytes + the resolved locale map."""
    ident = binfab.decode_identity(content) or {}
    name_key = ident.get("name_key")
    desc_key = ident.get("desc_key")
    name = (loc_map.get(name_key) if name_key else None) or _name_from_path(path)
    description = (loc_map.get(desc_key) if desc_key else None) or ""
    return {
        "codex_type": codex_type,
        "path": path,
        "name": name,
        "category": ident.get("category") or "",
        "description": description,
        "tradable": ident.get("tradable"),
        "name_key": name_key,
        "desc_key": desc_key,
        "blueprint": None,
        "data": {},
    }

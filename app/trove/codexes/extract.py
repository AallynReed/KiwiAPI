"""Per-prefab extraction → a codex entry dict. Pure (bytes + locale map in).

v1 is identity-level: name/category/description (locale-resolved), tradability,
and the source keys — which covers the entity prefabs (allies, mounts, dragons,
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

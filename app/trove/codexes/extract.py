"""Per-prefab extraction → a codex entry dict. Pure (bytes + locale map in).

Identity-level: name/category/description (locale-resolved), tradability, source
keys - covering the entity prefabs (allies, mounts, dragons, badges, items, fish,
mementos). On top of that, content-only bonus extraction: Power Rank, numeric stat
records, and visible/hidden ability refs (into `data`). Mastery (normal + geode)
and geode-companion upgrade-tree levels need lookup tables, so the indexer adds
those. Recipes / collection tables have no identity component and fall back to a
filename-derived name.
"""

from __future__ import annotations

from app.trove.codexes import binfab, bonuses, powerrank

# Types whose prefabs carry displayed stat/ability/Power-Rank bonuses (the handoff's
# "collection prefabs"). `mount` covers dragons too (split out after extraction).
# Geode companions are `item` and get their bonuses from the upgrade tree instead.
_BONUS_TYPES = frozenset({"ally", "mount", "badge"})


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

    # Content-only collection bonuses, only for the types that carry them (mastery +
    # geode-companion levels are added by the indexer, which has the lookup tables).
    data: dict = {}
    power_rank = None
    if codex_type in _BONUS_TYPES:
        stats = bonuses.extract_stat_bonuses(content)
        abilities = bonuses.extract_abilities(content)
        if stats:
            data["stats"] = stats
        if abilities:
            data["abilities"] = abilities
        power_rank = powerrank.decode_power_rank(content)

    return {
        "codex_type": codex_type,
        "path": path,
        "name": name,
        "category": ident.get("category") or "",
        "description": description,
        "tradable": ident.get("tradable"),
        "mastery_geode": None,
        "power_rank": power_rank,
        "name_key": name_key,
        "desc_key": desc_key,
        "blueprint": None,
        "data": data,
    }

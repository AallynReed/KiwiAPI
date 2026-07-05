"""Per-prefab extraction → a codex entry dict.

Identity-level: name/category/description (locale-resolved), tradability, source
keys. On top of that, content bonus extraction: Power Rank, numeric stat records,
and visible/hidden ability refs (into `data`), with the `$…` keys those carry
resolved to real text through the archived locale tables. Recipes have no identity
component, so they are parsed structurally (output / ingredients / requirements)
via `recipe.parse_recipe`, resolving referenced item names through `resolve_meta`.

Mastery (normal + geode) and geode-companion upgrade-tree levels need the indexer's
lookup tables, so the indexer adds those.
"""

from __future__ import annotations

import re

from app.trove.codexes import binfab, bonuses, localize, powerrank, recipe, styles

# Types whose prefabs carry displayed stat/ability/Power-Rank bonuses (the handoff's
# "collection prefabs"). `mount` covers dragons too (split out after extraction).
_BONUS_TYPES = frozenset({"ally", "mount", "badge"})

_PATH = re.compile(rb"[A-Za-z0-9_][A-Za-z0-9_\-/\[\].]*")
_BLUEPRINT_REF = re.compile(rb"[A-Za-z0-9_][A-Za-z0-9_\-/\[\].]*\.blueprint")


def _blueprint_ref(content: bytes, valid_blueprints: set[str] | None = None,
                   path: str = "") -> str | None:
    """The model blueprint a prefab references (logical path), or None.

    Prefabs name their model two ways: an explicit ``<dir>/<name>.blueprint`` (allies,
    mounts) OR an extensionless ``<year>/<cat>/<name>`` (items, tokens, consumables).
    The latter is indistinguishable from other strings by shape alone, so once we have
    the real blueprint file tree we accept any path-run whose path matches a real
    blueprint file -- stripping up to 5 leading bytes first (to drop a printable length
    prefix like 'F'/'6'/'3').
    """
    p = path.lower()
    if "item/skin" in p or ("item/unlocker" in p and b"equipment_" in content):
        return None

    # No blueprints set (dev / no validation): only an explicit ".blueprint" suffix is safe.
    if not valid_blueprints:
        m = _BLUEPRINT_REF.search(content)
        return m.group(0).decode("ascii", "ignore") if m else None

    explicit_fallback: str | None = None
    for m in _PATH.finditer(content):
        s = m.group(0).decode("ascii", "ignore")
        for i in range(min(5, len(s))):          # strip up to 5 leading junk bytes
            cand = s[i:]
            if not cand:
                continue
            cand_bp = cand if cand.endswith(".blueprint") else cand + ".blueprint"
            if cand_bp.lower() in valid_blueprints:
                return cand_bp
        if s.endswith(".blueprint") and explicit_fallback is None:
            explicit_fallback = s
    return explicit_fallback


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


def _enrich_bonuses(data: dict, loc_map: dict[str, str]) -> None:
    """Resolve the `$…` keys the bonus decoders left in `data` to real text."""
    for s in data.get("stats", []):
        s["stat_name"] = localize.resolve_stat_name(loc_map, s.get("stat"))
        if s.get("slot"):
            s["slot_name"] = localize.resolve_slot_name(loc_map, s["slot"])
    for a in data.get("abilities", []):
        if a.get("hidden"):
            continue
        a["name"] = _ability_name(a.get("ref", ""))
        text = localize.resolve_text(loc_map, a.get("key"))
        if text:
            a["description"] = text


def _ability_name(ref: str) -> str:
    seg = str(ref or "").rstrip("/").rsplit("/", 1)[-1]
    return seg.replace("_", " ").strip().title()


def _recipe_entry(path: str, content: bytes, loc_map: dict[str, str], resolve_meta, valid_blueprints: set[str] | None = None) -> dict:
    parsed = recipe.parse_recipe(content, resolve_meta=resolve_meta)
    data: dict = {"recipe": {
        "output": parsed["output"],
        "ingredients": parsed["ingredients"],
        "requirements": parsed["requirements"],
    }}
    return {
        "codex_type": "recipe",
        "path": path,
        "name": parsed["name"] or _name_from_path(path),
        "category": parsed["category"],
        "description": parsed["description"],
        "tradable": None,
        "mastery_geode": None,
        "power_rank": None,
        "name_key": None,
        "desc_key": None,
        "blueprint": _blueprint_ref(content, valid_blueprints=valid_blueprints, path=path),
        "data": data,
    }


def extract_entry(codex_type: str, path: str, content: bytes, loc_map: dict[str, str],
                  *, resolve_meta=None, valid_blueprints: set[str] | None = None) -> dict:
    """Codex entry from a prefab's bytes + the resolved locale map.

    `resolve_meta(item_path) -> {"name","desc"}` resolves referenced item prefabs
    (recipes); pass None for a locale-only extraction.
    """
    if codex_type == "recipe":
        return _recipe_entry(path, content, loc_map, resolve_meta, valid_blueprints=valid_blueprints)

    ident = binfab.decode_identity(content) or {}
    name_key = ident.get("name_key")
    desc_key = ident.get("desc_key")
    name = (loc_map.get(name_key) if name_key else None) or _name_from_path(path)
    description = (loc_map.get(desc_key) if desc_key else None) or ""

    # Content bonuses, only for the types that carry them (mastery + geode-companion
    # levels are added by the indexer, which has the lookup tables).
    data: dict = {}
    power_rank = None
    if codex_type in _BONUS_TYPES:
        stats = bonuses.extract_stat_bonuses(content)
        abilities = bonuses.extract_abilities(content)
        if stats:
            data["stats"] = stats
        if abilities:
            data["abilities"] = abilities
        _enrich_bonuses(data, loc_map)
        power_rank = powerrank.decode_power_rank(content)

    category = ident.get("category") or ""

    # Styles are the `equipment/` appearance prefabs: attach the equipment id + best-
    # effort slot family. Mastery is the standard EquipmentAppearance base, added by the
    # indexer. The in-prefab category is always "Equipment", so the family is the more
    # useful display category when we can detect a slot.
    if codex_type == "style":
        rel = path[len("prefabs/"):].removesuffix(".binfab") if path.startswith("prefabs/") else path.removesuffix(".binfab")
        family = styles.style_family(rel)
        data["style"] = {**styles.style_identity(rel), "family": family}
        category = family or category

    return {
        "codex_type": codex_type,
        "path": path,
        "name": name,
        "category": category,
        "description": description,
        "tradable": ident.get("tradable"),
        "mastery_geode": None,
        "power_rank": power_rank,
        "name_key": name_key,
        "desc_key": desc_key,
        "blueprint": _blueprint_ref(content, valid_blueprints=valid_blueprints, path=path),
        "data": data,
    }

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
#
# The ride/wear families were absent while they were also unclassified, so their stats
# were never looked for; boats in particular carry three slot blocks (mount, wings,
# boat) and every one of them was going unread.
_BONUS_TYPES = frozenset({
    "ally", "mount", "badge", "wings", "aura", "boat", "sail", "flask",
    "tome", "magrider", "fishingpole", "skin",
})

_PATH = re.compile(rb"[A-Za-z0-9_][A-Za-z0-9_\-/\[\].]*")
_BLUEPRINT_REF = re.compile(rb"[A-Za-z0-9_][A-Za-z0-9_\-/\[\].]*\.blueprint")
# Community-made models carry their author(s) in brackets, and multiple authors are
# separated by "," or " " -- neither of which `_PATH` accepts, so a credited name
# splits mid-string and only its tail survives ("AirRider].blueprint"). Match the
# bracketed form whole so those names stay intact.
_CREDITED = re.compile(rb"[A-Za-z0-9_][A-Za-z0-9_\-/.]*\[[^\[\]\x00-\x1f]{1,80}\](?:\.blueprint)?")


def _runs(content: bytes) -> list[bytes]:
    """Every path-like byte run in the file, in file order. Both regexes contribute;
    where they start at the same offset the longer (credited) match is tried first."""
    hits = [(m.start(), -len(m.group(0)), m.group(0))
            for rx in (_PATH, _CREDITED) for m in rx.finditer(content)]
    hits.sort()
    return [h[2] for h in hits]


def _unprefixed(run: bytes):
    """Candidate strings for one run, best first.

    binfab strings are length-prefixed and the prefix byte is often printable
    ('F'/'6'/'K'), so it rides along on the front of the match. When it equals the
    remaining length exactly, that byte *is* the prefix -- otherwise fall back to
    trying the first few offsets blind.
    """
    s = run.decode("ascii", "ignore")
    if s and ord(s[0]) == len(s) - 1:
        yield s[1:]
    for i in range(min(5, len(s))):
        if s[i:]:
            yield s[i:]


def _is_ui_model(name: str) -> bool:
    """``…_ui.blueprint`` / ``…_UI[Author].blueprint`` -- the whole-model catalog icon.

    Multi-part creatures (carpets, bulldozers, warhorses, cycles) also list every
    component blueprint they assemble from, and those parts are empty v5 placeholders
    that render blank, so a bare "first match wins" picks a banner or a wheel over
    the mount itself.
    """
    stem = name.rsplit("/", 1)[-1].removesuffix(".blueprint").split("[", 1)[0]
    return stem.lower().endswith("_ui")


def blueprint_ref(content: bytes, valid_blueprints: set[str] | None = None,
                  path: str = "", *, model_size=None) -> str | None:
    """The model blueprint a prefab references (logical path), or None.

    Prefabs name their model two ways: an explicit ``<dir>/<name>.blueprint`` (allies,
    mounts) OR an extensionless ``<year>/<cat>/<name>`` (items, tokens, consumables).
    The latter is indistinguishable from other strings by shape alone, so once we have
    the real blueprint file tree we accept any path-run that matches a real blueprint
    file. A run that matches nothing is a guess, not a reference -- we return None
    rather than store a name that can only 404.

    ``model_size(name) -> int`` optionally reports a blueprint's voxel count (0 when it
    can't be drawn). With it, a name that resolves to an empty placeholder no longer
    ends the search: a handful of creatures - the warhorse/bull mounts - ship no whole
    model at all, only parts, and the parts the prefab happens to name first are the
    empty banner slots. Without it, resolution is name-only (dev, tests, callers with
    no archive to read).
    """
    p = path.lower()
    if "item/skin" in p or ("item/unlocker" in p and b"equipment_" in content):
        return None

    # No blueprints set (dev / no validation): only an explicit ".blueprint" suffix is safe.
    if not valid_blueprints:
        m = _BLUEPRINT_REF.search(content)
        return m.group(0).decode("ascii", "ignore") if m else None

    candidates: list[str] = []
    for run in _runs(content):
        for cand in _unprefixed(run):
            cand_bp = cand if cand.endswith(".blueprint") else cand + ".blueprint"
            if cand_bp.lower() not in valid_blueprints:
                continue
            if cand_bp not in candidates:
                candidates.append(cand_bp)
            break
    if not candidates:
        return None

    ui = [c for c in candidates if _is_ui_model(c)]
    if model_size is None:                       # name-only resolution
        return ui[0] if ui else candidates[0]

    for c in ui:                                 # the whole-model icon, if it draws
        if model_size(c):
            return c
    if model_size(candidates[0]):                # what the prefab names first, if it draws
        return candidates[0]
    # Parts-only creature: show its largest piece rather than an empty slot. Still one of
    # THIS prefab's own blueprints, so it can't be another creature's model.
    biggest = max(candidates, key=model_size)
    return biggest if model_size(biggest) else candidates[0]


# Placeables (decorations, frameworks) name no model at all - their prefab is ~200 bytes
# carrying a locale key, a category and its own path, nothing else. The game resolves the
# mesh by NAME, and the blueprint tree mirrors the prefab path: `placeable/deco/<stem>` ->
# `deco_<stem>`, `placeable/frameworks/<stem>` -> `fw_<stem>`, optionally under a year
# directory and optionally carrying an `[author]` credit.
#
# This is a CONVENTION, not a reference the data hands us, so it is gated on the name
# resolving to exactly one blueprint in the branch: an ambiguous stem yields nothing
# rather than a coin-flip between two models. Blocks are deliberately absent - terrain
# blocks have no per-block model to find.
_PLACEABLE_MODEL_PREFIX = {"deco": "deco_", "frameworks": "fw_"}


def blueprint_stems(valid_blueprints: set[str]) -> dict[str, list[str]]:
    """``basename without its [author] credit`` -> the blueprints carrying that name."""
    stems: dict[str, list[str]] = {}
    for bp in valid_blueprints:
        base = bp.rsplit("/", 1)[-1].removesuffix(".blueprint").split("[", 1)[0]
        stems.setdefault(base, []).append(bp)
    return stems


def model_blueprint(model: str, valid_blueprints: set[str],
                    stems: dict[str, list[str]]) -> str | None:
    """A model name from the block table (`blocks.parse_block_models`) -> a real
    blueprint path. The table stores a bare basename, so it matches either directly or
    through the `[author]`-stripped index; an ambiguous name resolves to nothing."""
    m = model.replace("\\", "/").lower().removesuffix(".blueprint")
    if (m + ".blueprint") in valid_blueprints:
        return m + ".blueprint"
    hits = stems.get(m) or []
    return hits[0] if len(hits) == 1 else None


def conventional_blueprint(path: str, stems: dict[str, list[str]]) -> str | None:
    """The model a placeable prefab implies by name, or None when the name doesn't
    resolve to exactly one blueprint. See ``_PLACEABLE_MODEL_PREFIX``.

    Last resort only - `blocks.binfab` states the real mapping for most placeables and
    must be consulted first; a name cannot distinguish mirrored siblings (left/right
    arrows) and this gets those backwards."""
    parts = path.replace("\\", "/").removesuffix(".binfab").lower().split("/")
    if len(parts) < 3 or parts[0] != "placeable":
        return None
    prefix = _PLACEABLE_MODEL_PREFIX.get(parts[1])
    if not prefix:
        return None
    hits = stems.get(prefix + parts[-1]) or []
    return hits[0] if len(hits) == 1 else None


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


def _output_blueprint(output: dict | None, resolve_meta) -> str | None:
    """A recipe prefab names no model of its own -- it references the prefab of the
    item it crafts. Borrow that item's blueprint so the card shows what you get."""
    out_path = (output or {}).get("path")
    if not out_path or resolve_meta is None:
        return None
    return (resolve_meta(out_path) or {}).get("blueprint")


def _recipe_entry(path: str, content: bytes, loc_map: dict[str, str], resolve_meta,
                  valid_blueprints: set[str] | None = None, model_size=None,
                  prefab_exists=None) -> dict:
    parsed = recipe.parse_recipe(content, resolve_meta=resolve_meta,
                                 prefab_exists=prefab_exists)
    data: dict = {"recipe": {
        "output": parsed["output"],
        "ingredients": parsed["ingredients"],
        "requirements": parsed["requirements"],
    }}
    blueprint = blueprint_ref(content, valid_blueprints=valid_blueprints, path=path,
                              model_size=model_size)
    if blueprint is None:
        blueprint = _output_blueprint(parsed["output"], resolve_meta)
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
        "blueprint": blueprint,
        "data": data,
    }


def extract_entry(codex_type: str, path: str, content: bytes, loc_map: dict[str, str],
                  *, resolve_meta=None, valid_blueprints: set[str] | None = None,
                  model_size=None, prefab_exists=None,
                  power_rank_table: dict[int, int] | None = None,
                  style_rows: dict[str, dict] | None = None) -> dict:
    """Codex entry from a prefab's bytes + the resolved locale map.

    `resolve_meta(item_path) -> {"name","desc"}` resolves referenced item prefabs
    (recipes); pass None for a locale-only extraction.
    """
    if codex_type == "recipe":
        return _recipe_entry(path, content, loc_map, resolve_meta,
                             valid_blueprints=valid_blueprints, model_size=model_size,
                             prefab_exists=prefab_exists)

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
        power_rank = powerrank.decode_power_rank(content, power_rank_table)

    category = ident.get("category") or ""

    # Styles are the `equipment/` appearance prefabs: attach the equipment id + best-
    # effort slot family. Mastery is the standard EquipmentAppearance base, added by the
    # indexer. The in-prefab category is always "Equipment", so the family is the more
    # useful display category when we can detect a slot.
    if codex_type == "style":
        rel = path[len("prefabs/"):].removesuffix(".binfab") if path.startswith("prefabs/") else path.removesuffix(".binfab")
        # The loot catalogue STATES the slot family and (for hats) the appearance base;
        # `style_family` only guesses it from stem tokens and returns "" for anything
        # unconventionally named. Prefer the catalogue and keep the guess as a fallback
        # for a style no catalogue lists.
        row = (style_rows or {}).get(styles.equipment_id(rel))
        family = (row or {}).get("family") or styles.style_family(rel)
        data["style"] = {**styles.style_identity(rel), "family": family}
        if row:
            data["style"]["catalogue"] = row.get("source", "")
            data["style"]["raw_category"] = row.get("raw_category")
            if row.get("base_mastery") is not None:
                data["style"]["base_mastery"] = row["base_mastery"]
            if row.get("name_key") and not name_key:
                name_key = row["name_key"]
                name = loc_map.get(name_key) or name
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
        "blueprint": blueprint_ref(content, valid_blueprints=valid_blueprints, path=path,
                                   model_size=model_size),
        "data": data,
    }

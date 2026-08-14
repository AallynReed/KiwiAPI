"""Build the codex from the archive for a branch - incrementally.

The archive (``UpdateState`` + CAS) stays in Mongo; the parsed codex is written to
the Postgres ``codex_entry`` table (``pg_store``), one row per source prefab. Two
modes:

- ``reindex`` - a full (re)build from the materialized tree (``UpdateState``). The
  bootstrap/repair path. Content-incremental too: a prefab whose source sha is
  unchanged (vs. the stored row) is skipped, so re-running is cheap.
- ``reindex_changes`` - the steady-state path. Reads just the ``UpdateChange``
  rows for one new version (the delta) and touches only those rows, so a routine
  game patch never walks the other 99% of the game.

``ensure_indexed`` picks between them after a sync: full build if the branch's
codex table is empty (e.g. first deploy, or after switching to Postgres),
otherwise the delta. Names/descriptions resolve through the merged ``languages/``
locale tables. The whole codex is disposable - rebuildable from the archive.
"""

from __future__ import annotations

import asyncio
import logging
from dataclasses import dataclass, field

from app.core.config import settings
from app.core.utils import utcnow
from app.trove.codexes import (
    abilities,
    badges,
    binfab,
    blocks,
    catalogue,
    geode,
    links,
    localize,
    loot,
    mastery,
    pg_store,
    powerrank,
    providers,
    recipe,
    unlocks,
    upgrades,
)
from app.trove.codexes.extract import (
    blueprint_ref,
    blueprint_stems,
    conventional_blueprint,
    extract_entry,
    model_blueprint,
    refine_mount,
)
from app.trove.codexes.models import (
    ability_rows,
    requirement_rows,
    stat_rows,
    to_row,
    upgrade_rows,
)
from app.trove.codexes.types import LOCALE_ROOT, PREFABS_ROOT, classify
from app.trove.updates.cas import ContentStore
from app.trove.updates.models import UpdateChange, UpdateState

logger = logging.getLogger("kiwi.trove.codexes")

# Bump this whenever the parser/extraction logic changes (new fields, fixed decode,
# resolved strings, …). On the next sync the indexer force-rebuilds any branch whose
# stored version is behind, so a parser change reaches the data WITHOUT a game update
# or a manual rebuild - the steady-state delta only re-touches changed game files.
CODEX_PARSER_VERSION = 24  # v24: ability names and descriptions come from the referenced ability prefab instead of being derived from the ref - the derived description key had the wrong shape for part of the archive, and the derived NAME was an internal id in title case ("Enemydeath Damagebuff"), shown where the game names nothing at all
# v23: node effects chunk on the tree's own node keys instead of the filename stem, which found nothing for the class prestige trees (stored as prestige_bard.binfab, nodes named 01_bard_root_01) - all 90 prestige nodes had no effects. Also picks up the $..._description key those nodes carry
# v22: progression nodes carry their EFFECTS (name + description + ability refs from upgrade/upgrades/<key>.binfab, locale-resolved) alongside their costs, so a node row states what it gives as well as what it takes
# v21: geode companion level bonuses actually load - the tree lookup matched a filename pattern no file has, and pointed at trees/ (structure + costs) rather than upgrades/ (the per-level effects), so every companion has been served an empty levels list
# v20: the codex is relational - stats, abilities and typed relationship edges (crafts / ingredient / craftable_at / unlocks / upgrade_cost / member_of) get their own tables, plus badge requirements and progression-tree costs. Forces one rebuild to populate them
# v19: recipe product detection covers three more shapes it read as absent - a product path on a field the ingredients also use (banners, geode abilities), a product that IS a crafting material (conversion + gardening recipes), and the bare token costume/skin recipes name theirs with, resolved only against a prefab that exists
# v18: placeable models come from the game's OWN table (blocks/blocks.binfab) instead of the name convention, which cannot tell mirrored siblings apart and had every deco_arrow_*_left pointing at its right-facing twin; the convention stays only for the placeables that table omits
# v17: a blueprint that decodes to nothing no longer ends the search - the warhorse/bull mounts ship no whole model at all, only parts, and the parts their prefab names first are the empty banner slots, so they rendered blank; falls through to the largest part the same prefab names
# v16: placeable products resolve their model by the blueprint naming convention (placeable/deco/<stem> -> deco_<stem>, frameworks -> fw_<stem>), since a placeable prefab references no model at all - gated on the name matching EXACTLY ONE blueprint in the branch
# v15: recipe OUTPUT read from its own wire field (16 = item form, 1 = collection/equipment ref) instead of "first non-material path in the string scan", which named an ingredient as the product for 1,044 recipes and found none at all for 2,411; plus a last-quantified-record fallback for the plain crafting recipes that carry no product field
# v14: blueprint resolution - recipes borrow their output item's model (they reference none of their own), multi-part creatures prefer the whole-model "_ui" blueprint over a component part that renders blank, author-credited names survive the "," / " " between handles, and a run that matches no real blueprint file is dropped instead of stored as a name that can only 404

# Bumped when the rig extractor or its coverage changes - forces a rig-only rebuild on
# the next sync WITHOUT a (heavier) full codex re-parse.
RIG_PARSER_VERSION = 3  # v2: carry the source PREFAB onto every row - the creature's identity, which a shared skeleton + a flat blueprints/ folder cannot reconstruct
# v3: end a creature's mesh list at the NEXT .skeleton.gr2, not just the .gsf - a costume
# bundles the character with its transformed form and its pets, and 272 of 603 costumes were
# handing all of them to the character's own attach points (a werewolf head on the Lunar Lancer)

_FLUSH_AT = 1000

# Last manual-rebuild status per branch (in-memory; for the admin poll).
_REBUILD_STATUS: dict[str, dict] = {}
# The collection table that groups mounts (incl. dragons) by category.
_MOUNT_TABLE = PREFABS_ROOT + "collections/collection_mount.binfab"
# Per-item mastery multipliers (covers every collection type).
_MULTIPLIERS = PREFABS_ROOT + "meta/multipliers.binfab"
# Geode-mode mastery multipliers + the geode companion membership table.
_GEODE_MULTIPLIERS = PREFABS_ROOT + "meta/geode_multipliers.binfab"
_GEODE_TABLE = PREFABS_ROOT + "collections/collection_geodecompanion.binfab"
# The recipe catalogue membership table (category/group + which recipes are current).
_RECIPE_TABLE = PREFABS_ROOT + "collections/collection_recipe.binfab"
# The placeable/block -> model table: where the game states what a placeable looks like.
_BLOCKS_TABLE = PREFABS_ROOT + "blocks/blocks.binfab"
# The Power Rank tier -> rank join table.
_POWERRANK_TABLE = PREFABS_ROOT + "meta/powerrank.binfab"


@dataclass
class _Maps:
    """The lookup tables an extraction pass needs (loaded once per reindex)."""

    loc: dict[str, str] = field(default_factory=dict)
    mount_categories: dict[str, str] = field(default_factory=dict)
    multipliers: dict[str, dict] = field(default_factory=dict)
    geode_multipliers: dict[str, dict] = field(default_factory=dict)
    geode_members: dict[str, str] = field(default_factory=dict)
    upgrade_trees: dict[str, bytes] = field(default_factory=dict)
    # Recipe catalogue membership (recipe_id -> {category_key, group, order, …}) and
    # the inverted provider map (recipe_id -> [bench/profession rows]). Both are only
    # loaded when a recipe is actually being (re)parsed.
    recipe_catalogue: dict[str, dict] = field(default_factory=dict)
    recipe_providers: dict[str, list[dict]] = field(default_factory=dict)
    # Cross-prefab resolver (recipes): full prefab path -> source sha, + the store
    # to read them, with a memoized name/desc cache. Sync reads run inside to_thread.
    store: ContentStore | None = None
    prefab_shas: dict[str, str] = field(default_factory=dict)
    _item_meta: dict[str, dict] = field(default_factory=dict)
    _ability_meta: dict[str, dict] = field(default_factory=dict)
    valid_blueprints: set[str] = field(default_factory=set)
    # `blueprint basename without its [author] credit` -> blueprints carrying it, for the
    # placeable naming convention (see `extract.conventional_blueprint`).
    blueprint_stems: dict[str, list[str]] = field(default_factory=dict)
    # blueprint name (lowercased, rel to blueprints/) -> content sha, so `model_size` can
    # read a candidate's voxels out of the CAS.
    blueprint_shas: dict[str, str] = field(default_factory=dict)
    _model_size: dict[str, int] = field(default_factory=dict)
    # `placeable/block prefab path -> model name`, from `blocks/blocks.binfab`.
    block_models: dict[str, str] = field(default_factory=dict)
    # `tier marker -> Power Rank`, from `meta/powerrank.binfab` (the real join table
    # rather than the six tiers that were reverse-engineered by hand).
    power_rank_table: dict[int, int] = field(default_factory=dict)
    # `equipment id -> catalogue row`, from `loot/{hat,face,weapon_*,pvpbanner}.binfab`.
    # This is where a style's slot family and hat appearance base are STATED.
    style_rows: dict[str, dict] = field(default_factory=dict)

    def _read(self, rel: str) -> bytes | None:
        """Bytes of a referenced prefab by its logical path (rel to prefabs/, no ext)."""
        if self.store is None:
            return None
        key = rel.replace("\\", "/").removesuffix(".binfab")
        full = (key if key.startswith(PREFABS_ROOT) else PREFABS_ROOT + key) + ".binfab"
        sha = self.prefab_shas.get(full)
        return self.store.get(sha) if sha else None

    def prefab_exists(self, rel: str) -> bool:
        """Is there a prefab at this logical path? Gates the costume/skin product
        token, so a token only becomes a product path when the prefab is really there."""
        key = rel.replace("\\", "/").removesuffix(".binfab")
        full = (key if key.startswith(PREFABS_ROOT) else PREFABS_ROOT + key) + ".binfab"
        return full in self.prefab_shas

    def ability_meta(self, ref: str) -> dict:
        """Display text for an ability ref, read from ITS OWN prefab (memoized).

        Empty when the prefab isn't in the archive - the caller then shows no name,
        rather than a title-cased internal id."""
        norm = abilities.normalize_ref(ref)
        if not norm:
            return {}
        cached = self._ability_meta.get(norm)
        if cached is not None:
            return cached
        content = self._read(norm)
        meta = abilities.display(abilities.extract_keys(content), self.loc) if content else {}
        self._ability_meta[norm] = meta
        return meta

    def model_size(self, name: str) -> int:
        """Voxel count of a blueprint, 0 when it's an empty placeholder / unreadable.

        Memoized per name - the extractor asks about a couple of candidates per prefab,
        and the same part blueprints recur across a creature's whole family. The cheap
        ``is_empty_blueprint`` check short-circuits the placeholders, which is the case
        this exists to detect, so only real models pay for a decode."""
        from app.trove.render.voxel import decode, is_empty_blueprint, to_render_voxels
        key = name.replace("\\", "/").lower().removeprefix("blueprints/")
        cached = self._model_size.get(key)
        if cached is not None:
            return cached
        size = 0
        sha = self.blueprint_shas.get(key)
        raw = self.store.get(sha) if (sha and self.store is not None) else None
        if raw and not is_empty_blueprint(raw):
            try:
                size = len(to_render_voxels(decode(raw)))
            except Exception:  # noqa: BLE001 - an undecodable model is simply not usable
                size = 0
        self._model_size[key] = size
        return size

    def item_meta(self, rel: str) -> dict:
        """Resolve {name, desc, blueprint} for an item/collection prefab (memoized).
        Empty when the prefab or its locale keys can't be resolved.

        ``blueprint`` is what lets a recipe show its product: recipe prefabs carry no
        model reference at all, so the only way to a thumbnail is the output item's
        own prefab - which this already reads for the name."""
        norm = rel.replace("\\", "/").removesuffix(".binfab").lower()
        cached = self._item_meta.get(norm)
        if cached is not None:
            return cached
        meta = {"name": "", "desc": "", "blueprint": None}
        content = self._read(norm)
        if content:
            ident = binfab.decode_identity(content) or {}
            meta["name"] = self.loc.get(ident.get("name_key") or "", "") or ""
            meta["desc"] = self.loc.get(ident.get("desc_key") or "", "") or ""
            meta["blueprint"] = blueprint_ref(
                content, valid_blueprints=self.valid_blueprints, path=norm,
                model_size=self.model_size)
        if meta["blueprint"] is None:
            # Placeables reference no model of their own. The game's own table states
            # the mapping; the name convention is only for the placeables it omits.
            model = self.block_models.get(norm)
            if model:
                meta["blueprint"] = model_blueprint(
                    model, self.valid_blueprints, self.blueprint_stems)
            if meta["blueprint"] is None:
                meta["blueprint"] = conventional_blueprint(norm, self.blueprint_stems)
        self._item_meta[norm] = meta
        return meta


def _prefix_query(branch: str, prefix: str) -> dict:
    # All paths under `prefix` via an index-friendly range (￿ sorts after any char).
    return {"branch": branch, "path": {"$gte": prefix, "$lt": prefix + "￿"}}


async def load_locale_map(branch: str, store: ContentStore) -> dict[str, str]:
    """Merge every `languages/<en>/…` string table into one $key -> text map."""
    coll = UpdateState.get_pymongo_collection()
    rows = await coll.find(
        _prefix_query(branch, LOCALE_ROOT), {"path": 1, "content_sha256": 1, "_id": 0}
    ).to_list(length=None)
    loc: dict[str, str] = {}
    for row in rows:
        content = await asyncio.to_thread(store.get, row["content_sha256"])
        if content:
            loc.update(binfab.extract_localization_map(content))
    logger.info("codexes[%s]: locale map has %d keys (%d tables)", branch, len(loc), len(rows))
    return loc


async def _load_file(branch: str, store: ContentStore, path: str) -> bytes | None:
    """Fetch one archived file's bytes by logical path (None if absent)."""
    doc = await UpdateState.find_one({"branch": branch, "path": path})
    if doc is None:
        return None
    return await asyncio.to_thread(store.get, doc.content_sha256)


async def _load_upgrade_trees(branch: str, store: ContentStore) -> dict[str, bytes]:
    """Geode companion level BONUSES, keyed by the ref a companion prefab names
    (`gleemur_common_upgrade_tree`). Empty when the archive carries none.

    Two things were wrong here, and together they meant every geode companion has been
    served with an empty `levels` list for as long as this existed:

    - the path filter matched `*_upgrade_tree*.binfab`, but no file is named that. A
      companion prefab REFERENCES `<base>_upgrade_tree`; the files themselves are
      `prefabs/upgrade/{trees,upgrades}/<base>.binfab`. Nothing ever matched, so the
      map was always empty and `parse_upgrade_tree` was never called with anything.
    - even given the right directory, `trees/` is the wrong one. It holds a system's
      structure and its material costs (which `_index_upgrades` reads); the per-level
      stat and ability effects live in `upgrades/`. Pointing this at `trees/` returns
      zero levels for every companion.

    Keyed by `<stem>_upgrade_tree` so it joins directly to `find_upgrade_tree_ref`.
    """
    coll = UpdateState.get_pymongo_collection()
    rows = await coll.find(
        _prefix_query(branch, upgrades.UPGRADES_ROOT),
        {"path": 1, "content_sha256": 1, "_id": 0},
    ).to_list(length=None)
    trees: dict[str, bytes] = {}
    for row in rows:
        path = row["path"]
        if not path.endswith(".binfab"):
            continue
        content = await asyncio.to_thread(store.get, row["content_sha256"])
        if content:
            stem = path.rsplit("/", 1)[-1].removesuffix(".binfab")
            trees[f"{stem}_upgrade_tree"] = content
    return trees


async def _load_recipe_providers(branch: str, store: ContentStore,
                                 known_ids: set[str]) -> dict[str, list[dict]]:
    """`recipe_id -> [provider rows]` by reading the crafting-station + profession
    prefabs (`*_interactive`/`*_interactable` + `professions/`) and inverting the bare
    `recipe_*` tokens they list, matched against `known_ids` (the real recipe stems) so
    glued framing bytes are trimmed and non-recipes dropped. The path regex keeps this a
    bounded scan; a provider it misses just yields no benches for a recipe (never a wrong
    one).

    Note: a provider-prefab change alone won't refresh recipe rows on a delta (the map is
    rebuilt with the recipe parse); a parser bump / full rebuild reconciles it."""
    if not known_ids:
        return {}
    coll = UpdateState.get_pymongo_collection()
    rows = await coll.find(
        {"branch": branch, "path": {"$regex": providers.PROVIDER_PATH_RE.pattern,
                                    "$options": "i"}},
        {"path": 1, "content_sha256": 1, "_id": 0},
    ).to_list(length=None)
    prefabs: list[tuple[str, bytes]] = []
    for row in rows:
        path = row["path"]
        if not (path.startswith(PREFABS_ROOT) and path.endswith(".binfab")):
            continue
        content = await asyncio.to_thread(store.get, row["content_sha256"])
        if content:
            prefabs.append((path, content))
    return providers.build_provider_map(prefabs, known_ids)


async def _load_style_catalogues(branch: str, store: ContentStore) -> dict[str, dict]:
    """`equipment id -> catalogue row` from every `prefabs/loot/` style catalogue."""
    coll = UpdateState.get_pymongo_collection()
    docs = await coll.find(
        _prefix_query(branch, loot.LOOT_ROOT), {"path": 1, "content_sha256": 1, "_id": 0}
    ).to_list(length=None)
    catalogues: dict[str, bytes] = {}
    for doc in docs:
        path = doc["path"]
        if not loot.is_style_catalogue(path):
            continue
        content = await asyncio.to_thread(store.get, doc["content_sha256"])
        if content:
            catalogues[path] = content
    return await asyncio.to_thread(loot.style_index, catalogues) if catalogues else {}


async def _load_prefab_shas(branch: str) -> dict[str, str]:
    """Full `prefab path -> source sha` map for the branch (the recipe resolver's
    index into the archive). Projected (path+sha only), so it's a cheap scan."""
    coll = UpdateState.get_pymongo_collection()
    rows = await coll.find(
        _prefix_query(branch, PREFABS_ROOT), {"path": 1, "content_sha256": 1, "_id": 0}
    ).to_list(length=None)
    return {r["path"]: r["content_sha256"] for r in rows}


async def _load_valid_blueprints(branch: str, shas: dict[str, str] | None = None) -> set[str]:
    """All valid blueprint paths (lowercased, relative to blueprints/) in the branch.

    Pass ``shas`` to collect ``name -> content sha`` in the same pass - that's what lets
    the extractor ask whether a candidate blueprint actually has anything to draw."""
    blueprints = set()
    coll = UpdateState.get_pymongo_collection()
    cursor = coll.find(
        {"branch": branch, "path": {"$regex": "^blueprints/"}},
        {"path": 1, "content_sha256": 1, "_id": 0}
    )
    async for row in cursor:
        path = row["path"]
        if path.startswith("blueprints/"):
            path = path[len("blueprints/"):]
        blueprints.add(path.lower())
        if shas is not None and row.get("content_sha256"):
            shas[path.lower()] = row["content_sha256"]

    root = settings.trove_local_game_dir
    if root:
        from pathlib import Path
        bp_dir = Path(root) / "blueprints"
        if bp_dir.is_dir():
            try:
                for p in bp_dir.rglob("*.blueprint"):
                    try:
                        rel = p.relative_to(bp_dir).as_posix()
                        blueprints.add(rel.lower())
                    except ValueError:
                        continue
            except Exception:
                pass
    return blueprints


async def _load_maps(branch: str, store: ContentStore, *, with_resolver: bool = True) -> _Maps:
    """Load every lookup table the extractors need for `branch`. ``with_resolver``
    additionally loads the prefab-sha map used to resolve referenced item names
    (recipes); skip it for a delta with no recipes to avoid the extra scan."""
    mount_table = await _load_file(branch, store, _MOUNT_TABLE)
    multipliers = await _load_file(branch, store, _MULTIPLIERS)
    geode_multipliers = await _load_file(branch, store, _GEODE_MULTIPLIERS)
    geode_table = await _load_file(branch, store, _GEODE_TABLE)
    geode_members = geode.geode_companion_members(geode_table) if geode_table else {}
    # The recipe catalogue + provider maps are only needed to parse recipes; skip both
    # for a delta that touched no recipe (same gate as the cross-prefab name resolver).
    recipe_table = await _load_file(branch, store, _RECIPE_TABLE) if with_resolver else None
    prefab_shas = await _load_prefab_shas(branch) if with_resolver else {}
    # Authoritative recipe-id set (the recipes/ stems) - lets the provider scan trim the
    # glued framing bytes off the bare `recipe_*` tokens the bench prefabs list.
    recipes_root = PREFABS_ROOT + "recipes/"
    known_recipe_ids = {
        p[len(recipes_root):].removesuffix(".binfab").lower()
        for p in prefab_shas if p.startswith(recipes_root) and p.endswith(".binfab")
    }
    blocks_table = await _load_file(branch, store, _BLOCKS_TABLE)
    pr_table = await _load_file(branch, store, _POWERRANK_TABLE)
    style_rows = await _load_style_catalogues(branch, store)
    bp_shas: dict[str, str] = {}
    valid = await _load_valid_blueprints(branch, bp_shas)
    maps = _Maps(
        loc=await load_locale_map(branch, store),
        mount_categories=binfab.collection_category_map(mount_table) if mount_table else {},
        multipliers=mastery.parse_multipliers(multipliers) if multipliers else {},
        geode_multipliers=mastery.parse_geode_multipliers(geode_multipliers) if geode_multipliers else {},
        geode_members=geode_members,
        upgrade_trees=await _load_upgrade_trees(branch, store) if geode_members else {},
        recipe_catalogue=catalogue.parse_recipe_catalogue(recipe_table, known_recipe_ids) if recipe_table else {},
        recipe_providers=await _load_recipe_providers(branch, store, known_recipe_ids),
        store=store,
        prefab_shas=prefab_shas,
        valid_blueprints=valid,
        blueprint_stems=blueprint_stems(valid),
        blueprint_shas=bp_shas,
        block_models=blocks.parse_block_models(blocks_table) if blocks_table else {},
        power_rank_table=powerrank.parse_power_rank_table(pr_table) if pr_table else {},
        style_rows=style_rows,
    )
    logger.info("codexes[%s]: %d mount categories, %d mastery rows, %d geode rows, "
                "%d geode members, %d upgrade trees, %d prefab refs, %d blueprints, "
                "%d recipe catalogue, %d recipe providers, %d block models", branch,
                len(maps.mount_categories), len(maps.multipliers), len(maps.geode_multipliers),
                len(maps.geode_members), len(maps.upgrade_trees), len(maps.prefab_shas),
                len(maps.valid_blueprints), len(maps.recipe_catalogue), len(maps.recipe_providers),
                len(maps.block_models))
    return maps


class _Batch:
    """One flush unit: entry rows plus the child rows they own.

    The children are replaced SCOPED BY PATH rather than upserted, so a prefab that
    stops granting a stat actually loses the row. `paths` therefore has to include
    every prefab that was re-parsed - including ones that now yield no children at
    all - or their stale rows would survive forever."""

    def __init__(self) -> None:
        self.entries: list[tuple] = []
        self.paths: list[str] = []
        self.stats: list[tuple] = []
        self.abilities: list[tuple] = []
        self.links: list[tuple] = []

    def add(self, parsed: tuple) -> None:
        entry_row, stats, abilities, link_rows = parsed
        self.entries.append(entry_row)
        self.paths.append(entry_row[1])          # COLUMNS[1] == path
        self.stats.extend(stats)
        self.abilities.extend(abilities)
        self.links.extend(link_rows)

    def __len__(self) -> int:
        return len(self.entries)


async def _flush(batch: _Batch) -> None:
    if not batch.entries:
        return
    branch = batch.entries[0][0]                 # COLUMNS[0] == branch
    await pg_store.upsert_entries(batch.entries)
    await pg_store.replace_children(branch, batch.paths, stats=batch.stats,
                                    abilities=batch.abilities, links=batch.links)


def _attach_geode_companion(entry: dict, rel: str, content: bytes, maps: _Maps) -> None:
    """For an `item/companion/…` prefab, attach `data.geode_companion`: its rarity,
    upgrade-tree ref, and (when the tree binfab is in the archive) per-level bonuses
    with their `$…` stat/ability keys resolved to text."""
    ref = geode.find_upgrade_tree_ref(content)
    rarity = maps.geode_members.get(rel.lower())
    if not ref and not rarity:
        return
    tree = maps.upgrade_trees.get(ref) if ref else None
    levels = geode.parse_upgrade_tree(tree) if tree else []
    for level in levels:
        for stat in level.get("stats", []):
            stat["stat_name"] = localize.resolve_stat_name(maps.loc, stat.get("stat"))
        for ability in level.get("abilities", []):
            text = localize.resolve_text(maps.loc, ability.get("key"))
            if text:
                ability["description"] = text
    entry.setdefault("data", {})["geode_companion"] = {
        "upgrade_tree": ref, "rarity": rarity, "levels": levels,
    }


def _attach_recipe_catalogue_and_providers(rdata: dict, rel: str, maps: _Maps) -> None:
    """Attach catalogue membership + provider/bench rows to a recipe's `data.recipe`.

    Both are additive and never touch the output-derived `category`: `in_catalogue`
    distinguishes a current catalogue member from a source-only recipe (handoff:
    "Missing from catalogue is not delete evidence."), and `providers` lists the
    benches/professions that craft it."""
    stem = rel.replace("\\", "/").rsplit("/", 1)[-1].lower()
    cat = maps.recipe_catalogue.get(stem)
    rdata["in_catalogue"] = bool(cat)
    if cat:
        rdata["catalogue_order"] = cat["order"]
    provs = maps.recipe_providers.get(stem)
    if provs:
        rdata["providers"] = provs


def _parse_entry(branch: str, path: str, sha: str, ctype: str, maps: _Maps, now) -> tuple | None:
    """Read + parse one prefab into a `(entry row, stat rows, ability rows, link rows)`
    bundle (None if the blob is missing).

    Runs inside ``to_thread`` - all the blocking reads (the prefab itself plus
    recipe/companion cross-references) happen off the event loop. The child rows are a
    projection of the SAME decode that fills `data`, never a second parse."""
    content = maps.store.get(sha) if maps.store is not None else None
    if content is None:
        return None
    entry = extract_entry(ctype, path, content, maps.loc, resolve_meta=maps.item_meta,
                          valid_blueprints=maps.valid_blueprints, model_size=maps.model_size,
                          prefab_exists=maps.prefab_exists,
                          power_rank_table=maps.power_rank_table,
                          style_rows=maps.style_rows,
                          resolve_ability=maps.ability_meta)
    rel = path[len(PREFABS_ROOT):].removesuffix(".binfab")
    if ctype == "mount":  # split dragons out by their collection category
        refine_mount(entry, rel, maps.mount_categories)
    if ctype == "recipe":
        # Recipe mastery is row-local, not a per-type base: the trusted structural
        # prefab byte (multipliers row overrides). No-byte/conflicting -> None.
        info = recipe.resolve_recipe_mastery(rel, content, maps.multipliers)
        entry["mastery"] = info["value"]
        rdata = entry.setdefault("data", {}).setdefault("recipe", {})
        rdata["mastery"] = info["value"]
        rdata["mastery_source"] = info["source"]
        if info["prefab_byte"] is not None:
            rdata["mastery_prefab_byte"] = info["prefab_byte"]
        _attach_recipe_catalogue_and_providers(rdata, rel, maps)
        entry["mastery_geode"] = mastery.geode_mastery_for(rel, maps.geode_multipliers)
    else:
        # Styles (equipment/) resolve through the standard path: the EquipmentAppearance
        # base (1) unless a multipliers row scales it.
        entry["mastery"] = mastery.mastery_for(rel, maps.multipliers)
        entry["mastery_geode"] = mastery.geode_mastery_for(rel, maps.geode_multipliers)
    if rel.lower().startswith("item/companion/"):
        _attach_geode_companion(entry, rel, content, maps)
    return (
        to_row(entry, branch, sha, now),
        stat_rows(entry, branch),
        ability_rows(entry, branch),
        links.recipe_links(entry, branch) if ctype == "recipe" else [],
    )


# The single files (and one directory) that feed the branch-scoped tables. A delta
# only rebuilds those tables when it actually touched one of these.
_BADGES_TABLE = PREFABS_ROOT + "meta/badges.binfab"
_UNLOCKS_TABLE = PREFABS_ROOT + "collections/unlocks.binfab"


def _shared_sources_touched(changes: list[dict]) -> bool:
    """Did this version's change list include a shared-table source?"""
    for change in changes:
        path = change.get("path") or ""
        if path in (_BADGES_TABLE, _UNLOCKS_TABLE, badges.EXE_PATH):
            return True
        if path.startswith(upgrades.TREES_ROOT):
            return True
    return False


async def _load_metric_names(branch: str, store: ContentStore) -> list[str]:
    """The `PlayerMetric` enum from the client executable, indexed by ordinal.

    The archive mirrors the exe, so this stays current instead of drifting like a
    hardcoded table would. It's ~20MB read once per rebuild; without it the metric
    badges still decode their amount and are stored `blocked`, never guessed."""
    exe = await _load_file(branch, store, badges.EXE_PATH)
    if not exe:
        logger.warning("codexes[%s]: %s absent - badge metric names unresolved",
                       branch, badges.EXE_PATH)
        return []
    return await asyncio.to_thread(badges.parse_metric_names, exe)


async def _index_badges(branch: str, store: ContentStore) -> int:
    data = await _load_file(branch, store, _BADGES_TABLE)
    if not data:
        return 0
    metric_names = await _load_metric_names(branch, store)
    parsed = await asyncio.to_thread(badges.parse_badges, data, metric_names)
    if parsed["errors"]:
        logger.warning("codexes[%s]: badge decode reported %d issue(s): %s",
                       branch, len(parsed["errors"]), parsed["errors"][:3])
    return await pg_store.replace_requirements(
        branch, requirement_rows(parsed["rows"], branch))


def _resolve_effects(effects: dict[str, dict], loc: dict[str, str]) -> dict[str, dict]:
    """Resolve each node's `$…_name` key to its display text.

    Only the name is resolved. The ability refs are kept as refs: the `$…_description`
    key shape that works for collection bonuses is a convention of the combat/inventory
    spawners, and there is no evidence it applies to `abilities/discovery/*`. Deriving a
    key here would resolve to nothing on every row while looking like it had tried, so
    the refs are stored as the evidence they are."""
    out: dict[str, dict] = {}
    for node_key, effect in effects.items():
        name_key = effect.get("name_key") or ""
        desc_key = effect.get("desc_key") or ""
        out[node_key] = {
            "name_key": name_key,
            "name": localize.resolve_text(loc, name_key) if name_key else "",
            "desc_key": desc_key,
            "description": localize.resolve_text(loc, desc_key) if desc_key else "",
            "abilities": list(effect.get("abilities") or []),
        }
    return out


async def _index_upgrades(branch: str, store: ContentStore, loc: dict[str, str]) -> tuple[int, int]:
    """`(node count, cost-edge count)` for every progression tree in the branch.

    A system is described by TWO files: `trees/<key>.binfab` (structure + material
    costs) and `upgrades/<key>.binfab` (what each node grants). Both are read here so a
    node row can say what it takes and what it gives."""
    coll = UpdateState.get_pymongo_collection()
    docs = await coll.find(
        _prefix_query(branch, upgrades.TREES_ROOT), {"path": 1, "content_sha256": 1, "_id": 0}
    ).to_list(length=None)
    effect_docs = await coll.find(
        _prefix_query(branch, upgrades.UPGRADES_ROOT), {"path": 1, "content_sha256": 1, "_id": 0}
    ).to_list(length=None)
    effect_shas = {d["path"]: d["content_sha256"] for d in effect_docs}

    nodes: list[tuple] = []
    edges: list[tuple] = []
    for doc in docs:
        path = doc["path"]
        if not path.endswith(".binfab"):
            continue
        content = await asyncio.to_thread(store.get, doc["content_sha256"])
        if not content:
            continue
        parsed = await asyncio.to_thread(upgrades.parse_upgrade_costs, content, path)
        if not parsed["nodes"]:
            continue
        key = parsed["system_key"]
        effects: dict[str, dict] = {}
        sha = effect_shas.get(f"{upgrades.UPGRADES_ROOT}{key}.binfab")
        if sha:
            raw = await asyncio.to_thread(store.get, sha)
            if raw:
                node_keys = [n["node_key"] for n in parsed["nodes"]]
                effects = _resolve_effects(
                    await asyncio.to_thread(upgrades.parse_upgrade_effects, raw, node_keys), loc)
        nodes.extend(upgrade_rows(parsed, branch, path, effects))
        edges.extend(links.upgrade_links(parsed, branch, path))
    await pg_store.replace_upgrades(branch, nodes)
    await pg_store.replace_links_for(branch, links.UPGRADE_COST, edges)
    return len(nodes), len(edges)


async def _index_unlocks(branch: str, store: ContentStore) -> int:
    data = await _load_file(branch, store, _UNLOCKS_TABLE)
    if not data:
        return 0
    pairs = await asyncio.to_thread(unlocks.parse_unlocks, data)
    return await pg_store.replace_links_for(
        branch, links.UNLOCKS, links.unlock_links(pairs, branch))


async def _index_membership(branch: str, store: ContentStore) -> int:
    """`member_of` edges from every `collections/collection_*.binfab` catalogue."""
    coll = UpdateState.get_pymongo_collection()
    docs = await coll.find(
        _prefix_query(branch, PREFABS_ROOT + "collections/collection_"),
        {"path": 1, "content_sha256": 1, "_id": 0},
    ).to_list(length=None)
    edges: list[tuple] = []
    for doc in docs:
        path = doc["path"]
        if not path.endswith(".binfab"):
            continue
        content = await asyncio.to_thread(store.get, doc["content_sha256"])
        if not content:
            continue
        members = await asyncio.to_thread(binfab.collection_category_map, content)
        edges.extend(links.membership_links(members, path, branch))
    return await pg_store.replace_links_for(branch, links.MEMBER_OF, edges)


async def _index_shared_tables(branch: str, store: ContentStore, maps: _Maps) -> dict:
    """Rebuild the branch-scoped tables that come from single source files.

    Each is independent, so one failing source (a file the archive doesn't carry yet)
    leaves the others intact rather than aborting the whole index."""
    requirements = await _index_badges(branch, store)
    nodes, cost_edges = await _index_upgrades(branch, store, maps.loc)
    unlock_edges = await _index_unlocks(branch, store)
    member_edges = await _index_membership(branch, store)
    logger.info(
        "codexes[%s]: shared tables - %d badge requirements, %d upgrade nodes, "
        "%d cost edges, %d unlock edges, %d membership edges",
        branch, requirements, nodes, cost_edges, unlock_edges, member_edges,
    )
    return {"requirements": requirements, "upgrade_nodes": nodes,
            "link_edges": cost_edges + unlock_edges + member_edges}


async def reindex(branch: str, store: ContentStore, *, force: bool = False) -> dict:
    """Full (re)build for `branch`. By default a prefab whose source sha is unchanged
    vs. the stored row is skipped; ``force`` re-parses every prefab regardless (used
    after a parser change - the UPSERT overwrites in place, so no empty window)."""
    maps = await _load_maps(branch, store)
    # path -> current source sha: skip unchanged prefabs (unless forced) and prune
    # removed ones.
    existing = await pg_store.existing_shas(branch)

    coll = UpdateState.get_pymongo_collection()
    cursor = coll.find(
        _prefix_query(branch, PREFABS_ROOT), {"path": 1, "content_sha256": 1, "_id": 0}
    )

    batch = _Batch()
    counts = {"indexed": 0, "unchanged": 0, "missing_blob": 0, "removed": 0}
    seen: set[str] = set()
    now = utcnow()

    async for row in cursor:
        path, sha = row["path"], row["content_sha256"]
        ctype = classify(path)
        if ctype is None:
            continue
        seen.add(path)
        if not force and existing.get(path) == sha:
            counts["unchanged"] += 1
            continue
        parsed = await asyncio.to_thread(_parse_entry, branch, path, sha, ctype, maps, now)
        if parsed is None:
            counts["missing_blob"] += 1
            continue
        batch.add(parsed)
        counts["indexed"] += 1
        if len(batch) >= _FLUSH_AT:
            await _flush(batch)
            batch = _Batch()
    await _flush(batch)

    stale = [p for p in existing if p not in seen]
    if stale:
        await pg_store.delete_entries(branch, stale)
        counts["removed"] = len(stale)

    # The shared tables (badge requirements, progression trees, unlock edges) come from
    # single source files rather than from the per-prefab walk, so they are rebuilt
    # wholesale here - cheap, and it keeps them consistent with the entries just written.
    counts.update(await _index_shared_tables(branch, store, maps))

    # The branch is now built by the current parser - record it so a later parser
    # bump (not a game update) can tell this branch is stale and rebuild it.
    await pg_store.set_parser_version(branch, CODEX_PARSER_VERSION)

    logger.info(
        "codexes[%s]: %s indexed=%d unchanged=%d removed=%d missing_blob=%d",
        branch, "forced rebuild" if force else "full",
        counts["indexed"], counts["unchanged"], counts["removed"], counts["missing_blob"],
    )
    return counts


async def reindex_changes(branch: str, store: ContentStore, ordinal: int) -> dict:
    """Apply just the delta of one new version: only codex-relevant changes touched."""
    rows = await UpdateChange.get_pymongo_collection().find(
        {"branch": branch, "ordinal": ordinal},
        {"path": 1, "type": 1, "content_sha256": 1, "_id": 0},
    ).to_list(length=None)
    touched = [r for r in rows if classify(r["path"]) is not None]
    counts = {"indexed": 0, "removed": 0, "missing_blob": 0}
    # The shared-table sources are NOT codex-classified paths, so a patch that changes
    # only `meta/badges.binfab` has an empty `touched` and would otherwise return here
    # with the requirements left at their previous version.
    shared_touched = _shared_sources_touched(rows)
    if not touched and not shared_touched:
        return counts
    if not touched:
        counts.update(await _index_shared_tables(branch, store, await _load_maps(
            branch, store, with_resolver=False)))
        await pg_store.touch_meta(branch)
        return counts

    # The lookup tables are only needed to (re)parse prefabs - skip the loads for a
    # delta that's pure removals. The recipe resolver's prefab-sha scan is only
    # loaded when a recipe actually changed (it reads referenced item prefabs).
    parse_rows = [r for r in touched if r["type"] != "removed" and r.get("content_sha256")]
    needs_parse = bool(parse_rows)
    needs_resolver = any(classify(r["path"]) == "recipe" for r in parse_rows)
    maps = await _load_maps(branch, store, with_resolver=needs_resolver) if needs_parse else _Maps()
    now = utcnow()
    batch = _Batch()
    removed: list[str] = []

    for r in touched:
        path = r["path"]
        if r["type"] == "removed":
            removed.append(path)
            counts["removed"] += 1
            continue
        sha = r.get("content_sha256")
        if not sha:
            counts["missing_blob"] += 1
            continue
        parsed = await asyncio.to_thread(_parse_entry, branch, path, sha, classify(path), maps, now)  # type: ignore[arg-type]
        if parsed is None:
            counts["missing_blob"] += 1
            continue
        batch.add(parsed)
        counts["indexed"] += 1
        if len(batch) >= _FLUSH_AT:
            await _flush(batch)
            batch = _Batch()
    await _flush(batch)
    await pg_store.delete_entries(branch, removed)

    # Rebuild a shared table only when THIS delta touched one of its source files - a
    # routine patch usually touches none of them, and rebuilding all three on every
    # version would undo the point of the delta path.
    if shared_touched:
        counts.update(await _index_shared_tables(branch, store, maps))
    # Bump the branch's indexed-at so consumers keyed on it (e.g. the Mods Hub rig-map
    # cache) refresh after a game patch, even though a delta leaves parser_version put.
    await pg_store.touch_meta(branch)

    logger.info(
        "codexes[%s]: delta ordinal=%s indexed=%d removed=%d missing_blob=%d",
        branch, ordinal, counts["indexed"], counts["removed"], counts["missing_blob"],
    )
    return counts


def _index_decision(*, has_any: bool, stored_version: int, current_version: int,
                    changed: bool, ordinal) -> str:
    """What the post-sync hook should do: ``full`` (empty branch), ``rebuild`` (the
    parser advanced - re-parse everything), ``delta`` (a new game version), or
    ``noop``. Pure, so the precedence is unit-testable."""
    if not has_any:
        return "full"
    if stored_version < current_version:
        return "rebuild"
    if changed and ordinal is not None:
        return "delta"
    return "noop"


async def ensure_indexed(branch: str, store: ContentStore, summary: dict) -> dict:
    """Post-sync hook: full bootstrap when the branch is empty, a forced rebuild when
    the parser advanced since the last build, else the version delta (or nothing).
    Also rebuilds the rig map (Mods Hub 3D) whenever anything changed."""
    if not settings.postgres_enabled:
        logger.warning("codexes[%s]: Postgres disabled - skipping index", branch)
        return {"indexed": 0, "removed": 0, "missing_blob": 0}
    decision = _index_decision(
        has_any=await pg_store.has_any(branch),
        stored_version=await pg_store.get_parser_version(branch),
        current_version=CODEX_PARSER_VERSION,
        changed=bool(summary.get("changed")),
        ordinal=summary.get("ordinal"),
    )
    if decision == "full":
        counts = await reindex(branch, store)
    elif decision == "rebuild":
        logger.info("codexes[%s]: parser advanced to v%d - rebuilding", branch, CODEX_PARSER_VERSION)
        counts = await reindex(branch, store, force=True)
    elif decision == "delta":
        counts = await reindex_changes(branch, store, summary["ordinal"])
    else:
        counts = {"indexed": 0, "removed": 0, "missing_blob": 0}

    # Rig map (Mods Hub 3D): a full rebuild when the codex did a full/rebuild, when the
    # rig extractor advanced, or on a first build onto an already-synced archive; just
    # the changed prefabs on a routine game delta. Isolated - a rig failure must not
    # derail the codex/archiver.
    try:
        rig_stale = (await pg_store.get_rig_version(branch) < RIG_PARSER_VERSION
                     or await pg_store.rig_binding_count(branch) == 0)
        if decision in ("full", "rebuild") or rig_stale:
            await reindex_rigs(branch, store)
        elif decision == "delta":
            await reindex_rigs_changes(branch, store, summary["ordinal"])
    except Exception:  # noqa: BLE001 - a rig failure must not derail the codex/archiver
        logger.warning("codexes[%s]: rig reindex failed", branch, exc_info=True)
    return counts


def _rig_rows(branch: str, store: ContentStore, candidates: list[tuple[str, str]]) -> list[tuple]:
    """``(branch, prefab, blueprint, skeleton, ap_key)`` rows for the given (path, sha)
    prefabs. Reads each blob, cheap-prefilters on ``.skeleton.gr2``, extracts
    structurally. Sync (run in a thread): the blob reads + parse are the heavy part.

    ONE prefab is ONE creature - ``extract_rig_refs`` returns exactly that creature's
    part set - so the prefab path is carried onto every row as the creature's identity.
    Nothing downstream can reconstruct it (a skeleton is shared by every variant that
    uses it, and the blueprints all live in the same flat folder), so dropping it here
    is what made the embed assemble a chimera."""
    rows: list[tuple] = []
    for path, sha in candidates:
        content = store.get(sha)
        if not content or b".skeleton.gr2" not in content:
            continue                           # cheap pre-filter: no skeleton, no rig
        rig = binfab.extract_rig_refs(content)
        if not rig:
            continue
        skeleton = rig["skeleton"]
        rows.extend((branch, path, bp, skeleton, ap) for bp, ap in rig["parts"].items())
    return rows


async def reindex_rigs(branch: str, store: ContentStore) -> int:
    """Full rebuild of the ``rig_binding`` map: read EVERY prefab and extract its
    ``blueprint basename -> (skeleton, AP)`` structurally. Comprehensive on purpose -
    skeletons live not just in collections/ (mounts + allies' inline _npc) but also
    skins/ (player costumes), npc/ (mobs), placeable/, item/ (item-mounts), etc., so we
    scan the whole prefab tree (cheap `.skeleton.gr2` pre-filter skips the ~85% that
    have none). A mod then resolves whenever the game itself defines the rig - no
    folder allow-list to keep in sync, no name guessing."""
    coll = UpdateState.get_pymongo_collection()
    docs = await coll.find(
        _prefix_query(branch, PREFABS_ROOT), {"path": 1, "content_sha256": 1, "_id": 0}
    ).to_list(length=None)
    candidates = [(d["path"], d["content_sha256"]) for d in docs]
    rows = await asyncio.to_thread(_rig_rows, branch, store, candidates)
    n = await pg_store.replace_rig_bindings(branch, rows)
    await pg_store.set_rig_version(branch, RIG_PARSER_VERSION)
    await pg_store.touch_meta(branch)          # invalidate the Mods Hub rig-map cache
    logger.info("codexes[%s]: rig map rebuilt - %d bindings from %d prefabs",
                branch, n, len(candidates))
    return n


async def reindex_rigs_changes(branch: str, store: ContentStore, ordinal: int) -> int:
    """Steady-state rig update for one game version: re-extract only the prefabs that
    changed and re-state their bindings.

    Scoped by PREFAB, so a creature that loses a part in an update actually loses it -
    now that the prefab is part of the key, a stale row would otherwise keep being
    assembled onto that one creature until the next full rebuild. Removed prefabs are
    included in the delete set (they re-state to nothing)."""
    rows = await UpdateChange.get_pymongo_collection().find(
        {"branch": branch, "ordinal": ordinal},
        {"path": 1, "type": 1, "content_sha256": 1, "_id": 0},
    ).to_list(length=None)
    touched = [r for r in rows
               if r["path"].startswith(PREFABS_ROOT) and r["path"].endswith(".binfab")]
    prefabs = [r["path"] for r in touched]
    changed = [(r["path"], r["content_sha256"]) for r in touched
               if r["type"] != "removed" and r.get("content_sha256")]
    if not prefabs:
        return 0
    new_rows = await asyncio.to_thread(_rig_rows, branch, store, changed)
    n = await pg_store.replace_prefab_rig_bindings(branch, prefabs, new_rows)
    await pg_store.touch_meta(branch)
    logger.info("codexes[%s]: rig delta ordinal=%s - %d bindings from %d re-parsed "
                "prefabs (%d re-stated)", branch, ordinal, n, len(changed), len(prefabs))
    return n


def get_rebuild_status(branch: str) -> dict:
    """Last manual-rebuild status for a branch (for the admin poll)."""
    return _REBUILD_STATUS.get(branch) or {
        "running": False, "started_at": None, "finished_at": None, "counts": None, "error": None,
    }


async def rebuild(branch: str, store: ContentStore) -> dict:
    """Manual force-rebuild of a branch (admin action). Force re-parses every prefab
    with the current parser; tracks status in-memory for the admin poll."""
    started = utcnow().isoformat()
    _REBUILD_STATUS[branch] = {"running": True, "started_at": started,
                               "finished_at": None, "counts": None, "error": None}
    try:
        counts = await reindex(branch, store, force=True)
        await reindex_rigs(branch, store)      # rig map too, so the admin button refreshes both
    except Exception as exc:  # noqa: BLE001 - surface the failure to the poll, don't crash the task
        logger.exception("codexes[%s]: manual rebuild failed", branch)
        _REBUILD_STATUS[branch] = {"running": False, "started_at": started,
                                   "finished_at": utcnow().isoformat(), "counts": None, "error": str(exc)}
        return {"error": str(exc)}
    _REBUILD_STATUS[branch] = {"running": False, "started_at": started,
                               "finished_at": utcnow().isoformat(), "counts": counts, "error": None}
    return counts

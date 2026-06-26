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
from app.trove.codexes import binfab, geode, localize, mastery, pg_store
from app.trove.codexes.extract import extract_entry, refine_mount
from app.trove.codexes.models import to_row
from app.trove.codexes.types import LOCALE_ROOT, PREFABS_ROOT, classify
from app.trove.updates.cas import ContentStore
from app.trove.updates.models import UpdateChange, UpdateState

logger = logging.getLogger("kiwi.trove.codexes")

# Bump this whenever the parser/extraction logic changes (new fields, fixed decode,
# resolved strings, …). On the next sync the indexer force-rebuilds any branch whose
# stored version is behind, so a parser change reaches the data WITHOUT a game update
# or a manual rebuild - the steady-state delta only re-touches changed game files.
CODEX_PARSER_VERSION = 7  # v7: rig map moved to its own rig_binding table (reindex_rigs); codex entries drop data['rig']

# Bumped when the rig extractor or its coverage changes - forces a rig-only rebuild on
# the next sync WITHOUT a (heavier) full codex re-parse.
RIG_PARSER_VERSION = 1  # v1: scan the WHOLE prefab tree (skins/npc/placeable/item/…), not just collections

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


@dataclass
class _Maps:
    """The lookup tables an extraction pass needs (loaded once per reindex)."""

    loc: dict[str, str] = field(default_factory=dict)
    mount_categories: dict[str, str] = field(default_factory=dict)
    multipliers: dict[str, dict] = field(default_factory=dict)
    geode_multipliers: dict[str, dict] = field(default_factory=dict)
    geode_members: dict[str, str] = field(default_factory=dict)
    upgrade_trees: dict[str, bytes] = field(default_factory=dict)
    # Cross-prefab resolver (recipes): full prefab path -> source sha, + the store
    # to read them, with a memoized name/desc cache. Sync reads run inside to_thread.
    store: ContentStore | None = None
    prefab_shas: dict[str, str] = field(default_factory=dict)
    _item_meta: dict[str, dict] = field(default_factory=dict)
    valid_blueprints: set[str] = field(default_factory=set)


    def _read(self, rel: str) -> bytes | None:
        """Bytes of a referenced prefab by its logical path (rel to prefabs/, no ext)."""
        if self.store is None:
            return None
        key = rel.replace("\\", "/").removesuffix(".binfab")
        full = (key if key.startswith(PREFABS_ROOT) else PREFABS_ROOT + key) + ".binfab"
        sha = self.prefab_shas.get(full)
        return self.store.get(sha) if sha else None

    def item_meta(self, rel: str) -> dict:
        """Resolve {name, desc} for an item/collection prefab (memoized). Empty when
        the prefab or its locale keys can't be resolved."""
        norm = rel.replace("\\", "/").removesuffix(".binfab").lower()
        cached = self._item_meta.get(norm)
        if cached is not None:
            return cached
        meta = {"name": "", "desc": ""}
        content = self._read(norm)
        if content:
            ident = binfab.decode_identity(content) or {}
            meta["name"] = self.loc.get(ident.get("name_key") or "", "") or ""
            meta["desc"] = self.loc.get(ident.get("desc_key") or "", "") or ""
        self._item_meta[norm] = meta
        return meta


def _prefix_query(branch: str, prefix: str) -> dict:
    # All paths under `prefix` via an index-friendly range (￿ sorts after any char).
    return {"branch": branch, "path": {"$gte": prefix, "$lt": prefix + "￿"}}


async def _load_locale_map(branch: str, store: ContentStore) -> dict[str, str]:
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
    """Geode companion upgrade-tree binfabs, keyed by stem (e.g.
    `gleemur_common_upgrade_tree`). Empty when the archive carries none."""
    coll = UpdateState.get_pymongo_collection()
    rows = await coll.find(
        {"branch": branch, "path": {"$regex": r"_upgrade_tree[^/]*\.binfab$"}},
        {"path": 1, "content_sha256": 1, "_id": 0},
    ).to_list(length=None)
    trees: dict[str, bytes] = {}
    for row in rows:
        content = await asyncio.to_thread(store.get, row["content_sha256"])
        if content:
            trees[row["path"].rsplit("/", 1)[-1].removesuffix(".binfab")] = content
    return trees


async def _load_prefab_shas(branch: str) -> dict[str, str]:
    """Full `prefab path -> source sha` map for the branch (the recipe resolver's
    index into the archive). Projected (path+sha only), so it's a cheap scan."""
    coll = UpdateState.get_pymongo_collection()
    rows = await coll.find(
        _prefix_query(branch, PREFABS_ROOT), {"path": 1, "content_sha256": 1, "_id": 0}
    ).to_list(length=None)
    return {r["path"]: r["content_sha256"] for r in rows}


async def _load_valid_blueprints(branch: str) -> set[str]:
    """All valid blueprint paths (lowercased, relative to blueprints/) in the branch."""
    blueprints = set()
    coll = UpdateState.get_pymongo_collection()
    cursor = coll.find(
        {"branch": branch, "path": {"$regex": "^blueprints/"}},
        {"path": 1, "_id": 0}
    )
    async for row in cursor:
        path = row["path"]
        if path.startswith("blueprints/"):
            path = path[len("blueprints/"):]
        blueprints.add(path.lower())

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
    maps = _Maps(
        loc=await _load_locale_map(branch, store),
        mount_categories=binfab.collection_category_map(mount_table) if mount_table else {},
        multipliers=mastery.parse_multipliers(multipliers) if multipliers else {},
        geode_multipliers=mastery.parse_geode_multipliers(geode_multipliers) if geode_multipliers else {},
        geode_members=geode_members,
        upgrade_trees=await _load_upgrade_trees(branch, store) if geode_members else {},
        store=store,
        prefab_shas=await _load_prefab_shas(branch) if with_resolver else {},
        valid_blueprints=await _load_valid_blueprints(branch),
    )
    logger.info("codexes[%s]: %d mount categories, %d mastery rows, %d geode rows, "
                "%d geode members, %d upgrade trees, %d prefab refs, %d blueprints", branch,
                len(maps.mount_categories), len(maps.multipliers), len(maps.geode_multipliers),
                len(maps.geode_members), len(maps.upgrade_trees), len(maps.prefab_shas),
                len(maps.valid_blueprints))
    return maps



async def _flush(rows: list[tuple]) -> None:
    if rows:
        await pg_store.upsert_entries(rows)


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


def _parse_entry(branch: str, path: str, sha: str, ctype: str, maps: _Maps, now) -> tuple | None:
    """Read + parse one prefab into a `codex_entry` row tuple (None if the blob is
    missing). Runs inside ``to_thread`` - all the blocking reads (the prefab itself
    plus recipe/companion cross-references) happen off the event loop."""
    content = maps.store.get(sha) if maps.store is not None else None
    if content is None:
        return None
    entry = extract_entry(ctype, path, content, maps.loc, resolve_meta=maps.item_meta, valid_blueprints=maps.valid_blueprints)
    rel = path[len(PREFABS_ROOT):].removesuffix(".binfab")
    if ctype == "mount":  # split dragons out by their collection category
        refine_mount(entry, rel, maps.mount_categories)
    entry["mastery"] = mastery.mastery_for(rel, maps.multipliers)
    entry["mastery_geode"] = mastery.geode_mastery_for(rel, maps.geode_multipliers)
    if rel.lower().startswith("item/companion/"):
        _attach_geode_companion(entry, rel, content, maps)
    return to_row(entry, branch, sha, now)


async def reindex(branch: str, store: ContentStore, *, force: bool = False) -> dict:
    """Full (re)build for `branch`. By default a prefab whose source sha is unchanged
    vs. the stored row is skipped; ``force`` re-parses every prefab regardless (used
    after a parser change - the UPSERT overwrites in place, so no empty window)."""
    maps = await _load_maps(branch, store)
    # (postgres_enabled is checked by the ensure_indexed entry point.)

    # path -> current source sha: skip unchanged prefabs (unless forced) and prune
    # removed ones.
    existing = await pg_store.existing_shas(branch)

    coll = UpdateState.get_pymongo_collection()
    cursor = coll.find(
        _prefix_query(branch, PREFABS_ROOT), {"path": 1, "content_sha256": 1, "_id": 0}
    )

    rows: list[tuple] = []
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
        rows.append(parsed)
        counts["indexed"] += 1
        if len(rows) >= _FLUSH_AT:
            await _flush(rows)
            rows = []
    await _flush(rows)

    stale = [p for p in existing if p not in seen]
    if stale:
        await pg_store.delete_entries(branch, stale)
        counts["removed"] = len(stale)

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
    if not touched:
        return counts

    # The lookup tables are only needed to (re)parse prefabs - skip the loads for a
    # delta that's pure removals. The recipe resolver's prefab-sha scan is only
    # loaded when a recipe actually changed (it reads referenced item prefabs).
    parse_rows = [r for r in touched if r["type"] != "removed" and r.get("content_sha256")]
    needs_parse = bool(parse_rows)
    needs_resolver = any(classify(r["path"]) == "recipe" for r in parse_rows)
    maps = await _load_maps(branch, store, with_resolver=needs_resolver) if needs_parse else _Maps()
    now = utcnow()
    rows: list[tuple] = []
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
        rows.append(parsed)
        counts["indexed"] += 1
        if len(rows) >= _FLUSH_AT:
            await _flush(rows)
            rows = []
    await _flush(rows)
    await pg_store.delete_entries(branch, removed)
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
    """``(branch, blueprint, skeleton, ap_key)`` rows for the given (path, sha) prefabs.
    Reads each blob, cheap-prefilters on ``.skeleton.gr2``, extracts structurally. Sync
    (run in a thread): the blob reads + parse are the heavy part."""
    rows: list[tuple] = []
    for _path, sha in candidates:
        content = store.get(sha)
        if not content or b".skeleton.gr2" not in content:
            continue                           # cheap pre-filter: no skeleton, no rig
        rig = binfab.extract_rig_refs(content)
        if not rig:
            continue
        skeleton = rig["skeleton"]
        rows.extend((branch, bp, skeleton, ap) for bp, ap in rig["parts"].items())
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
    changed and UPSERT their bindings (a creature rarely loses a part, so a removed
    binding just lingers harmlessly until the next full rebuild)."""
    rows = await UpdateChange.get_pymongo_collection().find(
        {"branch": branch, "ordinal": ordinal},
        {"path": 1, "type": 1, "content_sha256": 1, "_id": 0},
    ).to_list(length=None)
    changed = [(r["path"], r["content_sha256"]) for r in rows
               if r["type"] != "removed" and r.get("content_sha256")
               and r["path"].startswith(PREFABS_ROOT) and r["path"].endswith(".binfab")]
    if not changed:
        return 0
    new_rows = await asyncio.to_thread(_rig_rows, branch, store, changed)
    n = await pg_store.upsert_rig_bindings(new_rows)
    await pg_store.touch_meta(branch)
    logger.info("codexes[%s]: rig delta ordinal=%s - %d bindings from %d changed prefabs",
                branch, ordinal, n, len(changed))
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

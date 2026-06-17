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
from app.trove.codexes import binfab, geode, mastery, pg_store
from app.trove.codexes.extract import extract_entry, refine_mount
from app.trove.codexes.models import to_row
from app.trove.codexes.types import LOCALE_ROOT, PREFABS_ROOT, classify
from app.trove.updates.cas import ContentStore
from app.trove.updates.models import UpdateChange, UpdateState

logger = logging.getLogger("kiwi.trove.codexes")

_FLUSH_AT = 1000
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


async def _load_maps(branch: str, store: ContentStore) -> _Maps:
    """Load every lookup table the extractors need for `branch`."""
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
    )
    logger.info("codexes[%s]: %d mount categories, %d mastery rows, %d geode rows, "
                "%d geode members, %d upgrade trees", branch, len(maps.mount_categories),
                len(maps.multipliers), len(maps.geode_multipliers),
                len(maps.geode_members), len(maps.upgrade_trees))
    return maps


async def _flush(rows: list[tuple]) -> None:
    if rows:
        await pg_store.upsert_entries(rows)


def _attach_geode_companion(entry: dict, rel: str, content: bytes, maps: _Maps) -> None:
    """For an `item/companion/…` prefab, attach `data.geode_companion`: its rarity,
    upgrade-tree ref, and (when the tree binfab is in the archive) per-level bonuses."""
    ref = geode.find_upgrade_tree_ref(content)
    rarity = maps.geode_members.get(rel.lower())
    if not ref and not rarity:
        return
    tree = maps.upgrade_trees.get(ref) if ref else None
    entry.setdefault("data", {})["geode_companion"] = {
        "upgrade_tree": ref,
        "rarity": rarity,
        "levels": geode.parse_upgrade_tree(tree) if tree else [],
    }


def _entry_row(branch: str, path: str, sha: str, ctype: str, content: bytes,
               maps: _Maps, now) -> tuple:
    """Parse one prefab into a `codex_entry` row tuple (pg_store INSERT order)."""
    entry = extract_entry(ctype, path, content, maps.loc)
    rel = path[len(PREFABS_ROOT):].removesuffix(".binfab")
    if ctype == "mount":  # split dragons out by their collection category
        refine_mount(entry, rel, maps.mount_categories)
    entry["mastery"] = mastery.mastery_for(rel, maps.multipliers)
    entry["mastery_geode"] = mastery.geode_mastery_for(rel, maps.geode_multipliers)
    if rel.lower().startswith("item/companion/"):
        _attach_geode_companion(entry, rel, content, maps)
    return to_row(entry, branch, sha, now)


async def reindex(branch: str, store: ContentStore) -> dict:
    """Full (re)build for `branch`, skipping prefabs whose bytes are unchanged."""
    maps = await _load_maps(branch, store)
    # (postgres_enabled is checked by the ensure_indexed entry point.)

    # path -> current source sha, to skip unchanged prefabs and prune removed ones.
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
        if existing.get(path) == sha:
            counts["unchanged"] += 1
            continue
        content = await asyncio.to_thread(store.get, sha)
        if content is None:
            counts["missing_blob"] += 1
            continue
        rows.append(_entry_row(branch, path, sha, ctype, content, maps, now))
        counts["indexed"] += 1
        if len(rows) >= _FLUSH_AT:
            await _flush(rows)
            rows = []
    await _flush(rows)

    stale = [p for p in existing if p not in seen]
    if stale:
        await pg_store.delete_entries(branch, stale)
        counts["removed"] = len(stale)

    logger.info(
        "codexes[%s]: full indexed=%d unchanged=%d removed=%d missing_blob=%d",
        branch, counts["indexed"], counts["unchanged"], counts["removed"], counts["missing_blob"],
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
    # delta that's pure removals.
    needs_parse = any(r["type"] != "removed" and r.get("content_sha256") for r in touched)
    maps = await _load_maps(branch, store) if needs_parse else _Maps()
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
        content = await asyncio.to_thread(store.get, sha) if sha else None
        if content is None:
            counts["missing_blob"] += 1
            continue
        rows.append(_entry_row(branch, path, sha, classify(path), content, maps, now))  # type: ignore[arg-type]
        counts["indexed"] += 1
        if len(rows) >= _FLUSH_AT:
            await _flush(rows)
            rows = []
    await _flush(rows)
    await pg_store.delete_entries(branch, removed)

    logger.info(
        "codexes[%s]: delta ordinal=%s indexed=%d removed=%d missing_blob=%d",
        branch, ordinal, counts["indexed"], counts["removed"], counts["missing_blob"],
    )
    return counts


async def ensure_indexed(branch: str, store: ContentStore, summary: dict) -> dict:
    """Post-sync hook: full bootstrap if the codex is empty, else apply the delta."""
    if not settings.postgres_enabled:
        logger.warning("codexes[%s]: Postgres disabled - skipping index", branch)
        return {"indexed": 0, "removed": 0, "missing_blob": 0}
    if not await pg_store.has_any(branch):
        return await reindex(branch, store)
    ordinal = summary.get("ordinal")
    if summary.get("changed") and ordinal is not None:
        return await reindex_changes(branch, store, ordinal)
    return {"indexed": 0, "removed": 0, "missing_blob": 0}

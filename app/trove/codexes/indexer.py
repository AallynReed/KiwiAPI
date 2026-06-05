"""Build the codex from the archive for a branch — incrementally.

Two modes, both upserting ``CodexEntry`` (one per source prefab):

- ``reindex`` — a full (re)build from the materialized tree (``UpdateState``). The
  bootstrap/repair path. Content-incremental too: a prefab whose source sha is
  unchanged is skipped, so re-running is cheap.
- ``reindex_changes`` — the steady-state path. Reads just the ``UpdateChange``
  rows for one new version (the delta) and touches only those entries, so a
  routine game patch never walks the other 99% of the game.

``ensure_indexed`` picks between them after a sync: full build if the codex is
empty (e.g. first deploy onto an already-synced archive), otherwise the delta.
Names/descriptions resolve through the merged ``languages/`` locale tables.
"""

from __future__ import annotations

import asyncio
import logging

from pymongo import DeleteOne, UpdateOne

from app.core.utils import utcnow
from app.trove.codexes import binfab
from app.trove.codexes.extract import extract_entry
from app.trove.codexes.models import CodexEntry
from app.trove.codexes.types import LOCALE_ROOT, PREFABS_ROOT, classify
from app.trove.updates.cas import ContentStore
from app.trove.updates.models import UpdateChange, UpdateState

logger = logging.getLogger("kiwi.trove.codexes")

_FLUSH_AT = 1000

WriteOp = UpdateOne | DeleteOne


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


async def _flush(ops: list[WriteOp]) -> None:
    if ops:
        await CodexEntry.get_pymongo_collection().bulk_write(ops, ordered=False)


def _upsert_op(branch: str, path: str, sha: str, ctype: str, content: bytes,
               loc_map: dict[str, str], now) -> UpdateOne:
    entry = extract_entry(ctype, path, content, loc_map)
    return UpdateOne(
        {"branch": branch, "path": path},
        {"$set": {**entry, "branch": branch, "content_sha256": sha, "indexed_at": now}},
        upsert=True,
    )


async def reindex(branch: str, store: ContentStore) -> dict:
    """Full (re)build for `branch`, skipping prefabs whose bytes are unchanged."""
    loc_map = await _load_locale_map(branch, store)

    # path -> current source sha, to skip unchanged prefabs and prune removed ones.
    existing: dict[str, str] = {}
    async for doc in CodexEntry.get_pymongo_collection().find(
        {"branch": branch}, {"path": 1, "content_sha256": 1, "_id": 0}
    ):
        existing[doc["path"]] = doc["content_sha256"]

    coll = UpdateState.get_pymongo_collection()
    cursor = coll.find(
        _prefix_query(branch, PREFABS_ROOT), {"path": 1, "content_sha256": 1, "_id": 0}
    )

    ops: list[WriteOp] = []
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
        ops.append(_upsert_op(branch, path, sha, ctype, content, loc_map, now))
        counts["indexed"] += 1
        if len(ops) >= _FLUSH_AT:
            await _flush(ops)
            ops = []
    await _flush(ops)

    stale = [p for p in existing if p not in seen]
    if stale:
        await CodexEntry.find(
            CodexEntry.branch == branch, {"path": {"$in": stale}}
        ).delete()
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

    # The locale map is only needed to (re)parse prefabs — skip the load for a
    # delta that's pure removals.
    needs_parse = any(r["type"] != "removed" and r.get("content_sha256") for r in touched)
    loc_map = await _load_locale_map(branch, store) if needs_parse else {}
    now = utcnow()
    ops: list[WriteOp] = []

    for r in touched:
        path = r["path"]
        if r["type"] == "removed":
            ops.append(DeleteOne({"branch": branch, "path": path}))
            counts["removed"] += 1
            continue
        sha = r.get("content_sha256")
        content = await asyncio.to_thread(store.get, sha) if sha else None
        if content is None:
            counts["missing_blob"] += 1
            continue
        ops.append(_upsert_op(branch, path, sha, classify(path), content, loc_map, now))  # type: ignore[arg-type]
        counts["indexed"] += 1
        if len(ops) >= _FLUSH_AT:
            await _flush(ops)
            ops = []
    await _flush(ops)

    logger.info(
        "codexes[%s]: delta ordinal=%s indexed=%d removed=%d missing_blob=%d",
        branch, ordinal, counts["indexed"], counts["removed"], counts["missing_blob"],
    )
    return counts


async def ensure_indexed(branch: str, store: ContentStore, summary: dict) -> dict:
    """Post-sync hook: full bootstrap if the codex is empty, else apply the delta."""
    has_any = await CodexEntry.find_one(CodexEntry.branch == branch) is not None
    if not has_any:
        return await reindex(branch, store)
    ordinal = summary.get("ordinal")
    if summary.get("changed") and ordinal is not None:
        return await reindex_changes(branch, store, ordinal)
    return {"indexed": 0, "removed": 0, "missing_blob": 0}

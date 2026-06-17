"""Beanie-backed `UpdateRepo` - the production persistence for the archiver.

The first sync touches tens of thousands of logical files, so the per-file writes
(change-log / state / manifest sidecar) are BUFFERED and flushed in bulk via
`bulk_write` - hundreds of round-trips instead of hundreds of thousands. All
writes are idempotent upserts/deletes, so a crash + resume re-does at most one
flush-batch with no duplicates. Reads (sidecar / state) happen before writes for
the same key within a sync, so buffering is safe; `finish_version` flushes first.
"""

from __future__ import annotations

import logging

from pymongo import DeleteOne, UpdateOne

from app.core.utils import utcnow
from app.trove.updates.models import (
    UpdateBranch,
    UpdateChange,
    UpdateManifestEntry,
    UpdateState,
    UpdateVersion,
)

logger = logging.getLogger("kiwi.updates.repo")

_FLUSH_THRESHOLD = 1000


class MongoUpdateRepo:
    def __init__(self) -> None:
        self._changes: list = []      # UpdateOne upserts on (branch, ordinal, path)
        self._state_ops: list = []    # UpdateOne upserts / DeleteOne on (branch, path)
        self._manifest_ops: list = []

    # --- reads (immediate) -------------------------------------------------

    async def get_manifest_sidecar(self, branch: str) -> dict[str, dict]:
        docs = await UpdateManifestEntry.find({"branch": branch}).to_list()
        return {d.path: {"sha1": d.sha1, "size": d.size} for d in docs}

    async def state_get(self, branch: str, path: str) -> dict | None:
        d = await UpdateState.find_one({"branch": branch, "path": path})
        if d is None:
            return None
        return {"content_sha256": d.content_sha256, "fnv_hash": d.fnv_hash, "size": d.size}

    async def get_archive_state(self, branch: str, directory: str) -> dict[str, dict]:
        docs = await UpdateState.find({"branch": branch, "archive": directory}).to_list()
        return {d.path: {"fnv_hash": d.fnv_hash, "content_sha256": d.content_sha256} for d in docs}

    async def begin_version(self, branch: str, version_tag: str, pointer: dict) -> tuple[int, bool]:
        inprog = await UpdateVersion.find_one({"branch": branch, "status": "in_progress"})
        if inprog is not None:
            return inprog.ordinal, True  # resume a crashed sync under the same ordinal
        last = await UpdateVersion.find({"branch": branch}).sort("-ordinal").limit(1).to_list()
        ordinal = (last[0].ordinal + 1) if last else 1
        await UpdateVersion(
            branch=branch, ordinal=ordinal, version_tag=version_tag, status="in_progress",
            content_path=pointer.get("content_path", ""), motd=pointer.get("motd", ""),
        ).insert()
        await self._update_branch(branch, status="syncing")
        return ordinal, False

    # --- buffered writes ---------------------------------------------------

    async def record_change(self, branch: str, ordinal: int, path: str, change_type: str,
                            content_sha256: str | None, fnv_hash: int | None, size: int) -> None:
        self._changes.append(UpdateOne(
            {"branch": branch, "ordinal": ordinal, "path": path},
            {"$set": {"type": change_type, "content_sha256": content_sha256,
                      "fnv_hash": fnv_hash, "size": size}},
            upsert=True,
        ))
        await self._maybe_flush()

    async def upsert_state(self, branch: str, path: str, content_sha256: str, fnv_hash: int | None,
                           size: int, archive: str | None, archive_index: int | None) -> None:
        self._state_ops.append(UpdateOne(
            {"branch": branch, "path": path},
            {"$set": {"content_sha256": content_sha256, "fnv_hash": fnv_hash, "size": size,
                      "archive": archive, "archive_index": archive_index}},
            upsert=True,
        ))
        await self._maybe_flush()

    async def remove_state(self, branch: str, path: str) -> None:
        self._state_ops.append(DeleteOne({"branch": branch, "path": path}))
        await self._maybe_flush()

    async def set_manifest_entry(self, branch: str, path: str, sha1: str, size: int) -> None:
        self._manifest_ops.append(UpdateOne(
            {"branch": branch, "path": path}, {"$set": {"sha1": sha1, "size": size}}, upsert=True,
        ))
        await self._maybe_flush()

    async def remove_manifest_entry(self, branch: str, path: str) -> None:
        self._manifest_ops.append(DeleteOne({"branch": branch, "path": path}))
        await self._maybe_flush()

    async def _maybe_flush(self) -> None:
        if (len(self._changes) >= _FLUSH_THRESHOLD or len(self._state_ops) >= _FLUSH_THRESHOLD
                or len(self._manifest_ops) >= _FLUSH_THRESHOLD):
            await self._flush()

    async def _flush(self) -> None:
        # Swap buffers out BEFORE awaiting, so concurrent tasks append to fresh lists.
        # Order: changes + state first, manifest sidecar LAST (its update is the
        # "done" marker - committing it last keeps resume safe).
        changes, self._changes = self._changes, []
        state_ops, self._state_ops = self._state_ops, []
        manifest_ops, self._manifest_ops = self._manifest_ops, []
        if changes:
            await UpdateChange.get_pymongo_collection().bulk_write(changes, ordered=False)
        if state_ops:
            await UpdateState.get_pymongo_collection().bulk_write(state_ops, ordered=False)
        if manifest_ops:
            await UpdateManifestEntry.get_pymongo_collection().bulk_write(manifest_ops, ordered=False)

    # --- version / branch (immediate) --------------------------------------

    async def finish_version(self, branch: str, ordinal: int, version_tag: str,
                             pointer: dict, counts: dict) -> None:
        await self._flush()  # commit every buffered write before marking the version done
        v = await UpdateVersion.find_one({"branch": branch, "ordinal": ordinal})
        if v is not None:
            v.status = "complete"
            v.completed_at = utcnow()
            v.files_added = counts["added"]
            v.files_modified = counts["modified"]
            v.files_removed = counts["removed"]
            v.bytes_added = counts["bytes_added"]
            await v.save()
        await self._update_branch(
            branch, content_path=pointer.get("content_path", ""), current_version=version_tag,
            current_ordinal=ordinal, last_probe_at=utcnow(), status="idle",
        )
        # Push a live "new build" event (SSE subscribers + the Discord bot). The
        # game_update source reads the live-US latest version, so its signature only
        # moves when that branch advances - PTS completions are deduped to no-ops.
        try:
            from app.events import bus as events_bus
            await events_bus.publish_type("game_update")
        except Exception:
            logger.warning("game_update event publish failed", exc_info=True)

    async def touch_probe(self, branch: str, content_path: str, version_tag: str) -> None:
        await self._update_branch(
            branch, content_path=content_path, current_version=version_tag,
            last_probe_at=utcnow(), status="idle",
        )

    async def mark_branch_error(self, branch: str, message: str) -> None:
        await self._update_branch(branch, status="error", last_error=message[:500])

    async def _update_branch(self, branch: str, **fields) -> None:
        doc = await UpdateBranch.find_one({"branch": branch})
        if doc is None:
            doc = UpdateBranch(branch=branch)
            for k, v in fields.items():
                setattr(doc, k, v)
            await doc.insert()
            return
        for k, v in fields.items():
            setattr(doc, k, v)
        await doc.save()

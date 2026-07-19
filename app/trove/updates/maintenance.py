"""One-off maintenance passes over the archive that steady-state ingest doesn't do.

``backfill_last_ordinal`` populates ``UpdateState.last_ordinal`` (the version a file
was last added/modified in) for files that predate that field. Ingest sets it going
forward, but existing rows are 0 until this runs once per branch. It's a pure
recompute from the append-only change-log, so it's safe to re-run any time.

Status is kept in-process (mirrors the codex-rebuild pattern) so the admin panel can
show progress; it resets on restart, which is fine for a manual, minutes-long job.
"""

from __future__ import annotations

import asyncio
import logging

from pymongo import UpdateOne

from app.core.utils import utcnow
from app.trove.updates.models import UpdateChange, UpdateState

logger = logging.getLogger("kiwi.updates.maintenance")

_BATCH = 1000

# branch → {running, updated, done, started_at, finished_at, error}
_status: dict[str, dict] = {}


def get_backfill_status(branch: str) -> dict:
    return _status.get(branch, {"running": False, "updated": 0, "done": False,
                                "started_at": None, "finished_at": None, "error": None})


async def backfill_last_ordinal(branch: str) -> int:
    """Recompute ``last_ordinal`` for every file in ``branch`` from the change-log.

    Aggregates ``max(ordinal)`` per path over ``UpdateChange`` (the newest change to a
    present file is always its last add/modify), then bulk-updates the matching
    ``UpdateState`` rows. Non-``upsert`` writes, so removed-file paths never resurrect.
    Returns the number of state rows updated.
    """
    _status[branch] = {"running": True, "updated": 0, "done": False,
                       "started_at": utcnow(), "finished_at": None, "error": None}
    updated = 0
    try:
        changes = UpdateChange.get_pymongo_collection()
        state = UpdateState.get_pymongo_collection()
        pipeline = [
            {"$match": {"branch": branch}},
            {"$group": {"_id": "$path", "last": {"$max": "$ordinal"}}},
        ]
        ops: list = []
        # async PyMongo: aggregate() is a coroutine returning the async cursor -
        # await it before iterating (``async for`` over the bare coroutine fails
        # with "requires an object with __aiter__ method, got coroutine").
        cursor = await changes.aggregate(pipeline, allowDiskUse=True)
        async for row in cursor:
            ops.append(UpdateOne(
                {"branch": branch, "path": row["_id"]},
                {"$set": {"last_ordinal": row["last"]}},
            ))
            if len(ops) >= _BATCH:
                res = await state.bulk_write(ops, ordered=False)
                updated += res.modified_count
                ops = []
                _status[branch]["updated"] = updated
                await asyncio.sleep(0)  # yield so the event loop stays responsive
        if ops:
            res = await state.bulk_write(ops, ordered=False)
            updated += res.modified_count
        _status[branch].update(updated=updated, done=True)
        logger.info("updates[%s]: last_ordinal backfill updated %d files", branch, updated)
    except Exception as exc:  # noqa: BLE001 - surface the message to the admin panel
        _status[branch]["error"] = str(exc)[:500]
        logger.exception("updates[%s]: last_ordinal backfill failed", branch)
        raise
    finally:
        _status[branch]["running"] = False
        _status[branch]["finished_at"] = utcnow()
    return updated

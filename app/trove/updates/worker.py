"""Background archiver: probe each branch every `trove_update_probe_seconds`.

Off unless `trove_update_enabled` - turning it on triggers the multi-GB first sync.

In production the API runs several uvicorn workers, and the lifespan (hence this
loop) starts in each. A Redis leader lock ensures exactly ONE worker actually
syncs at a time; the holder renews it on a heartbeat so the long first sync keeps
the lock, and if the holder dies the lock expires and another worker takes over.
With no Redis (dev / single worker) it simply runs.
"""

import asyncio
import logging
import secrets
from typing import Any

import httpx

from app.core.config import settings
from app.core.redis import get_redis
from app.trove.codexes.indexer import ensure_indexed as ensure_codex_indexed
from app.trove.updates.cas import ContentStore
from app.trove.updates.cdn import BRANCHES, CdnClient
from app.trove.updates.ingest import sync_branch
from app.trove.updates.repo import MongoUpdateRepo

logger = logging.getLogger("kiwi.trove.updates")

_LOCK_KEY = "kiwi:updates:leader"
_LOCK_TTL = 60        # seconds the lock survives without a renewal
_RENEW_EVERY = 20     # heartbeat cadence while holding it
# Renew/release only if we still own the token (compare-and-act).
_RENEW_LUA = "if redis.call('get', KEYS[1]) == ARGV[1] then return redis.call('pexpire', KEYS[1], ARGV[2]) else return 0 end"
_RELEASE_LUA = "if redis.call('get', KEYS[1]) == ARGV[1] then return redis.call('del', KEYS[1]) else return 0 end"

_task: asyncio.Task | None = None


async def _sync_all_branches(repo: MongoUpdateRepo, store: ContentStore) -> None:
    for branch, pointer_file in BRANCHES.items():
        try:
            async with CdnClient(settings.trove_update_base_url, settings.trove_update_prefix) as cdn:
                summary = await sync_branch(
                    branch, pointer_file, cdn, store, repo,
                    download_concurrency=settings.trove_update_concurrency,
                )
            if summary["changed"]:
                logger.info(
                    "updates[%s]: %s ordinal=%s  +%d ~%d -%d  (%d new bytes)",
                    branch, summary["version"], summary.get("ordinal"),
                    summary["added"], summary["modified"], summary["removed"], summary["bytes_added"],
                )
            else:
                logger.info("updates[%s]: no change (%s)", branch, summary["version"])
            # Keep the codex current: full bootstrap if it's empty (first deploy
            # onto an already-synced archive), otherwise just this version's delta.
            # Isolated - a codex failure must not derail the archiver.
            try:
                await ensure_codex_indexed(branch, store, summary)
            except asyncio.CancelledError:
                raise
            except Exception:
                logger.warning("codexes[%s] index failed", branch, exc_info=True)
        except asyncio.CancelledError:
            raise
        except httpx.HTTPStatusError as e:
            # A 404 on the pointer means the branch isn't published right now
            # (e.g. PTS is down) - that's expected, not an error worth alarming on.
            if e.response.status_code == 404:
                logger.info("updates[%s]: not published right now (404) - skipping", branch)
                continue
            logger.warning("updates[%s] sync failed", branch, exc_info=True)
            try:
                await repo.mark_branch_error(branch, str(e))
            except Exception:
                pass
        except Exception as e:
            logger.warning("updates[%s] sync failed", branch, exc_info=True)
            try:
                await repo.mark_branch_error(branch, str(e))
            except Exception:
                pass


async def _heartbeat(redis, token: str, stop: asyncio.Event) -> None:
    while True:
        try:
            await asyncio.wait_for(stop.wait(), timeout=_RENEW_EVERY)
            return  # stop set - cycle finished
        except TimeoutError:
            pass
        try:
            renewed = await redis.eval(_RENEW_LUA, 1, _LOCK_KEY, token, _LOCK_TTL * 1000)
        except Exception:
            renewed = 1  # a transient Redis hiccup shouldn't abort a multi-GB sync
        if not renewed:
            logger.warning("updates: lost the archiver leader lock mid-cycle")
            stop.set()
            return


async def _run_one_cycle(repo: MongoUpdateRepo, store: ContentStore) -> None:
    redis: Any = get_redis()  # untyped: redis-py types eval() for sync, breaking await
    if redis is None:
        await _sync_all_branches(repo, store)  # dev / single worker - no lock needed
        return
    token = secrets.token_hex(16)
    if not await redis.set(_LOCK_KEY, token, nx=True, ex=_LOCK_TTL):
        logger.debug("updates: another worker holds the archiver lock; skipping cycle")
        return
    stop = asyncio.Event()
    hb = asyncio.create_task(_heartbeat(redis, token, stop))
    try:
        await _sync_all_branches(repo, store)
    finally:
        stop.set()
        hb.cancel()
        try:
            await hb
        except asyncio.CancelledError:
            pass
        try:
            await redis.eval(_RELEASE_LUA, 1, _LOCK_KEY, token)
        except Exception:
            pass


async def _loop() -> None:
    repo = MongoUpdateRepo()
    store = ContentStore(settings.trove_update_store_dir)
    while True:
        try:
            await _run_one_cycle(repo, store)
        except asyncio.CancelledError:
            raise
        except Exception:
            logger.warning("updates: archiver cycle failed", exc_info=True)
        try:
            await asyncio.sleep(settings.trove_update_probe_seconds)
        except asyncio.CancelledError:
            raise


def start_update_archiver() -> None:
    global _task
    if not settings.trove_update_enabled:
        logger.info("updates: archiver disabled (set TROVE_UPDATE_ENABLED=true to run)")
        return
    if _task is None:
        _task = asyncio.create_task(_loop())


async def stop_update_archiver() -> None:
    global _task
    if _task is not None:
        _task.cancel()
        try:
            await _task
        except asyncio.CancelledError:
            pass
        _task = None

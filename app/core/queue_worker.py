"""Shared Redis-list queue plumbing for the outbound-notification workers.

Two features (webhooks, DM subs) fan events out through a durable Redis list:
``enqueue_or_inline`` pushes with ``LPUSH`` (or, without Redis, delivers inline in
a background task), and a per-uvicorn-worker :class:`RedisListConsumer` drains it
with a blocking ``BRPOP``. With several workers each blocking on ``BRPOP`` every
job is handed to exactly one worker, so each notification is delivered once
regardless of worker count. A worker that crashes mid-delivery loses that job (a
Discord notification, not state) - an acceptable trade for not running a
consumer-group/PEL machine.

The start/stop task lifecycle here is shared more widely (the giveaway auto-draw
loop uses it too), so the loop body is pluggable via :class:`LoopTask`.
"""

from __future__ import annotations

import asyncio
import json
import logging
from collections.abc import Awaitable, Callable


class LoopTask:
    """A single background ``asyncio.Task`` with idempotent ``start``/``stop``.

    ``run`` is the coroutine function to run as the task body (usually an
    infinite loop that re-raises ``CancelledError`` for clean shutdown). Calling
    ``start`` twice is a no-op; ``stop`` cancels and awaits the task.
    """

    def __init__(self, run: Callable[[], Awaitable[None]]) -> None:
        self._run = run
        self._task: asyncio.Task | None = None

    def start(self) -> None:
        if self._task is None:
            self._task = asyncio.create_task(self._run())

    async def stop(self) -> None:
        if self._task is not None:
            self._task.cancel()
            try:
                await self._task
            except asyncio.CancelledError:
                pass
            self._task = None


class RedisListConsumer(LoopTask):
    """Per-worker ``BRPOP`` consumer for one Redis-list queue.

    Blocks on ``BRPOP queue`` (5s timeout so cancellation is prompt), JSON-decodes
    each item and hands it to ``handler``. Both the BRPOP and the handler are
    guarded: a failure is logged (under ``log_name``) and the loop continues, so
    one bad job never kills the consumer. No-ops when Redis is unconfigured.
    """

    def __init__(
        self,
        *,
        get_redis: Callable[[], object | None],
        queue: str,
        handler: Callable[[dict], Awaitable[None]],
        logger: logging.Logger,
        log_name: str,
    ) -> None:
        super().__init__(self._consume)
        self._get_redis = get_redis
        self._queue = queue
        self._handler = handler
        self._logger = logger
        self._log_name = log_name

    async def _consume(self) -> None:
        redis = self._get_redis()
        if redis is None:
            return
        while True:
            try:
                item = await redis.brpop([self._queue], timeout=5)
            except asyncio.CancelledError:
                raise
            except Exception:
                self._logger.warning("%s BRPOP failed", self._log_name, exc_info=True)
                await asyncio.sleep(1)
                continue
            if item is None:
                continue                                     # timeout - just loop
            try:
                _key, raw = item
                await self._handler(json.loads(raw))
            except asyncio.CancelledError:
                raise
            except Exception:
                self._logger.warning("%s delivery failed", self._log_name, exc_info=True)


async def enqueue_or_inline(
    payload: dict,
    *,
    deliverable_types: tuple[str, ...],
    flag: str,
    queue: str,
    deliver: Callable[[dict], Awaitable[None]],
    get_redis: Callable[[], object | None],
    is_enabled: Callable[[str], Awaitable[bool]],
    logger: logging.Logger,
    log_name: str,
) -> None:
    """Enqueue one event for fan-out, or deliver it inline without Redis.

    No-ops unless the payload's ``type`` is in ``deliverable_types`` and ``flag``
    is enabled. With Redis, ``LPUSH``es the JSON payload onto ``queue``; without
    it (single-worker dev), delivers inline via a background ``deliver`` task so
    the feature still works.
    """
    if payload.get("type") not in deliverable_types:
        return
    if not await is_enabled(flag):
        return
    redis = get_redis()
    if redis is None:
        asyncio.create_task(deliver(payload))
        return
    try:
        await redis.lpush(queue, json.dumps(payload, default=str))
    except Exception:
        logger.warning("%s enqueue failed", log_name, exc_info=True)

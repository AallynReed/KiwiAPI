"""Page-view beacon for the website container (no database of its own).

``app.pageviews.collect`` turns each report from the site's stats script into an
event; this buffers them and POSTs batches to the API's internal ingest over the
compose network. Raw IP / User-Agent never leave this process; only the visitor
digest and coarse labels are sent.

Silent no-op unless ``INTERNAL_KEY`` is set (the ingest rejects unsigned posts),
so a misconfigured deploy loses analytics rows and nothing else.
"""
import asyncio
import logging

import httpx

from app.core.config import settings

logger = logging.getLogger("kiwi.pageviews")

_FLUSH_INTERVAL_SECONDS = 5.0
_FLUSH_AT = 50        # post early once this many events are buffered
_MAX_BUFFER = 5_000   # hard cap; drop oldest beyond this if the API is unreachable
_POST_SIZE = 500      # the ingest's per-request cap


class PageviewBeacon:
    """Buffer of page-view events, flushed to the API's internal ingest."""

    def __init__(self) -> None:
        self._buffer: list[dict] = []
        self._task: asyncio.Task | None = None
        self._wake = asyncio.Event()

    @property
    def enabled(self) -> bool:
        return bool(settings.internal_key) and settings.pageview_tracking_enabled

    def record(self, event: dict) -> None:
        """Queue an event (cheap, non-blocking). Never raises."""
        self._buffer.append(event)
        if len(self._buffer) > _MAX_BUFFER:
            dropped = len(self._buffer) - _MAX_BUFFER
            del self._buffer[:dropped]
            logger.warning("page-view beacon buffer full - dropped %d event(s)", dropped)
        if len(self._buffer) >= _FLUSH_AT:
            self._wake.set()

    async def _flush(self) -> None:
        if not self._buffer:
            return
        batch, self._buffer = self._buffer, []
        url = settings.internal_api_url.rstrip("/") + "/internal/pageviews"
        try:
            async with httpx.AsyncClient(timeout=8.0) as client:
                for start in range(0, len(batch), _POST_SIZE):
                    resp = await client.post(
                        url, json={"events": batch[start:start + _POST_SIZE]},
                        headers={"X-Internal-Key": settings.internal_key})
                    resp.raise_for_status()
        except Exception:
            # Dropped, not retried: analytics are best-effort and a retry queue
            # would outlive the outage it was meant to cover.
            logger.warning("page-view beacon failed for %d event(s)", len(batch),
                           exc_info=True)

    async def _loop(self) -> None:
        while True:
            try:
                await asyncio.wait_for(self._wake.wait(), timeout=_FLUSH_INTERVAL_SECONDS)
            except TimeoutError:
                pass
            except asyncio.CancelledError:
                raise
            self._wake.clear()
            await self._flush()

    def start(self) -> None:
        if self._task is None and self.enabled:
            self._task = asyncio.create_task(self._loop())

    async def stop(self) -> None:
        if self._task is not None:
            self._task.cancel()
            try:
                await self._task
            except asyncio.CancelledError:
                pass
            self._task = None
        await self._flush()   # drain whatever is left on shutdown


beacon = PageviewBeacon()


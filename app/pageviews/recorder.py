"""Buffered writer for page-view events.

A sibling of ``app/usage/recorder.py``: buffer ``PageView`` events in memory and
flush them in batches (``insert_many``) on a short timer or when the buffer fills
- turning N writes into N/batch writes. Metrics are best-effort: a crash can drop
the last unflushed batch, an acceptable trade for the lower write load. The buffer
is bounded so a Mongo outage can't grow it without limit.
"""

import asyncio
import logging

from app.pageviews.models import PageView

logger = logging.getLogger("kiwi.pageviews")

_FLUSH_INTERVAL_SECONDS = 3.0
_FLUSH_AT = 200      # flush early once this many events are buffered
_MAX_BUFFER = 10_000  # hard cap; drop oldest beyond this if the DB is unavailable


class _PageViewRecorder:
    def __init__(self) -> None:
        self._buffer: list[PageView] = []
        self._task: asyncio.Task | None = None
        self._wake = asyncio.Event()
        self._direct: set[asyncio.Task] = set()  # keep fire-and-forget writes alive

    def record(self, event: PageView) -> None:
        """Queue an event (cheap, non-blocking). Started-or-not, never raises."""
        if self._task is None:
            # Recorder not running (e.g. unit context) - best-effort direct write.
            # Hold a reference until done, else the task can be GC'd mid-flight.
            task = asyncio.ensure_future(self._direct_insert(event))
            self._direct.add(task)
            task.add_done_callback(self._direct.discard)
            return
        self._buffer.append(event)
        if len(self._buffer) > _MAX_BUFFER:
            dropped = len(self._buffer) - _MAX_BUFFER
            del self._buffer[:dropped]
            logger.warning("page-view buffer full - dropped %d event(s)", dropped)
        if len(self._buffer) >= _FLUSH_AT:
            self._wake.set()

    async def _direct_insert(self, event: PageView) -> None:
        try:
            await event.insert()
        except Exception:
            logger.exception("Failed to record page-view event")

    async def _flush(self) -> None:
        if not self._buffer:
            return
        batch, self._buffer = self._buffer, []
        try:
            await PageView.insert_many(batch)
        except Exception:
            logger.exception("Failed to flush %d page-view event(s)", len(batch))

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
        if self._task is None:
            self._task = asyncio.create_task(self._loop())

    async def stop(self) -> None:
        if self._task is not None:
            self._task.cancel()
            try:
                await self._task
            except asyncio.CancelledError:
                pass
            self._task = None
        await self._flush()  # drain whatever is left on shutdown


_recorder = _PageViewRecorder()


def record_page_view(event: PageView) -> None:
    _recorder.record(event)


def start_pageview_recorder() -> None:
    _recorder.start()


async def stop_pageview_recorder() -> None:
    await _recorder.stop()

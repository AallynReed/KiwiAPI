"""Generic buffered writer that batches N document inserts into N/batch ``insert_many``.

Best-effort by design: a crash can drop the last unflushed batch (acceptable for the
lower write load), and the buffer is bounded so a Mongo outage can't grow it forever.
"""

import asyncio
import logging
from typing import Generic, TypeVar

from beanie import Document

T = TypeVar("T", bound=Document)

_FLUSH_INTERVAL_SECONDS = 3.0
_FLUSH_AT = 200      # flush early once this many events are buffered
_MAX_BUFFER = 10_000  # hard cap; drop oldest beyond this if the DB is unavailable


class BufferedRecorder(Generic[T]):
    """In-memory buffer + timer/threshold flush for a single Beanie ``Document`` type."""

    def __init__(
        self,
        document: type[T],
        logger: logging.Logger,
        label: str,
        *,
        flush_at: int = _FLUSH_AT,
        flush_interval_seconds: float = _FLUSH_INTERVAL_SECONDS,
        max_buffer: int = _MAX_BUFFER,
    ) -> None:
        self._document = document
        self._logger = logger
        self._label = label
        self._flush_at = flush_at
        self._flush_interval_seconds = flush_interval_seconds
        self._max_buffer = max_buffer
        self._buffer: list[T] = []
        self._task: asyncio.Task | None = None
        self._wake = asyncio.Event()
        self._direct: set[asyncio.Task] = set()  # keep fire-and-forget writes alive

    def record(self, event: T) -> None:
        """Queue an event (cheap, non-blocking). Started-or-not, never raises."""
        if self._task is None:
            # Recorder not running (e.g. unit context) - best-effort direct write.
            # Hold a reference until done, else the task can be GC'd mid-flight.
            task = asyncio.ensure_future(self._direct_insert(event))
            self._direct.add(task)
            task.add_done_callback(self._direct.discard)
            return
        self._buffer.append(event)
        if len(self._buffer) > self._max_buffer:
            dropped = len(self._buffer) - self._max_buffer
            del self._buffer[:dropped]
            self._logger.warning("%s buffer full - dropped %d event(s)", self._label, dropped)
        if len(self._buffer) >= self._flush_at:
            self._wake.set()

    async def _direct_insert(self, event: T) -> None:
        try:
            await event.insert()
        except Exception:
            self._logger.exception("Failed to record %s event", self._label)

    async def _flush(self) -> None:
        if not self._buffer:
            return
        batch, self._buffer = self._buffer, []
        try:
            await self._document.insert_many(batch)
        except Exception:
            self._logger.exception("Failed to flush %d %s event(s)", len(batch), self._label)

    async def _loop(self) -> None:
        while True:
            try:
                await asyncio.wait_for(self._wake.wait(), timeout=self._flush_interval_seconds)
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

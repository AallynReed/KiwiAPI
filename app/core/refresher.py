"""Background refresh-loop scaffold shared by the upstream-relay modules."""

from __future__ import annotations

import asyncio
import logging
from collections.abc import Awaitable, Callable
from typing import Any

logger = logging.getLogger("kiwi.refresher")


class PeriodicRefresher:
    """Run ``refresh`` forever, sleeping ``delay`` seconds between cycles.

    A failing cycle is logged and retried rather than killing the loop;
    CancelledError propagates for clean shutdown. ``delay`` may be a number or a
    zero-arg callable evaluated each cycle (so runtime-config changes take effect,
    and callers with a dynamic cadence can compute the next sleep). ``log_result``,
    if given, formats the refresh return value into a per-cycle info line.
    """

    def __init__(
        self,
        refresh: Callable[[], Awaitable[Any]],
        *,
        name: str,
        delay: float | Callable[[], float],
        log_result: Callable[[Any], str] | None = None,
    ) -> None:
        self._refresh = refresh
        self._name = name
        self._delay = delay if callable(delay) else (lambda: delay)
        self._log_result = log_result
        self._task: asyncio.Task | None = None

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

    async def _loop(self) -> None:
        while True:
            try:
                result = await self._refresh()
                if self._log_result is not None:
                    logger.info("%s: %s", self._name, self._log_result(result))
            except asyncio.CancelledError:
                raise
            except Exception:
                logger.warning("%s failed", self._name, exc_info=True)
            try:
                await asyncio.sleep(self._delay())
            except asyncio.CancelledError:
                raise

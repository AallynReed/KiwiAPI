"""Buffered writer for usage events (see ``app/core/buffered_recorder.py``)."""

import logging

from app.core.buffered_recorder import BufferedRecorder
from app.usage.models import UsageEvent

logger = logging.getLogger("kiwi.usage")

_recorder: BufferedRecorder[UsageEvent] = BufferedRecorder(UsageEvent, logger, "usage")


def record_usage_event(event: UsageEvent) -> None:
    _recorder.record(event)


def start_usage_recorder() -> None:
    _recorder.start()


async def stop_usage_recorder() -> None:
    await _recorder.stop()

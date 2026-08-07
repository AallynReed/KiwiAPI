"""Buffered writer for usage events (see ``app/core/buffered_recorder.py``)."""

import logging

from app.core.buffered_recorder import BufferedRecorder
from app.usage.models import UsageEvent

logger = logging.getLogger("kiwi.usage")

recorder: BufferedRecorder[UsageEvent] = BufferedRecorder(UsageEvent, logger, "usage")

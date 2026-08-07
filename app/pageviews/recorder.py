"""Buffered writer for page-view events (see ``app/core/buffered_recorder.py``)."""

import logging

from app.core.buffered_recorder import BufferedRecorder
from app.pageviews.models import PageView

logger = logging.getLogger("kiwi.pageviews")

recorder: BufferedRecorder[PageView] = BufferedRecorder(PageView, logger, "page-view")

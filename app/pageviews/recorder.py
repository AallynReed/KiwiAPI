"""Buffered writer for page-view events (see ``app/core/buffered_recorder.py``)."""

import logging

from app.core.buffered_recorder import BufferedRecorder
from app.pageviews.models import PageView

logger = logging.getLogger("kiwi.pageviews")

_recorder: BufferedRecorder[PageView] = BufferedRecorder(PageView, logger, "page-view")


def record_page_view(event: PageView) -> None:
    _recorder.record(event)


def start_pageview_recorder() -> None:
    _recorder.start()


async def stop_pageview_recorder() -> None:
    await _recorder.stop()

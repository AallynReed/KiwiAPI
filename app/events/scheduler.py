"""Time-driven event scheduler (runs in the API process).

For each scheduled event source (``app/events/sources.py``), one asyncio task:
publish the current state, then sleep until the source's next occurrence boundary
and publish the rolled-over state - so a rotation hits the Redis events channel the
instant it changes, instead of being polled. Publishes are dedup'd by the bus, so
running this in every API worker is harmless (duplicate emits collapse).

Started/stopped from the app lifespan alongside the event bus.
"""
import asyncio
import logging

from app.core.utils import utcnow
from app.events import bus
from app.events.sources import SCHEDULED_SOURCES

logger = logging.getLogger("kiwi.events.scheduler")

_tasks: list[asyncio.Task] = []

# Wake at least hourly even for far-future boundaries: re-syncs the sleep against
# the clock and bounds drift. When the real boundary is < 1h away, the delay isn't
# clamped, so the actual occurrence is hit within ~2s.
_MAX_SLEEP = 3600
_MIN_SLEEP = 5


async def _run_source(source) -> None:
    while True:
        try:
            await bus.publish_source(source)
        except asyncio.CancelledError:
            raise
        except Exception:
            logger.warning("scheduler: publish %s failed", source.type, exc_info=True)

        try:
            nxt = source.next_at_fn()
        except Exception:
            logger.warning("scheduler: next_at %s failed", source.type, exc_info=True)
            nxt = None

        now = int(utcnow().timestamp())
        # +2s so we wake just PAST the boundary and read the rolled-over state.
        delay = (nxt - now + 2) if nxt else _MAX_SLEEP
        delay = max(_MIN_SLEEP, min(delay, _MAX_SLEEP))
        try:
            await asyncio.sleep(delay)
        except asyncio.CancelledError:
            raise


def start_event_scheduler() -> None:
    global _tasks
    if _tasks:
        return
    _tasks = [asyncio.create_task(_run_source(s)) for s in SCHEDULED_SOURCES]
    logger.info("event scheduler started (%d source(s))", len(_tasks))


async def stop_event_scheduler() -> None:
    global _tasks
    for task in _tasks:
        task.cancel()
    for task in _tasks:
        try:
            await task
        except asyncio.CancelledError:
            pass
    _tasks = []

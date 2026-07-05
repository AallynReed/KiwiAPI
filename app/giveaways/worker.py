"""Background auto-draw loop.

Every tick (60s) it opens scheduled giveaways whose start has passed and draws
open giveaways whose end has passed - see ``service.run_due``. Same start/stop
idiom as the other refreshers; wired into the app lifespan in ``app/main.py``.
"""
import asyncio
import logging

from app.core.queue_worker import LoopTask
from app.giveaways import service

logger = logging.getLogger("kiwi.giveaways")

_INTERVAL_SECONDS = 60


async def _loop() -> None:
    while True:
        try:
            await service.run_due()
            # Push to the live event channel (SSE + the bot's giveaway
            # announcement). Dedup makes this a no-op unless a new giveaway opened.
            from app.events import bus
            await bus.publish_type("giveaways")
        except asyncio.CancelledError:
            raise
        except Exception:
            logger.warning("giveaway worker tick failed", exc_info=True)
        try:
            await asyncio.sleep(_INTERVAL_SECONDS)
        except asyncio.CancelledError:
            raise


_task = LoopTask(_loop)


def start_giveaway_worker() -> None:
    _task.start()


async def stop_giveaway_worker() -> None:
    await _task.stop()

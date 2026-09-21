"""Page-view beacon for the website container (no database of its own).

Mirrors ``app.pageviews.middleware``: same page-route test, same cookieless
visitor hash, same best-effort posture - but instead of inserting a row it buffers
the event and POSTs batches to the API's internal ingest over the compose network.
Raw IP / User-Agent never leave this process; only the digest is sent.

Silent no-op unless ``INTERNAL_KEY`` is set (the ingest rejects unsigned posts),
so a misconfigured deploy loses analytics rows and nothing else.
"""
import asyncio
import logging

import httpx
from fastapi import FastAPI, Request

from app.core.config import settings
from app.pageviews.middleware import is_page_template, visitor_hash

logger = logging.getLogger("kiwi.pageviews")

_FLUSH_INTERVAL_SECONDS = 5.0
_FLUSH_AT = 50        # post early once this many events are buffered
_MAX_BUFFER = 5_000   # hard cap; drop oldest beyond this if the API is unreachable


class PageviewBeacon:
    """Buffer of page-view events, flushed to the API's internal ingest."""

    def __init__(self) -> None:
        self._buffer: list[dict] = []
        self._task: asyncio.Task | None = None
        self._wake = asyncio.Event()

    @property
    def enabled(self) -> bool:
        return bool(settings.internal_key) and settings.pageview_tracking_enabled

    def record(self, route: str, path: str, digest: str) -> None:
        """Queue an event (cheap, non-blocking). Never raises."""
        self._buffer.append({"route": route, "path": path, "visitor_hash": digest})
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
                resp = await client.post(
                    url, json={"events": batch},
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


def add_pageview_beacon_middleware(app: FastAPI) -> None:
    """Record one page view per showcase-site page load, beaconed to the API."""

    @app.middleware("http")
    async def record_pageview(request: Request, call_next):
        response = await call_next(request)

        if not beacon.enabled or request.method != "GET":
            return response
        if response.status_code != 200:
            return response
        if not response.headers.get("content-type", "").startswith("text/html"):
            return response

        # The matched route template (e.g. /player/{name}); the CONCRETE path is
        # what's stored, so each mod / player page gets its own row.
        route = request.scope.get("route")
        template = getattr(route, "path", request.url.path)
        if not is_page_template(template):
            return response

        try:
            beacon.record(template, request.url.path, visitor_hash(request))
        except Exception:
            logger.exception("Failed to queue page-view event")

        return response

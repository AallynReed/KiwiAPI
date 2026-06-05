"""Trovesaurus events relay: fetch the calendar feed, upsert into Mongo, serve from DB.

A background task periodically pulls ``trovesaurus.com/calendar/feed`` (a JSON
array) and upserts each event into the ``trove_events`` collection keyed by its
Trovesaurus id. Events are kept after they leave the upstream feed so the API can
serve ongoing / history / upcoming. The parser is split from the fetch so it can
be unit-tested with a fixed payload (no network).
"""

import asyncio
import logging

import httpx

from app.core.config import settings
from app.core.utils import utcnow
from app.trove.models import TroveEvent

logger = logging.getLogger("kiwi.trove.events")


def parse_events(raw: list[dict]) -> list[dict]:
    """Normalize the raw Trovesaurus calendar array into event dicts. Pure / no network."""
    events: list[dict] = []
    for ev in raw:
        event_id = str(ev.get("id") or "").strip()
        if not event_id:
            continue
        try:
            starts_at = int(str(ev.get("startdate", "")))
            ends_at = int(str(ev.get("enddate", "")))
        except (TypeError, ValueError):
            continue  # an event without usable dates can't be placed on the timeline
        events.append({
            "event_id": event_id,
            "name": (ev.get("name") or "").strip() or "Untitled",
            "url": (ev.get("url") or "").strip(),
            "category": (ev.get("category") or "").strip() or "Event",
            "image": (ev.get("image") or "").strip() or None,
            "icon": (ev.get("icon") or "").strip() or None,
            "lookup": (ev.get("lookup") or "").strip() or None,
            "starts_at": starts_at,
            "ends_at": ends_at,
        })
    return events


async def refresh_events() -> int:
    """Fetch the calendar feed and upsert events into Mongo (by event_id). Returns count."""
    async with httpx.AsyncClient(timeout=15, headers={"User-Agent": "KiwiAPI/1.0"}) as client:
        resp = await client.get(settings.trove_events_feed_url)
        resp.raise_for_status()
        raw = resp.json()

    events = parse_events(raw if isinstance(raw, list) else [])
    for ev in events:
        existing = await TroveEvent.find_one({"event_id": ev["event_id"]})
        if existing is None:
            await TroveEvent(**ev).insert()
            continue
        existing.name = ev["name"]
        existing.url = ev["url"]
        existing.category = ev["category"]
        existing.image = ev["image"]
        existing.icon = ev["icon"]
        existing.lookup = ev["lookup"]
        existing.starts_at = ev["starts_at"]
        existing.ends_at = ev["ends_at"]
        existing.updated_at = utcnow()
        await existing.save()

    await _prune()
    return len(events)


async def _prune() -> None:
    """Drop events that ended longer than `trove_events_history_days` ago."""
    cutoff = int(utcnow().timestamp()) - settings.trove_events_history_days * 86400
    await TroveEvent.find({"ends_at": {"$lt": cutoff}}).delete()


# --- Background refresher ---------------------------------------------------

_task: asyncio.Task | None = None


async def _loop() -> None:
    while True:
        try:
            count = await refresh_events()
            logger.info("Trovesaurus events refreshed: %d event(s)", count)
        except asyncio.CancelledError:
            raise
        except Exception:
            logger.warning("Trovesaurus events refresh failed", exc_info=True)
        try:
            await asyncio.sleep(settings.trove_events_refresh_seconds)
        except asyncio.CancelledError:
            raise


def start_events_refresher() -> None:
    global _task
    if _task is None:
        _task = asyncio.create_task(_loop())


async def stop_events_refresher() -> None:
    global _task
    if _task is not None:
        _task.cancel()
        try:
            await _task
        except asyncio.CancelledError:
            pass
        _task = None

"""Chaos Chest: the weekly featured-item rotation.

The featured item (name / blueprint / identifier) is relayed from Trovesaurus and
cached in Mongo (a single ``FeedCache`` doc, ``feed="chaos_chest"``); a background
task refreshes it so requests never hit upstream. The 7-day window is computed
deterministically from server time (``server_time.chaos_chest_window``) and used
as the fallback whenever upstream is unavailable or its item has gone stale.

Served under the (public) ``rotations`` scope.
"""

import logging
from datetime import datetime

from app.core.config import settings
from app.core.http import fetch_json
from app.core.refresher import PeriodicRefresher
from app.core.utils import utcnow
from app.trove import server_time
from app.trove.models import FeedCache

logger = logging.getLogger("kiwi.trove.chaos")

_FEED = "chaos_chest"


def _to_int(value) -> int | None:
    try:
        return int(value)
    except (TypeError, ValueError):
        return None


def normalize(payload) -> dict | None:
    """Trovesaurus payload -> ``{name, identifier, blueprint, start, end}`` or None.

    Defensive about shape (the endpoint may wrap in ``{"data": …}`` or a list).
    Returns None unless at least a ``name`` is present."""
    if isinstance(payload, dict) and isinstance(payload.get("data"), dict):
        payload = payload["data"]
    elif isinstance(payload, list):
        payload = payload[0] if payload else {}
    if not isinstance(payload, dict):
        return None
    name = payload.get("name")
    if not name:
        return None
    identifier = payload.get("identifier")
    if isinstance(identifier, str):
        identifier = identifier.replace("\\", "/")
    blueprint = payload.get("blueprint")
    if isinstance(blueprint, str):
        blueprint = blueprint.lower()
    return {
        "name": name,
        "identifier": identifier,
        "blueprint": blueprint,
        "start": _to_int(payload.get("start")),
        "end": _to_int(payload.get("end")),
    }


def build_response(cached: dict | None, fetched_at: datetime | None,
                   now: datetime | None = None) -> dict:
    """Merge the cached item with the computed weekly window into the API shape.

    The deterministic window always frames the response; the relayed item overlays
    its name/blueprint (and its own start/end, when present). A cached item that
    has already ended is treated as stale (chest rotated before the relay caught
    up) and dropped, so we never serve last week's item as current."""
    real = now or server_time.real_utc_now()
    now_ts = int(real.timestamp())
    window = server_time.chaos_chest_window(real)
    starts_at, ends_at = window["starts_at"], window["ends_at"]

    item = None
    if cached and cached.get("name"):
        end = cached.get("end")
        if end is None or end > now_ts:  # not yet expired
            item = {
                "name": cached["name"],
                "identifier": cached.get("identifier"),
                "blueprint": cached.get("blueprint"),
            }
            if cached.get("start"):
                starts_at = cached["start"]
            if end:
                ends_at = end

    return {
        "active": starts_at <= now_ts < ends_at,
        "starts_at": starts_at,
        "ends_at": ends_at,
        "seconds_remaining": max(0, ends_at - now_ts),
        "item": item,
        "fetched_at": fetched_at if item else None,
    }


async def refresh_chaos_chest() -> bool:
    """Fetch the current chaos chest from Trovesaurus and cache it. True if stored."""
    item = normalize(await fetch_json(settings.trove_chaos_chest_url))
    if item is None:
        return False
    existing = await FeedCache.find_one(FeedCache.feed == _FEED)
    if existing is None:
        await FeedCache(feed=_FEED, items=[item], fetched_at=utcnow()).insert()
    else:
        existing.items = [item]
        existing.fetched_at = utcnow()
        await existing.save()
    return True


async def get_chaos_chest(now: datetime | None = None) -> dict:
    """The current chaos chest (cached item + computed window), ready to serve.

    Source preference: the bot-captured item for the current week (see
    ``captures.get_chaos_chest_for_week``) wins, since it's read from the
    actual in-game cfg. Falls back to the Trovesaurus relay when the bot
    hasn't reported the current week yet (e.g., immediately after a reset).
    """
    # Imported lazily - captures.py imports server_time too and we'd loop otherwise.
    from app.trove.captures import get_chaos_chest_for_week

    real = now or server_time.real_utc_now()
    week = server_time.chaos_chest_window(real)
    capture = await get_chaos_chest_for_week(week["starts_at"])
    if capture is not None:
        cached = {
            "name": capture.name,
            "identifier": None,
            "blueprint": None,
            "start": week["starts_at"],
            "end": week["ends_at"],
        }
        return build_response(cached, capture.captured_at, now)

    doc = await FeedCache.find_one(FeedCache.feed == _FEED)
    cached = doc.items[0] if doc and doc.items else None
    fetched_at = doc.fetched_at if doc else None
    return build_response(cached, fetched_at, now)


# --- Background refresher ---------------------------------------------------

_refresher = PeriodicRefresher(
    refresh_chaos_chest,
    name="Chaos Chest relay",
    delay=lambda: settings.trove_chaos_refresh_seconds,
    log_result=lambda stored: "updated" if stored else "no item",
)


def start_chaos_refresher() -> None:
    _refresher.start()


async def stop_chaos_refresher() -> None:
    await _refresher.stop()

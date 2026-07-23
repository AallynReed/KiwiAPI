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
from urllib.parse import quote

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


# --- Item art ---------------------------------------------------------------
# The featured item's blueprint is what the render endpoint draws its icon from.
# The relay carries one; the bot capture (which wins) reads only a name out of the
# in-game cfg, so that name is reversed through the codex the same way the /market
# thumbnails are. Memoized per name - the item changes weekly, and a name the codex
# can't pin won't start resolving mid-week either, so misses are cached too.
_CODEX_BRANCH = "live-us"
_IMAGE_DIM = 256          # square px of the ready-made ``image_url`` render
_bp_cache: dict[str, str | None] = {}


async def resolve_item_blueprint(name: str) -> str | None:
    """The codex blueprint that best represents ``name``, or None when the codex
    has no unambiguous match (no icon beats a wrong icon). Best-effort: a codex or
    DB hiccup returns None rather than failing the chest payload."""
    key = (name or "").strip().lower()
    if not key:
        return None
    if key in _bp_cache:
        return _bp_cache[key]
    try:
        from app.trove.codexes import read as codexes_read
        resolved = await codexes_read.blueprints_for_names(_CODEX_BRANCH, [key])
    except Exception:  # noqa: BLE001 - the icon is cosmetic; never break the chest
        logger.warning("chaos: blueprint resolve failed for %r", name, exc_info=True)
        return None  # transient - not cached, so the next call retries
    _bp_cache[key] = bp = resolved.get(key)
    return bp


def item_image_url(blueprint: str | None) -> str | None:
    """Ready-to-use PNG URL for a blueprint, or None without one. Points at the
    codex renderer, which is tokenless like this endpoint - so a client reading
    the chest can show the item's icon without a second lookup or extra scope."""
    if not blueprint:
        return None
    return (f"{settings.api_url.rstrip('/')}/v1/codexes/render"
            f"?blueprint={quote(blueprint, safe='')}&dim={_IMAGE_DIM}")


async def get_chaos_chest(now: datetime | None = None) -> dict:
    """The current chaos chest (cached item + computed window), ready to serve.

    Source preference: the bot-captured item for the current week (see
    ``captures.get_chaos_chest_for_week``) wins, since it's read from the
    actual in-game cfg. Falls back to the Trovesaurus relay when the bot
    hasn't reported the current week yet (e.g., immediately after a reset).

    The item's ``blueprint`` is filled in from the codex when the source didn't
    carry one, and paired with an ``image_url`` clients can render directly.
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
        response = build_response(cached, capture.captured_at, now)
    else:
        doc = await FeedCache.find_one(FeedCache.feed == _FEED)
        cached = doc.items[0] if doc and doc.items else None
        fetched_at = doc.fetched_at if doc else None
        response = build_response(cached, fetched_at, now)

    item = response.get("item")
    if item:
        if not item.get("blueprint"):
            item["blueprint"] = await resolve_item_blueprint(item["name"])
        item["image_url"] = item_image_url(item["blueprint"])
    return response


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

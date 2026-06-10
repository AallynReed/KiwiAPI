"""Redis snapshot cache for the leaderboards page's hot reads.

The page boots on three queries: the anchor list, the boards at the latest
anchor (a ``distinct()`` aggregate), and a board's top-N entries. The first two
are the slow ones and only change when a new capture lands.

We cache them in Redis, WARMED by the same background warmer that recomputes
cheaters/activity - so every ingest + TTL boundary refreshes the latest snapshot
and the page can switch to a new capture with zero Mongo work. Reads are
read-through (fall back to Mongo + populate on a miss), and everything degrades
to a plain Mongo read when Redis isn't configured.

Keyed by anchor so old captures' entries simply expire; an ingest also
invalidates the exact anchor it touched, so a re-insert / back-fill can't serve
the pre-insert snapshot.
"""

import json
import logging

from app.core.redis import get_redis
from app.trove.leaderboards import service as lb_service

logger = logging.getLogger("kiwi.trove.leaderboards.cache")

_PREFIX = "lb:cache:"
# Keys self-expire after this if the warmer dies, so reads fall back to Mongo
# rather than serving forever-stale data. The warmer refreshes the latest
# snapshot every cheaters TTL (~30 min), well inside this window.
_TTL_SECONDS = 6 * 3600
_TS_KEY = _PREFIX + "timestamps"
# Full anchor list we cache (callers slice to their own limit). Sized to cover
# the day-picker's window even at a high capture rate; the page reads the latest
# capture per trove-day from it.
_TS_CACHE_LIMIT = 365
_TROVE_DAY_OFFSET = 11 * 3600   # Trove's daily reset is 11:00 UTC (trove-day boundary)


async def _rget(key: str):
    r = get_redis()
    if r is None:
        return None
    try:
        raw = await r.get(key)
        return json.loads(raw) if raw is not None else None
    except Exception:  # noqa: BLE001 - cache must never break a read
        logger.warning("lb cache: get %s failed", key, exc_info=True)
        return None


async def _rset(key: str, value, ttl: int = _TTL_SECONDS) -> None:
    r = get_redis()
    if r is None:
        return
    try:
        await r.set(key, json.dumps(value), ex=ttl)
    except Exception:  # noqa: BLE001
        logger.warning("lb cache: set %s failed", key, exc_info=True)


def _boards_key(anchor: int) -> str:
    return f"{_PREFIX}boards:{anchor}"


def _entries_key(anchor: int, uuid: int, limit: int, offset: int) -> str:
    return f"{_PREFIX}entries:{anchor}:{uuid}:{offset}:{limit}"


# --- read-through accessors (used by the site proxies) ----------------------

async def get_timestamps(limit: int = 60) -> list[int]:
    cached = await _rget(_TS_KEY)
    if cached is None:
        cached = await lb_service.list_timestamps(_TS_CACHE_LIMIT)
        await _rset(_TS_KEY, cached)
    # Hold the page at the latest FULLY-PROCESSED anchor: a freshly-ingested
    # capture whose entries/cheaters/activity haven't been warmed yet is filtered
    # out until the warmer publishes it (set_ready_anchor), so the page switches
    # to a new snapshot only once it's ready. No ready pointer (no Redis / before
    # the first publish) -> pass the raw list through unchanged.
    cached = _cap_to_ready(cached, await get_ready_anchor())
    return cached[:limit]


def _cap_to_ready(timestamps: list[int], ready: int | None) -> list[int]:
    """Drop anchors newer than the published ``ready`` anchor (pure helper)."""
    if ready is None:
        return timestamps
    return [t for t in timestamps if t <= ready]


async def get_boards(anchor: int) -> list[dict]:
    cached = await _rget(_boards_key(anchor))
    if cached is not None:
        return cached
    fresh = await lb_service.list_boards_at(anchor)
    await _rset(_boards_key(anchor), fresh)
    return fresh


_NO_COMPARISON = {"comparable": False, "prev_anchor": None, "reason": "no_prior_snapshot"}


async def get_entries(
    uuid: int, anchor: int, *, limit: int, offset: int,
) -> tuple[list[dict], int, dict]:
    key = _entries_key(anchor, uuid, limit, offset)
    cached = await _rget(key)
    if cached is not None:
        # ``.get`` keeps any pre-comparison cached payloads readable until they expire.
        return cached["items"], cached["total"], cached.get("comparison", _NO_COMPARISON)
    items, total, comparison = await lb_service.list_entries_with_deltas(
        uuid, anchor, limit=limit, offset=offset,
    )
    await _rset(key, {"items": items, "total": total, "comparison": comparison})
    return items, total, comparison


# --- warm + invalidate (used by the warmer + the ingest path) ---------------

async def _retention_days() -> int:
    """How many days' latest capture to keep pre-warmed - the hot-retention
    window. Falls back to 3 if the runtime-config lookup fails."""
    from app.admin import runtime_config
    try:
        return max(1, int(await runtime_config.get_setting("leaderboards_hot_retention_days")))
    except Exception:  # noqa: BLE001
        return 3


def _latest_per_day(anchors: list[int], days: int) -> list[int]:
    """The latest (max) anchor of each trove-day, for the most-recent ``days``
    days - i.e. the per-day default the day-picker shows."""
    by_day: dict[int, int] = {}
    for a in anchors:
        key = (a - _TROVE_DAY_OFFSET) // 86400
        if a > by_day.get(key, -1):
            by_day[key] = a
    return sorted(by_day.values(), reverse=True)[:days]


async def warm() -> None:
    """Refresh the Redis snapshot. Called by the leaderboards warmer (boot +
    every TTL + after each ingest). Without Redis the queries still run (warming
    Mongo's page cache like the old warmer did); only the Redis writes no-op.

    Pre-warms the LATEST capture of each of the most-recent ``hot_retention_days``
    trove-days (the per-day default the picker shows = what users hit most).
    Older days stay cache-aside (cached on first view)."""
    timestamps = await lb_service.list_timestamps(_TS_CACHE_LIMIT)
    await _rset(_TS_KEY, timestamps)
    if not timestamps:
        return
    days = await _retention_days()
    for anchor in _latest_per_day(timestamps, days):
        boards = await lb_service.list_boards_at(anchor)
        await _rset(_boards_key(anchor), boards)
        # Pre-warm the first board's first page too, so each day's default paint
        # is instant; the rest of the boards' pages are cached on demand.
        if boards:
            uuid = boards[0]["uuid"]
            items, total, comparison = await lb_service.list_entries_with_deltas(
                uuid, anchor, limit=100, offset=0,
            )
            await _rset(
                _entries_key(anchor, uuid, 100, 0),
                {"items": items, "total": total, "comparison": comparison},
            )


# --- published "ready" pointer + persisted derived snapshots ----------------
# The page reads the latest PUBLISHED anchor; the warmer flips it (set_ready_anchor)
# only after that anchor's entries + cheaters + activity are all cached, so the
# switch to a new capture is atomic. Cheaters/activity payloads are persisted per
# anchor (TTL = retention days) so a restart serves them straight from Redis
# instead of paying a cold full-board recompute.

_READY_KEY = _PREFIX + "ready_anchor"


def _cheaters_key(anchor: int) -> str:
    return f"{_PREFIX}cheaters:{anchor}"


def _activity_key(anchor: int) -> str:
    return f"{_PREFIX}activity:{anchor}"


async def _retention_seconds() -> int:
    return await _retention_days() * 86400


async def get_ready_anchor() -> int | None:
    val = await _rget(_READY_KEY)
    return int(val) if isinstance(val, int) else None


async def set_ready_anchor(anchor: int) -> None:
    # Retention-day TTL so a long outage eventually un-gates the page (fail-open
    # to the raw latest) rather than freezing it forever on a dead pointer.
    await _rset(_READY_KEY, anchor, ttl=await _retention_seconds())


async def get_cheaters(anchor: int) -> dict | None:
    return await _rget(_cheaters_key(anchor))


async def set_cheaters(anchor: int, payload: dict) -> None:
    await _rset(_cheaters_key(anchor), payload, ttl=await _retention_seconds())


async def get_activity(anchor: int) -> dict | None:
    return await _rget(_activity_key(anchor))


async def set_activity(anchor: int, payload: dict) -> None:
    await _rset(_activity_key(anchor), payload, ttl=await _retention_seconds())


async def invalidate_anchor(anchor: int) -> None:
    """Drop every cached key for one anchor - called after an ingest so a
    re-insert / back-fill onto an existing anchor can't serve the stale
    pre-insert snapshot. (A brand-new anchor has nothing to drop; harmless.)

    Includes the persisted cheaters/activity snapshots so a re-insert onto the
    SAME anchor forces a recompute instead of the warmer adopting the now-stale
    persisted result (which is otherwise still "fresh" by computed_at)."""
    r = get_redis()
    if r is None:
        return
    try:
        keys = [_boards_key(anchor), _cheaters_key(anchor), _activity_key(anchor)]
        async for k in r.scan_iter(match=f"{_PREFIX}entries:{anchor}:*"):
            keys.append(k)
        if keys:
            await r.delete(*keys)
    except Exception:  # noqa: BLE001
        logger.warning("lb cache: invalidate anchor=%s failed", anchor, exc_info=True)

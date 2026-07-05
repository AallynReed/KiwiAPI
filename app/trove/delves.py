"""Weekly delve rotations, relayed from an external source and stored in Mongo.

An external community source hosts a per-week delve-floor history; we fetch a
week's payload, store it as one ``DelveRotation`` document (depths passed through
as-is), and serve it. The current week's data accumulates as players submit
floors, so a background task refreshes it on a cadence; past weeks are imported
once (``delve_import.py``) and stay static. Weeks roll over Monday 11:00 UTC
(midnight in UTC-11), anchored to the first delve week. The source URL is read
from settings (env) - the refresher is off until it's set. Served under the
(public) ``rotations`` scope.
"""

import logging
from datetime import datetime, timedelta, timezone

import httpx

from app.core import features
from app.core.config import settings
from app.core.refresher import PeriodicRefresher
from app.core.utils import utcnow
from app.trove.models import DelveRotation

logger = logging.getLogger("kiwi.trove.delves")

UTC_MINUS_11 = timezone(timedelta(hours=-11))
# Week 1 began Monday 2025-11-03 (midnight UTC-11 == 11:00 UTC); 7-day weeks since.
WEEK_ONE = datetime(2025, 11, 3, tzinfo=UTC_MINUS_11)


def current_week_id(now: datetime | None = None) -> int:
    """The delve week id for `now` (real UTC), rolling over Monday 11:00 UTC."""
    local = (now or datetime.now(timezone.utc)).astimezone(UTC_MINUS_11)
    return (local - WEEK_ONE).days // 7 + 1


def normalize_payload(payload) -> dict:
    """Pull the floor list + total out of a source payload, defensively."""
    depths: list = []
    total = 0
    if isinstance(payload, dict):
        if isinstance(payload.get("depths"), list):
            depths = payload["depths"]
        total = payload["total"] if isinstance(payload.get("total"), int) else len(depths)
    return {"depths": depths, "total": total}


def _source_headers(week: int) -> dict[str, str]:
    # A browser-like UA (+ optional per-week Referer) in case the source gates on them.
    headers = {
        "User-Agent": (
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 "
            "(KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36"
        ),
        "Accept": "application/json, text/plain, */*",
        "Accept-Language": "en-US,en;q=0.9",
    }
    if settings.trove_delve_source_referer:
        headers["Referer"] = f"{settings.trove_delve_source_referer}?week={week}"
    return headers


async def fetch_week(week: int) -> dict:
    """Fetch one week's raw payload from the configured source."""
    url = f"{settings.trove_delve_source_url}?week={week}&limit=200"
    async with httpx.AsyncClient(
        timeout=30, follow_redirects=True, headers=_source_headers(week)
    ) as client:
        resp = await client.get(url)
        resp.raise_for_status()
        return resp.json()


async def store_week(week: int, payload) -> int:
    """Upsert a week's rotation. Returns the stored depth count.

    Won't clobber an already-populated week with an empty payload (guards against a
    transient bad fetch wiping good data)."""
    norm = normalize_payload(payload)
    existing = await DelveRotation.find_one(DelveRotation.week == week)
    if existing is not None and not norm["depths"] and existing.depths:
        return len(existing.depths)
    count = len(norm["depths"])
    if existing is None:
        await DelveRotation(week=week, depths=norm["depths"], total=norm["total"],
                            depth_count=count, fetched_at=utcnow()).insert()
    else:
        existing.depths = norm["depths"]
        existing.total = norm["total"]
        existing.depth_count = count
        existing.fetched_at = utcnow()
        await existing.save()
    return count


async def refresh_current_week() -> int:
    week = current_week_id()
    return await store_week(week, await fetch_week(week))


async def get_week(week: int) -> DelveRotation | None:
    return await DelveRotation.find_one(DelveRotation.week == week)


async def list_weeks() -> list[dict]:
    """Available weeks (metadata only - never loads the heavy depth lists)."""
    coll = DelveRotation.get_pymongo_collection()
    rows = await coll.find(
        {}, {"week": 1, "total": 1, "depth_count": 1, "fetched_at": 1, "_id": 0}
    ).sort("week", -1).to_list(length=None)
    return [
        {"week": r["week"], "total": r.get("total", 0),
         "count": r.get("depth_count", 0), "fetched_at": r["fetched_at"]}
        for r in rows
    ]


# --- Background refresher ---------------------------------------------------
# Schedule: a pull on startup, then HOURLY on the delve-Monday (when the new
# rotation drops and fills fastest), and otherwise once a day at the Trove daily
# reset (11:00 UTC). "Monday" is in the delve frame (UTC-11), i.e. the window
# Mon 11:00 UTC -> Tue 11:00 UTC.


def _seconds_until_next_pull(now: datetime) -> float:
    """Delay to the next scheduled pull (>= 1s)."""
    now = now.astimezone(timezone.utc)
    reset = now.replace(hour=11, minute=0, second=0, microsecond=0)  # next 11:00 UTC
    if reset <= now:
        reset += timedelta(days=1)
    if now.astimezone(UTC_MINUS_11).weekday() == 0:  # delve-Monday -> hourly
        next_hour = now.replace(minute=0, second=0, microsecond=0) + timedelta(hours=1)
        target = min(next_hour, reset)  # snap to the reset at the tail of Monday
    else:
        target = reset
    return max(1.0, (target - now).total_seconds())


# While the feature flag is OFF the loop idles at this cadence, re-checking the
# flag so a runtime enable resumes fetching within a few minutes (no restart).
_DISABLED_RECHECK_SECONDS = 300

# The next sleep is computed per-cycle by the refresh (dynamic cadence: hourly on
# delve-Monday, else the daily reset; the recheck interval while disabled). It is
# stashed here for the refresher's ``delay`` callback to read back.
_next_sleep: float = _DISABLED_RECHECK_SECONDS


async def _refresh_cycle() -> None:
    """One delve cycle: refresh when enabled, and set the next dynamic sleep."""
    global _next_sleep
    try:
        if await features.is_enabled(features.DELVES_FLAG):
            count = await refresh_current_week()
            logger.info("Delve refresh: week %s -> %d depths", current_week_id(), count)
            _next_sleep = _seconds_until_next_pull(datetime.now(timezone.utc))
        else:
            # Feature disabled - skip the fetch, idle, and re-check soon.
            _next_sleep = _DISABLED_RECHECK_SECONDS
    except Exception:
        _next_sleep = _seconds_until_next_pull(datetime.now(timezone.utc))
        raise


_refresher = PeriodicRefresher(
    _refresh_cycle,
    name="Delve refresh",
    delay=lambda: _next_sleep,
)


def start_delve_refresher() -> None:
    if not settings.trove_delve_source_url:
        logger.info("delves: no source URL configured (set TROVE_DELVE_SOURCE_URL) - refresher off")
        return
    _refresher.start()


async def stop_delve_refresher() -> None:
    await _refresher.stop()

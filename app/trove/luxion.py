"""Luxion: the captured dragon-merchant appearance + its deterministic schedule.

Unlike Corruxion/Fluxion (fixed cadences computed from an epoch), Luxion's start
date is dev-set and shifts around events, so it can't be computed - the bot
captures the first in-game sighting (``WelcomeLog.cfg`` -> ``luxion``) and the API
anchors the run to that trove-day's 00:00 (see ``captures.insert_luxion`` +
``LuxionAppearance``).

Once anchored, the run IS deterministic and matches the in-game format exactly:

- The event lasts ``LUXION_RUN_DAYS`` (7) days from the anchor.
- Each day the merchant is available for a 3-hour window, and that window opens
  3 hours LATER than the day before - so day ``d`` (0-indexed) opens at
  ``started_at + d * 27h`` (24h to the next day + 3h shift) and lasts 3h.

  day 0: 00:00-03:00, day 1: 03:00-06:00, day 2: 06:00-09:00, ... (trove time).

Served under the (public) ``rotations`` scope, mirroring the Corruxion/Fluxion
response shape (active + live window + upcoming schedule).
"""

from datetime import datetime

from app.trove import server_time

LUXION_RUN_DAYS = 7
_WINDOW = 3 * 3600          # 3-hour daily merchant window
_DAY_STEP = 27 * 3600       # window opens 3h later each day (24h + 3h shift)


def schedule_for(started_at: int) -> list[dict]:
    """The full list of daily 3-hour merchant windows for a run anchored at
    ``started_at`` (unix seconds), soonest first. ``state`` carries the run-day
    label ("Day 1" … "Day 7") so a consumer can show the weekly rotation without
    recomputing it."""
    return [
        {
            "starts_at": started_at + d * _DAY_STEP,
            "ends_at": started_at + d * _DAY_STEP + _WINDOW,
            "state": f"Day {d + 1}",
        }
        for d in range(LUXION_RUN_DAYS)
    ]


def _away() -> dict:
    """Response when we've never captured a Luxion appearance (or the collection
    is empty). No schedule - the next start is unpredictable until the bot sees
    it in-game again."""
    return {
        "active": False,
        "merchant_open": False,
        "starts_at": None,
        "ends_at": None,
        "seconds_remaining": 0,
        "current_window": None,
        "next_window": None,
        "schedule": [],
        "first_seen_at": None,
        "last_seen_at": None,
    }


def build_response(started_at: int, first_seen_at: datetime | None,
                   last_seen_at: datetime | None,
                   now: datetime | None = None) -> dict:
    """Assemble the served shape from a stored appearance + the computed schedule.

    ``active`` = within the 7-day run. ``merchant_open`` = inside one of the daily
    3-hour windows right now. ``seconds_remaining`` counts down to the most useful
    next boundary: the current window's close if the merchant is open, else the
    next window's open, else the run's end."""
    real = now or server_time.real_utc_now()
    now_ts = int(real.timestamp())
    end = started_at + LUXION_RUN_DAYS * 86400
    active = started_at <= now_ts < end
    sched = schedule_for(started_at)

    current = next(
        (w for w in sched if w["starts_at"] <= now_ts < w["ends_at"]), None
    )
    upcoming = [w for w in sched if w["starts_at"] > now_ts]
    nxt = upcoming[0] if upcoming else None

    if current is not None:
        seconds_remaining = current["ends_at"] - now_ts
    elif nxt is not None:
        seconds_remaining = nxt["starts_at"] - now_ts
    else:
        seconds_remaining = max(0, end - now_ts)

    return {
        "active": active,
        "merchant_open": current is not None,
        "starts_at": started_at,
        "ends_at": end,
        "seconds_remaining": max(0, seconds_remaining),
        "current_window": current,
        "next_window": nxt,
        "schedule": sched,
        "first_seen_at": first_seen_at,
        "last_seen_at": last_seen_at,
    }


async def get_luxion(now: datetime | None = None) -> dict:
    """The current Luxion status: the most recent captured appearance projected
    onto its deterministic 7-day schedule. ``active`` is False (and the schedule
    empty) when no appearance has ever been captured."""
    # Lazy import - captures.py imports server_time too and we'd loop otherwise.
    from app.trove.captures import get_latest_luxion

    doc = await get_latest_luxion()
    if doc is None:
        return _away()
    return build_response(doc.started_at, doc.first_seen_at, doc.last_seen_at, now)

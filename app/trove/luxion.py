"""Luxion: the captured dragon-merchant appearance + its deterministic schedule.

Luxion is a hybrid of the two rotation kinds in this project. Its *cadence* is
computed like Corruxion/Fluxion - a fixed grid running off a global epoch - but
*which run* is live is dev-set and shifts around events, so it can't be computed
at all. The bot captures the first in-game sighting (``WelcomeLog.cfg`` ->
``luxion``); all that signal carries is **the trove-day the run started on** (see
``captures.insert_luxion`` + ``LuxionAppearance.started_at``).

The cadence: the merchant is open for 3 hours, then away for 24 - a 27-hour
cycle that has been ticking continuously since ``CYCLE_EPOCH``, whether or not a
run is live. So the openings are always ``CYCLE_EPOCH + k * 27h``; a run just
picks up the grid rather than defining it.

That is the part that used to be wrong here: the run was assumed to open AT its
start day's reset and step 27h from there. It opens at the first grid slot at or
after that reset instead. Since the grid drifts +3h per day, that offset is
``{0,3,6,9,12,15,18,21}h`` depending on the day - so a run can quite normally
start 15 hours after the day it was reported on.

- ``run_start(day)`` snaps a captured start-day to that first grid opening.
- Day ``d`` (0-indexed) of the run opens at ``run_start + d * 27h`` for 3h.
- The run lasts ``LUXION_RUN_DAYS`` (7) windows.

(The in-game mod expresses this as a 9-long ``[0,3,…,24]`` offset table indexed by
day-since-epoch, with a roll-over flag when the offset hits 24. Deduped, that
emits exactly this grid - the table is just a longhand for +27h.)

Served under the (public) ``rotations`` scope, mirroring the Corruxion/Fluxion
response shape (active + live window + upcoming schedule).
"""

from datetime import datetime, timezone

from app.trove import server_time

LUXION_RUN_DAYS = 7
_WINDOW = 3 * 3600          # 3-hour merchant window
_CYCLE = 27 * 3600          # 3h open + 24h away; the opening shifts +3h each day

# Origin of the never-resetting 27h cycle grid: trove 2025-10-10 00:00, i.e. that
# day's reset. Every Luxion opening - past, future, in a run or not - sits exactly
# on ``CYCLE_EPOCH + k * _CYCLE``.
CYCLE_EPOCH = datetime(2025, 10, 10, 11, 0, tzinfo=timezone.utc)
_EPOCH_TS = int(CYCLE_EPOCH.timestamp())


def run_start(day_anchor: int) -> int:
    """The run's first merchant opening, given the trove-day it started on.

    ``day_anchor`` is the captured start day's 00:00 (11:00 UTC) in unix seconds.
    Returns the first 27h-grid slot at or after it. Because the epoch is itself a
    reset, that lands on ``day_anchor + {0,3,6,9,12,15,18,21}h`` - the offset the
    grid happens to give that day, NOT zero. The 8 offsets cycle over 9 days, so
    every 9th trove-day holds no opening at all and rolls to the next day's 00:00."""
    cycles = -((_EPOCH_TS - day_anchor) // _CYCLE)      # ceil division
    return _EPOCH_TS + cycles * _CYCLE


def run_bounds(day_anchor: int) -> tuple[int, int]:
    """``(first opening, run end)`` for a run captured on ``day_anchor``."""
    start = run_start(day_anchor)
    return start, start + LUXION_RUN_DAYS * 86400


def schedule_for(day_anchor: int) -> list[dict]:
    """The full list of 3-hour merchant windows for the run that started on the
    trove-day ``day_anchor`` (unix seconds), soonest first. ``state`` carries the
    run-day label ("Day 1" … "Day 7") so a consumer can show the weekly rotation
    without recomputing it."""
    start = run_start(day_anchor)
    return [
        {
            "starts_at": start + d * _CYCLE,
            "ends_at": start + d * _CYCLE + _WINDOW,
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


def build_response(day_anchor: int, first_seen_at: datetime | None,
                   last_seen_at: datetime | None,
                   now: datetime | None = None) -> dict:
    """Assemble the served shape from a stored appearance + the computed schedule.

    ``day_anchor`` is the stored start-DAY; ``starts_at`` in the response is the
    run's first actual opening (the 27h-grid slot that day). ``active`` = within
    the 7-day run. ``merchant_open`` = inside one of the 3-hour windows right now.
    ``seconds_remaining`` counts down to the most useful next boundary: the
    current window's close if the merchant is open, else the next window's open,
    else the run's end."""
    real = now or server_time.real_utc_now()
    now_ts = int(real.timestamp())
    started_at, end = run_bounds(day_anchor)
    active = started_at <= now_ts < end
    sched = schedule_for(day_anchor)

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

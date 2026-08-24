"""Luxion: the captured dragon-merchant appearance + its deterministic schedule.

Luxion is a hybrid of the two rotation kinds in this project. Its *cadence* is
computed like Corruxion/Fluxion - a fixed grid running off a global epoch - but
*which run* is live is dev-set and shifts around events, so it can't be computed
at all. The bot captures the in-game sighting (``WelcomeLog.cfg`` -> ``luxion``)
every hour the welcome screen advertises him (see ``captures.insert_luxion``).

**The sighting is the truth.** ``luxion = active`` is a direct read of the live
welcome screen, so the run is live from the moment we receive it - we do not make
the site wait for a computed grid slot before saying he's here. That is what used
to be wrong: the run was assumed to open at the first 27h-grid slot at or after
the start day's reset, which put ``starts_at`` up to 21 hours in the FUTURE while
the game was already advertising him, so the dashboard read "Away" all day.

- ``run_bounds(day, first_seen)`` -> the run: the sighting, to the run's end.
- The end is the daily reset ``LUXION_RUN_DAYS`` after the start day's reset.
  Measured, not assumed: in the 2026-07 run the welcome panel was absent at
  07-21 14:40 and present at 15:06, then present at 07-27 10:40 and gone at
  11:40 - i.e. it died exactly on the 07-27 reset, 6 trove-days after the
  07-21 anchor. Every hourly dump in between carried the signal.

The 3-hour merchant windows are a separate, deterministic thing: the merchant is
open for 3 hours then away for 24 - a 27-hour cycle ticking continuously since
``CYCLE_EPOCH``, whether or not a run is live. Openings are always
``CYCLE_EPOCH + k * _CYCLE``; a run picks up the grid rather than defining it, so
``schedule_for`` emits the grid slots that fall INSIDE the run and nothing after
it. Since the grid drifts +3h per day the run's first window normally lands hours
after the sighting - that gap is real, and it is why ``active`` (the run is on)
and ``merchant_open`` (a window is open right now) are separate flags.

(The in-game mod expresses the grid as a 9-long ``[0,3,…,24]`` offset table indexed
by day-since-epoch, with a roll-over flag when the offset hits 24. Deduped, that
emits exactly this grid - the table is just a longhand for +27h.)

Served under the (public) ``rotations`` scope, mirroring the Corruxion/Fluxion
response shape (active + live window + upcoming schedule).
"""

from datetime import datetime, timezone

from app.trove import server_time

# Trove-days from the start day's reset to the run's end reset. See the module
# docstring for the capture data this is measured from.
LUXION_RUN_DAYS = 6
_WINDOW = 3 * 3600          # 3-hour merchant window
_CYCLE = 27 * 3600          # 3h open + 24h away; the opening shifts +3h each day

# Origin of the never-resetting 27h cycle grid: trove 2025-10-10 00:00, i.e. that
# day's reset. Every Luxion opening - past, future, in a run or not - sits exactly
# on ``CYCLE_EPOCH + k * _CYCLE``.
CYCLE_EPOCH = datetime(2025, 10, 10, 11, 0, tzinfo=timezone.utc)
_EPOCH_TS = int(CYCLE_EPOCH.timestamp())


def _ts(value: datetime | int | None) -> int | None:
    """Unix seconds for a stored timestamp, passing ints straight through. Mongo
    hands datetimes back NAIVE, and a naive ``.timestamp()`` would read them in the
    host's local zone - hours off for anyone not on UTC. Everything we store is
    UTC, so say so."""
    if value is None or isinstance(value, int):
        return value
    if value.tzinfo is None:
        value = value.replace(tzinfo=timezone.utc)
    return int(value.timestamp())


def next_grid_slot(after: int) -> int:
    """The first 27h-grid opening at or after ``after`` (unix seconds).

    Because the epoch is itself a daily reset, a slot measured from a reset lands
    on ``reset + {0,3,6,9,12,15,18,21}h`` - the offset the grid happens to give
    that day. The 8 offsets cycle over 9 days, so every 9th trove-day holds no
    opening at all and rolls into the next day."""
    cycles = -((_EPOCH_TS - after) // _CYCLE)      # ceil division
    return _EPOCH_TS + cycles * _CYCLE


def run_bounds(day_anchor: int,
               first_seen: datetime | int | None = None) -> tuple[int, int]:
    """``(run start, run end)`` for a run captured on ``day_anchor``.

    ``first_seen`` is the first sighting (a stored datetime or unix seconds); when
    we have it, that IS the start - the game was advertising Luxion at that
    instant, so the run was on. Without it (the yearly calendar stores only
    start-day anchors) we fall back to the day's reset, which is the earliest the
    run can have begun."""
    start = _ts(first_seen)
    return (day_anchor if start is None else start), day_anchor + LUXION_RUN_DAYS * 86400


def schedule_for(day_anchor: int,
                 first_seen: datetime | int | None = None) -> list[dict]:
    """The 3-hour merchant windows inside the run that started on ``day_anchor``,
    soonest first. These are the global 27h-grid slots that fall within the run -
    CLIPPED to it, so a finished run stops advertising windows it never had.
    ``state`` carries the run-day label ("Day 1", "Day 2", …) so a consumer can
    show the rotation without recomputing it."""
    start, end = run_bounds(day_anchor, first_seen)
    out, opens, day = [], next_grid_slot(start), 1
    while opens < end:
        out.append({
            "starts_at": opens,
            "ends_at": opens + _WINDOW,
            "state": f"Day {day}",
        })
        opens += _CYCLE
        day += 1
    return out


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

    ``day_anchor`` is the stored start-DAY and ``first_seen_at`` the first sighting;
    ``starts_at`` in the response is that sighting, so ``active`` goes true the
    moment the bot reports Luxion rather than at the next grid slot.
    ``merchant_open`` stays separate: it is True only inside one of the 3-hour
    windows. ``seconds_remaining`` counts down to the most useful next boundary:
    the current window's close if the merchant is open, else the next window's
    open, else the run's end."""
    real = now or server_time.real_utc_now()
    now_ts = int(real.timestamp())
    started_at, end = run_bounds(day_anchor, first_seen_at)
    active = started_at <= now_ts < end
    sched = schedule_for(day_anchor, first_seen_at)

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
    """The current Luxion status: the most recent captured appearance, live from
    its first sighting to the run's end, with the 3-hour windows the 27h grid puts
    inside it. ``active`` is False (and the schedule empty) when no appearance has
    ever been captured."""
    # Lazy import - captures.py imports server_time too and we'd loop otherwise.
    from app.trove.captures import get_latest_luxion

    doc = await get_latest_luxion()
    if doc is None:
        return _away()
    return build_response(doc.started_at, doc.first_seen_at, doc.last_seen_at, now)

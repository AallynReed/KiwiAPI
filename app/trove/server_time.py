"""Trove server-time + event-cycle calculations.

Ported and cleaned from BetterTroveTools. Trove's "day" rolls over at 11:00 UTC,
so the game's internal clock is real UTC minus 11h — the "trove-time" frame. The
event anchors below (``FIRST_*``) are expressed in that trove-time frame; to turn
a trove-time instant back into a real wall-clock UTC timestamp, add the offset.

Every public function takes an explicit ``now`` (real UTC), defaulting to the
real clock — so the logic is deterministic and unit-testable.
"""

import json
from datetime import datetime, timedelta, timezone
from functools import lru_cache
from pathlib import Path

UTC = timezone.utc
TROVE_OFFSET = timedelta(hours=11)
_DATA_DIR = Path(__file__).parent / "gamedata"

# --- Event cycle constants + anchors (anchors are in the TROVE-TIME frame) ---
DRAGON_DURATION = timedelta(days=3)
DRAGON_INTERVAL = timedelta(days=14)
FLUXION_INTERVAL = timedelta(days=7)
FIRST_WEEK_BUFF = datetime(2020, 3, 23, tzinfo=UTC)
FIRST_CORRUXION = datetime(2024, 3, 8, tzinfo=UTC)
FIRST_FLUXION = datetime(2023, 7, 18, tzinfo=UTC)
INVASION_INTERVAL = timedelta(hours=27)
INVASION_DURATION = timedelta(hours=3)
FIRST_INVASION = datetime(2026, 3, 24, 9, tzinfo=UTC)


@lru_cache(maxsize=8)
def _load(filename: str) -> dict:
    try:
        return json.loads((_DATA_DIR / filename).read_text(encoding="utf-8"))
    except Exception:
        return {}


def real_utc_now() -> datetime:
    return datetime.now(UTC)


def trove_now(now: datetime | None = None) -> datetime:
    """Real UTC -> trove-time frame (the in-game day rolls over at 11:00 UTC)."""
    return (now or real_utc_now()) - TROVE_OFFSET


def _real_unix(trove_dt: datetime) -> int:
    """A trove-time instant -> real wall-clock unix seconds."""
    return int((trove_dt + TROVE_OFFSET).timestamp())


def _timer(active: bool, start: datetime, end: datetime, t: datetime, state: str | None = None) -> dict:
    remaining = (end - t) if active else (start - t)
    out = {
        "active": active,
        "starts_at": _real_unix(start),
        "ends_at": _real_unix(end),
        "seconds_remaining": max(0, int(remaining.total_seconds())),
    }
    if state is not None:
        out["state"] = state
    return out


# --- Daily / weekly buffs --------------------------------------------------

def current_daily_buff(now: datetime | None = None) -> dict:
    return _load("daily_buffs.json").get(str(trove_now(now).weekday()), {})


def _week_index(t: datetime) -> int:
    weeks = (t.timestamp() - FIRST_WEEK_BUFF.timestamp()) // (7 * 24 * 3600)
    return int(weeks % 4)


def current_weekly_buff(now: datetime | None = None) -> dict:
    return _load("weekly_buffs.json").get(str(_week_index(trove_now(now))), {})


# --- Merchant: Corruxion (a 14-day / 3-day "dragon" cycle) ------------------

def _dragon_calc(first: datetime, t: datetime) -> tuple[int, datetime, int]:
    completed, current = divmod(
        int((t - first).total_seconds()), int(DRAGON_INTERVAL.total_seconds())
    )
    next_dragon = first + (completed + 1) * DRAGON_INTERVAL
    return completed, next_dragon, current


def corruxion_timer(now: datetime | None = None) -> dict:
    t = trove_now(now)
    _, next_dragon, current = _dragon_calc(FIRST_CORRUXION, t)
    active = current < DRAGON_DURATION.total_seconds()
    start = (next_dragon - DRAGON_INTERVAL) if active else next_dragon
    return _timer(active, start, start + DRAGON_DURATION, t)


# --- Merchant: Fluxion (voting/selling phases within the dragon cycle) ------

def _fluxion_calc(t: datetime) -> tuple[float, float, float, datetime]:
    delta = t.timestamp() - FIRST_FLUXION.timestamp()
    completed, current = divmod(delta, DRAGON_INTERVAL.total_seconds())
    phase, current = divmod(current, FLUXION_INTERVAL.total_seconds())
    next_phase = FIRST_FLUXION + (completed * 2 + (phase + 1)) * FLUXION_INTERVAL
    return completed, phase, current, next_phase


def fluxion_timer(now: datetime | None = None) -> dict:
    t = trove_now(now)
    _, phase, current, next_phase = _fluxion_calc(t)
    active = current < DRAGON_DURATION.total_seconds()
    state = ("voting" if phase == 0 else "selling") if active else "away"
    start = (next_phase - FLUXION_INTERVAL) if active else next_phase
    return _timer(active, start, start + DRAGON_DURATION, t, state=state)


# --- Merchant: Invasion (27h/3h cycle, gated to 6-day windows every 28 days) -

def _invasion_cycle(t: datetime) -> tuple[int, int]:
    return divmod(int((t - FIRST_INVASION).total_seconds()), int(INVASION_INTERVAL.total_seconds()))


def _is_invasion_week(inv_start: datetime) -> bool:
    cycle, active = 28 * 24 * 3600, 6 * 24 * 3600
    return ((inv_start - FIRST_INVASION).total_seconds() % cycle) < active


def _is_invasion_active(t: datetime) -> bool:
    completed, current = _invasion_cycle(t)
    inv_start = FIRST_INVASION + completed * INVASION_INTERVAL
    return _is_invasion_week(inv_start) and current < INVASION_DURATION.total_seconds()


def _next_invasion(t: datetime) -> datetime:
    completed, _ = _invasion_cycle(t)
    cycle = completed + 1
    while True:
        start = FIRST_INVASION + cycle * INVASION_INTERVAL
        if _is_invasion_week(start):
            return start
        cycle += 1


def invasion_timer(now: datetime | None = None) -> dict:
    t = trove_now(now)
    active = _is_invasion_active(t)
    if active:
        completed, _ = _invasion_cycle(t)
        start = FIRST_INVASION + completed * INVASION_INTERVAL
    else:
        start = _next_invasion(t)
    return _timer(active, start, start + INVASION_DURATION, t)


# --- Server-time primitive + the full home-page snapshot -------------------

def _next_daily_reset(real_now: datetime) -> datetime:
    today = real_now.replace(hour=11, minute=0, second=0, microsecond=0)
    return today if real_now < today else today + timedelta(days=1)


def _next_weekly_reset(real_now: datetime) -> datetime:
    t = real_now - TROVE_OFFSET
    completed = (t - FIRST_WEEK_BUFF) // timedelta(days=7)
    return (FIRST_WEEK_BUFF + (completed + 1) * timedelta(days=7)) + TROVE_OFFSET


def server_time(now: datetime | None = None) -> dict:
    real = now or real_utc_now()
    return {
        "now_unix": int(real.timestamp()),
        "trove_day": trove_now(real).strftime("%A"),
        "daily_reset_at": int(_next_daily_reset(real).timestamp()),
        "weekly_reset_at": int(_next_weekly_reset(real).timestamp()),
    }


def calendar_snapshot(now: datetime | None = None) -> dict:
    """Everything the home page shows: server time, current buffs, merchant timers."""
    real = now or real_utc_now()
    return {
        "server_time": server_time(real),
        "daily": current_daily_buff(real),
        "weekly": current_weekly_buff(real),
        "merchants": {
            "corruxion": corruxion_timer(real),
            "fluxion": fluxion_timer(real),
            "invasion": invasion_timer(real),
        },
    }

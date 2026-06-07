"""Trove server-time + event-cycle calculations.

Ported and cleaned from BetterTroveTools. Trove's "day" rolls over at 11:00 UTC,
so the game's internal clock is real UTC minus 11h — the "trove-time" frame. The
event anchors below (``FIRST_*``) are in that frame; to turn a trove-time instant
back into real wall-clock UTC, add the offset.

Every public function takes an explicit ``now`` (real UTC), defaulting to the real
clock — so the logic is deterministic and unit-testable. Returned timestamps are
real-UTC unix seconds.
"""

import json
from datetime import datetime, timedelta, timezone
from functools import lru_cache
from pathlib import Path

UTC = timezone.utc
TROVE_OFFSET = timedelta(hours=11)
_DATA_DIR = Path(__file__).parent / "gamedata"
DAY = 86400

# --- Cycle constants + anchors (anchors are in the TROVE-TIME frame) --------
DRAGON_DURATION = timedelta(days=3)
DRAGON_INTERVAL = timedelta(days=14)
FLUXION_INTERVAL = timedelta(days=7)
FIRST_WEEK_BUFF = datetime(2020, 3, 23, tzinfo=UTC)
FIRST_CORRUXION = datetime(2024, 3, 8, tzinfo=UTC)
FIRST_FLUXION = datetime(2023, 7, 18, tzinfo=UTC)
FIRST_GARDENING = datetime(2025, 5, 23, tzinfo=UTC)


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


def is_trove_friday(now: datetime | None = None) -> bool:
    """True if the given real-UTC instant falls inside a trove-time Friday.

    A trove Friday begins at trove 00:00 = real UTC Fri 11:00 and lasts a full
    24h, so it spans roughly half of real-UTC Friday plus half of real-UTC
    Saturday. Used to decide whether the hourly-challenge cycle drops to its
    half-hourly Friday cadence.
    """
    return trove_now(now).weekday() == 4


# Each in-game challenge has a 20-minute active window — the rest of the cycle
# is "next challenge in N minutes" downtime.
CHALLENGE_DURATION = timedelta(minutes=20)


def challenge_window(now: datetime | None = None) -> dict:
    """The most-recent challenge window's start/end, plus whether it's currently
    active and whether it falls inside a trove-Friday (half-hourly cycle).

    Regular days: one challenge per hour, starting at :00 (UTC + Trove agree
    on minutes since the offset is whole hours).
    Trove Friday: two challenges per hour, starting at :00 and :30.

    The function always returns the LAST window-start whether or not we're
    still inside its 20-minute active span — so a query made during the gap
    between challenges still resolves to a sensible "what was the last one?"
    answer. ``active`` distinguishes the two cases.
    """
    real = (now or real_utc_now()).replace(second=0, microsecond=0)
    friday = is_trove_friday(real)
    if friday:
        window_minute = 0 if real.minute < 30 else 30
    else:
        window_minute = 0
    start = real.replace(minute=window_minute)
    end = start + CHALLENGE_DURATION
    real_full = now or real_utc_now()
    return {
        "starts_at": int(start.timestamp()),
        "ends_at": int(end.timestamp()),
        "active": start <= real_full < end,
        "is_friday_window": friday,
        "seconds_remaining": max(0, int(end.timestamp()) - int(real_full.timestamp())),
    }


# --- Server time -----------------------------------------------------------

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
        "now_iso": real.isoformat(),
        "trove_day": trove_now(real).strftime("%A"),
        "daily_reset_at": int(_next_daily_reset(real).timestamp()),
        "weekly_reset_at": int(_next_weekly_reset(real).timestamp()),
    }


# --- Daily / weekly buffs --------------------------------------------------

def daily_buffs(now: datetime | None = None) -> dict:
    data = _load("daily_buffs.json")
    current = data.get(str(trove_now(now).weekday()), {})
    week = [data[str(i)] for i in range(7) if str(i) in data]   # Monday..Sunday
    return {"current": current, "week": week}


def _week_index(t: datetime) -> int:
    weeks = (t.timestamp() - FIRST_WEEK_BUFF.timestamp()) // (7 * DAY)
    return int(weeks % 4)


def weekly_buffs(now: datetime | None = None) -> dict:
    data = _load("weekly_buffs.json")
    current = data.get(str(_week_index(trove_now(now))), {})
    rotation = [data[str(i)] for i in range(4) if str(i) in data]
    return {"current": current, "rotation": rotation}


# --- Merchant: Corruxion (14-day / 3-day cycle) ----------------------------

def _dragon_calc(first: datetime, t: datetime) -> tuple[int, datetime, int]:
    completed, current = divmod(
        int((t - first).total_seconds()), int(DRAGON_INTERVAL.total_seconds())
    )
    next_dragon = first + (completed + 1) * DRAGON_INTERVAL
    return completed, next_dragon, current


def corruxion(now: datetime | None = None, count: int = 8) -> dict:
    t = trove_now(now)
    _, next_dragon, current = _dragon_calc(FIRST_CORRUXION, t)
    active = current < DRAGON_DURATION.total_seconds()
    first_start = (next_dragon - DRAGON_INTERVAL) if active else next_dragon
    end = first_start + DRAGON_DURATION
    remaining = (end - t) if active else (first_start - t)
    schedule = [
        {
            "starts_at": _real_unix(first_start + i * DRAGON_INTERVAL),
            "ends_at": _real_unix(first_start + i * DRAGON_INTERVAL + DRAGON_DURATION),
        }
        for i in range(count)
    ]
    return {
        "active": active,
        "starts_at": _real_unix(first_start),
        "ends_at": _real_unix(end),
        "seconds_remaining": max(0, int(remaining.total_seconds())),
        "schedule": schedule,
    }


# --- Merchant: Fluxion (voting/selling, 7 days apart, 3-day windows) --------

def _fluxion_calc(t: datetime) -> tuple[float, float, float, datetime]:
    delta = t.timestamp() - FIRST_FLUXION.timestamp()
    completed, current = divmod(delta, DRAGON_INTERVAL.total_seconds())
    phase, current = divmod(current, FLUXION_INTERVAL.total_seconds())
    next_phase = FIRST_FLUXION + (completed * 2 + (phase + 1)) * FLUXION_INTERVAL
    return completed, phase, current, next_phase


def _fluxion_state(window_start: datetime) -> str:
    # Each 7-day window flips phase, starting with voting at FIRST_FLUXION.
    k = round((window_start - FIRST_FLUXION).total_seconds() / FLUXION_INTERVAL.total_seconds())
    return "voting" if int(k) % 2 == 0 else "selling"


def fluxion(now: datetime | None = None, count: int = 8) -> dict:
    t = trove_now(now)
    _, _, current, next_phase = _fluxion_calc(t)
    active = current < DRAGON_DURATION.total_seconds()
    first_start = (next_phase - FLUXION_INTERVAL) if active else next_phase
    end = first_start + DRAGON_DURATION
    remaining = (end - t) if active else (first_start - t)
    schedule = [
        {
            "starts_at": _real_unix(first_start + i * FLUXION_INTERVAL),
            "ends_at": _real_unix(first_start + i * FLUXION_INTERVAL + DRAGON_DURATION),
            "state": _fluxion_state(first_start + i * FLUXION_INTERVAL),
        }
        for i in range(count)
    ]
    return {
        "active": active,
        "state": _fluxion_state(first_start) if active else "away",
        "starts_at": _real_unix(first_start),
        "ends_at": _real_unix(end),
        "seconds_remaining": max(0, int(remaining.total_seconds())),
        "schedule": schedule,
    }


# --- Chaos Chest (weekly window) -------------------------------------------
# The featured item comes from Trovesaurus (relayed), but the 7-day window is
# deterministic — anchored to the fluxion epoch in real UTC, exactly as BTT does
# (first_fluxion + 11h). Used as the fallback whenever upstream gives no times.

def chaos_chest_window(now: datetime | None = None) -> dict:
    real = now or real_utc_now()
    base = FIRST_FLUXION + TROVE_OFFSET  # trove-frame anchor -> real-UTC anchor
    intervals = int((real - base).total_seconds() // (7 * DAY))
    start = base + timedelta(days=intervals * 7)
    end = start + timedelta(days=7)
    return {"starts_at": int(start.timestamp()), "ends_at": int(end.timestamp())}


# --- Gardening (plant harvest windows) -------------------------------------

def gardening(now: datetime | None = None) -> dict:
    real = now or real_utc_now()
    base = FIRST_GARDENING + TROVE_OFFSET          # real-UTC anchor (11:00 UTC)
    now_ts = real.timestamp()

    def harvest(cycle_days: int, ripe_after: int) -> tuple[datetime, datetime, datetime]:
        cycles = int((real - base).total_seconds() // (cycle_days * DAY))
        cycle_start = base + timedelta(days=cycles * cycle_days)
        h_start = cycle_start + timedelta(days=ripe_after)
        return cycle_start, h_start, h_start + timedelta(days=1)

    c2, h2s, h2e = harvest(2, 1)                    # 2-day plants ripen day 1->2
    c3, h3s, h3e = harvest(3, 2)                    # 3-day plants ripen day 2->3

    def win(name: str, s: datetime, e: datetime) -> dict:
        return {
            "name": name,
            "active": s.timestamp() <= now_ts < e.timestamp(),
            "starts_at": int(s.timestamp()),
            "ends_at": int(e.timestamp()),
        }

    upcoming: list[dict] = []
    for i in range(8):
        s2 = c2 + timedelta(days=i * 2 + 1)
        if s2.timestamp() > now_ts:
            upcoming.append(win("2-day plants", s2, s2 + timedelta(days=1)))
        s3 = c3 + timedelta(days=i * 3 + 2)
        if s3.timestamp() > now_ts:
            upcoming.append(win("3-day plants", s3, s3 + timedelta(days=1)))
    upcoming.sort(key=lambda x: x["starts_at"])

    return {
        "two_day": win("2-day plants", h2s, h2e),
        "three_day": win("3-day plants", h3s, h3e),
        "upcoming": upcoming[:10],
    }

"""Unit tests for the day-over-day comparability rule behind the leaderboards
rank/score deltas.

The "is this pair comparable?" decision is pure: a snapshot pair is comparable
iff NO reset boundary falls between the two anchors (``reset_boundaries_for_kind``
is empty) for the board's effective cadence. That collapses to:

  * lifetime (default/none): always comparable;
  * weekly: comparable except across the Monday 11:00 UTC reset;
  * daily: never comparable day-over-day.

We also pin ``_trove_day_start`` (which picks "yesterday's latest snapshot").
"""
from __future__ import annotations

from datetime import UTC, datetime, timedelta

from app.trove.leaderboards.service import (
    _trove_day_start,
    reset_boundaries_for_kind,
)


def _ts(y: int, m: int, d: int, hh: int, mm: int = 0) -> int:
    return int(datetime(y, m, d, hh, mm, tzinfo=UTC).timestamp())


def _comparable(kind: str, prev: int, now: int) -> bool:
    """Mirror of list_entries_with_deltas's gate: comparable iff no reset
    boundary lies in (prev, now]."""
    return not reset_boundaries_for_kind(kind, prev, now)


def _a_monday() -> datetime:
    """A Monday at 12:00 UTC (the week containing the project 'today')."""
    d = datetime(2026, 6, 10, 12, tzinfo=UTC)
    return d - timedelta(days=d.weekday())   # weekday()==0 is Monday


# --- _trove_day_start (which "day" a snapshot belongs to) -------------------

def test_trove_day_start_after_11_is_same_day():
    # 15:00 UTC is after the 11:00 reset -> belongs to that day's trove-day.
    assert _trove_day_start(_ts(2026, 6, 10, 15)) == _ts(2026, 6, 10, 11)


def test_trove_day_start_before_11_is_previous_day():
    # 09:00 UTC is before the 11:00 reset -> still the PREVIOUS trove-day.
    assert _trove_day_start(_ts(2026, 6, 10, 9)) == _ts(2026, 6, 9, 11)


def test_trove_day_start_exactly_11_is_that_day():
    assert _trove_day_start(_ts(2026, 6, 10, 11)) == _ts(2026, 6, 10, 11)


# --- comparability per cadence ---------------------------------------------

def test_daily_never_comparable_day_over_day():
    prev = _ts(2026, 6, 9, 12)
    now = _ts(2026, 6, 10, 12)
    # The 11:00 reset opening 6-10 falls between them.
    assert _comparable("daily", prev, now) is False


def test_weekly_comparable_within_a_week():
    mon = _a_monday()
    wed = int((mon + timedelta(days=2)).timestamp())
    thu = int((mon + timedelta(days=3)).timestamp())
    assert _comparable("weekly", wed, thu) is True


def test_weekly_not_comparable_across_monday():
    mon = _a_monday()
    sun = int((mon - timedelta(days=1)).timestamp())   # previous Sunday 12:00
    mon_noon = int(mon.timestamp())
    # Monday 11:00 reset sits between Sunday 12:00 and Monday 12:00.
    assert _comparable("weekly", sun, mon_noon) is False


def test_lifetime_always_comparable():
    prev = _ts(2026, 6, 9, 12)
    now = _ts(2026, 6, 10, 12)
    # Neither the hardcoded default nor the admin 'none' override ever reset.
    assert _comparable("default", prev, now) is True
    assert _comparable("none", prev, now) is True


def test_lifetime_comparable_even_across_a_long_gap():
    # Lifetime scores accumulate forever, so a multi-day gap is still valid.
    prev = _ts(2026, 6, 1, 12)
    now = _ts(2026, 6, 10, 12)
    assert _comparable("default", prev, now) is True

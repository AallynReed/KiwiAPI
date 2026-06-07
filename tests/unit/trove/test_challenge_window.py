"""Pure tests for ``server_time.challenge_window`` + ``is_trove_friday``.

The DB-side (capture insert/list) lives in integration tests; this module
covers just the window-anchor logic — pure ``datetime`` math, no fixtures.

Time-zone discipline: all inputs are explicit aware-UTC datetimes. The
function reads ``now.replace(second=0, microsecond=0)`` for the window math
but compares the original ``now`` for the ``active``/``seconds_remaining``
fields, so passing different sub-minute instants exercises both paths.
"""

from datetime import UTC, datetime, timedelta

from app.trove.server_time import (
    CHALLENGE_DURATION,
    challenge_window,
    is_trove_friday,
)


# --- is_trove_friday --------------------------------------------------------
# Trove time = real UTC - 11h, so a trove Friday spans real-UTC Fri 11:00 →
# Sat 11:00. Friday wall-clock is `weekday() == 4`.


def test_trove_friday_starts_at_real_friday_11_utc():
    # 2026-06-05 was a real-UTC Friday. At 10:59 UTC, trove time is Thu 23:59
    # → still trove-Thursday. At 11:00 UTC, trove flips to Fri 00:00.
    assert is_trove_friday(datetime(2026, 6, 5, 10, 59, tzinfo=UTC)) is False
    assert is_trove_friday(datetime(2026, 6, 5, 11, 0, tzinfo=UTC)) is True


def test_trove_friday_ends_at_real_saturday_11_utc():
    # Trove Friday lasts 24h → ends when real-UTC Sat hits 11:00.
    assert is_trove_friday(datetime(2026, 6, 6, 10, 59, tzinfo=UTC)) is True
    assert is_trove_friday(datetime(2026, 6, 6, 11, 0, tzinfo=UTC)) is False


def test_trove_friday_excludes_other_days():
    # Real-UTC Monday noon → trove Monday 01:00 → not Friday.
    assert is_trove_friday(datetime(2026, 6, 1, 12, 0, tzinfo=UTC)) is False


# --- challenge_window: weekday (hourly) cadence -----------------------------
# A real-UTC Wednesday is trove-Wed for most of the day — strictly hourly.


def test_weekday_window_anchors_to_hour_start():
    now = datetime(2026, 6, 3, 14, 5, 12, tzinfo=UTC)  # Wed
    w = challenge_window(now)
    assert w["is_friday_window"] is False
    assert w["starts_at"] == int(datetime(2026, 6, 3, 14, 0, tzinfo=UTC).timestamp())
    assert w["ends_at"] == w["starts_at"] + int(CHALLENGE_DURATION.total_seconds())
    assert w["active"] is True


def test_weekday_window_active_only_within_20_minutes():
    now = datetime(2026, 6, 3, 14, 19, 59, tzinfo=UTC)
    assert challenge_window(now)["active"] is True
    now = datetime(2026, 6, 3, 14, 20, 0, tzinfo=UTC)
    assert challenge_window(now)["active"] is False


def test_weekday_window_in_gap_still_reports_last_anchor():
    # XX:45 on a weekday — the :00 window expired at :20, the next is at :00
    # of the NEXT hour. The function returns the most-recent :00 anchor and
    # marks active=False.
    now = datetime(2026, 6, 3, 14, 45, tzinfo=UTC)
    w = challenge_window(now)
    assert w["starts_at"] == int(datetime(2026, 6, 3, 14, 0, tzinfo=UTC).timestamp())
    assert w["active"] is False
    assert w["seconds_remaining"] == 0


# --- challenge_window: trove-Friday (half-hourly) cadence ------------------
# Real-UTC Fri 11:00 → Sat 11:00 is trove-Friday. Cycle splits to :00 and :30.


def test_friday_first_half_anchors_to_zero():
    # Sat 04:15 UTC → trove Fri 17:15 → trove-Friday, weekday half-hour cycle.
    now = datetime(2026, 6, 6, 4, 15, tzinfo=UTC)
    w = challenge_window(now)
    assert w["is_friday_window"] is True
    assert w["starts_at"] == int(datetime(2026, 6, 6, 4, 0, tzinfo=UTC).timestamp())
    assert w["active"] is True  # 15 < 20


def test_friday_second_half_anchors_to_thirty():
    now = datetime(2026, 6, 6, 4, 35, tzinfo=UTC)
    w = challenge_window(now)
    assert w["is_friday_window"] is True
    assert w["starts_at"] == int(datetime(2026, 6, 6, 4, 30, tzinfo=UTC).timestamp())
    assert w["active"] is True  # within 30..50


def test_friday_active_only_within_each_half_window():
    # :20 → :30 is a 10-min gap on Fridays.
    now = datetime(2026, 6, 6, 4, 25, tzinfo=UTC)
    w = challenge_window(now)
    assert w["starts_at"] == int(datetime(2026, 6, 6, 4, 0, tzinfo=UTC).timestamp())
    assert w["active"] is False
    # :50 → :00 (next hour) is also a gap.
    now = datetime(2026, 6, 6, 4, 55, tzinfo=UTC)
    w = challenge_window(now)
    assert w["starts_at"] == int(datetime(2026, 6, 6, 4, 30, tzinfo=UTC).timestamp())
    assert w["active"] is False


def test_friday_cadence_drops_off_at_saturday_11_utc():
    # Right BEFORE the trove-day boundary: still on the half-hour cadence.
    now = datetime(2026, 6, 6, 10, 59, tzinfo=UTC)
    assert challenge_window(now)["is_friday_window"] is True
    # Right AFTER: back to hourly. Sat 11:00:00 UTC → trove Sat 00:00:00.
    now = datetime(2026, 6, 6, 11, 0, tzinfo=UTC)
    assert challenge_window(now)["is_friday_window"] is False


# --- seconds_remaining ------------------------------------------------------


def test_seconds_remaining_inside_active_window():
    # 5 minutes into the :00 window → 15 minutes left.
    now = datetime(2026, 6, 3, 14, 5, tzinfo=UTC)
    w = challenge_window(now)
    assert w["seconds_remaining"] == 15 * 60


def test_seconds_remaining_after_window_expired_is_zero():
    now = datetime(2026, 6, 3, 14, 25, tzinfo=UTC)
    assert challenge_window(now)["seconds_remaining"] == 0

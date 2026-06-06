"""Pure tests for the delve rotation: the week-id math (Monday 11:00 UTC rollover,
anchored to week 1 = 2025-11-03) and payload normalization."""

from datetime import datetime, timezone

from app.trove import delves

UTC = timezone.utc


def test_week_id_anchor_and_rollover():
    # Week 1 begins at the anchor (2025-11-03, midnight UTC-11 == 11:00 UTC).
    assert delves.current_week_id(datetime(2025, 11, 3, 11, 0, tzinfo=UTC)) == 1
    # Week 17 begins 16 weeks later: 2026-02-23 11:00 UTC.
    assert delves.current_week_id(datetime(2026, 2, 23, 11, 0, tzinfo=UTC)) == 17
    # One minute before that rollover is still week 16.
    assert delves.current_week_id(datetime(2026, 2, 23, 10, 59, tzinfo=UTC)) == 16
    # A week later is 18.
    assert delves.current_week_id(datetime(2026, 3, 2, 11, 0, tzinfo=UTC)) == 18


def _delay(dt: datetime) -> float:
    return delves._seconds_until_next_pull(dt)


def test_schedule_weekday_pulls_at_daily_reset():
    # Wednesday 14:00 UTC -> next pull at the Trove reset, Thursday 11:00 UTC.
    now = datetime(2026, 6, 3, 14, 0, tzinfo=UTC)  # Wed
    assert _delay(now) == (datetime(2026, 6, 4, 11, 0, tzinfo=UTC) - now).total_seconds()


def test_schedule_weekday_before_reset_is_same_day():
    now = datetime(2026, 6, 3, 9, 0, tzinfo=UTC)  # Wed 09:00 -> today's 11:00 reset
    assert _delay(now) == 2 * 3600


def test_schedule_monday_is_hourly():
    # 2026-06-01 is a Monday; 14:00 UTC is mid-delve-Monday -> next top of hour.
    assert _delay(datetime(2026, 6, 1, 14, 0, tzinfo=UTC)) == 3600


def test_schedule_monday_tail_snaps_to_reset():
    # Tue 10:00 UTC is still the delve-Monday (local Mon 23:00 UTC-11); the last
    # hourly tick coincides with the Tue 11:00 UTC reset.
    assert _delay(datetime(2026, 6, 2, 10, 0, tzinfo=UTC)) == 3600


def test_schedule_monday_morning_utc_is_still_trove_sunday():
    # Mon 09:00 UTC = delve-Sunday (local Sun 22:00 UTC-11) -> pull at the 11:00 reset.
    assert _delay(datetime(2026, 6, 1, 9, 0, tzinfo=UTC)) == 2 * 3600


def test_normalize_payload():
    assert delves.normalize_payload(
        {"depths": [{"id": 1}, {"id": 2}], "total": 5, "page": 1}
    ) == {"depths": [{"id": 1}, {"id": 2}], "total": 5}
    # total defaults to the depth count when absent
    assert delves.normalize_payload({"depths": [{"id": 1}]}) == {"depths": [{"id": 1}], "total": 1}
    assert delves.normalize_payload({"depths": []}) == {"depths": [], "total": 0}
    # junk shapes -> empty
    assert delves.normalize_payload("nope") == {"depths": [], "total": 0}
    assert delves.normalize_payload(None) == {"depths": [], "total": 0}
    assert delves.normalize_payload({"depths": "bad"}) == {"depths": [], "total": 0}

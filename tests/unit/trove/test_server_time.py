from datetime import datetime, timedelta, timezone

from app.trove import server_time as st

UTC = timezone.utc
DAY = 86400


def test_trove_now_offset():
    real = datetime(2024, 6, 1, 15, 0, tzinfo=UTC)
    assert st.trove_now(real) == real - timedelta(hours=11)


def test_server_time_daily_reset():
    # 09:00 UTC -> reset is today 11:00 UTC.
    info = st.server_time(datetime(2024, 6, 1, 9, 0, tzinfo=UTC))
    assert info["daily_reset_at"] == int(datetime(2024, 6, 1, 11, 0, tzinfo=UTC).timestamp())
    # 12:00 UTC (past reset) -> reset is tomorrow 11:00 UTC.
    info = st.server_time(datetime(2024, 6, 1, 12, 0, tzinfo=UTC))
    assert info["daily_reset_at"] == int(datetime(2024, 6, 2, 11, 0, tzinfo=UTC).timestamp())


def test_daily_buff_follows_trove_weekday():
    # 2024-06-03 is a Monday; at 12:00 UTC the trove day is already Monday.
    buff = st.current_daily_buff(datetime(2024, 6, 3, 12, 0, tzinfo=UTC))
    assert buff.get("weekday") == "Monday"
    # Just before 11:00 UTC it's still the previous trove day (Sunday).
    buff = st.current_daily_buff(datetime(2024, 6, 3, 10, 0, tzinfo=UTC))
    assert buff.get("weekday") == "Sunday"


def test_corruxion_active_at_anchor():
    # Exactly at the first corruxion (converted to real UTC) -> active for 3 days.
    now = st.FIRST_CORRUXION + st.TROVE_OFFSET
    t = st.corruxion_timer(now)
    assert t["active"] is True
    assert t["ends_at"] - t["starts_at"] == 3 * DAY
    assert abs(t["seconds_remaining"] - 3 * DAY) <= 1


def test_corruxion_inactive_midcycle():
    # 7 days in: past the 3-day window, next dragon is 7 days out.
    now = st.FIRST_CORRUXION + st.TROVE_OFFSET + timedelta(days=7)
    t = st.corruxion_timer(now)
    assert t["active"] is False
    assert abs(t["seconds_remaining"] - 7 * DAY) <= 1


def test_fluxion_phases():
    voting = st.fluxion_timer(st.FIRST_FLUXION + st.TROVE_OFFSET)
    assert voting["active"] is True and voting["state"] == "voting"
    selling = st.fluxion_timer(st.FIRST_FLUXION + st.TROVE_OFFSET + timedelta(days=7))
    assert selling["active"] is True and selling["state"] == "selling"


def test_invasion_active_at_anchor():
    t = st.invasion_timer(st.FIRST_INVASION + st.TROVE_OFFSET)
    assert t["active"] is True
    assert t["ends_at"] - t["starts_at"] == 3 * 3600  # 3-hour window


def test_calendar_snapshot_shape():
    snap = st.calendar_snapshot(datetime(2024, 6, 1, 12, 0, tzinfo=UTC))
    assert set(snap) == {"server_time", "daily", "weekly", "merchants"}
    assert set(snap["merchants"]) == {"corruxion", "fluxion", "invasion"}
    for m in snap["merchants"].values():
        assert {"active", "starts_at", "ends_at", "seconds_remaining"} <= set(m)
        assert m["ends_at"] >= m["starts_at"] and m["seconds_remaining"] >= 0

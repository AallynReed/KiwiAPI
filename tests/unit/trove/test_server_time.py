from datetime import datetime, timedelta, timezone

from app.trove import server_time as st

UTC = timezone.utc
DAY = 86400


def test_trove_now_offset():
    real = datetime(2024, 6, 1, 15, 0, tzinfo=UTC)
    assert st.trove_now(real) == real - timedelta(hours=11)


def test_server_time():
    info = st.server_time(datetime(2024, 6, 1, 9, 0, tzinfo=UTC))
    assert info["daily_reset_at"] == int(datetime(2024, 6, 1, 11, 0, tzinfo=UTC).timestamp())
    assert info["now_iso"].startswith("2024-06-01T09:00")
    info = st.server_time(datetime(2024, 6, 1, 12, 0, tzinfo=UTC))
    assert info["daily_reset_at"] == int(datetime(2024, 6, 2, 11, 0, tzinfo=UTC).timestamp())


def test_daily_buffs():
    buffs = st.daily_buffs(datetime(2024, 6, 3, 12, 0, tzinfo=UTC))  # Monday after 11:00
    assert buffs["current"]["weekday"] == "Monday"
    assert len(buffs["week"]) == 7
    # Before 11:00 it's still the previous trove day.
    assert st.daily_buffs(datetime(2024, 6, 3, 10, 0, tzinfo=UTC))["current"]["weekday"] == "Sunday"


def test_weekly_buffs():
    wk = st.weekly_buffs(datetime(2024, 6, 1, 12, 0, tzinfo=UTC))
    assert wk["current"]["name"]
    assert len(wk["rotation"]) == 4


def test_corruxion_active_at_anchor():
    c = st.corruxion(st.FIRST_CORRUXION + st.TROVE_OFFSET)
    assert c["active"] is True
    assert c["ends_at"] - c["starts_at"] == 3 * DAY
    assert abs(c["seconds_remaining"] - 3 * DAY) <= 1
    # Schedule: 8 windows, 14 days apart, starting at the current one.
    assert len(c["schedule"]) == 8
    assert c["schedule"][0]["starts_at"] == c["starts_at"]
    assert c["schedule"][1]["starts_at"] - c["schedule"][0]["starts_at"] == 14 * DAY


def test_corruxion_inactive_midcycle():
    c = st.corruxion(st.FIRST_CORRUXION + st.TROVE_OFFSET + timedelta(days=7))
    assert c["active"] is False
    assert abs(c["seconds_remaining"] - 7 * DAY) <= 1


def test_fluxion_phases_and_schedule():
    voting = st.fluxion(st.FIRST_FLUXION + st.TROVE_OFFSET)
    assert voting["active"] is True and voting["state"] == "voting"
    assert voting["schedule"][0]["state"] == "voting"
    assert voting["schedule"][1]["state"] == "selling"  # alternates every window
    selling = st.fluxion(st.FIRST_FLUXION + st.TROVE_OFFSET + timedelta(days=7))
    assert selling["active"] is True and selling["state"] == "selling"


def test_gardening_shape():
    g = st.gardening(datetime(2025, 6, 1, 12, 0, tzinfo=UTC))
    assert g["two_day"]["name"] == "2-day plants"
    assert g["three_day"]["name"] == "3-day plants"
    for w in (g["two_day"], g["three_day"]):
        assert w["ends_at"] - w["starts_at"] == DAY  # harvest window is 1 day
    starts = [u["starts_at"] for u in g["upcoming"]]
    assert starts == sorted(starts)  # upcoming windows are ordered

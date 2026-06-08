"""Unit tests for ``normalize_timestamp`` after the relaxation that
lets hourly bot captures land at their own per-minute anchors.
"""
from __future__ import annotations

from datetime import UTC, datetime, timedelta

from app.trove.leaderboards.service import normalize_timestamp


def _now_unix() -> int:
    return int(datetime.now(UTC).replace(second=0, microsecond=0).timestamp())


def test_rejects_none_or_zero():
    assert normalize_timestamp(None) == -1
    assert normalize_timestamp(0) == -1
    assert normalize_timestamp(-1) == -1


def test_accepts_recent_minute_aligned_timestamp():
    """The bot will pass int(time.time()) on each hourly capture. The
    helper must snap to the minute and accept it."""
    now = datetime.now(UTC).replace(second=42, microsecond=0)
    ts = int(now.timestamp())
    norm = normalize_timestamp(ts)
    # Seconds must be stripped
    parsed = datetime.fromtimestamp(norm, UTC)
    assert parsed.second == 0
    # Stays within the same minute
    assert abs(norm - ts) < 60


def test_accepts_arbitrary_hour():
    """Previously only 11:00 / 00:00 UTC were accepted. Now any hour
    is fine as long as it's within the back-fill window."""
    twelve_thirty_today = datetime.now(UTC).replace(
        hour=12, minute=30, second=0, microsecond=0,
    )
    if twelve_thirty_today > datetime.now(UTC):
        twelve_thirty_today -= timedelta(days=1)
    ts = int(twelve_thirty_today.timestamp())
    norm = normalize_timestamp(ts)
    assert norm == ts


def test_legacy_alias_00_utc_translates_to_11_utc():
    """Back-compat: a back-fill at 00:00 UTC of a recent day still
    becomes that day's 11:00 anchor."""
    today_00 = datetime.now(UTC).replace(hour=0, minute=0, second=0, microsecond=0)
    today_11 = today_00.replace(hour=11)
    if today_11 > datetime.now(UTC):
        # 00:00 anchor must remain in the back-window
        today_00 -= timedelta(days=1)
        today_11 -= timedelta(days=1)
    norm = normalize_timestamp(int(today_00.timestamp()))
    assert norm == int(today_11.timestamp())


def test_rejects_far_past():
    """Older than the back-fill window → reject. Stops a malicious
    caller from dumping entries at arbitrary historical anchors."""
    long_ago = datetime.now(UTC) - timedelta(days=30)
    assert normalize_timestamp(int(long_ago.timestamp())) == -1


def test_rejects_far_future():
    """A clock-skewed bot might be a few seconds ahead, but not more
    than the 5-minute guard. Reject anything beyond that."""
    future = datetime.now(UTC) + timedelta(minutes=10)
    assert normalize_timestamp(int(future.timestamp())) == -1


def test_accepts_within_window_back():
    """Within the 14-day back-fill window: accepted."""
    five_days_ago = datetime.now(UTC) - timedelta(days=5)
    ts = int(five_days_ago.timestamp())
    norm = normalize_timestamp(ts)
    # Same minute (seconds stripped)
    assert abs(norm - ts) < 60
    # Not -1
    assert norm > 0


def test_seconds_stripped_consistently():
    """Two timestamps within the same minute should normalize to the
    same value — important so the bot can submit at HH:MM:00 vs
    HH:MM:42 and land at the same anchor row."""
    base = datetime.now(UTC).replace(second=0, microsecond=0) - timedelta(hours=1)
    ts_a = int(base.timestamp())
    ts_b = int((base.replace(second=42)).timestamp())
    assert normalize_timestamp(ts_a) == normalize_timestamp(ts_b)

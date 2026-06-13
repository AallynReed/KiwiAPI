"""Gap detection for the activity estimate - windows spanning a missed capture
are skipped (not normalized) so the graph doesn't miscalculate over a gap.

The cutoff is NOT a hardcoded 1h: it's derived from the actual capture cadence
(median spacing) so a 2-hourly or jittery archive isn't chopped to pieces - only
genuinely-missed captures (markedly longer than the median) are dropped."""
from app.trove.leaderboards.activity import (
    _GAP_FACTOR,
    _GAP_FLOOR_HOURS,
    _gap_threshold_hours,
    _intervals_hours,
    _is_gap,
    _median,
)


def _expected_threshold(median: float) -> float:
    return max(_GAP_FLOOR_HOURS, median * _GAP_FACTOR)


# --- default-threshold (floor) behaviour, i.e. tight hourly data ---------------

def test_consecutive_windows_are_not_gaps():
    assert not _is_gap(1.0)                    # the normal hourly pair
    assert not _is_gap(1.25)                   # a little capture jitter is fine
    assert not _is_gap(_GAP_FLOOR_HOURS)       # exactly the floor is allowed


def test_multi_hour_windows_are_gaps():
    assert _is_gap(_GAP_FLOOR_HOURS + 0.01)    # just over the floor
    assert _is_gap(2.0)                         # one missed capture (hourly cadence)
    assert _is_gap(3.0)                         # two missed captures


def test_missing_or_zero_duration_is_not_a_gap():
    assert not _is_gap(None)
    assert not _is_gap(0.0)
    assert not _is_gap(None, 10.0)


# --- explicit threshold ------------------------------------------------------

def test_threshold_argument_is_respected():
    # A 2h window is a gap at the hourly floor but NOT under a 2-hourly cadence.
    assert _is_gap(2.0, _GAP_FLOOR_HOURS)
    assert not _is_gap(2.0, 3.8)               # 2-hourly cadence threshold
    assert _is_gap(5.0, 3.8)                   # a real miss even at 2-hourly


# --- median + interval helpers -----------------------------------------------

def test_median():
    assert _median([]) is None
    assert _median([2.0]) == 2.0
    assert _median([1.0, 3.0]) == 2.0          # even count -> mean of middle two
    assert _median([1.0, 2.0, 9.0]) == 2.0     # robust to the outlier
    assert _median([0.0, -1.0, 2.0]) == 2.0    # non-positive values ignored


def test_intervals_hours():
    # 11:00, 12:00, 14:00 -> 1h then 2h spacings.
    anchors = [1_000_000, 1_000_000 + 3600, 1_000_000 + 3 * 3600]
    assert _intervals_hours(anchors) == [1.0, 2.0]
    assert _intervals_hours([42]) == []        # single capture -> no intervals
    assert _intervals_hours([]) == []


# --- the adaptive cutoff ------------------------------------------------------

def test_gap_threshold_adapts_to_cadence():
    # Hourly cadence -> threshold = median*factor (floored).
    hourly = [1.0] * 20
    thr_hourly = _gap_threshold_hours(hourly)
    assert thr_hourly == _expected_threshold(1.0)
    assert _is_gap(2.0, thr_hourly)            # a real missed capture at hourly cadence
    assert not _is_gap(1.25, thr_hourly)       # jitter within cadence is fine

    # 2-hourly cadence -> threshold scales up so 2h windows aren't dropped.
    two_hourly = [2.0] * 20
    thr = _gap_threshold_hours(two_hourly)
    assert thr == _expected_threshold(2.0)
    assert thr > 2.0
    assert not _is_gap(2.0, thr)
    assert _is_gap(4.5, thr)                    # a genuine miss at 2-hourly

    # No data -> falls back to the floor.
    assert _gap_threshold_hours([]) == _GAP_FLOOR_HOURS


def test_gap_threshold_robust_to_occasional_gaps():
    # Mostly hourly with a couple of real gaps mixed in: the median (and thus the
    # threshold) still reflects the hourly cadence, so the real gaps stay classified.
    intervals = [1.0] * 18 + [2.0, 3.0]
    thr = _gap_threshold_hours(intervals)
    assert thr == _expected_threshold(1.0)
    assert _is_gap(2.0, thr)                    # 2h > hourly threshold -> still a gap

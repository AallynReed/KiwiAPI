"""Class Activity: class↔board mapping, per-class union/dedup, share, series shape.

No DB - the compute primitives are pure (maps in, dicts out); the one async test
monkeypatches the pg_store/cache reads. Mirrors test_activity_compute.py."""
import datetime as _dt
import time

from app.trove import stats
from app.trove.leaderboards import class_activity as ca
from app.trove.leaderboards import service as lb_service

UTC = _dt.timezone.utc


def _ts(dt: _dt.datetime) -> int:
    return int(dt.timestamp())


def _non_reset_window() -> tuple[int, int]:
    """A 1h window that does NOT cross a weekly (Mon 11:00 UTC) reset."""
    late = _dt.datetime(2026, 6, 3, 13, 0, tzinfo=UTC)   # a Wednesday-ish start
    for _ in range(8):
        e, l = _ts(late) - 3600, _ts(late)
        if not lb_service.reset_boundaries_for_kind("weekly", e, l):
            return e, l
        late += _dt.timedelta(days=1)
    raise AssertionError("no non-reset window found")


def _monday_crossing_window() -> tuple[int, int]:
    """A 1h window that straddles a Monday 11:00 UTC weekly reset."""
    base = _dt.datetime(2026, 6, 1, 11, 0, tzinfo=UTC)
    while base.weekday() != 0:        # 0 == Monday
        base += _dt.timedelta(days=1)
    mon = _ts(base)
    return mon - 1800, mon + 1800


# --- class ↔ board mapping --------------------------------------------------

def test_class_board_mapping():
    assert stats.class_count() == 18
    assert stats.class_index_for_board(4000) == 0
    assert stats.class_index_for_board(5000) == 0
    assert stats.class_index_for_board(4017) == 17
    assert stats.class_index_for_board(5017) == 17
    assert stats.class_name(0) == "Bard"
    assert stats.class_name(17) == "Vanguardian"
    boards = set(stats.class_board_uuids())
    assert len(boards) == 36
    assert {4000, 4017, 5000, 5017} <= boards


# --- _class_counts: union + dedup -------------------------------------------

def test_class_counts_unions_and_dedups_two_boards():
    early, late = _non_reset_window()
    # Class 2 (Candy Barbarian): Effort 4002 + Paragon 5002.
    late_maps = {
        4002: {"Both": 100.0, "OnlyEffort": 50.0, "Flat": 10.0},
        5002: {"Both": 200.0, "OnlyParagon": 30.0},
    }
    early_maps = {
        4002: {"Both": 90.0, "OnlyEffort": 49.0, "Flat": 10.0},
        5002: {"Both": 150.0},                       # OnlyParagon first-appears
    }
    counts = ca._class_counts(early_maps, late_maps, early, late)
    # Both (rose on both → counted once), OnlyEffort (rose), OnlyParagon (new).
    # Flat is unchanged → not active. → 3 distinct for class 2; no other class.
    assert counts == {2: 3}


def test_player_active_on_multiple_classes_counts_in_each():
    early, late = _non_reset_window()
    late_maps = {4000: {"X": 10.0}, 4001: {"X": 10.0}, 4002: {"X": 10.0}}
    early_maps = {4000: {"X": 5.0}, 4001: {"X": 5.0}, 4002: {"X": 5.0}}
    # The same player counts toward EACH class - share is share-of-activity,
    # not distinct players.
    assert ca._class_counts(early_maps, late_maps, early, late) == {0: 1, 1: 1, 2: 1}


def test_reset_crossing_window_yields_no_counts():
    early, late = _monday_crossing_window()
    assert lb_service.reset_boundaries_for_kind("weekly", early, late)  # crosses
    late_maps = {4000: {"X": 10.0}, 5000: {"X": 10.0}}
    early_maps = {4000: {"X": 5.0}, 5000: {"X": 5.0}}
    # Both boards reset inside the window → unmeasurable → no class entry (gap,
    # not a false 0).
    assert ca._class_counts(early_maps, late_maps, early, late) == {}


# --- share normalization ----------------------------------------------------

def test_build_current_share_sums_to_one_and_sorts_desc():
    cur = ca._build_current(1000, 4600, 1.0, {0: 30, 5: 20, 9: 50}, 9999)
    assert cur["total_active"] == 100
    assert abs(sum(c["share"] for c in cur["classes"]) - 1.0) < 1e-6
    assert [c["class_index"] for c in cur["classes"]] == [9, 0, 5]   # by active desc
    assert cur["classes"][0]["name"] == stats.class_name(9)
    assert cur["classes"][0]["share"] == 0.5
    # self-hosted icon URL per class
    assert cur["classes"][0]["icon"] == "/static/class-icons/knight.png"
    assert stats.class_icon(0) == "/static/class-icons/bard.png"


def test_build_current_empty_counts():
    cur = ca._build_current(None, None, None, {}, 9999)
    assert cur["total_active"] is None
    assert cur["classes"] == []


# --- series bucketing shape (async, monkeypatched reads) --------------------

async def test_series_shared_buckets_and_aligned_values(monkeypatch):
    from app.trove.leaderboards import cache as lb_cache
    from app.trove.leaderboards import pg_store

    now = int(time.time())
    we1, we2 = now - 7200, now - 3600   # two hourly windows → two 3600s buckets
    rows = [
        {"class_index": 0, "window_end": we1, "window_start": we1 - 3600,
         "duration_hours": 1.0, "estimate": 10, "computed_at": now},
        {"class_index": 1, "window_end": we1, "window_start": we1 - 3600,
         "duration_hours": 1.0, "estimate": 5, "computed_at": now},
        {"class_index": 0, "window_end": we2, "window_start": we2 - 3600,
         "duration_hours": 1.0, "estimate": 20, "computed_at": now},
        # class 1 absent in we2 (its value there must be null)
    ]

    async def fake_get(window_start=None):
        return rows

    async def fake_cache_get(period):
        return None

    async def fake_cache_set(period, payload):
        return None

    monkeypatch.setattr(pg_store, "get_class_estimates", fake_get)
    monkeypatch.setattr(lb_cache, "get_class_activity_series", fake_cache_get)
    monkeypatch.setattr(lb_cache, "set_class_activity_series", fake_cache_set)

    out = await ca.class_activity_series("1d")
    nb = len(out["buckets"])
    assert nb == 2
    # All classes present for a stable legend; every line aligns to `buckets`.
    assert len(out["classes"]) == stats.class_count()
    for c in out["classes"]:
        assert len(c["values"]) == nb
    by_idx = {c["class_index"]: c for c in out["classes"]}
    assert all(c["icon"] for c in out["classes"])               # every line carries an icon URL
    assert all(v is not None for v in by_idx[0]["values"])      # class 0 in both buckets
    assert by_idx[1]["values"].count(None) == 1                 # class 1 in one bucket only
    assert all(v is None for v in by_idx[5]["values"])          # class 5 never present

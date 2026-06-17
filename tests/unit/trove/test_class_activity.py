"""Class Activity: class↔board mapping, per-class dedup, the "clean"
(established) filter (Power Rank + Effort floors; Effort-only basis, Paragon
excluded), share normalization, and series shape (raw + clean).

No DB - the compute primitives are pure (maps in, dicts out); the one async test
monkeypatches the pg_store/cache reads + the threshold lookup. Mirrors
test_activity_compute.py."""
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


def test_power_rank_board_mapping():
    # Power Rank boards are the 1000+i parallel range (the clean-view gate).
    assert stats.class_pr_board_uuid(0) == 1000
    assert stats.class_pr_board_uuid(17) == 1017
    prs = stats.class_pr_board_uuids()
    assert len(prs) == 18
    assert prs[0] == 1000 and prs[-1] == 1017
    assert stats.class_index_for_board(1002) == 2   # uuid % 1000 still holds


def test_effort_board_mapping():
    # Effort (4000+i) is the sole basis for class-activity counts; the Effort-only
    # board list backs every load. (Paragon mapping still exists but is unused.)
    assert stats.class_effort_board_uuid(0) == 4000
    assert stats.class_effort_board_uuid(17) == 4017
    eff = stats.class_effort_board_uuids()
    assert len(eff) == 18 and eff[0] == 4000 and eff[-1] == 4017
    assert all(4000 <= u <= 4017 for u in eff)        # no Paragon (5000+i) in the list
    assert stats.class_paragon_board_uuid(0) == 5000  # helper kept, but not used in counts


# --- _class_counts: union + dedup (raw) -------------------------------------

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
    # No pr_maps passed → clean is unmeasurable (None).
    assert counts == {2: {"raw": 3, "clean": None}}


def test_player_active_on_multiple_classes_counts_in_each():
    early, late = _non_reset_window()
    late_maps = {4000: {"X": 10.0}, 4001: {"X": 10.0}, 4002: {"X": 10.0}}
    early_maps = {4000: {"X": 5.0}, 4001: {"X": 5.0}, 4002: {"X": 5.0}}
    # The same player counts toward EACH class - share is share-of-activity,
    # not distinct players.
    out = ca._class_counts(early_maps, late_maps, early, late)
    assert {i: c["raw"] for i, c in out.items()} == {0: 1, 1: 1, 2: 1}


def test_reset_crossing_window_yields_no_counts():
    early, late = _monday_crossing_window()
    assert lb_service.reset_boundaries_for_kind("weekly", early, late)  # crosses
    late_maps = {4000: {"X": 10.0}, 5000: {"X": 10.0}}
    early_maps = {4000: {"X": 5.0}, 5000: {"X": 5.0}}
    # Both boards reset inside the window → unmeasurable → no class entry (gap,
    # not a false 0).
    assert ca._class_counts(early_maps, late_maps, early, late) == {}


# --- _class_counts: the Power-Rank "clean" filter ---------------------------

def test_class_counts_clean_filters_by_power_rank():
    early, late = _non_reset_window()
    # Class 2: three players rise on Effort 4002.
    late_maps = {4002: {"Vet": 100.0, "Whale": 80.0, "Newbie": 50.0}}
    early_maps = {4002: {"Vet": 90.0, "Whale": 70.0, "Newbie": 49.0}}
    # Power Rank board 1002: Vet + Whale clear 25k; Newbie (5k) does not.
    pr_maps = {1002: {"Vet": 40000.0, "Whale": 26000.0, "Newbie": 5000.0}}
    c = ca._class_counts(early_maps, late_maps, early, late, pr_maps=pr_maps, threshold=25000)
    assert c[2]["raw"] == 3
    assert c[2]["clean"] == 2       # Newbie filtered out of the clean count

    # A player absent from the PR board counts as 0 PR → filtered.
    pr_missing_player = {1002: {"Vet": 40000.0}}
    c2 = ca._class_counts(early_maps, late_maps, early, late,
                          pr_maps=pr_missing_player, threshold=25000)
    assert c2[2]["clean"] == 1      # only Vet

    # PR board absent for the class → clean unmeasurable (None, the line gaps).
    c3 = ca._class_counts(early_maps, late_maps, early, late, pr_maps={}, threshold=25000)
    assert c3[2]["clean"] is None

    # threshold 0 → clean == raw (no filtering).
    c4 = ca._class_counts(early_maps, late_maps, early, late, pr_maps=pr_maps, threshold=0)
    assert c4[2]["clean"] == 3


def test_class_counts_clean_filters_by_power_rank_and_effort():
    early, late = _non_reset_window()
    # Class 2: four players all RISE on Effort 4002 (raw = 4). No Paragon board is
    # loaded any more - counts and the clean gate are Effort-only.
    late_maps = {4002: {"A": 100.0, "B": 40.0, "C": 100.0, "D": 100.0}}
    early_maps = {4002: {"A": 90.0, "B": 30.0, "C": 90.0, "D": 90.0}}   # all rose
    pr_maps = {1002: {"A": 40000.0, "B": 40000.0, "C": 40000.0, "D": 5000.0}}
    c = ca._class_counts(early_maps, late_maps, early, late, pr_maps=pr_maps,
                         threshold=25000, effort_threshold=50)
    assert c[2]["raw"] == 4
    # A + C clear both floors; B fails Effort (40<50); D fails Power Rank (5000<25000).
    assert c[2]["clean"] == 2

    # Effort floor at 0 -> Power-Rank-only: B (good PR) re-enters, only D is filtered.
    c2 = ca._class_counts(early_maps, late_maps, early, late, pr_maps=pr_maps,
                          threshold=25000, effort_threshold=0)
    assert c2[2]["clean"] == 3   # A, B, C (PR>=25k); D excluded


# --- share normalization + clean view ---------------------------------------

def test_build_current_raw_and_clean_views():
    counts = {
        0: {"raw": 30, "clean": 10},
        5: {"raw": 20, "clean": None},   # clean unmeasurable for this class
        9: {"raw": 50, "clean": 40},
    }
    cur = ca._build_current(1000, 4600, 1.0, counts, 9999, 25000, 50)
    assert cur["total_active"] == 100
    assert cur["total_active_clean"] == 50          # 10 + 40 (class 5 excluded)
    assert cur["power_rank_threshold"] == 25000
    assert cur["effort_threshold"] == 50
    assert "paragon_threshold" not in cur           # Paragon removed from the pipeline
    # No effort deltas in these counts → effort fields are null, not 0.
    assert cur["total_effort_added"] is None and cur["total_effort_added_clean"] is None
    assert cur["classes"][0]["effort_added"] is None

    # Default ordering is the clean view: clean desc, classes w/ None clean last.
    assert [c["class_index"] for c in cur["classes"]] == [9, 0, 5]
    top = cur["classes"][0]
    assert top["class_index"] == 9
    assert top["active_players"] == 50
    assert top["active_players_clean"] == 40
    assert abs(top["share"] - 0.5) < 1e-6            # raw share
    assert abs(top["share_clean"] - 40 / 50) < 1e-6  # clean share

    # Raw shares sum to 1; clean shares (over measurable classes) sum to 1.
    assert abs(sum(c["share"] for c in cur["classes"]) - 1.0) < 1e-6
    clean_shares = [c["share_clean"] for c in cur["classes"] if c["share_clean"] is not None]
    assert abs(sum(clean_shares) - 1.0) < 1e-6

    # Class 5: clean None → both clean fields None.
    c5 = next(c for c in cur["classes"] if c["class_index"] == 5)
    assert c5["active_players_clean"] is None
    assert c5["share_clean"] is None

    # self-hosted icon URL per class
    assert top["icon"] == "/static/class-icons/knight.png"
    assert stats.class_icon(0) == "/static/class-icons/bard.png"


def test_build_current_empty_counts():
    cur = ca._build_current(None, None, None, {}, 9999, 25000)
    assert cur["total_active"] is None
    assert cur["total_active_clean"] is None
    assert cur["classes"] == []


# --- the donut: a direct snapshot headcount (no activity delta) --------------

def test_snapshot_counts_presence_and_floors():
    # A single snapshot: who is PRESENT on each class's EFFORT board, not who rose.
    effort_maps = {
        4000: {"A": 100.0, "B": 40.0},   # class 0 Effort scores
        4002: {"X": 200.0},              # class 2 Effort
    }
    pr = {1000: {"A": 40000.0, "B": 40000.0}}   # no 1002 -> class 2 PR absent
    c = ca._snapshot_counts(effort_maps, pr, 25000, 50)
    assert c[0]["raw"] == 2              # A, B present on class 0's Effort board
    # established: A clears both floors; B fails Effort (40<50).
    assert c[0]["clean"] == 1
    assert c[2]["raw"] == 1              # X present on class 2
    assert c[2]["clean"] is None         # class 2's Power Rank board absent -> unmeasurable
    assert 5 not in c                    # a class with no players is omitted (no slice)


def test_effort_deltas_added_per_view():
    # Effort added this hour = Σ positive per-player gains over players in BOTH snaps.
    late = {4000: {"A": 1100.0, "B": 600.0, "N": 500.0}}   # N is a new entrant
    early = {4000: {"A": 1000.0, "B": 550.0}}              # N absent last hour
    pr = {1000: {"A": 40000.0, "B": 5000.0, "N": 40000.0}}
    d = ca._effort_deltas(late, early, pr, 25000, 50)
    assert d[0]["raw"] == 150            # A +100, B +50; N excluded (new entrant)
    assert d[0]["clean"] == 100          # only A clears PR (B's PR 5000 < 25000)
    assert 1 not in d                    # class with no late board omitted

    # PR board absent for the class → clean unmeasurable (None), raw still measured.
    d2 = ca._effort_deltas(late, early, {}, 25000, 50)
    assert d2[0]["raw"] == 150 and d2[0]["clean"] is None

    # A negative/flat gain is clamped to 0 (data corrections never subtract).
    d3 = ca._effort_deltas({4000: {"A": 900.0}}, {4000: {"A": 1000.0}},
                           {1000: {"A": 40000.0}}, 0, 0)
    assert d3[0]["raw"] == 0


async def test_current_donut_is_direct_snapshot(monkeypatch):
    from app.trove.leaderboards import activity as _act
    from app.trove.leaderboards import cache as lb_cache
    from app.trove.leaderboards import service as lb_service

    anchor = 1_700_000_000

    async def fake_stamps(limit=1, include_archive=True):
        return [anchor]

    async def fake_thresholds():
        return (25000, 50)

    async def fake_load(a, board_uuids):
        # PR boards vs Effort boards, by which range was requested (no Paragon).
        if 1000 in set(board_uuids):
            return {1000: {"A": 40000.0, "B": 40000.0}}
        return {4000: {"A": 100.0, "B": 40.0}}

    async def fake_cache_get():
        return None

    async def fake_cache_set(payload):
        return None

    monkeypatch.setattr(lb_service, "list_timestamps", fake_stamps)
    monkeypatch.setattr(ca, "_clean_thresholds", fake_thresholds)
    monkeypatch.setattr(_act, "_load_anchor_maps", fake_load)
    monkeypatch.setattr(lb_cache, "get_class_activity_current", fake_cache_get)
    monkeypatch.setattr(lb_cache, "set_class_activity_current", fake_cache_set)

    cur = await ca.class_activity_current()
    # snapshot, not a window: both bounds are the anchor, no duration.
    assert cur["window_start"] == anchor and cur["window_end"] == anchor
    assert cur["duration_hours"] is None
    assert cur["total_active"] == 2          # A + B present on class 0's Effort board
    assert cur["total_active_clean"] == 1    # only A clears both floors
    assert (cur["power_rank_threshold"], cur["effort_threshold"]) == (25000, 50)
    assert "paragon_threshold" not in cur
    assert "snapshot" in cur["methodology"].lower()
    c0 = next(c for c in cur["classes"] if c["class_index"] == 0)
    assert c0["active_players"] == 2 and c0["active_players_clean"] == 1
    assert abs(c0["share"] - 1.0) < 1e-9     # class 0 is the only one with players
    # Only one stamp → no previous capture → effort deltas are null (not 0).
    assert cur["total_effort_added"] is None and c0["effort_added"] is None


async def test_current_donut_effort_added(monkeypatch):
    from app.trove.leaderboards import activity as _act
    from app.trove.leaderboards import cache as lb_cache
    from app.trove.leaderboards import service as lb_service

    late, early = 1_700_003_600, 1_700_000_000   # two consecutive captures

    async def fake_stamps(limit=2, include_archive=True):
        return [late, early]

    async def fake_thresholds():
        return (25000, 50)

    async def fake_load(a, board_uuids):
        if 1000 in set(board_uuids):                       # PR boards (loaded at `late`)
            return {1000: {"A": 40000.0, "B": 5000.0, "N": 40000.0}}
        if a == late:
            return {4000: {"A": 1100.0, "B": 600.0, "N": 500.0}}   # N is new this hour
        return {4000: {"A": 1000.0, "B": 550.0}}                   # early (N absent)

    async def fake_cache_get():
        return None

    async def fake_cache_set(payload):
        return None

    monkeypatch.setattr(lb_service, "list_timestamps", fake_stamps)
    monkeypatch.setattr(lb_service, "reset_boundaries_for_kind", lambda kind, e, l: [])
    monkeypatch.setattr(ca, "_clean_thresholds", fake_thresholds)
    monkeypatch.setattr(_act, "_load_anchor_maps", fake_load)
    monkeypatch.setattr(lb_cache, "get_class_activity_current", fake_cache_get)
    monkeypatch.setattr(lb_cache, "set_class_activity_current", fake_cache_set)

    cur = await ca.class_activity_current()
    c0 = next(c for c in cur["classes"] if c["class_index"] == 0)
    # Effort added: A +100, B +50 (raw 150); only A clears PR -> clean 100. N excluded.
    assert c0["effort_added"] == 150
    assert c0["effort_added_clean"] == 100
    assert cur["total_effort_added"] == 150
    assert cur["total_effort_added_clean"] == 100


# --- series bucketing shape (async, monkeypatched reads) --------------------

async def test_series_shared_buckets_and_aligned_values(monkeypatch):
    from app.trove.leaderboards import cache as lb_cache
    from app.trove.leaderboards import pg_store

    now = int(time.time())
    we1, we2 = now - 7200, now - 3600   # two hourly windows → two 3600s buckets
    rows = [
        {"class_index": 0, "window_end": we1, "window_start": we1 - 3600,
         "duration_hours": 1.0, "estimate": 10, "estimate_clean": 6, "computed_at": now},
        {"class_index": 1, "window_end": we1, "window_start": we1 - 3600,
         "duration_hours": 1.0, "estimate": 5, "estimate_clean": None, "computed_at": now},
        {"class_index": 0, "window_end": we2, "window_start": we2 - 3600,
         "duration_hours": 1.0, "estimate": 20, "estimate_clean": 12, "computed_at": now},
        # class 1 absent in we2 (its value there must be null)
    ]

    async def fake_get(window_start=None):
        return rows

    async def fake_cache_get(period):
        return None

    async def fake_cache_set(period, payload):
        return None

    async def fake_thresholds():
        return (25000, 50)

    monkeypatch.setattr(pg_store, "get_class_estimates", fake_get)
    monkeypatch.setattr(lb_cache, "get_class_activity_series", fake_cache_get)
    monkeypatch.setattr(lb_cache, "set_class_activity_series", fake_cache_set)
    monkeypatch.setattr(ca, "_clean_thresholds", fake_thresholds)

    out = await ca.class_activity_series("1d")
    nb = len(out["buckets"])
    assert nb == 2
    assert out["power_rank_threshold"] == 25000
    assert out["effort_threshold"] == 50
    assert "paragon_threshold" not in out
    # All classes present for a stable legend; every line aligns to `buckets`.
    assert len(out["classes"]) == stats.class_count()
    for c in out["classes"]:
        assert len(c["values"]) == nb
        assert len(c["values_clean"]) == nb
    by_idx = {c["class_index"]: c for c in out["classes"]}
    assert all(c["icon"] for c in out["classes"])                 # every line carries an icon URL
    assert all(v is not None for v in by_idx[0]["values"])        # class 0 raw in both buckets
    assert all(v is not None for v in by_idx[0]["values_clean"])  # class 0 clean in both buckets
    assert by_idx[1]["values"].count(None) == 1                  # class 1 raw in one bucket only
    # class 1's only stored window had estimate_clean=None → clean line all gaps.
    assert all(v is None for v in by_idx[1]["values_clean"])
    assert all(v is None for v in by_idx[5]["values"])           # class 5 never present

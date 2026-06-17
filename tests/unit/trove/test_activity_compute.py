"""Regression tests for the live activity estimate (`_compute`) + rollups.

Definition under test:
  * a player is "active" in a capture window when their score ROSE on some board
    vs the previous capture, OR they newly appear with a non-zero score
    (``score > prev``, absent-previous treated as 0). No score ceilings, no board
    filters; a reset (score drops) isn't activity;
  * the 24h / 7d rollups are the distinct UNION of each capture's active set,
    materialized per window (here faked in memory) and counted - monotonic by
    construction (7d ⊇ 24h ⊇ 1h).
"""
import pytest

from app.trove.leaderboards import activity, pg_store

pytestmark = pytest.mark.asyncio


def _patch(monkeypatch, boards, maps_by_anchor):
    async def fake_list_boards_at(anchor):
        return boards

    async def fake_list_timestamps(limit=60, include_archive=True):
        return sorted(maps_by_anchor, reverse=True)

    async def fake_load_anchor_maps(anchor, board_uuids):
        return maps_by_anchor.get(anchor, {})

    async def fake_upsert(*a, **k):
        return None

    # In-memory stand-in for the materialized activity_active table: each window's
    # active set is stored once; the rollup unions the windows in range.
    materialized: dict[int, set] = {}

    async def fake_record(window_end, names):
        materialized[window_end] = {n.lower() for n in names}

    async def fake_count(early, late):
        u: set = set()
        for we, s in materialized.items():
            if early < we <= late:           # (early, late]
                u |= s
        return len(u)

    async def fake_prune(cutoff):
        for we in [w for w in materialized if w < cutoff]:
            del materialized[we]
        return 0

    monkeypatch.setattr(activity.lb_service, "list_boards_at", fake_list_boards_at)
    monkeypatch.setattr(activity.lb_service, "list_timestamps", fake_list_timestamps)
    monkeypatch.setattr(activity, "_load_anchor_maps", fake_load_anchor_maps)
    monkeypatch.setattr(pg_store, "upsert_estimate", fake_upsert)
    monkeypatch.setattr(pg_store, "record_active_window", fake_record)
    monkeypatch.setattr(pg_store, "count_active_since", fake_count)
    monkeypatch.setattr(pg_store, "prune_active_windows", fake_prune)


async def test_compute_counts_rise_or_nonzero_appearance(monkeypatch):
    late, early = 2_000_000, 2_000_000 - 3600          # 1h apart
    board = 100
    boards = [{
        "uuid": board, "name": "Effort", "category": "Stats",
        "player_board": True, "reset_kind": "default",
    }]
    maps = {
        late:  {board: {"Alice": 120.0, "Bob": 50.0, "Carol": 30.0}},
        early: {board: {"Alice": 100.0, "Bob": 50.0}},
    }
    _patch(monkeypatch, boards, maps)

    res = await activity._compute(late, early)

    # Alice rose (100->120) -> active. Carol newly appears with a non-zero score
    # (30 > 0) -> active. Bob is flat (50 == 50) -> not active. No score ceiling.
    assert res["estimate"] == 2
    assert {b["uuid"]: b["active_players"] for b in res["by_board"]} == {board: 2}
    # Only this window is materialized, so the rollups equal its set (2), monotonic.
    assert res["estimate_24h"] == 2
    assert res["estimate_7d"] == 2


async def test_no_score_ceiling(monkeypatch):
    # There is NO score cap: a player who rose on a huge lifetime-cumulative board
    # still counts (the only rule is "score went up / appeared non-zero").
    late, early = 2_000_000, 2_000_000 - 3600
    boards = [{"uuid": 100, "name": "Flux Earned", "category": "S",
               "player_board": True, "reset_kind": "default"}]
    maps = {
        late:  {100: {"Whale": 5_000_000.0, "Comp": 500.0}},
        early: {100: {"Whale": 4_000_000.0, "Comp": 400.0}},
    }
    _patch(monkeypatch, boards, maps)

    res = await activity._compute(late, early)
    assert res["estimate"] == 2                       # both Whale and Comp counted


async def test_rollups_union_materialized_windows_and_are_monotonic(monkeypatch):
    # Different player moves in each hour; the 24h/7d rollup is the distinct UNION
    # of those per-window sets - not a sum, not first-vs-last. All players are
    # present throughout, so each window has exactly its one mover.
    h = 3600
    a0, a1, a2, a3 = 2_000_000, 2_000_000 + h, 2_000_000 + 2 * h, 2_000_000 + 3 * h
    board = 100
    boards = [{"uuid": board, "name": "X", "category": "S",
               "player_board": True, "reset_kind": "default"}]
    maps = {
        a0: {board: {"A": 10.0, "B": 10.0, "C": 10.0}},
        a1: {board: {"A": 20.0, "B": 10.0, "C": 10.0}},   # A rose
        a2: {board: {"A": 20.0, "B": 20.0, "C": 10.0}},   # B rose
        a3: {board: {"A": 20.0, "B": 20.0, "C": 20.0}},   # C rose
    }
    _patch(monkeypatch, boards, maps)

    await activity._compute(a1, a0)                  # window a1 active = {A}
    await activity._compute(a2, a1)                  # window a2 active = {B}
    res = await activity._compute(a3, a2)            # window a3 active = {C}

    assert res["estimate"] == 1                      # last hour: only C moved
    # 24h/7d union the three windows: {A, B, C} = 3 (each window contributes its
    # own mover; nobody is double-counted, nobody is summed).
    assert res["estimate_24h"] == 3
    assert res["estimate_7d"] == 3
    assert res["estimate_7d"] >= res["estimate_24h"] > res["estimate"]


async def test_resetting_board_drop_is_not_activity(monkeypatch):
    # A reset drops the score (not an increase -> not counted); the post-reset climb
    # IS counted. No per-board reset bookkeeping needed.
    h = 3600
    a0, a1, a2, a3 = 2_000_000, 2_000_000 + h, 2_000_000 + 2 * h, 2_000_000 + 3 * h
    board = 100
    boards = [{"uuid": board, "name": "Daily", "category": "S",
               "player_board": True, "reset_kind": "daily"}]
    maps = {
        a0: {board: {"Idle": 500.0, "Grind": 100.0}},
        a1: {board: {"Idle": 500.0, "Grind": 200.0}},    # Grind rose; Idle flat
        a2: {board: {"Idle": 0.0, "Grind": 0.0}},         # reset (a drop)
        a3: {board: {"Idle": 0.0, "Grind": 80.0}},        # Grind climbs post-reset
    }
    _patch(monkeypatch, boards, maps)

    await activity._compute(a1, a0)                  # {Grind}
    await activity._compute(a2, a1)                  # reset drop -> {}
    res = await activity._compute(a3, a2)            # {Grind} (0 -> 80)

    # Over a0..a3 only Grind ever increased; Idle only dropped/stayed flat.
    assert res["estimate_24h"] == 1
    assert res["estimate_7d"] == 1


async def test_compute_skips_board_absent_from_early_snapshot(monkeypatch):
    late, early = 2_000_000, 2_000_000 - 3600
    boards = [
        {"uuid": 100, "name": "A", "category": "Stats", "player_board": True, "reset_kind": "default"},
        {"uuid": 200, "name": "B", "category": "Stats", "player_board": True, "reset_kind": "default"},
    ]
    maps = {
        late:  {100: {"Alice": 120.0}, 200: {"Zed": 5.0}},
        early: {100: {"Alice": 100.0}},                 # board 200 has no early snapshot
    }
    _patch(monkeypatch, boards, maps)

    res = await activity._compute(late, early)

    # Board 200 has no early snapshot -> can't tell, skipped. Only Alice (board 100,
    # strict increase) is active - NOT Zed (first appearance + no baseline).
    assert res["estimate"] == 1
    assert {b["uuid"]: b["active_players"] for b in res["by_board"]} == {100: 1}

"""Regression tests for the live activity estimate (`_compute`).

The migration-era bug this guards against: `_compute` loads each anchor as
``{board_uuid: {player: score}}`` but handed the WHOLE-anchor (uuid-keyed) map to
``_active_set``, which expects a single board's ``{player: score}``. Every
``early_scores.get(player_name)`` then missed (keys were board uuids), so EVERY
player counted as a first-appearance and the estimate ballooned to ~the entire
distinct-player population - identical across the 1h / 24h / 7d windows.
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

    monkeypatch.setattr(activity.lb_service, "list_boards_at", fake_list_boards_at)
    monkeypatch.setattr(activity.lb_service, "list_timestamps", fake_list_timestamps)
    monkeypatch.setattr(activity, "_load_anchor_maps", fake_load_anchor_maps)
    monkeypatch.setattr(pg_store, "upsert_estimate", fake_upsert)


async def test_compute_counts_per_board_not_whole_population(monkeypatch):
    late, early = 2_000_000, 2_000_000 - 3600          # 1h apart
    board = 100
    boards = [{
        "uuid": board, "name": "Enemies Defeated", "category": "Stats",
        "player_board": True, "reset_kind": "default",   # lifetime - never resets
    }]
    maps = {
        late:  {board: {"Alice": 120.0, "Bob": 50.0, "Carol": 30.0}},
        early: {board: {"Alice": 100.0, "Bob": 50.0}},
    }
    _patch(monkeypatch, boards, maps)

    res = await activity._compute(late, early)

    # Active = Alice (100 -> 120) + Carol (new). Bob is unchanged (50 == 50).
    # The pre-fix bug returned 3 (everyone "first-appears").
    assert res["estimate"] == 2
    assert {b["uuid"]: b["active_players"] for b in res["by_board"]} == {board: 2}
    # Only two anchors exist, so the wide windows fall back to the same pair -
    # but they must report the same SMALL number, not the whole population.
    assert res["estimate_24h"] == 2
    assert res["estimate_7d"] == 2


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

    # Board 200 is skipped (no early snapshot), so only Alice (board 100) is active -
    # NOT Alice + Zed. A whole-anchor mis-index would wrongly count Zed.
    assert res["estimate"] == 1
    assert {b["uuid"]: b["active_players"] for b in res["by_board"]} == {100: 1}

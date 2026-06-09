"""Unit tests for the synchronous helpers in
``app.trove.leaderboards.detection``.

The velocity check is async + Mongo-backed and is covered by integration
tests; here we exercise the math: Modified Z-score outlier detection,
rank-gap detection, and the direction-inference helper. Each test feeds
synthetic entries (no DB) and asserts the resulting evidence structure.
"""
from __future__ import annotations

import random

from app.trove.leaderboards.detection import (
    _detect_direction,
    _evidence_confidence,
    _parse_excluded,
    _player_confidence,
    _rank_gap_check,
    _reset_boundary_before,
    _score_outlier_check,
)


def _board(uuid: int = 1, name: str = "Test Board"):
    return {
        "uuid": uuid, "name": name, "category": "CONTESTS",
        "contest_type": None, "reset_kind": "default", "player_board": True,
    }


def _entry(name: str, rank: int, score: float):
    return {"player_name": name, "rank": rank, "score": score}


# ─── direction inference ────────────────────────────────────────────


def test_direction_higher_is_better():
    # Top rank holds the highest score → higher-is-better.
    entries = [_entry(f"p{i}", i + 1, 1000 - i * 10) for i in range(20)]
    assert _detect_direction(entries) is True


def test_direction_lower_is_better():
    # Top rank holds the LOWEST score (speedrun-style).
    entries = [_entry(f"p{i}", i + 1, 10 + i * 5) for i in range(20)]
    assert _detect_direction(entries) is False


def test_direction_degenerate_defaults_to_higher():
    # All scores equal → direction is ambiguous; helper defaults to True.
    entries = [_entry(f"p{i}", i + 1, 100) for i in range(10)]
    assert _detect_direction(entries) is True


# ─── score outlier (Modified Z-score / MAD) ─────────────────────────


def test_score_outlier_flags_extreme_high():
    """Classic case: a player with a wildly anomalous score gets
    flagged against the elite cohort's distribution."""
    rng = random.Random(42)
    # 100 normals with realistic top-of-cohort variance.
    entries = [_entry(f"p{i}", i + 1, rng.gauss(50, 8)) for i in range(100)]
    entries.append(_entry("Cheater", 0, 2000))  # absurd score
    entries.sort(key=lambda e: -e["score"])
    for i, e in enumerate(entries):
        e["rank"] = i + 1

    flagged: dict = {}
    _score_outlier_check(flagged, _board(), entries, z_threshold=5.0, higher_is_better=True)

    assert "Cheater" in flagged
    ev = flagged["Cheater"][1]["evidence"][0]
    assert ev["type"] == "score_outlier"
    assert ev["measurements"]["modified_z_score"] > 5.0
    assert ev["measurements"]["higher_is_better"] is True
    assert ev["measurements"]["cohort_size"] >= 50
    assert ev["measurements"]["board_size"] == 101


def test_score_outlier_does_not_flag_legitimate_top_player():
    """REGRESSION GUARD against the old MAD-Z-vs-full-board bug.
    A heavy-tailed leaderboard (typical Trove shape: most players score
    low, top players score 100×+ higher) MUST NOT flag every top-100
    player just because they're far from the median."""
    rng = random.Random(2026)
    entries = []
    # 5000 players with a heavy-tailed power-law-ish distribution -
    # mirrors the FLUX EARNED / LOOT COLLECTED shape from real Trove
    # leaderboards: median ~17M, top1 ~4.3B, top-100 within 1 order of
    # magnitude of each other.
    for i in range(5000):
        # Pareto draw, scaled so median ≈ 17M, top ~ a few billion.
        x = rng.paretovariate(1.5)
        entries.append(_entry(f"p{i}", 0, x * 17_000_000))
    entries.sort(key=lambda e: -e["score"])
    for i, e in enumerate(entries):
        e["rank"] = i + 1

    flagged: dict = {}
    _score_outlier_check(flagged, _board(), entries, z_threshold=5.0, higher_is_better=True)

    # Old method flagged ~750 on this kind of data; the new method
    # (log-MAD-Z on the elite cohort) drops it to single-digit %% of
    # the cohort. Pareto α=1.5 is more extreme than typical Trove
    # boards, so allow a modest non-zero count - what we're guarding
    # against is the catastrophic-false-positive regression.
    assert len(flagged) <= 30, (
        f"Heavy-tailed distribution shouldn't produce many flags; got "
        f"{len(flagged)} - the algorithm likely reverted to full-board "
        f"linear MAD-Z."
    )


def test_score_outlier_skips_inverted_direction():
    """A player who is bizarrely BELOW median on a higher-is-better
    board (a noob) should NOT be flagged - we only care about good
    outliers. Use a small board so the noob falls inside the cohort
    (otherwise the cohort cap excludes them on size alone)."""
    rng = random.Random(7)
    entries = [_entry(f"p{i}", 0, rng.gauss(100, 5)) for i in range(50)]
    entries.append(_entry("Noob", 0, 0))  # far below median
    entries.sort(key=lambda e: -e["score"])
    for i, e in enumerate(entries):
        e["rank"] = i + 1

    flagged: dict = {}
    _score_outlier_check(flagged, _board(), entries, z_threshold=5.0, higher_is_better=True)

    assert "Noob" not in flagged


def test_score_outlier_skips_flat_distribution():
    """All scores equal → top-5 of cohort all tied → capped-board
    skip path. Helper must not divide by zero or flag everyone."""
    entries = [_entry(f"p{i}", i + 1, 100) for i in range(60)]

    flagged: dict = {}
    _score_outlier_check(flagged, _board(), entries, z_threshold=5.0, higher_is_better=True)

    assert flagged == {}


def test_score_outlier_skips_capped_board():
    """Class boards cap top scores at 59731 (many players tied at the
    cap). Top-5 tied = no signal possible. Skip."""
    entries = []
    for i in range(100):
        # Top 30 all at the cap, rest below.
        score = 59731 if i < 30 else 50000 - i * 100
        entries.append(_entry(f"p{i}", 0, score))
    entries.sort(key=lambda e: -e["score"])
    for i, e in enumerate(entries):
        e["rank"] = i + 1

    flagged: dict = {}
    _score_outlier_check(flagged, _board(), entries, z_threshold=5.0, higher_is_better=True)

    assert flagged == {}


def test_score_outlier_multiple_cheaters_dont_mask_each_other():
    """MAD-based detection should resist the 'cheater inflates baseline'
    failure mode that mean+stddev suffers from. Plant THREE cheaters
    among normals with realistic top-of-cohort variance and verify
    all three are still flagged."""
    rng = random.Random(99)
    entries = [_entry(f"p{i}", 0, rng.gauss(50, 8)) for i in range(100)]
    entries.extend([
        _entry("Cheat1", 0, 2000),
        _entry("Cheat2", 0, 1800),
        _entry("Cheat3", 0, 2200),
    ])
    entries.sort(key=lambda e: -e["score"])
    for i, e in enumerate(entries):
        e["rank"] = i + 1

    flagged: dict = {}
    _score_outlier_check(flagged, _board(), entries, z_threshold=5.0, higher_is_better=True)

    for c in ("Cheat1", "Cheat2", "Cheat3"):
        assert c in flagged, f"{c} should still be flagged despite multiple cheaters present"


def test_score_outlier_lower_is_better_flags_speedrunner():
    """On a speedrun-style board (low = good), a player with an
    abnormally LOW score is the suspicious one."""
    rng = random.Random(7)
    entries = [_entry(f"p{i}", i + 1, rng.gauss(75, 5)) for i in range(60)]
    entries.insert(0, _entry("Speedhacker", 1, 1.0))
    entries.sort(key=lambda e: e["score"])
    for i, e in enumerate(entries):
        e["rank"] = i + 1

    flagged: dict = {}
    _score_outlier_check(flagged, _board(), entries, z_threshold=5.0, higher_is_better=False)

    assert "Speedhacker" in flagged
    ev = flagged["Speedhacker"][1]["evidence"][0]
    assert ev["measurements"]["modified_z_score"] < -5.0
    assert ev["measurements"]["higher_is_better"] is False


# ─── rank-gap check ─────────────────────────────────────────────────


def test_rank_gap_flags_lone_wolf_at_top():
    """Rank 1 has 10× rank 2's score; everyone else is tightly grouped.
    Rank-gap should flag rank 1."""
    entries = [_entry("Cheater", 1, 100000)]
    # Tightly grouped tail: each player ~5% below the previous.
    score = 10000
    for i in range(2, 50):
        entries.append(_entry(f"p{i}", i, score))
        score = int(score * 0.95)

    flagged: dict = {}
    _rank_gap_check(flagged, _board(), entries, higher_is_better=True)

    assert "Cheater" in flagged
    ev = flagged["Cheater"][1]["evidence"][0]
    assert ev["type"] == "rank_gap"
    assert ev["measurements"]["gap_multiplier"] >= 10
    assert ev["measurements"]["player_rank"] == 1


def test_rank_gap_silent_on_smooth_distribution():
    """Geometric decay with no anomalies → no rank-gap flags."""
    entries = []
    score = 10000
    for i in range(1, 50):
        entries.append(_entry(f"p{i}", i, score))
        score = int(score * 0.95)

    flagged: dict = {}
    _rank_gap_check(flagged, _board(), entries, higher_is_better=True)

    assert flagged == {}


def test_rank_gap_skips_undersized_boards():
    """Need at least 4 entries to have a meaningful baseline."""
    entries = [
        _entry("a", 1, 1000),
        _entry("b", 2, 10),
        _entry("c", 3, 9),
    ]

    flagged: dict = {}
    _rank_gap_check(flagged, _board(), entries, higher_is_better=True)

    assert flagged == {}


# ─── combined evidence ─────────────────────────────────────────────


# ─── confidence ────────────────────────────────────────────────────


def test_evidence_confidence_borderline_is_half():
    """At the threshold (Z = 3.5 exactly), confidence should be 0.5
    - the minimum 'just inside the flag zone' value."""
    ev = {
        "type": "score_outlier",
        "measurements": {"modified_z_score": 3.5, "threshold": 3.5},
    }
    assert _evidence_confidence(ev) == 0.5


def test_evidence_confidence_strong_score_outlier_hits_ceiling():
    """Z = 40 should hit score_outlier's ceiling (0.60), not the
    sigmoid's natural 0.99. score_outlier is the noisiest signal - by
    itself, it never exceeds the per-check ceiling no matter how
    extreme the z-score."""
    ev = {
        "type": "score_outlier",
        "measurements": {"modified_z_score": 40, "threshold": 3.5},
    }
    c = _evidence_confidence(ev)
    assert c == 0.60


def test_evidence_confidence_strong_velocity_hits_high_ceiling():
    """Velocity is the cleanest signal - its ceiling is 0.99."""
    ev = {
        "type": "velocity_outlier",
        "measurements": {"rate_multiplier": 100, "threshold_multiplier": 10.0},
    }
    c = _evidence_confidence(ev)
    assert c >= 0.98


def test_evidence_confidence_rank_gap_ceiling_is_intermediate():
    """rank_gap's ceiling is 0.85 - between score_outlier and velocity."""
    ev = {
        "type": "rank_gap",
        "measurements": {"gap_multiplier": 100, "threshold_multiplier": 10.0},
    }
    c = _evidence_confidence(ev)
    assert c == 0.85


def test_evidence_confidence_rank_gap_reads_threshold_multiplier():
    """Rank-gap evidence must look at gap_multiplier vs the echoed
    threshold_multiplier. Sigmoid value capped by rank_gap ceiling."""
    ev = {
        "type": "rank_gap",
        "measurements": {"gap_multiplier": 30.0, "threshold_multiplier": 10.0},
    }
    c = _evidence_confidence(ev)
    # 30/10 = 3× threshold → sigmoid ~0.93, but capped at rank_gap
    # ceiling of 0.85.
    assert c == 0.85


def test_evidence_confidence_unknown_type_is_neutral():
    """Defensive: unknown evidence types should not crash, should
    return a conservative 0.5."""
    ev = {"type": "something_else", "measurements": {}}
    assert _evidence_confidence(ev) == 0.5


def test_player_confidence_single_weak_board_is_at_most_max():
    """One board with a single 0.5 evidence → player confidence = 0.5.
    Adding a second 0.5 evidence ON THE SAME BOARD must NOT compound
    (they're correlated) - still 0.5."""
    boards_one = [{"evidence": [
        {"type": "score_outlier", "measurements": {"modified_z_score": 3.5, "threshold": 3.5}},
    ]}]
    assert _player_confidence(boards_one) == 0.5

    boards_two_same = [{"evidence": [
        {"type": "score_outlier", "measurements": {"modified_z_score": 3.5, "threshold": 3.5}},
        {"type": "rank_gap", "measurements": {"gap_multiplier": 10, "threshold_multiplier": 10}},
    ]}]
    # Both at the borderline; max within board = 0.5 - no inflation.
    assert _player_confidence(boards_two_same) == 0.5


def test_player_confidence_multiple_boards_same_type_caps_at_ceiling():
    """A player flagged on 5 SEPARATE boards but only by score_outlier
    must still be capped at the score_outlier ceiling (0.60). Noisy-OR
    would push to ~0.999 without the diversity cap, falsely suggesting
    high confidence on the noisiest signal."""
    one_board = {"evidence": [
        {"type": "score_outlier", "measurements": {"modified_z_score": 20, "threshold": 3.5}},
    ]}
    boards = [dict(one_board) for _ in range(5)]
    c = _player_confidence(boards)
    assert c <= 0.60


def test_player_confidence_multi_type_unlocks_full_aggregation():
    """When MORE THAN ONE check type fires, the diversity cap lifts
    and noisy-OR can compound across boards."""
    boards = [
        {"evidence": [
            {"type": "score_outlier", "measurements": {"modified_z_score": 20, "threshold": 3.5}},
            {"type": "velocity_outlier", "measurements": {"rate_multiplier": 50, "threshold_multiplier": 10}},
        ]},
        {"evidence": [
            {"type": "velocity_outlier", "measurements": {"rate_multiplier": 50, "threshold_multiplier": 10}},
        ]},
    ]
    c = _player_confidence(boards)
    # Two strong signals confirmed across boards → high confidence
    assert c >= 0.99


def test_player_confidence_velocity_only_can_be_high():
    """Velocity alone CAN reach high confidence (its ceiling is 0.99)
    even though the diversity cap applies - because the cap IS the
    ceiling for velocity."""
    boards = [{"evidence": [
        {"type": "velocity_outlier", "measurements": {"rate_multiplier": 100, "threshold_multiplier": 10}},
    ]}]
    c = _player_confidence(boards)
    assert c >= 0.95


def test_player_confidence_within_board_takes_max():
    """Three pieces of evidence on the SAME board at different
    strengths - confidence equals the max, NOT the noisy-OR product
    (which would inflate the false-positive rate)."""
    one_strong_two_weak = [{"evidence": [
        {"type": "score_outlier", "measurements": {"modified_z_score": 20, "threshold": 3.5}},
        {"type": "rank_gap", "measurements": {"gap_multiplier": 11, "threshold_multiplier": 10}},
        {"type": "velocity_outlier", "measurements": {"rate_multiplier": 11, "threshold_multiplier": 10}},
    ]}]
    c_combo = _player_confidence(one_strong_two_weak)
    only_strong = [{"evidence": [
        {"type": "score_outlier", "measurements": {"modified_z_score": 20, "threshold": 3.5}},
    ]}]
    c_alone = _player_confidence(only_strong)
    # Confidences should be EQUAL - within-board correlation handled.
    assert c_combo == c_alone


def test_per_board_confidence_via_format():
    """End-to-end check that the response payload carries a per-board
    `confidence` field equal to the max evidence confidence on that
    board."""
    from app.trove.leaderboards.detection import _format
    flagged = {
        "Cheater": {
            1012: {
                "uuid": 1012, "name": "GLYPH KICKER",
                "category": "CONTESTS", "contest_type": "daily",
                "rank": 1, "score": 99999,
                "evidence": [
                    {"type": "score_outlier",
                     "measurements": {"modified_z_score": 40, "threshold": 3.5}},
                    {"type": "rank_gap",
                     "measurements": {"gap_multiplier": 100, "threshold_multiplier": 10}},
                ],
            },
        },
    }
    result = _format(flagged, 1780830000, 5.0, 10.0, 20, 25)
    board = result["players"][0]["leaderboards"][0]
    # score_outlier capped at 0.60, rank_gap capped at 0.85 → board = 0.85
    assert board["confidence"] == 0.85
    # Each evidence must also carry its own confidence value
    assert all("confidence" in ev for ev in board["evidence"])


def test_player_confidence_empty_returns_zero():
    """Defensive: empty board list / empty evidence → 0.0, not 1.0."""
    assert _player_confidence([]) == 0.0
    assert _player_confidence([{"evidence": []}]) == 0.0


# ─── excluded-board parsing ────────────────────────────────────────


def test_parse_excluded_normal_input():
    assert _parse_excluded("1100, 21012, 5001") == {1100, 21012, 5001}


def test_parse_excluded_whitespace_only_empty_string():
    assert _parse_excluded("") == set()
    assert _parse_excluded("   ") == set()
    assert _parse_excluded(",,,") == set()


def test_parse_excluded_skips_non_numeric_tokens():
    """Defensive: a typo or stray text in the master-panel input
    must not blow up the whole analysis. Bad tokens are dropped, good
    ones still parsed."""
    assert _parse_excluded("1012, foo, 20, , bar") == {1012, 20}


def test_parse_excluded_dedupes():
    assert _parse_excluded("1012, 1012, 1012") == {1012}


# ─── reset-cycle boundary detection ────────────────────────────────


def test_reset_boundary_default_kind_returns_zero():
    """Boards that don't reset → return 0 so any prior anchor passes
    the velocity-check cycle membership filter."""
    assert _reset_boundary_before(1780887263, "default") == 0
    assert _reset_boundary_before(1780887263, "") == 0


def test_reset_boundary_daily_returns_most_recent_11_utc():
    """Trove daily resets are 11:00 UTC. For an anchor on 2026-06-08
    at 03:54 UTC (before today's reset), the most-recent reset is
    yesterday's 11:00 UTC."""
    # 2026-06-08 03:54:23 UTC
    anchor = 1780887263
    boundary = _reset_boundary_before(anchor, "daily")
    # Yesterday (2026-06-07) at 11:00 UTC
    from datetime import UTC, datetime
    b = datetime.fromtimestamp(boundary, UTC)
    assert b.year == 2026 and b.month == 6 and b.day == 7
    assert b.hour == 11 and b.minute == 0


def test_reset_boundary_daily_after_11_uses_same_day():
    """For an anchor AFTER today's 11:00 UTC, the boundary is today's
    11:00 UTC (not yesterday's)."""
    from datetime import UTC, datetime
    # 2026-06-08 at 15:00 UTC - after today's reset.
    anchor = int(datetime(2026, 6, 8, 15, 0, 0, tzinfo=UTC).timestamp())
    boundary = _reset_boundary_before(anchor, "daily")
    b = datetime.fromtimestamp(boundary, UTC)
    assert b.year == 2026 and b.month == 6 and b.day == 8
    assert b.hour == 11


def test_reset_boundary_weekly_returns_most_recent_monday_11_utc():
    """Weekly boards reset every Monday at 11:00 UTC. For an anchor on
    2026-06-08 (Monday) at 03:54 UTC - BEFORE today's 11:00 reset -
    the most recent Monday-11:00 was the *previous* Monday (2026-06-01)
    at 11:00 UTC."""
    anchor = 1780887263  # 2026-06-08 03:54 UTC (Monday, pre-11:00)
    boundary = _reset_boundary_before(anchor, "weekly")
    from datetime import UTC, datetime
    b = datetime.fromtimestamp(boundary, UTC)
    # 2026-06-01 was a Monday
    assert b.year == 2026 and b.month == 6 and b.day == 1
    assert b.weekday() == 0  # Monday
    assert b.hour == 11


def test_reset_boundary_weekly_after_monday_uses_same_monday():
    """An anchor right after Monday 11:00 UTC should use that same
    Monday as the boundary, not the previous week's."""
    from datetime import UTC, datetime
    # 2026-06-08 (Monday) at 15:00 UTC.
    anchor = int(datetime(2026, 6, 8, 15, 0, 0, tzinfo=UTC).timestamp())
    boundary = _reset_boundary_before(anchor, "weekly")
    b = datetime.fromtimestamp(boundary, UTC)
    assert b.year == 2026 and b.month == 6 and b.day == 8
    assert b.weekday() == 0
    assert b.hour == 11


def test_evidence_accumulates_across_checks():
    """A player flagged by BOTH score-outlier and rank-gap should have
    both pieces of evidence stacked in one board entry - not two."""
    entries = [_entry("Cheater", 1, 100000)]
    score = 50
    for i in range(2, 60):
        entries.append(_entry(f"p{i}", i, score))
        score = max(1, score - 1)

    flagged: dict = {}
    _score_outlier_check(flagged, _board(), entries, z_threshold=3.5, higher_is_better=True)
    _rank_gap_check(flagged, _board(), entries, higher_is_better=True)

    assert "Cheater" in flagged
    board_entry = flagged["Cheater"][1]
    types = {ev["type"] for ev in board_entry["evidence"]}
    assert "score_outlier" in types
    assert "rank_gap" in types
    # Same board entry, two pieces of evidence - not two board entries.
    assert len(flagged["Cheater"]) == 1

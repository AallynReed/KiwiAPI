"""Estimated active-player count via leaderboard score deltas.

A player whose score went up on at least one leaderboard between two
consecutive captures was active in that window. Restricting to
``reset_kind == "default"`` boards (lifetime accumulating stats —
ENEMIES DEFEATED, FLUX EARNED, LOOT COLLECTED, …) eliminates the
"everyone reset to 0" noise that daily / weekly boards would otherwise
inject as massive negative deltas.

The count is a **lower bound on active players**, not the absolute
total: it only sees players who scored on at least one tracked board
*and* whose ranking was within the top of that board. A casual player
who logged in for 20 minutes but never broke the top 5000 on any
lifetime stat won't be counted. That's fine for a "how busy is Trove
right now" UI — the trend is what matters, not the absolute number.

Cached in-process for ``cheaters_cache_ttl_seconds`` (we piggy-back
on the same tunable) keyed by the (latest, previous) anchor pair, so
a new hourly capture invalidates automatically.
"""
from __future__ import annotations

import asyncio
import logging
import time

from app.trove.leaderboards import service as lb_service

logger = logging.getLogger(__name__)


# Result cache: {(anchor_late, anchor_early): (stored_at, payload)}.
_CACHE: dict[tuple[int, int], tuple[float, dict]] = {}

# Last successful computation, kept across anchor changes so a cache
# miss for the newest window still serves the previous valid result
# immediately. Mirrors the same pattern in ``detection.py``.
_LAST_GOOD: dict | None = None

# Top-N entries per board to consider. Bigger = more players covered
# but more rows fetched per board. 5000 matches the bot's typical dump
# size, so we get the full per-board population in practice.
_BOARD_FETCH_LIMIT = 10000


async def estimate_active_players() -> dict:
    """Compute the activity estimate for the most-recent window. Cached
    by ``(latest_anchor, prev_anchor)``; falls back to ``_LAST_GOOD``
    when the newest pair hasn't been computed yet, so the user never
    waits on a fresh ingest. Returns a structured dict regardless of
    data availability — a single-anchor or empty DB returns a 'no
    estimate' shape with ``estimate=None``.
    """
    from app.admin import runtime_config

    cache_ttl = float(await runtime_config.get_setting("cheaters_cache_ttl_seconds"))

    # Need at least two anchors to compute a delta.
    stamps = await lb_service.list_timestamps(limit=2, include_archive=False)
    if len(stamps) < 2:
        return _empty()
    anchor_late, anchor_early = stamps[0], stamps[1]
    if anchor_late <= anchor_early:
        return _empty()

    cache_key = (anchor_late, anchor_early)
    now = time.time()
    hit = _CACHE.get(cache_key)
    if hit is not None and now - hit[0] < cache_ttl:
        global _LAST_GOOD
        _LAST_GOOD = hit[1]
        return hit[1]

    # Same "stale-but-known-good" trick as the cheaters cache: when the
    # current pair isn't computed yet, hand back the previous valid
    # payload and let the warmer fill in the new one. The warmer is
    # already running on the same TTL and is also tripped by ingest
    # via ``detection.trigger_warmer()``, so a long miss is rare.
    if _LAST_GOOD is not None:
        # No direct trigger here — the cheaters compute path in
        # detection.py owns the warmer wake. If a caller hits THIS
        # function before that one (e.g. activity-only client), it
        # still benefits from the existing TTL re-runs.
        return _LAST_GOOD

    payload = await _compute(anchor_late, anchor_early)
    _CACHE[cache_key] = (now, payload)
    _LAST_GOOD = payload
    _prune(now, cache_ttl)
    return payload


def invalidate_cache() -> None:
    _CACHE.clear()


# ─── Implementation ────────────────────────────────────────────────────


async def _compute(anchor_late: int, anchor_early: int) -> dict:
    """Iterate every ``reset_kind=default`` board (accumulating stat
    boards), fetch entries at both anchors, count distinct players with
    a positive delta."""
    boards = await lb_service.list_boards_at(anchor_late)
    duration_h = (anchor_late - anchor_early) / 3600.0

    active_union: set[str] = set()
    per_board: list[dict] = []

    for board in boards:
        if board.get("reset_kind") != "default":
            continue
        # Don't bother with player-board=false (server-tally rows like
        # CLUB POWER RANK) — those wouldn't tell us about individual
        # players being online anyway.
        if not board.get("player_board", True):
            continue

        late_entries, _ = await lb_service.list_entries(
            board["uuid"], anchor_late, limit=_BOARD_FETCH_LIMIT, offset=0,
        )
        early_entries, _ = await lb_service.list_entries(
            board["uuid"], anchor_early, limit=_BOARD_FETCH_LIMIT, offset=0,
        )
        if not late_entries or not early_entries:
            continue

        early_scores = {e["player_name"]: e["score"] for e in early_entries}
        active_on_board: set[str] = set()
        for e in late_entries:
            prev = early_scores.get(e["player_name"])
            if prev is None:
                # New entrant on this board between captures — they
                # had to score to get listed, so they were active.
                active_on_board.add(e["player_name"])
                continue
            if e["score"] > prev:
                active_on_board.add(e["player_name"])

        active_union.update(active_on_board)
        per_board.append({
            "uuid": board["uuid"],
            "name": board["name"],
            "category": board["category"],
            "active_players": len(active_on_board),
        })

    # Sort by per-board activity desc — the highest-engagement boards
    # rise to the top of the breakdown.
    per_board.sort(key=lambda b: -b["active_players"])

    return {
        "window_start": anchor_early,
        "window_end": anchor_late,
        "duration_hours": round(duration_h, 2),
        "estimate": len(active_union),
        "by_board": per_board,
        "boards_analyzed": len(per_board),
        "methodology": (
            "Distinct top-5000 leaderboard players whose score increased "
            "on at least one lifetime-accumulating board between the two "
            "most recent captures. Lower bound — players outside every "
            "board's top-N or inactive on tracked stats are not counted."
        ),
        "computed_at": int(time.time()),
    }


def _empty() -> dict:
    return {
        "window_start": None,
        "window_end": None,
        "duration_hours": None,
        "estimate": None,
        "by_board": [],
        "boards_analyzed": 0,
        "methodology": (
            "At least two captures required to compute a score-delta. "
            "Bot needs to send unique timestamps (not just the daily "
            "anchor) for this estimate to be available."
        ),
        "computed_at": int(time.time()),
    }


def _prune(now: float, ttl: float) -> None:
    expired = [k for k, (t, _) in _CACHE.items() if now - t > ttl * 2]
    for k in expired:
        del _CACHE[k]

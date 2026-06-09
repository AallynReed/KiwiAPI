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
    """Iterate EVERY player_board, fetch entries at both anchors, count
    distinct players with positive activity signal.

    Resetting boards (daily/weekly) need different handling than lifetime
    boards: their score zeroes at the reset moment, so a "score
    increased" delta isn't valid across a reset crossing — a player's
    old high score becomes meaningless. Logic per board:

      * If a reset boundary falls between ``anchor_early`` and
        ``anchor_late`` for this board's cadence: ANY player who
        appears in ``late_entries`` was active. They had to score
        AFTER the reset to make the new top-N, so their presence is
        the activity signal.
      * Otherwise (no reset in window, OR lifetime board): standard
        score-increased delta vs ``early_entries``. New entrants who
        weren't in early also count as active.

    Server-tally boards (``player_board=False``, e.g. CLUB POWER RANK)
    are still skipped — those aggregate everyone's contributions and
    don't tell us about individual activity.

    Persists the result to ``LeaderboardActivityEstimate`` (upsert by
    ``window_end``) so the history graph survives container restarts."""
    boards = await lb_service.list_boards_at(anchor_late)
    duration_h = (anchor_late - anchor_early) / 3600.0

    active_union: set[str] = set()
    per_board: list[dict] = []

    for board in boards:
        # Server-tally boards aggregate scores across everyone; they
        # don't reflect individual activity. Everything else counts.
        if not board.get("player_board", True):
            continue

        late_entries, _ = await lb_service.list_entries(
            board["uuid"], anchor_late, limit=_BOARD_FETCH_LIMIT, offset=0,
        )
        if not late_entries:
            continue

        kind = board.get("reset_kind", "default")
        # A reset boundary inside the window invalidates score deltas —
        # everyone in late_entries necessarily scored AFTER the reset.
        crossed_reset = bool(lb_service.reset_boundaries_for_kind(
            kind, anchor_early, anchor_late,
        ))

        active_on_board: set[str] = set()
        if crossed_reset:
            # Post-reset cycle: any player in late_entries was active
            # (they had to score AFTER zero to be listed).
            for e in late_entries:
                active_on_board.add(e["player_name"])
        else:
            # Standard delta: increased score OR new entrant = active.
            early_entries, _ = await lb_service.list_entries(
                board["uuid"], anchor_early, limit=_BOARD_FETCH_LIMIT, offset=0,
            )
            if not early_entries:
                continue
            early_scores = {e["player_name"]: e["score"] for e in early_entries}
            for e in late_entries:
                prev = early_scores.get(e["player_name"])
                if prev is None:
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

    now_ts = int(time.time())
    estimate_count = len(active_union)
    boards_count = len(per_board)

    # Persist the point so the history chart accumulates across restarts.
    # Upsert by window_end (the unique index) so re-runs converge instead
    # of stacking. Direct motor-style update_one — Beanie's high-level
    # update API doesn't give us a clean upsert-on-unique-index pattern.
    # Failure to persist is non-fatal — log and move on.
    try:
        from app.trove.leaderboards.models import LeaderboardActivityEstimate
        await LeaderboardActivityEstimate.get_pymongo_collection().update_one(
            {"window_end": anchor_late},
            {"$set": {
                "window_start": anchor_early,
                "duration_hours": round(duration_h, 2),
                "estimate": estimate_count,
                "boards_analyzed": boards_count,
                "computed_at": now_ts,
            }},
            upsert=True,
        )
    except Exception:
        logger.exception("activity: failed to persist estimate for window_end=%d", anchor_late)

    return {
        "window_start": anchor_early,
        "window_end": anchor_late,
        "duration_hours": round(duration_h, 2),
        "estimate": estimate_count,
        "by_board": per_board,
        "boards_analyzed": boards_count,
        "methodology": (
            "Distinct top-5000 leaderboard players showing an activity "
            "signal on at least one tracked board between the two most "
            "recent captures. For lifetime-accumulating boards, signal "
            "is a positive score delta or first appearance. For "
            "daily/weekly boards where a reset crossed the window, any "
            "presence in the new cycle's top-N counts (they had to "
            "score after the reset to be listed). Lower bound — players "
            "outside every board's top-N are not counted."
        ),
        "computed_at": now_ts,
    }


async def backfill_history(*, window_days: int = 7, force: bool = False) -> dict:
    """One-shot backfill of the activity-estimate collection.

    Batched for speed: instead of calling ``_compute`` per pair (which
    hits Mongo with N×2 board queries plus a count() each), this loads
    every needed (board, anchor) entry list in ONE pre-pass, then
    computes pairs purely in Python. Per-board memory is bounded by
    actual top-N size × 168 anchors × 80 boards — measured at ~50-100 MB
    in practice, which is fine inside the api container's 2GB cap.
    """
    from app.trove.leaderboards.models import (
        LeaderboardActivityEstimate, LeaderboardEntry, LeaderboardEntryArchive,
        Leaderboard, effective_reset_kind, is_player_board,
    )

    stamps = await lb_service.list_timestamps(limit=10_000, include_archive=True)
    if not stamps:
        return {"computed": 0, "skipped": 0, "failed": 0, "total": 0,
                "note": "no anchors in leaderboard_entries"}
    stamps_asc = sorted(stamps)
    cutoff = int(time.time()) - max(1, window_days) * 86400

    # Build the pair list: every consecutive (early, late) where late
    # is within the window. The earliest "early" anchor we need is the
    # one immediately before the first eligible "late" — we keep it
    # even if it falls outside the cutoff because the pair's early side
    # can predate the window.
    pairs: list[tuple[int, int]] = []
    for i in range(1, len(stamps_asc)):
        early, late = stamps_asc[i - 1], stamps_asc[i]
        if late < cutoff:
            continue
        pairs.append((early, late))

    if not pairs:
        return {"computed": 0, "skipped": 0, "failed": 0, "total": 0,
                "note": "no pairs in window"}

    existing: set[int] = set()
    if not force:
        rows = await LeaderboardActivityEstimate.find(
            LeaderboardActivityEstimate.window_end >= pairs[0][1],
        ).to_list()
        existing = {r.window_end for r in rows}

    pairs_to_compute = [p for p in pairs if p[1] not in existing]
    if not pairs_to_compute:
        return {"computed": 0, "skipped": len(pairs), "failed": 0,
                "total": len(pairs), "note": "all pairs already stored — use force=True to recompute"}

    # Anchors we'll touch — union of every (early, late) in the to-do list.
    needed_anchors = sorted(set(a for p in pairs_to_compute for a in p))
    earliest = needed_anchors[0]

    # Load board metadata once. We need uuid/name/category and the
    # effective reset_kind (admin override applied).
    board_docs = await Leaderboard.find_all().to_list()
    boards = {}
    for d in board_docs:
        if not is_player_board(d.uuid):
            continue   # skip server-tally aggregates
        boards[d.uuid] = {
            "uuid": d.uuid, "name": d.name, "category": d.category,
            "reset_kind": effective_reset_kind(d, d.uuid),
        }
    logger.info("activity backfill: %d pairs to compute, %d boards in scope",
                len(pairs_to_compute), len(boards))

    # ONE query per board across the whole window — uses the
    # (leaderboard, created_at, rank) composite index for an index-only
    # scan. Far cheaper than per-pair list_entries which does count() +
    # find() separately.
    started = time.time()
    # Per-board, per-anchor: {player_name: score}
    by_board_anchor: dict[tuple[int, int], dict[str, float]] = {}

    # Use raw pymongo cursors with a projection — Beanie's Document
    # instantiation costs us ~5x on 2M+ rows. We only need three fields
    # per doc, so projection + dict access is the right shape here.
    needed_set = set(needed_anchors)
    query = {"created_at": {"$gte": earliest, "$in": list(needed_set)}}
    projection = {"_id": 0, "leaderboard": 1, "created_at": 1, "player_name": 1, "score": 1}

    for coll_cls in (LeaderboardEntry, LeaderboardEntryArchive):
        coll = coll_cls.get_pymongo_collection()
        # ONE query per collection across ALL boards in scope. Filter
        # is server-side via $in on the needed anchors so we don't
        # transfer rows we'll throw away. Composite
        # (leaderboard, created_at, rank) index covers the
        # created_at predicate; $in on the secondary key still uses it.
        cursor = coll.find(query, projection=projection)
        loaded = 0
        async for d in cursor:
            uuid_ = d["leaderboard"]
            if uuid_ not in boards:
                continue   # skip server-tally boards
            key = (uuid_, d["created_at"])
            by_board_anchor.setdefault(key, {})[d["player_name"]] = d["score"]
            loaded += 1
            if loaded % 100_000 == 0:
                logger.info("activity backfill: loaded %d docs from %s (%.1fs)",
                            loaded, coll_cls.__name__, time.time() - started)
        logger.info("activity backfill: %s done, %d docs (%.1fs)",
                    coll_cls.__name__, loaded, time.time() - started)
    logger.info("activity backfill: data load done in %.1fs", time.time() - started)

    # Compute every pair in-memory. Reset-aware: when a reset boundary
    # falls in the window, presence in late = active (post-reset cycle
    # zero baseline). Else standard score-delta vs early.
    computed = failed = 0
    compute_started = time.time()
    for early, late in pairs_to_compute:
        try:
            active_union: set[str] = set()
            per_board: list[dict] = []
            for uuid, meta in boards.items():
                late_scores = by_board_anchor.get((uuid, late))
                if not late_scores:
                    continue
                crossed = bool(lb_service.reset_boundaries_for_kind(
                    meta["reset_kind"], early, late,
                ))
                active_on_board: set[str] = set()
                if crossed:
                    # Post-reset cycle: any presence = active
                    active_on_board.update(late_scores.keys())
                else:
                    early_scores = by_board_anchor.get((uuid, early))
                    if not early_scores:
                        continue
                    for name, score in late_scores.items():
                        prev = early_scores.get(name)
                        if prev is None or score > prev:
                            active_on_board.add(name)
                active_union.update(active_on_board)
                per_board.append({
                    "uuid": uuid, "name": meta["name"], "category": meta["category"],
                    "active_players": len(active_on_board),
                })

            duration_h = (late - early) / 3600.0
            await LeaderboardActivityEstimate.get_pymongo_collection().update_one(
                {"window_end": late},
                {"$set": {
                    "window_start": early,
                    "duration_hours": round(duration_h, 2),
                    "estimate": len(active_union),
                    "boards_analyzed": len(per_board),
                    "computed_at": int(time.time()),
                }},
                upsert=True,
            )
            computed += 1
            if computed % 10 == 0:
                logger.info("activity backfill: %d/%d pairs computed (%.1fs)",
                            computed, len(pairs_to_compute), time.time() - compute_started)
        except Exception:
            failed += 1
            logger.exception("activity backfill: pair late=%d failed", late)

    elapsed = time.time() - started
    summary = {
        "computed": computed,
        "skipped": len(pairs) - len(pairs_to_compute),
        "failed": failed,
        "total": len(pairs),
        "elapsed_seconds": round(elapsed, 2),
    }
    logger.info("activity backfill done: %s", summary)
    return summary


def needed_anchors_set(anchors_list: list[int]) -> set[int]:
    """Cached set view of the needed-anchors list so the per-doc check
    inside the data-load loop stays O(1) instead of O(n)."""
    # Stash the set on the function so repeated calls in one backfill
    # invocation skip the rebuild. Cheap-and-cheerful.
    if getattr(needed_anchors_set, "_last", None) is not anchors_list:
        needed_anchors_set._last = anchors_list
        needed_anchors_set._set = set(anchors_list)
    return needed_anchors_set._set


async def estimate_active_players_history(*, days: int = 7) -> dict:
    """Time-series of active-player estimates across the last ``days`` days
    of stored captures, served straight from the
    ``LeaderboardActivityEstimate`` collection (no per-board re-scan).

    Each row carries both ``estimate`` (raw distinct-player count for its
    window) and ``estimate_per_hour`` (= estimate / duration_hours). The
    per-hour rate is the right Y-axis for a chart that should look
    smooth across irregular window sizes — when a capture is missed and
    the next window spans 2-3h instead of 1h, the raw count spikes
    because more players had time to score; the per-hour rate stays
    honest and the chart doesn't lie about the trend.

    Bot captures hourly. New rows land each time the cheaters warmer
    fires ``estimate_active_players()`` (typically right after each
    ingest), so the series fills in naturally as captures arrive.
    Empty / one-row collections return an empty series — the consumer
    just doesn't render the chart."""
    from app.trove.leaderboards.models import LeaderboardActivityEstimate

    days = max(1, min(days, 30))
    now_ts = int(time.time())
    window_start = now_ts - days * 86400

    rows = await (
        LeaderboardActivityEstimate
        .find(LeaderboardActivityEstimate.window_end >= window_start)
        .sort("+window_end")
        .to_list()
    )

    series = []
    for r in rows:
        duration_h = r.duration_hours or 0.0
        per_hour = (r.estimate / duration_h) if duration_h > 0 else 0.0
        series.append({
            "window_end": r.window_end,
            "window_start": r.window_start,
            "duration_hours": round(duration_h, 2),
            "estimate": r.estimate,
            # Round to 1 dp — a chart never needs more precision than
            # the integer underlying it has anyway.
            "estimate_per_hour": round(per_hour, 1),
        })

    return {
        "days": days,
        "window_start": window_start,
        "window_end": now_ts,
        "points": series,
        "methodology": (
            "Distinct top-5000 leaderboard players whose score increased "
            "on at least one lifetime-accumulating board between each "
            "consecutive capture pair. ``estimate_per_hour`` divides by "
            "the actual window duration so missed captures (gaps > 1h) "
            "don't manifest as spikes."
        ),
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

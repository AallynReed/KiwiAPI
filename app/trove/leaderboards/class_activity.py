"""Per-CLASS active-player estimate, mirroring ``activity.py`` but grouped by
Trove class instead of unioned across all boards.

Each class has two leaderboards - Effort (``4000+i``) and Paragon (``5000+i``) -
so ``class_index = board_uuid % 1000`` (see ``stats.class_index_for_board``). A
class is "active" in a window when a player's score rose (or first appeared) on
EITHER its Effort or its Paragon board (deduped). Both board families reset
weekly (Mon 11:00 UTC), so windows that cross the reset are simply unmeasurable
for those boards (``activity._active_set`` returns ``None``) and contribute no
row - the series shows a gap there rather than a false zero.

We reuse activity.py's primitives wholesale (the score-delta rule, the per-anchor
map loader, the cadence-derived gap detection) and only change the grouping. The
estimate is a LOWER BOUND on players-per-class (same caveats as player activity),
and the "share" is share-of-class-activity: a player who plays N classes counts
in each, so shares sum to 100% but are not distinct players.
"""
from __future__ import annotations

import logging
import time

from app.trove import stats
from app.trove.leaderboards import activity as _act
from app.trove.leaderboards import service as lb_service

logger = logging.getLogger(__name__)

# Effort + Paragon boards are weekly-reset. (They're the fixed weekly set in
# models.py; we pass "weekly" to _active_set rather than per-board lookups.)
_KIND = "weekly"

# Last good /current payload, kept across anchors so a fresh-but-uncomputed or
# reset-crossing latest window still serves the previous result. Mirrors activity.
_LAST_GOOD: dict | None = None

_METHODOLOGY = (
    "Per class, distinct top-N leaderboard players whose score rose (or who first "
    "appear) on its Effort OR Paragon board between two consecutive captures, "
    "deduped. Both boards reset weekly (Mon 11:00 UTC); a window crossing the "
    "reset is unmeasurable and contributes no point. 'Share' is share of class "
    "activity (a player active on several classes counts in each), not distinct "
    "players. Lower bound: players outside a board's top-N aren't seen."
)


def _class_counts(
    early_maps: dict[int, dict[str, float]],
    late_maps: dict[int, dict[str, float]],
    early_ts: int,
    late_ts: int,
) -> dict[int, int]:
    """``{class_index: distinct_active_players}`` for one (early, late) pair.

    Unions a class's two boards and dedups players. A class is OMITTED (no key)
    when neither of its boards is measurable for the window (reset crossed / no
    early snapshot) - so the caller stores nothing for it and the series gaps
    instead of plotting a false 0. A measurable class with no active players
    keeps a 0 (genuinely quiet)."""
    by_class: dict[int, set[str]] = {}
    for uuid, late in late_maps.items():
        if not late:
            continue
        s = _act._active_set(late, early_maps.get(uuid, {}), _KIND, early_ts, late_ts)
        if s is None:
            continue  # reset crossed or no early data for this board
        by_class.setdefault(stats.class_index_for_board(uuid), set()).update(s)
    return {i: len(players) for i, players in by_class.items()}


# ─── warmer + /current ──────────────────────────────────────────────────────


async def estimate_class_activity(*, force: bool = False) -> dict:
    """Compute + persist the latest capture pair's per-class rows (warmer entry).
    Idempotent upsert; returns the /current-shaped payload. ``force`` is accepted
    for symmetry with activity.py but we always recompute the latest pair (cheap:
    one 36-board load each side)."""
    global _LAST_GOOD
    stamps = await lb_service.list_timestamps(limit=500, include_archive=True)
    if len(stamps) < 2:
        return _LAST_GOOD or _empty()
    stamps_desc = stamps
    anchor_late, anchor_early = stamps_desc[0], stamps_desc[1]
    if anchor_late <= anchor_early:
        return _LAST_GOOD or _empty()

    duration_h = (anchor_late - anchor_early) / 3600.0
    gap_threshold = _act._gap_threshold_hours(_act._intervals_hours(stamps_desc))
    if _act._is_gap(duration_h, gap_threshold):
        return _LAST_GOOD or _empty()  # missed capture - withhold

    board_uuids = stats.class_board_uuids()
    late_maps = await _act._load_anchor_maps(anchor_late, board_uuids)
    early_maps = await _act._load_anchor_maps(anchor_early, board_uuids)
    counts = _class_counts(early_maps, late_maps, anchor_early, anchor_late)

    now_ts = int(time.time())
    if counts:
        from app.trove.leaderboards import pg_store
        rows = [
            {"class_index": i, "window_end": anchor_late, "window_start": anchor_early,
             "duration_hours": round(duration_h, 2), "estimate": n, "computed_at": now_ts}
            for i, n in counts.items()
        ]
        try:
            await pg_store.upsert_class_estimates(rows)
        except Exception:
            logger.exception("class activity: persist failed for window_end=%d", anchor_late)

    payload = _build_current(anchor_early, anchor_late, duration_h, counts, now_ts)
    if counts:
        _LAST_GOOD = payload
    return payload


async def class_activity_current() -> dict:
    """Latest stored window's per-class counts + sum-normalized share, read from
    the DB (cheap MAX(window_end) query). Falls back to the warmer's last-good or
    an empty shell."""
    from app.trove.leaderboards import pg_store
    rows = await pg_store.latest_class_estimates()
    if not rows:
        return _LAST_GOOD or _empty()
    we = rows[0]["window_end"]
    ws = rows[0]["window_start"]
    dur = rows[0]["duration_hours"]
    counts = {r["class_index"]: r["estimate"] for r in rows}
    return _build_current(ws, we, dur, counts, rows[0]["computed_at"])


def _build_current(window_start, window_end, duration_h, counts: dict[int, int], computed_at: int) -> dict:
    total = sum(counts.values())
    classes = [
        {"class_index": i, "name": stats.class_name(i), "icon": stats.class_icon(i),
         "active_players": n, "share": round(n / total, 4) if total else 0.0}
        for i, n in counts.items()
    ]
    classes.sort(key=lambda c: (-c["active_players"], c["class_index"]))
    return {
        "window_start": window_start,
        "window_end": window_end,
        "duration_hours": round(duration_h, 2) if duration_h is not None else None,
        "total_active": total if counts else None,
        "classes": classes,
        "methodology": _METHODOLOGY,
        "computed_at": computed_at,
    }


def _empty() -> dict:
    return {
        "window_start": None, "window_end": None, "duration_hours": None,
        "total_active": None, "classes": [], "methodology": _METHODOLOGY,
        "computed_at": int(time.time()),
    }


def reset_caches() -> None:
    global _LAST_GOOD
    _LAST_GOOD = None


# ─── backfill ─────────────────────────────────────────────────────────────────


async def backfill_class_history(
    *, force: bool = False, since_ts: int | None = None, until_ts: int | None = None,
    window_days: int = 7,
) -> dict:
    """Rebuild per-class estimates for every consecutive capture pair in the
    window. Streaming, 1-deep sliding window (peak memory ~one capture's 36-board
    entries). Mirrors ``activity.backfill_history`` but grouped per class."""
    from app.trove.leaderboards import pg_store

    stamps = await lb_service.list_timestamps(limit=1_000_000, include_archive=True)
    if not stamps:
        return {"computed": 0, "skipped": 0, "gap_skipped": 0, "failed": 0,
                "total": 0, "note": "no anchors stored"}
    stamps_asc = sorted(stamps)
    lo = since_ts if since_ts is not None else int(time.time()) - max(1, window_days) * 86400
    hi = until_ts

    pairs = [
        (stamps_asc[i - 1], stamps_asc[i])
        for i in range(1, len(stamps_asc))
        if stamps_asc[i] >= lo and (hi is None or stamps_asc[i] <= hi)
    ]
    if not pairs:
        return {"computed": 0, "skipped": 0, "gap_skipped": 0, "failed": 0,
                "total": 0, "note": "no pairs in window"}

    existing: set[int] = set()
    if not force:
        existing = {r["window_end"] for r in await pg_store.get_class_estimates(pairs[0][1])}
    todo = [p for p in pairs if p[1] not in existing]
    if not todo:
        return {"computed": 0, "skipped": len(pairs), "gap_skipped": 0, "failed": 0,
                "total": len(pairs), "note": "all pairs already stored - use force=True"}

    board_uuids = stats.class_board_uuids()
    needed = sorted(set(a for pr in todo for a in pr))
    early_of = {late: early for early, late in todo}

    intervals = _act._intervals_hours(stamps_asc)
    gap_threshold = _act._gap_threshold_hours(intervals)
    span_days = round((stamps_asc[-1] - stamps_asc[0]) / 86400.0, 1)
    logger.info(
        "class activity backfill: %d pairs over %d anchors, %d classes; gap>%.2fh; "
        "spans %.1f days", len(todo), len(needed), stats.class_count(), gap_threshold, span_days,
    )

    started = time.time()
    computed = failed = gap_skipped = empty_skipped = 0
    prev_anchor: int | None = None
    prev_maps: dict[int, dict[str, float]] | None = None

    for idx, anchor in enumerate(needed):
        cur_maps = await _act._load_anchor_maps(anchor, board_uuids)
        early = early_of.get(anchor)
        if early is not None:
            if _act._is_gap((anchor - early) / 3600.0, gap_threshold):
                gap_skipped += 1
                try:
                    await pg_store.delete_class_estimate(anchor)
                except Exception:
                    logger.exception("class backfill: purge gap row late=%d failed", anchor)
            else:
                try:
                    early_maps = prev_maps if (early == prev_anchor and prev_maps is not None) \
                        else await _act._load_anchor_maps(early, board_uuids)
                    counts = _class_counts(early_maps, cur_maps, early, anchor)
                    now_ts = int(time.time())
                    rows = [
                        {"class_index": i, "window_end": anchor, "window_start": early,
                         "duration_hours": round((anchor - early) / 3600.0, 2),
                         "estimate": n, "computed_at": now_ts}
                        for i, n in counts.items()
                    ]
                    if rows:
                        await pg_store.upsert_class_estimates(rows)
                        computed += 1
                    else:
                        # reset-crossing window: purge any stale rows, store nothing
                        await pg_store.delete_class_estimate(anchor)
                        empty_skipped += 1
                except Exception:
                    failed += 1
                    logger.exception("class backfill: pair late=%d failed", anchor)
        prev_anchor = anchor
        prev_maps = cur_maps
        if (idx + 1) % 20 == 0:
            logger.info("class activity backfill: %d/%d anchors, %d computed (%.1fs)",
                        idx + 1, len(needed), computed, time.time() - started)

    summary = {
        "computed": computed,
        "skipped": len(pairs) - len(todo),
        "gap_skipped": gap_skipped,
        "empty_skipped": empty_skipped,
        "failed": failed,
        "total": len(pairs),
        "anchors": len(stamps_asc),
        "span_days": span_days,
        "gap_threshold_hours": round(gap_threshold, 2),
        "elapsed_seconds": round(time.time() - started, 2),
    }
    logger.info("class activity backfill done: %s", summary)
    return summary


async def reset_class_estimates() -> int:
    from app.trove.leaderboards import pg_store
    from app.trove.leaderboards import cache as lb_cache
    deleted = await pg_store.delete_all_class_estimates()
    reset_caches()
    await lb_cache.invalidate_all_activity()  # sweeps activity_class_series:* too
    logger.warning("class activity: RESET cleared %d stored estimates", deleted)
    return deleted


async def backfill_class_history_chunked(
    *, total_days: int = 400, force: bool = False, reset: bool = False,
) -> dict:
    """Wraps the streaming backfill. ``total_days<=0`` = all stored history;
    ``reset=True`` wipes first (implies force)."""
    reset_deleted = 0
    if reset:
        reset_deleted = await reset_class_estimates()
        force = True
    now = int(time.time())
    since = 0 if total_days <= 0 else now - total_days * 86400
    res = await backfill_class_history(since_ts=since, until_ts=now, force=force)
    out = dict(res)
    out["total_days"] = total_days
    out["reset_deleted"] = reset_deleted
    logger.info("class activity backfill (chunked) done: %s", out)
    return out


# ─── series (multi-line chart) ────────────────────────────────────────────────


async def class_activity_series(period: str = "7d") -> dict:
    """Per-class bucketed series for the Class Activity page. Shared x-axis
    (``buckets``) + per-class ``values`` (avg active/hr in each bucket, null when
    a class had no data in that bucket), so the page draws one aligned line per
    class. Read-through Redis cache (short TTL)."""
    from app.trove.leaderboards import pg_store
    from app.trove.leaderboards import cache as lb_cache

    period = (period or "7d").lower()
    if period not in _act._SERIES_PERIODS:
        period = "7d"
    cached = await lb_cache.get_class_activity_series(period)
    if cached is not None:
        return cached

    days, bucket = _act._SERIES_PERIODS[period]
    now_ts = int(time.time())
    if days is not None:
        window_start = now_ts - days * 86400
        rows = await pg_store.get_class_estimates(window_start)
    else:
        rows = await pg_store.get_class_estimates(None)
        window_start = rows[0]["window_end"] if rows else now_ts
    if bucket is None:
        span = max(3600, now_ts - window_start)
        bucket = max(3600, (int(span / 120) // 3600) * 3600)

    gap_threshold = _act._gap_threshold_hours(
        sorted({r["duration_hours"] for r in rows})
    )

    # bucket -> {t_sum, t_n, classes: {i: {sum, n}}}
    agg: dict[int, dict] = {}
    for r in rows:
        dur = r["duration_hours"] or 0.0
        if _act._is_gap(dur, gap_threshold):
            continue
        ph = (r["estimate"] / dur) if dur > 0 else 0.0
        b = (r["window_end"] // bucket) * bucket
        bd = agg.get(b)
        if bd is None:
            bd = agg[b] = {"t_sum": 0, "t_n": 0, "classes": {}}
        bd["t_sum"] += r["window_end"]
        bd["t_n"] += 1
        c = bd["classes"].get(r["class_index"])
        if c is None:
            c = bd["classes"][r["class_index"]] = {"sum": 0.0, "n": 0}
        c["sum"] += ph
        c["n"] += 1

    sorted_b = sorted(agg)
    buckets = [round(agg[b]["t_sum"] / agg[b]["t_n"]) for b in sorted_b]
    classes = []
    for i in range(stats.class_count()):
        values = []
        for b in sorted_b:
            c = agg[b]["classes"].get(i)
            values.append(round(c["sum"] / c["n"], 1) if c else None)
        classes.append({"class_index": i, "name": stats.class_name(i),
                        "icon": stats.class_icon(i), "values": values})

    payload = {
        "period": period,
        "bucket_seconds": bucket,
        "window_start": window_start,
        "window_end": now_ts,
        "buckets": buckets,
        "classes": classes,
        "methodology": _METHODOLOGY,
    }
    await lb_cache.set_class_activity_series(period, payload)
    return payload

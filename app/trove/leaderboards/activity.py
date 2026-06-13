"""Estimated active-player count via leaderboard score deltas.

A player whose score went up on at least one leaderboard between two
consecutive captures was active in that window. ALL player boards are used as
context, but a board whose reset falls inside a window is IGNORED for that
window: its score zeroes at the reset (daily every day 11:00 UTC, weekly
Monday 11:00 UTC), so a score-delta is meaningless there and counting everyone
present post-reset would spike the count. Lifetime boards (ENEMIES DEFEATED,
FLUX EARNED, LOOT COLLECTED, …) never reset so they always contribute; a
daily/weekly board contributes via score-delta on any window that stays inside
one of its cycles (i.e. doesn't cross its 11:00 UTC reset).

The count is a **lower bound on active players**, not the absolute
total: it only sees players who scored on at least one tracked board
*and* whose ranking was within the top of that board. A casual player
who logged in for 20 minutes but never broke the top 5000 on any
lifetime stat won't be counted. That's fine for a "how busy is Trove
right now" UI - the trend is what matters, not the absolute number.

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

# Top-N entries per board to consider. Bigger = more players covered but
# more rows fetched per board. The bot now dumps up to ~20k entries per
# board (was ~5k), so fetch past that to cover the full per-board population.
_BOARD_FETCH_LIMIT = 25000

# A consecutive capture pair spans roughly one capture interval. A window MUCH
# longer than that means a capture was MISSED (a gap): the distinct-active count
# over a multi-hour gap can't be turned into an honest per-hour rate - the same
# players stay active across the gap, so dividing by the duration UNDER-counts.
# So we skip gap windows entirely: no estimate is computed, stored, or plotted,
# and the graph resumes once normal-cadence captures return.
#
# The cadence is NOT assumed to be hourly. We derive the gap cutoff from the DATA
# (the median spacing of the surrounding captures), so a 2-hourly bot, a
# re-ingested archive at another interval, or jittery captures don't get chopped
# up - only genuinely-missed captures (markedly longer than the median) are
# dropped. ``_GAP_FLOOR_HOURS`` is the floor so tight hourly data keeps its old
# behaviour; ``_GAP_FACTOR`` is how many median-intervals long a window must be
# to count as a gap.
_GAP_FLOOR_HOURS = 1.5
_GAP_FACTOR = 1.9


def _median(values: list[float]) -> float | None:
    vals = sorted(v for v in values if v and v > 0)
    n = len(vals)
    if n == 0:
        return None
    mid = n // 2
    return vals[mid] if n % 2 else (vals[mid - 1] + vals[mid]) / 2.0


def _gap_threshold_hours(intervals_hours: list[float]) -> float:
    """Per-run gap cutoff (hours): a window longer than this is a missed capture.
    Derived from the median capture spacing so it adapts to any cadence; floored
    so jittery hourly data isn't chopped into pieces."""
    med = _median(intervals_hours)
    if med is None:
        return _GAP_FLOOR_HOURS
    return max(_GAP_FLOOR_HOURS, med * _GAP_FACTOR)


def _intervals_hours(anchors: list[int]) -> list[float]:
    """Consecutive spacings (hours) of a set of capture anchors, any order."""
    a = sorted(anchors)
    return [(a[i] - a[i - 1]) / 3600.0 for i in range(1, len(a))]


def _is_gap(duration_hours: float | None, threshold: float = _GAP_FLOOR_HOURS) -> bool:
    """A window that spans a missed capture (longer than the cadence-derived
    ``threshold``). Defaults to the floor when no cadence is known."""
    return duration_hours is not None and duration_hours > threshold


async def estimate_active_players(*, force: bool = False) -> dict:
    """Compute the activity estimate for the most-recent window. Cached
    by ``(latest_anchor, prev_anchor)``; falls back to ``_LAST_GOOD``
    when the newest pair hasn't been computed yet, so the user never
    waits on a fresh ingest. Returns a structured dict regardless of
    data availability - a single-anchor or empty DB returns a 'no
    estimate' shape with ``estimate=None``.

    ``force=True`` (used by the warmer) bypasses BOTH the cache hit and the
    _LAST_GOOD shortcut and recomputes + re-persists, so a re-ingest onto the
    same anchor refreshes the estimate instead of serving the pre-ingest slot.
    """
    from app.admin import runtime_config
    from app.trove.leaderboards import cache as lb_cache

    cache_ttl = float(await runtime_config.get_setting("cheaters_cache_ttl_seconds"))
    now = time.time()
    global _LAST_GOOD

    if not force:
        # Serve the latest PUBLISHED anchor's estimate so the live-pulse matches
        # the snapshot on screen; the persisted Redis copy survives restarts.
        serve_anchor = await lb_cache.get_ready_anchor()
        if serve_anchor is not None:
            persisted = await lb_cache.get_activity(serve_anchor)
            if persisted is not None:
                _LAST_GOOD = persisted
                return persisted
        # No published/persisted estimate yet - fall back to the in-process
        # cache / last-good (stale-but-good), else an empty placeholder.
        stamps = await lb_service.list_timestamps(limit=2, include_archive=False)
        if len(stamps) < 2:
            return _empty()
        hit = _CACHE.get((stamps[0], stamps[1]))
        if hit is not None and now - hit[0] < cache_ttl:
            _LAST_GOOD = hit[1]
            return hit[1]
        from app.trove.leaderboards import detection
        detection.trigger_warmer()
        if _LAST_GOOD is not None:
            return _LAST_GOOD
        return _empty()

    # Warmer: compute (or adopt a fresh persisted snapshot for) the raw latest pair.
    stamps = await lb_service.list_timestamps(limit=2, include_archive=False)
    if len(stamps) < 2:
        return _empty()
    anchor_late, anchor_early = stamps[0], stamps[1]
    if anchor_late <= anchor_early:
        return _empty()
    cache_key = (anchor_late, anchor_early)

    persisted = await lb_cache.get_activity(anchor_late)
    if persisted is not None and now - persisted.get("computed_at", 0) < cache_ttl:
        # Fresh processed snapshot already in Redis (restart, no new capture).
        _CACHE[cache_key] = (now, persisted)
        _LAST_GOOD = persisted
        return persisted

    payload = await _compute(anchor_late, anchor_early)
    _CACHE[cache_key] = (now, payload)
    _LAST_GOOD = payload
    await lb_cache.set_activity(anchor_late, payload)
    _prune(now, cache_ttl)
    return payload


def invalidate_cache() -> None:
    _CACHE.clear()


def reset_caches() -> None:
    """Drop the in-process activity caches (keyed cache + last-good). Used by the
    full leaderboards reset (the Postgres estimates are TRUNCATEd separately)."""
    global _LAST_GOOD
    _CACHE.clear()
    _LAST_GOOD = None


# ─── Implementation ────────────────────────────────────────────────────


def _pick_early_anchor(
    stamps_desc: list[int], anchor_late: int, seconds_back: int,
) -> int | None:
    """Pick the stored capture that best anchors a "last N seconds" window
    ending at ``anchor_late``.

    ``stamps_desc`` is the full anchor list, newest first. We want the
    NEWEST anchor that is at-or-before ``anchor_late - seconds_back`` so
    the window spans at least the requested duration. If no anchor is that
    old yet (early in a deploy, sparse history) we fall back to the OLDEST
    anchor strictly before ``anchor_late`` - the widest window the data can
    honestly support. Returns ``None`` when there's no earlier anchor at
    all (single capture)."""
    target = anchor_late - seconds_back
    oldest_before: int | None = None
    for s in stamps_desc:               # newest -> oldest
        if s >= anchor_late:
            continue                    # only look strictly into the past
        if s <= target:
            return s                    # newest anchor at-or-before the target
        oldest_before = s               # keep walking back; remember the oldest
    return oldest_before


def _active_set(
    late_scores: dict[str, float],
    early_scores: dict[str, float],
    kind: str,
    early_ts: int,
    late_ts: int,
) -> set[str] | None:
    """Distinct active players on ONE board between ``early_ts`` and ``late_ts``,
    from the two anchors' pre-loaded ``{player: score}`` maps.

    Returns ``None`` to mean "skip this board for this window" (distinct from an
    empty set = "no one active"):
      * the board's reset fell inside the window - its score zeroed there, so a
        score-delta is meaningless and counting everyone present would spike the
        estimate (daily 11:00 UTC, weekly Monday 11:00 UTC, lifetime never); or
      * there's no early snapshot for the board.
    Otherwise a positive score delta (or first appearance) vs the early snapshot
    counts the player as active. Pure (no I/O) - the anchors are loaded once,
    up front, by ``_compute`` via ``_load_anchor_maps``."""
    if lb_service.reset_boundaries_for_kind(kind, early_ts, late_ts):
        return None   # board reset inside this window -> ignore it here
    if not early_scores:
        return None
    active: set[str] = set()
    for name, score in late_scores.items():
        prev = early_scores.get(name)
        if prev is None or score > prev:
            active.add(name)
    return active


async def _compute(anchor_late: int, anchor_early: int) -> dict:
    """Iterate EVERY player_board, fetch entries at both anchors, count
    distinct players with a positive activity signal.

    ALL boards are used as context, but a board whose reset falls inside the
    window is IGNORED for that window - its score zeroed at the reset, so a
    score-delta is meaningless and counting everyone present post-reset would
    spike the estimate (the daily 11:00 UTC reset would dwarf the real signal).
    Cadence: daily resets every day at 11:00 UTC, weekly on Monday 11:00 UTC,
    lifetime (default/none) never - so lifetime boards always count, and a
    daily/weekly board counts via score-delta on any window that doesn't cross
    its reset.

    Server-tally boards (``player_board=False``, e.g. CLUB POWER RANK)
    are skipped - those aggregate everyone's contributions and don't tell us
    about individual activity.

    Persists the result to the ``activity_estimate`` table (upsert by
    ``window_end``) so the history graph survives container restarts."""
    boards = await lb_service.list_boards_at(anchor_late)
    duration_h = (anchor_late - anchor_early) / 3600.0

    # Wider rollups share the SAME late anchor; only the early endpoint
    # moves back. The distinct-active count between (late-24h, late) and
    # (late-7d, late) is an honest lower bound on "players active in the
    # last 24h / 7d" - no double-counting, unlike summing hourly windows.
    stamps_desc = await lb_service.list_timestamps(limit=500, include_archive=True)
    early_24h = _pick_early_anchor(stamps_desc, anchor_late, 86400)
    early_7d = _pick_early_anchor(stamps_desc, anchor_late, 7 * 86400)

    # Gap cutoff from the ACTUAL recent cadence (median spacing), not a hardcoded
    # 1h. A pair markedly longer than the median is a missed capture: its estimate
    # would be a miscalculation, so we withhold it (None) and store nothing.
    gap_threshold = _gap_threshold_hours(_intervals_hours(stamps_desc))
    gapped = _is_gap(duration_h, gap_threshold)

    # Server-tally boards aggregate scores across everyone; they don't reflect
    # individual activity. Everything else counts.
    player_boards = [b for b in boards if b.get("player_board", True)]
    board_uuids = [b["uuid"] for b in player_boards]
    meta_by_uuid = {b["uuid"]: b for b in player_boards}

    # Load each DISTINCT anchor ONCE as {uuid: {player: score}} via a single
    # projected query (no per-board round-trip, no Pydantic) instead of the old
    # ~one-list_entries-per-board-per-window. Late + up to three earlier
    # endpoints (1h / 24h / 7d) = at most four queries for the whole estimate.
    late_maps = await _load_anchor_maps(anchor_late, board_uuids)
    early_maps: dict[int, dict[int, dict[str, float]]] = {}
    if not gapped:
        # 1h early is the core estimate - unguarded (a failure should make the
        # warmer retry rather than publish a bogus 0).
        early_maps[anchor_early] = await _load_anchor_maps(anchor_early, board_uuids)
    # Wider early anchors are guarded so a slow/absent deep-archive read can
    # never abort the core 1h estimate or the warmer's publish.
    for ea in (early_24h, early_7d):
        if ea is None or ea in early_maps:
            continue
        try:
            early_maps[ea] = await _load_anchor_maps(ea, board_uuids)
        except Exception:
            logger.exception("activity: failed loading wide early anchor %d", ea)
            early_maps[ea] = {}

    active_union: set[str] = set()      # 1h window (drives the live pulse)
    active_24h: set[str] = set()
    active_7d: set[str] = set()
    per_board: list[dict] = []

    for uuid in board_uuids:
        late = late_maps.get(uuid)
        if not late:
            continue
        kind = meta_by_uuid[uuid].get("reset_kind", "default")

        # 1h window - the live pulse plus the per-board breakdown. Skipped on a
        # gap (the 1h estimate is withheld until consecutive captures resume).
        if not gapped:
            s = _active_set(
                late, early_maps.get(anchor_early, {}).get(uuid, {}),
                kind, anchor_early, anchor_late,
            )
            if s is not None:
                active_union.update(s)
                meta = meta_by_uuid[uuid]
                per_board.append({
                    "uuid": uuid,
                    "name": meta["name"],
                    "category": meta["category"],
                    "active_players": len(s),
                })

        # Wider windows - aggregate counts only, no per-board breakdown.
        for early_w, bucket in ((early_24h, active_24h), (early_7d, active_7d)):
            if early_w is None:
                continue
            s = _active_set(
                late, early_maps.get(early_w, {}).get(uuid, {}), kind, early_w, anchor_late,
            )
            if s:
                bucket.update(s)

    # Sort by per-board activity desc - the highest-engagement boards
    # rise to the top of the breakdown.
    per_board.sort(key=lambda b: -b["active_players"])

    now_ts = int(time.time())
    estimate_count = None if gapped else len(active_union)
    boards_count = len(per_board)
    # Span (in hours) the wide rollups actually covered - lets the UI label
    # them honestly ("7d" really means "since the oldest capture" until a
    # full week of history exists). None when the window had no anchor.
    span_24h = round((anchor_late - early_24h) / 3600.0, 1) if early_24h is not None else None
    span_7d = round((anchor_late - early_7d) / 3600.0, 1) if early_7d is not None else None

    # Persist the point so the history chart accumulates across restarts.
    # Upsert by window_end (the unique index) so re-runs converge instead
    # of stacking. Direct motor-style update_one - Beanie's high-level
    # update API doesn't give us a clean upsert-on-unique-index pattern.
    # Failure to persist is non-fatal - log and move on.
    # Persist the graph point ONLY for consecutive windows. A gap window stores
    # nothing - the graph has no point there and resumes on the next good pair.
    if not gapped:
        try:
            from app.trove.leaderboards import pg_store
            await pg_store.upsert_estimate(
                anchor_late, anchor_early, round(duration_h, 2),
                estimate_count, boards_count, now_ts,
            )
        except Exception:
            logger.exception("activity: failed to persist estimate for window_end=%d", anchor_late)

    return {
        "window_start": anchor_early,
        "window_end": anchor_late,
        "duration_hours": round(duration_h, 2),
        "estimate": estimate_count,
        # True when this pair spans a missed capture: the 1h estimate is withheld
        # (None) and nothing is stored for the graph until consecutive captures resume.
        "gapped": gapped,
        # Distinct active players over the wider rollups (same late anchor,
        # earlier endpoint). None when there isn't an earlier anchor yet.
        "estimate_24h": (len(active_24h) if early_24h is not None else None),
        "estimate_7d": (len(active_7d) if early_7d is not None else None),
        "window_24h_start": early_24h,
        "window_7d_start": early_7d,
        "span_24h_hours": span_24h,
        "span_7d_hours": span_7d,
        "by_board": per_board,
        "boards_analyzed": boards_count,
        "methodology": (
            "Distinct top-N leaderboard players whose score increased (or who "
            "first appear) on at least one board between the two most recent "
            "captures. All boards count, except a board is ignored on any "
            "window that crosses its reset (daily 11:00 UTC, weekly Monday "
            "11:00 UTC, lifetime never) - its score zeroes there so a delta "
            "would be meaningless. A window that spans a missed capture (markedly "
            "longer than the median capture cadence) is skipped entirely - such a "
            "gap can't be normalized to an honest per-hour rate. Lower bound: "
            "players outside every board's top-N aren't seen."
        ),
        "computed_at": now_ts,
    }


async def _load_anchor_maps(
    anchor: int, board_uuids: list[int],
) -> dict[int, dict[str, float]]:
    """All player-board entries at ONE anchor as ``{uuid: {player: score}}`` -
    a single Postgres query (``entry`` joined to ``player`` for the names). This
    is the unit the streaming backfill holds in memory: one capture at a time."""
    from app.trove.leaderboards import pg_store
    return await pg_store.anchor_maps(anchor, board_uuids)


def _pair_estimate(
    early_maps: dict[int, dict[str, float]],
    late_maps: dict[int, dict[str, float]],
    boards: dict[int, dict],
    early_ts: int,
    late_ts: int,
) -> tuple[int, int]:
    """Distinct active players + boards-analyzed for one ``(early, late)`` pair
    from the two anchors' pre-loaded score maps. Same rule as ``_compute``: a
    board whose reset falls inside the window is IGNORED for that window (its
    score zeroed, so a delta is meaningless); otherwise a positive score delta
    (or first appearance) vs the early snapshot. Daily resets every day 11:00
    UTC, weekly Monday 11:00 UTC, lifetime never."""
    active_union: set[str] = set()
    boards_analyzed = 0
    for uuid, meta in boards.items():
        late = late_maps.get(uuid)
        if not late:
            continue
        crossed = bool(lb_service.reset_boundaries_for_kind(
            meta["reset_kind"], early_ts, late_ts,
        ))
        if crossed:
            continue   # board reset inside this window -> ignore it here
        early = early_maps.get(uuid)
        if not early:
            continue
        for name, score in late.items():
            prev = early.get(name)
            if prev is None or score > prev:
                active_union.add(name)
        boards_analyzed += 1
    return len(active_union), boards_analyzed


async def backfill_history(
    *, window_days: int = 7, force: bool = False,
    since_ts: int | None = None, until_ts: int | None = None,
) -> dict:
    """Rebuild stored activity estimates for every consecutive capture pair
    whose LATE anchor falls in the window.

    MEMORY-SAFE by construction: anchors stream in ascending order holding a
    1-deep sliding window (the previous anchor's score maps), because all a
    per-window estimate needs is a consecutive pair. Peak memory is ~one
    capture's entries (tens of MB) no matter how long the range - the old
    batched version loaded the WHOLE range's entries into one dict (millions of
    rows -> multi-GB -> OOM on dense hourly data), which this replaces.

    Window: trailing ``window_days`` by default, or an explicit
    ``since_ts`` / ``until_ts`` slice (bounds on the LATE anchor)."""
    from app.trove.leaderboards import pg_store
    from app.trove.leaderboards.models import is_player_board

    stamps = await lb_service.list_timestamps(limit=1_000_000, include_archive=True)
    if not stamps:
        return {"computed": 0, "skipped": 0, "failed": 0, "total": 0,
                "note": "no anchors stored"}
    stamps_asc = sorted(stamps)
    lo = since_ts if since_ts is not None else int(time.time()) - max(1, window_days) * 86400
    hi = until_ts   # None = no upper bound

    # Consecutive (early, late) pairs whose late anchor is in [lo, hi].
    pairs: list[tuple[int, int]] = []
    for i in range(1, len(stamps_asc)):
        early, late = stamps_asc[i - 1], stamps_asc[i]
        if late < lo:
            continue
        if hi is not None and late > hi:
            continue
        pairs.append((early, late))
    if not pairs:
        return {"computed": 0, "skipped": 0, "failed": 0, "total": 0,
                "note": "no pairs in window"}

    existing: set[int] = set()
    if not force:
        rows = await pg_store.get_estimates(pairs[0][1])
        existing = {r["window_end"] for r in rows}
    todo = [p for p in pairs if p[1] not in existing]
    if not todo:
        return {"computed": 0, "skipped": len(pairs), "failed": 0,
                "total": len(pairs), "note": "all pairs already stored - use force=True to recompute"}

    # Player-board metadata (reset kind with admin override applied).
    boards: dict[int, dict] = {}
    for d in await pg_store.all_boards():
        if not is_player_board(d["uuid"]):
            continue
        boards[d["uuid"]] = {
            "name": d["name"], "category": d["category"], "reset_kind": d["reset_kind"],
        }
    board_uuids = list(boards.keys())

    # Anchors to stream, ascending: the union of every todo pair's endpoints.
    # Pairs are CONSECUTIVE stamps, so a late anchor's early side is the
    # immediately-preceding streamed anchor - a 1-deep window (prev) covers
    # every pair with no reload (the defensive branch handles force=False gaps).
    needed = sorted(set(a for pr in todo for a in pr))
    early_of = {late: early for early, late in todo}

    # Gap cutoff from the FULL stored cadence (median spacing), so a non-hourly or
    # jittery archive isn't chopped to pieces - only genuinely-missed captures
    # (markedly longer than the median) are dropped.
    intervals = _intervals_hours(stamps_asc)
    median_interval = _median(intervals)
    gap_threshold = _gap_threshold_hours(intervals)
    span_days = round((stamps_asc[-1] - stamps_asc[0]) / 86400.0, 1)

    logger.info(
        "activity backfill: %d pairs over %d anchors, %d boards; cadence median=%.2fh "
        "gap>%.2fh; data spans %.1f days (%d total anchors)",
        len(todo), len(needed), len(boards),
        median_interval or 0.0, gap_threshold, span_days, len(stamps_asc),
    )
    started = time.time()
    computed = failed = gap_skipped = 0
    prev_anchor: int | None = None
    prev_maps: dict[int, dict[str, float]] | None = None

    for i, anchor in enumerate(needed):
        cur_maps = await _load_anchor_maps(anchor, board_uuids)
        early = early_of.get(anchor)
        if early is not None:
            if _is_gap((anchor - early) / 3600.0, gap_threshold):
                # Missed-capture gap: no honest per-hour estimate. Skip it, and
                # purge any stale row a pre-gap-aware run left so the graph is clean.
                gap_skipped += 1
                try:
                    await pg_store.delete_estimate(anchor)
                except Exception:
                    logger.exception("activity backfill: purge gap row late=%d failed", anchor)
            else:
                try:
                    if early == prev_anchor and prev_maps is not None:
                        early_maps = prev_maps
                    else:
                        early_maps = await _load_anchor_maps(early, board_uuids)
                    est, nboards = _pair_estimate(early_maps, cur_maps, boards, early, anchor)
                    await pg_store.upsert_estimate(
                        anchor, early, round((anchor - early) / 3600.0, 2),
                        est, nboards, int(time.time()),
                    )
                    computed += 1
                except Exception:
                    failed += 1
                    logger.exception("activity backfill: pair late=%d failed", anchor)
        prev_anchor = anchor
        prev_maps = cur_maps
        if (i + 1) % 20 == 0:
            logger.info("activity backfill: streamed %d/%d anchors, %d computed (%.1fs)",
                        i + 1, len(needed), computed, time.time() - started)

    summary = {
        "computed": computed,
        "skipped": len(pairs) - len(todo),
        "gap_skipped": gap_skipped,
        "failed": failed,
        "total": len(pairs),
        "anchors": len(stamps_asc),
        "span_days": span_days,
        "median_interval_hours": round(median_interval, 2) if median_interval else None,
        "gap_threshold_hours": round(gap_threshold, 2),
        "elapsed_seconds": round(time.time() - started, 2),
    }
    logger.info("activity backfill done: %s", summary)
    return summary


async def reset_estimates() -> int:
    """Wipe the stored activity history (the Postgres ``activity_estimate`` table)
    AND every cache layer so a fresh recompute starts from a truly clean slate:
    the PG estimate table, the in-process estimate cache, and the Redis
    live-snapshot keys (``lb:cache:activity:*``).

    The table is fully DERIVED data - every row is rebuildable from the
    leaderboard captures via ``backfill_history*`` - so clearing it loses
    nothing irreplaceable; it just discards values from earlier (possibly
    miscalculated / interrupted) runs. Returns the number of PG rows deleted.

    (The same-origin ``/site/leaderboards/activity*`` proxies send ``no-cache``,
    so the page picks the recompute up immediately - no HTTP/CDN staleness.)"""
    from app.trove.leaderboards import pg_store
    from app.trove.leaderboards import cache as lb_cache
    deleted = await pg_store.delete_all_estimates()
    invalidate_cache()
    global _LAST_GOOD
    _LAST_GOOD = None
    redis_cleared = await lb_cache.invalidate_all_activity()
    logger.warning("activity: RESET cleared %d stored estimates, %d redis snapshots",
                   deleted, redis_cleared)
    return deleted


async def backfill_history_chunked(
    *, total_days: int = 400, chunk_days: int = 14, force: bool = False,
    reset: bool = False,
) -> dict:
    """Rebuild the activity history over a long range.

    Streaming (see ``backfill_history``) keeps peak memory at ~one capture's
    entries regardless of range, so the old day-chunking is no longer needed -
    ``chunk_days`` is accepted for API compatibility but ignored. This wraps a
    single streaming pass over the trailing ``total_days``.

    ``total_days <= 0`` means **all stored history** (no lower bound) - so a
    Reset & recalculate rebuilds the FULL series, not just a fixed window, even
    when the entry data reaches further back than the default.

    ``reset=True`` first wipes the whole estimate table and recomputes from
    scratch (implies ``force``) - to discard earlier miscalculated runs."""
    reset_deleted = 0
    if reset:
        reset_deleted = await reset_estimates()
        force = True            # clean slate -> recompute every window

    now = int(time.time())
    # 0/negative => no lower bound: cover every stored anchor (backfill_history
    # only processes pairs that actually exist, so this wastes no work).
    since = 0 if total_days <= 0 else now - total_days * 86400
    started = time.time()
    res = await backfill_history(since_ts=since, until_ts=now, force=force)

    out = dict(res)
    out["total_days"] = total_days
    out["reset_deleted"] = reset_deleted
    out["elapsed_seconds"] = round(time.time() - started, 2)
    logger.info("activity backfill (chunked) done: %s", out)
    return out


async def estimate_active_players_history(*, days: int = 7) -> dict:
    """Time-series of active-player estimates across the last ``days`` days
    of stored captures, served straight from the Postgres ``activity_estimate``
    table (no per-board re-scan).

    Each row carries both ``estimate`` (raw distinct-player count for its
    window) and ``estimate_per_hour`` (= estimate / duration_hours). The
    per-hour rate is the right Y-axis for a chart that should look
    smooth across irregular window sizes - when a capture is missed and
    the next window spans 2-3h instead of 1h, the raw count spikes
    because more players had time to score; the per-hour rate stays
    honest and the chart doesn't lie about the trend.

    New rows land each time the cheaters warmer fires
    ``estimate_active_players()`` (typically right after each ingest), so the
    series fills in naturally as captures arrive. Empty / one-row history
    returns an empty series - the consumer just doesn't render the chart."""
    from app.trove.leaderboards import pg_store

    days = max(1, min(days, 30))
    now_ts = int(time.time())
    window_start = now_ts - days * 86400

    rows = await pg_store.get_estimates(window_start)

    # Gap cutoff from the cadence of the STORED windows themselves, so a non-hourly
    # archive's points aren't all filtered out as "gaps" at display time.
    gap_threshold = _gap_threshold_hours([r["duration_hours"] for r in rows])

    series = []
    for r in rows:
        duration_h = r["duration_hours"] or 0.0
        if _is_gap(duration_h, gap_threshold):
            continue   # window spans a missed capture - skip it (not a real point)
        per_hour = (r["estimate"] / duration_h) if duration_h > 0 else 0.0
        series.append({
            "window_end": r["window_end"],
            "window_start": r["window_start"],
            "duration_hours": round(duration_h, 2),
            "estimate": r["estimate"],
            # Round to 1 dp - a chart never needs more precision than
            # the integer underlying it has anyway.
            "estimate_per_hour": round(per_hour, 1),
        })

    return {
        "days": days,
        "window_start": window_start,
        "window_end": now_ts,
        "points": series,
        "methodology": (
            "Distinct top-N leaderboard players whose score increased on at "
            "least one board between each consecutive capture pair (a board is "
            "ignored on any window crossing its reset). Windows that span a "
            "missed capture (markedly longer than the median capture cadence) are "
            "skipped - such a gap can't be normalized to an honest per-hour rate - "
            "so the series resumes once normal-cadence captures return."
        ),
    }


# Period -> (lookback_days | None for all-time, bucket_seconds | None for
# dynamic). Bucket sizes are chosen so each period yields a clean ~24-90
# points: raw-ish for short ranges, coarser for long ones so a year isn't
# 8.7k hourly dots. The Y value plotted is the AVERAGE estimate_per_hour
# inside each bucket - a smooth "how busy is Trove" activity level.
_SERIES_PERIODS: dict[str, tuple[int | None, int | None]] = {
    "1d": (1, 3600),            # 24 hourly points
    "7d": (7, 3600),            # ~168 hourly points
    "1m": (30, 86400),          # 30 daily points
    "3m": (90, 86400),          # 90 daily points
    "6m": (180, 2 * 86400),     # 90 two-daily points
    "1y": (365, 7 * 86400),     # ~52 weekly points
    "all": (None, None),        # dynamic bucket, ~120 points
}


async def activity_series(period: str = "7d") -> dict:
    """Bucketed activity-level time-series for the Player Activity page.

    Reads the stored per-window estimates and downsamples them into fixed
    time buckets sized to ``period`` (see ``_SERIES_PERIODS``), so the
    chart stays readable from "last 24h" to "all time" without shipping
    thousands of points. Each bucket reports the AVERAGE and PEAK
    ``estimate_per_hour`` of the windows that fall in it. Also returns the
    period peak (with its timestamp), the period average, and the latest
    live level so the page's stat cards don't need a second request.

    Cheap: a single indexed range scan + in-Python bucketing, no per-board
    re-computation. Empty collection -> empty ``points`` (page hides the
    chart). Unknown ``period`` falls back to ``7d``."""
    from app.trove.leaderboards import pg_store
    from app.trove.leaderboards import cache as lb_cache

    period = (period or "7d").lower()
    if period not in _SERIES_PERIODS:
        period = "7d"
    # Read-through Redis cache: the chart is identical for every visitor and only
    # shifts when a new capture lands (~hourly), so a short-TTL cache keeps the
    # Player Activity page off Mongo for the common case.
    cached = await lb_cache.get_activity_series(period)
    if cached is not None:
        return cached
    days, bucket = _SERIES_PERIODS[period]
    now_ts = int(time.time())

    if days is not None:
        window_start = now_ts - days * 86400
        rows = await pg_store.get_estimates(window_start)
    else:
        rows = await pg_store.get_estimates(None)
        window_start = rows[0]["window_end"] if rows else now_ts

    # All-time picks a bucket that yields ~120 points across whatever span
    # actually exists, rounded down to a whole hour for tidy axis labels.
    if bucket is None:
        span = max(3600, now_ts - window_start)
        bucket = max(3600, (int(span / 120) // 3600) * 3600)

    # Gap cutoff from the cadence of the STORED windows, so a non-hourly archive's
    # points aren't all dropped as "gaps".
    gap_threshold = _gap_threshold_hours([r["duration_hours"] for r in rows])

    # Group into fixed buckets keyed by bucket-start; average the per-hour
    # rate inside each (and keep the bucket's peak for the tooltip).
    agg: dict[int, dict] = {}
    for r in rows:
        dur = r["duration_hours"] or 0.0
        if _is_gap(dur, gap_threshold):
            continue   # window spans a missed capture - skip it
        ph = (r["estimate"] / dur) if dur > 0 else 0.0
        b = (r["window_end"] // bucket) * bucket
        a = agg.get(b)
        if a is None:
            agg[b] = {"sum": ph, "max": ph, "n": 1, "t_sum": r["window_end"]}
        else:
            a["sum"] += ph
            a["n"] += 1
            a["t_sum"] += r["window_end"]
            if ph > a["max"]:
                a["max"] = ph

    points: list[dict] = []
    peak: dict | None = None
    avg_running = 0.0
    for b in sorted(agg):
        a = agg[b]
        avg = a["sum"] / a["n"]
        # Plot at the AVERAGE actual capture time in the bucket, not the floored
        # bucket start - so a 21:52 capture shows at 21:52, not 21:00. For a fine
        # period (one capture per bucket) this is exactly that capture's time; for
        # a coarse bucket it's the centroid of the captures it holds.
        t = round(a["t_sum"] / a["n"])
        points.append({
            "t": t,
            "active": round(avg, 1),         # avg active players / hour
            "peak": round(a["max"], 1),      # busiest hour in the bucket
            "samples": a["n"],
        })
        avg_running += avg
        if peak is None or avg > peak["active"]:
            peak = {"t": t, "active": round(avg, 1)}

    average = round(avg_running / len(points), 1) if points else None
    last = next((r for r in reversed(rows) if not _is_gap(r["duration_hours"] or 0.0, gap_threshold)), None)
    latest = (
        round(last["estimate"] / last["duration_hours"], 1)
        if last and (last["duration_hours"] or 0) > 0 else None
    )

    payload = {
        "period": period,
        "bucket_seconds": bucket,
        "window_start": window_start,
        "window_end": now_ts,
        "points": points,
        "peak": peak,
        "average": average,
        "latest": latest,
        "methodology": (
            "Average distinct active players per hour in each time bucket, from "
            "the stored per-capture estimates; windows that span a missed capture "
            "(markedly longer than the median cadence) are skipped. Longer periods "
            "use coarser buckets (daily/weekly) so the line stays readable."
        ),
    }
    await lb_cache.set_activity_series(period, payload)
    return payload


def _empty() -> dict:
    return {
        "window_start": None,
        "window_end": None,
        "duration_hours": None,
        "estimate": None,
        "estimate_24h": None,
        "estimate_7d": None,
        "window_24h_start": None,
        "window_7d_start": None,
        "span_24h_hours": None,
        "span_7d_hours": None,
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

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

Captures are no longer hourly, so everything shown is a DAILY figure: the
distinct players active in the 24 hours (or 7 days) up to a capture, unioned
over every capture window in that span. That count doesn't depend on how
often captures land, unlike the old per-hour rate.

Cached in-process for ``cheaters_cache_ttl_seconds`` (we piggy-back
on the same tunable) keyed by the (latest, previous) anchor pair, so
a new capture invalidates automatically.
"""
from __future__ import annotations

import bisect
import logging
import time
from collections import deque

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

_DAY = 86400

# A rollup covers every capture window from the newest capture at-or-before its
# start, so a capture gap stretches it past its nominal span. Past this factor
# (36h for the 24h figure) it no longer measures that period and reads as None.
_ROLLUP_SLACK = 1.5

# Retention for the materialized per-window active sets (the 24h/7d rollup source).
# Covers the stretched 7d rollup (7d x slack); older windows are pruned.
_ACTIVE_RETENTION_SECONDS = 11 * _DAY

# Missed-capture detection, still used by class activity (its chart is a per-hour
# rate, which a window spanning a missed capture would under-count). Player
# activity no longer skips such windows: a union of active sets stays honest
# however long a window is.
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

# When a backfill is windowed (``since_ts`` given), enumerate anchors only back to
# ``lo - this`` rather than the whole history: enough margin for the warm-up day
# before ``lo`` plus that day's first EARLIER anchor, while partition-pruning skips
# the cold tier so the enumeration doesn't time out.
_BACKFILL_ANCHOR_MARGIN = 2 * 86400


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
    # In the gateway bot (Mongo + Redis, no Postgres) fetch over HTTP instead of
    # querying Postgres directly. The API process (postgres_enabled) computes it.
    from app.core.config import settings
    if not settings.postgres_enabled:
        from app.core.internal_api import internal_get
        data = await internal_get("/v1/activity/current")
        return data if data is not None else {
            "window_start": None, "window_end": None, "duration_hours": None,
            "estimate": None, "estimate_24h": None, "estimate_7d": None,
            "by_board": [], "boards_analyzed": 0, "methodology": "unavailable",
            "computed_at": 0,
        }

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
    """Pick the stored capture that best anchors a "last N seconds" window ending
    at ``anchor_late``.

    ``stamps_desc`` is the full anchor list, newest first. We want the NEWEST anchor
    at-or-before ``anchor_late - seconds_back`` so the window spans at least the
    requested duration. If no anchor is that old yet (early in a deploy, sparse
    history) we fall back to the OLDEST anchor strictly before ``anchor_late`` - the
    widest window the data can honestly support. Returns ``None`` when there's no
    earlier anchor at all (single capture)."""
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
    # New appearances count only ABOVE the previous capture's floor - a shallower
    # (truncated) early dump must not read as a surge of "new" players. See the
    # floor-guard rationale in _board_active_names.
    early_floor = min(early_scores.values())
    active: set[str] = set()
    for name, score in late_scores.items():
        prev = early_scores.get(name)
        if prev is None:
            if score > early_floor:
                active.add(name)
        elif score > prev:
            active.add(name)
    return active


def _board_active_names(
    late_scores: dict[str, float],
    early_scores: dict[str, float],
) -> set[str]:
    """Players active on ONE board between two captures: their score ROSE vs the
    previous capture, OR they newly appear with a score ABOVE the previous
    capture's floor. A reset needs no special-casing - the post-reset score is
    LOWER than the pre-reset one, so it isn't a rise (the player is counted on
    the captures where they actually climb). Pure (no I/O).

    The floor guard on new appearances is what keeps a truncated/partial board
    dump from faking a spike: if the previous capture only reached rank ~10k
    (score floor 872) but this one reaches ~20k (floor 139), the ~10k players in
    the newly-captured tail are NOT new activity - they were simply below the
    previous capture's horizon. Only a newcomer whose score clears that horizon
    (``score > early_floor``) would have been captured last time had they held
    it, so only they are a trustworthy "appeared and is active" signal. This
    costs at most a sliver of genuine tail entrants right at the boundary - fine
    for a metric that's explicitly a lower bound."""
    if not early_scores:
        return set()
    early_floor = min(early_scores.values())
    out: set[str] = set()
    for name, score in late_scores.items():
        prev = early_scores.get(name)
        if prev is None:
            if score > early_floor:
                out.add(name)
        elif score > prev:
            out.add(name)
    return out


def _rollup_measurable(early: int, late: int, period: int) -> bool:
    """A rollup from ``early`` to ``late`` honestly measures ``period`` only when a
    capture gap hasn't stretched it far past that span."""
    return late - early <= period * _ROLLUP_SLACK


async def _rollup(early: int | None, late: int, period: int) -> int | None:
    """Distinct active players over the materialized windows in ``(early, late]``,
    or None when that span isn't measurable (see ``_rollup_measurable``)."""
    if early is None or not _rollup_measurable(early, late, period):
        return None
    from app.trove.leaderboards import pg_store as _pg
    try:
        return await _pg.count_active_since(early, late)
    except Exception:
        logger.exception("activity: %ds rollup failed for late=%d", period, late)
        return None


async def _compute(anchor_late: int, anchor_early: int) -> dict:
    """Iterate EVERY player_board, fetch entries at both anchors, count distinct
    players with a positive activity signal (reset-crossing + lower-bound rules per
    the module docstring).

    Server-tally boards (``player_board=False``, e.g. CLUB POWER RANK) are skipped -
    they aggregate everyone's contributions, not individual activity. Persists to
    ``activity_estimate`` (upsert by ``window_end``) so the graph survives restarts."""
    boards = await lb_service.list_boards_at(anchor_late)
    duration_h = (anchor_late - anchor_early) / 3600.0

    # ``estimate`` = the distinct players who rose (or appeared above the previous
    # floor) since the previous capture. 24h / 7d = the distinct UNION of those
    # per-capture active sets across the period, read from the materialized
    # ``activity_active`` table (each capture stores its own set once - see
    # record_active_window - so a rollup is an indexed COUNT(DISTINCT), not a
    # re-scan of days of entries). Monotonic: 7d ⊇ 24h ⊇ this window.
    from app.trove.leaderboards import pg_store as _pg
    stamps_desc = await lb_service.list_timestamps(limit=500, include_archive=True)
    early_24h = _pick_early_anchor(stamps_desc, anchor_late, _DAY)
    early_7d = _pick_early_anchor(stamps_desc, anchor_late, 7 * _DAY)

    # Server-tally boards aggregate scores across everyone; they don't reflect
    # individual activity. Everything else counts.
    player_boards = [b for b in boards if b.get("player_board", True)]
    board_uuids = [b["uuid"] for b in player_boards]
    meta_by_uuid = {b["uuid"]: b for b in player_boards}

    # Unguarded loads: a failure should make the warmer retry rather than publish
    # a bogus 0.
    late_maps = await _load_anchor_maps(anchor_late, board_uuids)
    early_map = await _load_anchor_maps(anchor_early, board_uuids)

    active_union: set[str] = set()
    per_board: list[dict] = []
    for uuid in board_uuids:
        late = late_maps.get(uuid)
        if not late:
            continue
        s = _board_active_names(late, early_map.get(uuid, {}))
        if s:
            active_union.update(s)
            meta = meta_by_uuid[uuid]
            per_board.append({
                "uuid": uuid,
                "name": meta["name"],
                "category": meta["category"],
                "active_players": len(s),
            })

    per_board.sort(key=lambda b: -b["active_players"])

    now_ts = int(time.time())
    estimate_count = len(active_union)

    # Materialize this window's active set (idempotent), then prune the rolling
    # retention. Non-fatal: a storage hiccup must not abort the live estimate.
    try:
        await _pg.record_active_window(anchor_late, active_union)
        await _pg.prune_active_windows(anchor_late - _ACTIVE_RETENTION_SECONDS)
    except Exception:
        logger.exception("activity: failed to record active window late=%d", anchor_late)

    estimate_24h = await _rollup(early_24h, anchor_late, _DAY)
    estimate_7d = await _rollup(early_7d, anchor_late, 7 * _DAY)

    boards_count = len(per_board)
    # Span (in hours) the rollups actually cover - lets the UI label them honestly
    # ("7d" really means "since the oldest capture" until a full week exists).
    span_24h = round((anchor_late - early_24h) / 3600.0, 1) if early_24h is not None else None
    span_7d = round((anchor_late - early_7d) / 3600.0, 1) if early_7d is not None else None

    # Persist the graph point (upsert by window_end so re-runs converge). Non-fatal.
    try:
        await _pg.upsert_estimate(
            anchor_late, anchor_early, round(duration_h, 2),
            estimate_count, boards_count, now_ts, estimate_24h,
        )
    except Exception:
        logger.exception("activity: failed to persist estimate for window_end=%d", anchor_late)

    return {
        "window_start": anchor_early,
        "window_end": anchor_late,
        "duration_hours": round(duration_h, 2),
        "estimate": estimate_count,
        # Distinct active players over the last 24h / 7d - the union of every
        # capture's active set in the period. None until an earlier capture
        # exists, or when a capture gap stretches the span past measuring it.
        "estimate_24h": estimate_24h,
        "estimate_7d": estimate_7d,
        "window_24h_start": early_24h,
        "window_7d_start": early_7d,
        "span_24h_hours": span_24h,
        "span_7d_hours": span_7d,
        "by_board": per_board,
        "boards_analyzed": boards_count,
        "methodology": _METHODOLOGY,
        "computed_at": now_ts,
    }


_METHODOLOGY = (
    "Distinct top-N leaderboard players whose score rose on at least one board "
    "between two captures (or who newly appear above the previous capture's "
    "floor, so a shallower dump can't fake a surge). A reset isn't counted as "
    "activity (the score drops, it doesn't rise). The 24h / 7d figures are the "
    "distinct UNION of every capture's active set over the period, so they don't "
    "depend on how often captures land; one a capture gap stretches past 1.5x its "
    "period is withheld. `estimate` covers only the latest capture window. Lower "
    "bound: players outside every board's top-N aren't seen."
)


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
) -> tuple[set[str], int]:
    """Distinct active player NAMES + boards-analyzed for one ``(early, late)`` pair
    from the two anchors' pre-loaded score maps. SAME rule as the live compute
    (``_board_active_names``): a player counts on a board if their score rose there
    (or they newly appear above the previous capture's floor). Returns the SET so the backfill can
    both store the count (chart) and materialize the names (rollup table)."""
    active_union: set[str] = set()
    boards_analyzed = 0
    for uuid in boards:
        late = late_maps.get(uuid)
        if not late:
            continue
        early = early_maps.get(uuid)
        if not early:
            continue
        active_union |= _board_active_names(late, early)
        boards_analyzed += 1
    return active_union, boards_analyzed


async def backfill_history(
    *, window_days: int = 7, force: bool = False,
    since_ts: int | None = None, until_ts: int | None = None,
) -> dict:
    """Rebuild stored activity estimates for every consecutive capture pair
    whose LATE anchor falls in the window.

    MEMORY-SAFE by construction: anchors stream in ascending order holding a
    1-deep sliding window (the previous anchor's score maps), because all a
    per-window estimate needs is a consecutive pair. Peak memory is ~one
    capture's entries (tens of MB) plus a day of per-window active-name sets
    (the rolling 24h union), no matter how long the range.

    Window: trailing ``window_days`` by default, or an explicit
    ``since_ts`` / ``until_ts`` slice (bounds on the LATE anchor). The day before
    the window is replayed too, unstored, so its first points get a full 24h."""
    from app.trove.leaderboards import cache as lb_cache
    from app.trove.leaderboards import pg_store
    from app.trove.leaderboards.models import is_player_board

    lo = since_ts if since_ts is not None else int(time.time()) - max(1, window_days) * 86400
    hi = until_ts   # None = no upper bound
    # Enumerate only the anchors this window needs (+ a margin for the warm-up day
    # and the first pair's earlier side). Walking ALL anchors here times out on the
    # cold tier - see pg_store.list_timestamps(since=...). lo<=0 ("all history")
    # keeps the full walk.
    anchor_floor = None if lo <= 0 else max(0, lo - _BACKFILL_ANCHOR_MARGIN)
    stamps = await lb_service.list_timestamps(limit=1_000_000, since=anchor_floor)
    if not stamps:
        return {"computed": 0, "skipped": 0, "failed": 0, "total": 0,
                "note": "no anchors stored"}
    stamps_asc = sorted(stamps)

    # Consecutive (early, late) pairs whose late anchor is in [lo - 1 day, hi].
    pairs: list[tuple[int, int]] = []
    for i in range(1, len(stamps_asc)):
        early, late = stamps_asc[i - 1], stamps_asc[i]
        if late < lo - _DAY:
            continue
        if hi is not None and late > hi:
            continue
        pairs.append((early, late))
    stored = [p for p in pairs if p[1] >= lo]
    if not stored:
        return {"computed": 0, "skipped": 0, "failed": 0, "total": 0,
                "note": "no pairs in window"}

    existing: set[int] = set()
    if not force:
        rows = await pg_store.get_estimates(stored[0][1])
        existing = {r["window_end"] for r in rows}
    todo_stored = [p for p in stored if p[1] not in existing]
    if not todo_stored:
        return {"computed": 0, "skipped": len(stored), "failed": 0,
                "total": len(stored), "note": "all pairs already stored - use force=True to recompute"}
    todo = [p for p in pairs if p[1] < lo] + todo_stored

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
    needed = sorted({a for pr in todo for a in pr})
    early_of = {late: early for early, late in todo}

    median_interval = _median(_intervals_hours(stamps_asc))
    span_days = round((stamps_asc[-1] - stamps_asc[0]) / 86400.0, 1)

    logger.info(
        "activity backfill: %d pairs over %d anchors, %d boards; cadence median=%.2fh; "
        "data spans %.1f days (%d total anchors)",
        len(todo), len(needed), len(boards), median_interval or 0.0, span_days,
        len(stamps_asc),
    )
    started = time.time()
    # Only windows within the rolling retention feed the live 24h/7d rollup table.
    active_floor = int(time.time()) - _ACTIVE_RETENTION_SECONDS
    computed = failed = unmeasured = 0
    prev_anchor: int | None = None
    prev_maps: dict[int, dict[str, float]] | None = None
    # Contiguous run of this replay's windows, oldest first: (early, late, names).
    chain: deque[tuple[int, int, set[str]]] = deque()

    for i, anchor in enumerate(needed):
        cur_maps = await _load_anchor_maps(anchor, board_uuids)
        early = early_of.get(anchor)
        if early is not None:
            try:
                if early == prev_anchor and prev_maps is not None:
                    early_maps = prev_maps
                else:
                    early_maps = await _load_anchor_maps(early, board_uuids)
                active_names, nboards = _pair_estimate(
                    early_maps, cur_maps, boards, early, anchor)
                if anchor >= active_floor:
                    await pg_store.record_active_window(anchor, active_names)
                if chain and chain[-1][1] != early:
                    chain.clear()
                chain.append((early, anchor, active_names))
                est_24h = await _replay_rollup_24h(chain, stamps_asc, anchor, active_floor)
                if anchor >= lo:
                    await pg_store.upsert_estimate(
                        anchor, early, round((anchor - early) / 3600.0, 2),
                        len(active_names), nboards, int(time.time()), est_24h,
                    )
                    computed += 1
                    unmeasured += est_24h is None
            except Exception:
                failed += 1
                chain.clear()
                logger.exception("activity backfill: pair late=%d failed", anchor)
        prev_anchor = anchor
        prev_maps = cur_maps
        if (i + 1) % 20 == 0:
            logger.info("activity backfill: streamed %d/%d anchors, %d computed (%.1fs)",
                        i + 1, len(needed), computed, time.time() - started)

    if computed:
        # The live figures and chart cached before the replay may predate it.
        invalidate_cache()
        await lb_cache.invalidate_all_activity()

    summary = {
        "computed": computed,
        "skipped": len(stored) - len(todo_stored),
        "unmeasured_24h": unmeasured,
        "failed": failed,
        "total": len(stored),
        "anchors": len(stamps_asc),
        "span_days": span_days,
        "median_interval_hours": round(median_interval, 2) if median_interval else None,
        "elapsed_seconds": round(time.time() - started, 2),
    }
    logger.info("activity backfill done: %s", summary)
    return summary


async def _replay_rollup_24h(
    chain: deque, stamps_asc: list[int], anchor: int, active_floor: int,
) -> int | None:
    """The 24h rollup at ``anchor`` during a replay: the union of the in-memory
    ``chain`` when it reaches back to the rollup's start, else the materialized
    table when that span is still retained, else None."""
    j = bisect.bisect_right(stamps_asc, anchor - _DAY) - 1
    early = stamps_asc[j] if j >= 0 else None
    if early is None:
        return None
    while chain and chain[0][1] <= early:
        chain.popleft()
    if not _rollup_measurable(early, anchor, _DAY):
        return None
    if chain and chain[0][0] <= early:
        return len(set().union(*(c[2] for c in chain)))
    if early >= active_floor:
        from app.trove.leaderboards import pg_store
        return await pg_store.count_active_since(early, anchor)
    return None


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
    from app.trove.leaderboards import cache as lb_cache
    from app.trove.leaderboards import pg_store
    deleted = await pg_store.delete_all_estimates()
    active_deleted = await pg_store.delete_all_active_windows()
    invalidate_cache()
    global _LAST_GOOD
    _LAST_GOOD = None
    redis_cleared = await lb_cache.invalidate_all_activity()
    logger.warning("activity: RESET cleared %d stored estimates, %d active-window rows, "
                   "%d redis snapshots", deleted, active_deleted, redis_cleared)
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

    Each row carries ``estimate_24h`` (distinct players active in the 24h up to
    that capture - the value to chart), the raw per-window ``estimate`` and
    ``estimate_per_hour`` (= estimate / duration_hours, kept for older clients;
    it under-counts on long windows, since the same players stay active).

    New rows land each time the warmer fires ``estimate_active_players()``
    (right after each ingest), so the series fills in naturally as captures
    arrive. Empty / one-row history returns an empty series - the consumer just
    doesn't render the chart."""
    from app.trove.leaderboards import pg_store

    days = max(1, min(days, 30))
    now_ts = int(time.time())
    window_start = now_ts - days * 86400

    rows = await pg_store.get_estimates(window_start)

    series = []
    for r in rows:
        duration_h = r["duration_hours"] or 0.0
        per_hour = (r["estimate"] / duration_h) if duration_h > 0 else 0.0
        series.append({
            "window_end": r["window_end"],
            "window_start": r["window_start"],
            "duration_hours": round(duration_h, 2),
            "estimate": r["estimate"],
            "estimate_24h": r.get("estimate_24h"),
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
            "One point per consecutive capture pair. `estimate` is the distinct "
            "top-N leaderboard players whose score rose on at least one board "
            "between the two captures; `estimate_24h` is the distinct union of "
            "those players over the 24 hours up to the capture (null where a "
            "capture gap stretches that span past 36 hours)."
        ),
    }


# Period -> (lookback_days | None for all-time, bucket_seconds | None for
# dynamic). Used by class activity, whose chart still averages a per-hour rate
# per bucket.
_SERIES_PERIODS: dict[str, tuple[int | None, int | None]] = {
    "1d": (1, 3600),            # 24 hourly points
    "7d": (7, 3600),            # ~168 hourly points
    "1m": (30, 86400),          # 30 daily points
    "3m": (90, 86400),          # 90 daily points
    "6m": (180, 2 * 86400),     # 90 two-daily points
    "1y": (365, 7 * 86400),     # ~52 weekly points
    "all": (None, None),        # dynamic bucket, ~120 points
}

# The Player Activity periods: (lookback_days, minimum bucket_seconds). The value
# plotted is the 24h rollup at each capture, so a shorter range than a week has
# too few points to read; 1d was removed when captures stopped being hourly.
_ACTIVITY_PERIODS: dict[str, tuple[int, int]] = {
    "7d": (7, 6 * 3600),        # ~28 points
    "1m": (30, 86400),          # 30 daily points
}


async def activity_series(period: str = "7d") -> dict:
    """Bucketed daily-activity time-series for the Player Activity page.

    Reads the stored per-capture 24h rollups and downsamples them into fixed
    time buckets sized to ``period`` (see ``_ACTIVITY_PERIODS``) - never
    narrower than the capture cadence, so every bucket holds a capture and the
    line doesn't break into gaps. Each bucket reports the AVERAGE and PEAK
    24h count of the captures that fall in it. Also returns the period peak
    (with its timestamp), the period average, and the latest value so the
    page's stat cards don't need a second request.

    Cheap: a single indexed range scan + in-Python bucketing, no per-board
    re-computation. Empty collection -> empty ``points`` (page hides the
    chart). Unknown ``period`` falls back to ``7d``."""
    # Bot process (no Postgres): fetch the bucketed series over HTTP instead.
    from app.core.config import settings
    if not settings.postgres_enabled:
        from app.core.internal_api import internal_get
        p = (period or "7d").lower()
        data = await internal_get("/v1/activity/series", {"period": p})
        return data if data is not None else {
            "period": p, "bucket_seconds": 0, "window_start": 0, "window_end": 0,
            "points": [], "peak": None, "low": None, "average": None, "latest": None,
            "methodology": "unavailable",
        }

    from app.trove.leaderboards import cache as lb_cache
    from app.trove.leaderboards import pg_store

    period = (period or "7d").lower()
    if period not in _ACTIVITY_PERIODS:
        period = "7d"
    # Read-through Redis cache: the chart is identical for every visitor and only
    # shifts when a new capture lands, so a short-TTL cache keeps the Player
    # Activity page off Postgres for the common case.
    cached = await lb_cache.get_activity_series(period)
    if cached is not None:
        return cached
    days, bucket = _ACTIVITY_PERIODS[period]
    now_ts = int(time.time())
    window_start = now_ts - days * 86400
    rows = [r for r in await pg_store.get_estimates(window_start)
            if r.get("estimate_24h") is not None]

    cadence_h = _median([r["duration_hours"] for r in rows])
    if cadence_h:
        bucket = max(bucket, round(cadence_h) * 3600)

    # Group into fixed buckets keyed by bucket-start. Keep the bucket's mean AND
    # its extremes (busiest/quietest capture, each with the exact capture time),
    # so a wide bucket doesn't flatten the real peak (see the points loop below).
    agg: dict[int, dict] = {}
    for r in rows:
        v = float(r["estimate_24h"])
        te = r["window_end"]
        b = (te // bucket) * bucket
        a = agg.get(b)
        if a is None:
            agg[b] = {"sum": v, "n": 1, "t_sum": te,
                      "max": v, "max_t": te, "min": v, "min_t": te}
        else:
            a["sum"] += v
            a["n"] += 1
            a["t_sum"] += te
            if v > a["max"]:
                a["max"], a["max_t"] = v, te
            if v < a["min"]:
                a["min"], a["min_t"] = v, te

    sorted_b = sorted(agg)
    # The single busiest and quietest buckets across the whole range, ranked by
    # their TRUE max/min sample (not the bucket mean) - so a coarse bucket that
    # happens to hold the period's peak is the one we let spike to it.
    peak_b = max(sorted_b, key=lambda b: agg[b]["max"]) if sorted_b else None
    low_b = min(sorted_b, key=lambda b: agg[b]["min"]) if sorted_b else None

    points: list[dict] = []
    avg_running = 0.0
    for b in sorted_b:
        a = agg[b]
        avg = a["sum"] / a["n"]
        avg_running += avg
        # "Max for the highest point, min for the lowest, average for the rest":
        # the peak bucket plots its true max at the exact peak capture, the low
        # bucket its true min, every other bucket its mean at the capture centroid.
        if b == peak_b:
            active, t = round(a["max"], 1), a["max_t"]
        elif b == low_b:
            active, t = round(a["min"], 1), a["min_t"]
        else:
            active, t = round(avg, 1), round(a["t_sum"] / a["n"])
        points.append({
            "t": t,
            "active": active,                # plotted value (extreme-preserving)
            "avg": round(avg, 1),            # bucket mean (unsubstituted)
            "peak": round(a["max"], 1),      # busiest capture in the bucket
            "samples": a["n"],
        })

    # Peak / quietest stat cards read the TRUE extremes over the range, timestamped
    # at the actual capture - not the highest/lowest bucket average.
    peak = ({"t": agg[peak_b]["max_t"], "active": round(agg[peak_b]["max"], 1)}
            if peak_b is not None else None)
    low = ({"t": agg[low_b]["min_t"], "active": round(agg[low_b]["min"], 1)}
           if low_b is not None else None)
    average = round(avg_running / len(points), 1) if points else None
    latest = float(rows[-1]["estimate_24h"]) if rows else None

    payload = {
        "period": period,
        "bucket_seconds": bucket,
        "window_start": window_start,
        "window_end": now_ts,
        "points": points,
        "peak": peak,
        "low": low,
        "average": average,
        "latest": latest,
        "methodology": (
            "Distinct players active in the 24 hours up to each capture, from the "
            "stored per-capture rollups, averaged per time bucket. The busiest and "
            "quietest captures in the range are plotted at their true values so a "
            "bucket never flattens the real peak. Buckets are never narrower than "
            "the capture cadence; a capture whose 24h span a gap stretched past 36 "
            "hours is left out."
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

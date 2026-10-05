"""Per-CLASS active-player estimate, mirroring ``activity.py`` but grouped by
Trove class instead of unioned across all boards.

Each class has an Effort leaderboard (``4000+i``), so ``class_index = board_uuid
% 1000`` (see ``stats.class_index_for_board``). A class is "active" in a capture
window when a player's score rose (or first appeared) on its Effort board. Effort
boards reset weekly (Mon 11:00 UTC), so a window that crosses the reset is
unmeasurable (``activity._active_set`` returns ``None``) and contributes no row.

Like player activity, the chart value is DAILY: the distinct players active on a
class in the 24 hours up to each capture (``estimate_24h``), the union of every
capture window in that span - so it doesn't depend on how often captures land.
A span holding an unmeasurable window has no 24h value (the line gaps).

We reuse activity.py's primitives (the score-delta rule, the per-anchor map
loader, the rollup slack) and only change the grouping. The estimate is a LOWER
BOUND on players-per-class (same caveats as player activity), and the "share" is
share-of-class-activity: a player who plays N classes counts in each, so shares
sum to 100% but are not distinct players.
"""
from __future__ import annotations

import bisect
import logging
import time
from collections import deque

from app.trove import stats
from app.trove.leaderboards import activity as _act
from app.trove.leaderboards import service as lb_service

logger = logging.getLogger(__name__)

# Effort + Paragon boards are weekly-reset. (They're the fixed weekly set in
# models.py; we pass "weekly" to _active_set rather than per-board lookups.)
_KIND = "weekly"

# The XP stats board - the clean view's third floor (see _clean_thresholds):
# a player counts toward the clean view only when their score here clears the
# configured threshold at the window end. Global (not per-class).
_XP_BOARD = 21005

# Last good /current payload, kept across anchors so a fresh-but-uncomputed or
# reset-crossing latest window still serves the previous result. Mirrors activity.
_LAST_GOOD: dict | None = None
# Same idea for the donut, which is computed directly from the latest snapshot
# (see class_activity_current); a transient empty capture still serves the last good.
_LAST_GOOD_DONUT: dict | None = None

# A window this short that crosses the weekly reset (the bot captures again at
# it) counts as measured with no Effort activity, rather than unmeasurable: losing
# that stretch barely moves a 24h union, and the day after the reset keeps its
# value. Also covers the old hourly captures' reset hour and a late catch-up.
_SHORT_RESET_WINDOW_SECONDS = 2 * 3600

# Retention for the per-class active sets: enough for the stretched 24h rollup.
_ACTIVE_RETENTION_SECONDS = 3 * _act._DAY

# Period -> (lookback_days | None for all-time, minimum bucket_seconds | None for
# dynamic). The value plotted is each capture's 24h count, so there's no 1d range;
# buckets also widen to the capture cadence so every bucket holds a capture.
_SERIES_PERIODS: dict[str, tuple[int | None, int | None]] = {
    "7d": (7, 6 * 3600),        # ~28 points
    "1m": (30, 86400),          # 30 daily points
    "3m": (90, 86400),          # 90 daily points
    "6m": (180, 2 * 86400),     # 90 two-daily points
    "1y": (365, 7 * 86400),     # ~52 weekly points
    "all": (None, None),        # dynamic bucket, ~120 points
}

_METHODOLOGY = (
    "Per class, distinct top-N players whose score rose (or who first appear) on "
    "its Effort board, counted over the 24 hours up to each capture (the union of "
    "every capture window in that span). (Paragon boards are "
    "excluded as ambiguous - counts are Effort-only.) The default 'clean' "
    "(established) view keeps only players who, snapshot at the window end, clear "
    "every configured floor - Power Rank (1000+i board), Effort (4000+i) and XP "
    "(board 21005) - filtering new characters and throwaway alts; the 'All' view "
    "counts everyone. "
    "Effort boards reset weekly (Mon 11:00 UTC); a window crossing the reset is "
    "unmeasurable, so the 24 hours after it have no point. 'Share' is share of "
    "class activity (a "
    "player active on several classes counts in each), not distinct players. Lower "
    "bound: players outside a board's top-N aren't seen."
)

# The donut is computed differently from the series above: it's a direct headcount
# of the LATEST snapshot, with no activity (score-rose) condition at all.
_DONUT_METHODOLOGY = (
    "Per-class player share from the latest leaderboard snapshot - a direct "
    "headcount, NOT the activity pipeline. RAW counts the players present on a "
    "class's Effort board at the most recent capture (Paragon is excluded as "
    "ambiguous); the 'established' view keeps only those clearing every floor - "
    "Power Rank (1000+i), Effort (4000+i) and XP (board 21005). 'Share' is a "
    "class's count divided by "
    "the total across classes (a player on several classes counts in each), so "
    "shares sum to 100% but aren't distinct players. Each class also carries the "
    "Effort ADDED in the last 24 hours (this capture vs the newest one at least 24h "
    "earlier) - the sum of positive score gains over players on its Effort board, "
    "per view; skipped when a weekly reset falls in that span. Lower bound: players "
    "outside a board's top-N aren't seen."
)


async def _setting_int(key: str, fallback: int) -> int:
    """A runtime-tunable int setting (master config), falling back to the static
    default if the config lookup fails - a config hiccup must never break compute."""
    from app.admin import runtime_config
    try:
        return int(await runtime_config.get_setting(key))
    except Exception:  # noqa: BLE001
        return int(fallback)


async def _power_rank_threshold() -> int:
    from app.core.config import settings
    return await _setting_int("class_activity_power_rank_threshold",
                              settings.class_activity_power_rank_threshold)


async def _effort_threshold() -> int:
    from app.core.config import settings
    return await _setting_int("class_activity_effort_threshold",
                              settings.class_activity_effort_threshold)


async def _xp_threshold() -> int:
    from app.core.config import settings
    return await _setting_int("class_activity_xp_threshold",
                              settings.class_activity_xp_threshold)


async def _clean_thresholds() -> tuple[int, int, int]:
    """The three runtime-tunable "established"-player floors
    ``(power_rank, effort, xp)`` - a player counts toward a class's clean estimate
    only when it clears ALL of them (each is skipped at 0; XP reads the global
    ``_XP_BOARD`` snapshot at the window end)."""
    return (await _power_rank_threshold(), await _effort_threshold(),
            await _xp_threshold())


def _class_active_sets(
    early_maps: dict[int, dict[str, float]],
    late_maps: dict[int, dict[str, float]],
    early_ts: int,
    late_ts: int,
    *,
    pr_maps: dict[int, dict[str, float]] | None = None,
    threshold: float = 0,
    effort_threshold: float = 0,
    xp_map: dict[str, float] | None = None,
    xp_threshold: float = 0,
) -> dict[int, tuple[set[str], set[str] | None]]:
    """``{class_index: (active, clean_active | None)}`` for one (early, late) pair.

    ``clean_active`` gates the active set on three floors at the LATE anchor -
    Power Rank (``1000+i`` from ``pr_maps``), Effort (``4000+i`` from
    ``late_maps``) and, when ``xp_threshold > 0``, XP (the global board
    ``_XP_BOARD`` via ``xp_map``). A floor of 0 is a no-op, a player missing from a
    board reads as 0 (filtered). It is ``None`` (unmeasurable, stored as NULL so the
    clean line gaps) when no Power Rank snapshot is available (``pr_maps`` None, or
    that board absent at the anchor).

    A class is OMITTED (no key) when its Effort board isn't measurable for the window
    (reset crossed / no early snapshot) so the caller stores nothing and the series
    gaps; a measurable-but-quiet class keeps an empty set. A reset-crossing window
    no longer than ``_SHORT_RESET_WINDOW_SECONDS`` is measurable and empty."""
    short_reset = (late_ts - early_ts <= _SHORT_RESET_WINDOW_SECONDS
                   and lb_service.reset_boundaries_for_kind(_KIND, early_ts, late_ts))
    by_class: dict[int, set[str]] = {}
    for uuid, late in late_maps.items():
        if not late:
            continue
        if short_reset:
            if early_maps.get(uuid):
                by_class.setdefault(stats.class_index_for_board(uuid), set())
            continue
        s = _act._active_set(late, early_maps.get(uuid, {}), _KIND, early_ts, late_ts)
        if s is None:
            continue  # reset crossed or no early data for this board
        by_class.setdefault(stats.class_index_for_board(uuid), set()).update(s)

    # An ABSENT XP board (not captured) degrades the XP floor to a no-op rather
    # than zeroing the clean view (with the board missing, every player would
    # read 0 and fail the floor). A player merely missing FROM a captured board
    # reads as 0 and is filtered, same as the other floors.
    xp_on = xp_threshold > 0 and xp_map is not None
    out: dict[int, tuple[set[str], set[str] | None]] = {}
    for i, players in by_class.items():
        clean: set[str] | None = None
        if pr_maps is not None:
            pr = pr_maps.get(stats.class_pr_board_uuid(i))
            if pr is not None:
                effort = late_maps.get(stats.class_effort_board_uuid(i), {})
                clean = {
                    p for p in players
                    if pr.get(p, 0.0) >= threshold
                    and effort.get(p, 0.0) >= effort_threshold
                    and (not xp_on or xp_map.get(p, 0.0) >= xp_threshold)
                }
        out[i] = (players, clean)
    return out


def _counts_of(sets: dict[int, tuple[set[str], set[str] | None]]) -> dict[int, dict]:
    """``{class_index: {"raw": int, "clean": int|None}}`` - the set sizes."""
    return {i: {"raw": len(s), "clean": len(c) if c is not None else None}
            for i, (s, c) in sets.items()}


def _class_counts(early_maps, late_maps, early_ts, late_ts, **gates) -> dict[int, dict]:
    """``_counts_of(_class_active_sets(...))`` (same arguments)."""
    return _counts_of(_class_active_sets(early_maps, late_maps, early_ts, late_ts, **gates))


def _rows(early: int, late: int, counts: dict[int, dict], rollups: dict[int, tuple],
          now_ts: int) -> list[dict]:
    """One window's ``class_activity_estimate`` rows (measurable classes only)."""
    return [
        {"class_index": i, "window_end": late, "window_start": early,
         "duration_hours": round((late - early) / 3600.0, 2), "estimate": c["raw"],
         "estimate_clean": c["clean"], "computed_at": now_ts,
         "estimate_24h": rollups.get(i, (None, None))[0],
         "estimate_24h_clean": rollups.get(i, (None, None))[1]}
        for i, c in counts.items()
    ]


async def _stored_rollups_24h(stamps: list[int], late: int) -> dict[int, tuple]:
    """``{class_index: (raw_24h, clean_24h)}`` at ``late`` from the materialized
    per-class sets. A class gets a value only when every window in the span was
    measurable for it (and the clean view only when every one had a clean count);
    nothing when a capture gap stretched the span past the rollup slack."""
    from app.trove.leaderboards import pg_store
    asc = sorted(stamps)
    j = bisect.bisect_right(asc, late - _act._DAY) - 1
    early = asc[j] if j >= 0 else None
    if early is None or not _act._rollup_measurable(early, late, _act._DAY):
        return {}
    n = sum(1 for s in stamps if early < s <= late)
    out: dict[int, tuple] = {}
    for i, r in (await pg_store.class_rollup_since(early, late)).items():
        if r["windows"] >= n:
            out[i] = (r["raw"], r["clean"] if r["clean_windows"] >= n else None)
    return out


# ─── warmer + /current ──────────────────────────────────────────────────────


async def estimate_class_activity(*, force: bool = False) -> dict:
    """Compute + persist the latest capture pair's per-class rows (warmer entry),
    with each class's 24h rollup. Idempotent upsert; returns the /current-shaped
    payload. ``force`` is accepted for symmetry with activity.py but we always
    recompute the latest pair (cheap: one 18-board load each side)."""
    global _LAST_GOOD
    from app.trove.leaderboards import pg_store
    # Floored at the stretched 24h rollup: an unbounded walk reaches the cold tier.
    stamps_desc = await lb_service.list_timestamps(
        limit=500, since=int(time.time()) - _ACTIVE_RETENTION_SECONDS)
    if len(stamps_desc) < 2:
        return _LAST_GOOD or _empty()
    anchor_late, anchor_early = stamps_desc[0], stamps_desc[1]
    if anchor_late <= anchor_early:
        return _LAST_GOOD or _empty()
    duration_h = (anchor_late - anchor_early) / 3600.0

    pr_thr, effort_thr, xp_thr = await _clean_thresholds()
    board_uuids = stats.class_effort_board_uuids()   # Effort only (Paragon excluded)
    late_maps = await _act._load_anchor_maps(anchor_late, board_uuids)
    early_maps = await _act._load_anchor_maps(anchor_early, board_uuids)
    # Power Rank (+ XP, when that floor is on) snapshot at the window END gates
    # the clean view (one extra query; Effort scores come from late_maps above).
    gate_uuids = stats.class_pr_board_uuids() + ([_XP_BOARD] if xp_thr > 0 else [])
    pr_maps = await _act._load_anchor_maps(anchor_late, gate_uuids)
    sets = _class_active_sets(early_maps, late_maps, anchor_early, anchor_late,
                              pr_maps=pr_maps, threshold=pr_thr,
                              effort_threshold=effort_thr,
                              xp_map=pr_maps.get(_XP_BOARD), xp_threshold=xp_thr)
    counts = _counts_of(sets)

    now_ts = int(time.time())
    if counts:
        try:
            # The window's own row must exist before its 24h rollup can count it
            # as measurable; the rollup then rewrites the same rows.
            await pg_store.record_class_active_window(anchor_late, sets)
            await pg_store.prune_class_active_windows(anchor_late - _ACTIVE_RETENTION_SECONDS)
            await pg_store.upsert_class_estimates(
                _rows(anchor_early, anchor_late, counts, {}, now_ts))
            rollups = await _stored_rollups_24h(stamps_desc, anchor_late)
            await pg_store.upsert_class_estimates(
                _rows(anchor_early, anchor_late, counts, rollups, now_ts))
        except Exception:
            logger.exception("class activity: persist failed for window_end=%d", anchor_late)

    payload = _build_current(anchor_early, anchor_late, duration_h, counts, now_ts,
                             pr_thr, effort_thr, xp_thr)
    if counts:
        _LAST_GOOD = payload
    return payload


def _snapshot_counts(
    effort_maps: dict[int, dict[str, float]],
    pr_maps: dict[int, dict[str, float]],
    pr_threshold: float, effort_threshold: float,
    xp_map: dict[str, float] | None = None, xp_threshold: float = 0,
) -> dict[int, dict]:
    """``{class_index: {"raw", "clean"}}`` from ONE snapshot's presence - no
    activity (score-rose) condition, unlike ``_class_counts``. ``clean`` is None if
    the class's Power Rank board is absent; classes with no players are omitted.
    Same three clean floors as ``_class_counts`` (incl. the absent-XP-board no-op)."""
    xp_on = xp_threshold > 0 and xp_map is not None
    out: dict[int, dict] = {}
    for i in range(stats.class_count()):
        effort = effort_maps.get(stats.class_effort_board_uuid(i), {})
        players = set(effort)
        if not players:
            continue
        clean: int | None = None
        pr = pr_maps.get(stats.class_pr_board_uuid(i))
        if pr is not None:
            clean = sum(
                1 for p in players
                if pr.get(p, 0.0) >= pr_threshold
                and effort.get(p, 0.0) >= effort_threshold
                and (not xp_on or xp_map.get(p, 0.0) >= xp_threshold)
            )
        out[i] = {"raw": len(players), "clean": clean}
    return out


def _effort_deltas(
    effort_late: dict[int, dict[str, float]],
    effort_early: dict[int, dict[str, float]],
    pr_maps: dict[int, dict[str, float]],
    pr_threshold: float, effort_threshold: float,
    xp_map: dict[str, float] | None = None, xp_threshold: float = 0,
) -> dict[int, dict]:
    """Per-class Effort ADDED between two snapshots: Σ max(0, late - early)
    over players on the class's Effort board in BOTH snapshots. New entrants are
    excluded - their gain is unmeasurable on a weekly-accumulating board.
    ``clean`` gates on the Power-Rank + Effort + XP floors (None if the PR board
    is absent)."""
    xp_on = xp_threshold > 0 and xp_map is not None
    out: dict[int, dict] = {}
    for i in range(stats.class_count()):
        late = effort_late.get(stats.class_effort_board_uuid(i), {})
        if not late:
            continue
        early = effort_early.get(stats.class_effort_board_uuid(i), {})
        pr = pr_maps.get(stats.class_pr_board_uuid(i))
        raw = 0.0
        clean = 0.0 if pr is not None else None
        for p, lv in late.items():
            ev = early.get(p)
            if ev is None:
                continue                       # new entrant - gain unknown
            gain = lv - ev
            if gain <= 0:
                continue
            raw += gain
            if (clean is not None
                    and pr.get(p, 0.0) >= pr_threshold and lv >= effort_threshold
                    and (not xp_on or xp_map.get(p, 0.0) >= xp_threshold)):
                clean += gain
        out[i] = {"raw": int(round(raw)),
                  "clean": int(round(clean)) if clean is not None else None}
    return out


async def class_activity_current() -> dict:
    """Per-class player share for the DONUT, computed DIRECTLY from the latest
    leaderboard snapshot - a real headcount, with NO activity (score-rose) step
    (see ``_DONUT_METHODOLOGY``). Read-through Redis cache (the short series TTL);
    falls back to last good / an empty shell."""
    global _LAST_GOOD_DONUT
    from app.trove.leaderboards import cache as lb_cache
    cached = await lb_cache.get_class_activity_current()
    if cached is not None:
        return cached

    stamps = await lb_service.list_timestamps(
        limit=500, since=int(time.time()) - _ACTIVE_RETENTION_SECONDS)
    if not stamps:
        return _LAST_GOOD_DONUT or _empty()
    anchor = stamps[0]
    pr_thr, effort_thr, xp_thr = await _clean_thresholds()
    effort_boards = stats.class_effort_board_uuids()
    effort_maps = await _act._load_anchor_maps(anchor, effort_boards)
    gate_uuids = stats.class_pr_board_uuids() + ([_XP_BOARD] if xp_thr > 0 else [])
    pr_maps = await _act._load_anchor_maps(anchor, gate_uuids)
    xp_map = pr_maps.get(_XP_BOARD)
    counts = _snapshot_counts(effort_maps, pr_maps, pr_thr, effort_thr, xp_map, xp_thr)
    if not counts:
        return _LAST_GOOD_DONUT or _empty()

    # Effort added in the last 24 hours (this capture vs the newest one at least
    # 24h earlier), per view. Skip when a gap stretches that span past the rollup
    # slack, or a weekly reset falls inside it (scores zeroed → not "added").
    asc = sorted(stamps)
    j = bisect.bisect_right(asc, anchor - _act._DAY) - 1
    early = asc[j] if j >= 0 else None
    if (early is not None and _act._rollup_measurable(early, anchor, _act._DAY)
            and not lb_service.reset_boundaries_for_kind("weekly", early, anchor)):
        early_maps = await _act._load_anchor_maps(early, effort_boards)
        for i, d in _effort_deltas(effort_maps, early_maps, pr_maps, pr_thr, effort_thr,
                                   xp_map, xp_thr).items():
            if i in counts:
                counts[i]["effort_raw"] = d["raw"]
                counts[i]["effort_clean"] = d["clean"]

    payload = _build_current(anchor, anchor, None, counts, int(time.time()),
                             pr_thr, effort_thr, xp_thr, methodology=_DONUT_METHODOLOGY)
    _LAST_GOOD_DONUT = payload
    await lb_cache.set_class_activity_current(payload)
    return payload


def _build_current(window_start, window_end, duration_h, counts: dict[int, dict],
                   computed_at: int, threshold: int,
                   effort_threshold: int = 0, xp_threshold: int = 0,
                   methodology: str = _METHODOLOGY) -> dict:
    raw_total = sum(c["raw"] for c in counts.values())
    clean_present = [c["clean"] for c in counts.values() if c["clean"] is not None]
    clean_total = sum(clean_present) if clean_present else None
    # Effort added in the last 24h (donut only; absent on the activity-warmer payload).
    eff_raw = [c.get("effort_raw") for c in counts.values() if c.get("effort_raw") is not None]
    eff_clean = [c.get("effort_clean") for c in counts.values() if c.get("effort_clean") is not None]
    total_effort_added = sum(eff_raw) if eff_raw else None
    total_effort_added_clean = sum(eff_clean) if eff_clean else None
    classes = []
    for i, c in counts.items():
        raw, clean = c["raw"], c["clean"]
        classes.append({
            "class_index": i, "name": stats.class_name(i), "icon": stats.class_icon(i),
            "active_players": raw,
            "share": round(raw / raw_total, 4) if raw_total else 0.0,
            "active_players_clean": clean,
            "share_clean": (round(clean / clean_total, 4)
                            if (clean is not None and clean_total)
                            else (0.0 if clean is not None else None)),
            "effort_added": c.get("effort_raw"),
            "effort_added_clean": c.get("effort_clean"),
        })
    # Default page view is "clean": order by clean desc (classes with a clean
    # value first, None last), then by raw - so the donut/legend match the view.
    classes.sort(key=lambda c: (
        0 if c["active_players_clean"] is not None else 1,
        -(c["active_players_clean"] or 0),
        -c["active_players"], c["class_index"],
    ))
    return {
        "window_start": window_start,
        "window_end": window_end,
        "duration_hours": round(duration_h, 2) if duration_h is not None else None,
        "total_active": raw_total if counts else None,
        "total_active_clean": clean_total,
        "total_effort_added": total_effort_added,
        "total_effort_added_clean": total_effort_added_clean,
        "power_rank_threshold": threshold,
        "effort_threshold": effort_threshold,
        "xp_threshold": xp_threshold,
        "classes": classes,
        "methodology": methodology,
        "computed_at": computed_at,
    }


def _empty() -> dict:
    return {
        "window_start": None, "window_end": None, "duration_hours": None,
        "total_active": None, "total_active_clean": None,
        "total_effort_added": None, "total_effort_added_clean": None,
        "power_rank_threshold": 0, "effort_threshold": 0, "xp_threshold": 0,
        "classes": [], "methodology": _METHODOLOGY,
        "computed_at": int(time.time()),
    }


def reset_caches() -> None:
    global _LAST_GOOD, _LAST_GOOD_DONUT
    _LAST_GOOD = None
    _LAST_GOOD_DONUT = None


# ─── backfill ─────────────────────────────────────────────────────────────────


async def backfill_class_history(
    *, force: bool = False, since_ts: int | None = None, until_ts: int | None = None,
    window_days: int = 7,
) -> dict:
    """Rebuild per-class estimates (incl. the 24h rollups the chart plots) for every
    consecutive capture pair in the window. Streaming, 1-deep sliding window plus a
    day of per-class active sets for the rolling 24h union. The day before the
    window is replayed too, unstored, so its first points get a full 24h. Mirrors
    ``activity.backfill_history`` but grouped per class."""
    from app.trove.leaderboards import cache as lb_cache
    from app.trove.leaderboards import pg_store

    lo = since_ts if since_ts is not None else int(time.time()) - max(1, window_days) * 86400
    hi = until_ts
    # Enumerate only the window's anchors (+ margin) - a full-history walk times out
    # on the cold tier (see pg_store.list_timestamps). lo<=0 keeps the full walk.
    anchor_floor = None if lo <= 0 else max(0, lo - _act._BACKFILL_ANCHOR_MARGIN)
    stamps = await lb_service.list_timestamps(limit=1_000_000, since=anchor_floor)
    if not stamps:
        return {"computed": 0, "skipped": 0, "failed": 0, "total": 0,
                "note": "no anchors stored"}
    stamps_asc = sorted(stamps)

    pairs = [
        (stamps_asc[i - 1], stamps_asc[i])
        for i in range(1, len(stamps_asc))
        if stamps_asc[i] >= lo - _act._DAY and (hi is None or stamps_asc[i] <= hi)
    ]
    stored = [p for p in pairs if p[1] >= lo]
    if not stored:
        return {"computed": 0, "skipped": 0, "failed": 0, "total": 0,
                "note": "no pairs in window"}

    existing: set[int] = set()
    if not force:
        existing = {r["window_end"] for r in await pg_store.get_class_estimates(stored[0][1])}
    todo_stored = [p for p in stored if p[1] not in existing]
    if not todo_stored:
        return {"computed": 0, "skipped": len(stored), "failed": 0,
                "total": len(stored), "note": "all pairs already stored - use force=True"}
    todo = [p for p in pairs if p[1] < lo] + todo_stored

    board_uuids = stats.class_effort_board_uuids()   # Effort only (Paragon excluded)
    pr_thr, effort_thr, xp_thr = await _clean_thresholds()
    pr_board_uuids = stats.class_pr_board_uuids() \
        + ([_XP_BOARD] if xp_thr > 0 else [])
    needed = sorted({a for pr in todo for a in pr})
    early_of = {late: early for early, late in todo}

    span_days = round((stamps_asc[-1] - stamps_asc[0]) / 86400.0, 1)
    logger.info(
        "class activity backfill: %d pairs over %d anchors, %d classes; spans %.1f days",
        len(todo), len(needed), stats.class_count(), span_days,
    )

    started = time.time()
    active_floor = int(time.time()) - _ACTIVE_RETENTION_SECONDS
    computed = failed = empty_skipped = 0
    prev_anchor: int | None = None
    prev_maps: dict[int, dict[str, float]] | None = None
    # Contiguous run of this replay's windows, oldest first: (early, late, sets).
    chain: deque[tuple[int, int, dict]] = deque()

    for idx, anchor in enumerate(needed):
        cur_maps = await _act._load_anchor_maps(anchor, board_uuids)
        early = early_of.get(anchor)
        if early is not None:
            try:
                early_maps = prev_maps if (early == prev_anchor and prev_maps is not None) \
                    else await _act._load_anchor_maps(early, board_uuids)
                # Power Rank (+ XP, when that floor is on) snapshot at the
                # window END gates the clean view.
                pr_maps = await _act._load_anchor_maps(anchor, pr_board_uuids)
                sets = _class_active_sets(early_maps, cur_maps, early, anchor,
                                          pr_maps=pr_maps, threshold=pr_thr,
                                          effort_threshold=effort_thr,
                                          xp_map=pr_maps.get(_XP_BOARD),
                                          xp_threshold=xp_thr)
                if anchor >= active_floor:
                    await pg_store.record_class_active_window(anchor, sets)
                if chain and chain[-1][1] != early:
                    chain.clear()
                chain.append((early, anchor, sets))
                if anchor >= lo:
                    counts = _counts_of(sets)
                    if counts:
                        # Row first: the table fallback counts it as measurable.
                        await pg_store.upsert_class_estimates(
                            _rows(early, anchor, counts, {}, int(time.time())))
                        rollups = await _replay_rollups_24h(
                            chain, stamps_asc, anchor, active_floor)
                        if rollups:
                            await pg_store.upsert_class_estimates(
                                _rows(early, anchor, counts, rollups, int(time.time())))
                        computed += 1
                    else:
                        # reset-crossing window: purge any stale rows, store nothing
                        await pg_store.delete_class_estimate(anchor)
                        empty_skipped += 1
            except Exception:
                failed += 1
                chain.clear()
                logger.exception("class backfill: pair late=%d failed", anchor)
        prev_anchor = anchor
        prev_maps = cur_maps
        if (idx + 1) % 20 == 0:
            logger.info("class activity backfill: %d/%d anchors, %d computed (%.1fs)",
                        idx + 1, len(needed), computed, time.time() - started)

    if computed:
        await lb_cache.invalidate_all_activity()   # sweeps the class series cache too

    summary = {
        "computed": computed,
        "skipped": len(stored) - len(todo_stored),
        "empty_skipped": empty_skipped,
        "failed": failed,
        "total": len(stored),
        "anchors": len(stamps_asc),
        "span_days": span_days,
        "elapsed_seconds": round(time.time() - started, 2),
    }
    logger.info("class activity backfill done: %s", summary)
    return summary


async def _replay_rollups_24h(
    chain: deque, stamps_asc: list[int], anchor: int, active_floor: int,
) -> dict[int, tuple]:
    """Per-class 24h rollups at ``anchor`` during a replay: unions of the in-memory
    ``chain`` when it reaches back to the rollup's start (a class only where every
    window in it was measurable for that class), else the materialized sets when
    that span is still retained, else nothing."""
    j = bisect.bisect_right(stamps_asc, anchor - _act._DAY) - 1
    early = stamps_asc[j] if j >= 0 else None
    if early is None:
        return {}
    while chain and chain[0][1] <= early:
        chain.popleft()
    if not _act._rollup_measurable(early, anchor, _act._DAY):
        return {}
    if not (chain and chain[0][0] <= early):
        if early >= active_floor:
            return await _stored_rollups_24h(stamps_asc, anchor)
        return {}
    out: dict[int, tuple] = {}
    for i in chain[-1][2]:
        if not all(i in w[2] for w in chain):
            continue
        raw = len(set().union(*(w[2][i][0] for w in chain)))
        cleans = [w[2][i][1] for w in chain]
        clean = (len(set().union(*cleans))
                 if all(c is not None for c in cleans) else None)
        out[i] = (raw, clean)
    return out


async def reset_class_estimates() -> int:
    from app.trove.leaderboards import cache as lb_cache
    from app.trove.leaderboards import pg_store
    deleted = await pg_store.delete_all_class_estimates()
    await pg_store.delete_all_class_active_windows()
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


def _peak_preserve(avgs: list[float | None],
                   maxes: list[float | None]) -> list[float | None]:
    """Return ``avgs`` with the single highest bucket replaced by that bucket's
    TRUE max (from the high-resolution samples), so a coarse wide-timeframe bucket
    shows the real peak instead of a flattened average - matching the activity
    page's "max for the highest point, average for the rest" rule. Gap buckets
    (``None``) are left untouched; short periods (one capture per bucket) are
    unaffected since there max == avg.

    Only the peak is preserved (no min): the multi-line class chart has no
    "quietest" stat, and per-class downward spikes would just add noise. This does
    mildly inflate that one class's denominator in the "share" view at its peak
    bucket, but peaks fall on different buckets per class so the effect is diffuse
    and share is a secondary, normalized view."""
    peak_i = -1
    for i, m in enumerate(maxes):
        if m is None:
            continue
        if peak_i < 0 or m > maxes[peak_i]:
            peak_i = i
    if peak_i < 0:
        return avgs
    out = list(avgs)
    out[peak_i] = round(maxes[peak_i], 1)
    return out


async def class_activity_series(period: str = "7d") -> dict:
    """Per-class bucketed series for the Class Activity page. Shared x-axis
    (``buckets``) + per-class ``values`` (the average 24h active-player count of
    the captures in each bucket, null when a class had no measurable 24h there),
    so the page draws one aligned line per class. Unknown periods (incl. the
    removed ``1d``) fall back to ``7d``. Read-through Redis cache (short TTL)."""
    from app.trove.leaderboards import cache as lb_cache
    from app.trove.leaderboards import pg_store

    period = (period or "7d").lower()
    if period not in _SERIES_PERIODS:
        period = "7d"
    cached = await lb_cache.get_class_activity_series(period)
    if cached is not None:
        return cached

    days, bucket = _SERIES_PERIODS[period]
    now_ts = int(time.time())
    if days is not None:
        window_start = now_ts - days * 86400
        rows = await pg_store.get_class_estimates(window_start)
    else:
        rows = await pg_store.get_class_estimates(None)
        window_start = rows[0]["window_end"] if rows else now_ts
    rows = [r for r in rows if r.get("estimate_24h") is not None]
    if bucket is None:
        span = max(3600, now_ts - window_start)
        bucket = max(3600, (int(span / 120) // 3600) * 3600)
    cadence_h = _act._median(list({r["window_end"]: r["duration_hours"] for r in rows}.values()))
    if cadence_h:
        bucket = max(bucket, round(cadence_h) * 3600)

    # bucket -> {t_sum, t_n, classes: {i: {raw_sum, raw_n, raw_max, clean_sum,
    # clean_n, clean_max}}}. Raw + clean (Power-Rank-filtered) 24h counts are
    # averaged independently; clean_n only counts rows that HAD a clean value (NULL
    # = unmeasurable, so the clean line gaps there even when the raw line has a
    # point). ``raw_max``/``clean_max`` keep each bucket's busiest capture so the
    # wide-timeframe line can spike to the true peak instead of the mean.
    agg: dict[int, dict] = {}
    for r in rows:
        raw_v = float(r["estimate_24h"])
        b = (r["window_end"] // bucket) * bucket
        bd = agg.get(b)
        if bd is None:
            bd = agg[b] = {"t_sum": 0, "t_n": 0, "classes": {}}
        bd["t_sum"] += r["window_end"]
        bd["t_n"] += 1
        c = bd["classes"].get(r["class_index"])
        if c is None:
            c = bd["classes"][r["class_index"]] = {
                "raw_sum": 0.0, "raw_n": 0, "raw_max": 0.0,
                "clean_sum": 0.0, "clean_n": 0, "clean_max": 0.0,
            }
        c["raw_sum"] += raw_v
        c["raw_n"] += 1
        if raw_v > c["raw_max"]:
            c["raw_max"] = raw_v
        cl = r.get("estimate_24h_clean")
        if cl is not None:
            clean_v = float(cl)
            c["clean_sum"] += clean_v
            c["clean_n"] += 1
            if clean_v > c["clean_max"]:
                c["clean_max"] = clean_v

    sorted_b = sorted(agg)
    buckets = [round(agg[b]["t_sum"] / agg[b]["t_n"]) for b in sorted_b]
    classes = []
    for i in range(stats.class_count()):
        raw_avgs, raw_maxes, clean_avgs, clean_maxes = [], [], [], []
        for b in sorted_b:
            c = agg[b]["classes"].get(i)
            if c and c["raw_n"]:
                raw_avgs.append(round(c["raw_sum"] / c["raw_n"], 1))
                raw_maxes.append(c["raw_max"])
            else:
                raw_avgs.append(None)
                raw_maxes.append(None)
            if c and c["clean_n"]:
                clean_avgs.append(round(c["clean_sum"] / c["clean_n"], 1))
                clean_maxes.append(c["clean_max"])
            else:
                clean_avgs.append(None)
                clean_maxes.append(None)
        classes.append({"class_index": i, "name": stats.class_name(i),
                        "icon": stats.class_icon(i),
                        "values": _peak_preserve(raw_avgs, raw_maxes),
                        "values_clean": _peak_preserve(clean_avgs, clean_maxes)})

    pr_thr, effort_thr, xp_thr = await _clean_thresholds()
    payload = {
        "period": period,
        "bucket_seconds": bucket,
        "window_start": window_start,
        "window_end": now_ts,
        "power_rank_threshold": pr_thr,
        "effort_threshold": effort_thr,
        "xp_threshold": xp_thr,
        "buckets": buckets,
        "classes": classes,
        "methodology": _METHODOLOGY,
    }
    await lb_cache.set_class_activity_series(period, payload)
    return payload

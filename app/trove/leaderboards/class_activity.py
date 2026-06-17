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
# Same idea for the donut, which is computed directly from the latest snapshot
# (see class_activity_current); a transient empty capture still serves the last good.
_LAST_GOOD_DONUT: dict | None = None

_METHODOLOGY = (
    "Per class, distinct top-N players whose score rose (or who first appear) on "
    "its Effort board between two consecutive captures. (Paragon boards are "
    "excluded as ambiguous - counts are Effort-only.) The default 'clean' "
    "(established) view keeps only players who, snapshot at the window end, clear "
    "both configured floors - Power Rank (1000+i board) and Effort (4000+i) - "
    "filtering new characters and throwaway alts; the 'All' view counts everyone. "
    "Effort boards reset weekly (Mon 11:00 UTC); a window crossing the reset is "
    "unmeasurable and contributes no point. 'Share' is share of class activity (a "
    "player active on several classes counts in each), not distinct players. Lower "
    "bound: players outside a board's top-N aren't seen."
)

# The donut is computed differently from the series above: it's a direct headcount
# of the LATEST snapshot, with no activity (score-rose) condition at all.
_DONUT_METHODOLOGY = (
    "Per-class player share from the latest leaderboard snapshot - a direct "
    "headcount, NOT the activity pipeline. RAW counts the players present on a "
    "class's Effort board at the most recent capture (Paragon is excluded as "
    "ambiguous); the 'established' view keeps only those clearing both floors - "
    "Power Rank (1000+i) and Effort (4000+i). 'Share' is a class's count divided by "
    "the total across classes (a player on several classes counts in each), so "
    "shares sum to 100% but aren't distinct players. Each class also carries the "
    "Effort ADDED in the latest hour (this capture vs the previous) - the sum of "
    "positive score gains over players on its Effort board, per view; skipped right "
    "after a weekly reset. Lower bound: players outside a board's top-N aren't seen."
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
    """Power-Rank floor for the clean (established) view."""
    from app.core.config import settings
    return await _setting_int("class_activity_power_rank_threshold",
                              settings.class_activity_power_rank_threshold)


async def _effort_threshold() -> int:
    """Effort floor for the clean (established) view."""
    from app.core.config import settings
    return await _setting_int("class_activity_effort_threshold",
                              settings.class_activity_effort_threshold)


async def _clean_thresholds() -> tuple[int, int]:
    """The two runtime-tunable floors that define an "established" player:
    ``(power_rank, effort)``. A player counts toward a class's clean estimate only
    when they clear BOTH on that class. (Paragon is not used - it's ambiguous.)"""
    return (await _power_rank_threshold(), await _effort_threshold())


def _class_counts(
    early_maps: dict[int, dict[str, float]],
    late_maps: dict[int, dict[str, float]],
    early_ts: int,
    late_ts: int,
    *,
    pr_maps: dict[int, dict[str, float]] | None = None,
    threshold: float = 0,
    effort_threshold: float = 0,
) -> dict[int, dict]:
    """``{class_index: {"raw": int, "clean": int|None}}`` for one (early, late) pair.

    RAW is the class's Effort active set (``late_maps`` holds Effort boards only;
    Paragon is excluded as ambiguous). CLEAN (the "established" view) keeps only
    active players who, snapshot at the LATE anchor, clear BOTH per-class floors:
    Power Rank (board ``1000+i`` from ``pr_maps``) ``>= threshold`` and Effort
    (board ``4000+i`` from ``late_maps``) ``>= effort_threshold`` - filtering out
    new characters and throwaway alts. A floor of 0 is a no-op gate. A player
    missing from a board reads as 0 there.

    ``clean`` is ``None`` (unmeasurable, stored as NULL → the clean line gaps
    rather than plotting a false 0) when no Power Rank snapshot is available for
    the class (``pr_maps`` is None, or that board is absent at the anchor).

    A class is OMITTED entirely (no key) when its Effort board isn't measurable for
    the window (reset crossed / no early snapshot) - so the caller stores nothing
    and the series gaps. A measurable class with no active players keeps
    ``{"raw": 0, "clean": 0|None}`` (genuinely quiet)."""
    by_class: dict[int, set[str]] = {}
    for uuid, late in late_maps.items():
        if not late:
            continue
        s = _act._active_set(late, early_maps.get(uuid, {}), _KIND, early_ts, late_ts)
        if s is None:
            continue  # reset crossed or no early data for this board
        by_class.setdefault(stats.class_index_for_board(uuid), set()).update(s)

    out: dict[int, dict] = {}
    for i, players in by_class.items():
        clean: int | None = None
        if pr_maps is not None:
            pr = pr_maps.get(stats.class_pr_board_uuid(i))
            if pr is not None:
                effort = late_maps.get(stats.class_effort_board_uuid(i), {})
                clean = sum(
                    1 for p in players
                    if pr.get(p, 0.0) >= threshold
                    and effort.get(p, 0.0) >= effort_threshold
                )
        out[i] = {"raw": len(players), "clean": clean}
    return out


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

    pr_thr, effort_thr = await _clean_thresholds()
    board_uuids = stats.class_effort_board_uuids()   # Effort only (Paragon excluded)
    late_maps = await _act._load_anchor_maps(anchor_late, board_uuids)
    early_maps = await _act._load_anchor_maps(anchor_early, board_uuids)
    # Power Rank snapshot at the window END gates the clean view (one extra query;
    # Effort scores come from late_maps, already loaded above).
    pr_maps = await _act._load_anchor_maps(anchor_late, stats.class_pr_board_uuids())
    counts = _class_counts(early_maps, late_maps, anchor_early, anchor_late,
                           pr_maps=pr_maps, threshold=pr_thr, effort_threshold=effort_thr)

    now_ts = int(time.time())
    if counts:
        from app.trove.leaderboards import pg_store
        rows = [
            {"class_index": i, "window_end": anchor_late, "window_start": anchor_early,
             "duration_hours": round(duration_h, 2), "estimate": c["raw"],
             "estimate_clean": c["clean"], "computed_at": now_ts}
            for i, c in counts.items()
        ]
        try:
            await pg_store.upsert_class_estimates(rows)
        except Exception:
            logger.exception("class activity: persist failed for window_end=%d", anchor_late)

    payload = _build_current(anchor_early, anchor_late, duration_h, counts, now_ts,
                             pr_thr, effort_thr)
    if counts:
        _LAST_GOOD = payload
    return payload


def _snapshot_counts(
    effort_maps: dict[int, dict[str, float]],
    pr_maps: dict[int, dict[str, float]],
    pr_threshold: float, effort_threshold: float,
) -> dict[int, dict]:
    """``{class_index: {"raw", "clean"}}`` from ONE snapshot's presence - no
    activity (score-rose) condition. RAW = players present on a class's Effort board
    (Paragon is excluded as ambiguous); CLEAN (established) = those clearing both
    floors (Power Rank, Effort). ``clean`` is None if that class's Power Rank board
    is absent in the snapshot; classes with no players are omitted entirely."""
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
            )
        out[i] = {"raw": len(players), "clean": clean}
    return out


def _effort_deltas(
    effort_late: dict[int, dict[str, float]],
    effort_early: dict[int, dict[str, float]],
    pr_maps: dict[int, dict[str, float]],
    pr_threshold: float, effort_threshold: float,
) -> dict[int, dict]:
    """Per-class Effort ADDED over the latest capture pair: Σ max(0, late - early)
    over players on the class's Effort board in BOTH snapshots. ``raw`` = all such
    players; ``clean`` = those also clearing the Power-Rank + Effort floors at the
    late snapshot (None if the class's PR board is absent). New entrants are
    excluded - their hour's gain is unmeasurable on a weekly-accumulating board."""
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
                continue                       # new entrant - hour gain unknown
            gain = lv - ev
            if gain <= 0:
                continue
            raw += gain
            if (clean is not None
                    and pr.get(p, 0.0) >= pr_threshold and lv >= effort_threshold):
                clean += gain
        out[i] = {"raw": int(round(raw)),
                  "clean": int(round(clean)) if clean is not None else None}
    return out


async def class_activity_current() -> dict:
    """Per-class player share for the DONUT, computed DIRECTLY from the latest
    leaderboard snapshot - a real headcount, with NO activity (score-rose) step.
    RAW = players present on a class's Effort board at the newest capture (Paragon
    excluded); the established view applies the Power-Rank + Effort floors.
    Read-through Redis cache (the short series TTL); falls back to last good /
    an empty shell."""
    global _LAST_GOOD_DONUT
    from app.trove.leaderboards import cache as lb_cache
    cached = await lb_cache.get_class_activity_current()
    if cached is not None:
        return cached

    stamps = await lb_service.list_timestamps(limit=2, include_archive=True)
    if not stamps:
        return _LAST_GOOD_DONUT or _empty()
    anchor = stamps[0]
    pr_thr, effort_thr = await _clean_thresholds()
    effort_boards = stats.class_effort_board_uuids()
    effort_maps = await _act._load_anchor_maps(anchor, effort_boards)
    pr_maps = await _act._load_anchor_maps(anchor, stats.class_pr_board_uuids())
    counts = _snapshot_counts(effort_maps, pr_maps, pr_thr, effort_thr)
    if not counts:
        return _LAST_GOOD_DONUT or _empty()

    # Effort added in the latest hour (this capture vs the previous), per view.
    # Skip when the pair crosses a weekly reset (scores zeroed → not "added").
    if (len(stamps) >= 2
            and not lb_service.reset_boundaries_for_kind("weekly", stamps[1], anchor)):
        early_maps = await _act._load_anchor_maps(stamps[1], effort_boards)
        for i, d in _effort_deltas(effort_maps, early_maps, pr_maps, pr_thr, effort_thr).items():
            if i in counts:
                counts[i]["effort_raw"] = d["raw"]
                counts[i]["effort_clean"] = d["clean"]

    payload = _build_current(anchor, anchor, None, counts, int(time.time()),
                             pr_thr, effort_thr, methodology=_DONUT_METHODOLOGY)
    _LAST_GOOD_DONUT = payload
    await lb_cache.set_class_activity_current(payload)
    return payload


def _build_current(window_start, window_end, duration_h, counts: dict[int, dict],
                   computed_at: int, threshold: int,
                   effort_threshold: int = 0,
                   methodology: str = _METHODOLOGY) -> dict:
    raw_total = sum(c["raw"] for c in counts.values())
    clean_present = [c["clean"] for c in counts.values() if c["clean"] is not None]
    clean_total = sum(clean_present) if clean_present else None
    # Effort added this hour (donut only; absent on the activity-warmer payload).
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
        "classes": classes,
        "methodology": methodology,
        "computed_at": computed_at,
    }


def _empty() -> dict:
    return {
        "window_start": None, "window_end": None, "duration_hours": None,
        "total_active": None, "total_active_clean": None,
        "total_effort_added": None, "total_effort_added_clean": None,
        "power_rank_threshold": 0, "effort_threshold": 0,
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

    board_uuids = stats.class_effort_board_uuids()   # Effort only (Paragon excluded)
    pr_board_uuids = stats.class_pr_board_uuids()
    pr_thr, effort_thr = await _clean_thresholds()
    needed = sorted({a for pr in todo for a in pr})
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
                    # Power Rank snapshot at the window END gates the clean view.
                    pr_maps = await _act._load_anchor_maps(anchor, pr_board_uuids)
                    counts = _class_counts(early_maps, cur_maps, early, anchor,
                                           pr_maps=pr_maps, threshold=pr_thr,
                                           effort_threshold=effort_thr)
                    now_ts = int(time.time())
                    rows = [
                        {"class_index": i, "window_end": anchor, "window_start": early,
                         "duration_hours": round((anchor - early) / 3600.0, 2),
                         "estimate": c["raw"], "estimate_clean": c["clean"],
                         "computed_at": now_ts}
                        for i, c in counts.items()
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
    from app.trove.leaderboards import cache as lb_cache
    from app.trove.leaderboards import pg_store
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
    from app.trove.leaderboards import cache as lb_cache
    from app.trove.leaderboards import pg_store

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

    # bucket -> {t_sum, t_n, classes: {i: {raw_sum, raw_n, clean_sum, clean_n}}}.
    # Raw + clean (Power-Rank-filtered) per-hour rates are averaged independently;
    # clean_n only counts rows that HAD a clean value (NULL = unmeasurable, so the
    # clean line gaps there even when the raw line has a point).
    agg: dict[int, dict] = {}
    for r in rows:
        dur = r["duration_hours"] or 0.0
        if _act._is_gap(dur, gap_threshold):
            continue
        raw_ph = (r["estimate"] / dur) if dur > 0 else 0.0
        b = (r["window_end"] // bucket) * bucket
        bd = agg.get(b)
        if bd is None:
            bd = agg[b] = {"t_sum": 0, "t_n": 0, "classes": {}}
        bd["t_sum"] += r["window_end"]
        bd["t_n"] += 1
        c = bd["classes"].get(r["class_index"])
        if c is None:
            c = bd["classes"][r["class_index"]] = {
                "raw_sum": 0.0, "raw_n": 0, "clean_sum": 0.0, "clean_n": 0,
            }
        c["raw_sum"] += raw_ph
        c["raw_n"] += 1
        cl = r["estimate_clean"]
        if cl is not None:
            c["clean_sum"] += (cl / dur) if dur > 0 else 0.0
            c["clean_n"] += 1

    sorted_b = sorted(agg)
    buckets = [round(agg[b]["t_sum"] / agg[b]["t_n"]) for b in sorted_b]
    classes = []
    for i in range(stats.class_count()):
        values, values_clean = [], []
        for b in sorted_b:
            c = agg[b]["classes"].get(i)
            values.append(round(c["raw_sum"] / c["raw_n"], 1) if (c and c["raw_n"]) else None)
            values_clean.append(
                round(c["clean_sum"] / c["clean_n"], 1) if (c and c["clean_n"]) else None
            )
        classes.append({"class_index": i, "name": stats.class_name(i),
                        "icon": stats.class_icon(i),
                        "values": values, "values_clean": values_clean})

    pr_thr, effort_thr = await _clean_thresholds()
    payload = {
        "period": period,
        "bucket_seconds": bucket,
        "window_start": window_start,
        "window_end": now_ts,
        "power_rank_threshold": pr_thr,
        "effort_threshold": effort_thr,
        "buckets": buckets,
        "classes": classes,
        "methodology": _METHODOLOGY,
    }
    await lb_cache.set_class_activity_series(period, payload)
    return payload

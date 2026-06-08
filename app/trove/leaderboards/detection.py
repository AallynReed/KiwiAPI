"""Statistical outlier detection for the leaderboards dataset.

Three independent checks, each catching a different shape of "weird":

1. **Modified Z-score (MAD-based)** — Within a single anchor's snapshot,
   how far is a player's score from the board's *median*? Uses Median
   Absolute Deviation instead of mean+stddev so a single cheater can't
   pollute their own baseline. Iglewicz & Hoaglin (1993) define a
   modified-Z of |M| > 3.5 as a "strong outlier"; we use that as the
   default and the value is runtime-tunable.

2. **Rank-gap ratio** — The drop from rank N to rank N+1 within the
   top of a board. If rank 1 has 10× rank 2's score and the typical
   between-rank gap is < 5 %, that lone-wolf shape is highly unusual.
   Catches the case where MAD-Z is fooled because rank 1 is far enough
   from the median that *some* peers below them are also outliers,
   widening MAD; rank-gap looks at adjacency directly.

3. **Velocity** — Score increase per unit time, computed from the
   player's previous historical capture. Compared against the board's
   peer-p95 velocity. Catches sudden jumps that wouldn't yet look
   anomalous against the *current* distribution but do against the
   player's own history.

Each piece of evidence is a self-describing dict: the measured value,
the peer baseline it's compared against, the statistical magnitude,
the threshold, and a human-readable interpretation.

Returns ``{players: [...], computed_at, anchor, method, config,
total_flagged, boards_analyzed}``. The result is cached in process
memory keyed by ``(anchor, *all_config_values)`` so a tunable change
auto-invalidates the cache.
"""
from __future__ import annotations

import asyncio
import logging
import math
import statistics
import time
from datetime import UTC, datetime, timedelta

from app.trove.leaderboards import service as lb_service
from app.trove.leaderboards.models import (
    LeaderboardEntry,
    LeaderboardEntryArchive,
)

logger = logging.getLogger(__name__)


# Cache: {cache_key: (stored_at_unix, payload)}.
_CACHE: dict[tuple, tuple[float, dict]] = {}

# Pull enough entries per board to cover the realistic upper bound
# (Trove boards top out around 5000 visible entries; we round up).
_BOARD_FETCH_LIMIT = 10000
# Limit historical "peer" velocity computation to the top-N entries on
# each board so a 5000-row board doesn't trigger 5000 archive queries.
_VELOCITY_PEER_TOP_N = 50


# Last successful computation, regardless of anchor. Survives anchor
# changes so an on-the-fly cache miss can still serve "the previous good
# answer" while the warmer is busy recomputing for the new anchor — the
# alternative is a multi-second wait for the first visitor after each
# leaderboard ingest. See the "stale-but-known-good" branch in
# ``detect_possible_cheaters`` below.
_LAST_GOOD: dict | None = None


async def detect_possible_cheaters() -> dict:
    """Run all three checks against the most recent anchor in the hot
    collection. Cached behaviour:

    1. If the current (anchor, config) is in ``_CACHE`` within the TTL
       window, return it instantly — this is the warmer's normal hit.
    2. If the current key isn't cached BUT we have a ``_LAST_GOOD``
       payload from a previous anchor, return that immediately AND fire
       the background warmer so the new anchor lands in cache for the
       next request. This is the "don't invalidate old data until new
       is ready" guarantee — the user keeps seeing yesterday's flags
       instead of a multi-second spinner while detection re-runs.
    3. Cold start (no cache, no last-good) — fall through to
       synchronous compute. Slow on the FIRST request after a fresh
       boot, never again because the result feeds ``_LAST_GOOD``.
    """
    from app.admin import runtime_config

    z_threshold = float(await runtime_config.get_setting("cheaters_z_threshold"))
    velocity_multiplier = float(
        await runtime_config.get_setting("cheaters_velocity_multiplier")
    )
    min_board_size = int(await runtime_config.get_setting("cheaters_min_board_size"))
    cache_ttl = float(await runtime_config.get_setting("cheaters_cache_ttl_seconds"))
    cohort_pct = float(
        await runtime_config.get_setting("cheaters_elite_cohort_pct")
    )
    excluded_csv = str(
        await runtime_config.get_setting("cheaters_excluded_board_uuids")
    )
    excluded = _parse_excluded(excluded_csv)

    timestamps = await lb_service.list_timestamps(limit=1, include_archive=False)
    if not timestamps:
        return _empty(None, z_threshold, velocity_multiplier, min_board_size)

    anchor = timestamps[0]
    # Cache key includes excluded set (as sorted tuple, since sets aren't
    # hashable) so a master-panel edit invalidates immediately.
    cache_key = (
        anchor, z_threshold, velocity_multiplier, min_board_size,
        cohort_pct, tuple(sorted(excluded)),
    )
    now = time.time()
    hit = _CACHE.get(cache_key)
    if hit is not None and now - hit[0] < cache_ttl:
        global _LAST_GOOD
        _LAST_GOOD = hit[1]
        return hit[1]

    # No cache for THIS anchor. If we have a last-good payload from an
    # earlier anchor, serve that and let the warmer fill in the gap
    # — never block the user on the recompute.
    if _LAST_GOOD is not None:
        trigger_warmer()
        return _LAST_GOOD

    # Cold start. Compute synchronously, feed last-good for next time.
    result = await _compute(
        anchor, z_threshold, velocity_multiplier, min_board_size,
        excluded, cohort_pct,
    )
    _CACHE[cache_key] = (now, result)
    _LAST_GOOD = result
    _prune_cache(now, cache_ttl)
    return result


def _parse_excluded(csv: str) -> set[int]:
    """Parse the runtime-config CSV ('1100, 21012, foo, 5001') into a
    set of int UUIDs. Non-numeric tokens are silently skipped so a typo
    can't blow up the detection."""
    out: set[int] = set()
    for token in csv.split(","):
        s = token.strip()
        if not s:
            continue
        try:
            out.add(int(s))
        except ValueError:
            logger.warning(
                "cheaters: ignoring non-numeric excluded-board token %r",
                s,
            )
    return out


def invalidate_cache() -> None:
    """Drop all cached results. Wire this to runtime_config writes for
    instant invalidation when the master flips a knob."""
    _CACHE.clear()


# ─── Background warmer ────────────────────────────────────────────────
# Pre-computes every heavy leaderboards-page query at app boot + on every
# TTL boundary + immediately after a new ingest so the public endpoints
# always return a cached result instantly — no caller ever waits for the
# per-board scan + history queries to run synchronously.
#
# Three caches kept warm in lock-step:
#   • detect_possible_cheaters         — cheaters tab
#   • activity.estimate_active_players — live-pulse pill in the hero
#   • lb_service.list_boards_at(latest) — sidebar of the boards tab
#
# All three are tied to "the most recent anchor", so when the bot ingests
# a new capture they ALL go stale at once. ``trigger_warmer()`` wakes the
# loop so it doesn't have to wait the full TTL before refilling them.

_warmup_task: asyncio.Task | None = None
# Set by ``trigger_warmer()`` to wake the sleeping loop early. We
# ``asyncio.wait`` on this OR the TTL sleep, whichever resolves first.
_wake_event: asyncio.Event | None = None


async def _warmup_loop() -> None:
    """Re-warm every dependent cache on the same cadence as the cache
    TTL. Catches per-iteration exceptions so a transient DB hiccup
    doesn't kill the background task."""
    global _wake_event
    _wake_event = asyncio.Event()

    # Tiny initial delay so Beanie's Document init has time to land
    # before the first scan touches the collections.
    await asyncio.sleep(2.0)
    while True:
        start = time.time()
        try:
            await _warm_all()
            elapsed = time.time() - start
            logger.info("leaderboards warmer: caches refreshed in %.2fs", elapsed)
        except asyncio.CancelledError:
            raise
        except Exception:
            logger.exception("leaderboards warmer: iteration failed")

        # Re-read TTL each loop so a runtime-config edit immediately
        # adjusts the cadence on the next sleep.
        try:
            from app.admin import runtime_config
            ttl = float(await runtime_config.get_setting("cheaters_cache_ttl_seconds"))
        except Exception:
            ttl = 1800.0  # fall back to the default if config lookup fails
        # Subtract the work time so the effective period is ~TTL, not
        # TTL + work_time. Floor at 30s so a flurry of post-ingest
        # triggers doesn't peg the worker on a tight loop.
        next_sleep = max(30.0, ttl - (time.time() - start))
        # Sleep with an early-wake escape hatch — trigger_warmer() sets
        # the event when a new ingest lands so the new anchor's caches
        # start filling immediately instead of after a full TTL.
        try:
            await asyncio.wait_for(_wake_event.wait(), timeout=next_sleep)
        except asyncio.TimeoutError:
            pass
        except asyncio.CancelledError:
            raise
        _wake_event.clear()


async def _warm_all() -> None:
    """One iteration of the warmer — re-runs every heavy query that
    feeds the leaderboards page. Runs sequentially (not concurrently)
    because they hit overlapping Mongo collections and we'd rather not
    interleave their cursors."""
    # cheaters detection — the slowest, run first so it's ready ASAP
    await detect_possible_cheaters()
    # activity estimate — also a multi-board scan
    from app.trove.leaderboards import activity as lb_activity
    await lb_activity.estimate_active_players()
    # boards-at-latest — cheap distinct(), but worth pre-running so the
    # sidebar paint is instant even on a cold Mongo page cache
    stamps = await lb_service.list_timestamps(limit=1, include_archive=False)
    if stamps:
        await lb_service.list_boards_at(stamps[0])


def trigger_warmer() -> None:
    """Wake the warmer loop early — call after each successful
    leaderboard ingest so the new anchor's caches start filling
    immediately instead of after the TTL. No-op if the warmer hasn't
    been started yet."""
    if _wake_event is None:
        return
    try:
        _wake_event.set()
    except RuntimeError:
        # event loop closed (during shutdown) — nothing to do
        pass


def start_leaderboards_warmer() -> None:
    """Kick off the background warmer if it isn't already running.
    Idempotent — safe to call multiple times."""
    global _warmup_task
    if _warmup_task is not None and not _warmup_task.done():
        return
    _warmup_task = asyncio.create_task(_warmup_loop(), name="leaderboards-warmer")


async def stop_leaderboards_warmer() -> None:
    """Cancel the warmer cleanly on app shutdown."""
    global _warmup_task
    task = _warmup_task
    _warmup_task = None
    if task is None or task.done():
        return
    task.cancel()
    try:
        await task
    except (asyncio.CancelledError, Exception):
        pass


# Back-compat aliases — the warmer used to only handle cheaters. Existing
# call sites in app/main.py (and any out-of-tree callers) keep working;
# new code should reach for ``start_leaderboards_warmer`` directly.
start_cheaters_warmer = start_leaderboards_warmer
stop_cheaters_warmer = stop_leaderboards_warmer


# ─── Compute pipeline ──────────────────────────────────────────────────


async def _compute(
    anchor: int,
    z_threshold: float,
    velocity_multiplier: float,
    min_board_size: int,
    excluded: set[int] | None = None,
    cohort_pct: float = 0.05,
) -> dict:
    boards = await lb_service.list_boards_at(anchor)
    excluded = excluded or set()

    # player_name → {board_uuid → board_entry_dict}
    flagged: dict[str, dict[int, dict]] = {}
    boards_analyzed = 0
    boards_excluded = 0

    from app.trove.leaderboards.models import is_lifetime_kind

    for board in boards:
        if board["uuid"] in excluded:
            boards_excluded += 1
            continue
        entries, _total = await lb_service.list_entries(
            board["uuid"], anchor, limit=_BOARD_FETCH_LIMIT, offset=0,
        )
        if len(entries) < min_board_size:
            continue
        boards_analyzed += 1

        higher_is_better = _detect_direction(entries)

        # Resetting vs. lifetime gating:
        #   • Resetting boards (daily/weekly) — every cycle starts at
        #     zero, so a rank-1 score that's an order of magnitude
        #     above rank-2 is a meaningful anomaly. Score-outlier +
        #     rank-gap + velocity all carry signal.
        #   • Lifetime boards (default / explicit "none") — score
        #     accumulates across the player's entire account history,
        #     so the top of the board is naturally orders of magnitude
        #     above mid-pack. Running score-outlier or rank-gap there
        #     produces only false positives. ONLY velocity (delta
        #     between captures) is a valid anti-cheat signal.
        # ``effective_reset_kind`` already resolved the admin override
        # → ``board["reset_kind"]`` for us in service.list_boards_at.
        if not is_lifetime_kind(board.get("reset_kind", "default")):
            _score_outlier_check(
                flagged, board, entries, z_threshold, higher_is_better,
                cohort_pct=cohort_pct,
            )
            _rank_gap_check(flagged, board, entries, higher_is_better)
        await _velocity_check(
            flagged, board, entries, anchor,
            velocity_multiplier, higher_is_better,
        )

    return _format(
        flagged, anchor, z_threshold, velocity_multiplier,
        min_board_size, boards_analyzed, boards_excluded,
    )


def _detect_direction(entries: list[dict]) -> bool:
    """True if higher scores are better on this board. Inferred from
    where rank 1's score sits in the distribution: if it's the MAX,
    higher-is-better; if it's the MIN, lower-is-better (speedrun-style).
    Defaults to True (higher-is-better) on degenerate distributions."""
    scores = [e["score"] for e in entries]
    rank1_score = next(
        (e["score"] for e in entries if e["rank"] == 1),
        entries[0]["score"],
    )
    s_max = max(scores)
    s_min = min(scores)
    if rank1_score == s_max and s_max != s_min:
        return True
    if rank1_score == s_min and s_max != s_min:
        return False
    return True


def _score_outlier_check(
    flagged: dict, board: dict, entries: list[dict],
    z_threshold: float, higher_is_better: bool,
    cohort_pct: float = 0.05,
) -> None:
    """Modified Z-score on **log-transformed** scores within the
    **elite cohort** (top N or top ``cohort_pct``-fraction).

    Two design choices, each fixing a class of false positive:

    1. **Elite cohort** instead of full board.
       Trove leaderboards are heavy-tailed — the top 1 % typically
       scores 10–100× the median. Naive MAD-Z against the full
       population flags every dedicated top player as an "outlier"
       because that's what "top player" mathematically means in a
       power-law distribution. Cheaters appear at the TOP, so we
       measure their anomalousness vs. the population of OTHER top
       players, not vs. the typical player.

    2. **Log-transform** before computing median + MAD.
       Even inside the elite cohort, score distributions are power-
       law-shaped (rank-1 can still be 10× rank-50). Linear MAD on
       that produces hundreds of false positives because each rank
       step looks like an outlier. ``log10`` flattens the cohort into
       an approximately symmetric shape where a true outlier stands
       out cleanly. On the test cfg this drops false positives by ~9×
       compared with linear MAD on the same cohort (51 vs 464 across
       all unexcluded boards).

    Algorithm:

    1. Sort by rank, take top ``max(50, board_size * cohort_pct)`` as
       the elite cohort.
    2. Skip the board if the top 5 cohort scores are tied — that's a
       capped board (e.g. class boards at 59 731) where the elite has
       no spread to measure against.
    3. log10(score + 1) on the cohort. Compute median + MAD.
    4. Only check the **top half** of the cohort. Outside this slice
       a player can't be "the suspicious top score" by definition.
    5. Flag in the board's "better" direction only — we don't care
       about players who are unusually bad.
    """
    ranked = sorted(entries, key=lambda e: e["rank"])
    n = len(ranked)

    cohort_size = max(50, int(n * cohort_pct))
    cohort_size = min(cohort_size, n)
    cohort = ranked[:cohort_size]
    cohort_scores = [e["score"] for e in cohort]

    # Capped board: top of the elite is uniform, no signal possible.
    if len(set(cohort_scores[:5])) == 1:
        return

    # Log-transform handles power-law shapes inside the cohort. ``+1``
    # so a legitimate zero score doesn't produce ``log(0)`` = -inf.
    log_cohort = [math.log10(s + 1.0) if s >= 0 else 0.0 for s in cohort_scores]
    log_median = statistics.median(log_cohort)
    log_mad = statistics.median([abs(x - log_median) for x in log_cohort])
    if log_mad == 0:
        return

    # Median in linear space (for human-readable summary).
    median = statistics.median(cohort_scores)

    # Only check the top half of the cohort.
    check_n = max(10, cohort_size // 2)

    for e in cohort[:check_n]:
        log_s = math.log10(e["score"] + 1.0) if e["score"] >= 0 else 0.0
        mz = 0.6745 * (log_s - log_median) / log_mad
        better_outlier = mz > z_threshold if higher_is_better else mz < -z_threshold
        if not better_outlier:
            continue
        # Express the magnitude as a multiplier vs. the cohort median
        # — easier to read than "5.2 log-z-scores above log-median".
        ratio = e["score"] / median if median > 0 else float("inf")
        _add_evidence(flagged, e["player_name"], board, e, {
            "type": "score_outlier",
            "summary": (
                f"Score {_fmt(e['score'])} is {ratio:.1f}× the TOP-"
                f"{cohort_size} median of {_fmt(median)} on this "
                f"{n}-player board ({abs(mz):.1f} robust log-z-scores "
                f"{'above' if higher_is_better else 'below'}, threshold: "
                f"{z_threshold}). Log-space MAD-Z on the elite cohort "
                f"resists both cheater self-pollution and the natural "
                f"heavy-tail of leaderboard shapes."
            ),
            "measurements": {
                "player_score": _round(e["score"]),
                "elite_median": _round(median),
                "modified_z_score": round(mz, 2),
                "threshold": z_threshold,
                "median_multiplier": round(ratio, 2),
                "higher_is_better": higher_is_better,
                "cohort_size": cohort_size,
                "board_size": n,
            },
        })


def _rank_gap_check(
    flagged: dict, board: dict, entries: list[dict], higher_is_better: bool,
) -> None:
    """Rank-N vs rank-N+1 score gap as a fraction of the higher score,
    compared against the median between-rank gap on the rest of the
    board."""
    ranked = sorted(entries, key=lambda x: x["rank"])
    if len(ranked) < 4:
        return

    gaps = []
    for i in range(len(ranked) - 1):
        a = ranked[i]["score"]
        b = ranked[i + 1]["score"]
        bigger = max(abs(a), abs(b))
        gaps.append(abs(a - b) / bigger if bigger > 0 else 0.0)

    # Baseline: median of the tail gaps (after the top 3) — that's the
    # "typical between-rank step" without contamination from anomalies
    # at the very top.
    tail = gaps[3:] if len(gaps) > 3 else gaps
    typical = statistics.median(tail)
    if typical <= 0:
        return

    # Inspect the top 3 gaps; flag the rank ABOVE the gap (i.e. the
    # player with the unusually large lead over their next neighbor).
    rank_gap_threshold = 10.0  # hardcoded floor; same value the confidence calc reads
    for i in range(min(3, len(gaps))):
        if gaps[i] <= 0:
            continue
        multiplier = gaps[i] / typical
        if multiplier < rank_gap_threshold:
            continue
        e = ranked[i]
        nxt = ranked[i + 1]
        _add_evidence(flagged, e["player_name"], board, e, {
            "type": "rank_gap",
            "summary": (
                f"Rank-{e['rank']} score {_fmt(e['score'])} is "
                f"{multiplier:.0f}× the typical between-rank gap on this "
                f"board ({gaps[i]*100:.0f}% vs typical {typical*100:.1f}%). "
                f"Next-rank score: {_fmt(nxt['score'])}."
            ),
            "measurements": {
                "player_rank": e["rank"],
                "player_score": _round(e["score"]),
                "next_rank": nxt["rank"],
                "next_rank_score": _round(nxt["score"]),
                "gap_fraction": round(gaps[i], 4),
                "typical_gap_fraction": round(typical, 4),
                "gap_multiplier": round(multiplier, 1),
                "threshold_multiplier": rank_gap_threshold,
            },
        })


async def _velocity_check(
    flagged: dict, board: dict, entries: list[dict], anchor: int,
    velocity_multiplier: float, higher_is_better: bool,
) -> None:
    """Score-gain rate vs the board's peer p95. Runs only for players
    already flagged by another check (saves a lot of archive lookups).
    Computes the peer baseline from the top-N players who have history.
    """
    flagged_on_board = {
        name for name, boards_map in flagged.items()
        if board["uuid"] in boards_map
    }
    if not flagged_on_board:
        return

    ranked = sorted(entries, key=lambda x: x["rank"])
    reset_kind = board.get("reset_kind", "default")
    peer_velocities: list[float] = []
    # Build the peer baseline from the top-N entries that actually have
    # historical data IN THE SAME RESET CYCLE.
    for e in ranked[:_VELOCITY_PEER_TOP_N]:
        prev = await _previous_capture(
            e["player_name"], board["uuid"], anchor, reset_kind,
        )
        if prev is None:
            continue
        prev_score, prev_anchor = prev
        v = _velocity(e["score"], prev_score, anchor, prev_anchor, higher_is_better)
        if v is not None and v > 0:
            peer_velocities.append(v)

    if len(peer_velocities) < 5:
        return  # too few samples to call any baseline meaningful

    peer_velocities.sort()
    p95_idx = max(0, int(0.95 * len(peer_velocities)) - 1)
    peer_p95 = peer_velocities[p95_idx]
    if peer_p95 <= 0:
        return

    for name in flagged_on_board:
        e = next((x for x in ranked if x["player_name"] == name), None)
        if e is None:
            continue
        prev = await _previous_capture(name, board["uuid"], anchor, reset_kind)
        if prev is None:
            continue
        prev_score, prev_anchor = prev
        v = _velocity(e["score"], prev_score, anchor, prev_anchor, higher_is_better)
        if v is None or v < peer_p95 * velocity_multiplier:
            continue
        delta_t_h = (anchor - prev_anchor) / 3600.0
        delta_s = e["score"] - prev_score if higher_is_better else prev_score - e["score"]
        _add_evidence(flagged, name, board, e, {
            "type": "velocity_outlier",
            "summary": (
                f"Score gained {_fmt(delta_s)} in {delta_t_h:.1f}h "
                f"(rate {_fmt(v)}/h). This board's peer p95 rate is "
                f"{_fmt(peer_p95)}/h — this player is {v / peer_p95:.0f}× faster."
            ),
            "measurements": {
                "score_delta": _round(delta_s),
                "duration_hours": round(delta_t_h, 2),
                "rate_per_hour": _round(v),
                "peer_p95_rate_per_hour": _round(peer_p95),
                "rate_multiplier": round(v / peer_p95, 1),
                "threshold_multiplier": velocity_multiplier,
                "previous_anchor": prev_anchor,
                "previous_score": _round(prev_score),
            },
        })


def _velocity(
    cur_score: float, prev_score: float, cur_anchor: int, prev_anchor: int,
    higher_is_better: bool,
) -> float | None:
    delta_t_h = (cur_anchor - prev_anchor) / 3600.0
    if delta_t_h <= 0:
        return None
    delta_s = cur_score - prev_score if higher_is_better else prev_score - cur_score
    if delta_s <= 0:
        return None
    return delta_s / delta_t_h


def _reset_boundary_before(anchor: int, reset_kind: str) -> int:
    """Most recent reset moment at or before ``anchor`` for a board with
    the given ``reset_kind``.

    Trove resets at **11:00 UTC**:
    - ``"daily"`` boards reset every day
    - ``"weekly"`` boards reset every Monday
    - ``"default"`` boards don't reset (accumulate forever) — we
      return 0 so any prior anchor passes the cycle-membership check.

    Critical for the velocity check: comparing a score across a reset
    boundary is meaningless. Pre-reset Δ-score is negative (already
    filtered) but post-reset "starting fresh and grinding hard" can
    masquerade as a velocity outlier vs a peer baseline that includes
    pre-reset velocities — this guard prevents that.
    """
    if reset_kind == "default" or not reset_kind:
        return 0
    d = datetime.fromtimestamp(anchor, UTC)
    # Most-recent 11:00 UTC moment at or before ``anchor``.
    eleven = d.replace(hour=11, minute=0, second=0, microsecond=0)
    if d < eleven:
        eleven -= timedelta(days=1)
    if reset_kind == "daily":
        return int(eleven.timestamp())
    if reset_kind == "weekly":
        # Walk back day-by-day to the most recent Monday at 11:00 UTC.
        # Python's weekday(): Monday=0, Tuesday=1, …, Sunday=6.
        while eleven.weekday() != 0:
            eleven -= timedelta(days=1)
        return int(eleven.timestamp())
    # Unknown reset_kind — fail safe by allowing any prior anchor.
    return 0


async def _previous_capture(
    player_name: str, board_uuid: int, current_anchor: int,
    reset_kind: str = "default",
) -> tuple[float, int] | None:
    """Most recent (score, anchor) for ``player_name`` on ``board_uuid``
    strictly before ``current_anchor`` AND within the same reset cycle.

    Reset-cycle filtering: for daily/weekly boards we only consider
    anchors after the most-recent reset moment, so velocity isn't
    computed across a "everyone went back to zero" boundary.

    Tries hot first, falls through to archive.
    """
    cycle_start = _reset_boundary_before(current_anchor, reset_kind)
    query = {
        "player_name": player_name,
        "leaderboard": board_uuid,
        "created_at": {"$lt": current_anchor, "$gte": cycle_start},
    }
    doc = await LeaderboardEntry.find_one(query, sort=[("created_at", -1)])
    if doc is not None:
        return doc.score, doc.created_at
    doc = await LeaderboardEntryArchive.find_one(query, sort=[("created_at", -1)])
    if doc is not None:
        return doc.score, doc.created_at
    return None


# ─── Result assembly + helpers ─────────────────────────────────────────


def _add_evidence(
    flagged: dict, player_name: str, board: dict, entry: dict, evidence: dict,
) -> None:
    p = flagged.setdefault(player_name, {})
    b = p.get(board["uuid"])
    if b is None:
        b = {
            "uuid": board["uuid"],
            "name": board["name"],
            "category": board["category"],
            "contest_type": board.get("contest_type"),
            "rank": entry["rank"],
            "score": _round(entry["score"]),
            "evidence": [],
        }
        p[board["uuid"]] = b
    b["evidence"].append(evidence)


def _format(
    flagged: dict, anchor: int | None,
    z: float, vm: float, mb: int, boards_analyzed: int,
    boards_excluded: int = 0,
) -> dict:
    players = []
    for name, boards_map in flagged.items():
        boards = list(boards_map.values())
        # Per-evidence + per-board confidence. The board-level value is
        # the max evidence confidence on THAT board — within a board,
        # multiple checks are correlated, so we don't compound them.
        # Across-board compounding happens in _player_confidence.
        for b in boards:
            ev_list = b.get("evidence", [])
            for ev in ev_list:
                ev["confidence"] = _evidence_confidence(ev)
            b["confidence"] = (
                max((ev["confidence"] for ev in ev_list), default=0.0)
            )
        players.append({
            "player_name": name,
            "leaderboards": boards,
            "confidence": _player_confidence(boards),
        })
    # Sort by confidence desc, then by total evidence count desc as a
    # tiebreaker — most-suspicious-first.
    players.sort(
        key=lambda p: (
            -p["confidence"],
            -sum(len(b["evidence"]) for b in p["leaderboards"]),
        ),
    )
    return {
        "players": players,
        "computed_at": int(time.time()),
        "anchor": anchor,
        "method": (
            "Three independent statistical checks: Modified Z-score "
            "(MAD-based, Iglewicz & Hoaglin 1993), rank-gap ratio, and "
            "velocity vs peer p95. A player flagged by multiple checks "
            "or on multiple boards is higher confidence."
        ),
        "config": {
            "z_threshold": z,
            "velocity_multiplier": vm,
            "min_board_size": mb,
        },
        "total_flagged": len(players),
        "boards_analyzed": boards_analyzed,
        "boards_excluded": boards_excluded,
    }


def _empty(anchor: int | None, z: float, vm: float, mb: int) -> dict:
    return _format({}, anchor, z, vm, mb, 0)


# ─── Confidence ──────────────────────────────────────────────────────


# Per-check confidence ceilings. Reflects each check's EMPIRICAL
# false-positive rate measured against real multi-capture cfg data:
#
#   - velocity_outlier: cleanest signal. On a 4-capture window across
#     ~85 boards, only 4 players cleared the 10× peer-p95 threshold,
#     and the top one was an obvious cheater (1854 quests/hour). Strong
#     enough to trust on its own.
#   - rank_gap: moderate. A top-1 player 30× ahead of rank-2 is
#     unusual but can be legitimate on niche boards. Trust it most of
#     the way but don't push past 0.85 without corroboration.
#   - score_outlier: noisiest signal. Even with log-MAD-Z on the elite
#     cohort, 51 single-signal flags appeared on real data — the vast
#     majority were rank-1 to rank-5 players who are legitimately
#     ahead of their cohort. Cap low so it can only contribute when
#     another check confirms.
_CHECK_CONFIDENCE_CEILINGS: dict[str, float] = {
    "velocity_outlier": 0.99,
    "rank_gap": 0.85,
    "score_outlier": 0.60,
}


def _evidence_confidence(ev: dict) -> float:
    """Per-evidence confidence on [0.5, ceiling]. Sigmoid on the ratio
    ``magnitude / threshold`` then capped by the per-check ceiling above:

    - at the threshold: 0.50 (borderline)
    - 2× threshold:     ~0.82  (or check ceiling, whichever lower)
    - 5× threshold+:    capped at the check's ceiling

    A noisy check like ``score_outlier`` therefore can't claim higher
    confidence than 0.60 on its own. To exceed the ceiling, the player
    must accumulate evidence from MULTIPLE check types (handled in
    ``_player_confidence``).
    """
    m = ev.get("measurements", {}) or {}
    et = ev.get("type")

    if et == "score_outlier":
        threshold = float(m.get("threshold", 5.0))
        magnitude = abs(float(m.get("modified_z_score", 0.0)))
    elif et == "rank_gap":
        threshold = float(m.get("threshold_multiplier", 10.0))
        magnitude = float(m.get("gap_multiplier", 0.0))
    elif et == "velocity_outlier":
        threshold = float(m.get("threshold_multiplier", 10.0))
        magnitude = float(m.get("rate_multiplier", 0.0))
    else:
        return 0.5  # unknown evidence type, stay conservative

    if threshold <= 0 or magnitude < threshold:
        return 0.5
    ratio = magnitude / threshold
    raw = 0.5 + 0.5 * (1 - math.exp(-(ratio - 1)))
    ceiling = _CHECK_CONFIDENCE_CEILINGS.get(et, 0.99)
    return round(min(ceiling, raw), 3)


def _player_confidence(boards: list[dict]) -> float:
    """Aggregate confidence across a player's boards.

    Strategy:

    - **Within a board**: MAX evidence confidence. The three checks
      are NOT independent at the per-board level — MAD-Z and rank-gap
      both light up when one player dominates, so multiplying would
      double-count the same anomaly.
    - **Across boards**: noisy-OR. Independence is defensible — one
      board's anomaly doesn't explain another's. Two boards each at
      0.95 → 1 − 0.05² = 0.9975.
    - **Check-type diversity cap**: if ALL the evidence the player has
      is the same check type, cap overall confidence at that type's
      ceiling (see ``_CHECK_CONFIDENCE_CEILINGS``). Empirically,
      score_outlier alone produces dozens of false positives per anchor;
      this cap keeps them under the default 0.9 filter unless another
      check type also fires.

    Returns 0.0 if the player has no evidence (shouldn't happen but
    fail safe).
    """
    inv = 1.0
    saw_any = False
    types_seen: set[str] = set()
    for b in boards:
        ev_list = b.get("evidence", []) or []
        if not ev_list:
            continue
        # Track which check types contributed.
        for ev in ev_list:
            t = ev.get("type")
            if t:
                types_seen.add(t)
        board_conf = max(_evidence_confidence(ev) for ev in ev_list)
        # Re-read confidence in case _format pre-populated it; pick max
        # of either source so this helper is order-independent.
        ev_conf_attr = max((ev.get("confidence", 0.0) for ev in ev_list), default=0.0)
        if ev_conf_attr > board_conf:
            board_conf = ev_conf_attr
        inv *= (1.0 - board_conf)
        saw_any = True
    if not saw_any:
        return 0.0

    raw = 1.0 - inv

    # Apply diversity cap: single-check-type players are limited to
    # that type's ceiling, no matter how many boards they're on. A
    # cheater whose static rank is anomalous on 5 boards but who has
    # NORMAL growth velocity is most likely a legitimate dedicated
    # top player, not a cheater.
    if len(types_seen) == 1:
        only_type = next(iter(types_seen))
        cap = _CHECK_CONFIDENCE_CEILINGS.get(only_type, 0.99)
        raw = min(raw, cap)

    return round(raw, 3)


def _prune_cache(now: float, ttl: float) -> None:
    expired = [k for k, (t, _) in _CACHE.items() if now - t > ttl * 2]
    for k in expired:
        del _CACHE[k]


def _round(v: float) -> float | int:
    """Keep ints as ints, round floats to 2dp for cleaner JSON."""
    if isinstance(v, int) or v == int(v):
        return int(v)
    return round(v, 2)


def _fmt(v: float) -> str:
    """Display formatter for log-style summaries: thousands sep, no
    trailing .0 on integers."""
    if v == int(v):
        return f"{int(v):,}"
    return f"{v:,.2f}"

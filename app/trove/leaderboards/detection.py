"""Statistical outlier detection for the leaderboards dataset.

Four independent checks, each catching a different shape of "weird":

1. **Modified Z-score (MAD-based)** - Within a single anchor's snapshot,
   how far is a player's score from the board's *median*? Uses Median
   Absolute Deviation instead of mean+stddev so a single cheater can't
   pollute their own baseline. Iglewicz & Hoaglin (1993) define a
   modified-Z of |M| > 3.5 as a "strong outlier"; we use that as the
   default and the value is runtime-tunable.

2. **Rank-gap ratio** - The drop from rank N to rank N+1 within the
   top of a board. If rank 1 has 10× rank 2's score and the typical
   between-rank gap is < 5 %, that lone-wolf shape is highly unusual.
   Catches the case where MAD-Z is fooled because rank 1 is far enough
   from the median that *some* peers below them are also outliers,
   widening MAD; rank-gap looks at adjacency directly.

3. **Velocity** - Score increase per unit time, computed from the
   player's previous historical capture. Compared against the board's
   peer-p95 velocity. Catches sudden jumps that wouldn't yet look
   anomalous against the *current* distribution but do against the
   player's own history.

4. **Alt-cluster** - the only *group*-shaped check. The three above all
   hunt for a SINGLE player far above their peers; an "alt army" is the
   opposite - a pack of similarly-named accounts (``anana1 … anana20``,
   ``Aan_1 … Aan_7``) sitting at near-identical scores, where nobody is
   an individual outlier but the cluster is blatant. Groups entries by a
   normalised name stem (trailing digits/separators stripped) plus a
   small edit-distance merge, keeps the densest near-score subset per
   board, then merges the same name-family across boards. Confidence
   folds three signals: how *tight* the scores are, how *many* accounts,
   and on how *many* boards the family recurs. Emitted as a separate
   top-level ``clusters`` list (not per-player evidence) since the unit
   of suspicion is the family, not one account.

Each piece of per-player evidence is a self-describing dict: the measured
value, the peer baseline it's compared against, the statistical
magnitude, the threshold, and a human-readable interpretation.

Returns ``{players: [...], clusters: [...], computed_at, anchor, method,
config, total_flagged, boards_analyzed}``. The result is cached in
process memory keyed by ``(anchor, *all_config_values)`` so a tunable
change auto-invalidates the cache.
"""
from __future__ import annotations

import asyncio
import logging
import math
import re
import statistics
import time
from datetime import UTC, datetime, timedelta

from app.trove.leaderboards import service as lb_service

logger = logging.getLogger(__name__)


# Cache: {cache_key: (stored_at_unix, payload)}.
_CACHE: dict[tuple, tuple[float, dict]] = {}

# Co-movement is heavier (multi-capture) and changes slowly, so it's computed
# at most every ``cheaters_comovement_recompute_seconds`` and cached here by
# (week_start, config); the per-snapshot _compute reuses it across warm cycles.
# {"key": tuple, "computed_at": float, "clusters": list[dict]}
_COMOVEMENT_CACHE: dict | None = None

# Pull enough entries per board to cover the realistic upper bound. The bot's
# dump now carries up to ~20k entries per board (was ~5k), so fetch past that to
# analyse the FULL board rather than truncating cheater detection to the top.
_BOARD_FETCH_LIMIT = 25000
# Limit historical "peer" velocity computation to the top-N entries on
# each board so a 5000-row board doesn't trigger 5000 archive queries.
_VELOCITY_PEER_TOP_N = 50


# Last successful computation, regardless of anchor. Survives anchor
# changes so an on-the-fly cache miss can still serve "the previous good
# answer" while the warmer is busy recomputing for the new anchor - the
# alternative is a multi-second wait for the first visitor after each
# leaderboard ingest. See the "stale-but-known-good" branch in
# ``detect_possible_cheaters`` below.
_LAST_GOOD: dict | None = None


async def detect_possible_cheaters(*, force: bool = False) -> dict:
    """Run all three checks against a leaderboard snapshot.

    Two modes:

    * **Serving** (``force=False``) returns the latest *published* anchor's
      result - the one the page is showing (see ``cache.set_ready_anchor``) - so
      the cheaters tab always matches the boards/entries on screen. Served from,
      in order: the in-process cache, the persisted Redis snapshot (survives
      restarts, so no cold scan), the last-good payload, then an empty
      placeholder while the warmer fills in. Never blocks on the per-board scan.
    * **Warming** (``force=True``, the background warmer) computes the RAW latest
      anchor, writing the in-process cache + last-good + the persistent Redis
      snapshot. It ADOPTS a fresh persisted snapshot for that anchor instead of
      recomputing (e.g. right after a restart with no new capture), so boot
      doesn't pay a cold full-board scan.

    Without Redis configured, ``get_ready_anchor`` is None and this degrades to
    the old "serve the raw latest, stale-but-good" behaviour.
    """
    from app.admin import runtime_config
    from app.trove.leaderboards import cache as lb_cache

    z_threshold = float(await runtime_config.get_setting("cheaters_z_threshold"))
    velocity_multiplier = float(
        await runtime_config.get_setting("cheaters_velocity_multiplier")
    )
    min_board_size = int(await runtime_config.get_setting("cheaters_min_board_size"))
    cache_ttl = float(await runtime_config.get_setting("cheaters_cache_ttl_seconds"))
    cohort_pct = float(
        await runtime_config.get_setting("cheaters_elite_cohort_pct")
    )
    cluster_min_size = int(
        await runtime_config.get_setting("cheaters_cluster_min_size")
    )
    cluster_band_pct = float(
        await runtime_config.get_setting("cheaters_cluster_score_band_pct")
    )
    cluster_max_edit = int(
        await runtime_config.get_setting("cheaters_cluster_max_edit_distance")
    )
    cluster_size_full = int(
        await runtime_config.get_setting("cheaters_cluster_size_full")
    )
    excluded_csv = str(
        await runtime_config.get_setting("cheaters_excluded_board_uuids")
    )
    excluded = _parse_excluded(excluded_csv)
    # Alt-cluster detection has its OWN, independent board blacklist - the
    # per-player checks and the cluster check exclude different boards.
    cluster_excluded_csv = str(
        await runtime_config.get_setting("cheaters_cluster_excluded_board_uuids")
    )
    cluster_excluded = _parse_excluded(cluster_excluded_csv)
    # Co-movement (the primary, name-agnostic signal) knobs.
    cm = {
        "candidate_top_n": int(await runtime_config.get_setting("cheaters_comovement_candidate_top_n")),
        "min_hourly_gain": float(await runtime_config.get_setting("cheaters_comovement_min_hourly_gain")),
        "gain_percentile": float(await runtime_config.get_setting("cheaters_comovement_gain_percentile")),
        "gain_tolerance": float(await runtime_config.get_setting("cheaters_comovement_gain_tolerance_pct")),
        "min_matching_hours": int(await runtime_config.get_setting("cheaters_comovement_min_matching_hours")),
        "min_match_ratio": float(await runtime_config.get_setting("cheaters_comovement_min_match_ratio")),
        "min_density": float(await runtime_config.get_setting("cheaters_comovement_min_density")),
        "min_group_size": int(await runtime_config.get_setting("cheaters_comovement_min_group_size")),
        "max_cell_accounts": int(await runtime_config.get_setting("cheaters_comovement_max_cell_accounts")),
        "recompute_seconds": int(await runtime_config.get_setting("cheaters_comovement_recompute_seconds")),
        # Schedule correlation + fusion knobs (share the history load).
        "schedule_min_active_hours": int(await runtime_config.get_setting("cheaters_schedule_min_active_hours")),
        "schedule_min_similarity": float(await runtime_config.get_setting("cheaters_schedule_min_similarity")),
        "fusion_corroboration_bonus": float(await runtime_config.get_setting("cheaters_fusion_corroboration_bonus")),
        "footprint_min_jaccard": float(await runtime_config.get_setting("cheaters_footprint_min_jaccard")),
        # Per-player weekly uptime (no-sleep) check.
        "weekly_uptime_fraction": float(await runtime_config.get_setting("cheaters_weekly_uptime_fraction")),
    }
    now = time.time()
    global _LAST_GOOD

    def _key(anchor: int) -> tuple:
        # Excluded sets as sorted tuples (sets aren't hashable) so a master-panel
        # edit invalidates immediately. Co-movement/schedule/fusion knobs included
        # so a tweak invalidates the served result (recompute_seconds excluded -
        # it only throttles, doesn't change the answer).
        return (anchor, z_threshold, velocity_multiplier, min_board_size,
                cohort_pct, cluster_min_size, cluster_band_pct,
                cluster_max_edit, cluster_size_full, tuple(sorted(excluded)),
                tuple(sorted(cluster_excluded)),
                cm["candidate_top_n"], cm["min_hourly_gain"], cm["gain_percentile"],
                cm["gain_tolerance"], cm["min_matching_hours"], cm["min_match_ratio"],
                cm["min_density"], cm["min_group_size"], cm["max_cell_accounts"],
                cm["schedule_min_active_hours"], cm["schedule_min_similarity"],
                cm["fusion_corroboration_bonus"], cm["footprint_min_jaccard"],
                cm["weekly_uptime_fraction"])

    if not force:
        # Serve the latest PUBLISHED anchor (raw latest before the first publish
        # / without Redis).
        serve_anchor = await lb_cache.get_ready_anchor()
        if serve_anchor is None:
            ts = await lb_service.list_timestamps(limit=1, include_archive=False)
            serve_anchor = ts[0] if ts else None
        if serve_anchor is None:
            return _empty(None, z_threshold, velocity_multiplier, min_board_size)

        cache_key = _key(serve_anchor)
        hit = _CACHE.get(cache_key)
        if hit is not None and now - hit[0] < cache_ttl:
            _LAST_GOOD = hit[1]
            return hit[1]
        # Persisted snapshot survives restarts - serve instantly + seed memory.
        persisted = await lb_cache.get_cheaters(serve_anchor)
        if persisted is not None:
            _CACHE[cache_key] = (now, persisted)
            _LAST_GOOD = persisted
            return persisted
        # Nothing for the published anchor yet - kick the warmer and serve the
        # last-good payload (or an empty placeholder) instead of blocking.
        trigger_warmer()
        if _LAST_GOOD is not None:
            return _LAST_GOOD
        return _empty(serve_anchor, z_threshold, velocity_multiplier, min_board_size)

    # Warmer: compute (or adopt a fresh persisted snapshot for) the RAW latest.
    timestamps = await lb_service.list_timestamps(limit=1, include_archive=False)
    if not timestamps:
        return _empty(None, z_threshold, velocity_multiplier, min_board_size)
    anchor = timestamps[0]
    cache_key = _key(anchor)

    persisted = await lb_cache.get_cheaters(anchor)
    if persisted is not None and now - persisted.get("computed_at", 0) < cache_ttl:
        # Fresh processed snapshot already in Redis (restart, no new capture) -
        # adopt it instead of a cold recompute.
        _CACHE[cache_key] = (now, persisted)
        _LAST_GOOD = persisted
        return persisted

    result = await _compute(
        anchor, z_threshold, velocity_multiplier, min_board_size,
        excluded, cohort_pct,
        cluster_min_size=cluster_min_size,
        cluster_band_pct=cluster_band_pct,
        cluster_max_edit=cluster_max_edit,
        cluster_size_full=cluster_size_full,
        cluster_excluded=cluster_excluded,
        comovement=cm,
    )
    _CACHE[cache_key] = (now, result)
    _LAST_GOOD = result
    await lb_cache.set_cheaters(anchor, result)
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


def reset() -> None:
    """Drop ALL in-process cheater state (the keyed cache + the last-good
    fallback) for a full leaderboards reset, so nothing can serve a flag
    computed against the wiped data."""
    global _LAST_GOOD, _COMOVEMENT_CACHE
    _CACHE.clear()
    _LAST_GOOD = None
    _COMOVEMENT_CACHE = None


# ─── Background warmer ────────────────────────────────────────────────
# Pre-computes every heavy leaderboards-page query at app boot + on every
# TTL boundary + immediately after a new ingest so the public endpoints
# always return a cached result instantly - no caller ever waits for the
# per-board scan + history queries to run synchronously.
#
# Three caches kept warm in lock-step:
#   • detect_possible_cheaters         - cheaters tab
#   • activity.estimate_active_players - live-pulse pill in the hero
#   • lb_service.list_boards_at(latest) - sidebar of the boards tab
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
        # Sleep with an early-wake escape hatch - trigger_warmer() sets
        # the event when a new ingest lands so the new anchor's caches
        # start filling immediately instead of after a full TTL.
        try:
            await asyncio.wait_for(_wake_event.wait(), timeout=next_sleep)
        except TimeoutError:
            pass
        except asyncio.CancelledError:
            raise
        _wake_event.clear()


async def _warm_all() -> None:
    """One iteration of the warmer - re-runs every heavy query that
    feeds the leaderboards page. Runs sequentially (not concurrently)
    because they hit overlapping Mongo collections and we'd rather not
    interleave their cursors."""
    # cheaters detection - the slowest, run first so it's ready ASAP.
    # ``force=True`` makes the call bypass the stale-payload shortcut so
    # the NEW anchor's cache slot actually gets populated. Without this
    # the warmer would see _LAST_GOOD (the previous anchor's payload)
    # and return that without ever calling _compute() - and the user
    # would be stuck looking at yesterday's flags forever.
    res = await detect_possible_cheaters(force=True)
    # activity estimate - also a multi-board scan. NON-FATAL: it's the auxiliary
    # "live pulse" pill, so a failure here must not abort the pass before the
    # publish below (which would freeze the atomic snapshot switch). The pill
    # falls back to last-good/empty and the next pass retries it.
    from app.trove.leaderboards import activity as lb_activity
    try:
        await lb_activity.estimate_active_players(force=True)
    except Exception:
        logger.exception("leaderboards warmer: activity estimate failed (non-fatal)")
    # Per-class activity (Class Activity page) - same non-fatal treatment; only
    # the 36 Effort/Paragon boards load, so it's cheap.
    from app.trove.leaderboards import class_activity as lb_class_activity
    try:
        await lb_class_activity.estimate_class_activity(force=True)
    except Exception:
        logger.exception("leaderboards warmer: class activity estimate failed (non-fatal)")
    # Refresh the Redis snapshot for the leaderboards page (anchor list + boards
    # at the latest anchor + the first board's first page) so the page serves
    # the latest capture with zero Mongo work and can switch to a new capture
    # instantly. Without Redis this still warms Mongo's page cache as before.
    from app.trove.leaderboards import cache as lb_cache
    await lb_cache.warm()
    # PUBLISH: advance the page's "latest" to this anchor only now that its
    # cheaters + activity + boards/entries are all cached. Until this flips,
    # get_timestamps hides the freshly-ingested anchor, so the page switches to
    # a new capture atomically (never showing a half-processed snapshot).
    anchor = res.get("anchor") if isinstance(res, dict) else None
    if anchor is not None:
        await lb_cache.set_ready_anchor(anchor)
        # Pre-warm each board's default chart (7d / top-5) so selecting a board on
        # the page paints from Redis. AFTER publish + non-fatal, so it never delays
        # the atomic snapshot switch and a slow/failed warm doesn't freeze the page.
        try:
            warmed = await lb_cache.warm_board_histories(anchor)
            logger.info("leaderboards warmer: pre-warmed %d board-history charts", warmed)
        except Exception:
            logger.exception("leaderboards warmer: board-history pre-warm failed (non-fatal)")


def trigger_warmer() -> None:
    """Wake the warmer loop early - call after each successful
    leaderboard ingest so the new anchor's caches start filling
    immediately instead of after the TTL. No-op if the warmer hasn't
    been started yet."""
    if _wake_event is None:
        return
    try:
        _wake_event.set()
    except RuntimeError:
        # event loop closed (during shutdown) - nothing to do
        pass


def start_leaderboards_warmer() -> None:
    """Kick off the background warmer if it isn't already running.
    Idempotent - safe to call multiple times."""
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


# Back-compat aliases - the warmer used to only handle cheaters. Existing
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
    *,
    cluster_min_size: int = 3,
    cluster_band_pct: float = 0.02,
    cluster_max_edit: int = 2,
    cluster_size_full: int = 8,
    cluster_excluded: set[int] | None = None,
    comovement: dict | None = None,
) -> dict:
    boards = await lb_service.list_boards_at(anchor)
    excluded = excluded or set()
    # One projected query for the WHOLE anchor (plain dicts, no per-board
    # round-trip, no count, no Beanie/Pydantic hydration) instead of a
    # list_entries() call per board - the dominant cost on a million-row capture.
    by_board = await lb_service.entries_by_board_at(anchor, [b["uuid"] for b in boards])

    # player_name → {board_uuid → board_entry_dict}
    flagged: dict[str, dict[int, dict]] = {}
    boards_analyzed = 0
    boards_excluded = 0
    # Full meta of every board the analysis touched. Surfaced in the response
    # so the showcase site's /leaderboards cheaters tab can show exactly which
    # boards were scanned (transparency: a reader can verify the analysis
    # covered the boards they care about, instead of taking "N boards" on
    # faith). Per-board ``reset_kind`` is the effective value (admin override
    # already applied), so a board pinned to "none" via the admin panel shows
    # up correctly even when the hardcoded mapping says daily/weekly.
    analyzed_boards_meta: list[dict] = []
    excluded_boards_meta: list[dict] = []

    from app.trove.leaderboards.models import is_lifetime_kind

    def _board_meta(b: dict) -> dict:
        return {
            "uuid": b["uuid"],
            "name": b.get("name") or b.get("name_id") or str(b["uuid"]),
            "category": b.get("category") or b.get("category_id") or "",
            "reset_kind": b.get("reset_kind", "default"),
            "contest_type": b.get("contest_type"),
        }

    for board in boards:
        if board["uuid"] in excluded:
            boards_excluded += 1
            excluded_boards_meta.append({**_board_meta(board), "reason": "admin_excluded"})
            continue
        entries = by_board.get(board["uuid"], [])
        if len(entries) < min_board_size:
            # Too few entries for robust statistics - skip but record so the UI
            # can explain why a board the user expected to see is absent.
            excluded_boards_meta.append({
                **_board_meta(board),
                "reason": "below_min_size",
                "entries": len(entries),
            })
            continue
        boards_analyzed += 1
        analyzed_boards_meta.append({**_board_meta(board), "entries": len(entries)})

        higher_is_better = _detect_direction(entries)

        # Resetting vs. lifetime gating:
        #   • Resetting boards (daily/weekly) - every cycle starts at
        #     zero, so a rank-1 score that's an order of magnitude
        #     above rank-2 is a meaningful anomaly. Score-outlier +
        #     rank-gap + velocity all carry signal.
        #   • Lifetime boards (default / explicit "none") - score
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

    # Alt-cluster pass - a GROUP check, so it runs over the whole anchor after
    # the per-player loop. Uses its OWN board blacklist (``cluster_excluded``),
    # independent of the per-player ``excluded`` set. Independent of
    # ``min_board_size`` (clustering is robust on tiny boards) and of the
    # lifetime/resetting gate (an alt army shows up on every board kind).
    #
    # Offloaded to a worker thread: it's a pure-CPU, no-await pass over every
    # entry in the anchor (name-stem grouping + score-band scan), and the
    # warmer runs on the API's event loop. Running it inline would block every
    # request for its duration; ``to_thread`` lets the loop interleave request
    # handling between the GIL's periodic releases. ``_detect_clusters`` only
    # READS the already-materialised ``boards``/``by_board`` and shares no
    # mutable state, so it's safe to run off-thread.
    name_clusters, clusters_boards_scanned = await asyncio.to_thread(
        _detect_clusters, boards, by_board, cluster_excluded or set(),
        min_size=cluster_min_size,
        band_pct=cluster_band_pct,
        max_edit=cluster_max_edit,
        size_full=cluster_size_full,
    )

    # History-based signals (the PRIMARY, name-agnostic ones): co-movement
    # (lockstep gains) + schedule correlation (same active/idle hours). Heavier
    # (multi-capture) + slow-changing, so the whole pass is throttled + cached
    # separately and reused across warm cycles. Returns the producer outputs +
    # per-account features (active hours, board sets) for fusion.
    cmcfg = comovement or {}
    signals = await _comovement_clusters(
        anchor, by_board, boards, cluster_excluded or set(), cmcfg,
    )

    # Fold the per-player WEEKLY uptime flags (computed from the same week data)
    # into the per-player evidence, so the cheaters tab reflects week-long
    # behaviour - not just the last hour's velocity.
    board_by_uuid = {b["uuid"]: b for b in boards}
    for pf in signals.get("player_flags", []):
        board = board_by_uuid.get(pf["board_uuid"])
        if board is None:
            continue
        entry = next((e for e in by_board.get(pf["board_uuid"], [])
                      if e["player_name"] == pf["player_name"]), None)
        if entry is None:
            continue
        pct_active = pf["active_frac"] * 100.0
        _add_evidence(flagged, pf["player_name"], board, entry, {
            "type": "sustained_velocity",
            "summary": (
                f"Score rose in {pf['active_hours']} of the last {pf['num_slots']} "
                f"hourly captures since the weekly reset ({pct_active:.0f}% uptime). "
                f"No human plays {pf['threshold_frac'] * 100:.0f}%+ of every hour "
                f"for days - this account essentially never stops, the signature of "
                f"a no-sleep bot. Invisible to the per-hour check (each hour looks "
                f"normal alone)."
            ),
            "measurements": {
                "active_hours": pf["active_hours"],
                "captures_since_reset": pf["num_slots"],
                "uptime_fraction": round(pf["active_frac"], 3),
                "threshold_fraction": pf["threshold_frac"],
            },
        })

    # FUSION: merge name-stem + co-movement + schedule clusters by member
    # overlap, then score each by how many INDEPENDENT signals agree on it.
    clusters = _fuse(name_clusters, signals, cmcfg)

    return _format(
        flagged, anchor, z_threshold, velocity_multiplier,
        min_board_size, boards_analyzed, boards_excluded,
        analyzed_boards=analyzed_boards_meta,
        excluded_boards=excluded_boards_meta,
        clusters=clusters,
        clusters_boards_scanned=clusters_boards_scanned,
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
       Trove leaderboards are heavy-tailed - the top 1 % typically
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
    2. Skip the board if the top 5 cohort scores are tied - that's a
       capped board (e.g. class boards at 59 731) where the elite has
       no spread to measure against.
    3. log10(score + 1) on the cohort. Compute median + MAD.
    4. Only check the **top half** of the cohort. Outside this slice
       a player can't be "the suspicious top score" by definition.
    5. Flag in the board's "better" direction only - we don't care
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
        # - easier to read than "5.2 log-z-scores above log-median".
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

    # Baseline: median of the tail gaps (after the top 3) - that's the
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
    peers = ranked[:_VELOCITY_PEER_TOP_N]
    # Prefetch every needed previous-capture in ONE query per collection (the
    # top-N peers that seed the baseline PLUS the flagged players themselves)
    # instead of a find_one round-trip per name.
    need = {e["player_name"] for e in peers} | flagged_on_board
    prev_map = await _previous_captures_bulk(
        list(need), board["uuid"], anchor, reset_kind,
    )

    peer_velocities: list[float] = []
    # Build the peer baseline from the top-N entries that actually have
    # historical data IN THE SAME RESET CYCLE.
    for e in peers:
        prev = prev_map.get(e["player_name"])
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
        prev = prev_map.get(name)
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
                f"{_fmt(peer_p95)}/h - this player is {v / peer_p95:.0f}× faster."
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
    - ``"default"`` boards don't reset (accumulate forever) - we
      return 0 so any prior anchor passes the cycle-membership check.

    Critical for the velocity check: comparing a score across a reset
    boundary is meaningless. Pre-reset Δ-score is negative (already
    filtered) but post-reset "starting fresh and grinding hard" can
    masquerade as a velocity outlier vs a peer baseline that includes
    pre-reset velocities - this guard prevents that.
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
    # Unknown reset_kind - fail safe by allowing any prior anchor.
    return 0


async def _previous_captures_bulk(
    player_names: list[str], board_uuid: int, current_anchor: int,
    reset_kind: str = "default",
) -> dict[str, tuple[float, int]]:
    """Most recent ``(score, anchor)`` strictly before ``current_anchor`` AND
    within the same reset cycle, for MANY players on one board at once.

    Reset-cycle filtering (daily/weekly only consider anchors after the most-
    recent reset) keeps velocity from being computed across an "everyone went
    back to zero" boundary. One Postgres query (``DISTINCT ON`` picks the newest
    qualifying row per player).
    """
    if not player_names:
        return {}
    from app.trove.leaderboards import pg_store
    cycle_start = _reset_boundary_before(current_anchor, reset_kind)
    return await pg_store.previous_captures_bulk(
        player_names, board_uuid, current_anchor, cycle_start,
    )


# ─── Alt-cluster detection ─────────────────────────────────────────────
# A group-shaped check: catch coordinated multi-account ("alt army")
# patterns where many similarly-named accounts share near-identical
# scores. None of the per-player checks above can see this - each alt is
# unremarkable on its own; only the *family* is anomalous.

# Cluster confidence is capped here. The signal is very reliable when
# scores are near-identical AND names share a constructed stem (random
# players don't name themselves ``foo1 … foo20`` at the same score), but
# we keep it below 1.0 so a rare coincidence (two friends "Dragon" /
# "Dragon2" who happen to be close) can't pin to certainty.
_ALT_CONFIDENCE_CEILING = 0.95

# Strip a trailing run of digits + common separators so numbered alts
# collapse to one stem: "anana17" → "anana", "Aan_7" → "aan",
# "Player 001" → "player". A name with no trailing-number suffix
# ("woshiahuang") is left untouched.
_STEM_SUFFIX_RE = re.compile(r"[\s_\-.#]*\d[\d\s_\-.#]*$")

# Leet / number-substitution → canonical letters so "Dr4g0n", "Dr4g0n2" and
# "dragon" all collapse to one stem. Applied AFTER the trailing-counter strip,
# so "anana17" still stems to "anana" (not "ananait").
_LEET_MAP = str.maketrans({
    "4": "a", "3": "e", "1": "i", "0": "o", "5": "s", "7": "t",
    "8": "b", "9": "g", "@": "a", "$": "s", "!": "i", "|": "i",
})


def _name_stem(name: str) -> str:
    base = _STEM_SUFFIX_RE.sub("", name.strip().lower())
    return base.translate(_LEET_MAP)


def _edit_distance_le(a: str, b: str, max_d: int) -> bool:
    """True iff Levenshtein(a, b) ≤ ``max_d``. Banded DP with an
    early-out the moment a whole row exceeds the budget, so it stays
    cheap on the short strings (name stems) we feed it."""
    la, lb = len(a), len(b)
    if abs(la - lb) > max_d:
        return False
    if a == b:
        return True
    prev = list(range(lb + 1))
    for i in range(1, la + 1):
        cur = [i] + [0] * lb
        ca = a[i - 1]
        row_min = cur[0]
        for j in range(1, lb + 1):
            cost = 0 if ca == b[j - 1] else 1
            cur[j] = min(prev[j] + 1, cur[j - 1] + 1, prev[j - 1] + cost)
            if cur[j] < row_min:
                row_min = cur[j]
        if row_min > max_d:
            return False
        prev = cur
    return prev[lb] <= max_d


# Don't run the O(n²) edit-distance merge on a block bigger than this -
# exact-stem grouping still catches numbered alts; the fuzzy merge is a
# secondary catch for typo'd variants and isn't worth a pathological scan.
_STEM_BLOCK_CAP = 300


def _merge_stems(
    stems: list[str], max_edit: int, candidates: list[str] | None = None,
) -> dict[str, str]:
    """Union near-identical stems into one canonical key via edit
    distance, so typo'd alt families (``anana`` / ``anan`` / ``annna``)
    collapse together. Returns ``{stem: canonical_stem}``.

    Only ``candidates`` (default: every stem) are eligible for the
    quadratic fuzzy merge. Callers pass the small set of stems that
    already look alt-like (an exact group with ≥2 names) so a board with
    tens of thousands of *distinct* player names can't trigger an O(n²)
    scan over all of them - the dominant numbered-alt pattern
    (``anana1 … anana20``) already collapses to one EXACT stem and needs
    no fuzzy step. Non-candidate stems always map to themselves.

    To avoid an all-pairs scan within the pool, stems are blocked by
    ``(first char, length)`` - two stems within ``max_edit`` edits must
    agree on the first char (for small edits) and have near-equal length,
    so they land in the same or an adjacent block. ``max_edit <= 0``
    disables fuzzy merging.
    """
    parent = {s: s for s in stems}

    def find(x: str) -> str:
        while parent[x] != x:
            parent[x] = parent[parent[x]]
            x = parent[x]
        return x

    def union(a: str, b: str) -> None:
        ra, rb = find(a), find(b)
        if ra != rb:
            # Canonicalise to the lexicographically smaller stem for stable keys.
            parent[max(ra, rb)] = min(ra, rb)

    if max_edit > 0:
        pool = candidates if candidates is not None else stems
        blocks: dict[tuple, list[str]] = {}
        for s in pool:
            if not s:
                continue
            blocks.setdefault((s[0], len(s)), []).append(s)
        # Compare each stem against its own block and the ±1-length
        # neighbour blocks (an insert/delete shifts length by one).
        for (c, ln), group in blocks.items():
            cand = group + blocks.get((c, ln + 1), [])
            if len(cand) > _STEM_BLOCK_CAP:
                continue
            for i, a in enumerate(group):
                for b in cand[i + 1:]:
                    if a != b and _edit_distance_le(a, b, max_edit):
                        union(a, b)

    return {s: find(s) for s in stems}


def _densest_band(
    items: list[dict], band_pct: float, min_size: int,
) -> list[dict] | None:
    """Largest run of entries (sorted by score) whose total relative
    spread ``(hi-lo)/max(|hi|,|lo|)`` stays within ``band_pct``. Returns
    that subset, or None if no run reaches ``min_size``. One entry per
    distinct player (best rank kept) so the same name can't pad a band."""
    by_name: dict[str, dict] = {}
    for e in items:
        cur = by_name.get(e["player_name"])
        if cur is None or e["rank"] < cur["rank"]:
            by_name[e["player_name"]] = e
    uniq = sorted(by_name.values(), key=lambda e: e["score"])
    n = len(uniq)
    if n < min_size:
        return None
    left = 0
    best: tuple[int, int] | None = None
    for right in range(n):
        while left < right:
            lo = uniq[left]["score"]
            hi = uniq[right]["score"]
            denom = max(abs(hi), abs(lo), 1e-9)
            if (hi - lo) / denom <= band_pct:
                break
            left += 1
        size = right - left + 1
        if size >= min_size and (best is None or size > best[1] - best[0] + 1):
            best = (left, right)
    return uniq[best[0]:best[1] + 1] if best else None


def _detect_clusters(
    boards: list[dict], by_board: dict[int, list[dict]],
    excluded: set[int] | None,
    *, min_size: int, band_pct: float, max_edit: int, size_full: int,
) -> tuple[list[dict], int]:
    """Find coordinated alt clusters across the whole anchor.

    Per board: group entries by name stem, fuzzy-merge near stems, then
    keep each family's densest near-score subset (≥ ``min_size``). Across
    boards: merge occurrences of the same family. Confidence folds score
    tightness, family size, and board count. ``excluded`` is the cluster
    check's OWN board blacklist (distinct from the per-player one).

    Returns ``(clusters, boards_scanned)`` - the second value is how many
    boards the cluster pass actually examined (not excluded, ≥ ``min_size``
    entries), so the UI can report a cluster-accurate count instead of the
    per-player ``boards_analyzed`` (which is gated by a different threshold).
    """
    excluded = excluded or set()
    # canonical_stem → {"members": set[str], "boards": list[occurrence dict]}
    family: dict[str, dict] = {}
    boards_scanned = 0

    for board in boards:
        uuid = board["uuid"]
        if uuid in excluded:
            continue
        entries = by_board.get(uuid, [])
        if len(entries) < min_size:
            continue
        boards_scanned += 1

        # Exact-stem grouping, then a fuzzy merge of near stems.
        stem_groups: dict[str, list[dict]] = {}
        for e in entries:
            stem = _name_stem(e["player_name"])
            if len(stem) < 2:
                continue  # too short to be a meaningful family key
            stem_groups.setdefault(stem, []).append(e)
        if not stem_groups:
            continue
        # Only stems whose EXACT group already has ≥2 distinct names are
        # eligible for the quadratic fuzzy merge - on a board of tens of
        # thousands of unique names that pool is tiny, so the edit-distance
        # step stays cheap. (A purely typo'd family where every variant
        # appears exactly once is the rare miss; numbered alts share one
        # exact stem and are caught regardless.)
        candidate_stems = [
            s for s, ents in stem_groups.items()
            if len({e["player_name"] for e in ents}) >= 2
        ]
        canon = _merge_stems(list(stem_groups.keys()), max_edit, candidate_stems)
        merged: dict[str, list[dict]] = {}
        for stem, ents in stem_groups.items():
            merged.setdefault(canon.get(stem, stem), []).extend(ents)

        for cstem, ents in merged.items():
            if len({e["player_name"] for e in ents}) < min_size:
                continue
            subset = _densest_band(ents, band_pct, min_size)
            if subset is None:
                continue
            scores = [e["score"] for e in subset]
            ranks = [e["rank"] for e in subset]
            lo, hi = min(scores), max(scores)
            spread = (hi - lo) / max(abs(hi), abs(lo), 1e-9)
            fam = family.setdefault(cstem, {"members": set(), "boards": []})
            fam["members"].update(e["player_name"] for e in subset)
            fam["boards"].append({
                "uuid": uuid,
                "name": board.get("name") or str(uuid),
                "category": board.get("category") or board.get("category_id") or "",
                "contest_type": board.get("contest_type"),
                "members": len(subset),
                "member_names": sorted({e["player_name"] for e in subset})[:60],
                "score_min": _round(lo),
                "score_max": _round(hi),
                "spread": round(spread, 6),
                "rank_min": min(ranks),
                "rank_max": max(ranks),
            })

    clusters: list[dict] = []
    for cstem, fam in family.items():
        boards_occ = fam["boards"]
        member_count = len(fam["members"])
        if member_count < min_size or not boards_occ:
            continue
        conf, terms = _cluster_confidence(
            boards_occ, member_count, band_pct, min_size, size_full,
        )
        member_names = sorted(fam["members"])
        cap = 60
        clusters.append({
            "stem": cstem,
            "label": f"{cstem}*",
            "method": "name_stem",
            "member_count": member_count,
            "members": member_names[:cap],
            "members_truncated": max(0, member_count - cap),
            "board_count": len(boards_occ),
            "boards": sorted(
                boards_occ,
                key=lambda b: ((b.get("category") or "").lower(), (b.get("name") or "").lower()),
            ),
            "confidence": conf,
            "summary": _cluster_summary(cstem, member_names, member_count, boards_occ, terms),
            "measurements": terms,
        })

    # Most-suspicious-first, then biggest family as a tiebreaker.
    clusters.sort(key=lambda c: (-c["confidence"], -c["member_count"]))
    return clusters, boards_scanned


def _cluster_confidence(
    boards_occ: list[dict], member_count: int,
    band_pct: float, min_size: int, size_full: int,
) -> tuple[float, dict]:
    """Blend the three signals the feature is built around:

    * **closeness** - tightest board's score spread vs the band. Spread 0
      → 1.0; spread == band → 0.0. This is the dominant term: near-
      identical scores among similarly-named accounts is the core tell.
    * **size_term** - ramps 0→1 as the family grows from ``min_size`` to
      ``size_full`` accounts. More alts = more blatant.
    * **board_term** - ``1 - 0.5^board_count`` (1 board → 0.5, 2 → 0.75,
      3 → 0.875). A family clustered on several boards is far less likely
      to be coincidence.

    Sub-terms are returned so the response can show exactly how a
    confidence was reached (same transparency contract as the per-player
    evidence ``measurements``).
    """
    spreads = [b["spread"] for b in boards_occ]
    tightest = min(spreads) if spreads else band_pct
    closeness = max(0.0, 1.0 - (tightest / band_pct)) if band_pct > 0 else 0.0
    denom = max(1, size_full - min_size)
    size_term = min(1.0, max(0.0, (member_count - min_size) / denom))
    board_term = 1.0 - 0.5 ** len(boards_occ)

    raw = 0.5 + 0.49 * (0.50 * closeness + 0.25 * size_term + 0.25 * board_term)
    conf = round(min(_ALT_CONFIDENCE_CEILING, raw), 3)
    return conf, {
        "member_count": member_count,
        "board_count": len(boards_occ),
        "tightest_spread_pct": round(tightest * 100.0, 4),
        "score_band_pct": round(band_pct * 100.0, 4),
        "closeness": round(closeness, 3),
        "size_term": round(size_term, 3),
        "board_term": round(board_term, 3),
        "ceiling": _ALT_CONFIDENCE_CEILING,
    }


def _cluster_summary(
    cstem: str, member_names: list[str], member_count: int,
    boards_occ: list[dict], terms: dict,
) -> str:
    """Human-readable interpretation, rendered verbatim by the site (same
    convention as the per-player evidence summaries)."""
    if member_count <= 4:
        sample = ", ".join(member_names)
    else:
        sample = ", ".join(member_names[:3]) + f", … {member_names[-1]}"
    board_count = len(boards_occ)
    return (
        f"{member_count} similarly-named accounts ({sample}) cluster within "
        f"{terms['tightest_spread_pct']:.3g}% of each other on "
        f"{board_count} board(s). Coordinated multi-account ('alt army') "
        f"pattern: near-identical scores under a shared name stem '{cstem}'."
    )


# ─── Co-movement detection (the primary, name-agnostic signal) ─────────
# Accounts whose hourly score GAINS land in the same bucket in the same
# hours, across the captures since the last weekly reset, are progressing
# in lockstep - the signature of one operator running alts/bots together.
# Name similarity is only an optional confidence boost on top.
#
# Bounded: per board only the top-N by rank get their week-long series
# loaded; grouping is via an inverted (hour, gain-bucket) -> accounts index
# + capped within-cell co-occurrence + union-find (NO all-pairs over the
# whole board). The heavy, slow-changing pass is throttled + cached.

# Co-movement confidence is capped here. Lockstep gains across many hours are a
# very strong, hard-to-fake signal, but kept < 1.0 (a popular event can make a
# few accounts coincidentally match for a while).
_COMOVEMENT_CONFIDENCE_CEILING = 0.97


_EMPTY_SIGNALS = {"comovement": [], "schedule": [], "active_hours": {},
                  "board_sets": {}, "player_flags": []}


async def _comovement_clusters(
    anchor: int, by_board: dict[int, list[dict]], boards: list[dict],
    excluded: set[int], cfg: dict,
) -> dict:
    """Throttled wrapper: load the week's candidate series (async I/O) and run
    the pure-CPU history signals (co-movement + schedule) off the event loop.
    Returns ``{comovement, schedule, active_hours, board_sets}`` (the producer
    outputs + per-account features fusion needs). Cached by week-start + config
    so per-snapshot warm cycles reuse it within the throttle window."""
    global _COMOVEMENT_CACHE
    top_n = int(cfg.get("candidate_top_n", 0) or 0)
    if top_n <= 0:
        return _EMPTY_SIGNALS  # disabled
    min_match = int(cfg.get("min_matching_hours", 3))
    min_ratio = float(cfg.get("min_match_ratio", 0.7))
    min_density = float(cfg.get("min_density", 0.6))
    min_group = int(cfg.get("min_group_size", 2))
    min_gain = float(cfg.get("min_hourly_gain", 1.0))
    pct = float(cfg.get("gain_percentile", 0.90))
    tol = float(cfg.get("gain_tolerance", 0.05))
    max_cell = int(cfg.get("max_cell_accounts", 40))
    recompute_s = float(cfg.get("recompute_seconds", 3600))
    sched_min_hours = int(cfg.get("schedule_min_active_hours", 6))
    sched_min_sim = float(cfg.get("schedule_min_similarity", 0.8))
    uptime_frac = float(cfg.get("weekly_uptime_fraction", 0.85))

    week_start = _reset_boundary_before(anchor, "weekly")
    key = (week_start, top_n, min_gain, pct, tol, min_match, min_ratio,
           min_density, min_group, max_cell, sched_min_hours, sched_min_sim,
           uptime_frac, tuple(sorted(excluded)))
    now = time.time()
    cache = _COMOVEMENT_CACHE
    if cache is not None and cache["key"] == key and now - cache["computed_at"] < recompute_s:
        return cache["signals"]

    # Captures since the weekly reset (the window is capped at the week).
    all_ts = await lb_service.list_timestamps(limit=400)
    anchors = sorted(a for a in all_ts if week_start <= a <= anchor)
    if len(anchors) <= min_match:
        # Too few captures this week (early-week) - nothing to flag yet.
        _COMOVEMENT_CACHE = {"key": key, "computed_at": now, "signals": _EMPTY_SIGNALS}
        return _EMPTY_SIGNALS

    from app.trove.leaderboards import pg_store
    # Load each board's week-long candidate series CONCURRENTLY (bounded) instead
    # of ~85 sequential round-trips - the dominant wall-clock cost of the pass.
    sem = asyncio.Semaphore(8)

    async def _load(board: dict):
        uuid = board["uuid"]
        if uuid in excluded:
            return None
        entries = by_board.get(uuid, [])
        if len(entries) < min_group:
            return None
        candidates = [
            e["player_name"]
            for e in sorted(entries, key=lambda e: e["rank"])[:top_n]
        ]
        async with sem:
            series = await pg_store.comovement_series(uuid, candidates, week_start, anchor)
        return (uuid, board, series) if series else None

    series_by_board: dict[int, dict[str, dict[int, float]]] = {}
    meta_by_board: dict[int, dict] = {}
    for res in await asyncio.gather(*(_load(b) for b in boards)):
        if res is not None:
            uuid, board, series = res
            series_by_board[uuid] = series
            meta_by_board[uuid] = board

    t0 = time.time()
    signals = await asyncio.to_thread(
        _cluster_history_signals, series_by_board, meta_by_board, anchors,
        min_gain, pct, tol, min_match, min_ratio, min_density, min_group, max_cell,
        sched_min_hours, sched_min_sim, uptime_frac,
    )
    logger.info(
        "alt-signals: %d boards over %d captures -> %d co-movement + %d schedule "
        "clusters, %d weekly-uptime flags in %.2fs",
        len(series_by_board), len(anchors), len(signals["comovement"]),
        len(signals["schedule"]), len(signals["player_flags"]), time.time() - t0,
    )
    _COMOVEMENT_CACHE = {"key": key, "computed_at": now, "signals": signals}
    return signals


def _cluster_history_signals(
    series_by_board: dict[int, dict[str, dict[int, float]]],
    meta_by_board: dict[int, dict], anchors: list[int],
    min_gain: float, pct: float, tol: float, min_match: int, min_ratio: float,
    min_density: float, min_group: int, max_cell: int,
    sched_min_hours: int, sched_min_sim: float, uptime_frac: float,
) -> dict:
    """Pure CPU. Derive per-account features (active hours, board sets) from the
    loaded series, then run the history producers: co-movement (lockstep gains),
    schedule correlation (same active/idle hours), and the per-player weekly
    uptime (no-sleep) check."""
    slot_of = {a: i for i, a in enumerate(anchors)}
    active_hours: dict[str, set] = {}
    board_sets: dict[str, set] = {}
    for uuid, series in series_by_board.items():
        for name, sc in series.items():
            board_sets.setdefault(name, set()).add(uuid)
            pts = sorted((slot_of[a], v) for a, v in sc.items() if a in slot_of)
            for j in range(1, len(pts)):
                (s0, v0), (s1, v1) = pts[j - 1], pts[j]
                if s1 == s0 + 1 and (v1 - v0) >= min_gain:
                    active_hours.setdefault(name, set()).add(s1)

    comovement = _cluster_comovement(
        series_by_board, meta_by_board, anchors,
        min_gain, pct, tol, min_match, min_ratio, min_density, min_group, max_cell,
    )
    schedule = (
        _cluster_schedule(active_hours, board_sets, meta_by_board,
                          sched_min_hours, sched_min_sim, min_group, min_density,
                          max_cell, len(anchors))
        if sched_min_hours > 0 else []
    )
    player_flags = _weekly_uptime_flags(active_hours, board_sets, len(anchors) - 1, uptime_frac)
    return {
        "comovement": comovement, "schedule": schedule,
        "active_hours": active_hours, "board_sets": board_sets,
        "player_flags": player_flags,
    }


# Don't run the weekly uptime check until at least this many captures have
# elapsed since the reset - early in the week, "active fraction" is too noisy.
_WEEKLY_UPTIME_MIN_SLOTS = 48


def _weekly_uptime_flags(
    active_hours: dict[str, set], board_sets: dict[str, set],
    num_slots: int, uptime_frac: float,
) -> list[dict]:
    """Flag players active (score rose) in >= ``uptime_frac`` of the captures
    since the weekly reset. A human cannot play 85%+ of every hour for days; a
    no-sleep bot can. The per-hour velocity check can't see this - each hour
    looks normal in isolation - so it needs the full-week view."""
    if uptime_frac <= 0 or num_slots < _WEEKLY_UPTIME_MIN_SLOTS:
        return []
    flags: list[dict] = []
    for name, hrs in active_hours.items():
        frac = len(hrs) / num_slots
        if frac < uptime_frac:
            continue
        board_uuid = next(iter(board_sets.get(name, ())), None)
        if board_uuid is None:
            continue
        flags.append({
            "player_name": name, "board_uuid": board_uuid,
            "active_hours": len(hrs), "num_slots": num_slots,
            "active_frac": frac, "threshold_frac": uptime_frac,
        })
    return flags


def _cluster_comovement(
    series_by_board: dict[int, dict[str, dict[int, float]]],
    meta_by_board: dict[int, dict], anchors: list[int],
    min_gain: float, pct: float, tol: float,
    min_match: int, min_ratio: float, min_density: float,
    min_group: int, max_cell: int,
) -> list[dict]:
    """Pure CPU. Per board, diff each candidate's per-capture series into hourly
    gains, keep only the top-percentile gainers per hour, bucket them, and group
    accounts that move in lockstep. An edge requires shared (hour, bucket) cells
    that are BOTH an absolute count (``min_match``) AND a high fraction
    (``min_ratio``) of the rarer account's active hours - so a few coincidental
    matches across a long week can't chain the grinder crowd. Each formed
    component is then tightened to its dense core (``min_density``) so a loose
    transitive chain collapses. Groups are merged across boards by member
    overlap."""
    slot_of = {a: i for i, a in enumerate(anchors)}
    band = 1.0 + tol if tol > 0 else 1.0  # tolerance-band width

    board_groups: list[dict] = []  # {uuid, members:set, matching_hours, avg_gain}

    for uuid, series in series_by_board.items():
        # 1. hourly gains per slot: {slot: {name: gain}} (consecutive captures,
        #    gain >= the absolute floor).
        slot_gains: dict[int, dict[str, float]] = {}
        for name, sc in series.items():
            pts = sorted((slot_of[a], v) for a, v in sc.items() if a in slot_of)
            for j in range(1, len(pts)):
                (s0, v0), (s1, v1) = pts[j - 1], pts[j]
                if s1 != s0 + 1:
                    continue  # gap (missed capture) - not a comparable hour
                delta = v1 - v0
                if delta < min_gain:
                    continue
                slot_gains.setdefault(s1, {})[name] = delta

        # 2. per hour: keep only the top-percentile gainers (so a crowd at a
        #    common rate drops out), then group the survivors into tolerance
        #    bands by a SORTED SWEEP. Boundary-robust: two gains within `tol` of
        #    each other share a band regardless of absolute value - unlike fixed
        #    buckets, where a hair's-width difference can straddle an edge and
        #    split a real ring.
        cells: dict[tuple, set] = {}     # (slot, band#) -> {names in that band}
        acct_gain: dict[str, float] = {}
        for slot, gains in slot_gains.items():
            if 0.0 < pct < 1.0 and len(gains) >= 2:
                vals = sorted(gains.values())
                thr = vals[min(len(vals) - 1, int(pct * len(vals)))]
            else:
                thr = float("-inf")  # percentile gate disabled
            kept = sorted((g, nm) for nm, g in gains.items() if g >= thr)
            band_idx, band_anchor = 0, (kept[0][0] if kept else 0.0)
            for g, nm in kept:
                if g > band_anchor * band:
                    band_idx += 1
                    band_anchor = g
                cells.setdefault((slot, band_idx), set()).add(nm)
                acct_gain[nm] = acct_gain.get(nm, 0.0) + g

        parent: dict[str, str] = {}

        def find(x: str) -> str:
            parent.setdefault(x, x)
            while parent[x] != x:
                parent[x] = parent[parent[x]]
                x = parent[x]
            return x

        def union(a: str, b: str) -> None:
            ra, rb = find(a), find(b)
            if ra != rb:
                parent[max(ra, rb)] = min(ra, rb)

        co: dict[tuple, int] = {}          # pair -> matching (hour, bucket) cells
        active: dict[str, int] = {}        # name -> usable cells it appears in
        usable_cells: list[tuple] = []
        for cell, names in cells.items():
            # Skip common-event spikes (a bucket shared by too many accounts) -
            # noise, not a ring; also bounds the within-cell pair cost.
            if len(names) < 2 or len(names) > max_cell:
                continue
            usable_cells.append(cell)
            members = sorted(names)
            for m in members:
                active[m] = active.get(m, 0) + 1
            for i in range(len(members)):
                for k in range(i + 1, len(members)):
                    pair = (members[i], members[k])
                    co[pair] = co.get(pair, 0) + 1

        # Qualify edges: matches must be an absolute count AND a high fraction of
        # the rarer account's active hours (kills coincidental long-week chains).
        adj: dict[str, set] = {}
        any_edge = False
        for (a, b), c in co.items():
            if c < min_match:
                continue
            denom = min(active.get(a, 1), active.get(b, 1)) or 1
            if c / denom < min_ratio:
                continue
            union(a, b)
            adj.setdefault(a, set()).add(b)
            adj.setdefault(b, set()).add(a)
            any_edge = True
        if not any_edge:
            continue

        components: dict[str, set] = {}
        for node in adj:
            components.setdefault(find(node), set()).add(node)

        for mem in components.values():
            # Peel loose hangers-on so a transitive chain collapses to its dense
            # core (a real ring is near-clique; a chain is sparse).
            mem = _tighten_component(mem, adj, min_density)
            if len(mem) < min_group:
                continue
            mh = sum(1 for cell in usable_cells if len(cells[cell] & mem) >= 2)
            avg_gain = sum(acct_gain.get(m, 0.0) for m in mem) / len(mem)
            board_groups.append({
                "uuid": uuid, "members": set(mem),
                "matching_hours": mh, "avg_gain": avg_gain,
            })

    if not board_groups:
        return []

    # Cross-board merge: a ring active on several boards is one cluster.
    gp: dict[int, int] = {}

    def gfind(x: int) -> int:
        gp.setdefault(x, x)
        while gp[x] != x:
            gp[x] = gp[gp[x]]
            x = gp[x]
        return x

    def gunion(a: int, b: int) -> None:
        ra, rb = gfind(a), gfind(b)
        if ra != rb:
            gp[max(ra, rb)] = min(ra, rb)

    by_member: dict[str, int] = {}
    for idx, g in enumerate(board_groups):
        gfind(idx)
        for m in g["members"]:
            if m in by_member:
                gunion(idx, by_member[m])
            else:
                by_member[m] = idx

    merged: dict[int, list[int]] = {}
    for idx in range(len(board_groups)):
        merged.setdefault(gfind(idx), []).append(idx)

    clusters: list[dict] = []
    for group_idxs in merged.values():
        members: set[str] = set()
        boards_info: list[dict] = []
        max_match = 0
        gains: list[float] = []
        for gi in group_idxs:
            g = board_groups[gi]
            members |= g["members"]
            max_match = max(max_match, g["matching_hours"])
            gains.append(g["avg_gain"])
            bm = meta_by_board[g["uuid"]]
            boards_info.append({
                "uuid": g["uuid"],
                "name": bm.get("name") or str(g["uuid"]),
                "category": bm.get("category") or bm.get("category_id") or "",
                "contest_type": bm.get("contest_type"),
                "members": len(g["members"]),
                "member_names": sorted(g["members"])[:60],
                "matching_hours": g["matching_hours"],
                "avg_hourly_gain": _round(g["avg_gain"]),
            })
        if len(members) < min_group:
            continue
        avg_gain = sum(gains) / len(gains) if gains else 0.0
        clusters.append(_build_comovement_cluster(
            members, boards_info, max_match, avg_gain, min_match,
        ))

    clusters.sort(key=lambda c: (-c["confidence"], -c["member_count"]))
    return clusters


def _tighten_component(members: set[str], adj: dict[str, set], min_density: float) -> set[str]:
    """Reduce a connected component to its dense core: while the internal edge
    density is below ``min_density``, drop the member with the fewest in-group
    edges. A true ring is a near-clique (stays); a loose transitive chain peels
    down to ≤2 and is then discarded by the caller's ``min_group`` check."""
    mem = set(members)
    while len(mem) > 2:
        deg = {m: len(adj.get(m, _EMPTY) & mem) for m in mem}
        edges = sum(deg.values()) // 2
        possible = len(mem) * (len(mem) - 1) / 2
        if possible <= 0 or edges / possible >= min_density:
            break
        mem.discard(min(mem, key=lambda m: deg[m]))
    return mem


_EMPTY: frozenset = frozenset()


def _build_comovement_cluster(
    members: set[str], boards_info: list[dict], matching_hours: int,
    avg_gain: float, min_match: int,
) -> dict:
    member_names = sorted(members)
    n = len(member_names)
    # Optional name-stem corroboration: does one stem cover much of the group?
    stems: dict[str, int] = {}
    for nm in member_names:
        st = _name_stem(nm)
        if len(st) >= 2:
            stems[st] = stems.get(st, 0) + 1
    top_stem, top_count = max(stems.items(), key=lambda kv: kv[1]) if stems else ("", 0)
    name_corroborated = top_count >= max(2, n // 2)

    over = matching_hours - min_match
    matching_term = 1.0 - 0.6 ** (over + 1) if matching_hours >= min_match else 0.0
    size_term = 1.0 - 0.5 ** (n - 1)
    raw = 0.5 + 0.47 * (0.6 * matching_term + 0.4 * size_term)
    if name_corroborated:
        raw += 0.08
    conf = round(min(_COMOVEMENT_CONFIDENCE_CEILING, raw), 3)

    cap = 60
    board_count = len(boards_info)
    sample = (", ".join(member_names) if n <= 4
              else ", ".join(member_names[:3]) + f", … {member_names[-1]}")
    summary = (
        f"{n} accounts gained in LOCKSTEP - matching hourly score deltas across "
        f"{matching_hours} hour(s) since the weekly reset on {board_count} board(s) "
        f"({sample}). Avg matched gain ~{_fmt(avg_gain)}/hr. Coordinated alts/bots "
        f"progress together regardless of name"
        + (f"; this group also shares the name stem '{top_stem}'."
           if name_corroborated else ".")
    )
    return {
        "stem": top_stem if name_corroborated else "",
        "label": (f"{top_stem}*" if name_corroborated else f"{member_names[0]} +{n - 1}"),
        "method": "both" if name_corroborated else "co_movement",
        "member_count": n,
        "members": member_names[:cap],
        "members_truncated": max(0, n - cap),
        "board_count": board_count,
        "boards": sorted(
            boards_info,
            key=lambda b: ((b.get("category") or "").lower(), (b.get("name") or "").lower()),
        ),
        "confidence": conf,
        "summary": summary,
        "measurements": {
            "matching_hours": matching_hours,
            "group_size": n,
            "board_count": board_count,
            "avg_hourly_gain": _round(avg_gain),
            "name_corroborated": name_corroborated,
            "name_stem": top_stem if name_corroborated else None,
            "matching_term": round(matching_term, 3),
            "size_term": round(size_term, 3),
            "ceiling": _COMOVEMENT_CONFIDENCE_CEILING,
        },
    }


# ─── Schedule correlation (a name-agnostic producer) ───────────────────
# Accounts active/idle in the SAME hours all week share a play schedule -
# the tell for alts run by one operator even when they grind DIFFERENT
# content (which co-movement, a gain-magnitude check, can't see). Schedule
# ALONE is weak (many people play the same evenings), so it gets a low base
# confidence and relies on fusion to corroborate it.
_SCHEDULE_CONFIDENCE_CEILING = 0.85
# An account active in more than this fraction of the week's captures is "always
# on" - that's not a distinctive schedule (it matches every other busy account
# trivially), so it's excluded from schedule clustering. Such accounts are still
# caught by co-movement if they gain in lockstep.
_SCHEDULE_MAX_ACTIVE_FRACTION = 0.85


def _jaccard(a: set, b: set) -> float:
    if not a and not b:
        return 0.0
    inter = len(a & b)
    return inter / (len(a) + len(b) - inter)


def _cluster_schedule(
    active_hours: dict[str, set], board_sets: dict[str, set],
    meta_by_board: dict[int, dict], min_hours: int, min_sim: float,
    min_group: int, min_density: float, max_cell: int, num_slots: int,
) -> list[dict]:
    """Group accounts whose active-hour sets overlap by >= ``min_sim`` (Jaccard).
    Only accounts with a DISTINCTIVE partial schedule qualify (≥ ``min_hours``
    active, but not "always on" - see ``_SCHEDULE_MAX_ACTIVE_FRACTION``), since
    near-full schedules match every busy account trivially. Bounded: bucket by a
    coarse signature (6-slot blocks) so only same-rhythm accounts are compared
    pairwise; skip oversized buckets (a common schedule, not a ring)."""
    max_active = max(min_hours, int(num_slots * _SCHEDULE_MAX_ACTIVE_FRACTION))
    cands = {nm: hrs for nm, hrs in active_hours.items()
             if min_hours <= len(hrs) <= max_active}
    if len(cands) < min_group:
        return []
    # Coarse signature → bucket (6 consecutive slots = one block).
    buckets: dict[frozenset, list[str]] = {}
    for nm, hrs in cands.items():
        sig = frozenset(s // 6 for s in hrs)
        buckets.setdefault(sig, []).append(nm)

    parent: dict[str, str] = {}

    def find(x: str) -> str:
        parent.setdefault(x, x)
        while parent[x] != x:
            parent[x] = parent[parent[x]]
            x = parent[x]
        return x

    def union(a: str, b: str) -> None:
        ra, rb = find(a), find(b)
        if ra != rb:
            parent[max(ra, rb)] = min(ra, rb)

    adj: dict[str, set] = {}
    for names in buckets.values():
        if len(names) < 2 or len(names) > max_cell:
            continue
        for i in range(len(names)):
            for j in range(i + 1, len(names)):
                a, b = names[i], names[j]
                if _jaccard(cands[a], cands[b]) >= min_sim:
                    union(a, b)
                    adj.setdefault(a, set()).add(b)
                    adj.setdefault(b, set()).add(a)
    if not adj:
        return []

    comps: dict[str, set] = {}
    for node in adj:
        comps.setdefault(find(node), set()).add(node)

    out: list[dict] = []
    for mem in comps.values():
        mem = _tighten_component(mem, adj, min_density)
        if len(mem) < min_group:
            continue
        out.append(_build_schedule_cluster(mem, active_hours, board_sets, meta_by_board, min_hours))
    return out


def _build_schedule_cluster(
    members: set[str], active_hours: dict[str, set], board_sets: dict[str, set],
    meta_by_board: dict[int, dict], min_hours: int,
) -> dict:
    names = sorted(members)
    n = len(names)
    # Avg pairwise schedule similarity within the group (drives confidence).
    sims = [
        _jaccard(active_hours.get(a, set()), active_hours.get(b, set()))
        for i, a in enumerate(names) for b in names[i + 1:]
    ]
    cohesion = sum(sims) / len(sims) if sims else 0.0
    avg_hours = sum(len(active_hours.get(m, set())) for m in names) / n
    # Per-board presence (so the card + chart have boards to show).
    board_members: dict[int, list[str]] = {}
    for m in names:
        for u in board_sets.get(m, set()):
            board_members.setdefault(u, []).append(m)
    boards_info = []
    for u, ms in board_members.items():
        bm = meta_by_board.get(u, {})
        boards_info.append({
            "uuid": u,
            "name": bm.get("name") or str(u),
            "category": bm.get("category") or "",
            "contest_type": bm.get("contest_type"),
            "members": len(ms),
            "member_names": sorted(ms)[:60],
        })
    size_term = 1.0 - 0.5 ** (n - 1)
    raw = 0.5 + 0.35 * (0.6 * cohesion + 0.4 * size_term)
    conf = round(min(_SCHEDULE_CONFIDENCE_CEILING, raw), 3)
    top_stem, _ = _dominant_stem(names)
    sample = (", ".join(names) if n <= 4
              else ", ".join(names[:3]) + f", … {names[-1]}")
    return {
        "stem": "",
        "label": f"{names[0]} +{n - 1}",
        "method": "schedule",
        "member_count": n,
        "members": names[:60],
        "members_truncated": max(0, n - 60),
        "board_count": len(boards_info),
        "boards": sorted(boards_info, key=lambda b: ((b.get("category") or "").lower(),
                                                     (b.get("name") or "").lower())),
        "confidence": conf,
        "summary": (
            f"{n} accounts share a play SCHEDULE - active (and idle) in nearly the "
            f"same hours since the weekly reset ({cohesion * 100:.0f}% hour overlap, "
            f"~{avg_hours:.0f} active hours each) ({sample}). They log in and out "
            f"together regardless of which content they grind. Weak alone; "
            f"corroborated signals raise the confidence."
        ),
        "measurements": {
            "group_size": n, "schedule_cohesion": round(cohesion, 3),
            "avg_active_hours": round(avg_hours, 1), "ceiling": _SCHEDULE_CONFIDENCE_CEILING,
        },
    }


def _dominant_stem(names: list[str]) -> tuple[str, int]:
    """Most common non-trivial name stem across ``names`` + its count."""
    stems: dict[str, int] = {}
    for nm in names:
        st = _name_stem(nm)
        if len(st) >= 2:
            stems[st] = stems.get(st, 0) + 1
    return max(stems.items(), key=lambda kv: kv[1]) if stems else ("", 0)


# ─── Signal fusion ─────────────────────────────────────────────────────
# The payoff of having several INDEPENDENT signals: a group flagged by
# co-movement AND schedule AND a shared name is far more certain than one
# flagged by a single signal. Merge the producers' clusters by member
# overlap, re-evaluate every signal on each merged group, and raise
# confidence per corroborating signal.
_FUSION_CONFIDENCE_CEILING = 0.98


def _fuse(name_clusters: list[dict], signals: dict, cfg: dict) -> list[dict]:
    """Merge name-stem + co-movement + schedule clusters by member overlap, then
    score each merged group by how many independent signals agree on it."""
    bonus = float(cfg.get("fusion_corroboration_bonus", 0.06))
    footprint_min = float(cfg.get("footprint_min_jaccard", 0.6))
    active_hours = signals.get("active_hours", {})
    board_sets = signals.get("board_sets", {})

    producers = (
        list(signals.get("comovement", []))
        + list(signals.get("schedule", []))
        + list(name_clusters)
    )
    if not producers:
        return []

    # Union producer clusters that share members.
    parent: dict[int, int] = {}

    def find(x: int) -> int:
        parent.setdefault(x, x)
        while parent[x] != x:
            parent[x] = parent[parent[x]]
            x = parent[x]
        return x

    def union(a: int, b: int) -> None:
        ra, rb = find(a), find(b)
        if ra != rb:
            parent[max(ra, rb)] = min(ra, rb)

    by_member: dict[str, int] = {}
    for idx, c in enumerate(producers):
        find(idx)
        for m in c.get("members", []):
            if m in by_member:
                union(idx, by_member[m])
            else:
                by_member[m] = idx

    groups: dict[int, list[int]] = {}
    for idx in range(len(producers)):
        groups.setdefault(find(idx), []).append(idx)

    out: list[dict] = []
    for idxs in groups.values():
        parts = [producers[i] for i in idxs]
        # Prefer the richest producer cluster as the base (most members, then
        # highest single-signal confidence) and union all members.
        base = max(parts, key=lambda c: (c.get("member_count", 0), c.get("confidence", 0.0)))
        members = sorted({m for c in parts for m in c.get("members", [])})
        fused = dict(base)
        fused["members"] = members[:60]
        fused["member_count"] = len(members)
        fused["members_truncated"] = max(0, len(members) - 60)

        # Which signals fired on this group?
        methods = {c.get("method") for c in parts}
        corroborated: list[str] = []
        if "co_movement" in methods or "both" in methods:
            corroborated.append("co_movement")
        if "schedule" in methods:
            corroborated.append("schedule")
        # Name: any contributing cluster shared a stem, OR the merged members do.
        stem, stem_count = _dominant_stem(members)
        if "name_stem" in methods or "both" in methods or stem_count >= max(2, len(members) // 2):
            corroborated.append("name_stem")
        # Schedule cohesion as a corroboration even if schedule wasn't a producer.
        if "schedule" not in corroborated and active_hours:
            sims = [
                _jaccard(active_hours.get(a, set()), active_hours.get(b, set()))
                for i, a in enumerate(members) for b in members[i + 1:]
            ]
            if sims and sum(sims) / len(sims) >= float(cfg.get("schedule_min_similarity", 0.8)):
                corroborated.append("schedule")
        # Board-footprint cohesion.
        if board_sets:
            fps = [
                _jaccard(board_sets.get(a, set()), board_sets.get(b, set()))
                for i, a in enumerate(members) for b in members[i + 1:]
            ]
            if fps and sum(fps) / len(fps) >= footprint_min:
                corroborated.append("footprint")

        # De-dup while preserving order.
        corroborated = list(dict.fromkeys(corroborated))
        # Fused confidence: base single-signal confidence + a bonus per EXTRA
        # independent signal (capped). Diversity of evidence is the point.
        base_conf = max(c.get("confidence", 0.0) for c in parts)
        extra = max(0, len(corroborated) - 1)
        fused["confidence"] = round(min(_FUSION_CONFIDENCE_CEILING, base_conf + extra * bonus), 3)
        fused["corroborated_by"] = corroborated
        # Method label reflects the strongest producer, but "both" only when a
        # co-movement/schedule group ALSO has a name stem.
        if {"co_movement", "schedule"} & set(corroborated) and "name_stem" in corroborated:
            fused["method"] = "both"
        elif "co_movement" in corroborated:
            fused["method"] = "co_movement"
        elif "schedule" in corroborated:
            fused["method"] = "schedule"
        else:
            fused["method"] = "name_stem"
        if stem and fused["method"] in ("both", "name_stem"):
            fused["stem"] = stem
            fused["label"] = f"{stem}*"
        out.append(fused)

    out.sort(key=lambda c: (-c.get("confidence", 0.0), -c.get("member_count", 0)))
    return out


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
    analyzed_boards: list[dict] | None = None,
    excluded_boards: list[dict] | None = None,
    clusters: list[dict] | None = None,
    clusters_boards_scanned: int = 0,
) -> dict:
    players = []
    for name, boards_map in flagged.items():
        boards = list(boards_map.values())
        # Per-evidence + per-board confidence. The board-level value is
        # the max evidence confidence on THAT board - within a board,
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
    # tiebreaker - most-suspicious-first.
    players.sort(
        key=lambda p: (
            -p["confidence"],
            -sum(len(b["evidence"]) for b in p["leaderboards"]),
        ),
    )
    return {
        "players": players,
        "clusters": clusters or [],
        "clusters_boards_scanned": clusters_boards_scanned,
        "computed_at": int(time.time()),
        "anchor": anchor,
        "method": (
            "Four independent checks: Modified Z-score (MAD-based, "
            "Iglewicz & Hoaglin 1993), rank-gap ratio, and velocity vs "
            "peer p95 flag individual outliers; alt-cluster detection "
            "groups similarly-named accounts sitting at near-identical "
            "scores. A player flagged by multiple checks or on multiple "
            "boards - or a cluster tighter, larger, and on more boards - "
            "is higher confidence."
        ),
        "config": {
            "z_threshold": z,
            "velocity_multiplier": vm,
            "min_board_size": mb,
        },
        "total_flagged": len(players),
        "boards_analyzed": boards_analyzed,
        "boards_excluded": boards_excluded,
        # Detailed lists, sorted by (category, name) for stable rendering on
        # the showcase site. Empty arrays when the analysis didn't run (e.g.,
        # no anchor available yet).
        "analyzed_boards": sorted(
            analyzed_boards or [],
            key=lambda b: ((b.get("category") or "").lower(), (b.get("name") or "").lower()),
        ),
        "excluded_boards": sorted(
            excluded_boards or [],
            key=lambda b: ((b.get("category") or "").lower(), (b.get("name") or "").lower()),
        ),
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
#     cohort, 51 single-signal flags appeared on real data - the vast
#     majority were rank-1 to rank-5 players who are legitimately
#     ahead of their cohort. Cap low so it can only contribute when
#     another check confirms.
_CHECK_CONFIDENCE_CEILINGS: dict[str, float] = {
    "velocity_outlier": 0.99,
    # Sustained weekly throughput vs peer p95 - like velocity but week-long, so
    # a one-hour fluke can't trigger it; very clean, just under velocity.
    "sustained_velocity": 0.95,
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

    # Weekly uptime: the active fraction lives in the narrow band
    # [threshold, 1.0] where the generic magnitude/threshold sigmoid barely
    # moves. Map it directly - threshold → 0.7, ramping to the ceiling near
    # 100% uptime (a 24/7 bot).
    if et == "sustained_velocity":
        frac = float(m.get("uptime_fraction", 0.0))
        thr = float(m.get("threshold_fraction", 0.85))
        if thr >= 1.0 or frac < thr:
            return 0.5
        raw = 0.7 + 0.3 * ((frac - thr) / (1.0 - thr))
        return round(min(_CHECK_CONFIDENCE_CEILINGS["sustained_velocity"], raw), 3)

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
      are NOT independent at the per-board level - MAD-Z and rank-gap
      both light up when one player dominates, so multiplying would
      double-count the same anomaly.
    - **Across boards**: noisy-OR. Independence is defensible - one
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

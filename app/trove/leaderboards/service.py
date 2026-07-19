"""Write- and read-side helpers for the leaderboards scope.

Storage is **PostgreSQL** (``pg_store`` - partitioned ``entry`` table + board /
player dimensions, COPY bulk-load). This module keeps the pure logic (reset-
boundary math, delta computation, the synthetic reset-zero injection, timestamp
normalisation) and delegates every DB touch to ``pg_store``. The Redis cache
layer (``cache.py``) sits in front of the reads unchanged.

The insert is idempotent for a given ``created_at`` (the snapshot's 11:00-UTC
anchor): re-running the same dump replaces that anchor's rows in one transaction,
so readers (Postgres MVCC) never see a partial write - no in-flight guard needed.
"""

from __future__ import annotations

import asyncio
import logging
from datetime import UTC, datetime, timedelta

from app.trove.leaderboards import mastery as mastery_calc
from app.trove.leaderboards import pg_store
from app.trove.leaderboards.parser import parse_dump

logger = logging.getLogger(__name__)


# --- timestamp anchoring ----------------------------------------------------

def _trove_day_anchor(now: datetime | None = None) -> int:
    """Today's Trove-day anchor in unix seconds: the most recent 11:00 UTC reset."""
    real = (now or datetime.now(UTC)).replace(microsecond=0)
    today_11 = real.replace(hour=11, minute=0, second=0)
    anchor = today_11 if real >= today_11 else (today_11 - timedelta(days=1))
    return int(anchor.timestamp())


# --- Reset boundaries (history-chart visualisation) -------------------------

_RESET_HOUR_UTC = 11        # Trove daily/weekly resets at 11:00 UTC
_WEEKLY_RESET_WEEKDAY = 0   # Monday (Python's weekday() convention)


def reset_boundaries_for_kind(kind: str, t_start: int, t_end: int) -> list[int]:
    """Unix-seconds reset moments for a board of ``kind`` strictly inside the
    half-open interval ``(t_start, t_end]``. Lifetime cadences never reset."""
    if kind not in ("daily", "weekly") or t_end <= t_start:
        return []
    dt = datetime.fromtimestamp(t_start, UTC)
    candidate = dt.replace(hour=_RESET_HOUR_UTC, minute=0, second=0, microsecond=0)
    if candidate <= dt:
        candidate += timedelta(days=1)
    if kind == "weekly":
        while candidate.weekday() != _WEEKLY_RESET_WEEKDAY:
            candidate += timedelta(days=1)
        step = timedelta(days=7)
    else:
        step = timedelta(days=1)
    out: list[int] = []
    while True:
        ts = int(candidate.timestamp())
        if ts > t_end:
            break
        out.append(ts)
        candidate += step
    return out


def _inject_reset_zeros(points: list[dict], kind: str) -> list[dict]:
    """Insert synthetic ``(rank=0, score=0, synthetic=True)`` points at each reset
    boundary between two real captures so a daily/weekly history chart cliff-drops
    to 0 instead of sloping smoothly across the reset."""
    annotated = [{**p, "synthetic": False} for p in points]
    if kind not in ("daily", "weekly") or len(annotated) < 2:
        return annotated
    out: list[dict] = []
    for i, p in enumerate(annotated):
        out.append(p)
        if i + 1 >= len(annotated):
            continue
        nxt = annotated[i + 1]
        boundaries = reset_boundaries_for_kind(kind, p["created_at"], nxt["created_at"])
        if not boundaries:
            continue
        last_score = float(p["score"])
        last_rank = int(p["rank"])
        for r in boundaries:
            if last_score > 0:
                out.append({
                    "created_at": r - 1, "rank": last_rank,
                    "score": last_score, "synthetic": True,
                })
                if r < nxt["created_at"]:
                    out.append({
                        "created_at": r, "rank": 0, "score": 0.0, "synthetic": True,
                    })
            last_score = 0.0
            last_rank = 0
    return out


def normalize_timestamp(ts: int | None, *, allow_backfill: bool = False) -> int:
    """Validate/normalize a user-supplied ``created_at``. ``allow_backfill`` lifts
    the 14-day backward limit (master bulk re-seed). 00:00 UTC aliases to 11:00."""
    if ts is None or ts <= 0:
        return -1
    parsed = datetime.fromtimestamp(ts, UTC).replace(second=0, microsecond=0)
    now = datetime.now(UTC).replace(second=0, microsecond=0)
    if parsed > now + timedelta(minutes=5):
        return -1
    if not allow_backfill and parsed < now - timedelta(days=14):
        return -1
    if parsed.hour == 0 and parsed.minute == 0:
        parsed = parsed.replace(hour=11)
    return int(parsed.timestamp())


def _trove_day_start(anchor: int) -> int:
    """The 11:00-UTC reset that opens ``anchor``'s trove-day (unix seconds)."""
    return ((anchor - _RESET_HOUR_UTC * 3600) // 86400) * 86400 + _RESET_HOUR_UTC * 3600


async def archive_query_cutoff() -> int:
    """Reads for anchors below this pay the tighter archive rate-limit bucket
    (runtime-tunable). Time-based - independent of storage."""
    from app.admin import runtime_config
    days = await runtime_config.get_setting("leaderboards_archive_query_threshold_days")
    return int((datetime.now(UTC) - timedelta(days=days)).timestamp())


async def is_archive_query(anchor: int) -> bool:
    return anchor < await archive_query_cutoff()


# --- capture completeness guard ---------------------------------------------
# A live scrape can return a SHORT dump when the in-game leaderboards are briefly
# unavailable or crash mid-run: whole boards missing, or a board truncated
# part-way. Storing it poisons every delta consumer (activity, cheater velocity,
# renames, day-over-day deltas) because players merely absent from the short
# capture read as "newly appeared / active" against it. So a materially-incomplete
# LIVE capture is REJECTED (not stored) - the hour becomes a clean time-gap that
# every consumer already bridges over. Detection is by board PRESENCE (id set),
# with a secondary gross-collapse check; gated to non-reset windows since board
# rotation / zeroing at a daily/weekly reset is legitimate, not a failure.

_COLLAPSE_FLOOR = 100   # ignore collapse on boards that never had a real population


def _completeness_verdict(
    cur_counts: dict[int, int], prev_counts: dict[int, int],
    *, max_missing: int, collapse_frac: float,
) -> dict | None:
    """Pure decision: a reason dict when ``cur_counts`` is materially incomplete
    vs the previous good capture ``prev_counts`` (both ``{board_uuid: count}``),
    else None. ``missing`` = boards present before but gone now; ``collapsed`` =
    boards whose population fell below ``collapse_frac`` of before (only boards
    that had ≥ ``_COLLAPSE_FLOOR`` entries, so tiny boards can't trip it)."""
    prev_ids, cur_ids = set(prev_counts), set(cur_counts)
    missing = sorted(prev_ids - cur_ids)
    collapsed: list[int] = []
    if collapse_frac > 0:
        for uuid in prev_ids & cur_ids:
            pc = prev_counts[uuid]
            if pc >= _COLLAPSE_FLOOR and cur_counts[uuid] < pc * collapse_frac:
                collapsed.append(uuid)
        collapsed.sort()
    if len(missing) <= max_missing and not collapsed:
        return None
    parts: list[str] = []
    if len(missing) > max_missing:
        parts.append(f"{len(missing)} board(s) missing vs previous capture")
    if collapsed:
        parts.append(f"{len(collapsed)} board(s) collapsed below {collapse_frac:.0%} of previous")
    return {"reason": "; ".join(parts), "missing_boards": missing, "collapsed_boards": collapsed}


async def _assess_capture_completeness(boards, anchor: int) -> dict | None:
    """Gather the previous-capture baseline + config and run ``_completeness_verdict``
    for a LIVE ingest. Returns a reason dict to REJECT, or None to accept. No-op
    when the guard is off, on a cold start, at a reset boundary (board rotation
    expected), or when the previous capture is too sparse to judge against."""
    from app.admin import runtime_config

    if not bool(await runtime_config.get_setting("capture_completeness_enabled")):
        return None
    prev = await pg_store.previous_anchor_before(anchor)
    if prev is None:
        return None   # first/cold capture - nothing to compare against
    # Non-reset gate: at a daily (11:00 UTC) / weekly (Mon 11:00) reset the board
    # set legitimately rotates (contest boards) and daily boards zero out, so a
    # changed board set there is expected, not a failed scrape.
    if (reset_boundaries_for_kind("daily", prev, anchor)
            or reset_boundaries_for_kind("weekly", prev, anchor)):
        return None
    prev_counts = await pg_store.board_counts_at(prev)
    min_prev = int(await runtime_config.get_setting("capture_min_prev_boards"))
    if len(prev_counts) < min_prev:
        return None   # not enough baseline to judge completeness
    max_missing = int(await runtime_config.get_setting("capture_max_missing_boards"))
    collapse_frac = float(await runtime_config.get_setting("capture_collapse_frac"))
    cur_counts = {b.uuid: len(b.entries) for b in boards}
    verdict = _completeness_verdict(
        cur_counts, prev_counts, max_missing=max_missing, collapse_frac=collapse_frac,
    )
    if verdict is None:
        return None
    verdict.update({"prev_anchor": prev, "prev_boards": len(prev_counts),
                    "cur_boards": len(cur_counts)})
    return verdict


# --- insert -----------------------------------------------------------------

async def insert_dump(
    text: str, *, timestamp: int | None = None, allow_backfill: bool = False,
) -> dict:
    """Parse + persist a dump into Postgres (one transaction, idempotent on the
    anchor). ``allow_backfill`` lifts the 14-day anchor limit for a re-seed AND
    skips the completeness guard (a bulk re-seed of already-collected data trusts
    the operator; the guard is for live captures)."""
    boards = parse_dump(text)
    if not boards:
        logger.warning("leaderboards: parsed 0 boards from %d-char dump", len(text))
        return {"boards": 0, "entries": 0, "created_at": None}

    if timestamp is not None and timestamp > 0:
        created_at = normalize_timestamp(timestamp, allow_backfill=allow_backfill)
        if created_at == -1:
            created_at = _trove_day_anchor()
    else:
        created_at = _trove_day_anchor()

    # Reject a materially-incomplete LIVE capture rather than store a poisoned one.
    if not allow_backfill:
        verdict = await _assess_capture_completeness(boards, created_at)
        if verdict is not None:
            logger.warning(
                "leaderboards: REJECTED incomplete capture anchor=%d (%s); prev=%s "
                "boards %d->%d missing=%s collapsed=%s",
                created_at, verdict["reason"], verdict["prev_anchor"],
                verdict["prev_boards"], verdict["cur_boards"],
                verdict["missing_boards"][:20], verdict["collapsed_boards"][:20],
            )
            return {
                "skipped": True, "created_at": created_at,
                "boards": len(boards),
                "entries": sum(len(b.entries) for b in boards),
                **verdict,
            }

    summary = await pg_store.write_snapshot(boards, created_at)
    logger.info(
        "leaderboards: ingested anchor=%d boards=%d entries=%d cleared=%d",
        created_at, summary["boards"], summary["entries"], summary["cleared_before_insert"],
    )
    return summary


async def reset_all(*, drop_boards: bool = False) -> dict:
    """**Destructive.** TRUNCATE the leaderboards tables + clear every derived
    cache. Board metadata (incl. admin reset-cadence overrides) is kept unless
    ``drop_boards``. All derived from captures - a re-ingest rebuilds it."""
    from app.trove.leaderboards import activity, class_activity, detection, renames
    from app.trove.leaderboards import cache as lb_cache

    summary = await pg_store.reset_all(drop_boards=drop_boards)
    detection.reset()
    activity.reset_caches()
    class_activity.reset_caches()
    renames.reset()
    summary["redis_keys_cleared"] = await lb_cache.reset_all()
    logger.warning("leaderboards: FULL RESET %s", summary)
    return summary


# --- read -------------------------------------------------------------------

async def list_timestamps(limit: int = 60, *, include_archive: bool = True) -> list[int]:
    """Distinct anchors with stored entries, newest first. (``include_archive`` is
    a no-op now - one partitioned table holds everything.)"""
    return await pg_store.list_timestamps(limit)


async def list_boards_at(created_at: int) -> list[dict]:
    return await pg_store.list_boards_at(created_at)


async def get_board(uuid: int) -> dict | None:
    return await pg_store.get_board(uuid)


async def list_entries(
    uuid: int, created_at: int, *, limit: int = 100, offset: int = 0,
) -> tuple[list[dict], int]:
    return await pg_store.list_entries(uuid, created_at, limit=limit, offset=offset)


async def entries_by_board_at(
    anchor: int, uuids: list[int] | None = None,
) -> dict[int, list[dict]]:
    """All entries at one anchor grouped by board, sorted by rank - the bulk
    loader for cheater detection + the activity 1h breakdown."""
    return await pg_store.entries_by_board_at(anchor, uuids)


# --- record highs (free "how high can these stats go" endpoint) -------------
# These boards are *lifetime* (never reset), so their current rank-1 IS the
# highest the stat has ever reached in-game. Mastery boards store a running
# POINTS total the client turns into a level; Power Rank stores the rank value
# directly. Board ids are Trove's, not ours.
_TROVE_MASTERY_UUID = 1
_GEODE_MASTERY_UUID = 20
_POWER_RANK_UUIDS = list(range(1000, 1017))   # 1000-1016, one board per class
# Geode Mastery is soft-capped at 100 in-game: the level bar stops there even
# though the points keep accruing. We surface both the capped and true level.
_GEODE_LEVEL_CAP = 100


def _mastery_block(top: dict | None, *, cap: int | None = None) -> dict | None:
    """Shape one mastery board's rank-1 into a points->level summary."""
    if not top:
        return None
    points = max(0, int(round(top["score"])))
    level, into_level, to_next = mastery_calc.level_from_points(points)
    block = {
        "points": points,
        "level": min(level, cap) if cap is not None else level,
        "points_into_level": into_level,
        "points_to_next_level": to_next,
        "player_name": top["player_name"],
        "anchor": top["anchor"],
    }
    if cap is not None:
        # Show the soft cap AND what the level would be uncapped (e.g. 100 vs 143).
        block["level_cap"] = cap
        block["uncapped_level"] = level
        block["capped"] = level > cap
    return block


async def mastery_records() -> dict:
    """The current highest Trove Mastery, Geode Mastery and Power Rank in the
    game - the absolute ceiling each stat has reached, from the rank-1 holder of
    the relevant lifetime board(s). Mastery is reported as both points and level;
    Power Rank is the single highest value across all 17 per-class boards."""
    # Latest PUBLISHED snapshot - the same warmer-gated anchor the leaderboards
    # page reads (Redis read-through, falls back to a DISTINCT-anchor query on a
    # cold cache). Local import: cache.py imports this module, so importing it at
    # module scope would be circular.
    from app.trove.leaderboards import cache as leaderboards_cache
    empty = {"trove_mastery": None, "geode_mastery": None, "power_rank": None}
    ts = await leaderboards_cache.get_timestamps(1)
    if not ts:
        return empty
    tops = await pg_store.top_entries_for_boards(
        [_TROVE_MASTERY_UUID, _GEODE_MASTERY_UUID, *_POWER_RANK_UUIDS], ts[0],
    )

    # Power Rank: one number - the highest rank-1 across every class board.
    power = None
    for uuid in _POWER_RANK_UUIDS:
        top = tops.get(uuid)
        if not top:
            continue
        value = int(round(top["score"]))
        if power is None or value > power["value"]:
            power = {
                "value": value,
                "board_uuid": uuid,
                "player_name": top["player_name"],
                "anchor": top["anchor"],
            }

    return {
        "trove_mastery": _mastery_block(tops.get(_TROVE_MASTERY_UUID)),
        "geode_mastery": _mastery_block(tops.get(_GEODE_MASTERY_UUID), cap=_GEODE_LEVEL_CAP),
        "power_rank": power,
    }


async def _previous_day_anchor(uuid: int, created_at: int) -> int | None:
    return await pg_store.previous_day_anchor(uuid, created_at)


async def list_entries_with_deltas(
    uuid: int, created_at: int, *, limit: int = 100, offset: int = 0,
) -> tuple[list[dict], int, dict]:
    """``list_entries`` plus per-player day-over-day rank/score deltas (when a
    comparable prior snapshot exists - i.e. no reset crossed between them)."""
    items, total = await list_entries(uuid, created_at, limit=limit, offset=offset)

    prev_anchor = await _previous_day_anchor(uuid, created_at)
    if prev_anchor is None:
        return items, total, {
            "comparable": False, "prev_anchor": None, "reason": "no_prior_snapshot",
        }

    kind = await pg_store.board_reset_kind(uuid)
    if reset_boundaries_for_kind(kind, prev_anchor, created_at):
        return items, total, {
            "comparable": False, "prev_anchor": prev_anchor, "reason": "crossed_reset",
        }

    if items:
        names = [it["player_name"] for it in items]
        prev = await pg_store.prev_rows_for_players(uuid, prev_anchor, names)
        for it in items:
            p = prev.get(it["player_name"].lower())
            if p is None:
                it.update(is_new=True, prev_rank=None, prev_score=None,
                          rank_delta=None, score_delta=None)
            else:
                it.update(
                    is_new=False, prev_rank=p["rank"], prev_score=p["score"],
                    rank_delta=p["rank"] - it["rank"],
                    score_delta=it["score"] - p["score"],
                )
    return items, total, {"comparable": True, "prev_anchor": prev_anchor, "reason": "ok"}


async def _attach_player_history_deltas(player_name: str, rows: list[dict]) -> None:
    """In-place: add day-over-day deltas to player-history rows (same comparable
    rule as the entries table). One windowed query for the player's recent rows."""
    window_start = int(datetime.now(UTC).timestamp()) - 8 * 86400
    docs = await pg_store.player_rows_window(player_name.strip(), window_start)

    by_board: dict[int, list[tuple[int, int, float]]] = {}
    for d in docs:
        by_board.setdefault(d["leaderboard"], []).append(
            (d["created_at"], d["rank"], d["score"])
        )
    for series in by_board.values():
        series.sort(reverse=True)

    kinds = await pg_store.board_kinds(list(by_board.keys()))

    for r in rows:
        series = by_board.get(r["leaderboard"])
        if not series:
            continue
        day = _trove_day_start(r["created_at"])
        prev = next((p for p in series if p[0] < day), None)
        if prev is None:
            continue
        kind = kinds.get(r["leaderboard"], "default")
        if reset_boundaries_for_kind(kind, prev[0], r["created_at"]):
            continue
        r["prev_rank"] = prev[1]
        r["prev_score"] = prev[2]
        r["rank_delta"] = prev[1] - r["rank"]
        r["score_delta"] = r["score"] - prev[2]


async def player_history(
    player_name: str, *, limit: int = 50, uuid: int | None = None,
    include_archive: bool = True, with_deltas: bool = False,
    window_start: int | None = None,
) -> list[dict]:
    """Most recent dumps that featured a player (case-insensitive), optional board
    filter. ``window_start`` bounds the scan to recent partitions (see
    pg_store.player_rows) - callers that only need recent appearances pass it to
    avoid the slow all-partition scan. (``include_archive`` is a no-op now.)"""
    out = await pg_store.player_rows(
        player_name.strip(), limit=limit, uuid=uuid, window_start=window_start,
    )
    if with_deltas and out:
        await _attach_player_history_deltas(player_name.strip(), out)
    return out


async def _is_verified_trove_name(name: str) -> bool:
    """True when a site account has claimed AND been approved for this Trove name
    (ties the public profile to the manual master-approval claim flow)."""
    try:
        from app.site_auth.models import SiteUser
        doc = await SiteUser.find_one(
            {"claimed_trove_name": name.strip().lower(), "claim_verified": True}
        )
        return doc is not None
    except Exception:
        return False


async def player_profile(name: str, *, limit: int = 200) -> dict:
    """Public profile aggregate for one player: recent appearances (board names +
    day-over-day deltas), a summary, and whether the name is a verified claimed
    identity. ``recent`` is empty when the name has never been captured."""
    name = name.strip()
    # Every lookup below keys on the name and is independent, so run them
    # concurrently - the profile used to serialize ~6 round-trips (and the
    # board aggregate scanned the player's whole cross-partition history, which
    # was 30-90s for prolific players). ``board_rows`` now reads the O(boards)
    # player_board_agg; ``recent`` is bounded to a recent window (it feeds the
    # /v1 payload only - the page renders from ``boards``).
    recent_window = int(datetime.now(UTC).timestamp()) - _PROFILE_RECENT_WINDOW_DAYS * 86400
    (
        board_rows, stored_name, verified, renames_out, alt_clusters,
        last_played, rows,
    ) = await asyncio.gather(
        pg_store.player_board_summary(name),
        pg_store.player_canonical_name(name),
        _is_verified_trove_name(name),
        _profile_renames(name),
        _profile_alt_clusters(name, []),
        _profile_last_played(name),
        player_history(name, limit=limit, with_deltas=True, window_start=recent_window),
    )
    canonical = stored_name or (rows[0]["player_name"] if rows else name)
    # Board names for every board referenced by the recent rows OR the aggregate
    # (the aggregate spans all history; ``recent`` only a window), fetched once.
    uuids = sorted({r["leaderboard"] for r in rows} | {b["leaderboard"] for b in board_rows})
    meta = await pg_store.board_meta(uuids) if uuids else {}
    recent = [
        {**r, "board_name": (meta.get(r["leaderboard"]) or {}).get("name")}
        for r in rows
    ]
    boards = [
        {
            **b,
            "board_name": (meta.get(b["leaderboard"]) or {}).get("name"),
            "category": (meta.get(b["leaderboard"]) or {}).get("category"),
        }
        for b in board_rows
    ]
    best = boards[0] if boards else None            # board_rows are best_rank-ascending
    latest_anchor = max((b["last_seen"] for b in board_rows), default=None)
    top10 = sum(1 for b in board_rows if (b["best_rank"] or 1e9) <= 10)
    top100 = sum(1 for b in board_rows if (b["best_rank"] or 1e9) <= 100)
    # Total appearances across all boards over all history (from the aggregate) -
    # was previously len(rows), which the old code capped at the history limit.
    total_appearances = sum(int(b.get("appearances") or 0) for b in board_rows)

    return {
        "player_name": canonical,
        "verified": verified,
        "summary": {
            "boards_appeared": len(board_rows),
            # ``appearances`` (total captures) kept for /v1 back-compat; the page
            # now counts per-leaderboard via ``boards``.
            "appearances": total_appearances,
            "best_rank": best["best_rank"] if best else None,
            "best_rank_board_uuid": best["leaderboard"] if best else None,
            "best_rank_board_name": best["board_name"] if best else None,
            "top10_count": top10,
            "top100_count": top100,
            "latest_anchor": latest_anchor,
            # "Last seen" is latest_anchor (mere presence); "last played" is the
            # most recent capture their score actually rose (real activity).
            "last_played": last_played,
        },
        "boards": boards,
        "recent": recent,
        # Username history reconstructed by the rename detector, and any alt
        # cluster this identity was grouped into (both null/empty when the
        # respective feature is off or nothing was found).
        "renames": renames_out,
        "alt_clusters": alt_clusters,
    }


# How far back the "last played" query looks. Bounds the per-profile cost to
# recent partitions; a player whose score hasn't risen in this window reads as
# no recent activity (last_played=None), which is the honest answer.
_LAST_PLAYED_WINDOW_DAYS = 90

# Window for the profile's ``recent`` list (the /v1 back-compat per-capture rows;
# the /player page renders from ``boards``, not this). Bounds the scan to recent
# partitions so a prolific player's profile no longer scans all of history.
_PROFILE_RECENT_WINDOW_DAYS = 14


def _parse_board_csv(csv: str) -> set[int]:
    """Comma-separated board-UUID string → set of ints. Non-numeric tokens are
    skipped so a typo in the runtime config can't break the profile."""
    out: set[int] = set()
    for tok in (csv or "").split(","):
        tok = tok.strip()
        if not tok:
            continue
        try:
            out.add(int(tok))
        except ValueError:
            logger.warning("last_played: ignoring non-numeric excluded-board token %r", tok)
    return out


async def _profile_last_played(canonical: str) -> int | None:
    """Most recent capture where this player's score rose on a non-excluded board
    (the 'last played' activity signal), or None when no movement in the window.
    Never raises - a lookup failure must not take down the profile."""
    from app.admin import runtime_config
    csv = str(await runtime_config.get_setting("last_played_excluded_board_uuids") or "")
    now = int(datetime.now(UTC).timestamp())
    window_start = now - _LAST_PLAYED_WINDOW_DAYS * 86400
    try:
        return await pg_store.player_last_played(
            canonical, excluded=_parse_board_csv(csv), window_start=window_start,
        )
    except Exception:
        logger.exception("profile last-played lookup failed for %s", canonical)
        return None


async def _profile_renames(canonical: str) -> dict | None:
    """The rename chain touching ``canonical`` (aliases + ordered edges), or None
    when this identity has no detected renames. Never raises - a rename lookup
    failure must not take down the whole profile."""
    from app.trove.leaderboards import renames as leaderboards_renames
    try:
        hist = await leaderboards_renames.history(canonical)
    except Exception:
        logger.exception("profile rename lookup failed for %s", canonical)
        return None
    if not hist.get("rename_count"):
        return None
    return {
        "aliases": hist.get("aliases", []),
        "current_name": hist.get("current_name"),
        "count": hist.get("rename_count", 0),
        "edges": hist.get("edges", []),
    }


async def _profile_alt_clusters(canonical: str, aliases: list[str]) -> list[dict]:
    """Alt clusters (from the cached detection pass) whose membership includes
    this identity under any of its known names. Empty when clusters are disabled
    or the player is in none. Never raises."""
    from app.trove.leaderboards import detection as leaderboards_detection
    try:
        payload = await leaderboards_detection.detect_possible_cheaters()
    except Exception:
        logger.exception("profile alt-cluster lookup failed for %s", canonical)
        return []
    names_lc = {canonical.lower(), *(a.lower() for a in aliases)}
    # Project just the fields the profile card renders - the raw cluster carries
    # heavy per-board member detail the page doesn't use.
    keep = ("stem", "label", "method", "confidence", "member_count", "members",
            "members_truncated", "board_count", "corroborated_by", "summary")
    return [
        {k: c[k] for k in keep if k in c}
        for c in (payload.get("clusters") or [])
        if any((m or "").lower() in names_lc for m in (c.get("members") or []))
    ]


async def board_history(uuid: int, *, days: int = 7, top: int = 5) -> dict:
    """Score-vs-time trajectories for the current top-``top`` players on a board
    over the last ``days`` days, with synthetic reset-zero cliffs."""
    days = max(1, min(days, 30))
    top = max(1, min(top, 20))
    now = int(datetime.now(UTC).timestamp())
    window_start = now - days * 86400

    anchors, _latest, top_meta, series_rows = await pg_store.board_top_series(
        uuid, window_start, top,
    )
    if not anchors:
        return {"uuid": uuid, "days": days, "window_start": window_start,
                "window_end": now, "anchors": [], "series": []}
    if not top_meta:
        return {"uuid": uuid, "days": days, "window_start": window_start,
                "window_end": now, "anchors": anchors, "series": []}

    # Keyed by player_id (names can collide / change case across captures; the id
    # is stable). top_meta is already rank-ordered, so iterating it preserves the
    # plot order.
    names = {m["player_id"]: m["player_name"] for m in top_meta}
    current_rank = {m["player_id"]: m["rank"] for m in top_meta}
    per_player: dict[int, list[dict]] = {pid: [] for pid in names}
    for r in series_rows:
        per_player[r["player_id"]].append(
            {"created_at": r["created_at"], "rank": r["rank"], "score": r["score"]}
        )
    for pts in per_player.values():
        pts.sort(key=lambda p: p["created_at"])

    board_kind = await pg_store.board_reset_kind(uuid)
    series = [
        {
            "player_name": names[m["player_id"]],
            "current_rank": current_rank.get(m["player_id"]),
            "points": _inject_reset_zeros(per_player[m["player_id"]], board_kind),
        }
        for m in top_meta
    ]
    return {"uuid": uuid, "days": days, "window_start": window_start,
            "window_end": now, "anchors": anchors, "series": series}


# ── board health ─────────────────────────────────────────────────────────────

def _median(xs: list[float]) -> float:
    s = sorted(xs)
    n = len(s)
    if n == 0:
        return 0.0
    mid = n // 2
    return s[mid] if n % 2 else (s[mid - 1] + s[mid]) / 2.0


def _gini(xs: list[float]) -> float:
    """Gini coefficient of non-negative values (0 = perfectly even, →1 = one player
    dominates) - a "competitiveness" proxy over the top-N scores."""
    s = sorted(x for x in xs if x >= 0)
    n = len(s)
    total = sum(s)
    if n == 0 or total == 0:
        return 0.0
    cum = sum(i * x for i, x in enumerate(s, start=1))
    return (2.0 * cum) / (n * total) - (n + 1) / n


def compute_board_health(items: list[dict], *, comparable: bool) -> dict:
    """Pure board-health metrics from a board's rank-ordered top-N entries.

    ``items`` carry ``score`` and, when ``comparable``, the day-over-day
    ``is_new`` / ``score_delta`` / ``prev_score`` fields produced by
    ``list_entries_with_deltas``. Competitiveness always computes; turnover and
    score inflation are null when the two snapshots aren't comparable (a reset
    crossed, or no prior snapshot)."""
    out = {
        "leader_share": None, "p1_pn_ratio": None, "gini": None,
        "turnover_rate": None, "new_entrants": None,
        "median_score_gain": None, "median_score_gain_pct": None,
    }
    scores = [float(it["score"]) for it in items if it.get("score") is not None]
    if scores:
        total = sum(scores)
        if total > 0:
            out["leader_share"] = round(scores[0] / total, 4)
        if scores[-1] > 0:
            out["p1_pn_ratio"] = round(scores[0] / scores[-1], 4)
        out["gini"] = round(_gini(scores), 4)
    if comparable and items:
        new_entrants = sum(1 for it in items if it.get("is_new"))
        out["new_entrants"] = new_entrants
        out["turnover_rate"] = round(new_entrants / len(items), 4)
        gains = [float(it["score_delta"]) for it in items
                 if not it.get("is_new") and it.get("score_delta") is not None]
        if gains:
            out["median_score_gain"] = round(_median(gains), 2)
        pcts = [float(it["score_delta"]) / float(it["prev_score"]) for it in items
                if not it.get("is_new") and it.get("prev_score")]
        if pcts:
            out["median_score_gain_pct"] = round(_median(pcts) * 100, 2)
    return out


async def board_health(uuid: int, *, top: int = 50) -> dict | None:
    """Health summary for one board: roster turnover + day-over-day score inflation
    (when the snapshots are comparable, i.e. no reset crossed) and competitiveness
    (score concentration). None when the board has no stored entries."""
    board = await get_board(uuid)
    latest = await pg_store.latest_anchor_for_board(uuid)
    if board is None or latest is None:
        return None
    items, total, comparison = await list_entries_with_deltas(uuid, latest, limit=top, offset=0)
    comparable = bool(comparison.get("comparable"))
    return {
        "uuid": uuid,
        "name": board.get("name"),
        "category": board.get("category"),
        "reset_kind": board.get("reset_kind"),
        "anchor": latest,
        "prev_anchor": comparison.get("prev_anchor"),
        "comparable": comparable,
        "comparison_reason": comparison.get("reason"),
        "population": total,
        "sample_size": len(items),
        **compute_board_health(items, comparable=comparable),
    }


async def player_history_series(player_name: str, *, days: int = 7) -> dict:
    """Score-vs-time trajectories for ONE player, grouped per board, over the last
    ``days`` days, with synthetic reset-zero cliffs."""
    days = max(1, min(days, 30))
    now = int(datetime.now(UTC).timestamp())
    window_start = now - days * 86400
    name = player_name.strip()

    all_rows = await pg_store.player_rows_window(name, window_start)
    if not all_rows:
        return {"player_name": name, "canonical_name": name, "days": days,
                "window_start": window_start, "window_end": now,
                "anchors": [], "series": []}

    canonical = max(all_rows, key=lambda d: d["created_at"])["player_name"]

    by_board: dict[int, list[dict]] = {}
    anchors: set[int] = set()
    for d in all_rows:
        by_board.setdefault(d["leaderboard"], []).append(
            {"created_at": d["created_at"], "rank": d["rank"], "score": d["score"]}
        )
        anchors.add(d["created_at"])
    for pts in by_board.values():
        pts.sort(key=lambda p: p["created_at"])

    meta = await pg_store.board_meta(list(by_board.keys()))

    series = []
    for uuid_, pts in by_board.items():
        best_rank = min(p["rank"] for p in pts)
        m = meta.get(uuid_, {"name": f"Board #{uuid_}", "reset_kind": "default"})
        series.append({
            "uuid": uuid_, "name": m["name"], "best_rank": best_rank,
            "points": _inject_reset_zeros(pts, m["reset_kind"]),
        })
    series.sort(key=lambda s: s["best_rank"])

    return {"player_name": name, "canonical_name": canonical, "days": days,
            "window_start": window_start, "window_end": now,
            "anchors": sorted(anchors), "series": series}

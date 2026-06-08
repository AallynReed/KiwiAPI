"""Write- and read-side helpers for the leaderboards scope.

The insert pipeline:

1. Parse the raw cfg dump (``parser.parse_dump``).
2. Decide the dump's ``created_at`` anchor (latest 11:00 UTC reset, configurable
   via the ``timestamp`` field on the request for back-fills).
3. Upsert one ``Leaderboard`` per board (creating + extending its ``contests``
   list if the board's category id marks the dump as a contest window).
4. Wipe any prior rows for that ``created_at`` (idempotency — re-running the
   same dump on the same timestamp must converge), then insert all the entries
   for THIS dump.
5. Prune entries older than ``LEADERBOARD_RETENTION_DAYS``.

Reads are simple Beanie queries; everything is filtered by exact-equality on
``created_at`` so the (board, created_at, rank) composite index does the work.
"""

from __future__ import annotations

import logging
import re
from datetime import UTC, datetime, timedelta

from beanie.odm.bulk import BulkWriter

from app.core.config import settings
from app.trove.leaderboards.models import (
    Leaderboard,
    LeaderboardEntry,
    LeaderboardEntryArchive,
    contest_type_for,
    effective_reset_kind,
    is_player_board,
    reset_kind,
)
from app.trove.leaderboards.parser import ParsedBoard, parse_dump

logger = logging.getLogger(__name__)

# Hot retention: rows newer than this stay in ``LeaderboardEntry`` (the fast,
# tightly-indexed collection). Older rows are moved to
# ``LeaderboardEntryArchive`` at the tail of each insert. Board metadata is
# never archived — it doesn't grow unbounded.
#
# The actual value is runtime-tunable via
# ``runtime_config.get_setting("leaderboards_hot_retention_days")``. Default
# is in settings.py. Reads go through ``_hot_cutoff()`` below — no module-
# level constant any more, so an admin tweak in the panel takes effect on
# the next insert without a redeploy.

# How many entries to insert per bulk write (caps memory + bulk-op size).
_BULK_BATCH = 1000


# --- timestamp anchoring ----------------------------------------------------

def _trove_day_anchor(now: datetime | None = None) -> int:
    """Today's Trove-day anchor in unix seconds: the most recent 11:00 UTC reset.

    Trove's daily reset is 11:00 UTC. The bot dumps shortly before each reset, so
    the natural anchor is the 11:00 UTC stamp the dump describes — yesterday's
    11:00 if we're before today's 11:00, otherwise today's 11:00.
    """
    real = (now or datetime.now(UTC)).replace(microsecond=0)
    today_11 = real.replace(hour=11, minute=0, second=0)
    anchor = today_11 if real >= today_11 else (today_11 - timedelta(days=1))
    return int(anchor.timestamp())


def normalize_timestamp(ts: int | None) -> int:
    """Validate/normalize a user-supplied ``created_at`` query.

    Accepts any minute-aligned unix timestamp within roughly the recent
    window — back-fills are bounded so a clock-skewed or malicious
    caller can't dump entries at an arbitrary past or future anchor.

    Legacy aliases preserved: 00:00 UTC is still translated to the
    same day's 11:00 UTC so old back-fills continue to work.
    Out-of-window or zero values return -1 so the caller can decide.
    """
    if ts is None or ts <= 0:
        return -1
    parsed = datetime.fromtimestamp(ts, UTC).replace(second=0, microsecond=0)
    now = datetime.now(UTC).replace(second=0, microsecond=0)

    # Clock-skew + abuse guard. 5 min forward catches mild bot skew;
    # 14 d backward covers reasonable back-fills (most data falls off
    # to archive after the hot retention window anyway).
    if parsed > now + timedelta(minutes=5):
        return -1
    if parsed < now - timedelta(days=14):
        return -1

    # Legacy alias: 00:00 UTC of a day is treated as that day's 11:00.
    if parsed.hour == 0 and parsed.minute == 0:
        parsed = parsed.replace(hour=11)

    return int(parsed.timestamp())


# --- insert -----------------------------------------------------------------

async def _upsert_board(parsed: ParsedBoard, created_at: int) -> Leaderboard:
    """Find-or-create the ``Leaderboard`` doc; add a contest record if this dump
    marks the board's current window as a Daily/Weekly contest."""
    lb = await Leaderboard.find_one(Leaderboard.uuid == parsed.uuid)
    if lb is None:
        lb = await Leaderboard(
            uuid=parsed.uuid,
            name_id=parsed.name_id,
            name=parsed.name,
            category_id=parsed.category_id,
            category=parsed.category,
        ).insert()
    ctype = contest_type_for(parsed.category_id)
    if ctype is not None:
        if not any(c.get("time") == created_at for c in lb.contests):
            lb.contests.append({"time": created_at, "type": ctype})
            await lb.save()
    return lb


async def _hot_cutoff() -> tuple[int, int]:
    """Unix-seconds boundary + the days value that produced it.

    Returns ``(cutoff_unix, days)`` — the days are surfaced for logging
    only (``_move_to_archive`` reports them so an operator knows what
    cutoff was applied). Threshold read from runtime config so admin
    panel edits take effect on the next call (5-second cache TTL).
    """
    from app.admin import runtime_config

    days = await runtime_config.get_setting("leaderboards_hot_retention_days")
    cutoff = int((datetime.now(UTC) - timedelta(days=days)).timestamp())
    return cutoff, days


async def archive_query_cutoff() -> int:
    """Unix-seconds boundary: reads for anchors below this count as "archive
    queries" and pay the tighter rate-limit bucket.

    Distinct from ``_hot_cutoff`` — hot retention (storage tier) and archive
    rate-limit threshold (user-facing policy) move independently. A query
    for an anchor in the cold collection but younger than the threshold
    pays only the standard per-token limit; older than the threshold pays
    the archive limit on top.

    The threshold is runtime-tunable from the master admin panel so it
    can move without a redeploy as capture cadence changes.
    """
    from app.admin import runtime_config

    days = await runtime_config.get_setting("leaderboards_archive_query_threshold_days")
    return int((datetime.now(UTC) - timedelta(days=days)).timestamp())


async def is_archive_query(anchor: int) -> bool:
    return anchor < await archive_query_cutoff()


async def _move_to_archive() -> int:
    """Move hot rows older than the retention window into the archive collection.

    Streams in chunks so a one-time burst (e.g. first run after deployment, when
    months of history are eligible at once) doesn't load everything into memory.
    Returns the number of rows moved.
    """
    cutoff, days = await _hot_cutoff()
    moved = 0
    while True:
        # Take a chunk of the oldest hot rows below cutoff.
        chunk = await (
            LeaderboardEntry.find(LeaderboardEntry.created_at < cutoff)
            .sort("+created_at")
            .limit(_BULK_BATCH)
            .to_list()
        )
        if not chunk:
            break
        # Copy → archive in one bulk write…
        async with BulkWriter() as bw:
            for d in chunk:
                await LeaderboardEntryArchive.insert_one(
                    LeaderboardEntryArchive(
                        player_name=d.player_name, rank=d.rank, score=d.score,
                        leaderboard=d.leaderboard, created_at=d.created_at,
                    ),
                    bulk_writer=bw,
                )
        # …then drop them from hot. Filter by _id so we don't accidentally
        # delete any rows added between the find and the delete.
        ids = [d.id for d in chunk]
        await LeaderboardEntry.find({"_id": {"$in": ids}}).delete()
        moved += len(chunk)
    if moved:
        logger.info("leaderboards: moved %d entries older than %d days into archive",
                    moved, days)
    return moved


async def insert_dump(text: str, *, timestamp: int | None = None) -> dict:
    """Parse + persist a dump. Idempotent for a given ``created_at``: re-running
    the same dump on the same timestamp replaces the previous rows for it.

    Returns a small summary dict for logging / debugging.
    """
    boards = parse_dump(text)
    if not boards:
        logger.warning("leaderboards: parsed 0 boards from %d-char dump", len(text))
        return {"boards": 0, "entries": 0, "created_at": None}

    if timestamp is not None and timestamp > 0:
        created_at = normalize_timestamp(timestamp)
        if created_at == -1:
            # Caller asked for an arbitrary stamp — fall back to the default day
            # anchor so the dump still lands somewhere usable.
            created_at = _trove_day_anchor()
    else:
        created_at = _trove_day_anchor()

    # Replace any prior data for this anchor (idempotent re-run).
    res = await LeaderboardEntry.find(LeaderboardEntry.created_at == created_at).delete()
    cleared = getattr(res, "deleted_count", 0) or 0

    entries_total = 0
    for board in boards:
        await _upsert_board(board, created_at)
        if not board.entries:
            continue
        # insert_many handles batching internally — avoid manual BulkWriter
        # commits since BulkWriter.commit() doesn't clear its op queue.
        docs = [
            LeaderboardEntry(
                player_name=ent.player_name,
                rank=ent.rank,
                score=ent.score,
                leaderboard=board.uuid,
                created_at=created_at,
            )
            for ent in board.entries
        ]
        await LeaderboardEntry.insert_many(docs)
        entries_total += len(docs)

    archived = await _move_to_archive()
    logger.info(
        "leaderboards: ingested anchor=%d boards=%d entries=%d cleared=%d archived=%d",
        created_at, len(boards), entries_total, cleared, archived,
    )
    return {
        "boards": len(boards),
        "entries": entries_total,
        "cleared_before_insert": cleared,
        "archived_old": archived,
        "created_at": created_at,
    }


# --- read -------------------------------------------------------------------

async def _entries_collection_for(created_at: int):
    """Hot or cold? Returns the right Beanie Document class for an anchor.
    Async because the cutoff is now runtime-tunable (Mongo lookup, cached)."""
    cutoff, _days = await _hot_cutoff()
    return LeaderboardEntry if created_at >= cutoff else LeaderboardEntryArchive


async def list_timestamps(limit: int = 60, *, include_archive: bool = True) -> list[int]:
    """Distinct ``created_at`` anchors that have entries stored, newest first.

    Unions hot + archive by default so callers see the full history. Set
    ``include_archive=False`` for a fast hot-only listing.
    """
    pipeline = [
        {"$group": {"_id": "$created_at"}},
        {"$sort": {"_id": -1}},
        {"$limit": limit},
    ]
    hot_rows = await LeaderboardEntry.aggregate(pipeline).to_list()
    seen = {r["_id"] for r in hot_rows}
    timestamps = list(seen)
    if include_archive:
        cold_rows = await LeaderboardEntryArchive.aggregate(pipeline).to_list()
        for r in cold_rows:
            if r["_id"] not in seen:
                seen.add(r["_id"])
                timestamps.append(r["_id"])
    timestamps.sort(reverse=True)
    return timestamps[:limit]


async def list_boards_at(created_at: int) -> list[dict]:
    """All boards that have entries stored at ``created_at``, with each board's
    metadata + the contest type (if any) for THIS anchor.

    Routes to hot or archive based on the anchor's age — old anchors go straight
    to the archive collection so the hot collection's index footprint stays
    small."""
    coll = await _entries_collection_for(created_at)
    uuids = await coll.distinct("leaderboard", {"created_at": created_at})
    if not uuids:
        return []
    docs = await Leaderboard.find({"uuid": {"$in": uuids}}).to_list()
    out: list[dict] = []
    for d in docs:
        contest_type = None
        for c in d.contests:
            if c.get("time") == created_at:
                contest_type = c.get("type")
                break
        out.append({
            "uuid": d.uuid,
            "name_id": d.name_id,
            "name": d.name,
            "category_id": d.category_id,
            "category": d.category,
            "contest_type": contest_type,
            # ``effective_reset_kind`` prefers the per-doc admin override
            # over the hardcoded mapping, so a board flipped to "none"
            # in the portal flows through here for detection / activity.
            "reset_kind": effective_reset_kind(d, d.uuid),
            "reset_kind_override": d.reset_kind_override,
            "player_board": is_player_board(d.uuid),
        })
    out.sort(key=lambda b: (b["category"], b["name"]))
    return out


async def get_board(uuid: int) -> dict | None:
    """A single board's metadata (no entries)."""
    d = await Leaderboard.find_one(Leaderboard.uuid == uuid)
    if d is None:
        return None
    return {
        "uuid": d.uuid,
        "name_id": d.name_id,
        "name": d.name,
        "category_id": d.category_id,
        "category": d.category,
        "reset_kind": effective_reset_kind(d, d.uuid),
        "reset_kind_override": d.reset_kind_override,
        "player_board": is_player_board(d.uuid),
        "contests": list(d.contests),
    }


async def list_entries(
    uuid: int, created_at: int, *, limit: int = 100, offset: int = 0,
) -> tuple[list[dict], int]:
    """Top-N entries for a board at a given timestamp, ranked.

    Routes the query to ``LeaderboardEntry`` (hot) or ``LeaderboardEntryArchive``
    (cold) based on the anchor's age — the read never spans both collections,
    so the planner picks the same composite index either way.
    """
    coll = await _entries_collection_for(created_at)
    query = {"leaderboard": uuid, "created_at": created_at}
    total = await coll.find(query).count()
    docs = (
        await coll.find(query)
        .sort("+rank")
        .skip(offset)
        .limit(limit)
        .to_list()
    )
    items = [
        {"player_name": d.player_name, "rank": d.rank, "score": d.score}
        for d in docs
    ]
    return items, total


async def player_history(
    player_name: str, *, limit: int = 50, uuid: int | None = None,
    include_archive: bool = True,
) -> list[dict]:
    """Most recent dumps that featured a player. Optional board filter.

    Match is case-insensitive: players type with whatever casing they
    remember and the captured ``player_name`` is whatever Trove stored.
    Implemented as an anchored ``$regex`` with the ``i`` option. This
    bypasses the ``(player_name, created_at)`` prefix index for the name
    component, but the secondary sort key still pages efficiently and
    the limit caps the result set; the hot collection is 3 days small.

    Queries hot first and falls through to archive only if we haven't
    filled the requested ``limit`` yet — recent activity is the common
    case, so most calls never touch the cold collection.
    """
    name = player_name.strip()
    # Anchored regex with the ``i`` option for case-insensitive equality.
    # ``re.escape`` so names containing regex metacharacters (dots,
    # parens, etc.) match literally.
    ci_match = {"$regex": f"^{re.escape(name)}$", "$options": "i"}
    query: dict = {"player_name": ci_match}
    if uuid is not None:
        query["leaderboard"] = uuid

    hot_docs = (
        await LeaderboardEntry.find(query)
        .sort("-created_at")
        .limit(limit)
        .to_list()
    )
    out = [
        {
            "player_name": d.player_name, "rank": d.rank, "score": d.score,
            "leaderboard": d.leaderboard, "created_at": d.created_at,
        }
        for d in hot_docs
    ]
    if include_archive and len(out) < limit:
        remaining = limit - len(out)
        cold_docs = (
            await LeaderboardEntryArchive.find(query)
            .sort("-created_at")
            .limit(remaining)
            .to_list()
        )
        out.extend(
            {
                "player_name": d.player_name, "rank": d.rank, "score": d.score,
                "leaderboard": d.leaderboard, "created_at": d.created_at,
            }
            for d in cold_docs
        )
    return out


async def board_history(
    uuid: int, *, days: int = 7, top: int = 5,
) -> dict:
    """Score-vs-time trajectories for the current top-``top`` players on a
    board over the last ``days`` days of hourly captures. Drives the
    per-board chart on the public leaderboards page.

    "Current top" is defined as the top ``top`` ranks at the *most recent*
    anchor in the window — gives the chart a coherent story (the players
    you can see on the entries table today, plus where they were yesterday
    and the day before). A player who briefly cracked the top-N then
    dropped off won't appear; we accept that to keep the chart legible.

    Routing: spans hot and archive transparently. Hot retention is 3 days
    by default and the window is 7, so most queries hit both collections.
    The composite ``(leaderboard, created_at, rank)`` index covers the
    board+window predicate; the ``player_name`` filter then narrows in
    memory across at most ``top * 168`` rows (7 days × 24 hourly anchors).

    Returns ``{uuid, days, window_start, window_end, anchors, series}``
    where ``anchors`` is every distinct ``created_at`` in the window,
    ascending, and ``series`` is one entry per top-player with their
    sorted ``points`` (``{created_at, rank, score}`` triples — missing
    anchors mean the player wasn't in that capture's stored slice and the
    chart should leave a gap).
    """
    days = max(1, min(days, 30))   # clamp — sanity, archive is the limit
    top = max(1, min(top, 20))     # 20 lines is already a busy chart
    now = int(datetime.now(UTC).timestamp())
    window_start = now - days * 86400

    # 1) Distinct anchors in window for this board, hot ∪ archive.
    pipeline = [
        {"$match": {"leaderboard": uuid, "created_at": {"$gte": window_start}}},
        {"$group": {"_id": "$created_at"}},
        {"$sort": {"_id": -1}},
    ]
    hot_anchors = {r["_id"] for r in await LeaderboardEntry.aggregate(pipeline).to_list()}
    cold_anchors = {r["_id"] for r in await LeaderboardEntryArchive.aggregate(pipeline).to_list()}
    anchors = sorted(hot_anchors | cold_anchors)   # ascending for the chart
    if not anchors:
        return {
            "uuid": uuid, "days": days,
            "window_start": window_start, "window_end": now,
            "anchors": [], "series": [],
        }

    # 2) Latest anchor → its top-N players define the chart's line set.
    latest = anchors[-1]
    latest_coll = await _entries_collection_for(latest)
    top_docs = (
        await latest_coll.find({"leaderboard": uuid, "created_at": latest})
        .sort("+rank")
        .limit(top)
        .to_list()
    )
    if not top_docs:
        return {
            "uuid": uuid, "days": days,
            "window_start": window_start, "window_end": now,
            "anchors": anchors, "series": [],
        }
    names = [d.player_name for d in top_docs]
    current_rank = {d.player_name: d.rank for d in top_docs}

    # 3) All (player, created_at, rank, score) rows for those players in
    # window, from both collections. ``name in names`` reuses the
    # canonical casing from step 2 — Trove keeps player_name stable per
    # account so cross-anchor exact-match is reliable. Small set; one
    # round-trip per collection is enough.
    row_query = {
        "leaderboard": uuid,
        "created_at": {"$gte": window_start},
        "player_name": {"$in": names},
    }
    hot_rows = await LeaderboardEntry.find(row_query).to_list()
    cold_rows = await LeaderboardEntryArchive.find(row_query).to_list()

    # 4) Bucket by player_name → ascending points by created_at.
    per_player: dict[str, list[dict]] = {n: [] for n in names}
    for d in hot_rows + cold_rows:
        per_player.setdefault(d.player_name, []).append(
            {"created_at": d.created_at, "rank": d.rank, "score": d.score},
        )
    for pts in per_player.values():
        pts.sort(key=lambda p: p["created_at"])

    series = [
        {
            "player_name": n,
            "current_rank": current_rank.get(n),
            "points": per_player[n],
        }
        for n in names   # preserve rank order from step 2
    ]
    return {
        "uuid": uuid, "days": days,
        "window_start": window_start, "window_end": now,
        "anchors": anchors, "series": series,
    }


async def player_history_series(
    player_name: str, *, days: int = 7,
) -> dict:
    """Score-vs-time trajectories for ONE player, grouped per board, over
    the last ``days`` days of captures. Drives the per-player chart on
    the public leaderboards page.

    Differs from ``player_history`` in two ways:
      • bounded by a time window, not a row-count limit (full coverage
        of the chart range, not "the 50 most recent rows");
      • grouped by ``leaderboard`` so the caller can draw one line per
        board the player appears on, rather than a flat row list.

    Case-insensitive name match — same convention as ``player_history``.
    Returns ``{player_name, canonical_name, days, window_start,
    window_end, anchors, series}``. ``anchors`` is every distinct
    ``created_at`` the player has rows for in the window. ``series`` is
    one entry per board they appeared on, each with its sorted
    ``points`` and resolved board ``name`` (when known).
    """
    days = max(1, min(days, 30))
    now = int(datetime.now(UTC).timestamp())
    window_start = now - days * 86400

    name = player_name.strip()
    ci_match = {"$regex": f"^{re.escape(name)}$", "$options": "i"}
    query = {"player_name": ci_match, "created_at": {"$gte": window_start}}

    hot_rows = await LeaderboardEntry.find(query).to_list()
    cold_rows = await LeaderboardEntryArchive.find(query).to_list()
    all_rows = hot_rows + cold_rows

    if not all_rows:
        return {
            "player_name": name, "canonical_name": name, "days": days,
            "window_start": window_start, "window_end": now,
            "anchors": [], "series": [],
        }

    # Canonical name: whichever casing the most-recent row has. Same
    # principle as the cheaters panel — Trove's stored casing wins over
    # whatever the user typed.
    canonical = max(all_rows, key=lambda d: d.created_at).player_name

    by_board: dict[int, list[dict]] = {}
    anchors: set[int] = set()
    for d in all_rows:
        by_board.setdefault(d.leaderboard, []).append(
            {"created_at": d.created_at, "rank": d.rank, "score": d.score},
        )
        anchors.add(d.created_at)
    for pts in by_board.values():
        pts.sort(key=lambda p: p["created_at"])

    # Resolve board names. One round-trip with $in keeps it cheap even
    # for a prolific player.
    board_docs = await Leaderboard.find({"uuid": {"$in": list(by_board.keys())}}).to_list()
    board_name = {b.uuid: b.name for b in board_docs}

    series = []
    for uuid_, pts in by_board.items():
        # Sort series by "best rank achieved this window" so the most
        # competitive board renders first in the chart legend.
        best_rank = min(p["rank"] for p in pts)
        series.append({
            "uuid": uuid_,
            "name": board_name.get(uuid_, f"Board #{uuid_}"),
            "best_rank": best_rank,
            "points": pts,
        })
    series.sort(key=lambda s: s["best_rank"])

    return {
        "player_name": name, "canonical_name": canonical, "days": days,
        "window_start": window_start, "window_end": now,
        "anchors": sorted(anchors), "series": series,
    }

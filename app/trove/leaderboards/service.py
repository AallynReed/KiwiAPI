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
from datetime import UTC, datetime, timedelta

from beanie.odm.bulk import BulkWriter

from app.core.config import settings
from app.trove.leaderboards.models import (
    Leaderboard,
    LeaderboardEntry,
    LeaderboardEntryArchive,
    contest_type_for,
    is_player_board,
    reset_kind,
)
from app.trove.leaderboards.parser import ParsedBoard, parse_dump

logger = logging.getLogger(__name__)

# Hot retention: rows newer than this stay in ``LeaderboardEntry`` (the fast,
# tightly-indexed collection). Older rows are moved to
# ``LeaderboardEntryArchive`` at the tail of each insert. Board metadata is
# never archived — it doesn't grow unbounded.
LEADERBOARD_HOT_RETENTION_DAYS = 30

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

    Accepts either the canonical 11:00 UTC anchor or 00:00 UTC (which we translate
    to that day's 11:00). Anything else returns ``None``-equivalent (-1) so the
    caller can decide how to respond.
    """
    if ts is None or ts <= 0:
        return -1
    parsed = datetime.fromtimestamp(ts, UTC).replace(minute=0, second=0, microsecond=0)
    if parsed.hour == 0:
        parsed = parsed.replace(hour=11)
    elif parsed.hour != 11:
        return -1
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


def _hot_cutoff() -> int:
    """Unix-seconds boundary: rows with ``created_at < this`` belong in archive."""
    return int(
        (datetime.now(UTC) - timedelta(days=LEADERBOARD_HOT_RETENTION_DAYS)).timestamp()
    )


def archive_query_cutoff() -> int:
    """Unix-seconds boundary: reads for anchors below this count as "archive
    queries" and pay the tighter rate-limit bucket.

    Distinct from ``_hot_cutoff`` — hot retention (storage tier) and archive
    rate-limit threshold (user-facing policy) move independently. A query for
    days 30–90 hits the cold collection but uses the normal per-token limit;
    a query for >90 days pays the archive limit on top.
    """
    days = settings.leaderboards_archive_query_threshold_days
    return int((datetime.now(UTC) - timedelta(days=days)).timestamp())


def is_archive_query(anchor: int) -> bool:
    return anchor < archive_query_cutoff()


async def _move_to_archive() -> int:
    """Move hot rows older than the retention window into the archive collection.

    Streams in chunks so a one-time burst (e.g. first run after deployment, when
    months of history are eligible at once) doesn't load everything into memory.
    Returns the number of rows moved.
    """
    cutoff = _hot_cutoff()
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
                    moved, LEADERBOARD_HOT_RETENTION_DAYS)
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
        async with BulkWriter() as bw:
            for ent in board.entries:
                await LeaderboardEntry.insert_one(
                    LeaderboardEntry(
                        player_name=ent.player_name,
                        rank=ent.rank,
                        score=ent.score,
                        leaderboard=board.uuid,
                        created_at=created_at,
                    ),
                    bulk_writer=bw,
                )
                entries_total += 1
                if entries_total % _BULK_BATCH == 0:
                    await bw.commit()

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

def _entries_collection_for(created_at: int):
    """Hot or cold? Returns the right Beanie Document class for an anchor."""
    return LeaderboardEntry if created_at >= _hot_cutoff() else LeaderboardEntryArchive


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
    coll = _entries_collection_for(created_at)
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
            "reset_kind": reset_kind(d.uuid),
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
        "reset_kind": reset_kind(d.uuid),
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
    coll = _entries_collection_for(created_at)
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

    Queries hot first and falls through to archive only if we haven't filled
    the requested ``limit`` yet — recent activity is the common case, so most
    calls never touch the cold collection.
    """
    query: dict = {"player_name": player_name}
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

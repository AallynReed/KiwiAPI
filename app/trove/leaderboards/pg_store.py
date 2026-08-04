"""PostgreSQL data layer for the leaderboards domain.

All raw-SQL access to the partitioned ``entry`` table + ``board`` / ``player`` /
``board_contest`` / ``activity_estimate``. The higher layers (``service`` /
``activity`` / ``detection``) keep their compute logic and call these primitives;
the Redis cache layer (``cache.py``) sits in front of the read paths unchanged.

Returns plain dicts (same shapes the old Mongo helpers returned) so the callers
don't care that the bytes now come from Postgres. Bulk insert uses ``COPY`` into a
temp staging table → set-based player upsert → ``INSERT … SELECT`` into the
anchor's day-partition (sub-second for a ~720k-row snapshot).
"""
from __future__ import annotations

import json

from app.core.postgres import acquire
from app.trove.leaderboards import pg_schema
from app.trove.leaderboards.models import (
    RESET_KIND_VALUES,
    is_player_board,
    reset_kind,
)
from app.trove.leaderboards.parser import ParsedBoard

_RESET_HOUR_UTC = 11


def _effective_reset_kind(override: str | None, uuid: int) -> str:
    """Admin override (if a valid value) else the hardcoded ``models.reset_kind``."""
    if isinstance(override, str) and override in RESET_KIND_VALUES:
        return override
    return reset_kind(uuid)


def trove_day_start(anchor: int) -> int:
    """The 11:00-UTC reset that opens ``anchor``'s trove-day (unix seconds)."""
    return ((anchor - _RESET_HOUR_UTC * 3600) // 86400) * 86400 + _RESET_HOUR_UTC * 3600


# ── write ─────────────────────────────────────────────────────────────────────

async def write_snapshot(boards: list[ParsedBoard], anchor: int) -> dict:
    """Persist one parsed dump at ``anchor`` (idempotent: replaces the anchor).

    COPY the rows into a temp table, upsert boards + players, then INSERT…SELECT
    the entries (resolving player_id) into the day-partition - all in one
    transaction so readers never see a partial snapshot."""
    board_rows = [
        (b.uuid, b.name_id, b.name, b.category_id, b.category) for b in boards
    ]
    contest_rows = [(b.uuid, anchor, b.contest) for b in boards if b.contest]
    entries_total = sum(len(b.entries) for b in boards)

    def _records():
        for b in boards:
            u = b.uuid
            for e in b.entries:
                yield (u, anchor, e.rank, e.score, e.player_name)

    async with acquire() as con:
        async with con.transaction():
            await pg_schema.ensure_partition(con, anchor)
            res = await con.execute("DELETE FROM entry WHERE anchor = $1", anchor)
            cleared = int(res.split()[-1]) if res.startswith("DELETE") else 0

            if board_rows:
                await con.executemany(
                    "INSERT INTO board (uuid, name_id, name, category_id, category) "
                    "VALUES ($1, $2, $3, $4, $5) "
                    "ON CONFLICT (uuid) DO UPDATE SET "
                    "name_id = EXCLUDED.name_id, name = EXCLUDED.name, "
                    "category_id = EXCLUDED.category_id, category = EXCLUDED.category",
                    board_rows,
                )
            if contest_rows:
                await con.executemany(
                    "INSERT INTO board_contest (board_uuid, anchor, type) "
                    "VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
                    contest_rows,
                )

            await con.execute(
                "CREATE TEMP TABLE _stg (board_uuid int, anchor bigint, rank int, "
                "score float8, name text) ON COMMIT DROP"
            )
            await con.copy_records_to_table(
                "_stg", records=_records(),
                columns=["board_uuid", "anchor", "rank", "score", "name"],
            )
            # New players → assign ids (keep latest casing). Then resolve every
            # entry's player_id by joining the staging rows back to player.
            await con.execute(
                "INSERT INTO player (name, name_lower) "
                "SELECT DISTINCT ON (lower(name)) name, lower(name) FROM _stg "
                "ORDER BY lower(name) "
                "ON CONFLICT (name_lower) DO UPDATE SET name = EXCLUDED.name"
            )
            await con.execute(
                "INSERT INTO entry (board_uuid, anchor, rank, score, player_id) "
                "SELECT s.board_uuid, s.anchor, s.rank, s.score, p.id "
                "FROM _stg s JOIN player p ON p.name_lower = lower(s.name)"
            )
            # Record case-only name collisions while the RAW spellings are still
            # in reach. The player upsert above folds "Robot" and "robot" into one
            # row keyed by name_lower, so after this transaction there is no way
            # to tell that two distinct accounts were merged - this is the only
            # point where that evidence exists.
            await _record_case_collisions(con, anchor)
            # Fold this capture into the per-(player, board) lifetime aggregate
            # in the SAME transaction, so the /player profile reads it in
            # O(boards) instead of scanning the player's full history.
            await _fold_anchor_into_agg(con, anchor)
    return {
        "boards": len(boards), "entries": entries_total, "created_at": anchor,
        "cleared_before_insert": cleared, "archived_old": 0,
    }


async def reset_all(*, drop_boards: bool = False) -> dict:
    """Wipe the leaderboards data. TRUNCATE is metadata-only (no per-row scan)."""
    async with acquire() as con:
        entries = await con.fetchval(
            "SELECT COALESCE(SUM(c.reltuples)::bigint, 0) FROM pg_inherits i "
            "JOIN pg_class c ON c.oid = i.inhrelid WHERE i.inhparent = 'entry'::regclass"
        )
        estimates = await con.fetchval("SELECT count(*) FROM activity_estimate")
        async with con.transaction():
            await con.execute(
                "TRUNCATE entry, player, activity_estimate, activity_active, "
                "class_activity_estimate, player_rename, player_duplicate, "
                "player_board_agg RESTART IDENTITY"
            )
            boards = 0
            if drop_boards:
                boards = await con.fetchval("SELECT count(*) FROM board")
                await con.execute("TRUNCATE board, board_contest")
    return {
        "hot_entries_deleted": int(entries or 0),
        "archive_entries_deleted": 0,
        "activity_estimates_deleted": int(estimates or 0),
        "boards_deleted": int(boards or 0),
        "dropped_boards": drop_boards,
    }


# ── reads ─────────────────────────────────────────────────────────────────────

async def list_timestamps(limit: int = 60, since: int | None = None) -> list[int]:
    # Loose index scan (skip scan): seek the newest anchor, then repeatedly the
    # next-lower one, stopping at ``limit``. Each step is an index MAX using the
    # per-partition ``entry_p_*_anchor_idx``, so this early-stops after ~``limit``
    # seeks. A plain ``SELECT DISTINCT anchor ... LIMIT`` can't early-stop - it
    # HashAggregates the WHOLE table (grown to ~228M rows across 38 partitions, 31
    # on the slow cold-tier disk → ~2 min), which blew past the 120s statement
    # timeout and froze the warmer's snapshot publish (page stuck on a stale
    # anchor). The skip scan returns the same newest-first anchors in ~6s.
    #
    # ``since`` (unix seconds) FLOORS the walk: the recursive seek also requires
    # ``anchor >= since``, so it stops the moment it drops below the floor and -
    # crucially - partition pruning skips every partition older than ``since``
    # outright. A windowed backfill ("last day") MUST pass this: without it the
    # walk enumerates a year of anchors across the cold tier and hits the 120s
    # command_timeout. ``None`` keeps the original unbounded whole-history walk.
    async with acquire() as con:
        if since is None:
            rows = await con.fetch(
                """
                WITH RECURSIVE a AS (
                    SELECT (SELECT max(anchor) FROM entry) AS anchor
                    UNION ALL
                    SELECT (SELECT max(e.anchor) FROM entry e WHERE e.anchor < a.anchor)
                    FROM a WHERE a.anchor IS NOT NULL
                )
                SELECT anchor FROM a WHERE anchor IS NOT NULL LIMIT $1
                """,
                limit,
            )
        else:
            rows = await con.fetch(
                """
                WITH RECURSIVE a AS (
                    SELECT (SELECT max(anchor) FROM entry WHERE anchor >= $2) AS anchor
                    UNION ALL
                    SELECT (SELECT max(e.anchor) FROM entry e
                            WHERE e.anchor < a.anchor AND e.anchor >= $2)
                    FROM a WHERE a.anchor IS NOT NULL
                )
                SELECT anchor FROM a WHERE anchor IS NOT NULL LIMIT $1
                """,
                limit, since,
            )
    return [r["anchor"] for r in rows]


# Trove-day boundary: captures roll over at 11:00 UTC (the UTC-11 server day), so
# a trove-day is the window [11:00 UTC, next-day 11:00 UTC).
_TROVE_DAY_OFFSET = 11 * 3600


async def list_days(limit: int = 40) -> list[int]:
    """The LATEST anchor of each trove-day, newest first, for up to ``limit`` days.

    Powers the archive date-picker: it needs one representative capture per day, not
    every hourly one. Like ``list_timestamps`` this is a loose index scan, but each
    step jumps to the start of the current anchor's trove-day and takes the max
    anchor strictly before it - i.e. one index seek PER DAY. So even the full
    cold-tiered archive is ~40 seeks, not a table scan. Stops early when the archive
    runs out (the recursive term yields NULL)."""
    async with acquire() as con:
        rows = await con.fetch(
            """
            WITH RECURSIVE d AS (
                SELECT (SELECT max(anchor) FROM entry) AS anchor
                UNION ALL
                SELECT (SELECT max(e.anchor) FROM entry e
                        WHERE e.anchor < ((d.anchor - $2) / 86400) * 86400 + $2)
                FROM d WHERE d.anchor IS NOT NULL
            )
            SELECT anchor FROM d WHERE anchor IS NOT NULL LIMIT $1
            """,
            limit, _TROVE_DAY_OFFSET,
        )
    return [r["anchor"] for r in rows]


async def previous_anchor_before(anchor: int) -> int | None:
    """The most recent stored capture strictly before ``anchor`` (any board) - the
    baseline the completeness guard judges a new dump against."""
    async with acquire() as con:
        return await con.fetchval("SELECT MAX(anchor) FROM entry WHERE anchor < $1", anchor)


async def board_counts_at(anchor: int) -> dict[int, int]:
    """``{board_uuid: entry_count}`` at one anchor - the board-presence + per-board
    population snapshot the completeness guard compares (anchor is constant, so
    Postgres prunes to that day's partition)."""
    async with acquire() as con:
        rows = await con.fetch(
            "SELECT board_uuid, count(*) AS n FROM entry WHERE anchor = $1 GROUP BY board_uuid",
            anchor,
        )
    return {r["board_uuid"]: int(r["n"]) for r in rows}


async def delete_anchor(anchor: int) -> int:
    """Delete every entry row for one capture (a bad/rejected capture cleanup).
    Returns the number of rows removed."""
    async with acquire() as con:
        res = await con.execute("DELETE FROM entry WHERE anchor = $1", anchor)
    return int(res.split()[-1]) if res.startswith("DELETE") else 0


async def list_boards_at(anchor: int) -> list[dict]:
    async with acquire() as con:
        rows = await con.fetch(
            "SELECT b.uuid, b.name_id, b.name, b.category_id, b.category, "
            "       b.reset_kind_override, bc.type AS contest_type "
            "FROM (SELECT DISTINCT board_uuid FROM entry WHERE anchor = $1) e "
            "JOIN board b ON b.uuid = e.board_uuid "
            "LEFT JOIN board_contest bc ON bc.board_uuid = b.uuid AND bc.anchor = $1 "
            "ORDER BY b.category, b.name",
            anchor,
        )
    return [
        {
            "uuid": r["uuid"], "name_id": r["name_id"], "name": r["name"],
            "category_id": r["category_id"], "category": r["category"],
            "contest_type": r["contest_type"],
            "reset_kind": _effective_reset_kind(r["reset_kind_override"], r["uuid"]),
            "reset_kind_override": r["reset_kind_override"],
            "player_board": is_player_board(r["uuid"]),
        }
        for r in rows
    ]


async def get_board(uuid: int) -> dict | None:
    async with acquire() as con:
        b = await con.fetchrow(
            "SELECT uuid, name_id, name, category_id, category, reset_kind_override "
            "FROM board WHERE uuid = $1", uuid,
        )
        if b is None:
            return None
        contests = await con.fetch(
            "SELECT anchor AS time, type FROM board_contest WHERE board_uuid = $1 "
            "ORDER BY anchor", uuid,
        )
    return {
        "uuid": b["uuid"], "name_id": b["name_id"], "name": b["name"],
        "category_id": b["category_id"], "category": b["category"],
        "reset_kind": _effective_reset_kind(b["reset_kind_override"], b["uuid"]),
        "reset_kind_override": b["reset_kind_override"],
        "player_board": is_player_board(b["uuid"]),
        "contests": [{"time": c["time"], "type": c["type"]} for c in contests],
    }


async def latest_anchor_for_board(uuid: int) -> int | None:
    """The most recent anchor that has entries for ``uuid`` (None if never stored)."""
    async with acquire() as con:
        return await con.fetchval("SELECT MAX(anchor) FROM entry WHERE board_uuid = $1", uuid)


async def top_entries_for_boards(uuids: list[int], anchor: int) -> dict[int, dict]:
    """The rank-1 entry for each of ``uuids`` at one ``anchor`` (the caller
    passes the latest published snapshot; rank-1 is the highest score).

    Powers the free Mastery / Power Rank "records" endpoint. ``anchor`` is a
    CONSTANT here, so Postgres PRUNES the RANGE-partitioned ``entry`` table to
    that single day's partition and reads rank-1 per board straight off the
    ``(board_uuid, anchor, rank)`` index (``DISTINCT ON`` + ``ORDER BY rank``).
    Boards absent from that snapshot are simply missing from the result.
    """
    if not uuids:
        return {}
    async with acquire() as con:
        rows = await con.fetch(
            "SELECT DISTINCT ON (e.board_uuid) "
            "       e.board_uuid, p.name AS player_name, e.score, e.anchor "
            "FROM entry e "
            "JOIN player p ON p.id = e.player_id "
            "WHERE e.board_uuid = ANY($1) AND e.anchor = $2 "
            "ORDER BY e.board_uuid, e.rank",
            uuids, anchor,
        )
    return {
        r["board_uuid"]: {
            "player_name": r["player_name"],
            "score": r["score"],
            "anchor": r["anchor"],
        }
        for r in rows
    }


async def list_entries(uuid: int, anchor: int, *, limit: int, offset: int) -> tuple[list[dict], int]:
    async with acquire() as con:
        total = await con.fetchval(
            "SELECT count(*) FROM entry WHERE board_uuid = $1 AND anchor = $2", uuid, anchor,
        )
        rows = await con.fetch(
            "SELECT p.name AS player_name, e.rank, e.score "
            "FROM entry e JOIN player p ON p.id = e.player_id "
            "WHERE e.board_uuid = $1 AND e.anchor = $2 "
            "ORDER BY e.rank LIMIT $3 OFFSET $4",
            uuid, anchor, limit, offset,
        )
    items = [
        {"player_name": r["player_name"], "rank": r["rank"], "score": r["score"]}
        for r in rows
    ]
    return items, int(total or 0)


async def entries_by_board_at(anchor: int, uuids: list[int] | None = None) -> dict[int, list[dict]]:
    """All entries at ``anchor`` grouped by board, sorted by rank - the bulk
    loader for cheater detection + the activity 1h breakdown."""
    sql = (
        "SELECT e.board_uuid, p.name AS player_name, e.rank, e.score "
        "FROM entry e JOIN player p ON p.id = e.player_id WHERE e.anchor = $1"
    )
    args: list = [anchor]
    if uuids is not None:
        sql += " AND e.board_uuid = ANY($2)"
        args.append(uuids)
    sql += " ORDER BY e.board_uuid, e.rank"
    async with acquire() as con:
        rows = await con.fetch(sql, *args)
    out: dict[int, list[dict]] = {}
    for r in rows:
        out.setdefault(r["board_uuid"], []).append(
            {"player_name": r["player_name"], "rank": r["rank"], "score": r["score"]}
        )
    return out


async def anchor_maps(anchor: int, board_uuids: list[int]) -> dict[int, dict[str, float]]:
    """``{uuid: {player_name: score}}`` at one anchor - the activity unit."""
    if not board_uuids:
        return {}
    async with acquire() as con:
        rows = await con.fetch(
            "SELECT e.board_uuid, p.name AS player_name, e.score "
            "FROM entry e JOIN player p ON p.id = e.player_id "
            "WHERE e.anchor = $1 AND e.board_uuid = ANY($2)",
            anchor, board_uuids,
        )
    out: dict[int, dict[str, float]] = {}
    for r in rows:
        out.setdefault(r["board_uuid"], {})[r["player_name"]] = r["score"]
    return out


# ── per-window active-player set (the 24h/7d rollup source) ───────────────────

async def record_active_window(window_end: int, names) -> None:
    """Materialize one capture window's DISTINCT active players (lower-cased names).
    Idempotent: replaces any existing rows for ``window_end`` (a re-ingest onto the
    same anchor converges). Called once per capture by the warmer + the backfill."""
    rows = sorted({n.lower() for n in names if n})
    async with acquire() as con:
        async with con.transaction():
            await con.execute("DELETE FROM activity_active WHERE window_end = $1", window_end)
            if rows:
                await con.executemany(
                    "INSERT INTO activity_active (window_end, player_lower) VALUES ($1, $2) "
                    "ON CONFLICT DO NOTHING",
                    [(window_end, n) for n in rows],
                )


async def count_active_since(early: int, late: int) -> int:
    """Distinct active players across every materialized window in ``(early, late]``
    - the true distinct UNION over the period, as a single indexed COUNT(DISTINCT).
    ``early`` (the window-start anchor) is excluded; ``late`` is included."""
    async with acquire() as con:
        val = await con.fetchval(
            "SELECT COUNT(DISTINCT player_lower) FROM activity_active "
            "WHERE window_end > $1 AND window_end <= $2",
            early, late,
        )
    return int(val or 0)


async def prune_active_windows(cutoff: int) -> int:
    """Drop materialized windows older than ``cutoff`` (rolling retention). The 7d
    rollup never reads further back, so anything older is dead weight."""
    async with acquire() as con:
        res = await con.execute("DELETE FROM activity_active WHERE window_end < $1", cutoff)
    return int(res.split()[-1]) if res.startswith("DELETE") else 0


async def delete_all_active_windows() -> int:
    async with acquire() as con:
        res = await con.execute("DELETE FROM activity_active")
    return int(res.split()[-1]) if res.startswith("DELETE") else 0


async def previous_day_anchor(uuid: int, anchor: int) -> int | None:
    """Latest stored anchor for ``uuid`` from a strictly-earlier trove-day."""
    day_start = trove_day_start(anchor)
    async with acquire() as con:
        return await con.fetchval(
            "SELECT MAX(anchor) FROM entry WHERE board_uuid = $1 AND anchor < $2",
            uuid, day_start,
        )


async def prev_rows_for_players(uuid: int, prev_anchor: int, names: list[str]) -> dict[str, dict]:
    """``{player_name_lower: {rank, score}}`` for ``names`` on a board at one
    earlier anchor - feeds the entries-table day-over-day deltas."""
    if not names:
        return {}
    lowered = [n.lower() for n in names]
    async with acquire() as con:
        rows = await con.fetch(
            "SELECT p.name_lower, e.rank, e.score "
            "FROM entry e JOIN player p ON p.id = e.player_id "
            "WHERE e.board_uuid = $1 AND e.anchor = $2 AND p.name_lower = ANY($3)",
            uuid, prev_anchor, lowered,
        )
    return {r["name_lower"]: {"rank": r["rank"], "score": r["score"]} for r in rows}


async def comovement_series(
    board_uuid: int, player_names: list[str], since_anchor: int, until_anchor: int,
) -> dict[str, dict[int, float]]:
    """``{player_name: {anchor: score}}`` for the given players on one board
    across EVERY capture in ``[since_anchor, until_anchor]`` - the per-hour
    series the co-movement detector diffs into lockstep gains.

    Candidate-limited by the caller (top-N by rank) so a board of tens of
    thousands of accounts doesn't load its whole week. One query per board."""
    if not player_names:
        return {}
    lowered = [n.lower() for n in player_names]
    async with acquire() as con:
        rows = await con.fetch(
            "SELECT p.name AS player_name, e.anchor, e.score "
            "FROM entry e JOIN player p ON p.id = e.player_id "
            "WHERE e.board_uuid = $1 AND p.name_lower = ANY($2) "
            "AND e.anchor >= $3 AND e.anchor <= $4",
            board_uuid, lowered, since_anchor, until_anchor,
        )
    out: dict[str, dict[int, float]] = {}
    for r in rows:
        out.setdefault(r["player_name"], {})[r["anchor"]] = r["score"]
    return out


async def previous_captures_bulk(player_names: list[str], board_uuid: int,
                                 anchor: int, cycle_start: int) -> dict[str, tuple[float, int]]:
    """Latest ``(score, anchor)`` strictly before ``anchor`` and ``>= cycle_start``
    for MANY players on one board - the velocity baseline (one query)."""
    if not player_names:
        return {}
    lowered = [n.lower() for n in player_names]
    async with acquire() as con:
        rows = await con.fetch(
            "SELECT DISTINCT ON (p.name_lower) p.name AS player_name, e.score, e.anchor "
            "FROM entry e JOIN player p ON p.id = e.player_id "
            "WHERE e.board_uuid = $1 AND p.name_lower = ANY($2) "
            "AND e.anchor < $3 AND e.anchor >= $4 "
            "ORDER BY p.name_lower, e.anchor DESC",
            board_uuid, lowered, anchor, cycle_start,
        )
    return {r["player_name"]: (r["score"], r["anchor"]) for r in rows}


async def player_rows(
    name: str, *, limit: int, uuid: int | None = None, window_start: int | None = None,
) -> list[dict]:
    """Most recent appearances of one player (case-insensitive), newest first.

    ``window_start`` (unix seconds) bounds the scan to anchors at/after it, which
    lets Postgres PRUNE to recent partitions instead of merge-scanning every
    historical partition to find the newest rows - the difference between a
    sub-second read and tens of seconds for a player with years of history. Pass
    it whenever the caller only needs recent appearances (the profile's ``recent``
    and the leaderboards panel, which shows only the latest capture)."""
    sql = (
        "SELECT e.board_uuid AS leaderboard, e.anchor AS created_at, e.rank, e.score, "
        "p.name AS player_name "
        "FROM entry e JOIN player p ON p.id = e.player_id WHERE p.name_lower = lower($1)"
    )
    args: list = [name]
    if window_start is not None:
        args.append(window_start)
        sql += f" AND e.anchor >= ${len(args)}"
    if uuid is not None:
        args.append(uuid)
        sql += f" AND e.board_uuid = ${len(args)}"
    args.append(limit)
    sql += f" ORDER BY e.anchor DESC LIMIT ${len(args)}"
    async with acquire() as con:
        rows = await con.fetch(sql, *args)
    return [
        {"player_name": r["player_name"], "rank": r["rank"], "score": r["score"],
         "leaderboard": r["leaderboard"], "created_at": r["created_at"]}
        for r in rows
    ]


async def player_canonical_name(name: str) -> str | None:
    """The stored (latest-casing) display name for a player, or None if unknown.
    One indexed lookup on the unique ``name_lower`` - cheap, so the profile can
    resolve the canonical name without scanning the player's history."""
    async with acquire() as con:
        return await con.fetchval(
            "SELECT name FROM player WHERE name_lower = lower($1)", name.strip(),
        )


async def player_rows_window(
    name: str, window_start: int, window_end: int | None = None,
) -> list[dict]:
    """All of one player's rows in ``[window_start, window_end]`` (per-player chart).

    ``window_end`` matters for HISTORICAL windows. With only the lower bound, a
    7-day window anchored in the past still scans every partition from its start
    to NOW - for a name last seen weeks ago that is most of the archive, nearly
    all of it on the cold tier, and it blew the 120s command_timeout during the
    duplicates backfill. Bounding both ends lets partition pruning keep the scan
    to the ~7 partitions actually being asked about. ``None`` keeps the original
    "from here to now" behaviour that the live chart wants."""
    async with acquire() as con:
        # Resolve the player FIRST (one indexed hit on the unique name_lower), then
        # read entries by player_id. Filtering on ``p.name_lower`` across the join
        # left the planner walking ``entry`` and joining back, which on a
        # cold-tiered historical window ran tens of seconds per player. Querying
        # ``player_id`` directly uses the (player_id, anchor) index the table was
        # built for.
        who = await con.fetchrow(
            "SELECT id, name FROM player WHERE name_lower = lower($1)", name.strip())
        if who is None:
            return []
        sql = (
            "SELECT board_uuid AS leaderboard, anchor AS created_at, rank, score "
            "FROM entry WHERE player_id = $1 AND anchor >= $2"
        )
        args: list = [who["id"], window_start]
        if window_end is not None:
            args.append(window_end)
            sql += f" AND anchor <= ${len(args)}"
        rows = await con.fetch(sql, *args)
    return [
        {"player_name": who["name"], "rank": r["rank"], "score": r["score"],
         "leaderboard": r["leaderboard"], "created_at": r["created_at"]}
        for r in rows
    ]


async def player_last_played(
    name: str, *, excluded: set[int], window_start: int,
) -> int | None:
    """Most recent anchor at/after ``window_start`` where this player's score
    ROSE on some non-excluded board vs their previous appearance on that board -
    the "last played" signal (real activity, not mere presence on a lifetime
    board that carries a score forever). None when no rise is found in the window.

    Bounded to the window so the query prunes to recent partitions instead of
    scanning the player's entire cross-partition history on every profile load.

    The ``slot`` rank is what makes this correct for a DUPLICATED name (a name
    Trove's dump lists twice on one board - see ``duplicates``). Without it the
    lag alternates between the two co-existing rows, so ``score > prev`` fires on
    every single capture and the player reads as permanently active. Ranking the
    rows within each ``(board, anchor)`` by score and lagging per slot compares
    like with like. For the overwhelming majority - one row per board per anchor -
    slot is always 1 and the result is identical to a plain lag."""
    excl = sorted(excluded)
    async with acquire() as con:
        val = await con.fetchval(
            "WITH me AS ("
            "  SELECT e.board_uuid, e.anchor, e.score, "
            "         row_number() OVER (PARTITION BY e.board_uuid, e.anchor "
            "                            ORDER BY e.score DESC, e.rank ASC) AS slot "
            "  FROM entry e JOIN player p ON p.id = e.player_id "
            "  WHERE p.name_lower = lower($1) AND e.anchor >= $2 "
            "        AND NOT (e.board_uuid = ANY($3::int[])) "
            "), s AS ("
            "  SELECT anchor, score, "
            "         lag(score) OVER (PARTITION BY board_uuid, slot ORDER BY anchor) AS prev "
            "  FROM me "
            ") "
            "SELECT max(anchor) FROM s WHERE prev IS NOT NULL AND score > prev",
            name.strip().lower(), window_start, excl,
        )
    return int(val) if val is not None else None


async def player_board_summary(name: str) -> list[dict]:
    """Per-leaderboard aggregate of one player's ENTIRE appearance history: best
    rank ever, current rank/score (latest capture), capture count, and first/last
    seen - one row per board, best boards first.

    O(boards) read of the incrementally-maintained ``player_board_agg`` (folded
    per capture in write_snapshot). Previously this scanned the player's whole
    cross-partition history, which was 30-90s for prolific players. If the
    aggregate is empty (never rebuilt on an existing dataset), the caller should
    run ``rebuild_player_board_agg`` once to backfill it."""
    sql = (
        "SELECT a.board_uuid AS leaderboard, a.best_rank, a.appearances::int AS appearances,"
        "       a.first_seen, a.last_seen, a.latest_rank, a.latest_score "
        "FROM player_board_agg a JOIN player p ON p.id = a.player_id "
        "WHERE p.name_lower = lower($1) "
        "ORDER BY a.best_rank ASC, a.last_seen DESC"
    )
    async with acquire() as con:
        rows = await con.fetch(sql, name.strip())
    return [dict(r) for r in rows]


async def _fold_anchor_into_agg(con, anchor: int) -> None:
    """Fold one just-inserted capture into ``player_board_agg`` (called INSIDE
    write_snapshot's transaction, after the entry INSERT). One set-based upsert
    over this anchor's partition only - cheap.

    Idempotent for the common forward-ingest + exact-replay cases: ``appearances``
    increments only when the anchor is strictly newer than what's already folded
    (``last_folded_anchor``), so re-ingesting the same hour doesn't double-count.
    best/first/last/latest use LEAST/GREATEST/CASE so an out-of-order backfill
    still corrects best-rank / first-seen (only its appearance count is missed -
    trued up by rebuild_player_board_agg)."""
    await con.execute(
        "INSERT INTO player_board_agg AS a "
        "  (player_id, board_uuid, best_rank, appearances, first_seen, last_seen, "
        "   latest_rank, latest_score, last_folded_anchor) "
        # DISTINCT ON guarantees one row per (player, board) so ON CONFLICT never
        # tries to touch the same key twice in one statement (which errors). A
        # player is normally on a board once per capture; this just hardens
        # against a freak double-listing.
        "SELECT DISTINCT ON (e.player_id, e.board_uuid) "
        "       e.player_id, e.board_uuid, e.rank, 1, e.anchor, e.anchor, "
        "       e.rank, e.score, e.anchor "
        "FROM entry e WHERE e.anchor = $1 "
        "ORDER BY e.player_id, e.board_uuid, e.rank ASC "
        "ON CONFLICT (player_id, board_uuid) DO UPDATE SET "
        "  best_rank    = LEAST(a.best_rank, EXCLUDED.best_rank), "
        "  first_seen   = LEAST(a.first_seen, EXCLUDED.first_seen), "
        "  last_seen    = GREATEST(a.last_seen, EXCLUDED.last_seen), "
        "  appearances  = a.appearances + "
        "                 (CASE WHEN EXCLUDED.last_folded_anchor > a.last_folded_anchor "
        "                       THEN 1 ELSE 0 END), "
        "  latest_rank  = CASE WHEN EXCLUDED.last_seen >= a.last_seen "
        "                      THEN EXCLUDED.latest_rank ELSE a.latest_rank END, "
        "  latest_score = CASE WHEN EXCLUDED.last_seen >= a.last_seen "
        "                      THEN EXCLUDED.latest_score ELSE a.latest_score END, "
        "  last_folded_anchor = GREATEST(a.last_folded_anchor, EXCLUDED.last_folded_anchor)",
        anchor,
    )


async def rebuild_player_board_agg() -> int:
    """Full recompute of ``player_board_agg`` from the entry table (the expensive
    all-history scan, done ONCE). Use to seed the table on an existing dataset,
    or to true up appearance counts after out-of-order backfills. Returns the row
    count written. Runs in a transaction so readers never see a half-built table."""
    async with acquire() as con:
        async with con.transaction():
            await con.execute("TRUNCATE player_board_agg")
            await con.execute(
                "INSERT INTO player_board_agg "
                "  (player_id, board_uuid, best_rank, appearances, first_seen, "
                "   last_seen, latest_rank, latest_score, last_folded_anchor) "
                "WITH r AS ("
                "  SELECT player_id, board_uuid, rank, score, anchor,"
                "         ROW_NUMBER() OVER (PARTITION BY player_id, board_uuid "
                "                            ORDER BY anchor DESC) AS rn"
                "  FROM entry"
                ") "
                "SELECT player_id, board_uuid, MIN(rank)::int, COUNT(*)::bigint,"
                "       MIN(anchor)::bigint, MAX(anchor)::bigint,"
                "       (MAX(rank)  FILTER (WHERE rn = 1))::int,"
                "       (MAX(score) FILTER (WHERE rn = 1)),"
                "       MAX(anchor)::bigint "
                "FROM r GROUP BY player_id, board_uuid"
            )
            count = await con.fetchval("SELECT count(*) FROM player_board_agg")
    return int(count or 0)


async def board_top_series(
    uuid: int, window_start: int, top: int,
) -> tuple[list[int], int | None, list[dict], list[dict]]:
    """For board_history: only the TOP-``top`` players' trajectories - NOT the whole
    board's window. Three cheap indexed queries instead of hauling every player's
    every row into Python:

      1. distinct anchors in the window (the chart x-axis);
      2. the latest anchor's top-``top`` rows ordered by rank (the lines to plot);
      3. just those players' rows across the window (``player_id = ANY`` - highly
         selective, served by the ``(player_id, anchor)`` index).

    Returns ``(anchors_asc, latest, top_meta, series_rows)`` where ``top_meta`` is
    ``[{player_id, player_name, rank}]`` (rank order) and ``series_rows`` is
    ``[{player_id, created_at, rank, score}]`` for those players only."""
    async with acquire() as con:
        anchor_rows = await con.fetch(
            "SELECT DISTINCT anchor FROM entry WHERE board_uuid = $1 AND anchor >= $2 "
            "ORDER BY anchor", uuid, window_start,
        )
        anchors = [r["anchor"] for r in anchor_rows]
        if not anchors:
            return [], None, [], []
        latest = anchors[-1]
        top_rows = await con.fetch(
            "SELECT e.player_id, p.name AS player_name, e.rank "
            "FROM entry e JOIN player p ON p.id = e.player_id "
            "WHERE e.board_uuid = $1 AND e.anchor = $2 "
            "ORDER BY e.rank LIMIT $3",
            uuid, latest, top,
        )
        if not top_rows:
            return anchors, latest, [], []
        player_ids = [r["player_id"] for r in top_rows]
        series_rows = await con.fetch(
            "SELECT e.player_id, e.anchor AS created_at, e.rank, e.score "
            "FROM entry e "
            "WHERE e.board_uuid = $1 AND e.anchor >= $2 AND e.player_id = ANY($3)",
            uuid, window_start, player_ids,
        )
    top_meta = [
        {"player_id": r["player_id"], "player_name": r["player_name"], "rank": r["rank"]}
        for r in top_rows
    ]
    series = [
        {"player_id": r["player_id"], "created_at": r["created_at"],
         "rank": r["rank"], "score": r["score"]}
        for r in series_rows
    ]
    return anchors, latest, top_meta, series


async def board_reset_kind(uuid: int) -> str:
    """Effective reset cadence for one board (override else hardcoded)."""
    async with acquire() as con:
        ov = await con.fetchval("SELECT reset_kind_override FROM board WHERE uuid = $1", uuid)
    return _effective_reset_kind(ov, uuid)


async def admin_list_boards() -> list[dict]:
    """Every board's full metadata + raw override (the admin reset-cadence table)."""
    async with acquire() as con:
        rows = await con.fetch(
            "SELECT uuid, name, name_id, category, category_id, reset_kind_override FROM board"
        )
    return [dict(r) for r in rows]


async def set_reset_kind_override(uuid: int, value: str | None) -> dict | None:
    """Pin/clear a board's reset-cadence override (admin). Returns the updated row
    or None if no such board."""
    async with acquire() as con:
        row = await con.fetchrow(
            "UPDATE board SET reset_kind_override = $2 WHERE uuid = $1 "
            "RETURNING uuid, name, name_id, category, category_id, reset_kind_override",
            uuid, value,
        )
    return dict(row) if row else None


async def all_boards() -> list[dict]:
    """Every board's metadata + effective reset_kind (for the activity backfill)."""
    async with acquire() as con:
        rows = await con.fetch(
            "SELECT uuid, name, category, reset_kind_override FROM board"
        )
    return [
        {"uuid": r["uuid"], "name": r["name"], "category": r["category"],
         "reset_kind": _effective_reset_kind(r["reset_kind_override"], r["uuid"])}
        for r in rows
    ]


async def board_meta(uuids: list[int]) -> dict[int, dict]:
    """``{uuid: {name, category, reset_kind}}`` for a set of boards - for the
    per-player / per-board history charts (name + cadence for the reset-zero
    injection) and the profile page's category grouping."""
    if not uuids:
        return {}
    async with acquire() as con:
        rows = await con.fetch(
            "SELECT uuid, name, category, reset_kind_override FROM board WHERE uuid = ANY($1)",
            uuids,
        )
    return {
        r["uuid"]: {
            "name": r["name"],
            "category": r["category"],
            "reset_kind": _effective_reset_kind(r["reset_kind_override"], r["uuid"]),
        }
        for r in rows
    }


async def board_kinds(uuids: list[int]) -> dict[int, str]:
    """``{uuid: reset_kind}`` for a set of boards - the flat cadence map used by
    the player-history delta comparability check (no name needed)."""
    if not uuids:
        return {}
    async with acquire() as con:
        rows = await con.fetch(
            "SELECT uuid, reset_kind_override FROM board WHERE uuid = ANY($1)", uuids,
        )
    return {r["uuid"]: _effective_reset_kind(r["reset_kind_override"], r["uuid"]) for r in rows}


# ── activity estimates ────────────────────────────────────────────────────────

async def upsert_estimate(window_end: int, window_start: int, duration_hours: float,
                          estimate: int, boards_analyzed: int, computed_at: int) -> None:
    async with acquire() as con:
        await con.execute(
            "INSERT INTO activity_estimate "
            "(window_end, window_start, duration_hours, estimate, boards_analyzed, computed_at) "
            "VALUES ($1, $2, $3, $4, $5, $6) "
            "ON CONFLICT (window_end) DO UPDATE SET "
            "window_start = EXCLUDED.window_start, duration_hours = EXCLUDED.duration_hours, "
            "estimate = EXCLUDED.estimate, boards_analyzed = EXCLUDED.boards_analyzed, "
            "computed_at = EXCLUDED.computed_at",
            window_end, window_start, duration_hours, estimate, boards_analyzed, computed_at,
        )


async def delete_estimate(window_end: int) -> None:
    async with acquire() as con:
        await con.execute("DELETE FROM activity_estimate WHERE window_end = $1", window_end)


async def get_estimates(window_start: int | None = None) -> list[dict]:
    async with acquire() as con:
        if window_start is None:
            rows = await con.fetch(
                "SELECT window_end, window_start, duration_hours, estimate, boards_analyzed, "
                "computed_at FROM activity_estimate ORDER BY window_end"
            )
        else:
            rows = await con.fetch(
                "SELECT window_end, window_start, duration_hours, estimate, boards_analyzed, "
                "computed_at FROM activity_estimate WHERE window_end >= $1 ORDER BY window_end",
                window_start,
            )
    return [dict(r) for r in rows]


async def delete_all_estimates() -> int:
    async with acquire() as con:
        res = await con.execute("DELETE FROM activity_estimate")
    return int(res.split()[-1]) if res.startswith("DELETE") else 0


# ── per-class activity estimates ───────────────────────────────────────────────

async def upsert_class_estimates(rows: list[dict]) -> None:
    """Batch-upsert one window's per-class rows (≤ one per class). Each row:
    {class_index, window_end, window_start, duration_hours, estimate,
    estimate_clean (int|None), computed_at}. ``estimate_clean`` is the
    Power-Rank-filtered count (NULL when that view is unmeasurable)."""
    if not rows:
        return
    async with acquire() as con:
        await con.executemany(
            "INSERT INTO class_activity_estimate "
            "(class_index, window_end, window_start, duration_hours, estimate, "
            " estimate_clean, computed_at) "
            "VALUES ($1, $2, $3, $4, $5, $6, $7) "
            "ON CONFLICT (class_index, window_end) DO UPDATE SET "
            "window_start = EXCLUDED.window_start, duration_hours = EXCLUDED.duration_hours, "
            "estimate = EXCLUDED.estimate, estimate_clean = EXCLUDED.estimate_clean, "
            "computed_at = EXCLUDED.computed_at",
            [(r["class_index"], r["window_end"], r["window_start"],
              r["duration_hours"], r["estimate"], r.get("estimate_clean"),
              r["computed_at"]) for r in rows],
        )


async def get_class_estimates(window_start: int | None = None) -> list[dict]:
    cols = ("class_index, window_end, window_start, duration_hours, estimate, "
            "estimate_clean, computed_at")
    async with acquire() as con:
        if window_start is None:
            rows = await con.fetch(
                f"SELECT {cols} FROM class_activity_estimate ORDER BY window_end, class_index"
            )
        else:
            rows = await con.fetch(
                f"SELECT {cols} FROM class_activity_estimate WHERE window_end >= $1 "
                "ORDER BY window_end, class_index",
                window_start,
            )
    return [dict(r) for r in rows]


async def delete_class_estimate(window_end: int) -> None:
    async with acquire() as con:
        await con.execute("DELETE FROM class_activity_estimate WHERE window_end = $1", window_end)


async def delete_all_class_estimates() -> int:
    async with acquire() as con:
        res = await con.execute("DELETE FROM class_activity_estimate")
    return int(res.split()[-1]) if res.startswith("DELETE") else 0


# ── player renames ──────────────────────────────────────────────────────────────

async def all_anchors_asc(*, since: int | None = None) -> list[int]:
    """Every distinct capture anchor, OLDEST first - the backfills walk over this.

    Uses the SAME loose index scan (skip scan) as ``list_timestamps``, for the same
    reason: a plain ``SELECT DISTINCT anchor FROM entry`` HashAggregates the whole
    partitioned table (hundreds of millions of rows, most of them on the cold-tier
    disk) and blows straight past the 120s ``command_timeout``. That is exactly how
    this failed - the rename and duplicate backfills both died on their FIRST query,
    before their single-flight lock was even released. The skip scan seeks the max
    anchor, then repeatedly the next-lower one, so it costs one index seek per
    capture instead of a full scan.

    ``since`` (unix seconds) floors the walk and lets partition pruning skip every
    older partition outright - pass it for a windowed rebuild."""
    sql = (
        """
        WITH RECURSIVE a AS (
            SELECT (SELECT max(anchor) FROM entry{floor1}) AS anchor
            UNION ALL
            SELECT (SELECT max(e.anchor) FROM entry e WHERE e.anchor < a.anchor{floor2})
            FROM a WHERE a.anchor IS NOT NULL
        )
        SELECT anchor FROM a WHERE anchor IS NOT NULL ORDER BY anchor ASC
        """
    )
    async with acquire() as con:
        if since is None:
            rows = await con.fetch(sql.format(floor1="", floor2=""))
        else:
            rows = await con.fetch(
                sql.format(floor1=" WHERE anchor >= $1",
                           floor2=" AND e.anchor >= $1"),
                since,
            )
    return [r["anchor"] for r in rows]


async def upsert_renames(rows: list[dict]) -> None:
    """Idempotent batch-insert of detected renames. The ``(from,to,to_anchor)``
    unique key means re-running the same capture pair (live warm re-fires, or the
    backfill re-walks) refreshes the row in place instead of duplicating. Each row:
    ``{from_name, to_name, from_anchor, to_anchor, confidence, matched_boards,
    evidence(dict), method_version, created_at}``."""
    if not rows:
        return
    payload = [
        (
            r["from_name"], r["from_name"].strip().lower(),
            r["to_name"], r["to_name"].strip().lower(),
            int(r["from_anchor"]), int(r["to_anchor"]),
            float(r["confidence"]), int(r["matched_boards"]),
            json.dumps(r.get("evidence") or {}),
            int(r.get("method_version", 1)), int(r["created_at"]),
        )
        for r in rows
    ]
    async with acquire() as con:
        await con.executemany(
            "INSERT INTO player_rename (from_name, from_name_lower, to_name, "
            "to_name_lower, from_anchor, to_anchor, confidence, matched_boards, "
            "evidence, method_version, created_at) "
            "VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9::jsonb, $10, $11) "
            "ON CONFLICT (from_name_lower, to_name_lower, to_anchor) DO UPDATE SET "
            "from_name = EXCLUDED.from_name, to_name = EXCLUDED.to_name, "
            "from_anchor = EXCLUDED.from_anchor, confidence = EXCLUDED.confidence, "
            "matched_boards = EXCLUDED.matched_boards, evidence = EXCLUDED.evidence, "
            "method_version = EXCLUDED.method_version, created_at = EXCLUDED.created_at",
            payload,
        )


def _rename_row(r) -> dict:
    ev = r["evidence"]
    if isinstance(ev, str):
        try:
            ev = json.loads(ev)
        except (ValueError, TypeError):
            ev = {}
    return {
        "id": r["id"],
        "from_name": r["from_name"], "to_name": r["to_name"],
        "from_anchor": r["from_anchor"], "to_anchor": r["to_anchor"],
        "confidence": r["confidence"], "matched_boards": r["matched_boards"],
        "evidence": ev or {}, "method_version": r["method_version"],
        "created_at": r["created_at"],
    }


_RENAME_COLS = (
    "id, from_name, to_name, from_anchor, to_anchor, confidence, matched_boards, "
    "evidence, method_version, created_at"
)


async def list_renames(*, limit: int, offset: int) -> tuple[list[dict], int]:
    """Detected renames, MOST-RECENT-first (by the capture the new name appeared
    in), with the total count for pagination."""
    async with acquire() as con:
        total = await con.fetchval("SELECT count(*) FROM player_rename")
        rows = await con.fetch(
            f"SELECT {_RENAME_COLS} FROM player_rename "
            "ORDER BY to_anchor DESC, confidence DESC, id DESC LIMIT $1 OFFSET $2",
            limit, offset,
        )
    return [_rename_row(r) for r in rows], int(total or 0)


async def renames_for_name(name: str) -> list[dict]:
    """Every rename edge touching ``name`` (as either the old OR the new name),
    newest first - the raw edges the caller chains into a full history."""
    lowered = name.strip().lower()
    async with acquire() as con:
        rows = await con.fetch(
            f"SELECT {_RENAME_COLS} FROM player_rename "
            "WHERE from_name_lower = $1 OR to_name_lower = $1 "
            "ORDER BY to_anchor DESC, id DESC",
            lowered,
        )
    return [_rename_row(r) for r in rows]


async def count_renames() -> int:
    async with acquire() as con:
        return int(await con.fetchval("SELECT count(*) FROM player_rename") or 0)


async def latest_rename_to_anchor() -> int | None:
    """The newest ``to_anchor`` we've already recorded a rename for - lets the live
    pass skip re-scanning a pair it has already processed."""
    async with acquire() as con:
        return await con.fetchval("SELECT MAX(to_anchor) FROM player_rename")


async def delete_all_renames() -> int:
    async with acquire() as con:
        res = await con.execute("DELETE FROM player_rename")
    return int(res.split()[-1]) if res.startswith("DELETE") else 0


# ── duplicate names ──────────────────────────────────────────────────────────

async def _record_case_collisions(con, anchor: int) -> None:
    """Persist names whose RAW spellings in this dump differ only by case.

    Must run inside ``write_snapshot``'s transaction, against the ``_stg`` temp
    table - the ``player`` upsert keys on ``lower(name)``, so once the entries are
    resolved the fact that two spellings were merged is gone for good.

    Never clobbers a ``same_name`` record for the same name: a name can be hit by
    both problems at once, and that combination is recorded as ``kind='both'``."""
    await con.execute(
        "INSERT INTO player_duplicate (name_lower, name, kind, verdict, boards, "
        "  max_occurrences, spellings, first_anchor, last_anchor, evidence, "
        "  method_version, updated_at) "
        "SELECT lower(s.name), max(s.name), 'case', 'case_only', "
        "       count(DISTINCT s.board_uuid)::int, count(DISTINCT s.name)::int, "
        "       to_jsonb(array_agg(DISTINCT s.name)), $1, $1, '{}'::jsonb, 1, $1 "
        "FROM _stg s GROUP BY lower(s.name) HAVING count(DISTINCT s.name) > 1 "
        "ON CONFLICT (name_lower) DO UPDATE SET "
        "  name = EXCLUDED.name, "
        "  kind = CASE WHEN player_duplicate.kind IN ('same_name', 'both') "
        "              THEN 'both' ELSE 'case' END, "
        "  verdict = CASE WHEN player_duplicate.kind IN ('same_name', 'both') "
        "                 THEN player_duplicate.verdict ELSE EXCLUDED.verdict END, "
        "  boards = CASE WHEN player_duplicate.kind IN ('same_name', 'both') "
        "                THEN player_duplicate.boards ELSE EXCLUDED.boards END, "
        "  max_occurrences = GREATEST(player_duplicate.max_occurrences, "
        "                             EXCLUDED.max_occurrences), "
        "  spellings = EXCLUDED.spellings, "
        "  first_anchor = LEAST(player_duplicate.first_anchor, EXCLUDED.first_anchor), "
        "  last_anchor = GREATEST(player_duplicate.last_anchor, EXCLUDED.last_anchor), "
        "  updated_at = EXCLUDED.updated_at",
        anchor,
    )


async def duplicate_groups_at(anchor: int) -> list[dict]:
    """Every ``(name, board)`` that carries MORE THAN ONE entry row at ``anchor``.

    One row per duplicated pair: ``{name, board_uuid, occurrences}``.

    Counted on DISTINCT ``(rank, score)``, not raw rows. A name listed twice at
    the same rank AND the same score is one row emitted twice by the capture, not
    two identities - two real entities tied on score still occupy different ranks.
    Treating an identical repeat as a second player asserts something the data
    does not support, and it is not rare: 14 of 120 duplicated pairs in a live
    capture were identical repeats, almost all of them on the club boards.

    Grouped on ``player_id``, with the handful of names resolved in a second
    lookup, rather than joining ``player`` inside the aggregate. An anchor holds
    ~700k entry rows and yields ~100 duplicated pairs, so joining to get a name
    per row does ~700k lookups to label ~100 results. Measured on live data, the
    join costs 1.9x on a hot partition and 2.8x on a cold one (0.48s -> 0.25s,
    0.72s -> 0.26s); across the whole-archive backfill that is the difference
    between ~16 and ~5 minutes.

    Batching several anchors into one grouped scan was also measured and is
    WORSE (0.80s/anchor for a whole day-partition vs 0.26s one at a time), so the
    per-anchor loop stays. What remains is inherent: finding a name listed twice
    on a board means grouping every row of every capture, ~692M rows."""
    async with acquire() as con:
        rows = await con.fetch(
            "SELECT player_id, board_uuid, "
            "       count(DISTINCT (rank, score))::int AS occurrences "
            "FROM entry WHERE anchor = $1 "
            "GROUP BY player_id, board_uuid "
            "HAVING count(DISTINCT (rank, score)) > 1 "
            "ORDER BY player_id, board_uuid",
            anchor,
        )
        if not rows:
            return []
        ids = sorted({r["player_id"] for r in rows})
        names = {
            n["id"]: n["name"] for n in await con.fetch(
                "SELECT id, name FROM player WHERE id = ANY($1::bigint[])", ids)
        }
    return [
        {"name": names.get(r["player_id"], str(r["player_id"])),
         "board_uuid": r["board_uuid"], "occurrences": r["occurrences"]}
        for r in rows
    ]


async def upsert_duplicates(rows: list[dict]) -> None:
    """Idempotent batch-upsert of detected duplicate-name records (one per name).

    ``first_anchor`` folds to the EARLIEST ever seen and ``last_anchor`` to the
    latest, so re-running the live pass or re-walking the archive tightens the
    dating instead of resetting it. A name already flagged ``case`` is promoted to
    ``both`` rather than overwritten."""
    if not rows:
        return
    payload = [
        (
            r["name"].strip().lower(), r["name"], r.get("kind", "same_name"),
            r.get("verdict", "all_idle"), int(r.get("boards", 0)),
            int(r.get("max_occurrences", 2)),
            json.dumps(r.get("spellings") or []),
            int(r["first_anchor"]), int(r["last_anchor"]),
            json.dumps(r.get("evidence") or {}),
            int(r.get("method_version", 1)), int(r["updated_at"]),
        )
        for r in rows
    ]
    async with acquire() as con:
        await con.executemany(
            "INSERT INTO player_duplicate (name_lower, name, kind, verdict, boards, "
            "  max_occurrences, spellings, first_anchor, last_anchor, evidence, "
            "  method_version, updated_at) "
            "VALUES ($1, $2, $3, $4, $5, $6, $7::jsonb, $8, $9, $10::jsonb, $11, $12) "
            "ON CONFLICT (name_lower) DO UPDATE SET "
            "  name = EXCLUDED.name, "
            "  kind = CASE WHEN player_duplicate.kind IN ('case', 'both') "
            "              THEN 'both' ELSE EXCLUDED.kind END, "
            "  verdict = EXCLUDED.verdict, boards = EXCLUDED.boards, "
            "  max_occurrences = EXCLUDED.max_occurrences, "
            "  first_anchor = LEAST(player_duplicate.first_anchor, EXCLUDED.first_anchor), "
            "  last_anchor = GREATEST(player_duplicate.last_anchor, EXCLUDED.last_anchor), "
            "  evidence = EXCLUDED.evidence, "
            "  method_version = EXCLUDED.method_version, "
            "  updated_at = EXCLUDED.updated_at",
            payload,
        )


_DUP_COLS = (
    "name, kind, verdict, boards, max_occurrences, spellings, first_anchor, "
    "last_anchor, evidence, method_version, updated_at"
)


def _duplicate_row(r) -> dict:
    def _json(val, fallback):
        if isinstance(val, str):
            try:
                return json.loads(val)
            except (ValueError, TypeError):
                return fallback
        return val if val is not None else fallback

    return {
        "name": r["name"], "kind": r["kind"], "verdict": r["verdict"],
        "boards": r["boards"], "max_occurrences": r["max_occurrences"],
        "spellings": _json(r["spellings"], []),
        "first_anchor": r["first_anchor"], "last_anchor": r["last_anchor"],
        "evidence": _json(r["evidence"], {}) or {},
        "method_version": r["method_version"], "updated_at": r["updated_at"],
    }


async def list_duplicates(
    *, limit: int, offset: int, kind: str | None = None,
) -> tuple[list[dict], int]:
    """Recorded duplicate-name groups: still-current first (newest ``last_anchor``),
    then widest blast radius, with the total for pagination."""
    where = ""
    args: list = []
    if kind:
        args.append(kind)
        # 'both' rows satisfy either filter - they carry both problems.
        where = f" WHERE (kind = ${len(args)} OR kind = 'both')"
    async with acquire() as con:
        total = await con.fetchval(
            f"SELECT count(*) FROM player_duplicate{where}", *args)
        rows = await con.fetch(
            f"SELECT {_DUP_COLS} FROM player_duplicate{where} "
            "ORDER BY last_anchor DESC, boards DESC, name_lower ASC "
            f"LIMIT ${len(args) + 1} OFFSET ${len(args) + 2}",
            *args, limit, offset,
        )
    return [_duplicate_row(r) for r in rows], int(total or 0)


async def duplicate_for_name(name: str) -> dict | None:
    """The duplicate record for one name, or None when the name is clean."""
    async with acquire() as con:
        row = await con.fetchrow(
            f"SELECT {_DUP_COLS} FROM player_duplicate WHERE name_lower = lower($1)",
            name.strip(),
        )
    return _duplicate_row(row) if row is not None else None


async def duplicate_names_lower() -> set[str]:
    """Just the flagged ``name_lower`` set - a cheap membership test for read
    paths that only need "is this player ambiguous?" without the evidence."""
    async with acquire() as con:
        rows = await con.fetch("SELECT name_lower FROM player_duplicate")
    return {r["name_lower"] for r in rows}


async def count_duplicates() -> int:
    async with acquire() as con:
        return int(await con.fetchval("SELECT count(*) FROM player_duplicate") or 0)


async def latest_duplicate_anchor() -> int | None:
    """Newest capture any duplication was recorded in - lets the tab mark which
    groups are still current versus historical."""
    async with acquire() as con:
        return await con.fetchval("SELECT MAX(last_anchor) FROM player_duplicate")


async def delete_all_duplicates() -> int:
    async with acquire() as con:
        res = await con.execute("DELETE FROM player_duplicate")
    return int(res.split()[-1]) if res.startswith("DELETE") else 0


# ── cold-tier partition management ────────────────────────────────────────────
# Aged trove-day partitions are moved off the fast NVMe data dir onto a slower
# ``cold`` tablespace (a redundant RAID1 disk) once they pass the hot window. New
# partitions are always created on pg_default (hot) - see pg_schema.ensure_partition -
# so NOTHING here ever runs on the write path; only aged partitions are relocated.
# Each partition's table + every index is moved with its OWN autocommit ALTER, so
# at most ONE partition holds an ACCESS EXCLUSIVE lock at a time and a long run
# never blocks the hot partitions. ``SET TABLESPACE`` rewrites into the new
# location and frees the old files immediately (no VACUUM needed to reclaim NVMe).
# The ``cold`` tablespace is an ops-provisioned resource (a bind-mounted host dir +
# ``CREATE TABLESPACE cold LOCATION '/cold'``); when it's absent every function
# here is a safe no-op so the app still runs on a plain single-disk deploy.

COLD_TABLESPACE = "cold"


async def cold_tablespace_exists() -> bool:
    async with acquire() as con:
        return bool(await con.fetchval(
            "SELECT 1 FROM pg_tablespace WHERE spcname = $1", COLD_TABLESPACE
        ))


def _partition_lo(relname: str) -> int | None:
    """``entry_p_<lo>`` -> ``<lo>`` (the trove-day-start unix seconds the partition
    covers), or None if the name doesn't match. Also validates the name is a pure
    ``entry_p_<int>`` so it's safe to interpolate into DDL (identifiers can't be
    parameterized)."""
    prefix = "entry_p_"
    if not relname.startswith(prefix):
        return None
    try:
        return int(relname[len(prefix):])
    except ValueError:
        return None


async def _entry_partitions(con) -> list[dict]:
    """Every ``entry`` partition with its lo bound, current tablespace name
    ('pg_default' when ``reltablespace`` = 0), and total size (table + indexes)."""
    rows = await con.fetch(
        "SELECT c.oid, c.relname, "
        "       COALESCE(t.spcname, 'pg_default') AS tablespace, "
        "       pg_total_relation_size(c.oid) AS bytes "
        "FROM pg_inherits i "
        "JOIN pg_class c ON c.oid = i.inhrelid "
        "LEFT JOIN pg_tablespace t ON t.oid = c.reltablespace "
        "WHERE i.inhparent = 'entry'::regclass"
    )
    out: list[dict] = []
    for r in rows:
        lo = _partition_lo(r["relname"])
        if lo is None:
            continue
        out.append({"oid": r["oid"], "name": r["relname"], "lo": lo,
                    "tablespace": r["tablespace"], "bytes": int(r["bytes"])})
    return out


def _cold_keep_from(now: int, after_days: int) -> int:
    """Oldest trove-day-start that stays HOT: keeps ``after_days`` trove-days
    INCLUDING today's. Partitions whose lo is below this move to cold."""
    return trove_day_start(now) - (max(1, after_days) - 1) * 86400


async def tier_status(after_days: int, now: int) -> dict:
    """Hot vs cold partition counts + bytes per tablespace, and how many
    partitions are eligible to move right now (aged, still on pg_default)."""
    if not await cold_tablespace_exists():
        return {"cold_tablespace": False}
    keep_from = _cold_keep_from(now, after_days)
    async with acquire() as con:
        parts = await _entry_partitions(con)
    hot = [p for p in parts if p["tablespace"] == "pg_default"]
    cold = [p for p in parts if p["tablespace"] == COLD_TABLESPACE]
    eligible = [p for p in hot if p["lo"] < keep_from]
    return {
        "cold_tablespace": True,
        "after_days": after_days,
        "keep_from": keep_from,
        "partitions": len(parts),
        "hot_partitions": len(hot),
        "cold_partitions": len(cold),
        "hot_bytes": sum(p["bytes"] for p in hot),
        "cold_bytes": sum(p["bytes"] for p in cold),
        "eligible_partitions": len(eligible),
        "eligible_bytes": sum(p["bytes"] for p in eligible),
    }


async def tier_cold_partitions(after_days: int, now: int,
                               limit: int | None = None) -> dict:
    """Move every ``entry`` partition that has aged past ``after_days`` and is
    still on pg_default onto the ``cold`` tablespace (table + all its indexes),
    oldest first. ``limit`` caps how many partitions move in one call (the warmer
    drips a few per pass; the admin trigger passes None to drain the whole
    backlog). No-op when the cold tablespace is absent. Returns the moved
    partition names + bytes relocated."""
    if not await cold_tablespace_exists():
        return {"cold_tablespace": False, "moved": [], "moved_bytes": 0}
    keep_from = _cold_keep_from(now, after_days)
    async with acquire() as con:
        parts = await _entry_partitions(con)
        eligible = sorted(
            (p for p in parts
             if p["tablespace"] == "pg_default" and p["lo"] < keep_from),
            key=lambda p: p["lo"],   # oldest first
        )
        if limit is not None:
            eligible = eligible[:limit]
        moved: list[str] = []
        moved_bytes = 0
        for p in eligible:
            # Partition index names are auto-generated; resolve them per partition.
            # ``regclass::text`` quotes any identifier that needs it.
            idx = await con.fetch(
                "SELECT indexrelid::regclass::text AS name "
                "FROM pg_index WHERE indrelid = $1", p["oid"],
            )
            # Table first, then each index - each its own autocommit statement so
            # only one object is locked at a time (asyncpg autocommits outside an
            # explicit transaction block). ``p["name"]`` is validated pure
            # ``entry_p_<int>`` by _partition_lo, so the interpolation is safe.
            await con.execute(
                f'ALTER TABLE {p["name"]} SET TABLESPACE {COLD_TABLESPACE}'
            )
            for r in idx:
                await con.execute(
                    f'ALTER INDEX {r["name"]} SET TABLESPACE {COLD_TABLESPACE}'
                )
            moved.append(p["name"])
            moved_bytes += p["bytes"]
    return {"cold_tablespace": True, "moved": moved, "moved_bytes": moved_bytes}

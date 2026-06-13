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

from app.core.postgres import acquire
from app.trove.leaderboards import pg_schema
from app.trove.leaderboards.models import (
    RESET_KIND_VALUES, is_player_board, reset_kind,
)
from app.trove.leaderboards.parser import ParsedBoard

_RESET_HOUR_UTC = 11


def _effective_reset_kind(override: str | None, uuid: int) -> str:
    """Admin override (if a valid value) else the hardcoded mapping - the PG
    equivalent of ``models.effective_reset_kind`` for a plain override string."""
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
                "TRUNCATE entry, player, activity_estimate, class_activity_estimate "
                "RESTART IDENTITY"
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

async def list_timestamps(limit: int = 60) -> list[int]:
    async with acquire() as con:
        rows = await con.fetch(
            "SELECT DISTINCT anchor FROM entry ORDER BY anchor DESC LIMIT $1", limit
        )
    return [r["anchor"] for r in rows]


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


async def player_rows(name: str, *, limit: int, uuid: int | None = None) -> list[dict]:
    """Most recent appearances of one player (case-insensitive), newest first."""
    sql = (
        "SELECT e.board_uuid AS leaderboard, e.anchor AS created_at, e.rank, e.score, "
        "p.name AS player_name "
        "FROM entry e JOIN player p ON p.id = e.player_id WHERE p.name_lower = lower($1)"
    )
    args: list = [name]
    if uuid is not None:
        sql += " AND e.board_uuid = $2"
        args.append(uuid)
        sql += " ORDER BY e.anchor DESC LIMIT $3"
    else:
        sql += " ORDER BY e.anchor DESC LIMIT $2"
    args.append(limit)
    async with acquire() as con:
        rows = await con.fetch(sql, *args)
    return [
        {"player_name": r["player_name"], "rank": r["rank"], "score": r["score"],
         "leaderboard": r["leaderboard"], "created_at": r["created_at"]}
        for r in rows
    ]


async def player_rows_window(name: str, window_start: int) -> list[dict]:
    """All of one player's rows since ``window_start`` (for the per-player chart)."""
    async with acquire() as con:
        rows = await con.fetch(
            "SELECT e.board_uuid AS leaderboard, e.anchor AS created_at, e.rank, e.score, "
            "p.name AS player_name "
            "FROM entry e JOIN player p ON p.id = e.player_id "
            "WHERE p.name_lower = lower($1) AND e.anchor >= $2",
            name, window_start,
        )
    return [
        {"player_name": r["player_name"], "rank": r["rank"], "score": r["score"],
         "leaderboard": r["leaderboard"], "created_at": r["created_at"]}
        for r in rows
    ]


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
    """``{uuid: {name, reset_kind}}`` for a set of boards - for the per-player /
    per-board history charts (name + cadence for the reset-zero injection)."""
    if not uuids:
        return {}
    async with acquire() as con:
        rows = await con.fetch(
            "SELECT uuid, name, reset_kind_override FROM board WHERE uuid = ANY($1)", uuids,
        )
    return {
        r["uuid"]: {
            "name": r["name"],
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
    {class_index, window_end, window_start, duration_hours, estimate, computed_at}."""
    if not rows:
        return
    async with acquire() as con:
        await con.executemany(
            "INSERT INTO class_activity_estimate "
            "(class_index, window_end, window_start, duration_hours, estimate, computed_at) "
            "VALUES ($1, $2, $3, $4, $5, $6) "
            "ON CONFLICT (class_index, window_end) DO UPDATE SET "
            "window_start = EXCLUDED.window_start, duration_hours = EXCLUDED.duration_hours, "
            "estimate = EXCLUDED.estimate, computed_at = EXCLUDED.computed_at",
            [(r["class_index"], r["window_end"], r["window_start"],
              r["duration_hours"], r["estimate"], r["computed_at"]) for r in rows],
        )


async def get_class_estimates(window_start: int | None = None) -> list[dict]:
    cols = "class_index, window_end, window_start, duration_hours, estimate, computed_at"
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


async def latest_class_estimates() -> list[dict]:
    """All per-class rows for the most recent stored window (for /current)."""
    cols = "class_index, window_end, window_start, duration_hours, estimate, computed_at"
    async with acquire() as con:
        rows = await con.fetch(
            f"SELECT {cols} FROM class_activity_estimate "
            "WHERE window_end = (SELECT MAX(window_end) FROM class_activity_estimate) "
            "ORDER BY estimate DESC, class_index"
        )
    return [dict(r) for r in rows]


async def delete_class_estimate(window_end: int) -> None:
    async with acquire() as con:
        await con.execute("DELETE FROM class_activity_estimate WHERE window_end = $1", window_end)


async def delete_all_class_estimates() -> int:
    async with acquire() as con:
        res = await con.execute("DELETE FROM class_activity_estimate")
    return int(res.split()[-1]) if res.startswith("DELETE") else 0

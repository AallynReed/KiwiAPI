"""Postgres schema for the leaderboards domain + partition management.

Tables:
  board             - one row per leaderboard (dimension)
  board_contest     - (board, anchor) contest windows, normalized out of the old
                      in-document array
  player            - one row per distinct player (dimension); the name is stored
                      ONCE here and entries reference ``player.id`` (8 bytes) instead
                      of repeating a ~20-byte name 44M times
  entry             - the FACT table, ``PARTITION BY RANGE (anchor)``; one row per
                      (board, anchor, player). One partition per trove-day, so old
                      data is just an old partition you can DROP wholesale.
  activity_estimate - the per-window active-player series (derived, small)

Indexes live on the PARTITIONED PARENT, so every partition (existing + future)
inherits them automatically:
  entry (board_uuid, anchor, rank)  - top-N of a board at a time
  entry (player_id, anchor)         - a player's history across time

Created idempotently on startup (IF NOT EXISTS) - same no-migration-framework
approach as Beanie's index creation on the Mongo side. No FKs on the fact table:
the relations are enforced by the ingest logic, and FKs on a 44M-row partitioned
table cost more than they're worth.
"""
from datetime import UTC, datetime, timedelta

_RESET_HOUR_UTC = 11   # Trove day boundary (matches the Mongo _trove_day_start)

_SCHEMA = """
CREATE TABLE IF NOT EXISTS board (
    uuid                INTEGER PRIMARY KEY,
    name_id             TEXT NOT NULL,
    name                TEXT NOT NULL,
    category_id         TEXT NOT NULL,
    category            TEXT NOT NULL,
    reset_kind_override TEXT
);

CREATE TABLE IF NOT EXISTS board_contest (
    board_uuid INTEGER NOT NULL,
    anchor     BIGINT  NOT NULL,
    type       TEXT    NOT NULL,
    PRIMARY KEY (board_uuid, anchor)
);

CREATE TABLE IF NOT EXISTS player (
    id         BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    name       TEXT NOT NULL,
    name_lower TEXT NOT NULL UNIQUE
);

CREATE TABLE IF NOT EXISTS entry (
    board_uuid INTEGER NOT NULL,
    anchor     BIGINT  NOT NULL,
    rank       INTEGER NOT NULL,
    score      DOUBLE PRECISION NOT NULL,
    player_id  BIGINT  NOT NULL
) PARTITION BY RANGE (anchor);

CREATE INDEX IF NOT EXISTS entry_brc ON entry (board_uuid, anchor, rank);
CREATE INDEX IF NOT EXISTS entry_pa  ON entry (player_id, anchor);
CREATE INDEX IF NOT EXISTS entry_a   ON entry (anchor);

CREATE TABLE IF NOT EXISTS activity_estimate (
    window_end      BIGINT PRIMARY KEY,
    window_start    BIGINT NOT NULL,
    duration_hours  DOUBLE PRECISION NOT NULL,
    estimate        INTEGER NOT NULL,
    boards_analyzed INTEGER NOT NULL,
    computed_at     BIGINT NOT NULL
);

CREATE TABLE IF NOT EXISTS class_activity_estimate (
    class_index    INTEGER NOT NULL,
    window_end     BIGINT  NOT NULL,
    window_start   BIGINT  NOT NULL,
    duration_hours DOUBLE PRECISION NOT NULL,
    estimate       INTEGER NOT NULL,
    estimate_clean INTEGER,
    computed_at    BIGINT  NOT NULL,
    PRIMARY KEY (class_index, window_end)
);
CREATE INDEX IF NOT EXISTS class_activity_we ON class_activity_estimate (window_end);
-- ``estimate_clean`` (the Power-Rank-filtered "clean" view) was added after the
-- table shipped; ADD it idempotently so an existing deploy gains the column.
-- NULL = clean unmeasurable for that window (no Power Rank board snapshot).
ALTER TABLE class_activity_estimate ADD COLUMN IF NOT EXISTS estimate_clean INTEGER;
"""


async def init(con) -> None:
    """Create the tables + parent indexes (idempotent)."""
    await con.execute(_SCHEMA)


def trove_day_bounds(anchor: int) -> tuple[int, int]:
    """``[day_start, day_start + 1d)`` in unix seconds for the trove-day (11:00 UTC
    boundary) containing ``anchor`` - the partition range for that anchor."""
    d = datetime.fromtimestamp(anchor, UTC)
    start = d.replace(hour=_RESET_HOUR_UTC, minute=0, second=0, microsecond=0)
    if d < start:
        start -= timedelta(days=1)
    lo = int(start.timestamp())
    return lo, lo + 86400


async def ensure_partition(con, anchor: int) -> None:
    """Create the day-partition for ``anchor`` if missing. Cheap + idempotent;
    the parent's partitioned indexes propagate to the new partition automatically.
    Called before each ingest."""
    lo, hi = trove_day_bounds(anchor)
    await con.execute(
        f"CREATE TABLE IF NOT EXISTS entry_p_{lo} PARTITION OF entry "
        f"FOR VALUES FROM ({lo}) TO ({hi})"
    )

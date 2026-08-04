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

-- Per-window distinct ACTIVE players (the names that strictly increased a score
-- this capture, scores above the cap excluded). The 24h / 7d "active players"
-- rollups are COUNT(DISTINCT player_lower) over the windows in range - the true
-- distinct union, materialized incrementally (one small write per capture) so the
-- rollups are an indexed count, never a re-scan of days of entries. Rolling ~8-day
-- retention (pruned by the warmer); fully derived from the captures.
CREATE TABLE IF NOT EXISTS activity_active (
    window_end   BIGINT NOT NULL,
    player_lower TEXT   NOT NULL,
    PRIMARY KEY (window_end, player_lower)
);

-- Per-(player, board) lifetime aggregate: best rank ever, appearance count,
-- first/last-seen anchors, and the latest capture's rank/score. Maintained
-- INCREMENTALLY at ingest (one small set-based upsert per capture - see
-- pg_store._fold_anchor_into_agg), same pattern as activity_active. Lets the
-- /player profile read a player's per-board standings in O(boards) instead of
-- scanning their entire cross-partition history (which was 30-90s for prolific
-- players). ``last_folded_anchor`` guards the appearance count against
-- double-count on an idempotent re-ingest of the same anchor; a full trueup
-- after out-of-order backfills is pg_store.rebuild_player_board_agg().
CREATE TABLE IF NOT EXISTS player_board_agg (
    player_id          BIGINT  NOT NULL,
    board_uuid         INTEGER NOT NULL,
    best_rank          INTEGER NOT NULL,
    appearances        BIGINT  NOT NULL,
    first_seen         BIGINT  NOT NULL,
    last_seen          BIGINT  NOT NULL,
    latest_rank        INTEGER NOT NULL,
    latest_score       DOUBLE PRECISION NOT NULL,
    last_folded_anchor BIGINT  NOT NULL,
    PRIMARY KEY (player_id, board_uuid)
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
-- Detected player renames. Trove leaderboards carry no stable UID (a rename
-- mints a brand-new ``player`` row), so a rename is RECONSTRUCTED: a name that
-- vanished between two adjacent captures while a new name appeared with the same
-- lifetime-board score fingerprint. One row per detected transition; the
-- ``(from,to,to_anchor)`` unique key makes re-runs (live warm + backfill)
-- idempotent. Chaining ``to_name_lower -> from_name_lower`` reconstructs a full
-- rename history per identity. ``evidence`` holds the matched boards + scores +
-- confidence sub-terms (same transparency contract as cheater detection).
CREATE TABLE IF NOT EXISTS player_rename (
    id              BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    from_name       TEXT   NOT NULL,
    from_name_lower TEXT   NOT NULL,
    to_name         TEXT   NOT NULL,
    to_name_lower   TEXT   NOT NULL,
    from_anchor     BIGINT NOT NULL,
    to_anchor       BIGINT NOT NULL,
    confidence      DOUBLE PRECISION NOT NULL,
    matched_boards  INTEGER NOT NULL,
    evidence        JSONB   NOT NULL DEFAULT '{}'::jsonb,
    method_version  INTEGER NOT NULL DEFAULT 1,
    created_at      BIGINT  NOT NULL,
    UNIQUE (from_name_lower, to_name_lower, to_anchor)
);
CREATE INDEX IF NOT EXISTS player_rename_to_anchor ON player_rename (to_anchor DESC);
CREATE INDEX IF NOT EXISTS player_rename_from ON player_rename (from_name_lower);
CREATE INDEX IF NOT EXISTS player_rename_to ON player_rename (to_name_lower);

-- Names that resolve to more than one identity. Two distinct causes, one column
-- (``kind``) apart: ``same_name`` = Trove's own dump lists the identical spelling
-- twice on the SAME board (two rows, two scores, one ``player`` row here), and
-- ``case`` = two spellings differing only in capitalisation, which ``name_lower``
-- folds into one player. Both make a single profile show two people's numbers, so
-- both are recorded here and surfaced on the Possible-duplicates tab.
-- One row per name (keyed by ``name_lower``): re-detecting refreshes it in place,
-- keeping the EARLIEST ``first_anchor`` so the record dates the duplication.
-- ``evidence`` holds the per-board series breakdown (which line is still moving,
-- which has stalled) - the same transparency contract as rename/cheater evidence.
CREATE TABLE IF NOT EXISTS player_duplicate (
    name_lower      TEXT   PRIMARY KEY,
    name            TEXT   NOT NULL,
    kind            TEXT   NOT NULL DEFAULT 'same_name',
    verdict         TEXT   NOT NULL DEFAULT 'all_idle',
    boards          INTEGER NOT NULL DEFAULT 0,
    max_occurrences INTEGER NOT NULL DEFAULT 2,
    spellings       JSONB   NOT NULL DEFAULT '[]'::jsonb,
    first_anchor    BIGINT  NOT NULL,
    last_anchor     BIGINT  NOT NULL,
    evidence        JSONB   NOT NULL DEFAULT '{}'::jsonb,
    method_version  INTEGER NOT NULL DEFAULT 1,
    updated_at      BIGINT  NOT NULL
);
CREATE INDEX IF NOT EXISTS player_duplicate_last ON player_duplicate (last_anchor DESC);
CREATE INDEX IF NOT EXISTS player_duplicate_kind ON player_duplicate (kind);

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

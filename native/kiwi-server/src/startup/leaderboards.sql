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

CREATE TABLE IF NOT EXISTS activity_active (
    window_end   BIGINT NOT NULL,
    player_lower TEXT   NOT NULL,
    PRIMARY KEY (window_end, player_lower)
);

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
ALTER TABLE player_board_agg ADD COLUMN IF NOT EXISTS last_rise BIGINT;

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
ALTER TABLE class_activity_estimate ADD COLUMN IF NOT EXISTS estimate_clean INTEGER;
ALTER TABLE activity_estimate ADD COLUMN IF NOT EXISTS estimate_24h INTEGER;
ALTER TABLE class_activity_estimate ADD COLUMN IF NOT EXISTS estimate_24h INTEGER;
ALTER TABLE class_activity_estimate ADD COLUMN IF NOT EXISTS estimate_24h_clean INTEGER;

CREATE TABLE IF NOT EXISTS class_activity_active (
    window_end   BIGINT  NOT NULL,
    class_index  INTEGER NOT NULL,
    player_lower TEXT    NOT NULL,
    clean        BOOLEAN NOT NULL,
    PRIMARY KEY (window_end, class_index, player_lower)
);

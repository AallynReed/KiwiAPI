-- Older table shapes the startup DDL upgrades in place.
CREATE TABLE player_board_agg (
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
CREATE TABLE codex_meta (
    branch         TEXT PRIMARY KEY,
    parser_version INTEGER NOT NULL DEFAULT 0,
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE TABLE rig_binding (
    branch     TEXT NOT NULL,
    blueprint  TEXT NOT NULL,
    skeleton   TEXT NOT NULL,
    ap_key     TEXT NOT NULL,
    PRIMARY KEY (branch, blueprint)
);

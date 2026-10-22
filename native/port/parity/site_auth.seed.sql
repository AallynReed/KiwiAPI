-- Site accounts sandbox: two players with captures (2030-01-05 trove-day, ahead of
-- the hot-window floor) for the claim baseline and /me/trove-stats.
-- DDL matches app/trove/leaderboards/pg_schema.py.
CREATE TABLE IF NOT EXISTS board (
    uuid                INTEGER PRIMARY KEY,
    name_id             TEXT NOT NULL,
    name                TEXT NOT NULL,
    category_id         TEXT NOT NULL,
    category            TEXT NOT NULL,
    reset_kind_override TEXT
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
CREATE TABLE IF NOT EXISTS entry_p_1893841200 PARTITION OF entry FOR VALUES FROM (1893841200) TO (1893927600);

INSERT INTO board (uuid, name_id, name, category_id, category, reset_kind_override) VALUES
    (1, 'Leaderboard_TroveMastery', 'Trove Mastery', 'Leaderboard_Category_Mastery', 'Mastery', NULL),
    (1000, 'Leaderboard_PR_Knight', 'Knight Power Rank', 'Leaderboard_Category_PowerRank', 'Power Rank', NULL)
ON CONFLICT DO NOTHING;

INSERT INTO player (name, name_lower) VALUES ('Alpha', 'alpha'), ('bravo', 'bravo') ON CONFLICT DO NOTHING;

INSERT INTO entry (board_uuid, anchor, rank, score, player_id)
SELECT b, a, r, s, (SELECT id FROM player WHERE name_lower = p)
FROM (VALUES (1, 1893841200, 5, 1000.5, 'alpha'), (1, 1893870000, 4, 1200.0, 'alpha'),
             (1000, 1893841200, 10, 30000.25, 'alpha'), (1, 1893841200, 6, 999.0, 'bravo')) AS v(b, a, r, s, p);

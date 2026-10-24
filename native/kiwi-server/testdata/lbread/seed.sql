-- Leaderboards read-side sandbox: boards, players and four trove-days of captures
-- (2030-01-05 .. 2030-01-08, across the Monday reset), dated ahead so the /site
-- browse window keeps them. DDL matches app/trove/leaderboards/pg_schema.py.
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
CREATE TABLE IF NOT EXISTS entry_p_1893841200 PARTITION OF entry FOR VALUES FROM (1893841200) TO (1893927600);
CREATE TABLE IF NOT EXISTS entry_p_1893927600 PARTITION OF entry FOR VALUES FROM (1893927600) TO (1894014000);
CREATE TABLE IF NOT EXISTS entry_p_1894014000 PARTITION OF entry FOR VALUES FROM (1894014000) TO (1894100400);
CREATE TABLE IF NOT EXISTS entry_p_1894100400 PARTITION OF entry FOR VALUES FROM (1894100400) TO (1894186800);

INSERT INTO board (uuid, name_id, name, category_id, category, reset_kind_override) VALUES
    (1, 'Leaderboard_TroveMastery', 'Trove Mastery', 'Leaderboard_Category_Mastery', 'Mastery', NULL),
    (20, 'Leaderboard_GeodeMastery', 'Geode Mastery', 'Leaderboard_Category_Mastery', 'Mastery', NULL),
    (1000, 'Leaderboard_PR_Knight', 'Knight Power Rank', 'Leaderboard_Category_PowerRank', 'Power Rank', NULL),
    (1001, 'Leaderboard_PR_Gunslinger', 'Gunslinger Power Rank', 'Leaderboard_Category_PowerRank', 'Power Rank', NULL),
    (4000, 'Leaderboard_Effort_Knight', 'Knight Effort', 'Leaderboard_Category_Effort', 'Effort', NULL),
    (32000, 'Leaderboard_Leviathans', 'Leviathans', 'Leaderboard_Category_Daily', 'Daily', NULL),
    (1100, 'Leaderboard_ClubPR', 'Club Power Rank', 'Leaderboard_Category_Clubs', 'Clubs', NULL),
    (2001, 'Leaderboard_DelveWins', 'Delve Wins été', 'Leaderboard_Category_Delves', 'Delves', 'none'),
    (50000, 'Leaderboard_日本', 'Ünïcødé 日本 "q"', 'Leaderboard_Category_Z', 'Zeta \ cat', 'monthly'),
    (777, 'Leaderboard_Empty', 'Never Captured', 'Leaderboard_Category_Z', 'Zeta \ cat', NULL)
ON CONFLICT DO NOTHING;

INSERT INTO board_contest (board_uuid, anchor, type) VALUES
    (1000, 1893927600, 'daily'), (1, 1894100400, 'weekly')
ON CONFLICT DO NOTHING;

INSERT INTO player (name, name_lower)
SELECT n, lower(n) FROM unnest(ARRAY['Alpha', 'bravo', 'Charlie_99', 'ΣΊΣΥΦΟΣ', 'İstanbul', 'ẞIG', 'Delta',
                                     'echo', 'Foxtrot', 'golfé', 'Hotel', 'ΜΙΚΡΟΣ']) AS n
ON CONFLICT DO NOTHING;

WITH a(anchor, k) AS (VALUES (1893841200, 0), (1893927600, 1), (1893942000, 2), (1894014000, 3),
                             (1894039200, 4), (1894100400, 5), (1894107600, 6)),
     b(uuid, base) AS (VALUES (1, 4000000.0), (20, 2500000.0), (1000, 41000.0), (1001, 39500.0),
                              (4000, 90000.0), (32000, 12000.0), (1100, 300000.0), (2001, 50.0),
                              (50000, 3.0)),
     p AS (SELECT id, (row_number() OVER (ORDER BY name_lower COLLATE "C") - 1) AS i FROM player),
     r AS (SELECT generate_series(1, 8) AS rank)
INSERT INTO entry (board_uuid, anchor, rank, score, player_id)
SELECT b.uuid, a.anchor, r.rank,
       round((b.base * (1 + 0.013 * a.k) / r.rank + (r.rank % 3) * 0.1)::numeric, r.rank % 3)::float8, p.id
FROM a CROSS JOIN b CROSS JOIN r
JOIN p ON p.i = (r.rank * 7 + a.k * 5 + b.uuid) % 12
WHERE NOT (b.uuid = 32000 AND a.k IN (1, 2, 4))
  AND NOT (b.uuid = 1001 AND a.k < 5)
  AND NOT (b.uuid = 2001 AND r.rank = 8 AND a.k % 2 = 0)
  AND NOT EXISTS (SELECT 1 FROM entry e WHERE e.anchor = a.anchor);

-- Trove's dump listing one name twice on a board.
INSERT INTO entry (board_uuid, anchor, rank, score, player_id)
SELECT 1000, e.anchor, 9, 11.5 + e.rank, e.player_id
FROM entry e
WHERE e.board_uuid = 1000 AND e.rank = 1
  AND NOT EXISTS (SELECT 1 FROM entry d WHERE d.board_uuid = 1000 AND d.anchor = e.anchor AND d.rank = 9);

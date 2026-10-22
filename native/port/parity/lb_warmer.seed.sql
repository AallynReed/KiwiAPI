-- Four days of 6-hourly captures ending just before now (both stacks seed within
-- the same 6h slot): lifetime, weekly, daily, Power Rank, Effort and club boards,
-- a rename across the newest pair (OldWarm -> NewWarm) and a same-name duplicate.
CREATE TABLE IF NOT EXISTS entry_p_parity PARTITION OF entry DEFAULT;

INSERT INTO board (uuid, name_id, name, category_id, category, reset_kind_override) VALUES
  (1, 'LB_MASTERY', 'TROVE MASTERY', 'CAT_MASTERY', 'Mastery', NULL),
  (20, 'LB_GEODE', 'GEODE MASTERY', 'CAT_MASTERY', 'Mastery', NULL),
  (11, 'LB_COINS', 'COINS COLLECTED', 'CAT_STATS', 'Stats', NULL),
  (2001, 'LB_DELVE', 'DELVE DEPTH', 'CAT_DELVES', 'Delves', NULL),
  (32000, 'LB_LEVI', 'LEVIATHANS', 'CAT_DAILY', 'Daily', NULL),
  (1100, 'LB_CLUB', 'CLUB POWER RANK', 'CAT_CLUBS', 'Clubs', NULL),
  (21005, 'LB_XP', 'EXPERIENCE', 'CAT_STATS', 'Stats', 'none')
ON CONFLICT DO NOTHING;
INSERT INTO board (uuid, name_id, name, category_id, category)
SELECT 1000 + i, 'LB_PR_' || i, 'POWER RANK ' || i, 'CAT_PR', 'Power Rank' FROM generate_series(0, 17) i
UNION ALL
SELECT 4000 + i, 'LB_EFFORT_' || i, 'EFFORT ' || i, 'CAT_EFFORT', 'Effort' FROM generate_series(0, 17) i
ON CONFLICT DO NOTHING;

INSERT INTO player (name, name_lower)
SELECT n, lower(n) FROM (
  SELECT 'Warm' || g AS n FROM generate_series(1, 40) g
  UNION ALL SELECT unnest(ARRAY['Ünïcode', 'ΣΟΦΟΣ', 'OldWarm', 'NewWarm', 'Twin', 'Runaway'])
) s
ON CONFLICT DO NOTHING;

CREATE TEMP TABLE parity_anchors AS
SELECT k, (floor(extract(epoch FROM now()) / 21600)::bigint * 21600 - 600 - k * 21600) AS anchor
FROM generate_series(0, 15) k;

INSERT INTO entry (board_uuid, anchor, rank, score, player_id)
SELECT board_uuid, anchor,
       (row_number() OVER (PARTITION BY board_uuid, anchor ORDER BY score DESC, player_id))::int,
       score, player_id
FROM (
  SELECT b.uuid AS board_uuid, a.anchor, p.id AS player_id,
         ((abs(hashtext(p.name || ':' || b.uuid)::bigint) % 5000)
          + (15 - a.k) * (abs(hashtext(p.name || '/' || b.uuid)::bigint) % 4) * 25)::float8
         * CASE WHEN b.uuid BETWEEN 1000 AND 1017 THEN 10 WHEN b.uuid = 21005 THEN 1000 ELSE 1 END
         * CASE WHEN p.name = 'Runaway' AND b.uuid = 11 THEN 400 ELSE 1 END AS score
  FROM parity_anchors a CROSS JOIN board b CROSS JOIN player p
  WHERE p.name NOT IN ('OldWarm', 'NewWarm', 'Twin')
    AND abs(hashtext(p.name || '#' || b.uuid || '#' || a.k)::bigint) % 10 < 8
) r;

INSERT INTO entry (board_uuid, anchor, rank, score, player_id)
SELECT v.b, a.anchor, 900 + v.r, v.s, p.id
FROM parity_anchors a
CROSS JOIN (VALUES (1, 1, 7654321.0), (20, 2, 1234567.0), (1000, 3, 456780.25)) AS v(b, r, s)
JOIN player p ON p.name_lower = CASE WHEN a.k = 0 THEN 'newwarm' ELSE 'oldwarm' END;

INSERT INTO entry (board_uuid, anchor, rank, score, player_id)
SELECT 11, a.anchor, v.r, v.s + (15 - a.k) * v.step, p.id
FROM parity_anchors a
CROSS JOIN (VALUES (950, 8100000.0, 0.0), (960, 2600000.0, 12500.0)) AS v(r, s, step)
JOIN player p ON p.name_lower = 'twin';

DROP TABLE parity_anchors;

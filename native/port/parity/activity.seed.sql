-- Eight days of 6-hourly captures ending just before now (both stacks seed within
-- the same 6h slot), plus some stored estimates for the backfills to skip or reset.
CREATE TABLE IF NOT EXISTS entry_p_parity PARTITION OF entry DEFAULT;

INSERT INTO board (uuid, name_id, name, category_id, category) VALUES
  (1, 'LB_ENEMIES', 'ENEMIES DEFEATED', 'CAT_COMBAT', 'Combat'),
  (2, 'LB_LOOT', 'LOOT COLLECTED', 'CAT_COMBAT', 'Combat'),
  (3, 'LB_FLUX', 'FLUX EARNED', 'CAT_ECONOMY', 'Économie'),
  (1100, 'LB_CLUB', 'CLUB POWER RANK', 'CAT_CLUBS', 'Clubs'),
  (21005, 'LB_XP', 'EXPERIENCE', 'CAT_STATS', 'Stats')
ON CONFLICT DO NOTHING;
INSERT INTO board (uuid, name_id, name, category_id, category)
SELECT 1000 + i, 'LB_PR_' || i, 'POWER RANK ' || i, 'CAT_PR', 'Power Rank' FROM generate_series(0, 17) i
UNION ALL
SELECT 4000 + i, 'LB_EFFORT_' || i, 'EFFORT ' || i, 'CAT_EFFORT', 'Effort' FROM generate_series(0, 17) i
ON CONFLICT DO NOTHING;

INSERT INTO player (name, name_lower)
SELECT n, lower(n) FROM (
  SELECT 'Parity' || g AS n FROM generate_series(1, 60) g
  UNION ALL SELECT unnest(ARRAY['Ünïcode', 'ΣΟΦΟΣ', 'dash-er', 'İstanbul'])
) s
ON CONFLICT DO NOTHING;

CREATE TEMP TABLE parity_anchors AS
SELECT k, (floor(extract(epoch FROM now()) / 21600)::bigint * 21600 - 600 - k * 21600) AS anchor
FROM generate_series(0, 31) k;

INSERT INTO entry (board_uuid, anchor, rank, score, player_id)
SELECT board_uuid, anchor,
       (row_number() OVER (PARTITION BY board_uuid, anchor ORDER BY score DESC, player_id))::int,
       score, player_id
FROM (
  SELECT b.uuid AS board_uuid, a.anchor, p.id AS player_id,
         ((abs(hashtext(p.name || ':' || b.uuid)::bigint) % 5000)
          + (31 - a.k) * (abs(hashtext(p.name || '/' || b.uuid)::bigint) % 4) * 25)::float8
         * CASE WHEN b.uuid BETWEEN 1000 AND 1017 THEN 10 WHEN b.uuid = 21005 THEN 1000 ELSE 1 END AS score
  FROM parity_anchors a CROSS JOIN board b CROSS JOIN player p
  WHERE abs(hashtext(p.name || '#' || b.uuid || '#' || a.k)::bigint) % 10 < 8
) r;

INSERT INTO activity_estimate (window_end, window_start, duration_hours, estimate, boards_analyzed, computed_at, estimate_24h)
SELECT anchor, anchor - 21600, 6.0, 100 + k, 3, anchor + 60, CASE WHEN k % 5 = 0 THEN NULL ELSE 400 + k * 3 END
FROM parity_anchors WHERE k BETWEEN 2 AND 20
ON CONFLICT DO NOTHING;

INSERT INTO class_activity_estimate
  (class_index, window_end, window_start, duration_hours, estimate, estimate_clean, computed_at, estimate_24h, estimate_24h_clean)
SELECT c, anchor, anchor - 21600, 6.0, 10 + c, CASE WHEN c % 4 = 0 THEN NULL ELSE c END, anchor + 60,
       CASE WHEN (c + k) % 6 = 0 THEN NULL ELSE 30 + c + k END,
       CASE WHEN (c + k) % 3 = 0 THEN NULL ELSE 5 + c END
FROM parity_anchors CROSS JOIN generate_series(0, 17) c WHERE k BETWEEN 1 AND 12
ON CONFLICT DO NOTHING;

DROP TABLE parity_anchors;

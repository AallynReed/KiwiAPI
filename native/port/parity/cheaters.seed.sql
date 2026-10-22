-- A few days of hourly-ish captures ending on the current hour (both twins seed within one hour).
DO $$
DECLARE
  now_h bigint := (floor(extract(epoch FROM now()) / 3600) * 3600)::bigint;
  a bigint;
  lo bigint;
BEGIN
  INSERT INTO board (uuid, name_id, name, category_id, category, reset_kind_override) VALUES
    (4001, 'LB_4001', 'Effort Knight', 'Leaderboard_Category_Effort', 'Effort', NULL),
    (32000, 'LB_32000', 'Leviathans', 'Leaderboard_Category_Daily', 'Daily', NULL),
    (1, 'LB_1', 'Trove Mastery', 'Leaderboard_Category_Mastery', 'Mastery', 'none'),
    (1100, 'LB_1100', 'Club Power Rank', 'Leaderboard_Category_Clubs', 'Clubs', NULL)
  ON CONFLICT (uuid) DO NOTHING;
  INSERT INTO player (name, name_lower) VALUES
    ('Parity Runaway', 'parity runaway'), ('Knight02', 'knight02'), ('knight03', 'knight03'),
    ('Knight04', 'knight04'), ('Levi', 'levi'), ('Ahab', 'ahab'), ('Néo', 'néo'), ('SomeClub', 'someclub')
  ON CONFLICT (name_lower) DO NOTHING;
  FOR d IN 0..3 LOOP
    FOR h IN 0..2 LOOP
      a := now_h - d * 86400 - h * 6 * 3600;
      lo := ((a - 39600) / 86400) * 86400 + 39600;
      EXECUTE format('CREATE TABLE IF NOT EXISTS entry_p_%s PARTITION OF entry FOR VALUES FROM (%s) TO (%s)',
                     lo, lo, lo + 86400);
      INSERT INTO entry (board_uuid, anchor, rank, score, player_id)
        SELECT 4001, a, v.r, v.s + d * 7 + h, p.id
        FROM (VALUES ('parity runaway', 1, 9000.5), ('knight02', 2, 3700.0), ('knight03', 3, 3400.25),
                     ('knight04', 4, 3100.0)) AS v(n, r, s)
        JOIN player p ON p.name_lower = v.n;
      INSERT INTO entry (board_uuid, anchor, rank, score, player_id)
        SELECT 32000, a, v.r, greatest(0, v.s - d * 40 - h * 3), p.id
        FROM (VALUES ('levi', 1, 500.0), ('ahab', 2, 460.0)) AS v(n, r, s)
        JOIN player p ON p.name_lower = v.n;
      INSERT INTO entry (board_uuid, anchor, rank, score, player_id)
        SELECT 1, a, v.r, v.s - d * 100 - h, p.id
        FROM (VALUES ('néo', 1, 900000.0), ('levi', 2, 891000.0)) AS v(n, r, s)
        JOIN player p ON p.name_lower = v.n;
    END LOOP;
  END LOOP;
  INSERT INTO entry (board_uuid, anchor, rank, score, player_id)
    SELECT 1100, now_h, 1, 5.0, id FROM player WHERE name_lower = 'someclub';
  INSERT INTO board_contest (board_uuid, anchor, type)
    VALUES (4001, now_h, 'Leaderboard_Category_Contests') ON CONFLICT DO NOTHING;
END $$;

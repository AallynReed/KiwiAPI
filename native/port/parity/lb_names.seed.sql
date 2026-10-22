-- Three captures in one trove-day: a rename (OldName -> NewName), two same-name
-- duplications (one live line + one frozen) and a case-only record to promote.
CREATE TABLE IF NOT EXISTS entry_p_1760007600 PARTITION OF entry FOR VALUES FROM (1760007600) TO (1760094000);

INSERT INTO board (uuid, name_id, name, category_id, category, reset_kind_override) VALUES
  (1, 'TROVE_MASTERY', 'Trove Mastery', 'c', 'Mastery', NULL),
  (20, 'GEODE_MASTERY', 'Geode Mastery', 'c', 'Mastery', NULL),
  (1000, 'PR_KNIGHT', 'Power Rank Knight', 'c', 'Power Rank', NULL),
  (11, 'COINS', 'Coins Collected', 'c', 'Stats', NULL),
  (4000, 'EFFORT_KNIGHT', 'Effort Knight', 'c', 'Effort', NULL),
  (1100, 'CLUB_PR', 'Club Power Rank', 'c', 'Clubs', NULL)
ON CONFLICT (uuid) DO NOTHING;

INSERT INTO player (name, name_lower) VALUES
  ('Alpha', 'alpha'), ('OldName', 'oldname'), ('NewName', 'newname'), ('Twin', 'twin'),
  ('Robot', 'robot'), ('ClubCo', 'clubco')
ON CONFLICT (name_lower) DO NOTHING;

INSERT INTO entry (board_uuid, anchor, rank, score, player_id)
SELECT b, a, r, s, (SELECT id FROM player WHERE name_lower = n)
FROM (VALUES
  (1, 1760011200, 1, 5000000.0, 'alpha'), (20, 1760011200, 2, 300000.0, 'alpha'), (4000, 1760011200, 1, 10.0, 'alpha'),
  (1, 1760032800, 1, 5000100.0, 'alpha'), (20, 1760032800, 2, 300000.0, 'alpha'), (4000, 1760032800, 1, 25.0, 'alpha'),
  (1, 1760054400, 1, 5000200.0, 'alpha'), (20, 1760054400, 2, 300000.0, 'alpha'), (4000, 1760054400, 1, 40.0, 'alpha'),
  (1, 1760011200, 2, 7654321.0, 'oldname'), (20, 1760011200, 1, 1234567.0, 'oldname'), (1000, 1760011200, 1, 45678.25, 'oldname'),
  (1, 1760032800, 2, 7654321.0, 'oldname'), (20, 1760032800, 1, 1234567.0, 'oldname'), (1000, 1760032800, 1, 45678.25, 'oldname'),
  (1, 1760054400, 2, 7654321.0, 'newname'), (20, 1760054400, 1, 1234567.0, 'newname'), (1000, 1760054400, 1, 45678.25, 'newname'),
  (11, 1760011200, 3, 8100000.0, 'twin'), (11, 1760011200, 40, 2600000.0, 'twin'),
  (11, 1760032800, 3, 8100000.0, 'twin'), (11, 1760032800, 38, 2850000.0, 'twin'),
  (11, 1760054400, 3, 8100000.0, 'twin'), (11, 1760054400, 35, 3100000.0, 'twin'),
  (11, 1760011200, 7, 900.0, 'robot'), (11, 1760011200, 8, 800.0, 'robot'),
  (11, 1760054400, 7, 900.0, 'robot'), (11, 1760054400, 8, 805.0, 'robot'), (11, 1760054400, 8, 805.0, 'robot'),
  (1100, 1760054400, 1, 50000.0, 'clubco'), (1100, 1760054400, 2, 40000.0, 'clubco')
) AS v(b, a, r, s, n);

INSERT INTO player_duplicate (name_lower, name, kind, verdict, boards, max_occurrences, spellings,
                              first_anchor, last_anchor, evidence, method_version, updated_at)
VALUES ('robot', 'Robot', 'case', 'case_only', 1, 2, to_jsonb(ARRAY['ROBOT', 'Robot']), 1760011200, 1760011200,
        '{}'::jsonb, 1, 1760011200),
       ('ghost', 'Ghost', 'same_name', 'all_idle', 1, 2, '[]'::jsonb, 1759000000, 1759000000, '{}'::jsonb, 1, 1759000000)
ON CONFLICT (name_lower) DO NOTHING;

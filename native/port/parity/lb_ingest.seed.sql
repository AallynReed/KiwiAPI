-- Boards of the parity dumps, pinned never-resetting so the completeness guard
-- judges them across the daily reset (the dumps' upserts keep the override).
INSERT INTO board (uuid, name_id, name, category_id, category, reset_kind_override)
SELECT uuid, 'Seed_' || uuid, 'Seed board ' || uuid, 'Leaderboard_Category_Seed', 'SEED', 'none'
FROM unnest(ARRAY[1, 20, 1000, 1001, 1002, 1003, 1004, 1005, 1006, 1007, 1008, 1009, 1010, 1011, 1012, 1013,
                  1014, 1015, 1016, 4000, 32000]) AS t(uuid)
ON CONFLICT (uuid) DO UPDATE SET reset_kind_override = EXCLUDED.reset_kind_override;

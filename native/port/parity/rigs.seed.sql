-- The live-us rig map the model projects resolve against: a costume with a head
-- scale, two NPC copies of one part, a skeleton with no baked rig and an AP the
-- baked skeleton lacks.
CREATE TABLE IF NOT EXISTS codex_meta (branch TEXT PRIMARY KEY, parser_version INTEGER NOT NULL DEFAULT 0,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now());
ALTER TABLE codex_meta ADD COLUMN IF NOT EXISTS rig_version INTEGER NOT NULL DEFAULT 0;
CREATE TABLE IF NOT EXISTS rig_binding (branch TEXT NOT NULL, prefab TEXT NOT NULL, blueprint TEXT NOT NULL,
    skeleton TEXT NOT NULL, ap_key TEXT NOT NULL, PRIMARY KEY (branch, prefab, blueprint));
ALTER TABLE rig_binding ADD COLUMN IF NOT EXISTS mesh_scale REAL NOT NULL DEFAULT 1;
ALTER TABLE rig_binding ADD COLUMN IF NOT EXISTS head_scale REAL NOT NULL DEFAULT 1;

INSERT INTO codex_meta (branch, parser_version, updated_at, rig_version) VALUES
('live-us', 27, '2026-10-20 00:00:00+00', 4)
ON CONFLICT (branch) DO UPDATE SET parser_version = EXCLUDED.parser_version, updated_at = EXCLUDED.updated_at;

INSERT INTO rig_binding (branch, prefab, blueprint, skeleton, ap_key, mesh_scale, head_scale) VALUES
('live-us', 'prefabs/npc/beetle.binfab', 'c_m_goldenbeetle_head', 'adventurer', 'head', 0.75, 1),
('live-us', 'prefabs/npc/delve/beetle.binfab', 'c_m_goldenbeetle_head', 'adventurer', 'head', 1, 1),
('live-us', 'prefabs/skins/adventurer/adv_knight.binfab', 'c_c_snowman_hat_01', 'adventurer', 'hat', 1, 0.588),
('live-us', 'prefabs/skins/adventurer/adv_knight.binfab', 'c_m_goldenbeetle_head', 'adventurer', 'head', 1, 0.588),
('live-us', 'prefabs/skins/adventurer/adv_knight.binfab', 'equipment_weapon_1h_sword_001', 'adventurer', 'r_hand', 1, 0.588),
('live-us', 'prefabs/skins/adventurer/adv_knight.binfab', 'char_hair_face_mustache_001', 'adventurer', 'face', 0.5, 0.588),
('live-us', 'prefabs/collections/mount/nobake.binfab', 'c_c_snowman_hat_01', 'not_a_baked_rig', 'hat', 1, 1),
('live-us', 'prefabs/collections/mount/ghost.binfab', 'e_cave_a_airlock_branch', 'bat', 'nope_ap', 1, 1),
('live-us', 'prefabs/collections/mount/lonely.binfab', 's_hat', 'bat', 'head', 1, 1)
ON CONFLICT DO NOTHING;

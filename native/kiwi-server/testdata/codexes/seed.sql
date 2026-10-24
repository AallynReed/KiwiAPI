-- A small codex for live-us (every table, floats that print differently in
-- pydantic and json.dumps, unicode, a crafting chain with market prices) and a
-- pts branch the admin rebuild replaces from a blob-less archive.
CREATE TABLE IF NOT EXISTS codex_entry (
    branch TEXT NOT NULL, path TEXT NOT NULL, codex_type TEXT NOT NULL, content_sha256 TEXT NOT NULL,
    name TEXT NOT NULL DEFAULT '', category TEXT NOT NULL DEFAULT '', description TEXT NOT NULL DEFAULT '',
    tradable BOOLEAN, mastery INTEGER, mastery_geode INTEGER, power_rank INTEGER, name_key TEXT, desc_key TEXT,
    blueprint TEXT, data JSONB NOT NULL DEFAULT '{}'::jsonb, indexed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (branch, path));
CREATE TABLE IF NOT EXISTS codex_stat (
    branch TEXT NOT NULL, path TEXT NOT NULL, ord INTEGER NOT NULL, stat_key TEXT NOT NULL, stat_id INTEGER,
    stat_name TEXT NOT NULL DEFAULT '', operation TEXT NOT NULL DEFAULT '', amount DOUBLE PRECISION,
    value DOUBLE PRECISION, is_percent BOOLEAN NOT NULL DEFAULT false, slot_key TEXT, label TEXT NOT NULL DEFAULT '',
    level INTEGER NOT NULL DEFAULT 0, PRIMARY KEY (branch, path, ord));
CREATE TABLE IF NOT EXISTS codex_ability (
    branch TEXT NOT NULL, path TEXT NOT NULL, ord INTEGER NOT NULL, ref TEXT NOT NULL,
    hidden BOOLEAN NOT NULL DEFAULT false, loc_key TEXT, name TEXT NOT NULL DEFAULT '',
    description TEXT NOT NULL DEFAULT '', PRIMARY KEY (branch, path, ord));
CREATE TABLE IF NOT EXISTS codex_link (
    branch TEXT NOT NULL, src_path TEXT NOT NULL, rel TEXT NOT NULL, dst_path TEXT NOT NULL,
    ord INTEGER NOT NULL DEFAULT 0, qty DOUBLE PRECISION, data JSONB NOT NULL DEFAULT '{}'::jsonb,
    PRIMARY KEY (branch, src_path, rel, dst_path, ord));
CREATE TABLE IF NOT EXISTS codex_requirement (
    branch TEXT NOT NULL, collection TEXT NOT NULL, rank INTEGER NOT NULL, rank_name TEXT NOT NULL DEFAULT '',
    badge_id TEXT NOT NULL DEFAULT '', completion_kind TEXT NOT NULL DEFAULT '', requirement_key TEXT NOT NULL DEFAULT '',
    label TEXT NOT NULL DEFAULT '', amount BIGINT, difficulty INTEGER NOT NULL DEFAULT 0, status TEXT NOT NULL DEFAULT '',
    context JSONB NOT NULL DEFAULT '{}'::jsonb, PRIMARY KEY (branch, collection, rank));
CREATE TABLE IF NOT EXISTS codex_upgrade (
    branch TEXT NOT NULL, system_kind TEXT NOT NULL, system_key TEXT NOT NULL, node_key TEXT NOT NULL, rank INTEGER,
    source_path TEXT NOT NULL DEFAULT '', costs JSONB NOT NULL DEFAULT '[]'::jsonb, requires JSONB NOT NULL DEFAULT '[]'::jsonb,
    PRIMARY KEY (branch, system_key, node_key));
ALTER TABLE codex_upgrade ADD COLUMN IF NOT EXISTS effects JSONB NOT NULL DEFAULT '{}'::jsonb;
CREATE TABLE IF NOT EXISTS codex_meta (branch TEXT PRIMARY KEY, parser_version INTEGER NOT NULL DEFAULT 0,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now());
ALTER TABLE codex_meta ADD COLUMN IF NOT EXISTS rig_version INTEGER NOT NULL DEFAULT 0;
CREATE TABLE IF NOT EXISTS rig_binding (branch TEXT NOT NULL, prefab TEXT NOT NULL, blueprint TEXT NOT NULL,
    skeleton TEXT NOT NULL, ap_key TEXT NOT NULL, PRIMARY KEY (branch, prefab, blueprint));
ALTER TABLE rig_binding ADD COLUMN IF NOT EXISTS mesh_scale REAL NOT NULL DEFAULT 1;
ALTER TABLE rig_binding ADD COLUMN IF NOT EXISTS head_scale REAL NOT NULL DEFAULT 1;
CREATE TABLE IF NOT EXISTS market_listing (
    id UUID PRIMARY KEY, name TEXT NOT NULL, type TEXT, stack INTEGER NOT NULL, price BIGINT NOT NULL,
    price_each DOUBLE PRECISION NOT NULL, last_seen BIGINT NOT NULL, created_at BIGINT NOT NULL DEFAULT 0);

INSERT INTO codex_entry (branch, path, codex_type, content_sha256, name, category, description, tradable, mastery,
                         mastery_geode, power_rank, name_key, desc_key, blueprint, data, indexed_at) VALUES
('live-us', 'prefabs/collections/mount/dragon_amperium.binfab', 'dragon', 'sha-a', 'Amperium Dragon', 'Dragon',
 'Crackles with “power”.', true, 50, 75, 30, '$n', '$d', 'C_MT_dragonserpent_amperium_ui.blueprint',
 '{"stats": [{"stat": "$Stat_MaxHealth", "amount": 1000.0, "value": 1000.0, "is_percent": false},
             {"stat": "$Stat_MagicFind", "amount": 0.00001, "value": 1e16, "is_percent": true}],
   "abilities": [{"ref": "abilities/x", "hidden": false, "name": "Zap"}]}', '2026-10-20 12:34:56.12345+00'),
('live-us', 'prefabs/collections/mount/horse.binfab', 'mount', 'sha-b', 'Horse', 'Mounts', 'Neigh', false, 50, NULL,
 5, NULL, NULL, NULL, '{}', '2026-10-20 12:34:56+00'),
('live-us', 'prefabs/collections/boat/clipper.binfab', 'boat', 'sha-c', 'Clipper_50%', 'Boats', '', NULL, 50, NULL,
 NULL, NULL, NULL, 'boat.blueprint', '{"stats": [{"stat": "$Stat_MagicFind", "amount": 0.30000001192092896, "value": 30.000001192092896}]}',
 '2026-10-21 01:02:03.000001+00'),
('live-us', 'prefabs/recipes/recipe_a.binfab', 'recipe', 'sha-r1', 'Thing A', 'Mount', '', NULL, 2, NULL, NULL, NULL,
 NULL, NULL, '{"recipe": {"output": {"path": "item/a", "name": "", "amount": 2},
  "ingredients": [{"path": "item/b", "name": "Thing B", "amount": 3}, {"path": "item/c", "name": "", "amount": 0}],
  "requirements": []}}', '2026-10-20 00:00:00+00'),
('live-us', 'prefabs/recipes/recipe_b.binfab', 'recipe', 'sha-r2', 'Thing B', 'Recipe', '', NULL, NULL, NULL, NULL,
 NULL, NULL, NULL, '{"recipe": {"output": {"path": "item/b", "name": "Thing B", "amount": 1},
  "ingredients": [{"path": "item/d", "name": "Glim", "amount": 5}, {"path": "item/a", "name": "Thing A", "amount": 1}]}}',
 '2026-10-20 00:00:00+00'),
('live-us', 'prefabs/item/b.binfab', 'item', 'sha-i', 'Thing B', 'Crafting', 'An ingredient', true, NULL, NULL, NULL,
 NULL, NULL, 'b.blueprint', '{}', '2026-10-20 00:00:00+00'),
('pts', 'prefabs/collections/mount/horse.binfab', 'mount', 'sha-old', 'Old Horse', '', '', NULL, NULL, NULL, NULL,
 NULL, NULL, NULL, '{}', '2026-01-01 00:00:00+00'),
('pts', 'prefabs/item/gone.binfab', 'item', 'sha-gone', 'Gone', '', '', NULL, NULL, NULL, NULL, NULL, NULL, NULL,
 '{}', '2026-01-01 00:00:00+00')
ON CONFLICT DO NOTHING;

INSERT INTO codex_stat VALUES
('live-us', 'prefabs/collections/mount/dragon_amperium.binfab', 0, '$Stat_MaxHealth', 2, 'Maximum Health', 'Add', 1000, 1000, false, NULL, 'hp', 0),
('live-us', 'prefabs/collections/mount/dragon_amperium.binfab', 1, '$Stat_MagicFind', 13, 'Magic Find', 'Add', 0.1, 0.1, false, '$EquipmentSlot_Wings', '', 0),
('live-us', 'prefabs/collections/mount/dragon_amperium.binfab', 2, '$Stat_MagicFind', 13, 'Magic Find', 'Add', 25, 25, false, '$EquipmentSlot_Mount', '', 0),
('live-us', 'prefabs/collections/boat/clipper.binfab', 0, '$Stat_MagicFind', 13, 'Magic Find', 'MultiplySum', 0.30000001192092896, 30.000001192092896, true, '$EquipmentSlot_Boat', 'mf', 0),
('live-us', 'prefabs/collections/mount/horse.binfab', 0, '$Stat_MagicFind', NULL, '', 'Set', NULL, NULL, false, NULL, '', 0),
('live-us', 'prefabs/collections/mount/horse.binfab', 1, '$Stat_SpellDamage', 1, 'Magic Damage', 'Add', 0.00001, 0.00001, true, NULL, '', 0),
('pts', 'prefabs/item/gone.binfab', 0, '$Stat_Jump', 9, 'Jump', 'Add', 1, 1, false, NULL, '', 0)
ON CONFLICT DO NOTHING;

INSERT INTO codex_ability VALUES
('live-us', 'prefabs/collections/mount/dragon_amperium.binfab', 0, 'abilities/x', false, '$k', 'Zap', 'Zaps things'),
('live-us', 'prefabs/collections/boat/clipper.binfab', 0, 'abilities/x', false, NULL, '', ''),
('live-us', 'prefabs/collections/boat/clipper.binfab', 1, 'abilities/hidden', true, NULL, '', '')
ON CONFLICT DO NOTHING;

INSERT INTO codex_link VALUES
('live-us', 'prefabs/recipes/recipe_b.binfab', 'crafts', 'prefabs/item/b.binfab', 0, 1, '{}'),
('live-us', 'prefabs/recipes/recipe_b.binfab', 'ingredient', 'prefabs/item/d.binfab', 0, 5, '{}'),
('live-us', 'prefabs/recipes/recipe_a.binfab', 'ingredient', 'prefabs/item/b.binfab', 0, 3, '{}'),
('live-us', 'prefabs/recipes/recipe_b.binfab', 'craftable_at', 'prefabs/placeable/crafting/bench.binfab', 0, NULL,
 '{"lane": "bench", "category": "$Tab", "category_order": null}'),
('live-us', 'prefabs/item/b.binfab', 'unlocks', 'prefabs/collections/mount/horse.binfab', 0, NULL, '{}'),
('live-us', 'prefabs/collections/mount/horse.binfab', 'member_of', 'prefabs/collections/collection_mount.binfab', 3, NULL, '{"group": "Mounts"}'),
('live-us', 'barrier:barrier_01', 'upgrade_cost', 'prefabs/item/b.binfab', 0, 2.5,
 '{"system_kind": "geode_module", "system_key": "barrier", "node_key": "barrier_01", "source_path": "prefabs/upgrade/trees/barrier.binfab"}'),
('pts', 'barrier:barrier_01', 'upgrade_cost', 'prefabs/item/b.binfab', 0, 1, '{}'),
('pts', 'prefabs/item/x.binfab', 'member_of', 'prefabs/collections/collection_x.binfab', 0, NULL, '{}'),
('pts', 'prefabs/item/x.binfab', 'unlocks', 'prefabs/item/y.binfab', 0, NULL, '{}')
ON CONFLICT DO NOTHING;

INSERT INTO codex_requirement VALUES
('live-us', 'collections/badge/blocks_bronze', 1, 'bronze', 'blocks', 'metric', 'BlocksDestroyed', 'Blocks Destroyed', 1000, 0, 'decoded', '{"metric_index": 3, "encoded_metric": 6}'),
('live-us', 'collections/badge/blocks_silver', 2, 'silver', 'blocks', 'metric', 'BlocksDestroyed', 'Blocks Destroyed', 9007199254740993, 0, 'decoded', '{}'),
('live-us', 'collections/badge/st', 1, 'bronze', 'st', 'STBossKilled', 'x:Ultra', 'Defeat', NULL, 3, 'blocked', '{"difficulty": "Ultra"}'),
('pts', 'collections/badge/old', 1, 'bronze', 'old', 'none', 'none', '', 0, 0, 'decoded', '{}')
ON CONFLICT DO NOTHING;

INSERT INTO codex_upgrade (branch, system_kind, system_key, node_key, rank, source_path, costs, requires, effects) VALUES
('live-us', 'geode_module', 'barrier', 'barrier_02', 2, 'prefabs/upgrade/trees/barrier.binfab',
 '[{"item": "item/crafting/x", "quantity": 10}]', '["barrier_01"]', '{"name": "Shield"}'),
('live-us', 'geode_module', 'barrier', 'barrier_01', 1, 'prefabs/upgrade/trees/barrier.binfab',
 '[{"item": "item/crafting/x", "quantity": 5}, {"item": "item/currency/y", "quantity": 1}]', '[]', '{}'),
('live-us', 'class_prestige', 'prestige_bard', '01_bard_root_01', NULL, 'p', '[]', '[]', '{}'),
('pts', 'geode_module', 'barrier', 'barrier_01', 1, 'p', '[]', '[]', '{}')
ON CONFLICT DO NOTHING;

INSERT INTO codex_meta (branch, parser_version, updated_at, rig_version) VALUES
('live-us', 27, '2026-10-20 00:00:00+00', 4), ('pts', 26, '2026-01-01 00:00:00+00', 3)
ON CONFLICT DO NOTHING;

INSERT INTO rig_binding VALUES
('live-us', 'prefabs/collections/mount/horse.binfab', 'c_m_horse_body', 'mount_horse', 'body', 1, 1),
('pts', 'prefabs/collections/mount/horse.binfab', 'c_m_horse_body', 'mount_horse', 'body', 0.5, 0.588)
ON CONFLICT DO NOTHING;

WITH t AS (SELECT extract(epoch FROM now())::bigint AS now)
INSERT INTO market_listing (id, name, type, stack, price, price_each, last_seen, created_at)
SELECT md5('codex:' || n || ':' || s)::uuid, n, 'Material', 1, p * s, p * s, t.now + 86400, t.now - 3600
FROM t, (VALUES ('Thing B', 12.345), ('Glim', 0.1)) AS v(n, p), generate_series(1, 3) s
ON CONFLICT DO NOTHING;

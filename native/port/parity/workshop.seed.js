// Twin sandbox seed for the Mod Workshop: a live-us tree for the placement check
// (only the paths are read, so no blobs).
const sha = (c) => c.repeat(64);
const paths = [
  "blueprints/equipment/weapon/equipment_weapon_1h_sword_001.blueprint",
  "ui/pvpplayercounterui.swf",
  "prefabs/collections/flask/effect_crit.binfab",
  "blueprints/creatures/beetle/c_m_goldenbeetle_head.blueprint",
  "blueprints/skins/adventurer/c_c_snowman_hat_01.blueprint",
  "blueprints/npc/merchant/c_c_snowman_hat_01.blueprint",
  "Textures/Shared/Foo.dds",
  "blueprints/skins/adventurer/char_hair_face_mustache_001.blueprint",
];
db.update_branches.deleteMany({branch: "live-us"});
db.update_branches.insertOne({branch: "live-us", content_path: "patchkiwi-live-us01", current_version: "TWIN-1",
  current_ordinal: NumberInt(1), last_probe_at: null, status: "idle", last_error: null});
db.update_state.insertMany(paths.map((path, i) => ({
  branch: "live-us", path, content_sha256: sha("abcdef0123456789"[i]), fnv_hash: null, size: NumberInt(10),
  archive: null, archive_index: null, last_ordinal: NumberInt(1)})));

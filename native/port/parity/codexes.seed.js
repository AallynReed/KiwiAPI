// A pts archive with no blobs on disk: the admin rebuild walks it, finds every blob
// missing, prunes the stale pts rows and empties the shared tables it rebuilds.
const sha = (i) => i.toString(16).padStart(64, "0");
const paths = [
  "prefabs/collections/mount/horse.binfab",
  "prefabs/collections/mount/dragon_new.binfab",
  "prefabs/recipes/recipe_new.binfab",
  "prefabs/item/companion/sample.binfab",
  "prefabs/equipment/hat_new.binfab",
  "prefabs/meta/badges.binfab",
  "prefabs/meta/multipliers.binfab",
  "prefabs/collections/unlocks.binfab",
  "prefabs/upgrade/trees/barrier.binfab",
  "prefabs/upgrade/upgrades/barrier.binfab",
  "prefabs/collections/collection_mount.binfab",
  "prefabs/loot/hat.binfab",
  "prefabs/placeable/crafting/workbench_interactive.binfab",
  "prefabs/npc/not_a_codex.binfab",
  "languages/en/prefabs_x.binfab",
  "blueprints/c_m_horse_body.blueprint",
  "Trove_x64.exe",
];
db.update_state.insertMany(paths.map((path, i) => ({
  branch: "pts", path, content_sha256: sha(i + 1), fnv_hash: null, size: 10, archive: null,
  archive_index: null, last_ordinal: 1,
})));
db.update_state.createIndex({branch: 1, path: 1}, {unique: true});

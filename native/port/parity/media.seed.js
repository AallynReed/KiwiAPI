// Twin sandbox seed for SWF/audio: archive rows whose blobs are not in the
// (empty) update store, and a path removed after its last version.
const sha = (c) => c.repeat(64);
db.update_state.insertMany([
  {branch: "live-us", path: "ui/seeded.swf", content_sha256: sha("a"), fnv_hash: null, size: NumberInt(10),
   archive: null, archive_index: null, last_ordinal: NumberInt(3)},
  {branch: "live-us", path: "audio/seeded.bnk", content_sha256: sha("b"), fnv_hash: null, size: NumberInt(10),
   archive: null, archive_index: null, last_ordinal: NumberInt(3)},
  {branch: "pts", path: "audio/seeded.txt", content_sha256: sha("c"), fnv_hash: null, size: NumberInt(10),
   archive: null, archive_index: null, last_ordinal: NumberInt(3)},
]);
db.update_changes.insertMany([
  {branch: "live-us", ordinal: NumberInt(1), path: "audio/removed.bnk", type: "added", content_sha256: sha("d"),
   fnv_hash: null, size: NumberInt(10)},
  {branch: "live-us", ordinal: NumberInt(2), path: "audio/removed.bnk", type: "removed", content_sha256: null,
   fnv_hash: null, size: NumberInt(0)},
]);

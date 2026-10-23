// The pts timeline the fake CDN does not publish: a change-log for the
// last_ordinal backfill to recompute, state rows still at 0, one row already
// right, and a removed path that must not come back.
db.update_versions.insertMany([
  {_id: ObjectId("65f100000000000000000001"), branch: "pts", ordinal: NumberInt(1), version_tag: "PTS-1", status: "complete",
   captured_at: ISODate("2026-09-01T00:00:00Z"), completed_at: ISODate("2026-09-01T00:05:00Z"), content_path: "patchkiwi-pts",
   motd: "", files_added: NumberInt(3), files_modified: NumberInt(0), files_removed: NumberInt(0), bytes_added: NumberInt(30)},
  {_id: ObjectId("65f100000000000000000002"), branch: "pts", ordinal: NumberInt(2), version_tag: "PTS-2", status: "complete",
   captured_at: ISODate("2026-09-08T00:00:00Z"), completed_at: ISODate("2026-09-08T00:05:00Z"), content_path: "patchkiwi-pts",
   motd: "", files_added: NumberInt(0), files_modified: NumberInt(1), files_removed: NumberInt(1), bytes_added: NumberInt(10)},
  // A live-us sync that died half way: the next one resumes its ordinal; the legacy row lacks the later fields.
  {_id: ObjectId("65f100000000000000000003"), branch: "live-us", ordinal: NumberLong(1), version_tag: "TWIN-0",
   status: "in_progress", captured_at: ISODate("2026-10-01T00:00:00.123Z"), content_path: "patchkiwi-live-us01"},
]);
db.update_changes.insertMany([
  {branch: "pts", ordinal: NumberInt(1), path: "a.bin", type: "added", content_sha256: "a1".repeat(32), fnv_hash: null, size: NumberInt(10)},
  {branch: "pts", ordinal: NumberInt(1), path: "b.bin", type: "added", content_sha256: "b1".repeat(32), fnv_hash: null, size: NumberInt(10)},
  {branch: "pts", ordinal: NumberInt(1), path: "gone.bin", type: "added", content_sha256: "c1".repeat(32), fnv_hash: null, size: NumberInt(10)},
  {branch: "pts", ordinal: NumberInt(2), path: "b.bin", type: "modified", content_sha256: "b2".repeat(32), fnv_hash: null, size: NumberInt(10)},
  {branch: "pts", ordinal: NumberLong(2), path: "gone.bin", type: "removed", content_sha256: null, fnv_hash: null, size: NumberInt(0)},
]);
db.update_state.insertMany([
  {branch: "pts", path: "a.bin", content_sha256: "a1".repeat(32), fnv_hash: null, size: NumberInt(10), archive: null, archive_index: null,
   last_ordinal: NumberInt(0)},
  {branch: "pts", path: "b.bin", content_sha256: "b2".repeat(32), fnv_hash: null, size: NumberInt(10), archive: null, archive_index: null,
   last_ordinal: NumberInt(2)},
  {branch: "pts", path: "legacy.bin", content_sha256: "d1".repeat(32), fnv_hash: null, size: NumberInt(10), archive: null,
   archive_index: null},
]);

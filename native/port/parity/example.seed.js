// Twin sandbox example: fixed ids and dates keep a seed deterministic.
db.runtime_config.insertOne({key: "design_feedback_rate_limit_max", value: "1000",
  updated_at: ISODate("2026-01-01T00:00:00Z"), updated_by_user_id: null});
db.supporters.insertOne({_id: ObjectId("65a000000000000000000001"), name: "Parity Seeded", added_by: null,
  created_at: ISODate("2026-01-01T00:00:00Z")});
db.design_feedback.insertOne({_id: "2026-01-01|voxel|auto|up", count: NumberInt(3), day: "2026-01-01",
  design: "voxel", theme: "auto", vote: "up", updated_at: ISODate("2026-01-01T12:00:00Z")});

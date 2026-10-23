// Twin sandbox seed for /openapi.json: flags stored as JSON, as junk, and as non-bool values.
db.runtime_config.deleteMany({key: {$regex: "^feature_"}});
db.runtime_config.insertMany([
  {key: "feature_delves_enabled", value: "true", updated_at: ISODate("2026-01-01T00:00:00Z"), updated_by_user_id: null},
  {key: "feature_market_enabled", value: "false", updated_at: ISODate("2026-01-01T00:00:00Z"), updated_by_user_id: null},
  {key: "feature_codexes_enabled", value: "0", updated_at: ISODate("2026-01-01T00:00:00Z"), updated_by_user_id: null},
  {key: "feature_store_enabled", value: "not json", updated_at: ISODate("2026-01-01T00:00:00Z"), updated_by_user_id: null},
  {key: "feature_updates_enabled", value: "\"\"", updated_at: ISODate("2026-01-01T00:00:00Z"), updated_by_user_id: null},
  {key: "feature_embed_viewer_enabled", value: "[0]", updated_at: ISODate("2026-01-01T00:00:00Z"), updated_by_user_id: null}
]);

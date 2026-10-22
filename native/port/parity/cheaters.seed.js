// Cheater detection on (alt clusters off) so serving takes the snapshot path, not the disabled payload.
db.runtime_config.deleteMany({key: {$in: ["feature_cheater_detection_enabled", "feature_alt_clusters_enabled"]}});
db.runtime_config.insertMany([
  {key: "feature_cheater_detection_enabled", value: "true", updated_at: new Date(), updated_by_user_id: null},
  {key: "feature_alt_clusters_enabled", value: "false", updated_at: new Date(), updated_by_user_id: null},
]);

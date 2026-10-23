// Twin sandbox seed for the file tools: the Unlock Debug patcher ships off,
// so it is switched on to reach the patch itself.
db.runtime_config.deleteMany({key: "feature_unlock_debug_enabled"});
db.runtime_config.insertOne({key: "feature_unlock_debug_enabled", value: "true", updated_at: new Date(), updated_by_user_id: null});

// Targets the moderation report cases resolve, and a throttle the votes won't hit.
db.mod_projects.insertMany([
  {owner_handle: "parity", slug: "parity-mod", title: "Parity Mod", visibility: "public"},
  {owner_handle: "parity", slug: "parity-draft", title: "D", visibility: "draft"},
]);
db.mod_profiles.insertOne({handle: "parity", display_name: "", site_user_id: new ObjectId()});
db.runtime_config.insertOne({key: "design_feedback_rate_limit_max", value: "1000", updated_at: new Date(), updated_by_user_id: null});

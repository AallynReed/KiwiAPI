// Legacy shapes startup migrates, written into both stacks before either app boots.
const id = (n) => ObjectId(n.toString(16).padStart(24, "0"));
const day = (s) => ISODate(s + "T11:00:00Z");

db.mod_projects.createIndex({source: 1, source_id: 1}, {name: "source_1_source_id_1", unique: true, sparse: true});
db.mod_projects.createIndex({slug: 1}, {unique: true});
db.mod_releases.createIndex({project_id: 1, tag: 1}, {unique: true});
db.site_users.createIndex({email: 1}, {unique: true});
db.usage_events.createIndex({created_at: 1}, {name: "created_at_ttl", expireAfterSeconds: 86400});
db.page_views.createIndex({created_at: 1});

db.users.insertOne({
  _id: id(1), email: "parity@aallyn.net",
  hashed_password: "$argon2id$v=19$m=65536,t=3,p=4$ZHxN6uydcROdeYROs5VerQ$CsR2b/PmhDl3seL14+A3irsi0A+OpsBEyxssv4AK5iI",
  display_name: "Old admin", is_active: true, is_verified: false, is_superuser: false, roles: [], token_version: 0,
  github_id: null, email_bounced: false, created_at: day("2025-01-01"), updated_at: day("2025-01-01"), last_login_at: null,
});

db.site_users.insertMany([
  {_id: id(10), username: "alice", discord_id: NumberLong(111), email: "alice@example.com",
   discord_guilds: [{id: "1", name: "g"}], discord_guilds_synced_at: day("2025-02-01")},
  {_id: id(11), username: "aallyn", discord_id: NumberLong(222)},
]);

db.mod_projects.insertMany([
  {_id: id(20), slug: "p1", title: "P1", owner_id: id(10), owner_handle: "old-alice", visibility: "public",
   taken_down: false, created_at: day("2025-03-01")},
  {_id: id(21), slug: "p2", title: "P2", owner_id: id(11), owner_handle: "aallyn", visibility: "draft",
   taken_down: false, last_release_at: null, created_at: day("2025-03-02")},
  {_id: id(22), slug: "p3", title: "P3", owner_id: id(10), owner_handle: "alice-was", visibility: "public",
   taken_down: true, takedown_reason: "test", created_at: day("2025-03-03")},
  {_id: id(23), slug: "p4", title: "P4", owner_id: id(11), owner_handle: "aallyn", visibility: "public",
   taken_down: false, issues_enabled: true, last_release_at: day("2025-04-01"), created_at: day("2025-03-04")},
  {_id: id(24), slug: "p5", title: "P5", owner_id: null, owner_handle: "", visibility: "unlisted", taken_down: false},
]);

db.mod_releases.insertMany([
  {_id: id(30), project_id: id(20), tag: "1.0", branch: "main", tmod_sha: "a".repeat(64), status: "published",
   published_at: day("2025-05-01"), created_at: day("2025-05-01")},
  {_id: id(31), project_id: id(20), tag: "1.1", branch: "main", tmod_sha: "b".repeat(64), status: "published",
   published_at: null, created_at: day("2025-05-02")},
  {_id: id(32), project_id: id(21), tag: "0.1", branch: "main", tmod_sha: "c".repeat(64), status: "published",
   published_at: day("2025-05-03"), created_at: day("2025-05-03")},
  {_id: id(33), project_id: id(22), tag: "2.0", branch: "main", tmod_sha: "d".repeat(64),
   prior_tmod_shas: ["e".repeat(64), ""], status: "draft", published_at: null, created_at: day("2025-05-04")},
  {_id: id(34), project_id: id(23), tag: "3.0", branch: "main", tmod_sha: "f".repeat(64), status: "published",
   published_at: day("2025-05-05"), announced_at: day("2025-05-06"), created_at: day("2025-05-05")},
]);

db.modpack_projects.insertOne({_id: id(40), slug: "pack", title: "Pack", owner_id: id(10), owner_username: "alice",
  owner_handle: "old-alice", visibility: "public", taken_down: false});

db.sessions.insertOne({_id: id(50), user_id: id(1), refresh_token_hash: "s1", ip: "203.0.113.9", user_agent: "UA",
  device: null, created_at: day("2025-06-01"), last_used_at: day("2025-06-01"), expires_at: day("2099-01-01"), revoked: false});
db.site_sessions.insertOne({_id: id(51), site_user_id: id(10), refresh_token_hash: "s2", user_agent: "UA",
  created_at: day("2025-06-01"), last_used_at: day("2025-06-01"), expires_at: day("2099-01-01"), revoked: false});

db.giveaways.insertOne({_id: id(60), title: "G", prize_name: "Prize", starts_at: day("2025-07-01"),
  ends_at: day("2025-07-02"), status: "ended", winner_email: "w@example.com"});
db.prize_codes.insertOne({_id: id(61), vault_item_id: id(62), code: "CODE", status: "awarded",
  awarded_to_email: "w@example.com"});

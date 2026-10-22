// Accounts, tokens, usage, site users, claims and username requests the admin write specs act on.
// Some rows are stored the way an old writer left them (int64 small ints, a missing field, an
// upper-case email domain, int baselines) so a save shows Beanie's normalization on both stacks.
const now = Date.now();
const ago = (mins) => new Date(now - mins * 60000);
const t0 = new Date("2026-01-02T03:04:05.123Z");
const U1 = ObjectId("65ad00000000000000000a01"), U2 = ObjectId("65ad00000000000000000a02");
const T1 = ObjectId("65ad00000000000000000b01"), T2 = ObjectId("65ad00000000000000000b02"), T3 = ObjectId("65ad00000000000000000b03");
const S1 = ObjectId("65ad00000000000000000c01"), S2 = ObjectId("65ad00000000000000000c02"), S3 = ObjectId("65ad00000000000000000c03");

db.users.deleteMany({_id: {$in: [U1, U2]}});
db.users.insertMany([
  {_id: U1, email: "ada@kiwi-mail.net", hashed_password: "x", display_name: "Ada", is_active: true, is_verified: true,
   is_superuser: false, roles: [], token_version: NumberInt(1), github_id: null, email_bounced: false,
   created_at: t0, updated_at: t0, last_login_at: ago(90)},
  {_id: U2, email: "Bob@Kiwi-Mail.NET", hashed_password: "x", display_name: null, is_active: true, is_verified: false,
   is_superuser: false, roles: ["beta"], token_version: NumberLong(5), github_id: NumberLong("12345678901"),
   created_at: t0, updated_at: t0, last_login_at: null},
]);
db.api_tokens.deleteMany({_id: {$in: [T1, T2, T3]}});
db.api_tokens.insertMany([
  {_id: T1, user_id: U1, name: "laptop", prefix: "kiwi_ab12cd34", hashed_token: "admin-parity-h1", scopes: NumberInt(0),
   ip_salt: null, allowed_ip_hashes: ["a", "b"], revoked: false, revoked_at: null, revoke_reason: null, expires_at: null,
   expiry_warned: false, created_at: t0, last_used_at: ago(30), rotated_at: null, request_count: NumberInt(12)},
  {_id: T2, user_id: U1, name: "bot", prefix: "kiwi_zz99yy88", hashed_token: "admin-parity-h2", scopes: NumberInt(1536),
   ip_salt: null, allowed_ip_hashes: [], revoked: true, revoked_at: t0, revoke_reason: "Revoked by administrator",
   expires_at: null, expiry_warned: false, created_at: t0, last_used_at: null, rotated_at: null, request_count: NumberLong(3000000000)},
  {_id: T3, user_id: U2, name: "ci", prefix: "kiwi_cc00dd11", hashed_token: "admin-parity-h3", scopes: NumberLong(2),
   revoked: false, created_at: t0, request_count: NumberInt(1)},
]);
db.sessions.deleteMany({user_id: {$in: [U1, U2]}});
db.sessions.insertMany([
  {user_id: U2, refresh_token_hash: "admin-parity-r1", device: "Firefox on Linux", created_at: t0, last_used_at: t0,
   expires_at: new Date(now + 86400000), revoked: false},
  {user_id: U2, refresh_token_hash: "admin-parity-r2", device: null, created_at: t0, last_used_at: t0,
   expires_at: new Date(now + 86400000), revoked: true},
]);
db.usage_events.deleteMany({user_id: {$in: [U1, U2]}});
db.usage_events.insertMany([
  {_id: ObjectId("65ad00000000000000000e01"), user_id: U1, token_id: T1, method: "GET", route: "/v1/stats/classes",
   path: "/v1/stats/classes", status_code: NumberInt(200), duration_ms: 12.5, created_at: ago(200)},
  {_id: ObjectId("65ad00000000000000000e02"), user_id: U1, token_id: T1, method: "GET", route: "/v1/stats/classes",
   path: "/v1/stats/classes", status_code: NumberInt(429), duration_ms: NumberInt(3), created_at: ago(100)},
  {_id: ObjectId("65ad00000000000000000e03"), user_id: U2, token_id: T3, method: "POST", route: "/v1/gems/evaluate",
   path: "/v1/gems/evaluate", status_code: NumberInt(500), duration_ms: 2.675, created_at: ago(50)},
]);

db.site_users.deleteMany({_id: {$in: [S1, S2, S3]}});
db.site_users.insertMany([
  {_id: S1, username: "kiwi_fan", discord_handle: "KiwiFan", notify_email: null, display_name: "Kiwi Fän", is_active: true,
   is_verified: true, is_deleted: false, discord_id: NumberLong("812345678901234567"), discord_avatar: null,
   token_version: NumberInt(0), claimed_trove_name: "sir kiwi", claimed_trove_display: "Sir Kiwi", claimed_at: t0,
   claim_verified: false, claim_baseline: {"12": NumberInt(4), "1000": 17.5}, claim_verified_at: null,
   creator_token_hash: null, creator_token_prefix: null, creator_token_at: null, mod_issues_seen_at: null,
   created_at: t0, updated_at: t0, last_login_at: ago(10)},
  {_id: S2, username: "lurker", discord_handle: "", is_active: true, is_verified: true, discord_id: NumberInt(0),
   token_version: NumberLong(2), claimed_trove_name: "lurk", claimed_trove_display: "", claimed_at: t0,
   claim_verified: true, claim_baseline: {}, claim_verified_at: t0, created_at: new Date("2026-01-03T00:00:00Z"),
   updated_at: t0},
  {_id: S3, username: "quiet", discord_handle: "Quiet", is_active: true, is_verified: true, discord_id: null,
   token_version: NumberInt(0), claim_baseline: {}, created_at: new Date("2026-01-04T00:00:00Z"), updated_at: t0,
   last_login_at: null},
]);
db.site_sessions.deleteMany({site_user_id: {$in: [S1, S2, S3]}});
db.site_sessions.insertMany([
  {site_user_id: S1, refresh_token_hash: "admin-parity-sr1", device: null, created_at: t0, last_used_at: t0,
   expires_at: new Date(now + 86400000), revoked: false},
  {site_user_id: S2, refresh_token_hash: "admin-parity-sr2", device: null, created_at: t0, last_used_at: t0,
   expires_at: new Date(now + 86400000), revoked: false},
]);
db.username_change_requests.deleteMany({site_user_id: {$in: [S1, S2, S3]}});
db.username_change_requests.insertMany([
  {_id: ObjectId("65ad00000000000000000d01"), site_user_id: S3, current_username: "quiet",
   requested_username: "quiet_one", status: "pending", reason: "", resolved_by: null, resolved_at: null, created_at: ago(60)},
  {_id: ObjectId("65ad00000000000000000d02"), site_user_id: S2, current_username: "lurker",
   requested_username: "lurk.er", status: "pending", reason: "", resolved_by: null, resolved_at: null, created_at: ago(30)},
  {_id: ObjectId("65ad00000000000000000d03"), site_user_id: S1, current_username: "kiwi_fan",
   requested_username: "sir_kiwi", status: "pending", reason: "", resolved_by: null, resolved_at: null, created_at: ago(20)},
]);
db.mod_projects.deleteMany({_id: {$in: [ObjectId("65ad00000000000000000f01"), ObjectId("65ad00000000000000000f02")]}});
db.mod_projects.insertMany([
  {_id: ObjectId("65ad00000000000000000f01"), owner_id: S3, owner_handle: "quiet", slug: "admin-parity-a"},
  {_id: ObjectId("65ad00000000000000000f02"), owner_id: S1, owner_handle: "kiwi_fan", slug: "admin-parity-b"},
]);
db.modpack_projects.deleteMany({_id: ObjectId("65ad00000000000000000f03")});
db.modpack_projects.insertOne({_id: ObjectId("65ad00000000000000000f03"), owner_id: S3, owner_handle: "quiet", slug: "admin-parity-p"});

db.runtime_config.deleteMany({key: {$in: ["embed_rate_limit_max", "cheaters_z_threshold"]}});
db.page_views.deleteMany({visitor_hash: /^admin-parity/});
db.page_views.insertMany([
  {route: "/", path: "/", visitor_hash: "admin-parity-v1", pid: "admin-parity-p1", engaged_s: NumberInt(40), channel: "search",
   source: "Google", device: "desktop", browser: "Firefox", os: "Linux", lang: "en",
   perf: {ttfb: NumberInt(120), fcp: NumberInt(800), lcp: NumberInt(1500), cls: NumberInt(20)}, created_at: ago(300), last_at: ago(299)},
  {route: "/mods/{owner}/{slug}", path: "/mods/quiet/admin-parity-a", visitor_hash: "admin-parity-v1", pid: "admin-parity-p2",
   engaged_s: NumberInt(5), channel: "internal", device: "desktop", browser: "Firefox", os: "Linux", lang: "en",
   created_at: ago(295), last_at: ago(294)},
  {route: "/", path: "/", visitor_hash: "admin-parity-v2", pid: "admin-parity-p3", engaged_s: NumberInt(0), channel: "direct",
   device: "mobile", browser: "Safari", os: "iOS", lang: "fr", perf: {fcp: NumberInt(4200)}, created_at: ago(2), last_at: ago(1)},
]);
db.bot_presence.deleteMany({_id: "current"});
db.bot_presence.insertOne({_id: "current", guild_count: NumberInt(12), member_count: NumberLong(3456), updated_at: t0});
db.bot_command_usage.deleteMany({_id: {$in: ["status", "gem", "ping"]}});
db.bot_command_usage.insertMany([
  {_id: "status", count: NumberInt(40), last_used_at: t0},
  {_id: "gem", count: 7.9},
  {_id: "ping", count: NumberLong(2), last_used_at: null},
]);
db.decoder_runs.deleteMany({trigger: "admin-parity"});
db.decoder_runs.insertMany([
  {name: "fish", trigger: "admin-parity", ordinal: NumberInt(9), version_tag: null, code_version: "x",
   started_at: new Date(now + 3600000), finished_at: new Date(now + 3600000), ok: false, entries: null, error: "boom", traceback: null},
]);

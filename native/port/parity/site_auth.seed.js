// Site accounts twin seed: five Discord accounts (A..E, E a legacy row missing
// newer fields), their sessions (refresh tokens "parity-refresh-<x>"), and the
// content C owns or touches so self-delete has something to anonymize.
const T0 = ISODate("2026-01-01T00:00:00Z");
const T1 = ISODate("2026-01-02T03:04:05.678Z");
const FAR = ISODate("2099-01-01T00:00:00Z");
const A = ObjectId("65b000000000000000000001");
const B = ObjectId("65b000000000000000000002");
const C = ObjectId("65b000000000000000000003");
const D = ObjectId("65b000000000000000000004");
const E = ObjectId("65b000000000000000000005");
const P1 = ObjectId("65b000000000000000000101");
const P2 = ObjectId("65b000000000000000000102");
const K1 = ObjectId("65b000000000000000000201");
const K2 = ObjectId("65b000000000000000000202");
const G1 = ObjectId("65b000000000000000000501");

function user(id, username, extra) {
  return Object.assign({
    _id: id, username: username, discord_handle: "", notify_email: null, display_name: null,
    is_active: true, is_verified: true, is_deleted: false, discord_id: null, discord_avatar: null,
    token_version: NumberInt(0), claimed_trove_name: null, claimed_trove_display: null, claimed_at: null,
    claim_verified: false, claim_baseline: {}, claim_verified_at: null, creator_token_hash: null,
    creator_token_prefix: null, creator_token_at: null, mod_issues_seen_at: null,
    created_at: T0, updated_at: T0, last_login_at: null,
  }, extra);
}

db.site_users.insertMany([
  user(A, "alpha_user", {discord_handle: "Alpha.Live", display_name: "Alpha", discord_id: NumberLong("46137344"),
    discord_avatar: "a_hash1", last_login_at: T1}),
  user(B, "bravo_user", {discord_id: NumberLong("123456789012345678"), discord_avatar: "plainhash",
    claimed_trove_name: "bravo", claimed_trove_display: "Bravo", claimed_at: T1, claim_baseline: {"1": 999.0}}),
  user(C, "charlie_user", {discord_handle: "Charlie", notify_email: "c@example.com",
    discord_id: NumberLong("987654321098765432")}),
  user(D, "delta_user", {is_verified: false, discord_id: NumberLong("5")}),
  {_id: E, username: "echo_user", is_active: true, is_verified: true, discord_id: NumberLong("4194305"),
    token_version: NumberLong(0), claim_baseline: {"20": NumberInt(7)}, created_at: T0, updated_at: T0},
]);

function session(id, owner, hash, device, extra) {
  return Object.assign({_id: ObjectId(id), site_user_id: owner, refresh_token_hash: hash, device: device,
    created_at: T0, last_used_at: T1, expires_at: FAR, revoked: false}, extra);
}

db.site_sessions.insertMany([
  session("65b0000000000000000000a0", A, "revoked-a0", null, {revoked: true}),
  session("65b0000000000000000000a1", A, "5193082d84f29b314be653500e36ff9e2a04e0ed67c3da779c14a82f08ea030f", "Chrome on Windows"),
  session("65b0000000000000000000a2", A, "3a793532e2028d235ca31e13506b2495c0f461c4ed252a5265b1ca27e7ee007c", null),
  session("65b0000000000000000000a3", A, "expired-a3", "Firefox", {expires_at: T1}),
  session("65b0000000000000000000b1", B, "273608efccce4f5ffe4e11be96b110edd4fec0767f86c7b5cebc1853c605af97", "Safari on iOS"),
  session("65b0000000000000000000c1", C, "8c00dabd90b1f9dfb4dda13f436b03276c91aec11392c8aa62e8dca180102321", "Edge on Windows"),
  session("65b0000000000000000000d1", D, "189734fbb2c9c483b1f343e491e5f419eac2e1cbc77b2929c814ae86f82014ff", null),
  session("65b0000000000000000000e1", E, "6e7a92a4fbc582f59f75b7c7670bbafa7d250a55af7791c9601e20efb23f8c2e", null),
]);

db.username_change_requests.insertMany([
  {_id: ObjectId("65b0000000000000000000f1"), site_user_id: C, current_username: "charlie_user",
    requested_username: "charlie.new", status: "pending", reason: "", resolved_by: null, resolved_at: null, created_at: T0},
  {_id: ObjectId("65b0000000000000000000f2"), site_user_id: A, current_username: "alpha_user",
    requested_username: "alpha_old", status: "rejected", reason: "nope", resolved_by: null, resolved_at: T1,
    created_at: ISODate("2025-12-01T00:00:00Z")},
]);

db.mod_projects.insertMany([
  {_id: P1, slug: "c-mod", title: "C Mod", owner_id: C, owner_username: "Charlie", owner_handle: "charlie_user",
    collaborators: [], visibility: "public", star_count: NumberInt(0), created_at: T0, updated_at: T0},
  {_id: P2, slug: "b-mod", title: "B Mod", owner_id: B, owner_username: "bravo_user", owner_handle: "bravo_user",
    collaborators: [{user_id: C, username: "charlie_user"}, {user_id: A, username: "alpha_user", extra: 1}],
    visibility: "unlisted", star_count: NumberInt(2), created_at: T0, updated_at: T0},
]);
db.modpack_projects.insertMany([
  {_id: K1, slug: "c-pack", title: "C Pack", owner_id: C, owner_username: "Charlie", owner_handle: "charlie_user",
    collaborators: [], visibility: "public", star_count: NumberInt(0), created_at: T0, updated_at: T0},
  {_id: K2, slug: "a-pack", title: "A Pack", owner_id: A, owner_username: "alpha_user", owner_handle: "alpha_user",
    collaborators: [{user_id: C, username: "charlie_user"}], star_count: NumberInt(1), created_at: T0, updated_at: T0},
]);
db.mod_stars.insertMany([
  {_id: ObjectId("65b000000000000000000301"), project_id: P2, site_user_id: C, created_at: T1},
  {_id: ObjectId("65b000000000000000000302"), project_id: P2, site_user_id: A, created_at: T1},
]);
db.modpack_stars.insertOne({_id: ObjectId("65b000000000000000000401"), modpack_id: K2, site_user_id: C, created_at: T1});
db.giveaways.insertOne({_id: G1, title: "G", description: null, prize_name: "Prize", starts_at: T0, ends_at: T1,
  status: "ended", vault_item_id: null, prize_code_id: null, winner_user_id: C, winner_username: "charlie_user",
  drawn_at: T1, entry_count: NumberInt(2), created_at: T0, updated_at: T1});
db.giveaway_entries.insertMany([
  {_id: ObjectId("65b000000000000000000601"), giveaway_id: G1, site_user_id: C, username: "charlie_user", entered_at: T1},
  {_id: ObjectId("65b000000000000000000602"), giveaway_id: G1, site_user_id: A, username: "alpha_user", entered_at: T1},
]);
db.mod_profiles.insertOne({_id: ObjectId("65b000000000000000000701"), site_user_id: C, handle: "charlie_user",
  display_name: "Charlie", tagline: "hi é", readme: "about", created_at: T0, updated_at: T0});
db.mod_git_tokens.insertOne({_id: ObjectId("65b000000000000000000801"), site_user_id: C, token_hash: "h1",
  prefix: "kiwi_git_ab", name: "laptop", revoked: false, created_at: T0, last_used_at: null});
db.mod_claim_requests.insertOne({_id: ObjectId("65b000000000000000000901"), project_id: P2, project_slug: "b-mod",
  project_title: "B Mod", claimant_id: C, claimant_username: "charlie_user", message: "", status: "pending",
  resolved_by: null, resolved_at: null, created_at: T0});
db.dm_subscriptions.insertOne({_id: ObjectId("65b000000000000000000a01"), owner_id: C, owner_discord_id: NumberLong("987654321098765432"),
  label: "my dms", events: ["challenge"], created_at: T0, updated_at: T0});
db.site_webhooks.insertOne({_id: ObjectId("65b000000000000000000b01"), owner_id: C, label: "hook",
  url: "https://discord.com/api/webhooks/1/x", events: [], created_at: T0, updated_at: T0});
db.image_designs.insertOne({_id: ObjectId("65b000000000000000000c01"), owner_id: C, name: "card", created_at: T0, updated_at: T0});
db.email_outbox.insertMany([
  {_id: ObjectId("65b000000000000000000d01"), to: "c@example.com", subject: "s", text: "t", html: null,
    status: "pending", attempts: NumberInt(0), last_error: null, next_attempt_at: FAR, claimed_at: null,
    created_at: T0, sent_at: null},
  {_id: ObjectId("65b000000000000000000d02"), to: "other@example.com", subject: "s", text: "t", html: null,
    status: "pending", attempts: NumberInt(0), last_error: null, next_attempt_at: FAR, claimed_at: null,
    created_at: T0, sent_at: null},
]);

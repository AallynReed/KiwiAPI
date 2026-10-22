// Dev Portal accounts for portal.writes.json; every password is Sandbox-pass-123.
const HASH = "$argon2id$v=19$m=65536,t=3,p=4$jCSKK8dWNaIEd2tvu9q62g$lZ2Uv9b2kcfSApIvkK2BjPB6cPTRFdpwLZDhHdGzsVw";
const T = ISODate("2026-01-02T03:04:05.678Z");
function user(id, email, extra) {
  return Object.assign({
    _id: ObjectId(id), email: email, hashed_password: HASH, display_name: "Seed", is_active: true,
    is_verified: true, is_superuser: false, roles: [], token_version: NumberInt(0), github_id: null,
    email_bounced: false, created_at: T, updated_at: T, last_login_at: null,
  }, extra || {});
}
db.users.insertMany([
  user("6a0000000000000000000001", "seed@example.com"),
  user("6a0000000000000000000002", "taken@example.com"),
  user("6a0000000000000000000003", "lock@example.com"),
  user("6a0000000000000000000004", "gone@example.com", {github_id: NumberInt(77)}),
  user("6a0000000000000000000005", "legacy@example.com", {display_name: null}),
]);
db.users.updateOne({_id: ObjectId("6a0000000000000000000005")}, {$unset: {email_bounced: "", github_id: ""}});
db.api_tokens.insertOne({
  _id: ObjectId("6a00000000000000000000a1"), user_id: ObjectId("6a0000000000000000000004"), name: "old",
  prefix: "kiwi_oldoldol", hashed_token: "0".repeat(64), scopes: NumberInt(0), ip_salt: null,
  allowed_ip_hashes: [], revoked: false, revoked_at: null, revoke_reason: null, expires_at: null,
  expiry_warned: false, created_at: T, last_used_at: null, rotated_at: null, request_count: NumberInt(3),
});
db.usage_events.insertMany([
  {user_id: ObjectId("6a0000000000000000000004"), token_id: ObjectId("6a00000000000000000000a1"), method: "GET",
   route: "/v1/x", path: "/v1/x", status_code: NumberInt(200), duration_ms: 1.5, created_at: new Date()},
  {user_id: ObjectId("6a0000000000000000000001"), token_id: ObjectId("6a00000000000000000000a1"), method: "GET",
   route: "/v1/y/{id}", path: "/v1/y/2", status_code: NumberInt(429), duration_ms: NumberInt(4), created_at: new Date()},
]);

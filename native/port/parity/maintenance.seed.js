// Tokens the daily sweep revokes, warns about or leaves alone, and 429s for the digest.
const admin = db.users.findOne({is_superuser: true});
const day = 86400000, now = Date.now();
const tok = (name, extra) => Object.assign({
  user_id: admin._id, name: name, prefix: "kiwi_" + name, hashed_token: "h_" + name, scopes: 0,
  ip_salt: "c2FsdA", allowed_ip_hashes: [], revoked: false, revoked_at: null, revoke_reason: null,
  expires_at: null, expiry_warned: false, created_at: new Date(now - 30 * day), last_used_at: null,
  rotated_at: null, request_count: 0,
}, extra);
db.api_tokens.insertMany([
  tok("stale_used", {last_used_at: new Date(now - 400 * day), created_at: new Date(now - 500 * day)}),
  tok("stale_never", {created_at: new Date(now - 400 * day)}),
  tok("fresh", {last_used_at: new Date(now - day)}),
  tok("expiring", {last_used_at: new Date(now - day), expires_at: new Date(now + 2 * day)}),
  tok("warned", {last_used_at: new Date(now - day), expires_at: new Date(now + 3 * day), expiry_warned: true}),
  tok("revoked", {revoked: true, revoked_at: new Date(now - 10 * day), revoke_reason: "manual", created_at: new Date(now - 400 * day)}),
]);
const t = db.api_tokens.findOne({name: "fresh"});
const ev = [];
for (let i = 0; i < 25; i++) {
  ev.push({user_id: admin._id, token_id: t._id, method: "GET", route: "/v1/x", path: "/v1/x",
    status_code: 429, duration_ms: 1.0, created_at: new Date(now - (i + 1) * 60000)});
}
db.usage_events.insertMany(ev);

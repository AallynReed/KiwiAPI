// The two site users discord.writes.json signs in as (its fixed site JWTs), so
// the big guild id specs reach the dashboard's handlers.
const T0 = ISODate("2026-09-01T12:00:00.250Z");
const user = (_id, username, discord_id) => ({
  _id, username, discord_handle: username, notify_email: null, display_name: null, is_active: true,
  is_verified: true, is_deleted: false, discord_id, discord_avatar: null, token_version: NumberInt(0),
  claimed_trove_name: null, claimed_trove_display: null, claimed_at: null, claim_verified: false,
  claim_baseline: {}, created_at: T0, updated_at: T0,
});
db.site_users.insertMany([user(ObjectId("65f0000000000000000000a1"), "paritylinked", NumberLong("312345678901234567")),
                          user(ObjectId("65f0000000000000000000a2"), "parityunlinked", null)]);

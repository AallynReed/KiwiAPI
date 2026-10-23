// Two site users (one linked to Discord), a French guild config and a club,
// shaped as Beanie writes them. Specs sign in with fixed site JWTs minted for
// these ids against the sandbox SECRET_KEY.
const T0 = ISODate("2026-09-01T12:00:00.250Z");
const LINKED = ObjectId("65f0000000000000000000a1"), UNLINKED = ObjectId("65f0000000000000000000a2");
const user = (_id, username, discord_id) => ({
  _id, username, discord_handle: username, notify_email: null, display_name: null, is_active: true,
  is_verified: true, is_deleted: false, discord_id, discord_avatar: null, token_version: NumberInt(0),
  claimed_trove_name: null, claimed_trove_display: null, claimed_at: null, claim_verified: false,
  claim_baseline: {}, created_at: T0, updated_at: T0,
});
db.site_users.insertMany([user(LINKED, "paritylinked", NumberLong("312345678901234567")),
                          user(UNLINKED, "parityunlinked", null)]);
db.guild_configs.insertOne({
  _id: ObjectId("65d000000000000000000001"), guild_id: NumberLong("4242424242424242"), announcements: {},
  live_board: {enabled: false, channel_id: null, message_id: null, channel_missing: false},
  market_watch: {enabled: false, channel_id: null, ping_role_ids: [], channel_missing: false, items: []},
  language: "fr", config_perms: {}, hourly_challenge_enabled: false, announce_channel_id: null,
  last_announced_challenge_anchor: null, updated_by: null, created_at: T0, updated_at: T0,
});
db.clubs.insertOne({
  _id: ObjectId("65e000000000000000000001"), guild_id: NumberLong("4242424242424242"), name: "Alpha",
  public: false, description: null, banner_url: null, avatar_url: null, discord_url: null, website_url: null,
  role_links: {}, updated_by: null, created_at: T0, updated_at: T0,
});

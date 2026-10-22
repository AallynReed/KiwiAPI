// Two site users (one linked to Discord) and their webhooks / DM subscriptions,
// shaped as Beanie writes them. The specs sign in with fixed site JWTs minted
// for these ids against the sandbox SECRET_KEY.
const T0 = ISODate("2026-09-01T12:00:00.250Z"), T1 = ISODate("2026-09-02T08:15:00Z");
const LINKED = ObjectId("65f0000000000000000000a1"), UNLINKED = ObjectId("65f0000000000000000000a2");
const user = (_id, username, discord_id) => ({
  _id, username, discord_handle: username, notify_email: null, display_name: null, is_active: true,
  is_verified: true, is_deleted: false, discord_id, discord_avatar: null, token_version: NumberInt(0),
  claimed_trove_name: null, claimed_trove_display: null, claimed_at: null, claim_verified: false,
  claim_baseline: {}, created_at: T0, updated_at: T0,
});
db.site_users.insertMany([user(LINKED, "paritylinked", NumberLong("312345678901234567")),
                          user(UNLINKED, "parityunlinked", null)]);

const health = (o) => Object.assign({active: true, consecutive_failures: NumberInt(0), last_status: null,
  last_error: null, last_delivered_at: null, disabled_reason: null}, o);
const tmpl = (o) => Object.assign({enabled: false, content: null, title: null, url: null, description: null,
  color: null, fields: [], footer: null, show_image: true, image_design_id: null, image_url: null}, o);
db.site_webhooks.insertMany([
  Object.assign(health({}), {_id: ObjectId("65b000000000000000000001"), owner_id: LINKED, label: "Main",
    url: "https://discord.com/api/webhooks/1234567890/AbCdEfGhIjKlMnOp_token-value",
    events: ["challenge", "game_update"],
    templates: {challenge: tmpl({enabled: true, title: "Hi {name}",
      fields: [{name: "A", value: "{type_label}", inline: true}]})},
    created_at: T0, updated_at: T0}),
  Object.assign(health({active: false, consecutive_failures: NumberInt(12), last_status: NumberInt(500),
    last_error: "HTTP 500", last_delivered_at: T0, disabled_reason: "Auto-disabled after 12 failed deliveries."}),
    {_id: ObjectId("65b000000000000000000002"), owner_id: LINKED, label: "Old",
     url: "https://ptb.discord.com/api/v10/webhooks/42/short", events: ["mod_release"], templates: {},
     created_at: T1, updated_at: T1}),
  Object.assign(health({}), {_id: ObjectId("65b000000000000000000003"), owner_id: UNLINKED, label: "Theirs",
    url: "https://discord.com/api/webhooks/77/zzzzzzzzzz", events: ["challenge"], templates: {},
    created_at: T0, updated_at: T0}),
]);

db.dm_subscriptions.insertMany([
  Object.assign(health({}), {_id: ObjectId("65c000000000000000000001"), owner_id: LINKED,
    owner_discord_id: NumberLong("312345678901234567"), label: "Alerts", events: ["challenge", "market_watch"],
    filters: {challenge_types: ["racing"], watch: [{name: "Dragon Egg", max_price_each: new Double(50000)}]},
    watch_state: {}, created_at: T0, updated_at: T0}),
  Object.assign(health({active: false, consecutive_failures: NumberInt(8),
    disabled_reason: "Auto-disabled after 8 failed DMs."}),
    {_id: ObjectId("65c000000000000000000002"), owner_id: LINKED, owner_discord_id: NumberLong("312345678901234567"),
     label: "Updates", events: ["game_update"], filters: {}, watch_state: {"Dragon Egg": NumberInt(1790000000)},
     created_at: T1, updated_at: T1}),
]);

// A Discord-linked site user (the webhooks seed's, signed in with the same fixed
// sandbox JWT) owning one design, and a design owned by someone else, shaped as
// Beanie writes them.
const T0 = ISODate("2026-09-01T12:00:00.250Z");
const LINKED = ObjectId("65f0000000000000000000a1"), STRANGER = ObjectId("65f0000000000000000000a9");
db.site_users.insertOne({
  _id: LINKED, username: "paritylinked", discord_handle: "paritylinked", notify_email: null, display_name: null,
  is_active: true, is_verified: true, is_deleted: false, discord_id: NumberLong("312345678901234567"),
  discord_avatar: null, token_version: NumberInt(0), claimed_trove_name: null, claimed_trove_display: null,
  claimed_at: null, claim_verified: false, claim_baseline: {}, created_at: T0, updated_at: T0,
});
const layer = (o) => Object.assign({type: "text", x: 0.0, y: 0.0, opacity: 1.0, text: "", font_size: NumberInt(32),
  color: "#ffffff", bold: false, align: "left", max_width: null, w: 120.0, h: 48.0, radius: NumberInt(0),
  image_sha: null}, o);
const bg = (o) => Object.assign({type: "solid", color1: "#0d1117", color2: "#16213e", angle: NumberInt(90),
  image_sha: null, fit: "cover"}, o);
db.image_designs.insertMany([
  {_id: ObjectId("65e000000000000000000001"), owner_id: LINKED, name: "Seeded", width: NumberInt(400),
   height: NumberInt(150), background: bg({type: "gradient", angle: NumberInt(30)}),
   layers: [layer({text: "Challenge: {name} {ends_at:R}", x: 10.0, y: 8.0, font_size: NumberInt(22)}),
            layer({type: "rect", x: 10.0, y: 60.0, w: 180.5, h: 50.0, radius: NumberInt(12), color: "#44cc88",
                   opacity: 0.8}),
            layer({text: "{overall_dot} {overall_text}", x: 390.0, y: 120.0, align: "right", font_size: NumberInt(14)})],
   bind_type: "challenge", created_at: T0, updated_at: T0},
  {_id: ObjectId("65e000000000000000000002"), owner_id: STRANGER, name: "Not yours", width: NumberInt(120),
   height: NumberInt(60), background: bg({}), layers: [layer({text: "hi"})], bind_type: null,
   created_at: T0, updated_at: T0},
]);

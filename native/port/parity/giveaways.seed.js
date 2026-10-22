// Giveaways twin seed: fixed ids, dates relative to seeding (copied to B by mongodump).
const NOW = Date.now(), DAY = 86400000;
const at = (d) => new Date(NOW + d * DAY);
const oid = (s) => ObjectId(s);
const ALICE = oid("65e100000000000000000a01"), BOB = oid("65e100000000000000000a02"), CAROL = oid("65e100000000000000000a03");
const V1 = oid("65e200000000000000000001"), V2 = oid("65e200000000000000000002"), V3 = oid("65e200000000000000000003");
const G = (n) => oid("65e3000000000000000000" + n);
const C = (n) => oid("65e4000000000000000000" + n);

const user = (_id, username, display_name, notify_email) => ({
  _id, username, discord_handle: username, notify_email, display_name, is_active: true, is_verified: true,
  is_deleted: false, discord_id: null, discord_avatar: null, token_version: NumberInt(0),
  created_at: at(-30), updated_at: at(-30), last_login_at: null,
});
db.site_users.insertMany([
  user(ALICE, "alice", "Alice", "alice@example.com"),
  user(BOB, "bob", null, null),
  user(CAROL, "carol", "", "carol@example.com"),
]);

db.vault_items.insertMany([
  {_id: V1, name: "Radiant Mount", description: "A shiny mount", created_at: at(-20), updated_at: at(-20)},
  {_id: V2, name: "Dragon Egg", description: null, created_at: at(-19), updated_at: at(-19)},
  {_id: V3, name: "Empty Drawer", description: null, created_at: at(-18), updated_at: at(-18)},
]);

let n = 0;
const code = (_id, vault_item_id, c, status, giveaway_id, awarded_to, awarded_at) => ({
  _id, vault_item_id, code: c, status, giveaway_id, awarded_to, awarded_at, created_at: at(-17 + (n++) / 100),
});
db.prize_codes.insertMany([
  code(C("01"), V1, "RAD-0001", "reserved", G("01"), null, null),
  code(C("02"), V1, "RAD-0002", "reserved", G("02"), null, null),
  code(C("03"), V1, "RAD-0003", "reserved", G("03"), null, null),
  code(C("04"), V1, "RAD-0004", "awarded", G("05"), CAROL, at(-3)),
  code(C("05"), V1, "RAD-0005", "available", null, null, null),
  code(C("06"), V1, "RAD-0006", "available", null, null, null),
  code(C("11"), V2, "EGG-0001", "reserved", G("04"), null, null),
  code(C("12"), V2, "EGG-0002", "reserved", G("07"), null, null),
  code(C("13"), V2, "EGG-0003", "available", null, null, null),
]);

const gw = (_id, title, description, prize_name, starts, ends, status, vault_item_id, prize_code_id, created, extra = {}) =>
  Object.assign({
    _id, title, description, prize_name, starts_at: at(starts), ends_at: at(ends), status, vault_item_id, prize_code_id,
    winner_user_id: null, winner_username: null, drawn_at: null, entry_count: NumberInt(0),
    created_at: at(created), updated_at: at(created),
  }, extra);
db.giveaways.insertMany([
  gw(G("01"), "Weekly Radiant", "A shiny mount", "Radiant Mount", -1, 5, "open", V1, C("01"), -10),
  gw(G("02"), "Solo Draw", null, "Radiant Mount", -1, 6, "open", V1, C("02"), -9),
  gw(G("03"), "Nobody Came", "", "Radiant Mount", -1, 4, "open", V1, C("03"), -8),
  gw(G("04"), "Future Egg", null, "Dragon Egg", 2, 9, "scheduled", V2, C("11"), -7),
  gw(G("05"), "Last Week", "Won already", "Radiant Mount", -8, -3, "drawn", V1, C("04"), -12,
     {winner_user_id: CAROL, winner_username: "carol", drawn_at: at(-3), entry_count: NumberInt(1)}),
  gw(G("06"), "Quiet One", null, "Dragon Egg", -15, -10, "closed", null, null, -16),
  gw(G("07"), "Cancel Me", null, "Dragon Egg", -1, 3, "open", V2, C("12"), -6),
]);

db.giveaway_entries.insertOne({_id: oid("65e500000000000000000001"), giveaway_id: G("05"), site_user_id: CAROL,
  username: "carol", entered_at: at(-5)});

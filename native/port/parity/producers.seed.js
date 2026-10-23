// Event producers twin seed: the market interest list, and giveaways for the worker
// (one to open, one to draw with a single entrant, one to close with none).
const NOW = Date.now(), DAY = 86400000;
const at = (d) => new Date(NOW + d * DAY);
const oid = (s) => ObjectId(s);

db.market_interest_items.insertMany([
  {name: "Glim", added_by: null, added_at: ISODate("2026-01-01T00:00:00Z")},
  {name: "Plasmium", added_by: null, added_at: ISODate("2026-01-01T00:00:00Z")},
]);

const DAVE = oid("65e100000000000000000b01");
db.site_users.insertOne({_id: DAVE, username: "dave", discord_handle: "dave", notify_email: "dave@example.com",
  display_name: "Dave", is_active: true, is_verified: true, is_deleted: false, discord_id: null, discord_avatar: null,
  token_version: NumberInt(0), created_at: at(-30), updated_at: at(-30), last_login_at: null});

const V = oid("65e200000000000000000b01");
db.vault_items.insertOne({_id: V, name: "Parity Mount", description: null, created_at: at(-20), updated_at: at(-20)});

const C = (n) => oid("65e4000000000000000b00" + n);
const G = (n) => oid("65e3000000000000000b00" + n);
const code = (_id, c, giveaway_id, i) => ({_id, vault_item_id: V, code: c, status: "reserved", giveaway_id,
  awarded_to: null, awarded_at: null, created_at: at(-17 + i / 100)});
db.prize_codes.insertMany([code(C("01"), "PAR-0001", G("01"), 1), code(C("02"), "PAR-0002", G("02"), 2),
  code(C("03"), "PAR-0003", G("03"), 3)]);

const gw = (_id, title, starts, ends, status, prize_code_id, created) => ({
  _id, title, description: null, prize_name: "Parity Mount", starts_at: at(starts), ends_at: at(ends), status,
  vault_item_id: V, prize_code_id, winner_user_id: null, winner_username: null, drawn_at: null,
  entry_count: NumberInt(0), created_at: at(created), updated_at: at(created)});
db.giveaways.insertMany([
  gw(G("01"), "Starts Now", -0.01, 3, "scheduled", C("01"), -2),
  gw(G("02"), "Draw Me", -3, -0.01, "open", C("02"), -4),
  gw(G("03"), "Empty Room", -3, -0.01, "open", C("03"), -5),
]);
db.giveaway_entries.insertOne({_id: oid("65e500000000000000000b01"), giveaway_id: G("02"), site_user_id: DAVE,
  username: "dave", entered_at: at(-1)});

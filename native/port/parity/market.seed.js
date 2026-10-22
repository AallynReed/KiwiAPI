// Interest list and sidebar categories the market write specs edit (fixed ids, Beanie's field order and int32 orders).
const at = new Date("2026-01-02T03:04:05.123Z");
db.market_interest_items.deleteMany({});
db.market_interest_items.insertMany([
  {name: "Glim", added_by: null, added_at: at},
  {name: "Plasmium", added_by: null, added_at: at},
  {name: "Shapestone Ore", added_by: null, added_at: at},
  {name: "Eye of Q'bthulhu", added_by: null, added_at: at},
]);
db.market_item_categories.deleteMany({});
db.market_item_categories.insertMany([
  {_id: ObjectId("65a000000000000000000c01"), name: "Materials", order: NumberInt(0), items: ["Glim", "Plasmium", "Glim"], created_by: null, created_at: at},
  {_id: ObjectId("65a000000000000000000c02"), name: "Ores", order: NumberInt(1), items: ["Shapestone Ore", "Gone Item"], created_by: null, created_at: at},
  {_id: ObjectId("65a000000000000000000c03"), name: "alpha spare", order: NumberInt(1), items: [], created_by: null, created_at: at},
]);

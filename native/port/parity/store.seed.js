// A small store catalog shaped exactly as Beanie writes it (doubles for costs, int32 stamps).
const A = 1792665676, DAY = 86400;
const price = (currency, cost, can_purchase = true, monthly = 0, sale = "") =>
  ({currency, cost: new Double(cost), can_purchase, monthly, sale});
const tex = (texture, x = 0, y = 0, text = "", overlay = false) => ({texture, x, y, text, overlay});
const product = (o) => Object.assign({
  code: "", kind: "product", name: "", image: "", info: "", informational: false, tradable: false,
  prices: [], price_string: "", price_string_currency: "", price_string_sale: "", promo: "",
  deal_expires_at: null, interact_label: "", interact_enabled: false, trial_limits: "",
  class_level: null, class_power_rank: null, class_shield_frame: null, class_sub_name: "", class_icon: "",
  textures: [], loot_title: "", loot_body: "", categories: [], first_seen: 0, last_seen: 0,
  price_history: [], availability: [],
}, o);

db.store_state.insertOne({key: "state", last_anchor: A, title: "$StoreTitle", product_count: 5, category_count: 3});
db.store_products.insertMany([
  product({code: "TROVE_SOLARION_SPARK_PACK", kind: "starter", name: "Solarion Spark Pack",
    image: "ui/store/ui_store_boat_dragon.dds", prices: [price("EUR", 499, true, 0, "Sale_50")],
    promo: "$Promo_Hot", deal_expires_at: A + 2 * DAY, categories: [0, 1],
    textures: [tex("ui/store/ui_store_boat_dragon.dds"), tex("ui/store/sales_tag_burst.dds", 4, -2, "50%", true)],
    first_seen: A - 30 * DAY, last_seen: A,
    price_history: [{ts: A - 30 * DAY, prices: [price("EUR", 999)], price_string: ""},
                    {ts: A - 3 * DAY, prices: [price("EUR", 499, true, 0, "Sale_50")], price_string: ""}],
    availability: [[A - 30 * DAY, A - 20 * DAY], [A - 3 * DAY, A]]}),
  product({code: "PLAYERCLASS_KNIGHT", kind: "product", name: "Knight", prices: [price("TWC", 650, false)],
    class_level: 1, class_power_rank: 2, class_shield_frame: 3, class_sub_name: "$Sub", class_icon: "ui/class/knight.dds",
    categories: [4], first_seen: A - 60 * DAY, last_seen: A, availability: [[A - 60 * DAY, A]],
    price_history: [{ts: A - 60 * DAY, prices: [price("TWC", 650, false)], price_string: ""}]}),
  product({code: "TROVE_PATRON_15_CREDITS_NOTRADE", kind: "patron", name: "Patron 15",
    prices: [price("TWC", 1500, true, 1500), price("TWP", 0.1 + 0.2)], categories: [2],
    first_seen: A - 10 * DAY, last_seen: A, availability: [[A - 10 * DAY, A]]}),
  product({code: "BOMBER_ROYALE_STYLES_PACK", name: "Bomber Royale Styles", prices: [price("TWC", 1e16)],
    loot_title: "$Loot", loot_body: "Rare 5%\nCommon 95%", categories: [0],
    first_seen: A - 90 * DAY, last_seen: A - 40 * DAY,
    availability: [[A - 90 * DAY, A - 90 * DAY + DAY / 2], [A - 50 * DAY, A - 40 * DAY]]}),
  {code: "LEGACY_ONLY_CODE", kind: "trial", name: "Légacy ✓", last_seen: A - 200 * DAY},
]);
db.store_categories.insertMany([
  {index: 0, label: "$StoreCategory_Deals", icon: "", codes: ["TROVE_SOLARION_SPARK_PACK", "BOMBER_ROYALE_STYLES_PACK"], last_seen: A},
  {index: 1, label: "$StoreCategory_New", icon: "ui/icon/new.dds", codes: ["TROVE_SOLARION_SPARK_PACK"], last_seen: A},
  {index: 2, label: "$StoreCategory_Patron", icon: "", codes: [], last_seen: A - DAY},
]);
db.update_state.insertMany([
  {branch: "live-us", path: "ui/store/ui_store_boat_dragon.dds", content_sha256: "a".repeat(64), fnv_hash: null,
   size: 4, archive: null, archive_index: null, last_ordinal: 3},
  {branch: "live-us", path: "ui/store/Mixed_Case.PNG", content_sha256: "b".repeat(64), fnv_hash: null,
   size: 4, archive: null, archive_index: null, last_ordinal: 3},
]);
db.update_changes.insertMany([
  {branch: "live-us", ordinal: 2, path: "ui/store/gone.dds", type: "added", content_sha256: "c".repeat(64), fnv_hash: null, size: 1},
  {branch: "live-us", ordinal: 4, path: "ui/store/gone.dds", type: "removed", content_sha256: null, fnv_hash: null, size: 0},
]);

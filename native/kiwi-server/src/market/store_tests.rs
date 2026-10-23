//! Live-store checks, opt-in because they write: point POSTGRES_DSN and
//! MONGO_URI / MONGO_DB at throwaway stores (postgres:17, mongo:7) and run
//! `cargo test -p kiwi-server market::store_tests -- --ignored`.

use mongodb::bson::{Bson, Document, doc, oid::ObjectId};

use super::{
    admin::{self, UpdateBody},
    reads::{self, DealsQuery},
    store::{self, CATEGORIES, INTEREST, Live},
};
use crate::{config::Settings, state::AppState};

async fn state() -> AppState {
    AppState::new(Settings::from_env().unwrap()).await.unwrap()
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS market_listing (
    id UUID PRIMARY KEY, name TEXT NOT NULL, type TEXT, stack INTEGER NOT NULL, price BIGINT NOT NULL,
    price_each DOUBLE PRECISION NOT NULL, last_seen BIGINT NOT NULL, created_at BIGINT NOT NULL DEFAULT 0);
TRUNCATE market_listing;";

#[tokio::test]
#[ignore = "writes to POSTGRES_DSN"]
async fn postgres_round_trip() {
    let state = state().await;
    let now = chrono::Utc::now().timestamp();
    let client = state.pg().get().await.unwrap();
    client.batch_execute(SCHEMA).await.unwrap();
    let rows = format!(
        "INSERT INTO market_listing VALUES
         ('00000000-0000-1000-8000-0000000000a1', 'Glim', NULL, 1, 9000000000000000000, 100.0, {n}, {n}),
         ('00000000-0000-1000-8000-0000000000a2', 'Glim', 'M', 2, 400, 200.0, {n}, {n}),
         ('00000000-0000-1000-8000-0000000000a3', 'Glim', 'M', 4, 1200, 300.0, {n}, {n}),
         ('00000000-0000-1000-8000-0000000000a4', 'Glim', 'M', 1, 200, 200.0, {n}, {n}),
         ('00000000-0000-1000-8000-0000000000a5', 'Glim', 'M', 1, 200, 200.0, {n}, {n})",
        n = now - 60
    );
    client.batch_execute(&rows).await.unwrap();
    let live = Live(&state);

    let summary = reads::summary_core(&live, "Glim").await.unwrap();
    assert_eq!(summary["total_price"], serde_json::json!(9000000000000002000_i64));
    assert_eq!(summary["median_each"], serde_json::json!(200.0));

    let page = reads::listings_core(
        &live,
        &reads::ListingsQuery::<500> {
            name: None,
            price_min: Some(150.0),
            price_max: None,
            last_seen_after: None,
            hide_expired: true,
            sort: "+price_each".into(),
            limit: 10,
            offset: 0,
        },
    )
    .await
    .unwrap();
    assert_eq!(page["total"], 4);
    assert_eq!(page["items"][3]["id"], "00000000-0000-1000-8000-0000000000a3");

    // `$4` in the deals query is inferred as int4, so asyncpg (and this port)
    // truncate min_discount to 0: every listing at or under the median counts.
    let deals = reads::deals_core(&live, &DealsQuery { days: 7, min_discount: 0.3, limit: 10 }).await.unwrap();
    assert_eq!(deals["count"], 4, "{deals}");
    assert_eq!(deals["items"][0]["discount"], serde_json::json!(0.5));

    assert_eq!(store::reset(&live).await.unwrap(), 5);
}

#[tokio::test]
#[ignore = "writes to MONGO_URI"]
async fn mongo_admin_writes() {
    let state = state().await;
    let db = state.mongo().clone();
    db.collection::<Document>(CATEGORIES).delete_many(doc! {}).await.unwrap();
    db.collection::<Document>(INTEREST).delete_many(doc! {}).await.unwrap();
    let admin_id = ObjectId::new();

    admin::create_category_core(&state, admin_id, " Gems ").await.unwrap();
    let created = db.collection::<Document>(CATEGORIES).find_one(doc! {"name": "Gems"}).await.unwrap().unwrap();
    let keys: Vec<&str> = created.keys().map(String::as_str).collect();
    assert_eq!(keys, ["_id", "name", "order", "items", "created_by", "created_at"]);
    assert_eq!(created.get("order"), Some(&Bson::Int32(0)));
    let id = created.get_object_id("_id").unwrap().to_hex();
    let body = UpdateBody { name: None, items: Some(vec![" b".into(), "A".into(), "".into(), "b".into()]) };
    let view = admin::update_category_core(&state, &id, &body).await.unwrap();
    assert_eq!(view["items"], serde_json::json!(["A", "b"]));
    assert_eq!(admin::reorder_core(&state, &[id.clone(), id]).await.unwrap(), 1);

    admin::replace_interest_core(&state, admin_id, &[" Glim".into(), "Glim".into(), "Plasmium".into()]).await.unwrap();
    let names: Vec<String> = store::Store::interest(&Live(&state)).await.unwrap().into_iter().collect();
    assert_eq!(names, ["Glim", "Plasmium"]);
    let item = db.collection::<Document>(INTEREST).find_one(doc! {"name": "Glim"}).await.unwrap().unwrap();
    let keys: Vec<&str> = item.keys().map(String::as_str).collect();
    assert_eq!(keys, ["_id", "name", "added_by", "added_at"]);
}

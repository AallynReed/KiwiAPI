//! Store-backed round trip, opt-in because it writes: point MONGO_URI /
//! MONGO_DB / REDIS_URL at throwaway stores and run
//! `cargo test -p kiwi-server feeds_store_roundtrip -- --ignored`.

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use mongodb::bson::{Bson, Document, doc, oid::ObjectId};
use serde_json::{Value, json};
use tower::ServiceExt;

use super::{
    events::{self, ParsedEvent},
    store,
};
use crate::{config::Settings, state::AppState};

async fn get_json(app: &axum::Router, path: &str) -> (StatusCode, axum::http::HeaderMap, Value) {
    let resp = app.clone().oneshot(Request::get(path).body(Body::empty()).unwrap()).await.unwrap();
    let (parts, body) = resp.into_parts();
    let bytes = to_bytes(body, usize::MAX).await.unwrap();
    (parts.status, parts.headers, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

fn keys(d: &Document) -> Vec<&str> {
    d.keys().map(String::as_str).collect()
}

#[tokio::test]
#[ignore = "writes to MONGO_URI / REDIS_URL"]
async fn feeds_store_roundtrip() {
    if std::env::var("PY_UPSTREAM").is_err() {
        // SAFETY: only this ignored test runs in its process invocation.
        unsafe { std::env::set_var("PY_UPSTREAM", "http://127.0.0.1:9") };
    }
    let state = AppState::new(Settings::from_env().unwrap()).await.unwrap();
    let db = state.mongo().clone();
    let marker = ObjectId::new().to_hex();
    let event_id = format!("kiwi-test-{marker}");
    let category = format!("kiwi-test-{marker}");
    let mut ev = ParsedEvent {
        event_id: event_id.clone(),
        name: "Test".into(),
        url: String::new(),
        category: category.clone(),
        image: None,
        icon: Some("i".into()),
        lookup: None,
        starts_at: 1_000,
        ends_at: 5_000_000_000,
    };
    let coll = db.collection::<Document>(store::EVENTS);
    store::upsert_by(&db, &store::EVENT_MODEL, "event_id", &event_id, events::fields(&ev).unwrap()).await.unwrap();
    let first = coll.find_one(doc! { "event_id": &event_id }).await.unwrap().unwrap();
    let order = [
        "_id",
        "event_id",
        "name",
        "url",
        "category",
        "image",
        "icon",
        "lookup",
        "starts_at",
        "ends_at",
        "first_seen_at",
        "updated_at",
    ];
    assert_eq!(keys(&first), order);
    assert_eq!(first.get("starts_at"), Some(&Bson::Int32(1_000)));
    assert_eq!(first.get("ends_at"), Some(&Bson::Int64(5_000_000_000)));
    assert_eq!(first.get("image"), Some(&Bson::Null));

    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    ev.name = "Renamed".into();
    store::upsert_by(&db, &store::EVENT_MODEL, "event_id", &event_id, events::fields(&ev).unwrap()).await.unwrap();
    let second = coll.find_one(doc! { "event_id": &event_id }).await.unwrap().unwrap();
    assert_eq!(keys(&second), order);
    assert_eq!(second.get_str("name").unwrap(), "Renamed");
    assert_eq!(second.get("first_seen_at"), first.get("first_seen_at"));
    assert!(second.get_datetime("updated_at").unwrap() > first.get_datetime("updated_at").unwrap());

    let feed = format!("kiwi-test-{marker}");
    let items = vec![json!({"title": "t", "url": "u", "channel": "c", "video_id": null, "published_at": null,
                            "thumbnail_url": "x"})];
    let fields =
        doc! { "items": store::items_bson(&items).unwrap(), "fetched_at": crate::dt::to_bson(crate::dt::now()) };
    store::upsert_by(&db, &store::FEED_MODEL, "feed", &feed, fields).await.unwrap();
    let cached = db.collection::<Document>(store::FEED_CACHE).find_one(doc! { "feed": &feed }).await.unwrap().unwrap();
    assert_eq!(keys(&cached), ["_id", "feed", "items", "fetched_at"]);
    let (items, _) = store::feed(&db, &feed).await.unwrap();
    assert_eq!(store::video(&items[0])["thumbnail_url"], json!("x"));

    let app = crate::routes::app(state.clone());
    let (status, headers, body) = get_json(&app, &format!("/v1/feeds/events/upcoming?category={category}")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(headers.contains_key("x-ratelimit-limit"));
    assert_eq!(body["count"], json!(0));
    let (status, _, body) = get_json(&app, &format!("/v1/feeds/events?category={category}")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["items"][0]["event_id"], json!(event_id));
    assert_eq!(body["items"][0]["status"], json!("ongoing"));
    let (status, _, body) = get_json(&app, "/v1/feeds/news?limit=0").await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"]["details"][0]["type"], json!("greater_than_equal"));
    let (status, headers, body) = get_json(&app, "/v1/feeds/bilibili/image?url=http://i0.hdslb.com/x.jpg").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(!headers.contains_key("x-ratelimit-limit"));
    assert_eq!(body["error"]["message"], json!("only https hdslb.com images may be proxied"));
    let (status, _, _) = get_json(&app, "/v1/misc/news-history").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    coll.delete_many(doc! { "event_id": &event_id }).await.unwrap();
    db.collection::<Document>(store::FEED_CACHE).delete_many(doc! { "feed": &feed }).await.unwrap();
}

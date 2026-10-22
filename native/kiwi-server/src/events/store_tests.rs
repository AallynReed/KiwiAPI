//! Store-backed checks, opt-in because they write: point REDIS_URL and
//! MONGO_URI / MONGO_DB at throwaway stores and run
//! `cargo test -p kiwi-server events_store -- --ignored --test-threads 1`.

use std::time::Duration;

use futures::StreamExt;
use mongodb::bson::{DateTime, Document, doc, oid::ObjectId};
use serde_json::json;

use super::{bus, sources::Source};
use crate::{config::Settings, pyjson, state::AppState};

async fn state() -> AppState {
    if std::env::var("PY_UPSTREAM").is_err() {
        // SAFETY: set before any other thread reads the environment.
        unsafe { std::env::set_var("PY_UPSTREAM", "http://127.0.0.1:9") };
    }
    AppState::new(Settings::from_env().unwrap()).await.unwrap()
}

async fn cmd<T: redis::FromRedisValue>(state: &AppState, args: &[&str]) -> T {
    let mut con = state.redis();
    let mut c = redis::cmd(args[0]);
    for a in &args[1..] {
        c.arg(*a);
    }
    c.query_async(&mut con).await.unwrap()
}

#[tokio::test]
#[ignore = "writes to REDIS_URL"]
async fn events_store_publish_dedups_and_fans_out() {
    let state = state().await;
    let channel = state.settings().events_channel.clone();
    let client = redis::Client::open(state.settings().redis_url.clone().unwrap()).unwrap();
    let mut pubsub = client.get_async_pubsub().await.unwrap();
    pubsub.subscribe(&channel).await.unwrap();

    let kind = format!("parity_{}", std::process::id());
    let _: () = cmd(&state, &["DEL", &format!("events:sig:{kind}")]).await;
    assert!(bus::publish(&state, &kind, "a", json!({"n": 1, "s": "Ünï"})).await);
    assert!(!bus::publish(&state, &kind, "a", json!({"n": 2})).await);
    assert!(bus::publish(&state, &kind, "b", json!({"n": 3})).await);
    let sig: String = cmd(&state, &["GET", &format!("events:sig:{kind}")]).await;
    assert_eq!(sig, "b");
    let ttl: i64 = cmd(&state, &["TTL", &format!("events:sig:{kind}")]).await;
    assert_eq!(ttl, -1, "no TTL, as Python");

    let mut messages = pubsub.on_message();
    for n in [1, 3] {
        let msg = tokio::time::timeout(Duration::from_secs(2), messages.next()).await.unwrap().unwrap();
        let text = String::from_utf8(msg.get_payload_bytes().to_vec()).unwrap();
        let payload = pyjson::loads(text.as_bytes()).unwrap();
        assert_eq!(payload["data"]["n"], n);
        assert_eq!(text, pyjson::dumps_spaced(&payload), "json.dumps(default=str) bytes");
        assert!(text.starts_with(&format!("{{\"type\": \"{kind}\", \"data\": ")));
    }
    drop(messages);

    let queues = [state.settings().webhooks_queue.clone(), state.settings().dm_subs_queue.clone()];
    let mut before = Vec::new();
    for q in &queues {
        before.push(cmd::<i64>(&state, &["LLEN", q]).await);
    }
    let sig = format!("parity:{}", ObjectId::new());
    assert!(bus::publish(&state, "challenge", &sig, json!({"name": "Parity"})).await);
    for (q, n) in queues.iter().zip(before) {
        let after: i64 = cmd(&state, &["LLEN", q]).await;
        assert_eq!(after, n + 1, "{q} (feature flags on)");
        let head: String = cmd(&state, &["LINDEX", q, "0"]).await;
        assert_eq!(pyjson::loads(head.as_bytes()).unwrap()["data"]["name"], "Parity");
        let _: () = cmd(&state, &["LPOP", q]).await;
    }
    let before: i64 = cmd(&state, &["LLEN", &queues[0]]).await;
    assert!(bus::publish(&state, "corruxion", &sig, json!({})).await);
    assert_eq!(cmd::<i64>(&state, &["LLEN", &queues[0]]).await, before, "corruxion is no webhook type");
    let _: () = cmd(&state, &["LPOP", &queues[1]]).await;
}

#[tokio::test]
#[ignore = "subscribes on REDIS_URL"]
async fn events_store_listener_fans_out() {
    let state = state().await;
    tokio::spawn(bus::listen(state.clone()));
    tokio::time::sleep(Duration::from_millis(500)).await;
    let mut sub = bus::subscribe();
    let channel = state.settings().events_channel.clone();
    let _: i64 = cmd(&state, &["PUBLISH", &channel, "not json"]).await;
    let _: i64 = cmd(&state, &["PUBLISH", &channel, r#"{"type":"t","data":{"f":1.0},"ts":1}"#]).await;
    let ev = tokio::time::timeout(Duration::from_secs(2), sub.recv()).await.unwrap().unwrap();
    assert_eq!(
        super::stream::frame(&ev).unwrap(),
        "event: t\ndata: {\"type\": \"t\", \"data\": {\"f\": 1.0}, \"ts\": 1}\n\n"
    );
    drop(sub);
    assert_eq!(bus::connection_count(), 0);
}

#[tokio::test]
#[ignore = "writes to MONGO_URI"]
async fn events_store_mongo_sources() {
    let state = state().await;
    let db = state.mongo().clone();
    let far = DateTime::from_millis(4_102_444_800_123);
    let marker = ObjectId::new();

    let news = db.collection::<Document>("trove_news");
    news.insert_one(doc! { "_id": marker, "url": "https://parity/x", "title": "Parity Ünï", "published_at": far })
        .await
        .unwrap();
    let data = Source::TroveNews.data(&state).await.unwrap();
    assert_eq!(
        pyjson::dumps_spaced(&data),
        r#"{"item": {"title": "Parity Ünï", "url": "https://parity/x", "published_at": "2100-01-01T00:00:00.123000+00:00"}}"#
    );
    assert_eq!(Source::TroveNews.sig(&data, 0).as_deref(), Some("https://parity/x"));
    news.delete_one(doc! { "_id": marker }).await.unwrap();

    let giveaways = db.collection::<Document>("giveaways");
    giveaways.insert_one(doc! { "_id": marker, "title": "Parity", "status": "open", "created_at": far }).await.unwrap();
    let data = Source::Giveaways.data(&state).await.unwrap();
    assert_eq!(data, json!({"newest": {"id": marker.to_hex(), "title": "Parity", "created_at": 4_102_444_800_i64}}));
    giveaways.delete_one(doc! { "_id": marker }).await.unwrap();

    let versions = db.collection::<Document>("update_versions");
    versions
        .insert_one(doc! { "_id": marker, "branch": "live-us", "ordinal": 2_000_000_000_i64, "version_tag": "p",
        "status": "complete", "files_added": 3, "bytes_added": 5_000_000_000_i64 })
        .await
        .unwrap();
    let data = Source::GameUpdate.data(&state).await.unwrap();
    assert_eq!(
        pyjson::dumps_spaced(&data),
        r#"{"version": {"branch": "live-us", "ordinal": 2000000000, "version_tag": "p", "files_added": 3, "files_modified": 0, "files_removed": 0, "bytes_added": 5000000000, "completed_at": null}}"#
    );
    assert_eq!(Source::GameUpdate.sig(&data, 0).as_deref(), Some("live-us:2000000000"));
    versions.delete_one(doc! { "_id": marker }).await.unwrap();

    let now = super::sources::now_us();
    let w = kiwi_core::rotations::challenge_window(now);
    let challenges = db.collection::<Document>("challenge_captures");
    let prev = challenges.find_one_and_delete(doc! { "window_anchor": w.starts_at }).await.unwrap();
    challenges
        .insert_one(doc! { "window_anchor": w.starts_at, "window_ends_at": w.ends_at, "name": "Cursed Vale",
        "is_friday_window": w.is_friday_window, "captured_at": DateTime::from_millis(1_792_000_000_777) })
        .await
        .unwrap();
    let data = Source::Challenge.data(&state).await.unwrap();
    assert_eq!(data["captured_at"], "2026-10-14 17:46:40.777000+00:00");
    assert_eq!(data["type"], "dungeon");
    assert_eq!(Source::Challenge.sig(&data, now), Some(format!("{}:Cursed Vale", w.starts_at)));
    challenges.delete_one(doc! { "window_anchor": w.starts_at }).await.unwrap();
    if let Some(prev) = prev {
        challenges.insert_one(prev).await.unwrap();
    }

    let snapshot = bus::current_snapshot(&state).await;
    let kinds: Vec<&str> = snapshot.iter().map(|e| e["type"].as_str().unwrap()).collect();
    let order: Vec<&str> = super::sources::SOURCES.iter().map(|s| s.kind()).filter(|k| kinds.contains(k)).collect();
    assert_eq!(kinds, order, "SOURCES order");
    for kind in ["longshade", "wild_mana", "daily_bonuses", "activity"] {
        assert!(kinds.contains(&kind), "{kind} always has a signature");
    }
    assert!(snapshot.iter().all(|e| e["ts"] == snapshot[0]["ts"]));
}

#[tokio::test]
#[ignore = "rate-limits on REDIS_URL, reads MONGO_URI"]
async fn events_store_route_streams() {
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    let state = state().await;
    let app = super::router().with_state(state.clone());
    let req = Request::get(super::STREAM_PATH)
        .extension(axum::extract::ConnectInfo(std::net::SocketAddr::from(([127, 0, 0, 1], 9))))
        .body(Body::empty())
        .unwrap();
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(res.headers()["content-type"], "text/event-stream; charset=utf-8");
    assert!(res.headers().get("x-ratelimit-limit").is_none(), "Python drops the sub-response headers");
    let mut body = res.into_body();
    let first = body.frame().await.unwrap().unwrap().into_data().unwrap();
    assert_eq!(&first[..], super::stream::RETRY);
    let second = body.frame().await.unwrap().unwrap().into_data().unwrap();
    let text = String::from_utf8(second.to_vec()).unwrap();
    assert!(text.starts_with("event: "), "{text}");
    assert_eq!(bus::connection_count(), 0, "subscribed only after the snapshot is out");
}

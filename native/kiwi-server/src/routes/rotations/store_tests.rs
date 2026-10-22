//! Store-backed checks, opt-in because they write: point MONGO_URI / MONGO_DB
//! (and REDIS_URL, or the Mongo rate-limit fallback is used) at throwaway
//! stores and run `cargo test -p kiwi-server rotations_store -- --ignored`.

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use mongodb::bson::{Bson, DateTime, Document, doc};
use tower::ServiceExt;

use super::{bodies, delves, store};
use crate::{config::Settings, pyjson, state::AppState};

async fn state() -> AppState {
    if std::env::var("PY_UPSTREAM").is_err() {
        // SAFETY: set before any other thread reads the environment.
        unsafe { std::env::set_var("PY_UPSTREAM", "http://127.0.0.1:9") };
    }
    AppState::new(Settings::from_env().unwrap()).await.unwrap()
}

fn keys(d: &Document) -> Vec<&str> {
    d.keys().map(String::as_str).collect()
}

async fn get(state: &AppState, path: &str) -> (StatusCode, String) {
    let app = super::router().with_state(state.clone());
    let resp = app.oneshot(Request::get(path).body(Body::empty()).unwrap()).await.unwrap();
    let status = resp.status();
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    (status, String::from_utf8(body.to_vec()).unwrap())
}

#[tokio::test]
#[ignore = "writes to MONGO_URI"]
async fn rotations_store_writes_python_shapes() {
    let state = state().await;
    let db = state.mongo().clone();
    let week = 900_000 + i64::from(std::process::id() % 1000);
    let coll = db.collection::<Document>(store::DELVES);

    let payload = pyjson::loads(br#"{"depths":[{"depth":1,"x":2.0,"big":3000000000}],"total":true}"#).unwrap();
    delves::store_week(&state, week, &payload).await.unwrap();
    let row = coll.find_one(doc! { "week": week }).await.unwrap().unwrap();
    assert_eq!(keys(&row), ["_id", "week", "depths", "total", "depth_count", "fetched_at"]);
    assert_eq!(row.get("total"), Some(&Bson::Int32(1)), "a new row coerces the bool");
    let depth = row.get_array("depths").unwrap()[0].as_document().unwrap().clone();
    assert!(matches!(depth.get("x"), Some(Bson::Double(_))) && matches!(depth.get("big"), Some(Bson::Int64(_))));

    let empty = pyjson::loads(br#"{"depths":[],"total":5}"#).unwrap();
    assert_eq!(delves::store_week(&state, week, &empty).await.unwrap(), 1, "empty payload keeps stored depths");
    let again = pyjson::loads(br#"{"depths":[{"depth":2},{"depth":3}],"total":true}"#).unwrap();
    delves::store_week(&state, week, &again).await.unwrap();
    let row = coll.find_one(doc! { "week": week }).await.unwrap().unwrap();
    assert_eq!(keys(&row), ["_id", "week", "depths", "total", "depth_count", "fetched_at"]);
    assert_eq!(row.get("total"), Some(&Bson::Boolean(true)), "save() writes the assigned value");
    assert_eq!(row.get("depth_count"), Some(&Bson::Int32(2)));

    let (status, body) = get(&state, &format!("/v1/rotations/delves?week={week}")).await;
    if status == StatusCode::OK {
        assert!(body.starts_with(&format!(r#"{{"week":{week},"is_current":false,"total":1,"count":2"#)), "{body}");
    } else {
        assert_eq!(status, StatusCode::NOT_FOUND, "delves disabled: {body}");
    }
    coll.delete_one(doc! { "week": week }).await.unwrap();

    let feeds = db.collection::<Document>(store::FEED_CACHE);
    let before = feeds.find_one(doc! { "feed": super::chaos::FEED }).await.unwrap();
    let item = super::chaos::normalize(&pyjson::loads(br#"{"name":"N","start":"5"}"#).unwrap()).unwrap();
    store::save_chaos_feed(&db, &item).await.unwrap();
    let row = feeds.find_one(doc! { "feed": super::chaos::FEED }).await.unwrap().unwrap();
    assert_eq!(keys(&row), ["_id", "feed", "items", "fetched_at"]);
    let stored = row.get_array("items").unwrap()[0].as_document().unwrap().clone();
    assert_eq!(keys(&stored), ["name", "identifier", "blueprint", "start", "end"]);
    assert_eq!(stored.get("start"), Some(&Bson::Int32(5)));
    assert_eq!(stored.get("end"), Some(&Bson::Null));
    match before {
        Some(old) => {
            feeds.replace_one(doc! { "_id": old.get("_id").unwrap().clone() }, old).await.unwrap();
        }
        None => {
            feeds.delete_one(doc! { "feed": super::chaos::FEED }).await.unwrap();
        }
    }

    let lux = db.collection::<Document>(store::LUXION);
    let anchor = 4_000_000_000_i64 + i64::from(std::process::id());
    let seen = DateTime::from_millis(anchor * 1000 + 123);
    lux.insert_one(doc! { "started_at": anchor, "first_seen_at": seen, "last_seen_at": seen, "ended_at": Bson::Null })
        .await
        .unwrap();
    let latest = store::latest_luxion(&db).await.unwrap().unwrap();
    assert_eq!(latest.started_at, anchor);
    let (status, body) = get(&state, "/v1/rotations/luxion").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains(r#""first_seen_at":""#), "{body}");
    let (status, body) = get(&state, "/v1/rotations/luxion/history?limit=1").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.starts_with(&format!(r#"{{"items":[{}"#, pyjson::pydantic(&bodies::luxion_history_item(&latest)))));
    lux.delete_one(doc! { "started_at": anchor }).await.unwrap();

    assert_eq!(get(&state, "/v1/rotations/luxion/history?limit=0").await.0, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(get(&state, "/v1/rotations/challenge/history?limit=500").await.0, StatusCode::OK);
    assert_eq!(get(&state, "/v1/rotations/challenge/history?limit=501").await.0, StatusCode::UNPROCESSABLE_ENTITY);
    for path in ["/v1/rotations/chaos-chest", "/v1/rotations/challenge/current", "/v1/rotations/calendar"] {
        assert_eq!(get(&state, path).await.0, StatusCode::OK, "{path}");
    }
}

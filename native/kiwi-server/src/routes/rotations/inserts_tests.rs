//! Capture inserts against `testdata/producers/golden.json`
//! (`native/port/parity/gen_producers_fixtures.py`).

use std::sync::OnceLock;

use chrono::{DateTime, Utc};
use mongodb::bson::{Bson, Document, doc};
use serde_json::Value;

use super::{
    bodies::{ChallengeDoc, LuxionDoc},
    inserts::{
        LuxionStep, challenge_fields, challenge_name_ok, chaos_fields, current_daily_reset, luxion_fields, luxion_step,
    },
};
use crate::auth::pyutil::bson_int;

pub(crate) fn golden() -> &'static Value {
    static G: OnceLock<Value> = OnceLock::new();
    G.get_or_init(|| {
        serde_json::from_str(include_str!("../../../testdata/producers/golden.json")).expect("producers golden")
    })
}

fn canonical(d: Document) -> Value {
    Bson::Document(d).into_canonical_extjson()
}

fn at() -> DateTime<Utc> {
    DateTime::from_timestamp_micros(1_792_666_800_123_456).unwrap()
}

#[test]
fn daily_reset_matches_python() {
    for row in golden()["daily_reset"].as_array().unwrap() {
        assert_eq!(current_daily_reset(row[0].as_i64().unwrap()), row[1].as_i64().unwrap(), "{row}");
    }
}

#[test]
fn luxion_live_run_matches_python() {
    for case in golden()["luxion_live"].as_array().unwrap() {
        let first = DateTime::from_timestamp_millis(case["first_seen_ms"].as_i64().unwrap()).unwrap();
        let doc = LuxionDoc {
            started_at: case["started_at"].as_i64().unwrap(),
            first_seen_at: first,
            last_seen_at: first,
            ended_at: case["ended"].as_bool().unwrap().then_some(first),
        };
        let now = case["now"].as_i64().unwrap();
        let live = case["live"].as_bool().unwrap();
        assert_eq!(luxion_step(Some(&doc), now, true), if live { LuxionStep::Refresh } else { LuxionStep::Open });
        assert_eq!(
            luxion_step(Some(&doc), now, false),
            if live { LuxionStep::Close } else { LuxionStep::NothingToClose }
        );
    }
    assert_eq!(luxion_step(None, 0, true), LuxionStep::Open);
    assert_eq!(luxion_step(None, 0, false), LuxionStep::NothingToClose);
}

#[test]
fn capture_documents_match_beanie() {
    let docs = &golden()["documents"];
    assert_eq!(canonical(chaos_fields(1_792_576_800, "Chest", at())), docs["chaos"]);
    assert_eq!(canonical(chaos_fields(3_000_000_000, "Chest", at())), docs["chaos_big"]);
    let ch = ChallengeDoc {
        window_anchor: 1_792_576_800,
        window_ends_at: 1_792_578_000,
        name: "Cursed Vale".into(),
        is_friday_window: true,
        captured_at: at(),
    };
    assert_eq!(canonical(challenge_fields(&ch)), docs["challenge"]);
    let open = LuxionDoc { started_at: 1_792_576_800, first_seen_at: at(), last_seen_at: at(), ended_at: None };
    assert_eq!(canonical(luxion_fields(&open)), docs["luxion_open"]);
    let closed = LuxionDoc { ended_at: Some(at()), ..open };
    let mut want = docs["luxion_closed"].clone();
    want.as_object_mut().unwrap().shift_remove("_id");
    assert_eq!(canonical(luxion_fields(&closed)), want);
    let summary = doc! { "started_at": bson_int(0), "active": false, "refreshed": true };
    assert_eq!(canonical(summary), docs["log_luxion"]);
}

fn header_map(ctype: &Value) -> axum::http::HeaderMap {
    let mut h = axum::http::HeaderMap::new();
    if let Some(ct) = ctype.as_str() {
        h.insert(axum::http::header::CONTENT_TYPE, ct.parse().unwrap());
    }
    h
}

/// The request through body reading, validation and the name check, as
/// `(status, body)` rendered under the case's request id.
async fn capture_reply(case: &Value, luxion: bool) -> Option<(u16, String)> {
    use crate::validation::{read_body, validate};
    let bytes: Vec<u8> = case["body"].as_str().unwrap().chars().map(|c| c as u8).collect();
    let rid = case["request_id"].as_str().unwrap().to_owned();
    let headers = header_map(&case["content_type"]);
    let err = match read_body(&headers, axum::body::Body::from(bytes)).await {
        Err(e) => e,
        Ok(body) if luxion => match validate::<super::inserts::LuxionInsert>(&body) {
            Err(e) => e,
            Ok(_) => return None,
        },
        Ok(body) => match validate::<super::inserts::CaptureInsert>(&body) {
            Err(e) => e,
            Ok(req) => {
                let name = kiwi_core::pystr::strip(&req.name);
                let url = case["url"].as_str().unwrap();
                if url.contains("challenge") && !challenge_name_ok(name) {
                    crate::error::ApiError::bad_request("challenge name is empty or 'none'")
                } else if url.contains("chaos") && name.is_empty() {
                    crate::error::ApiError::bad_request("name is empty")
                } else {
                    return None;
                }
            }
        },
    };
    let status = err.status.as_u16();
    Some((status, crate::request_id::scope(rid, async move { err.body() }).await))
}

#[tokio::test]
async fn capture_request_errors_match_fastapi() {
    let http = &golden()["http"];
    for (section, luxion) in [("capture", false), ("luxion", true)] {
        for case in http[section].as_array().unwrap() {
            let got = capture_reply(case, luxion).await;
            let want = (case["status"].as_u64().unwrap() as u16, case["response"].as_str().unwrap().to_owned());
            assert_eq!(got, Some(want), "{case}");
        }
    }
}

#[test]
fn challenge_names_rejected_like_python() {
    for bad in ["", "none", "None", "NONE", "nOnE"] {
        assert!(!challenge_name_ok(bad), "{bad:?}");
    }
    for good in ["nonee", "Cursed Vale", "RAMPAGE ALERT!", "n one"] {
        assert!(challenge_name_ok(good), "{good:?}");
    }
}

/// Opt-in: point MONGO_URI / MONGO_DB at a throwaway database and run
/// `cargo test -p kiwi-server inserts_store -- --ignored --test-threads 1`.
#[tokio::test]
#[ignore = "writes to MONGO_URI"]
async fn inserts_store_capture_lifecycles() {
    use super::{inserts, store};
    use crate::{config::Settings, state::AppState};

    let state = AppState::new(Settings::from_env().unwrap()).await.unwrap();
    let db = state.mongo();
    for (coll, key) in
        [(store::LUXION, "started_at"), (store::CHAOS, "week_anchor"), (store::CHALLENGE, "window_anchor")]
    {
        let c = db.collection::<Document>(coll);
        c.delete_many(doc! {}).await.unwrap();
        let index = mongodb::IndexModel::builder()
            .keys(doc! { key: -1 })
            .options(mongodb::options::IndexOptions::builder().unique(true).build())
            .build();
        c.create_index(index).await.unwrap();
    }

    let real = crate::dt::now();
    let anchor = current_daily_reset(real.timestamp_micros());
    assert_eq!(inserts::insert_luxion(db, false, real).await.unwrap(), (None, false));
    assert_eq!(inserts::insert_luxion(db, true, real).await.unwrap(), (Some(anchor), true));
    assert_eq!(inserts::insert_luxion(db, true, real).await.unwrap(), (Some(anchor), false));
    assert_eq!(inserts::insert_luxion(db, false, real).await.unwrap(), (Some(anchor), false));
    assert!(store::latest_luxion(db).await.unwrap().unwrap().ended_at.is_some());
    assert_eq!(inserts::insert_luxion(db, true, real).await.unwrap(), (Some(anchor), false), "raced re-open");
    assert_eq!(store::latest_luxion(db).await.unwrap().unwrap().ended_at, None);
    let raw = db.collection::<Document>(store::LUXION).find_one(doc! {}).await.unwrap().unwrap();
    let keys: Vec<&str> = raw.keys().map(String::as_str).collect();
    assert_eq!(keys, ["_id", "started_at", "first_seen_at", "last_seen_at", "ended_at"]);

    let now = real.timestamp_micros();
    let (week, new) = inserts::insert_chaos(db, "A", now).await.unwrap();
    assert!(new);
    assert_eq!(inserts::insert_chaos(db, "B", now).await.unwrap(), (week, false));
    assert_eq!(store::chaos_for_week(db, week).await.unwrap().unwrap().name, "B");

    let (window, new) = inserts::insert_challenge(db, "Cursed Vale", now).await.unwrap();
    assert!(new);
    assert_eq!(inserts::insert_challenge(db, "Permafrost", now).await.unwrap(), (window, false));
    assert_eq!(store::challenge_for_window(db, window).await.unwrap().unwrap().name, "Permafrost");
}

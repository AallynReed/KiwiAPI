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

#[test]
fn challenge_names_rejected_like_python() {
    for bad in ["", "none", "None", "NONE", "nOnE"] {
        assert!(!challenge_name_ok(bad), "{bad:?}");
    }
    for good in ["nonee", "Cursed Vale", "RAMPAGE ALERT!", "n one"] {
        assert!(challenge_name_ok(good), "{good:?}");
    }
}

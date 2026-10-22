//! The digest and sweep rules against `testdata/email/golden.json`.

use std::collections::HashMap;

use chrono::{DateTime, TimeDelta, Utc};
use kiwi_core::mail;
use mongodb::bson::{Bson, Document, doc, oid::ObjectId};
use serde_json::Value;

use super::{
    days_left, digest_due, digest_pipeline, digest_rows, hours_us, inactive_filter, py_num_text, times_09, token_set,
};
use crate::email::tests::{golden, s, settings};

fn now() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-10-22T11:00:00.123456+00:00").unwrap().with_timezone(&Utc)
}

fn oid(v: &Value) -> Bson {
    v.as_str().map_or(Bson::Null, |h| Bson::ObjectId(ObjectId::parse_str(h).unwrap()))
}

#[test]
fn digests() {
    let app_name = settings().app_name;
    for c in golden()["digests"].as_array().unwrap() {
        let hours = c["hours"].clone();
        let window = hours_us(&hours).unwrap();
        let last = c["last"].as_str().map(|t| DateTime::parse_from_rfc3339(t).unwrap().with_timezone(&Utc));
        assert_eq!(digest_due(now(), last, window), c["marked"].as_bool().unwrap(), "{c}");
        if !c["marked"].as_bool().unwrap() {
            continue;
        }
        let since = now() - TimeDelta::microseconds(window);
        let pipeline: Vec<Value> =
            digest_pipeline(since).into_iter().map(|d| Bson::Document(d).into_canonical_extjson()).collect();
        assert_eq!(Value::Array(pipeline), c["pipeline"]);
        let rows: Vec<Document> = c["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| doc! {"_id": oid(&r["_id"]), "count": r["count"].as_i64().unwrap(), "user_id": oid(&r["user_id"])})
            .collect();
        let emails: HashMap<ObjectId, String> = c["users"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| (ObjectId::parse_str(k).unwrap(), s(v).to_owned()))
            .collect();
        let names: HashMap<ObjectId, (String, String)> = c["tokens"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| (ObjectId::parse_str(k).unwrap(), (s(&v[0]).to_owned(), s(&v[1]).to_owned())))
            .collect();
        let digest = digest_rows(&rows, &emails, &names);
        let total: i64 = digest.iter().map(|r| r.count).sum();
        let sent = total >= c["threshold"].as_i64().unwrap();
        assert_eq!(sent, c["sent"].as_bool().unwrap());
        if !sent {
            continue;
        }
        let (subject, text, html) = mail::rate_limit_digest(&app_name, &py_num_text(&hours), total, &digest);
        let m = &c["mail"];
        assert_eq!((subject.as_str(), text.as_str(), html.as_str()), (s(&m["subject"]), s(&m["text"]), s(&m["html"])));
        assert_eq!(s(&m["to"]), "Alerts@Example.test");
    }
}

#[test]
fn window_scaling() {
    for pair in golden()["times09"].as_array().unwrap() {
        assert_eq!(times_09(pair[0].as_i64().unwrap()), pair[1].as_i64().unwrap());
    }
    assert_eq!(hours_us(&serde_json::json!(1.5)), Some(5_400_000_000));
    assert_eq!(py_num_text(&serde_json::json!(24)), "24");
    assert_eq!(py_num_text(&serde_json::json!(1.5)), "1.5");
}

#[test]
fn days_left_floors() {
    let n = now();
    assert_eq!(days_left(n + TimeDelta::hours(47), n), 1);
    assert_eq!(days_left(n + TimeDelta::hours(48), n), 2);
    assert_eq!(days_left(n + TimeDelta::seconds(5), n), 0);
    assert_eq!(days_left(n - TimeDelta::seconds(5), n), 0);
}

#[test]
fn revoke_filter() {
    let f = inactive_filter(DateTime::from_timestamp(1_000, 0).unwrap());
    let cutoff = Bson::DateTime(mongodb::bson::DateTime::from_millis(1_000_000));
    assert_eq!(
        f,
        doc! {"revoked": false, "$or": [
            {"last_used_at": {"$ne": Bson::Null, "$lt": cutoff.clone()}},
            {"last_used_at": Bson::Null, "created_at": {"$lt": cutoff}},
        ]}
    );
}

#[test]
fn token_save_shape() {
    let t: crate::auth::models::ApiToken = mongodb::bson::from_document(doc! {
        "_id": ObjectId::new(), "user_id": ObjectId::new(), "name": "n", "prefix": "p", "hashed_token": "h",
        "scopes": 3_000_000_000_i64, "request_count": 4,
    })
    .unwrap();
    let d = token_set(&t);
    let keys: Vec<&str> = d.keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        [
            "_id",
            "user_id",
            "name",
            "prefix",
            "hashed_token",
            "scopes",
            "ip_salt",
            "allowed_ip_hashes",
            "revoked",
            "revoked_at",
            "revoke_reason",
            "expires_at",
            "expiry_warned",
            "created_at",
            "last_used_at",
            "rotated_at",
            "request_count"
        ]
    );
    assert_eq!(d.get("scopes"), Some(&Bson::Int64(3_000_000_000)));
    assert_eq!(d.get("request_count"), Some(&Bson::Int32(4)));
    assert_eq!(d.get("ip_salt"), Some(&Bson::Null));
}

//! Checks against `testdata/email/golden.json`
//! (`native/port/parity/gen_email_fixtures.py`).

use std::sync::OnceLock;

use chrono::{TimeDelta, Utc};
use kiwi_core::mail::{Brand, Button, render_email, token_expiring};
use serde_json::Value;

use super::{
    OutboxEmail,
    mailer::{DeliveryError, build_message, make_msgid},
    worker::{apply_outcome, backoff},
};
use crate::config::Settings;

pub(crate) fn golden() -> &'static Value {
    static G: OnceLock<Value> = OnceLock::new();
    G.get_or_init(|| serde_json::from_str(include_str!("../../testdata/email/golden.json")).expect("fixture"))
}

pub(crate) fn s(v: &Value) -> &str {
    v.as_str().unwrap_or_else(|| panic!("string: {v}"))
}

pub(crate) fn settings() -> Settings {
    let vars: Vec<(String, String)> =
        golden()["env"].as_object().unwrap().iter().map(|(k, v)| (k.clone(), s(v).to_owned())).collect();
    Settings::from_vars(vars).expect("settings")
}

#[test]
fn renders_like_jinja() {
    let st = settings();
    let brand = Brand { app_name: &st.app_name, dev_url: &st.dev_url, docs_url: &st.docs_url };
    for c in golden()["render"].as_array().unwrap() {
        let paragraphs: Vec<String> = serde_json::from_value(c["paragraphs"].clone()).unwrap();
        let button = c["button"].as_object().map(|b| Button { label: s(&b["label"]).into(), url: s(&b["url"]).into() });
        let html = render_email(&brand, s(&c["heading"]), &paragraphs, button.as_ref(), c["note"].as_str());
        assert_eq!(html, s(&c["html"]), "{}", c["name"]);
    }
}

#[test]
fn token_expiring_notice() {
    let st = settings();
    for c in golden()["notices"].as_array().unwrap() {
        let n = token_expiring(
            &st.app_name,
            &st.dev_url,
            s(&c["token_name"]),
            s(&c["prefix"]),
            c["days_left"].as_i64().unwrap(),
        );
        assert_eq!(n.subject, s(&c["subject"]));
        assert_eq!(n.text(), s(&c["text"]));
        assert_eq!(n.html(&super::brand(&st)), s(&c["html"]));
    }
}

#[test]
fn outbox_document_shape() {
    let doc = OutboxEmail::new("x@example.test", "S", "T", None).to_doc();
    let keys: Vec<&str> = doc.keys().map(String::as_str).collect();
    let mut want = vec!["_id"];
    want.extend(golden()["outbox_keys"].as_array().unwrap().iter().map(s));
    assert_eq!(keys, want);
    assert_eq!(doc.get("attempts"), Some(&mongodb::bson::Bson::Int32(0)));
    assert_eq!(doc.get("html"), Some(&mongodb::bson::Bson::Null));
    let back = OutboxEmail::from_doc(&doc).unwrap();
    assert_eq!(back.to_doc(), doc);
}

#[test]
fn retry_backoff() {
    for pair in golden()["backoff"].as_array().unwrap() {
        let secs = pair[1].as_f64().unwrap() as i64;
        assert_eq!(backoff(60, pair[0].as_i64().unwrap()), TimeDelta::seconds(secs));
    }
}

#[test]
fn outcomes() {
    let st = settings();
    let now = Utc::now();
    let mut msg = OutboxEmail::new("a@b.c", "s", "t", None);
    assert!(!apply_outcome(&mut msg, &Ok(()), &st, now));
    assert_eq!((msg.status.as_str(), msg.attempts, msg.sent_at), ("sent", 1, Some(now)));

    let transient = DeliveryError { message: "x".repeat(600), permanent: false, code: Some(451) };
    let mut msg = OutboxEmail::new("a@b.c", "s", "t", None);
    assert!(!apply_outcome(&mut msg, &Err(transient.clone()), &st, now));
    assert_eq!((msg.status.as_str(), msg.attempts), ("pending", 1));
    assert_eq!(msg.last_error.as_ref().map(|e| e.len()), Some(500));
    assert_eq!(msg.next_attempt_at, now + TimeDelta::seconds(60));
    msg.attempts = 4;
    assert!(!apply_outcome(&mut msg, &Err(transient), &st, now));
    assert_eq!((msg.status.as_str(), msg.attempts), ("failed", 5));

    let bounce = DeliveryError { message: "SMTP 550: no".into(), permanent: true, code: Some(550) };
    let mut msg = OutboxEmail::new("a@b.c", "s", "t", None);
    assert!(apply_outcome(&mut msg, &Err(bounce), &st, now));
    assert_eq!((msg.status.as_str(), msg.attempts, msg.last_error.as_deref()), ("bounced", 1, Some("SMTP 550: no")));
}

#[test]
fn message_shape() {
    let st = settings();
    let m = build_message(&st, "Someone <someone@example.test>", "Hi ✓", "plain ✓", Some("<p>html</p>")).unwrap();
    let raw = String::from_utf8_lossy(&m.formatted()).into_owned();
    assert!(raw.contains("From: Kiwi <no-reply@aallyn.net>"), "{raw}");
    assert!(raw.contains("multipart/alternative"), "{raw}");
    assert!(raw.contains("Message-ID: <") && raw.contains("@aallyn.net>"), "{raw}");
    assert!(raw.contains("Date: "), "{raw}");
    let single = build_message(&st, "a@example.test", "s", "t", Some("")).unwrap();
    assert!(!String::from_utf8_lossy(&single.formatted()).contains("multipart"));
    let bad = build_message(&st, "not an address", "s", "t", None).unwrap_err();
    assert!(bad.permanent);
    assert!(make_msgid("x.test").ends_with("@x.test>"));
}

/// Queue, claim, deliver-fail and bounce against a real Mongo (no SMTP).
#[tokio::test]
#[ignore = "needs MONGO_URI pointing at a throwaway mongo (creates and drops a scratch database)"]
async fn outbox_store_flow() {
    use std::sync::Arc;

    use mongodb::bson::{Document, doc};

    use crate::{
        db::Db,
        state::{AppState, Inner},
    };

    let uri = std::env::var("MONGO_URI").expect("MONGO_URI");
    let client = mongodb::Client::with_uri_str(&uri).await.unwrap();
    let db = client.database(&format!("kiwi_email_test_{}", mongodb::bson::oid::ObjectId::new().to_hex()));
    let mut st = settings();
    st.smtp_host = Some("127.0.0.1".into());
    st.smtp_port = 9;
    let state =
        AppState { inner: Arc::new(Inner { settings: st, db: Db { mongo: Some(db.clone()), redis: None, pg: None } }) };
    db.collection::<Document>("users")
        .insert_one(doc! {"email": "dead@example.test", "hashed_password": "x", "email_bounced": true})
        .await
        .unwrap();
    super::queue_email(&state, "Dead@Example.test", "s", "t", None).await.unwrap();
    super::queue_email(&state, "live@example.test", "s", "t", Some("<p>h</p>")).await.unwrap();
    let outbox = db.collection::<Document>(super::OUTBOX);
    assert_eq!(outbox.count_documents(doc! {}).await.unwrap(), 1);
    let claimed = super::worker::claim_next(&db).await.unwrap().unwrap();
    assert_eq!((claimed.status.as_str(), claimed.to.as_str()), ("sending", "live@example.test"));
    assert!(super::worker::claim_next(&db).await.unwrap().is_none());
    db.collection::<Document>(super::OUTBOX)
        .update_one(doc! {"_id": claimed.id}, doc! {"$set": {"status": "pending"}})
        .await
        .unwrap();
    assert_eq!(super::worker::run_once(&state).await.unwrap(), 1);
    let after = outbox.find_one(doc! {}).await.unwrap().unwrap();
    assert_eq!(after.get_str("status").unwrap(), "pending");
    assert_eq!(after.get_i32("attempts").unwrap(), 1);
    db.drop().await.unwrap();
}

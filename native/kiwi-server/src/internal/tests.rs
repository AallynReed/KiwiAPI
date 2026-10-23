//! Checks against `testdata/internal/golden.json`
//! (`native/port/parity/gen_internal_fixtures.py`).

use std::sync::{Arc, OnceLock};

use axum::{
    Router,
    body::Body,
    http::{HeaderName, HeaderValue, Method, Request, StatusCode},
};
use chrono::{DateTime, Utc};
use mongodb::bson::{Bson, Document};
use serde_json::Value;
use tower::ServiceExt;

use super::{
    internal_key_ok,
    pageviews::{Batch, updates, view_docs},
    parse_countries,
};
use crate::{
    config::Settings,
    db::Db,
    middleware::{self, EmbedSource, Mw, MwConfig, forwarded::TrustedHosts},
    state::{AppState, Inner},
    validation::{BodyInput, validate},
};

fn golden() -> &'static Value {
    static G: OnceLock<Value> = OnceLock::new();
    G.get_or_init(|| serde_json::from_str(include_str!("../../testdata/internal/golden.json")).expect("fixture"))
}

fn s(v: &Value) -> &str {
    v.as_str().unwrap_or_else(|| panic!("string: {v}"))
}

fn latin1(s: &str) -> Vec<u8> {
    s.chars().map(|c| c as u32 as u8).collect()
}

fn state() -> AppState {
    let vars: Vec<(String, String)> =
        golden()["env"].as_object().unwrap().iter().map(|(k, v)| (k.clone(), s(v).to_owned())).collect();
    let settings = Settings::from_vars(vars).expect("settings");
    AppState { inner: Arc::new(Inner { settings, db: Db { mongo: None, redis: None, pg: None } }) }
}

fn app() -> Router {
    let mw =
        Mw::new(MwConfig::from_lookup(&|_| None), None, EmbedSource::Fixed(vec![]), TrustedHosts::parse("127.0.0.1"));
    let inner = middleware::wrap(super::router(), Arc::new(mw), None)
        .fallback(|| async { StatusCode::IM_A_TEAPOT })
        .with_state(state());
    middleware::outer(inner)
}

fn case(name: &str) -> &'static Value {
    golden()["http"].as_array().unwrap().iter().find(|c| c["name"] == name).unwrap_or_else(|| panic!("{name}"))
}

#[tokio::test]
async fn http_replay() {
    let app = app();
    let mut bad = Vec::new();
    for c in golden()["http"].as_array().unwrap() {
        let mut req = Request::builder()
            .method(Method::from_bytes(s(&c["method"]).as_bytes()).unwrap())
            .uri(s(&c["url"]))
            .body(Body::from(c["body"].as_str().unwrap().as_bytes().to_vec()))
            .unwrap();
        for pair in c["headers"].as_array().unwrap() {
            req.headers_mut().append(
                HeaderName::from_bytes(s(&pair[0]).as_bytes()).unwrap(),
                HeaderValue::from_bytes(&latin1(s(&pair[1]))).unwrap(),
            );
        }
        req.headers_mut().insert("x-request-id", HeaderValue::from_str(s(&c["request_id"])).unwrap());
        let resp = app.clone().oneshot(req).await.unwrap();
        let status = resp.status().as_u16();
        let allow = resp.headers().get("allow").map(|v| v.to_str().unwrap().to_owned());
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let body = String::from_utf8(body.to_vec()).unwrap();
        let name = s(&c["name"]);
        let want_allow =
            c["resp_headers"].as_array().unwrap().iter().find(|h| h[0] == "allow").map(|h| s(&h[1]).to_owned());
        if u64::from(status) != c["status"].as_u64().unwrap() || body != s(&c["resp"]) || allow != want_allow {
            bad.push(format!(
                "{name}: {status} {allow:?} {body}\n  want {} {want_allow:?} {}",
                c["status"],
                s(&c["resp"])
            ));
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

fn ext(docs: &[Document]) -> Value {
    let arr: Vec<Value> = docs.iter().map(|d| Bson::Document(d.clone()).into_canonical_extjson()).collect();
    Value::Array(arr)
}

fn now() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s(&golden()["now"])).unwrap().with_timezone(&Utc)
}

#[test]
fn recorded_writes() {
    for name in ["ingest_ok", "ingest_extra_fields", "ingest_empty"] {
        let c = case(name);
        let body: Value = serde_json::from_str(s(&c["body"])).unwrap();
        let batch: Batch = validate(&BodyInput::Json(body)).unwrap();
        let views = view_docs(&batch.events, now());
        let want_views = c["inserted"].as_array().unwrap().first().map(|i| i["docs"].clone());
        assert_eq!(want_views, (!views.is_empty()).then(|| ext(&views)), "{name} views");
        let ops: Vec<Value> = updates(&batch.events, now())
            .into_iter()
            .map(|(pid, op)| {
                serde_json::json!({"filter": {"pid": pid}, "update": Bson::Document(op).into_canonical_extjson()})
            })
            .collect();
        let want_ops = c["updates"].as_array().unwrap().first().map(|u| u["ops"].clone());
        assert_eq!(want_ops, (!ops.is_empty()).then_some(Value::Array(ops)), "{name} updates");
    }
}

#[test]
fn countries() {
    for pair in golden()["countries"].as_array().unwrap() {
        let want: Vec<String> = serde_json::from_value(pair[1].clone()).unwrap();
        assert_eq!(parse_countries(s(&pair[0])), want, "{}", pair[0]);
    }
}

#[test]
fn key_check() {
    assert_eq!(internal_key_ok("", "anything").ok(), Some(false));
    assert_eq!(internal_key_ok("abc", "abc").ok(), Some(true));
    assert_eq!(internal_key_ok("abc", "abd").ok(), Some(false));
    assert_eq!(internal_key_ok("abc", "ab").ok(), Some(false));
    assert_eq!(internal_key_ok("abc", "").ok(), Some(false));
    assert!(internal_key_ok("abc", "\u{e9}").is_err());
    assert_eq!(internal_key_ok("", "\u{e9}").ok(), Some(false));
}

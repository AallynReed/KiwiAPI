//! Byte checks against `testdata/openapi/golden.json` (sha256 of the real
//! Python `/openapi.json` body per feature-flag combination) and freshness of
//! the committed base spec, all from native/port/parity/gen_openapi.py.

use std::sync::{Arc, OnceLock};

use axum::{
    Router,
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tower::ServiceExt;

use super::{BASE, OPS, Ops, Spec};
use crate::{
    config::Settings,
    db::Db,
    features, middleware,
    middleware::{EmbedSource, Mw, MwConfig, forwarded::TrustedHosts},
    runtime_config,
    state::{AppState, Inner},
};

fn golden() -> &'static Value {
    static G: OnceLock<Value> = OnceLock::new();
    G.get_or_init(|| serde_json::from_str(include_str!("../../testdata/openapi/golden.json")).expect("golden"))
}

fn routes_snapshot() -> Value {
    serde_json::from_str(include_str!("../../testdata/openapi/routes.json")).expect("routes.json")
}

fn env(key: &str) -> &'static str {
    golden()["env"][key].as_str().expect("env")
}

fn golden_spec() -> &'static Spec {
    static S: OnceLock<Spec> = OnceLock::new();
    S.get_or_init(|| Spec::build(env("APP_NAME"), env("API_URL")))
}

fn sha(b: &[u8]) -> String {
    hex::encode(Sha256::digest(b))
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array().expect("array").iter().map(|s| s.as_str().expect("str").to_owned()).collect()
}

fn settings() -> Settings {
    Settings::from_vars([("APP_NAME", env("APP_NAME").to_owned()), ("API_URL", env("API_URL").to_owned())])
        .expect("settings")
}

fn app() -> Router {
    let db = Db { mongo: None, redis: None, pg: None };
    let state = AppState { inner: Arc::new(Inner { settings: settings(), db }) };
    let mw =
        Mw::new(MwConfig::from_lookup(&|_| None), None, EmbedSource::Fixed(vec![]), TrustedHosts::parse("127.0.0.1"));
    let inner = middleware::wrap(super::router(), Arc::new(mw), None)
        .fallback(|| async { StatusCode::IM_A_TEAPOT })
        .with_state(state);
    middleware::outer(inner)
}

async fn send(method: Method, path: &str) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
    let req = Request::builder().method(method).uri(path).body(Body::empty()).unwrap();
    let res = app().oneshot(req).await.unwrap();
    let (parts, body) = res.into_parts();
    let bytes = axum::body::to_bytes(body, usize::MAX).await.unwrap();
    (parts.status, parts.headers, bytes.to_vec())
}

#[test]
fn every_flag_combination_matches_python() {
    let g = golden();
    let spec = golden_spec();
    assert_eq!(spec.flags(), strings(&g["flags"]).as_slice());
    assert_eq!(sha(&spec.render(&[])), g["marked_sha256"].as_str().unwrap(), "markers");
    let cases = g["cases"].as_array().unwrap();
    assert!(cases.len() > 30);
    for case in cases {
        let disabled = strings(&case["disabled"]);
        let body = spec.render(&disabled);
        assert_eq!(body.len() as u64, case["len"].as_u64().unwrap(), "{}", case["name"]);
        assert_eq!(sha(&body), case["sha256"].as_str().unwrap(), "{}", case["name"]);
        assert_eq!(spec.render(&disabled), body, "cached render {}", case["name"]);
    }
}

#[test]
fn registry_defaults_match_python_defaults() {
    let s = settings();
    let defaults = golden()["cases"].as_array().unwrap().iter().find(|c| c["name"] == "defaults").unwrap();
    let off: Vec<String> = golden_spec()
        .flags()
        .iter()
        .filter(|f| !features::py_truthy(&runtime_config::spec(f).expect("flag registered").default_value(&s)))
        .cloned()
        .collect();
    assert_eq!(off, strings(&defaults["disabled"]));
}

#[tokio::test]
async fn served_with_default_flags() {
    let defaults = golden()["cases"].as_array().unwrap().iter().find(|c| c["name"] == "defaults").unwrap();
    let (status, headers, body) = send(Method::GET, "/openapi.json").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers[header::CONTENT_TYPE], "application/json");
    assert_eq!(sha(&body), defaults["sha256"].as_str().unwrap());
    assert_eq!(headers[header::CONTENT_LENGTH], body.len().to_string().as_str());

    let (status, headers, head_body) = send(Method::HEAD, "/openapi.json").await;
    assert_eq!(status, StatusCode::OK);
    assert!(head_body.is_empty());
    assert_eq!(headers[header::CONTENT_LENGTH], body.len().to_string().as_str());

    let (status, headers, _) = send(Method::POST, "/openapi.json").await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(headers[header::ALLOW], "GET");
}

#[test]
fn docs_pages_read_this_spec() {
    let swagger = crate::routes::leaf::meta::swagger_html("x");
    let redoc = crate::routes::leaf::meta::redoc_html("x");
    assert!(swagger.contains("url: '/openapi.json'"));
    assert!(redoc.contains("<redoc spec-url=\"/openapi.json\"></redoc>"));
}

/// `{name:conv}` -> `{name}`, the `path_format` the spec is keyed by.
fn path_format(path: &str) -> String {
    regex::Regex::new(r"\{(\w+):\w+\}").unwrap().replace_all(path, "{$1}").into_owned()
}

#[test]
fn ops_and_snapshot_agree_with_base() {
    let base: Value = serde_json::from_str(BASE).unwrap();
    let ops: Ops = serde_json::from_str(OPS).unwrap();
    let paths = base["paths"].as_object().unwrap();
    let has = |p: &str, m: &str| paths.get(p).and_then(|i| i.get(m)).is_some();
    for (p, m) in ops.tokenless.iter().chain(&ops.master) {
        assert!(has(p, m), "{m} {p}");
    }
    for (p, m, f) in &ops.gated {
        assert!(has(p, m), "{m} {p}");
        assert!(runtime_config::spec(f).is_some(), "{f}");
    }
    let mut served = std::collections::HashSet::new();
    for r in routes_snapshot().as_array().unwrap() {
        for m in r["methods"].as_array().unwrap() {
            served.insert((path_format(r["path"].as_str().unwrap()), m.as_str().unwrap().to_lowercase()));
        }
    }
    for (p, item) in paths {
        for m in item.as_object().unwrap().keys() {
            assert!(served.contains(&(p.clone(), m.clone())), "{m} {p} is in base.json but no Python route serves it");
        }
    }
}

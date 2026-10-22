//! Gates without a database: every flag reads its registry default.

use std::sync::Arc;

use axum::{Router, body::Body, http::Request, middleware, routing::get};
use http_body_util::BodyExt;
use hyper_util::{client::legacy::Client, rt::TokioExecutor};
use tower::ServiceExt;

use super::*;
use crate::{config::Settings, db::Db, state::Inner};

fn state() -> AppState {
    let settings = Settings::from_vars([("PY_UPSTREAM", "http://py:8000")]).unwrap();
    let upstream = Client::builder(TokioExecutor::new()).build_http();
    AppState { inner: Arc::new(Inner { settings, db: Db { mongo: None, redis: None, pg: None }, upstream }) }
}

/// Stands in for an auth extractor: it always rejects with 401.
struct Unauthorized;

impl FromRequestParts<AppState> for Unauthorized {
    type Rejection = StatusCode;

    async fn from_request_parts(_: &mut Parts, _: &AppState) -> Result<Self, StatusCode> {
        Err(StatusCode::UNAUTHORIZED)
    }
}

async fn send(app: Router, method: &str, uri: &str) -> (StatusCode, Value) {
    let req = Request::builder().method(method).uri(uri).body(Body::empty()).unwrap();
    let res = app.oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

fn disabled_body(name: &str) -> Value {
    serde_json::json!({"error": {"code": "not_found", "message": format!("The {name} is currently disabled."),
                                 "details": null, "request_id": "-"}})
}

#[tokio::test]
async fn extractor_gates_before_auth() {
    let app = Router::new()
        .route("/off", get(|_: RequireFeature<UnlockDebug>, _: Unauthorized| async { "x" }))
        .route("/on", get(|_: RequireFeature<Market>, _: Unauthorized| async { "x" }))
        .with_state(state());
    let (status, body) = send(app.clone(), "GET", "/off").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body, disabled_body("Unlock Debug patcher"));
    let (status, _) = send(app, "GET", "/on").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn method_route_layer_gates_matched_methods_only() {
    let state = state();
    let off = middleware::from_fn_with_state(state.clone(), gate::<UnlockDebug>);
    let on = middleware::from_fn_with_state(state.clone(), gate::<Market>);
    let app = Router::new()
        .route("/x", get(|| async { "ok" }).route_layer(off))
        .route("/on", get(|| async { "ok" }).route_layer(on))
        .with_state(state);
    let (status, body) = send(app.clone(), "GET", "/x").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body, disabled_body("Unlock Debug patcher"));
    let (status, _) = send(app.clone(), "HEAD", "/x").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = send(app.clone(), "POST", "/x").await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
    let (status, _) = send(app.clone(), "GET", "/on").await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = send(app, "GET", "/other").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body, Value::Null);
}

#[tokio::test]
async fn router_route_layer_also_gates_wrong_methods() {
    let state = state();
    let app = Router::new()
        .route("/x", get(|| async { "ok" }))
        .route_layer(middleware::from_fn_with_state(state.clone(), gate::<UnlockDebug>))
        .with_state(state);
    let (status, body) = send(app, "POST", "/x").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body, disabled_body("Unlock Debug patcher"));
}

#[tokio::test]
async fn flags_read_registry_defaults() {
    let state = state();
    assert!(is_enabled(&state, MARKET_FLAG).await.unwrap());
    assert!(!is_enabled(&state, DELVES_FLAG).await.unwrap());
    assert!(!is_enabled(&state, UNLOCK_DEBUG_FLAG).await.unwrap());
    assert!(is_enabled(&state, "feature_nope").await.is_err());
    assert_eq!(runtime_config::get_rate_limit(&state, "api_rate_limit").await.unwrap(), (120, 60));
    let err = runtime_config::get_setting(&state, "nope").await.unwrap_err();
    assert!(err.downcast_ref::<runtime_config::UnknownSetting>().is_some());
}

#[test]
fn every_flag_is_registered() {
    for (flag, _) in GATES {
        let spec = runtime_config::spec(flag).unwrap_or_else(|| panic!("{flag}"));
        assert_eq!(spec.ty, runtime_config::Ty::Bool);
    }
    let in_registry = runtime_config::REGISTRY.iter().filter(|s| s.key.starts_with("feature_")).count();
    assert_eq!(in_registry, 49);
    assert_eq!(Market::FLAG, MARKET_FLAG);
}

#[test]
fn truthiness() {
    use serde_json::json;
    for (v, want) in [
        (json!(null), false),
        (json!(false), false),
        (json!(0), false),
        (json!(0.0), false),
        (json!(""), false),
        (json!([]), false),
        (json!({}), false),
        (json!(true), true),
        (json!(-1), true),
        (json!(0.5), true),
        (json!("false"), true),
        (json!([0]), true),
    ] {
        assert_eq!(py_truthy(&v), want, "{v}");
    }
}

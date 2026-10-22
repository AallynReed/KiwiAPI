//! Extractors end to end through an axum router. Every path asserted here
//! answers before any store is touched; run without MONGO_URI/REDIS_URL a
//! store access would surface as a 500.

use axum::{
    body::Body,
    http::{Request, StatusCode},
    routing::get,
    Router,
};
use chrono::{Duration, Utc};
use http_body_util::BodyExt;
use serde_json::Value;
use tower::ServiceExt;

use super::{jwt, scopes, CurrentSiteUser, CurrentUser, MasterIngest, OptionalSiteUser, RequireScope, TokenAuth};
use crate::{config::Settings, state::AppState};

async fn state() -> AppState {
    if std::env::var("PY_UPSTREAM").is_err() {
        // SAFETY: set once before any thread reads it; the value is constant.
        unsafe { std::env::set_var("PY_UPSTREAM", "http://127.0.0.1:9") };
    }
    AppState::new(Settings::from_env().unwrap()).await.unwrap()
}

async fn app() -> Router {
    Router::new()
        .route("/portal", get(|_: CurrentUser| async { "ok" }))
        .route("/token", get(|_: TokenAuth| async { "ok" }))
        .route("/scoped", get(|_: RequireScope<{ scopes::STATS_READ }>| async { "ok" }))
        .route("/ingest", get(|_: MasterIngest| async { "ok" }))
        .route("/site", get(|_: CurrentSiteUser| async { "ok" }))
        .route(
            "/maybe",
            get(|u: OptionalSiteUser| async move {
                if u.0.is_some() {
                    "user"
                } else {
                    "anon"
                }
            }),
        )
        .with_state(state().await)
}

async fn call(path: &str, headers: &[(&str, &str)]) -> (StatusCode, Option<String>, Value) {
    let mut req = Request::get(path);
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let resp = app().await.oneshot(req.body(Body::empty()).unwrap()).await.unwrap();
    let status = resp.status();
    let www = resp.headers().get("www-authenticate").map(|v| v.to_str().unwrap().to_owned());
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body = serde_json::from_slice(&bytes).unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into()));
    (status, www, body)
}

fn assert_401(r: &(StatusCode, Option<String>, Value), message: &str) {
    assert_eq!(r.0, StatusCode::UNAUTHORIZED, "{r:?}");
    assert_eq!(r.1.as_deref(), Some("Bearer"));
    assert_eq!(r.2["error"]["code"], "not_authenticated");
    assert_eq!(r.2["error"]["message"], message);
}

fn secret() -> &'static str {
    &super::auth_settings().secret_key
}

fn site_token() -> String {
    jwt::create_site_access_token_at(secret(), "65f0a1b2c3d4e5f601234567", 0, None, Duration::minutes(15), Utc::now())
}

fn portal_token() -> String {
    jwt::create_access_token_at(secret(), "65f0a1b2c3d4e5f601234567", 0, None, Duration::minutes(15), Utc::now())
}

#[tokio::test]
async fn missing_credentials() {
    assert_401(&call("/portal", &[]).await, "Authentication required");
    assert_401(&call("/token", &[]).await, "API token required");
    assert_401(&call("/scoped", &[("authorization", "Basic abc")]).await, "API token required");
    assert_401(&call("/ingest", &[("authorization", "Bearer")]).await, "Authentication required");
    assert_401(&call("/site", &[]).await, "Authentication required");
}

#[tokio::test]
async fn bad_checksum_is_rejected_offline() {
    let (full, _, _) = super::tokens::generate_api_token("kiwi");
    let mut bad = full.clone();
    let last = bad.pop().unwrap();
    bad.push(if last == 'a' { 'b' } else { 'a' });
    let auth = format!("Bearer {bad}");
    assert_401(&call("/token", &[("authorization", &auth)]).await, "Invalid or revoked API token");
    assert_401(&call("/scoped", &[("authorization", &auth)]).await, "Invalid or revoked API token");
    assert_401(&call("/ingest", &[("authorization", &auth)]).await, "Invalid or revoked API token");
}

#[tokio::test]
async fn session_jwt_checks_before_lookup() {
    let expired =
        jwt::create_access_token_at(secret(), "x", 0, None, Duration::minutes(15), Utc::now() - Duration::hours(1));
    assert_401(
        &call("/portal", &[("authorization", &format!("Bearer {expired}"))]).await,
        "Invalid or expired session token",
    );
    assert_401(&call("/portal", &[("authorization", "Bearer a.b.c")]).await, "Invalid or expired session token");
    let site = format!("Bearer {}", site_token());
    assert_401(&call("/portal", &[("authorization", &site)]).await, "Wrong-audience token");
    assert_401(&call("/ingest", &[("authorization", &site)]).await, "Wrong-audience token");
    let portal = format!("Bearer {}", portal_token());
    assert_401(&call("/site", &[("authorization", &portal)]).await, "Wrong-audience token");
}

#[tokio::test]
async fn site_cookie_needs_an_allowed_origin() {
    let cookie = format!("kiwi_site_access={}", portal_token());
    assert_401(&call("/site", &[("cookie", &cookie)]).await, "Authentication required");
    assert_401(
        &call("/site", &[("cookie", &cookie), ("origin", "https://docs.aallyn.net")]).await,
        "Authentication required",
    );
    let origin = super::site::allowed_cookie_origins(super::auth_settings()).remove(0);
    assert_401(&call("/site", &[("cookie", &cookie), ("origin", &origin)]).await, "Wrong-audience token");
    assert_401(&call("/site", &[("cookie", &cookie), ("sec-fetch-site", "same-origin")]).await, "Wrong-audience token");
}

#[tokio::test]
async fn optional_site_user_swallows_errors() {
    let (status, _, body) = call("/maybe", &[("authorization", "Bearer junk")]).await;
    assert_eq!((status, body), (StatusCode::OK, Value::String("anon".into())));
    let (status, _, body) = call("/maybe", &[]).await;
    assert_eq!((status, body), (StatusCode::OK, Value::String("anon".into())));
}

//! Checks against `testdata/site_auth/golden.json`, captured from the real
//! Python helpers and app by `native/port/parity/gen_site_auth_fixtures.py`.

use std::sync::{Arc, OnceLock};

use axum::{
    Router,
    body::Body,
    http::{HeaderName, HeaderValue, Method, Request, StatusCode},
    response::Response,
};
use chrono::{TimeZone, Utc};
use kiwi_core::pystr;
use mongodb::bson::{Bson, Document, doc, oid::ObjectId};
use serde_json::Value;
use tower::ServiceExt;

use super::{
    email, oauth,
    store::{self, to_public},
    usernames,
};
use crate::{
    auth::{auth_settings, models::SiteUser},
    config::Settings,
    db::Db,
    dt,
    middleware::{self, EmbedSource, Mw, MwConfig, forwarded::TrustedHosts},
    pyjson,
    state::{AppState, Inner},
};

fn golden() -> &'static Value {
    static G: OnceLock<Value> = OnceLock::new();
    G.get_or_init(|| serde_json::from_str(include_str!("../../testdata/site_auth/golden.json")).expect("fixture"))
}

fn arr(key: &str) -> &'static Vec<Value> {
    golden()[key].as_array().unwrap_or_else(|| panic!("fixture section {key}"))
}

fn s(v: &Value) -> &str {
    v.as_str().unwrap_or_else(|| panic!("string: {v}"))
}

fn report(section: &str, bad: Vec<String>) {
    assert!(bad.is_empty(), "{section}: {} mismatches:\n{}", bad.len(), bad.join("\n"));
}

#[test]
fn emails_validate_like_pydantic() {
    let mut bad = Vec::new();
    for case in arr("emails") {
        let input = s(&case["in"]);
        let got = email::validate(pystr::strip(input)).map(|e| e.to_lowercase());
        let want = case["out"].as_str().map(str::to_owned);
        if got != want {
            bad.push(format!("{input:?}: got {got:?} want {want:?}"));
        }
    }
    report("emails", bad);
}

#[test]
fn safe_next_matches_urlsplit() {
    let mut bad = Vec::new();
    for case in arr("safe_next") {
        let got = oauth::safe_next(case["in"].as_str());
        let ok = match (&got, case.get("out")) {
            (Ok(g), Some(w)) => g == s(w),
            (Err(_), None) => true,
            _ => false,
        };
        if !ok {
            bad.push(format!("{:?}: got {got:?} want {case}", case["in"]));
        }
    }
    report("safe_next", bad);
}

async fn body_text(resp: Response) -> String {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.expect("body");
    String::from_utf8(bytes.to_vec()).expect("utf-8")
}

#[tokio::test]
async fn emit_matches_python() {
    let mut bad = Vec::new();
    let app_url = s(&golden()["env"]["APP_URL"]);
    for case in arr("emit") {
        let result = match (case["ok"].as_str(), case["error"].as_str()) {
            (Some(ok), _) => Ok(ok),
            (None, Some(e)) => Err(e),
            _ => panic!("case {case}"),
        };
        let resp = oauth::emit(app_url, s(&case["mode"]) == "app", result, s(&case["next"]));
        let status = resp.status().as_u16();
        let headers = resp.headers().clone();
        let body = body_text(resp).await;
        if u64::from(status) != case["status"].as_u64().expect("status") {
            bad.push(format!("{case}: status {status}"));
        }
        for pair in case["headers"].as_array().expect("headers") {
            let name = s(&pair[0]);
            if name == "content-length" {
                continue;
            }
            let ours = headers.get(name).map(|v| v.to_str().unwrap_or("").to_owned());
            if ours.as_deref() != Some(s(&pair[1])) {
                bad.push(format!("{}: {name} {ours:?} want {}", case["next"], pair[1]));
            }
        }
        if body != s(&case["body"]) {
            bad.push(format!("{}: body\n  got  {body}\n  want {}", case["next"], case["body"]));
        }
    }
    report("emit", bad);
}

#[test]
fn usernames_normalize_like_python() {
    let mut bad = Vec::new();
    for case in arr("usernames") {
        let got = usernames::normalize(s(&case["in"]));
        let ok = match (&got, case.get("out")) {
            (Ok(g), Some(w)) => g == s(w),
            (Err(e), None) => {
                u64::from(e.status.as_u16()) == case["status"].as_u64().expect("status")
                    && e.code == s(&case["code"])
                    && e.message == s(&case["message"])
            }
            _ => false,
        };
        if !ok {
            bad.push(format!("{case}: got {got:?}"));
        }
    }
    report("usernames", bad);
}

#[test]
fn username_base_matches_python() {
    let mut bad = Vec::new();
    for case in arr("unique_username") {
        let got = oauth::username_base(case["in"].as_str().unwrap_or(""));
        if got != s(&case["out"]) {
            bad.push(format!("{:?}: got {got:?} want {}", case["in"], case["out"]));
        }
    }
    report("unique_username", bad);
}

#[test]
fn discord_token_ttl_matches_python() {
    for case in arr("token_ttl") {
        assert_eq!(oauth::token_ttl(&case["in"]), case["out"].as_i64().expect("ttl"), "{case}");
    }
}

fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32, sec: u32, ms: i64) -> mongodb::bson::DateTime {
    let t = Utc.with_ymd_and_hms(y, mo, d, h, mi, sec).single().expect("time") + chrono::Duration::milliseconds(ms);
    dt::to_bson(t)
}

fn user(username: &str) -> SiteUser {
    SiteUser {
        id: ObjectId::parse_str("65a1b2c3d4e5f60718293a4b").expect("oid"),
        username: username.into(),
        discord_handle: String::new(),
        notify_email: None,
        display_name: None,
        is_active: true,
        is_verified: false,
        is_deleted: false,
        discord_id: None,
        discord_avatar: None,
        token_version: 0,
        claimed_trove_name: None,
        claimed_trove_display: None,
        claimed_at: None,
        claim_verified: false,
        claim_baseline: Document::new(),
        claim_verified_at: None,
        creator_token_hash: None,
        creator_token_prefix: None,
        creator_token_at: None,
        mod_issues_seen_at: None,
        created_at: utc(2026, 3, 5, 0, 0, 0, 0),
        updated_at: utc(2026, 3, 5, 0, 0, 0, 0),
        last_login_at: None,
    }
}

#[test]
fn public_view_matches_pydantic() {
    let t0 = utc(2026, 3, 4, 5, 6, 7, 123);
    let t1 = utc(2026, 3, 5, 0, 0, 0, 0);
    let mut a = user("kiwi");
    a.discord_id = Some(123456789012345678);
    a.discord_avatar = Some("abc123".into());
    a.created_at = t0;
    let mut b = a.clone();
    b.discord_handle = "Kiwi.Live".into();
    b.discord_avatar = Some("a_anim".into());
    b.display_name = Some("Kíwi \"Q\"".into());
    b.notify_email = Some("k@example.com".into());
    b.claimed_trove_name = Some("kiwi".into());
    b.claimed_trove_display = Some("Kiwi".into());
    b.claimed_at = Some(t0);
    b.claim_verified = true;
    b.claim_verified_at = Some(t1);
    b.claim_baseline = doc! { "1": 2.0, "33": 5.5 };
    b.last_login_at = Some(t1);
    let mut c = user("nobody");
    c.is_verified = true;
    let mut d = user("zero");
    d.discord_id = Some(4194304 * 5);
    d.discord_avatar = Some(String::new());
    for (u, case) in [a, b, c, d].iter().zip(arr("public")) {
        assert_eq!(pyjson::pydantic(&to_public(u, None)), s(&case["public"]));
    }
}

#[test]
fn username_request_dto_matches_python() {
    let base = usernames::Request {
        id: ObjectId::parse_str("65a1b2c3d4e5f60718293a4c").expect("oid"),
        site_user_id: ObjectId::parse_str("65a1b2c3d4e5f60718293a4b").expect("oid"),
        current_username: "kiwi".into(),
        requested_username: "kiwi2".into(),
        status: "pending".into(),
        reason: String::new(),
        resolved_by: None,
        resolved_at: None,
        created_at: utc(2026, 3, 4, 5, 6, 7, 123),
    };
    let rejected = usernames::Request {
        id: ObjectId::parse_str("65a1b2c3d4e5f60718293a4d").expect("oid"),
        requested_username: "k3".into(),
        status: "rejected".into(),
        reason: "no".into(),
        resolved_at: Some(utc(2026, 3, 4, 5, 6, 7, 123)),
        created_at: utc(2026, 3, 5, 0, 0, 0, 0),
        ..base.clone()
    };
    for (r, want) in [base, rejected].iter().zip(arr("dto")) {
        assert_eq!(pyjson::dumps_spaced(&usernames::dto(r, None)), s(want));
    }
}

#[test]
fn user_doc_is_beanie_shaped() {
    let mut u = user("kiwi");
    u.discord_id = Some(123456789012345678);
    u.token_version = 3;
    u.claim_baseline = doc! { "7": 5_i32, "8": 2.5 };
    let d = store::user_doc(&u);
    let keys: Vec<&str> = d.keys().map(String::as_str).collect();
    assert_eq!(keys[0], "_id");
    assert_eq!(keys.len(), 24);
    assert_eq!(d.get("discord_id"), Some(&Bson::Int64(123456789012345678)));
    assert_eq!(d.get("token_version"), Some(&Bson::Int32(3)));
    assert_eq!(d.get_document("claim_baseline").expect("baseline").get("7"), Some(&Bson::Double(5.0)));
    assert_eq!(d.get("notify_email"), Some(&Bson::Null));
}

#[test]
fn state_values_parse_like_python() {
    assert_eq!(oauth::parse_state(r#"{"mode": "app", "next": "/x?y=1"}"#), Some((true, "/x?y=1".into())));
    assert_eq!(oauth::parse_state(r#"{"mode": "web", "next": ""}"#), Some((false, String::new())));
    assert_eq!(oauth::parse_state("app"), Some((true, String::new())));
    assert_eq!(oauth::parse_state("web"), Some((false, String::new())));
    assert_eq!(oauth::parse_state("[1]"), None);
    assert_eq!(oauth::parse_state(r#"{"next": 5}"#), None);
    assert_eq!(oauth::parse_state(r#"{"next": 0}"#), Some((false, String::new())));
    assert_eq!(oauth::parse_state(r#"{"next": "//[x"}"#), None);
}

// ---------------------------------------------------------------------------
// HTTP, store-free

fn state() -> AppState {
    let env = golden()["env"].as_object().expect("env");
    let vars: Vec<(String, String)> = env.iter().map(|(k, v)| (k.clone(), s(v).to_owned())).collect();
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

#[tokio::test]
async fn store_free_http_matches_python() {
    // The cookie origin pin reads the process-wide auth settings, not the fixture's.
    let fixture_origin = auth_settings().app_url == s(&golden()["env"]["APP_URL"]);
    let fixture_secret = auth_settings().secret_key == s(&golden()["env"]["SECRET_KEY"]);
    let mut bad = Vec::new();
    for case in arr("http") {
        let name = s(&case["name"]);
        if (name == "me_cookie_ok_origin" && !fixture_origin) || (name == "verify_wrong_kind_jwt" && !fixture_secret) {
            continue;
        }
        let mut req = Request::builder()
            .method(Method::from_bytes(s(&case["method"]).as_bytes()).expect("method"))
            .uri(s(&case["url"]));
        for pair in case["headers"].as_array().expect("headers") {
            req = req.header(
                HeaderName::from_bytes(s(&pair[0]).as_bytes()).expect("name"),
                HeaderValue::from_str(s(&pair[1])).expect("value"),
            );
        }
        let body = s(&case["body"]).to_owned();
        let resp = app().oneshot(req.body(Body::from(body)).expect("request")).await.expect("response");
        let status = resp.status().as_u16();
        let headers = resp.headers().clone();
        let text = body_text(resp).await;
        if u64::from(status) != case["status"].as_u64().expect("status") {
            bad.push(format!("{name}: status {status} want {}", case["status"]));
        }
        for h in ["content-type", "www-authenticate", "location", "cache-control", "allow"] {
            let ours = headers.get(h).map(|v| v.to_str().unwrap_or("").to_owned());
            let want = case["resp_headers"].get(h).map(|v| s(v).to_owned());
            if ours != want {
                bad.push(format!("{name}: {h} {ours:?} want {want:?}"));
            }
        }
        if headers.get_all("set-cookie").iter().count() != case["set_cookie"].as_array().map_or(0, Vec::len) {
            bad.push(format!("{name}: set-cookie count"));
        }
        // uvicorn drops a HEAD body on the wire; the raw ASGI capture still has it.
        if s(&case["method"]) != "HEAD" && text != s(&case["resp"]) {
            bad.push(format!("{name}:\n  got  {text}\n  want {}", s(&case["resp"])));
        }
    }
    report("http", bad);
}

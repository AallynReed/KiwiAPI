//! Replays testdata/routing/golden.json (gen_routing_fixtures.py) through both
//! apps in native mode, and checks every Python route reaches a Rust one.

use std::{
    collections::HashMap,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
};

use axum::{
    Router,
    body::Body,
    extract::ConnectInfo,
    http::{HeaderName, HeaderValue, Method, Request},
};
use base64::Engine as _;
use hyper_util::{client::legacy::Client, rt::TokioExecutor};
use serde_json::Value;
use tower::ServiceExt;

use super::*;

fn golden() -> &'static Value {
    static G: OnceLock<Value> = OnceLock::new();
    G.get_or_init(|| {
        let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/routing/golden.json");
        serde_json::from_str(&std::fs::read_to_string(p).expect("golden")).expect("golden parses")
    })
}

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn closed_port() -> String {
    let l = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = l.local_addr().expect("addr");
    drop(l);
    format!("http://{addr}")
}

fn api_app() -> Router {
    let settings = crate::config::Settings::from_vars([("PY_UPSTREAM", closed_port())]).expect("settings");
    let db = crate::db::Db { mongo: None, redis: None, pg: None };
    let upstream = Client::builder(TokioExecutor::new()).build_http();
    let state = crate::state::AppState { inner: Arc::new(crate::state::Inner { settings, db, upstream }) };
    crate::routes::app_with(state, true)
}

async fn web_app() -> Router {
    let geo = std::env::temp_dir().join(format!("kiwi-routing-geo-{}", std::process::id()));
    std::fs::create_dir_all(&geo).expect("geo dir");
    let mut vars: Vec<(String, String)> = golden()["web_env"]
        .as_object()
        .expect("env")
        .iter()
        .map(|(k, v)| (k.clone(), v.as_str().expect("str").to_owned()))
        .collect();
    vars.extend([
        ("SITE_ROOT".into(), repo().join("site").to_string_lossy().into_owned()),
        ("GEOIP_DIR".into(), geo.to_string_lossy().into_owned()),
        ("GAMEDATA_DIR".into(), repo().join("assets/gamedata").to_string_lossy().into_owned()),
        ("INTERNAL_API_URL".into(), closed_port()),
        ("PY_UPSTREAM".into(), closed_port()),
    ]);
    let lookup: HashMap<String, String> = vars.iter().map(|(k, v)| (k.to_lowercase(), v.clone())).collect();
    let settings = crate::config::Settings::from_vars(vars).expect("settings");
    let cfg = crate::middleware::MwConfig::from_lookup(&|k| lookup.get(k).cloned());
    let web = Arc::new(crate::web::Web::new(&settings, cfg));
    let state = crate::state::AppState::new(settings).await.expect("state");
    crate::web::app_with(state, web, true)
}

fn latin1_bytes(s: &str) -> Vec<u8> {
    s.chars().map(|c| c as u32 as u8).collect()
}

async fn replay(app: Router, cases: &Value) {
    let mut failures = Vec::new();
    for case in cases.as_array().expect("cases") {
        let name = case["name"].as_str().expect("name");
        let method = Method::from_bytes(case["method"].as_str().expect("method").as_bytes()).expect("method");
        let mut req = Request::builder().method(method).uri(case["path"].as_str().expect("path"));
        for h in case["headers"].as_array().expect("headers") {
            let (k, v) = (h[0].as_str().expect("k"), h[1].as_str().expect("v"));
            req = req.header(
                HeaderName::from_bytes(k.as_bytes()).expect("name"),
                HeaderValue::from_bytes(&latin1_bytes(v)).expect("value"),
            );
        }
        let peer: SocketAddr = format!("{}:50000", case["peer"].as_str().unwrap_or("10.0.0.9")).parse().expect("peer");
        let req = req.extension(ConnectInfo(peer)).body(Body::empty()).expect("request");
        let res = app.clone().oneshot(req).await.expect("infallible");
        let status = res.status().as_u16();
        let mut got: Vec<(String, String)> = res
            .headers()
            .iter()
            .map(|(k, v)| (k.as_str().to_owned(), v.as_bytes().iter().map(|&b| b as char).collect()))
            .collect();
        let exact = http_body::Body::size_hint(res.body()).exact();
        let body = axum::body::to_bytes(res.into_body(), usize::MAX).await.expect("body").to_vec();
        let is_head = case["method"] == "HEAD";
        if !got.iter().any(|(k, _)| k == "content-length")
            && !is_head
            && !matches!(status, 204 | 304)
            && exact.is_some()
        {
            got.push(("content-length".into(), body.len().to_string()));
        }
        let mut want: Vec<(String, String)> = case["response_headers"]
            .as_array()
            .expect("headers")
            .iter()
            .map(|h| (h[0].as_str().expect("k").to_owned(), h[1].as_str().expect("v").to_owned()))
            .collect();
        if is_head {
            got.retain(|(k, _)| k != "content-length");
            want.retain(|(k, _)| k != "content-length");
        }
        got.sort();
        want.sort();
        let want_body =
            base64::engine::general_purpose::STANDARD.decode(case["body_b64"].as_str().expect("b64")).expect("b64");
        let mut diffs = Vec::new();
        if status != case["status"].as_u64().expect("status") as u16 {
            diffs.push(format!("status {status} != {}", case["status"]));
        }
        if got != want {
            diffs.push(format!("headers\n   got  {got:?}\n   want {want:?}"));
        }
        if body != want_body {
            diffs.push(format!(
                "body\n   got  {:?}\n   want {:?}",
                String::from_utf8_lossy(&body[..body.len().min(300)]),
                String::from_utf8_lossy(&want_body[..want_body.len().min(300)])
            ));
        }
        if !diffs.is_empty() {
            failures.push(format!("{name}: {}", diffs.join("\n  ")));
        }
    }
    assert!(failures.is_empty(), "{} cases differ:\n{}", failures.len(), failures.join("\n"));
}

#[tokio::test]
async fn api_matches_python() {
    replay(api_app(), &golden()["api"]).await;
}

#[tokio::test]
async fn web_matches_python() {
    replay(web_app().await, &golden()["web"]).await;
}

/// The concrete path `coverage_tests` sends for a template.
fn concrete(path: &str) -> String {
    param_re()
        .replace_all(path, |c: &regex::Captures| match c.get(2).map(|m| m.as_str()) {
            Some(":int") => "1".to_owned(),
            Some(":path") => "a/b".to_owned(),
            _ => "x".to_owned(),
        })
        .into_owned()
}

/// Served only while this process owns the warmer (`RUST_OWNS`).
const OWNED_ONLY: [&str; 1] = ["POST /v1/leaderboards/cheaters/recompute"];

async fn unrouted(app: Router, t: &Table, raw: &Value) -> Vec<String> {
    let mut missing = Vec::new();
    for (i, row) in raw.as_array().expect("table").iter().enumerate() {
        let path = row[0].as_str().expect("path");
        let methods: Vec<String> = match row[1].as_array() {
            Some(m) => m.iter().map(|x| x.as_str().expect("m").to_owned()).filter(|m| m != "HEAD").collect(),
            None => vec!["GET".to_owned()],
        };
        let target = if row[1].is_null() { format!("{path}/app.js") } else { concrete(path) };
        for m in methods {
            let method = Method::from_bytes(m.as_bytes()).expect("method");
            let peer: SocketAddr = "203.0.113.9:5000".parse().expect("peer");
            let req = Request::builder()
                .method(method.clone())
                .uri(&target)
                .extension(ConnectInfo(peer))
                .body(Body::empty())
                .expect("req");
            let res = tokio::time::timeout(std::time::Duration::from_secs(10), app.clone().oneshot(req)).await;
            let no_route = match res {
                Ok(Ok(r)) => r.extensions().get::<NoRoute>().is_some(),
                _ => false,
            };
            if no_route || !matches!(t.resolve(&method, &target), Outcome::Full(j) if j <= i) {
                missing.push(format!("{m} {path}"));
            }
        }
    }
    missing
}

#[tokio::test]
async fn every_api_route_reaches_rust() {
    let raw: Value = serde_json::from_str(include_str!("routes.json")).expect("routes");
    let missing = unrouted(api_app(), Table::api(), &raw["api"]).await;
    let missing: Vec<String> = missing.into_iter().filter(|m| !OWNED_ONLY.contains(&m.as_str())).collect();
    assert!(missing.is_empty(), "Python routes with no Rust route: {missing:?}");
}

/// The website pages no Rust route serves yet (they still go to web-py).
#[tokio::test]
#[ignore = "a report for the cutover"]
async fn web_route_gaps() {
    let raw: Value = serde_json::from_str(include_str!("routes.json")).expect("routes");
    let missing = unrouted(web_app().await, Table::web(), &raw["web"]).await;
    println!("{} web routes without a Rust route: {missing:?}", missing.len());
}

#[test]
fn outcomes() {
    let t = Table::api();
    let get = Method::GET;
    assert!(matches!(t.resolve(&get, "/health"), Outcome::Full(_)));
    assert!(matches!(t.resolve(&get, "/health\n"), Outcome::Full(_)));
    assert!(matches!(t.resolve(&Method::PUT, "/health"), Outcome::Partial(i) if t.allow(i) == "GET"));
    assert_eq!(t.resolve(&get, "/health/"), Outcome::Redirect("/health".into()));
    assert_eq!(t.resolve(&get, "/health//"), Outcome::Redirect("/health".into()));
    assert_eq!(t.resolve(&get, "/v1/leaderboards/5/"), Outcome::Redirect("/v1/leaderboards/5".into()));
    assert_eq!(t.resolve(&get, "/v1/leaderboards/x"), Outcome::NotFound);
    assert_eq!(t.resolve(&get, "/"), Outcome::NotFound);
    assert!(matches!(t.resolve(&get, "/git/mods/a/b.git/info/refs"), Outcome::Full(_)));
    assert_eq!(t.resolve(&get, "/git/mods/a/.git/info/refs"), Outcome::NotFound);
    assert!(t.knows("/git/mods/{handle}/{slug}/info/refs") && t.knows("/v1/leaderboards/{uuid}"));
    assert!(!t.knows("/__nowhere/{x}"));
    let w = Table::web();
    assert_eq!(w.resolve(&get, "/static"), Outcome::Redirect("/static/".into()));
    assert!(matches!(w.resolve(&Method::POST, "/static/"), Outcome::Full(_)));
    assert!(w.knows("/static/{*path}"));
}

#[test]
fn proxy_mode_defers() {
    let t = Table::api();
    let uri = |s: &str| s.parse::<Uri>().expect("uri");
    let lb = "/v1/leaderboards/{uuid}";
    assert!(!defers(t, lb, &Method::GET, &uri("/v1/leaderboards/1000")));
    assert!(!defers(t, lb, &Method::HEAD, &uri("/v1/leaderboards/1000")));
    assert!(!defers(t, lb, &Method::GET, &uri("/v1/leaderboards/%31")));
    assert!(!defers(t, lb, &Method::GET, &uri("/v1/leaderboards/9999999999")));
    assert!(defers(t, lb, &Method::GET, &uri("/v1/leaderboards/abc")));
    assert!(!defers(t, lb, &Method::POST, &uri("/v1/leaderboards/1000")));
    assert!(defers(t, lb, &Method::POST, &uri("/v1/leaderboards/warm")));
    assert!(!defers(t, "/site/leaderboards/{uuid}/entries", &Method::GET, &uri("/site/leaderboards/abc/entries")));
    assert!(defers(t, "/git/mods/{handle}/{slug}/info/refs", &Method::GET, &uri("/git/mods/a/b/info/refs")));
    assert!(!defers(t, "/git/mods/{handle}/{slug}/info/refs", &Method::GET, &uri("/git/mods/a/b.git/info/refs")));
    assert!(defers(t, "/tokens/activity", &Method::PATCH, &uri("/tokens/activity")));
    assert!(!defers(t, "/__rust_only", &Method::GET, &uri("/__rust_only")));
}

#[test]
fn encodings() {
    assert_eq!(canonical("/a b/é%/x?#\\\n"), "/a%20b/%C3%A9%25/x%3F%23%5C%0A");
    assert_eq!(canonical("/ok-._~!$&'()*+,;=:@/"), "/ok-._~!$&'()*+,;=:@/");
    assert_eq!(shape("/git/mods/{handle}/{slug}.git/info/refs"), "/git/mods/{}/{}/info/refs");
    assert_eq!(shape("/v1/{p:path}"), "/v1/{*}");
    assert_eq!(shape("/static/{*path}"), "/static/{*}");
    assert_eq!(location("https", Some("Kiwi:0443"), "/a b?c", "x=\"1\""), "https://Kiwi:0443/a%20b%3Fc?x=%221%22");
    assert_eq!(location("http", Some("a/b"), "//x", ""), "http://0.0.0.0:8000//x");
}

#[test]
fn pydantic_int_strings() {
    for c in golden()["pyint"].as_array().expect("pyint") {
        let (s, want) = (c[0].as_str().expect("s"), c[1].as_str().expect("want"));
        let got = match crate::validation::str_as_int(s) {
            Ok(v) => v.to_string(),
            Err(_) if want == "int_parsing" => "int_parsing".to_owned(),
            Err(_) => "error".to_owned(),
        };
        if s.len() <= 30 && want.len() < 30 {
            assert_eq!(got, want, "{s:?}");
        }
    }
}

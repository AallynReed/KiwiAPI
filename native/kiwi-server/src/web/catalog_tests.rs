//! The reference and catalog pages against goldens from the production-pinned
//! Python website (native/port/parity/gen_web_catalog_fixtures.py), replayed
//! through the whole kiwi-web stack with a fake api.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use axum::{
    Router,
    body::Body,
    extract::{ConnectInfo, Request},
    http::{HeaderName, HeaderValue, Method, StatusCode},
    response::IntoResponse,
};
use base64::Engine as _;
use http_body_util::BodyExt;
use serde_json::Value;
use sha2::{Digest, Sha256};
use tower::ServiceExt;

use super::{Web, app, catalog};
use crate::{config::Settings, middleware::MwConfig, state::AppState};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn testdata(dir: &str, name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata").join(dir).join(name)
}

fn data(name: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(testdata("web_catalog", name)).expect("fixture")).expect("parses")
}

fn golden() -> &'static Value {
    static G: OnceLock<Value> = OnceLock::new();
    G.get_or_init(|| data("golden.json"))
}

/// The generator's fingerprint: site/templates, site/static and the game data files.
fn fingerprint() -> String {
    let site = repo().join("site");
    let mut h = Sha256::new();
    for sub in ["templates", "static"] {
        let mut files: Vec<(Vec<String>, PathBuf)> = Vec::new();
        let mut stack = vec![site.join(sub)];
        while let Some(dir) = stack.pop() {
            for e in std::fs::read_dir(&dir).expect("site dir").flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else {
                    let rel = p.strip_prefix(site.join(sub)).expect("under root");
                    let mut key: Vec<String> = vec![sub.to_owned()];
                    key.extend(rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()));
                    if cfg!(windows) {
                        key = key.into_iter().map(|k| k.to_lowercase()).collect();
                    }
                    files.push((key, p));
                }
            }
        }
        files.sort();
        for (_, p) in files {
            let rel = p.strip_prefix(&site).expect("under site");
            let posix: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
            h.update(posix.join("/").as_bytes());
            h.update([0u8]);
            h.update(std::fs::read(&p).expect("read"));
        }
    }
    for name in golden()["gamedata"].as_array().expect("gamedata") {
        let name = name.as_str().expect("name");
        h.update(name.as_bytes());
        h.update([0u8]);
        h.update(std::fs::read(repo().join("assets/gamedata").join(name)).expect("gamedata"));
    }
    hex::encode(h.finalize())
}

fn unchanged() -> bool {
    static OK: OnceLock<bool> = OnceLock::new();
    *OK.get_or_init(|| {
        let same = golden()["fingerprint"].as_str() == Some(fingerprint().as_str());
        if !same {
            eprintln!("site/ or game data changed since the catalog goldens; run gen_web_catalog_fixtures.py");
        }
        same
    })
}

async fn fake_api(scenario: &Value) -> String {
    let map: HashMap<String, Value> =
        scenario.as_object().map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect()).unwrap_or_default();
    let map = Arc::new(map);
    let router = Router::new().fallback(move |req: Request| {
        let map = map.clone();
        async move {
            let key = match req.uri().query() {
                Some(q) => format!("{}?{q}", req.uri().path()),
                None => req.uri().path().to_owned(),
            };
            match map.get(&key) {
                Some(v) => crate::pyjson::json_response(crate::pyjson::dumps(v)),
                None => StatusCode::NOT_FOUND.into_response(),
            }
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move { axum::serve(listener, router).await });
    format!("http://{addr}")
}

async fn closed_port() -> String {
    let l = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = l.local_addr().expect("addr");
    drop(l);
    format!("http://{addr}")
}

struct Rig {
    app: Router,
    geo: PathBuf,
}

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.geo);
    }
}

async fn rig(scenario: &str) -> Rig {
    let upstream = data("upstream.json");
    let api = if scenario == "down" { closed_port().await } else { fake_api(&upstream[scenario]).await };
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let geo = std::env::temp_dir().join(format!("kiwi-web-catalog-{scenario}-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&geo).expect("geo dir");
    for f in ["dbip-country-lite.mmdb", "dbip-asn-lite.mmdb"] {
        std::fs::copy(testdata("web", f), geo.join(f)).expect("mmdb");
    }
    let root = repo();
    let mut vars: Vec<(String, String)> = golden()["env"]
        .as_object()
        .expect("env")
        .iter()
        .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_owned())))
        .collect();
    vars.extend([
        ("SITE_ROOT".into(), root.join("site").to_string_lossy().into_owned()),
        ("GEOIP_DIR".into(), geo.to_string_lossy().into_owned()),
        ("GAMEDATA_DIR".into(), root.join("assets/gamedata").to_string_lossy().into_owned()),
        ("INTERNAL_API_URL".into(), api),
        ("PY_UPSTREAM".into(), "http://127.0.0.1:9".into()),
    ]);
    let lookup: HashMap<String, String> = vars.iter().map(|(k, v)| (k.to_lowercase(), v.clone())).collect();
    let settings = Settings::from_vars(vars.clone()).expect("settings");
    let cfg = MwConfig::from_lookup(&|k| lookup.get(k).cloned());
    let web = Web::new(&settings, cfg);
    web.country_db.load_now();
    web.asn_db.load_now();
    let state = AppState::new(settings).await.expect("state");
    Rig { app: app(state, Arc::new(web)), geo }
}

const COMPARED: [&str; 12] = [
    "content-type",
    "content-length",
    "cache-control",
    "vary",
    "location",
    "content-security-policy",
    "x-frame-options",
    "x-content-type-options",
    "referrer-policy",
    "permissions-policy",
    "allow",
    "x-request-id",
];

fn header<'a>(hs: &'a [(String, String)], name: &str) -> Option<&'a str> {
    hs.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
}

fn unz(b64: &str) -> Vec<u8> {
    use std::io::Read;
    let raw = base64::engine::general_purpose::STANDARD.decode(b64).expect("b64");
    let mut out = Vec::new();
    flate2::read::ZlibDecoder::new(&raw[..]).read_to_end(&mut out).expect("zlib");
    out
}

async fn replay(scenario: &str) {
    if !unchanged() {
        return;
    }
    let rig = rig(scenario).await;
    let mut failures = Vec::new();
    for case in golden()["requests"].as_array().expect("requests").iter().filter(|c| c["scenario"] == scenario) {
        let name = case["name"].as_str().unwrap_or("?");
        let method = Method::from_bytes(case["method"].as_str().unwrap_or("GET").as_bytes()).expect("method");
        let mut builder = Request::builder().method(method).uri(case["path"].as_str().expect("path"));
        for h in case["headers"].as_array().expect("headers") {
            let (k, v) = (h[0].as_str().unwrap_or(""), h[1].as_str().unwrap_or(""));
            builder = builder
                .header(HeaderName::from_bytes(k.as_bytes()).expect("name"), HeaderValue::from_str(v).expect("v"));
        }
        let mut req = builder.body(Body::empty()).expect("request");
        req.extensions_mut().insert(ConnectInfo(SocketAddr::from(([203, 0, 113, 7], 50000))));
        let res = rig.app.clone().oneshot(req).await.expect("infallible");
        let status = res.status();
        let mut headers: Vec<(String, String)> = res
            .headers()
            .iter()
            .map(|(k, v)| (k.as_str().to_owned(), String::from_utf8_lossy(v.as_bytes()).into_owned()))
            .collect();
        let body = res.into_body().collect().await.expect("body").to_bytes().to_vec();
        if header(&headers, "content-length").is_none() && case["method"] != "HEAD" {
            headers.push(("content-length".into(), body.len().to_string()));
        }
        if u64::from(status.as_u16()) != case["status"].as_u64().unwrap_or(0) {
            failures.push(format!("{name}: status {status} != {}", case["status"]));
            continue;
        }
        let want_headers: Vec<(String, String)> = case["resp_headers"]
            .as_array()
            .expect("headers")
            .iter()
            .map(|h| (h[0].as_str().unwrap_or("").to_owned(), h[1].as_str().unwrap_or("").to_owned()))
            .collect();
        for h in COMPARED {
            if h == "content-length" && case["method"] == "HEAD" {
                continue;
            }
            let (got, want) = (header(&headers, h), header(&want_headers, h));
            if got != want {
                failures.push(format!("{name}: {h}: {got:?} != {want:?}"));
            }
        }
        let want = unz(case["resp_body_z"].as_str().unwrap_or(""));
        if case["method"] != "HEAD" && body != want {
            let at = body.iter().zip(&want).position(|(a, b)| a != b).unwrap_or(body.len().min(want.len()));
            let lo = at.saturating_sub(160);
            failures.push(format!(
                "{name}: body differs at {at} (len {} vs {})\n  got:  {:?}\n  want: {:?}",
                body.len(),
                want.len(),
                String::from_utf8_lossy(&body[lo..(at + 160).min(body.len())]),
                String::from_utf8_lossy(&want[lo..(at + 160).min(want.len())]),
            ));
        }
    }
    assert!(failures.is_empty(), "{} differences:\n{}", failures.len(), failures.join("\n"));
}

#[tokio::test]
async fn catalog_on() {
    replay("on").await;
}

#[tokio::test]
async fn catalog_off() {
    replay("off").await;
}

#[tokio::test]
async fn catalog_down() {
    replay("down").await;
}

/// `ssr.cached`: a failed fetch is kept for the TTL when there is nothing older,
/// and an older payload stands in for a later failure.
#[tokio::test]
async fn ssr_cache_keeps_failures_and_last_good() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let hits = Arc::new(AtomicUsize::new(0));
    let up = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (h, u) = (hits.clone(), up.clone());
    let router = Router::new().fallback(move || {
        let (h, u) = (h.clone(), u.clone());
        async move {
            h.fetch_add(1, Ordering::SeqCst);
            if u.load(Ordering::SeqCst) {
                crate::pyjson::json_response(r#"{"items":[1]}"#)
            } else {
                StatusCode::BAD_GATEWAY.into_response()
            }
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move { axum::serve(listener, router).await });
    let api = super::internal::InternalApi::new(&format!("http://{addr}"));
    let cache = catalog::SsrCache::default();
    let p = [("limit", "24".to_owned()), ("sort", "x".to_owned())];
    assert!(cache.get(&api, "/a", &p, 60.0).await.is_none());
    up.store(true, Ordering::SeqCst);
    assert!(cache.get(&api, "/a", &p, 60.0).await.is_none());
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    let good = cache.get(&api, "/a", &p, 0.0).await;
    assert_eq!(good.as_deref(), Some(&serde_json::json!({"items": [1]})));
    up.store(false, Ordering::SeqCst);
    assert_eq!(cache.get(&api, "/a", &p, 0.0).await, good);
    assert_eq!(hits.load(Ordering::SeqCst), 3);
}

/// The reference view models over synthetic game data, raising where Python raises.
#[test]
fn view_models_match_python() {
    let cases = data("views.json");
    for (n, set) in cases["sets"].as_array().expect("sets").iter().enumerate() {
        let name = set["name"].as_str().unwrap_or("?");
        let dir = std::env::temp_dir().join(format!("kiwi-web-views-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        for (file, content) in set["files"].as_object().expect("files") {
            std::fs::write(dir.join(file), serde_json::to_string(content).expect("json")).expect("write");
        }
        let gd = crate::gamedata::Gamedata::new(&dir, None);
        for (view, want) in set["results"].as_object().expect("results") {
            let got = match view.as_str() {
                "classes" => super::reference::classes_view(&gd),
                "abilities" => super::reference::ability_tabs(&gd),
                _ => super::reference::allies_view(&gd),
            };
            match (got, want.get("ok")) {
                (Ok(got), Some(want)) => assert_eq!(&got, want, "{name}: {view}"),
                (Err(_), None) => {}
                (got, _) => panic!("{name}: {view}: got {got:?}, Python {want}"),
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
    let cmd = &cases["commands"];
    assert_eq!(super::reference::commands_view(&cmd["input"]).expect("commands"), cmd["ok"]);
}

/// matchit falls back from the static `/mods/why` to the parameter routes.
#[tokio::test]
async fn mods_why_child_is_a_project() {
    if !unchanged() {
        return;
    }
    let rig = rig("on").await;
    let mut req = Request::get("/mods/why/thing").body(Body::empty()).expect("request");
    req.extensions_mut().insert(ConnectInfo(SocketAddr::from(([203, 0, 113, 7], 50000))));
    let res = rig.app.clone().oneshot(req).await.expect("infallible");
    assert_eq!(res.status(), StatusCode::OK);
    let body = res.into_body().collect().await.expect("body").to_bytes();
    assert!(String::from_utf8_lossy(&body).contains("<meta name=\"mh-handle\" content=\"why\">"));
}

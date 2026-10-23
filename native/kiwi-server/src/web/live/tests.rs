//! The live pages against goldens from the production-pinned Python website
//! (native/port/parity/gen_web_live_fixtures.py): every request replayed
//! through the whole kiwi-web stack over a fake api, with the api reads each
//! request made, plus the ssr formatters over awkward inputs.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use axum::{
    Router,
    body::Body,
    extract::{ConnectInfo, Request},
    http::{HeaderName, HeaderValue, Method, StatusCode},
    response::IntoResponse,
};
use http_body_util::BodyExt;
use serde_json::Value;
use sha2::{Digest, Sha256};
use tower::ServiceExt;

use super::ssr;
use crate::web::{Web, app};
use crate::{config::Settings, middleware::MwConfig, state::AppState};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn data(name: &str) -> Value {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/web_live").join(name);
    serde_json::from_str(&std::fs::read_to_string(p).expect("fixture")).expect("fixture parses")
}

fn golden() -> &'static Value {
    static G: OnceLock<Value> = OnceLock::new();
    G.get_or_init(|| data("golden.json"))
}

/// The generator's fingerprint of `site/templates`, `site/static` and the
/// maintenance page, files ordered the way `pathlib` sorts them.
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
                    continue;
                }
                let rel = p.strip_prefix(site.join(sub)).expect("under root");
                let mut key: Vec<String> = vec![sub.to_owned()];
                key.extend(rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()));
                if cfg!(windows) {
                    key = key.into_iter().map(|k| k.to_lowercase()).collect();
                }
                files.push((key, p));
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
    h.update(std::fs::read(repo().join("assets/maintenance.html")).expect("page"));
    hex::encode(h.finalize())
}

fn site_unchanged() -> bool {
    static OK: OnceLock<bool> = OnceLock::new();
    *OK.get_or_init(|| {
        let same = golden()["fingerprint"].as_str() == Some(fingerprint().as_str());
        if !same {
            eprintln!("site/ changed since the live-page goldens were made; run gen_web_live_fixtures.py");
        }
        same
    })
}

type Seen = Arc<Mutex<Vec<String>>>;

async fn fake_api(scenario: &Value, seen: Seen) -> String {
    let map: HashMap<String, Value> =
        scenario.as_object().map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect()).unwrap_or_default();
    let map = Arc::new(map);
    let router = Router::new().fallback(move |req: Request| {
        let (map, seen) = (map.clone(), seen.clone());
        async move {
            let key = match req.uri().query() {
                Some(q) => format!("{}?{q}", req.uri().path()),
                None => req.uri().path().to_owned(),
            };
            seen.lock().unwrap().push(key.clone());
            match map.get(&key) {
                Some(v) if !v.is_null() => crate::pyjson::json_response(crate::pyjson::dumps(v)),
                _ => StatusCode::NOT_FOUND.into_response(),
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
    seen: Seen,
    geo: PathBuf,
}

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.geo);
    }
}

async fn rig(scenario: &str) -> Rig {
    let seen: Seen = Arc::default();
    let api = if scenario == "down" {
        closed_port().await
    } else {
        fake_api(&data("upstream.json")[scenario], seen.clone()).await
    };
    let geo = std::env::temp_dir().join(format!("kiwi-web-live-{scenario}-{}", std::process::id()));
    std::fs::create_dir_all(&geo).expect("geo dir");
    for f in ["dbip-country-lite.mmdb", "dbip-asn-lite.mmdb"] {
        std::fs::copy(Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/web").join(f), geo.join(f)).expect("mmdb");
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
    ]);
    let lookup: HashMap<String, String> = vars.iter().map(|(k, v)| (k.to_lowercase(), v.clone())).collect();
    let settings = Settings::from_vars(vars.clone()).expect("settings");
    let cfg = MwConfig::from_lookup(&|k| lookup.get(k).cloned());
    let web = Arc::new(Web::new(&settings, cfg));
    let state = AppState::new(settings).await.expect("state");
    Rig { app: app(state, web), seen, geo }
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
    "strict-transport-security",
    "permissions-policy",
    "x-request-id",
];

fn unz(b64: &str) -> Vec<u8> {
    use base64::Engine as _;
    use std::io::Read;
    let raw = base64::engine::general_purpose::STANDARD.decode(b64).expect("b64");
    let mut out = Vec::new();
    flate2::read::ZlibDecoder::new(&raw[..]).read_to_end(&mut out).expect("zlib");
    out
}

/// The auto theme's time of day is the clock's, not the fixture's.
fn untimed(body: &[u8]) -> String {
    let s = String::from_utf8_lossy(body);
    match s.find("data-vx-tod=\"") {
        Some(at) => {
            let start = at + "data-vx-tod=\"".len();
            let end = start + s[start..].find('"').unwrap_or(0);
            format!("{}*{}", &s[..start], &s[end..])
        }
        None => s.into_owned(),
    }
}

/// A `content-length` without the bytes `untimed` masks.
fn timeless_length(value: Option<&str>, body: &[u8]) -> Option<String> {
    let masked = body.len().saturating_sub(untimed(body).len());
    value.map(|v| v.parse::<usize>().map_or_else(|_| v.to_owned(), |n| n.saturating_sub(masked).to_string()))
}

fn header<'a>(hs: &'a [(String, String)], name: &str) -> Option<&'a str> {
    hs.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
}

fn api_reads(seen: &[String]) -> Vec<String> {
    let mut out: Vec<String> = seen
        .iter()
        .filter(|k| k.starts_with("/site/") && *k != "/site/feature-flags" && *k != "/site/embed/allowed-origins")
        .map(|k| k.to_lowercase())
        .collect();
    out.sort();
    out
}

async fn replay(scenario: &str) {
    if !site_unchanged() {
        return;
    }
    let rig = rig(scenario).await;
    let mut failures = Vec::new();
    for case in golden()["requests"].as_array().expect("requests").iter().filter(|c| c["scenario"] == scenario) {
        let name = case["name"].as_str().unwrap_or("?");
        let method = Method::from_bytes(case["method"].as_str().unwrap_or("GET").as_bytes()).expect("method");
        let mut builder = Request::builder().method(method).uri(case["path"].as_str().expect("path"));
        for h in case["headers"].as_array().expect("headers") {
            builder = builder.header(
                HeaderName::from_bytes(h[0].as_str().unwrap_or("").as_bytes()).expect("name"),
                HeaderValue::from_str(h[1].as_str().unwrap_or("")).expect("value"),
            );
        }
        let mut req = builder.body(Body::empty()).expect("request");
        req.extensions_mut().insert(ConnectInfo(SocketAddr::from(([203, 0, 113, 7], 50000))));
        rig.seen.lock().unwrap().clear();
        let res = rig.app.clone().oneshot(req).await.expect("infallible");
        let status = res.status();
        let headers: Vec<(String, String)> = res
            .headers()
            .iter()
            .map(|(k, v)| (k.as_str().to_owned(), String::from_utf8_lossy(v.as_bytes()).into_owned()))
            .collect();
        let body = res.into_body().collect().await.expect("body").to_bytes().to_vec();
        if u64::from(status.as_u16()) != case["status"].as_u64().unwrap_or(0) {
            failures.push(format!("{name}: status {status} != {}", case["status"]));
            continue;
        }
        let want_headers: Vec<(String, String)> = case["resp_headers"]
            .as_array()
            .expect("resp headers")
            .iter()
            .map(|h| (h[0].as_str().unwrap_or("").to_owned(), h[1].as_str().unwrap_or("").to_owned()))
            .collect();
        let want_body = unz(case["resp_body_z"].as_str().unwrap_or(""));
        for h in COMPARED {
            let (got, want) = if h == "content-length" && case["method"] != "HEAD" {
                (timeless_length(header(&headers, h), &body), timeless_length(header(&want_headers, h), &want_body))
            } else {
                (header(&headers, h).map(str::to_owned), header(&want_headers, h).map(str::to_owned))
            };
            if got != want {
                failures.push(format!("{name}: {h}: {got:?} != {want:?}"));
            }
        }
        let want_reads: Vec<String> = api_reads(
            &case["fetched"]
                .as_array()
                .expect("fetched")
                .iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect::<Vec<_>>(),
        );
        let got_reads = api_reads(&rig.seen.lock().unwrap());
        if scenario != "down" && got_reads != want_reads {
            failures.push(format!("{name}: api reads {got_reads:?} != {want_reads:?}"));
        }
        if case["method"] == "HEAD" {
            continue;
        }
        let (got, want) = (untimed(&body), untimed(&unz(case["resp_body_z"].as_str().unwrap_or(""))));
        if got != want {
            let at = got.bytes().zip(want.bytes()).position(|(a, b)| a != b).unwrap_or(got.len().min(want.len()));
            let lo = at.saturating_sub(160);
            let window = |s: &str| String::from_utf8_lossy(&s.as_bytes()[lo..(at + 160).min(s.len())]).into_owned();
            failures.push(format!(
                "{name}: body differs at {at} (len {} vs {})\n  got:  {:?}\n  want: {:?}",
                got.len(),
                want.len(),
                window(&got),
                window(&want),
            ));
        }
    }
    assert!(failures.is_empty(), "{} differences:\n{}", failures.len(), failures.join("\n"));
}

#[tokio::test]
async fn scenario_full() {
    replay("full").await;
}

#[tokio::test]
async fn scenario_odd() {
    replay("odd").await;
}

#[tokio::test]
async fn scenario_off() {
    replay("off").await;
}

#[tokio::test]
async fn scenario_down() {
    replay("down").await;
}

#[test]
fn formatters_match_python() {
    let f = &golden()["formatters"];
    for row in f["num"].as_array().expect("num") {
        assert_eq!(ssr::num(Some(&row[0])), row[1].as_str().unwrap(), "num({})", row[0]);
    }
    for row in f["date"].as_array().expect("date") {
        assert_eq!(ssr::date(Some(&row[0])), row[1].as_str().unwrap(), "date({})", row[0]);
        assert_eq!(ssr::datetime_attr(Some(&row[0])), row[2].as_str().unwrap(), "datetime_attr({})", row[0]);
    }
    for row in f["text"].as_array().expect("text") {
        assert_eq!(ssr::text(Some(&row[0]), 220), row[1].as_str().unwrap(), "text({})", row[0]);
        assert_eq!(ssr::text(Some(&row[0]), 20), row[2].as_str().unwrap(), "text({}, 20)", row[0]);
    }
}

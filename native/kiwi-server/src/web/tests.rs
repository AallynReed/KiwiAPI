//! The website against goldens from the production-pinned Python app
//! (native/port/parity/gen_web_fixtures.py): every scenario's requests replayed
//! through the whole Rust stack with a fake api, plus Jinja2 template cases.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use axum::{
    Router,
    body::Body,
    extract::{ConnectInfo, Request},
    http::{HeaderName, HeaderValue, Method, StatusCode},
    response::{IntoResponse, Response},
};
use base64::Engine as _;
use http_body::Body as _;
use http_body_util::BodyExt;
use serde_json::Value;
use sha2::{Digest, Sha256};
use tower::ServiceExt;

use super::{Web, app, pageviews, templates};
use crate::{config::Settings, middleware::MwConfig, state::AppState};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn data(name: &str) -> Value {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/web").join(name);
    serde_json::from_str(&std::fs::read_to_string(p).expect("fixture")).expect("fixture parses")
}

fn golden() -> &'static Value {
    static G: OnceLock<Value> = OnceLock::new();
    G.get_or_init(|| data("golden.json"))
}

/// The generator's fingerprint of `site/templates`, `site/static` and the
/// maintenance page: path parts compared the way `pathlib` sorts them.
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
    h.update(std::fs::read(repo().join("assets/maintenance.html")).expect("page"));
    hex::encode(h.finalize())
}

fn site_unchanged() -> bool {
    static OK: OnceLock<bool> = OnceLock::new();
    *OK.get_or_init(|| {
        let same = golden()["fingerprint"].as_str() == Some(fingerprint().as_str());
        if !same {
            eprintln!("site/ changed since the web goldens were made; run gen_web_fixtures.py to refresh them");
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
    web: Arc<Web>,
    _geo: tempdir::Dir,
}

mod tempdir {
    pub struct Dir(pub std::path::PathBuf);

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

async fn rig(scenario: &str) -> Rig {
    rig_with(scenario, "http://127.0.0.1:9".into()).await
}

async fn rig_with(scenario: &str, py_upstream: String) -> Rig {
    let upstream = data("upstream.json");
    let api = if scenario == "down" { closed_port().await } else { fake_api(&upstream[scenario]).await };
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let geo = std::env::temp_dir().join(format!("kiwi-web-test-{scenario}-{}-{n}", std::process::id()));
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
        ("PY_UPSTREAM".into(), py_upstream),
    ]);
    let lookup: HashMap<String, String> = vars.iter().map(|(k, v)| (k.to_lowercase(), v.clone())).collect();
    let settings = Settings::from_vars(vars.clone()).expect("settings");
    let cfg = MwConfig::from_lookup(&|k| lookup.get(k).cloned());
    let mut web = Web::new(&settings, cfg);
    web.maintenance_page = std::fs::read_to_string(root.join(super::geo::PAGE_PATH))
        .expect("maintenance page")
        .replace("\r\n", "\n")
        .into();
    web.country_db.load_now();
    web.asn_db.load_now();
    let web = Arc::new(web);
    let state = AppState::new(settings).await.expect("state");
    Rig { app: app(state, web.clone()), web, _geo: tempdir::Dir(geo) }
}

const COMPARED: [&str; 18] = [
    "content-type",
    "content-length",
    "cache-control",
    "vary",
    "location",
    "content-encoding",
    "content-security-policy",
    "x-frame-options",
    "x-content-type-options",
    "referrer-policy",
    "strict-transport-security",
    "permissions-policy",
    "allow",
    "retry-after",
    "accept-ranges",
    "content-range",
    "x-request-id",
    "set-cookie",
];

fn stat_file(path: &str) -> Option<PathBuf> {
    let p = path.split('?').next().unwrap_or(path);
    if let Some(rel) = p.strip_prefix("/static/") {
        return Some(repo().join("site/static").join(rel.replace("%2E", ".")));
    }
    p.strip_prefix("/gamedata/").map(|n| repo().join("assets/gamedata").join(n))
}

fn stat_tags(file: &Path) -> (String, String) {
    let meta = std::fs::metadata(file).expect("stat");
    crate::drops::download::stat_headers(crate::drops::download::st_mtime(meta.modified().expect("mtime")), meta.len())
}

async fn send(rig: &Rig, case: &Value) -> (StatusCode, Vec<(String, String)>, Vec<u8>) {
    let method = Method::from_bytes(case["method"].as_str().unwrap_or("GET").as_bytes()).expect("method");
    let mut builder = Request::builder().method(method).uri(case["path"].as_str().expect("path"));
    for h in case["headers"].as_array().expect("headers") {
        let (k, mut v) = (h[0].as_str().unwrap_or(""), h[1].as_str().unwrap_or("").to_owned());
        if k == "if-none-match"
            && let Some(rel) = case["etag_of"].as_str()
        {
            v = format!("W/{}", stat_tags(&repo().join("site/static").join(rel)).0);
        }
        builder = builder
            .header(HeaderName::from_bytes(k.as_bytes()).expect("name"), HeaderValue::from_str(&v).expect("value"));
    }
    let body = base64::engine::general_purpose::STANDARD.decode(case["body_b64"].as_str().unwrap_or("")).expect("b64");
    let mut req = builder.body(Body::from(body)).expect("request");
    let ip = case["client"].as_str().unwrap_or("203.0.113.7");
    req.extensions_mut().insert(ConnectInfo(SocketAddr::new(ip.parse().expect("ip"), 50000)));
    let res: Response = rig.app.clone().oneshot(req).await.expect("infallible");
    let status = res.status();
    let mut headers: Vec<(String, String)> = res
        .headers()
        .iter()
        .map(|(k, v)| (k.as_str().to_owned(), String::from_utf8_lossy(v.as_bytes()).into_owned()))
        .collect();
    let exact = res.body().size_hint().exact();
    let bytes = res.into_body().collect().await.expect("body").to_bytes().to_vec();
    if !headers.iter().any(|(k, _)| k == "content-length")
        && !matches!(status.as_u16(), 204 | 304)
        && case["method"] != "HEAD"
        && let Some(n) = exact
    {
        headers.push(("content-length".into(), n.to_string()));
    }
    (status, headers, bytes)
}

fn unz(b64: &str) -> Vec<u8> {
    use std::io::Read;
    let raw = base64::engine::general_purpose::STANDARD.decode(b64).expect("b64");
    let mut out = Vec::new();
    flate2::read::ZlibDecoder::new(&raw[..]).read_to_end(&mut out).expect("zlib");
    out
}

fn header<'a>(hs: &'a [(String, String)], name: &str) -> Option<&'a str> {
    hs.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
}

async fn replay(scenario: &str) {
    if !site_unchanged() {
        return;
    }
    let rig = rig(scenario).await;
    let mut failures = Vec::new();
    for case in golden()["requests"].as_array().expect("requests").iter().filter(|c| c["scenario"] == scenario) {
        let name = case["name"].as_str().unwrap_or("?");
        let (status, headers, body) = send(&rig, case).await;
        let want_headers: Vec<(String, String)> = case["resp_headers"]
            .as_array()
            .expect("resp headers")
            .iter()
            .map(|h| (h[0].as_str().unwrap_or("").to_owned(), h[1].as_str().unwrap_or("").to_owned()))
            .collect();
        if status.as_u16() as u64 != case["status"].as_u64().unwrap_or(0) {
            failures.push(format!("{name}: status {status} != {}", case["status"]));
            continue;
        }
        for h in COMPARED {
            let (got, want) = (header(&headers, h), header(&want_headers, h));
            if got != want {
                failures.push(format!("{name}: {h}: {got:?} != {want:?}"));
            }
        }
        if header(&want_headers, "etag").is_some() {
            let file = stat_file(case["path"].as_str().unwrap_or("")).expect("file");
            let (etag, lm) = stat_tags(&file);
            let lm_want = header(&want_headers, "last-modified").map(|_| lm.as_str());
            if header(&headers, "etag") != Some(etag.as_str()) || header(&headers, "last-modified") != lm_want {
                failures.push(format!("{name}: etag/last-modified {:?}", header(&headers, "etag")));
            }
        }
        let want_body = unz(case["resp_body_z"].as_str().unwrap_or(""));
        if case["method"] != "HEAD" && body != want_body {
            let at = body.iter().zip(&want_body).position(|(a, b)| a != b).unwrap_or(body.len().min(want_body.len()));
            let lo = at.saturating_sub(120);
            failures.push(format!(
                "{name}: body differs at {at} (len {} vs {})\n  got:  {:?}\n  want: {:?}",
                body.len(),
                want_body.len(),
                String::from_utf8_lossy(&body[lo..(at + 120).min(body.len())]),
                String::from_utf8_lossy(&want_body[lo..(at + 120).min(want_body.len())]),
            ));
        }
        let got_events = rig.web.beacon.take();
        let want_events = case["events"].as_array().cloned().unwrap_or_default();
        if got_events.len() != want_events.len() {
            failures.push(format!("{name}: {} events != {}", got_events.len(), want_events.len()));
        }
        for (g, w) in got_events.iter().zip(&want_events) {
            let mut g = g.clone();
            if let (Some(gm), Some(wh)) = (g.as_object_mut(), w.get("visitor_hash")) {
                let ua = header(
                    &case["headers"]
                        .as_array()
                        .expect("headers")
                        .iter()
                        .map(|h| (h[0].as_str().unwrap_or("").to_owned(), h[1].as_str().unwrap_or("").to_owned()))
                        .collect::<Vec<_>>(),
                    "user-agent",
                )
                .unwrap_or("")
                .to_owned();
                let ip = case["client"].as_str().unwrap_or("203.0.113.7");
                let v = pageviews::Visitor { ip, ua: &ua, secret: rig.web.salt(), date: "2026-10-23" };
                if wh.as_str() != Some(pageviews::visitor_hash(&v).as_str()) {
                    failures.push(format!("{name}: visitor hash"));
                }
                gm.insert("visitor_hash".into(), wh.clone());
            }
            if &g != w {
                failures.push(format!("{name}: event {g} != {w}"));
            }
        }
    }
    assert!(failures.is_empty(), "{} differences:\n{}", failures.len(), failures.join("\n"));
}

#[tokio::test]
async fn scenario_on() {
    replay("on").await;
}

#[tokio::test]
async fn scenario_off() {
    replay("off").await;
}

#[tokio::test]
async fn scenario_down() {
    replay("down").await;
}

#[tokio::test]
async fn scenario_geo() {
    replay("geo").await;
}

#[test]
fn jinja2_cases() {
    let t = data("templates.json");
    let ctx = minijinja::Value::from_serialize(&t["context"]);
    for case in t["cases"].as_array().expect("cases") {
        let mut env = templates::build(Path::new("unused"));
        let src = case["src"].as_str().expect("src").to_owned();
        env.add_template_owned("t.html", src.clone()).expect("parses");
        let got = env.get_template("t.html").and_then(|tpl| tpl.render(&ctx)).expect("renders");
        assert_eq!(got, case["out"].as_str().expect("out"), "{src}");
    }
}

#[test]
fn swf_markdown_matches_markdown_it() {
    let dir = repo().join("site/static/swf-docs");
    let golden = data("swf_docs.json");
    for (name, want) in golden["docs"].as_object().expect("docs") {
        let src = std::fs::read_to_string(dir.join(name)).expect("md").replace("\r\n", "\n");
        if hex::encode(Sha256::digest(src.as_bytes())) != golden["sha256"][name].as_str().unwrap_or("") {
            continue;
        }
        let want = String::from_utf8(unz(want.as_str().expect("html"))).expect("utf-8");
        assert_eq!(super::swf_docs::render_markdown(&src), want, "{name}");
    }
}

/// `pageviews::ROUTES` is the Python website's route table, in order.
#[test]
fn route_table_matches_python() {
    let py: Vec<&str> =
        golden()["routes"].as_array().expect("routes").iter().map(|r| r[0].as_str().unwrap_or("")).collect();
    assert_eq!(py, pageviews::ROUTES.to_vec());
}

/// Which website routes Rust answers and which go to web-py, written to
/// `KIWI_COVERAGE_OUT` (default: stdout).
#[tokio::test]
#[ignore]
async fn web_route_coverage() {
    let py = fake_api(&Value::Null).await;
    let rig = rig_with("down", py).await;
    let (mut rust, mut python) = (Vec::new(), Vec::new());
    for r in golden()["routes"].as_array().expect("routes") {
        let path = r[0].as_str().unwrap_or("");
        let concrete = if path == "/static" {
            "/static/appearance.css".to_owned()
        } else {
            path.replace("{doc}", "atlas").replace("{name}", "pvp.json").replace(['{', '}'], "")
        };
        let methods: Vec<&str> =
            r[1].as_array().map(|m| m.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
        for m in if methods.is_empty() { vec!["GET"] } else { methods } {
            let mut req = Request::builder().method(m).uri(&concrete).body(Body::empty()).expect("request");
            req.extensions_mut().insert(ConnectInfo(SocketAddr::from(([203, 0, 113, 9], 5000))));
            let res = rig.app.clone().oneshot(req).await.expect("infallible");
            let line = format!("{m} {path}");
            if res.status() == StatusCode::NOT_FOUND && res.headers().get("x-request-id").is_none() {
                python.push(line)
            } else {
                rust.push(line)
            }
        }
    }
    let report = format!(
        "rust ({}):\n  {}\npython ({}):\n  {}\n",
        rust.len(),
        rust.join("\n  "),
        python.len(),
        python.join("\n  ")
    );
    match std::env::var("KIWI_COVERAGE_OUT") {
        Ok(p) => std::fs::write(p, report).expect("write"),
        Err(_) => println!("{report}"),
    }
}

#[test]
fn page_templates() {
    for (path, want) in [
        ("/", Some("/")),
        ("/mods/why", Some("/mods/why")),
        ("/mods/a/b", Some("/mods/{handle}/{slug}")),
        ("/mods/a", Some("/mods/{handle}")),
        ("/mods/", None),
        ("/player/x y", Some("/player/{name}")),
        ("/terms\n", Some("/terms")),
        ("/activity/og.png", None),
        ("/static/x.css", None),
        ("/unlock_debug", Some("/unlock_debug")),
        ("/embed/viewer", None),
        ("/s", None),
    ] {
        assert_eq!(pageviews::page_template(path), want, "{path:?}");
    }
}

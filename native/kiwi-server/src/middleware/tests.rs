//! Replays the golden fixtures captured from the Python stack
//! (`native/port/parity/gen_middleware_fixtures.py`) through the Rust layers.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use axum::{
    Router,
    body::Body,
    http::{HeaderName, HeaderValue, Method, Request, StatusCode},
    response::Response,
    routing::{MethodFilter, on},
};
use base64::Engine;
use serde_json::Value;
use tower::ServiceExt;

use super::*;

const FIXTURES: &str = include_str!("fixtures.json");

/// Differences from Python that are on purpose (see idempotency.rs).
const KNOWN: &[(&str, &str)] = &[("idem_cookies", "set-cookie")];

/// Paths no Rust route matches: they must reach the fallback (the proxy in
/// production) with no layer touching the response.
const PROXIED: &[&str] = &["embed_sub"];

fn b64(s: &str) -> Vec<u8> {
    base64::engine::general_purpose::STANDARD.decode(s).unwrap()
}

fn latin1_bytes(s: &str) -> Vec<u8> {
    s.chars().map(|c| c as u32 as u8).collect()
}

fn method_filter(m: &str) -> MethodFilter {
    match m {
        "GET" => MethodFilter::GET,
        "POST" => MethodFilter::POST,
        "PUT" => MethodFilter::PUT,
        "DELETE" => MethodFilter::DELETE,
        other => panic!("method {other}"),
    }
}

fn route_response(spec: &Value) -> Response {
    if spec["kind"] == "raise" {
        panic!("boom");
    }
    let body = if spec["kind"] == "stream" {
        let chunks: Vec<Result<bytes::Bytes, std::io::Error>> = spec["chunks_b64"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| Ok(bytes::Bytes::from(b64(c.as_str().unwrap()))))
            .collect();
        Body::from_stream(futures::stream::iter(chunks))
    } else {
        Body::from(b64(spec["body_b64"].as_str().unwrap()))
    };
    let mut res = Response::new(body);
    *res.status_mut() = StatusCode::from_u16(spec["status"].as_u64().unwrap() as u16).unwrap();
    for h in spec["headers"].as_array().unwrap() {
        let name = HeaderName::from_bytes(h[0].as_str().unwrap().as_bytes()).unwrap();
        res.headers_mut().append(name, HeaderValue::from_bytes(&latin1_bytes(h[1].as_str().unwrap())).unwrap());
    }
    res
}

fn routes(table: &Value) -> Router {
    let mut r = Router::new();
    for spec in table.as_array().unwrap() {
        let mut filter: Option<MethodFilter> = None;
        for m in spec["methods"].as_array().unwrap() {
            let f = method_filter(m.as_str().unwrap());
            filter = Some(filter.map_or(f, |x| x.or(f)));
        }
        let s = Arc::new(spec.clone());
        r = r.route(
            spec["path"].as_str().unwrap(),
            on(filter.unwrap(), move || {
                let s = s.clone();
                async move { route_response(&s) }
            }),
        );
    }
    r
}

fn site_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../site/static")
}

fn mw(constants: &Value) -> Arc<Mw> {
    let mut cfg = MwConfig::from_lookup(&|_| None);
    cfg.static_root = site_root();
    let origins =
        constants["embed_origins"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_owned()).collect();
    Arc::new(Mw::new(
        cfg,
        Some(IdemStore::Memory(Mutex::new(HashMap::new()))),
        EmbedSource::Fixed(origins),
        forwarded::TrustedHosts::parse("127.0.0.1"),
    ))
}

fn app(table: &Value, mw: Arc<Mw>) -> Router {
    let inner = wrap(routes(table), mw, None).fallback(|| async { (StatusCode::IM_A_TEAPOT, "proxied") });
    outer(inner)
}

fn mask_rid(s: &str) -> String {
    let re = regex::Regex::new("req_[0-9a-f]{16}").unwrap();
    re.replace_all(s, "<rid>").into_owned()
}

fn redis_state(mw: &Mw) -> BTreeMap<String, (String, u64)> {
    match mw.idem.as_ref().unwrap() {
        IdemStore::Memory(m) => m.lock().unwrap().iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
        IdemStore::Redis(_) => unreachable!(),
    }
}

fn seed(mw: &Mw, state: &Value) {
    let IdemStore::Memory(m) = mw.idem.as_ref().unwrap() else { unreachable!() };
    let mut m = m.lock().unwrap();
    m.clear();
    for (k, v) in state.as_object().unwrap() {
        m.insert(k.clone(), (v[0].as_str().unwrap().to_owned(), v[1].as_u64().unwrap()));
    }
}

fn expected_state(v: &Value) -> BTreeMap<String, (String, u64)> {
    v.as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| (k.clone(), (v[0].as_str().unwrap().to_owned(), v[1].as_u64().unwrap())))
        .collect()
}

#[tokio::test]
async fn python_fixtures() {
    let fx: Value = serde_json::from_str(FIXTURES).unwrap();
    let mw = mw(&fx["constants"]);
    let app = app(&fx["routes"], mw.clone());
    let mut failures = Vec::new();

    for case in fx["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        seed(&mw, case.get("redis_before").unwrap_or(&Value::Object(Default::default())));
        let mut req = Request::builder()
            .method(Method::from_bytes(case["method"].as_str().unwrap().as_bytes()).unwrap())
            .uri(case["path"].as_str().unwrap());
        let mut fixed_rid = false;
        for h in case["request_headers"].as_array().unwrap() {
            let (k, v) = (h[0].as_str().unwrap(), h[1].as_str().unwrap());
            fixed_rid |= k.eq_ignore_ascii_case("x-request-id") && !v.is_empty();
            req = req.header(k, HeaderValue::from_bytes(&latin1_bytes(v)).unwrap());
        }
        let res = app.clone().oneshot(req.body(Body::empty()).unwrap()).await.unwrap();
        let status = res.status().as_u16();
        let mut got: Vec<(String, String)> = res
            .headers()
            .iter()
            .map(|(k, v)| (k.as_str().to_owned(), v.as_bytes().iter().map(|&b| b as char).collect()))
            .collect();
        let body = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap().to_vec();
        if PROXIED.contains(&name) {
            got.sort();
            let untouched = status == 418
                && body == b"proxied"
                && got
                    == [
                        ("content-length".into(), "7".into()),
                        ("content-type".into(), "text/plain; charset=utf-8".into()),
                    ];
            if !untouched {
                failures.push(format!("{name}: expected the untouched fallback, got {status} {got:?}"));
            }
            continue;
        }
        let mut want = case_response_headers(case);
        let mut want_body = b64(case["body_b64"].as_str().unwrap());
        if !fixed_rid {
            for h in got.iter_mut().chain(want.iter_mut()) {
                h.1 = mask_rid(&h.1);
            }
            want_body = mask_rid(&String::from_utf8_lossy(&want_body)).into_bytes();
        }
        let got_body = if fixed_rid { body } else { mask_rid(&String::from_utf8_lossy(&body)).into_bytes() };
        let skip: Vec<&str> = KNOWN.iter().filter(|(c, _)| *c == name).map(|(_, h)| *h).collect();
        got.retain(|(k, _)| !skip.contains(&k.as_str()));
        want.retain(|(k, _)| !skip.contains(&k.as_str()));
        got.sort();
        want.sort();

        let mut diffs = Vec::new();
        if status != case["status"].as_u64().unwrap() as u16 {
            diffs.push(format!("status {status} != {}", case["status"]));
        }
        if got != want {
            diffs.push(format!("headers\n   got  {got:?}\n   want {want:?}"));
        }
        if got_body != want_body {
            diffs.push(format!(
                "body ({} vs {} bytes)\n   got  {:?}\n   want {:?}",
                got_body.len(),
                want_body.len(),
                String::from_utf8_lossy(&got_body[..got_body.len().min(160)]),
                String::from_utf8_lossy(&want_body[..want_body.len().min(160)])
            ));
        }
        let state = redis_state(&mw);
        let want_state = expected_state(&case["redis_after"]);
        if state != want_state {
            diffs.push(format!("redis\n   got  {state:?}\n   want {want_state:?}"));
        }
        if !diffs.is_empty() {
            failures.push(format!("{name}: {}", diffs.join("\n  ")));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} cases differ:\n{}",
        failures.len(),
        fx["cases"].as_array().unwrap().len(),
        failures.join("\n")
    );
}

fn case_response_headers(case: &Value) -> Vec<(String, String)> {
    case["response_headers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| (h[0].as_str().unwrap().to_owned(), h[1].as_str().unwrap().to_owned()))
        .collect()
}

async fn raw_http(addr: std::net::SocketAddr, request: &str) -> (String, Vec<(String, String)>, usize) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
    s.write_all(request.as_bytes()).await.unwrap();
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).await.unwrap();
    let end = buf.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    let head = String::from_utf8_lossy(&buf[..end]).into_owned();
    let mut lines = head.split("\r\n");
    let status = lines.next().unwrap().to_owned();
    let headers =
        lines.filter_map(|l| l.split_once(':')).map(|(k, v)| (k.trim().to_lowercase(), v.trim().to_owned())).collect();
    (status, headers, buf.len() - end - 4)
}

/// Framing on the wire, as uvicorn sends it for the same responses.
#[tokio::test]
async fn wire_framing_matches_uvicorn() {
    let fx: Value = serde_json::from_str(FIXTURES).unwrap();
    let app = app(&fx["routes"], mw(&fx["constants"]));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let get =
        |m: &str, p: &str, extra: &str| format!("{m} {p} HTTP/1.1\r\nHost: t\r\nConnection: close\r\n{extra}\r\n");
    let has = |h: &[(String, String)], k: &str| h.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());

    let (st, h, body) = raw_http(addr, &get("HEAD", "/__mw/big", "")).await;
    assert!(st.contains("200") && has(&h, "content-length").as_deref() == Some("2701") && body == 0, "{st} {h:?}");
    assert_eq!(has(&h, "transfer-encoding"), None);

    let (st, h, body) = raw_http(addr, &get("HEAD", "/__mw/big", "Accept-Encoding: gzip\r\n")).await;
    assert!(st.contains("200") && body == 0, "{st}");
    assert_eq!(has(&h, "content-encoding").as_deref(), Some("gzip"));
    assert_eq!(has(&h, "content-length"), None, "{h:?}");

    let (st, h, _) = raw_http(addr, &get("GET", "/__mw/nocontent", "")).await;
    assert!(st.contains("204"), "{st}");
    assert_eq!((has(&h, "content-length"), has(&h, "transfer-encoding")), (None, None), "{h:?}");

    let (_, h, _) = raw_http(addr, &get("GET", "/__mw/big", "Accept-Encoding: gzip\r\n")).await;
    assert_eq!(has(&h, "transfer-encoding").as_deref(), Some("chunked"));
    assert_eq!(has(&h, "content-length"), None);

    let pre = "Origin: https://docs.aallyn.net\r\nAccess-Control-Request-Method: POST\r\n";
    let (st, h, _) = raw_http(addr, &get("OPTIONS", "/__mw/echo", pre)).await;
    assert!(st.contains("200") && has(&h, "content-length").as_deref() == Some("2"), "{st} {h:?}");
    assert_eq!(has(&h, "allow"), None);

    let (st, h, _) = raw_http(addr, &get("OPTIONS", "/site/rigs/a", "")).await;
    assert!(st.contains("204"), "{st}");
    assert_eq!((has(&h, "content-length"), has(&h, "allow")), (None, None), "{h:?}");
}

#[test]
fn python_constants() {
    let fx: Value = serde_json::from_str(FIXTURES).unwrap();
    let c = &fx["constants"];
    assert_eq!(c["api_csp"], paths::API_CSP);
    assert_eq!(c["site_csp"], paths::SITE_CSP);
    let origins: Vec<String> =
        c["embed_origins"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().into()).collect();
    assert_eq!(c["embed_csp"], paths::embed_csp(&origins));
    assert_eq!(c["embed_csp_none"], paths::embed_csp(&[]));
    let mut hosts = config::api_side_hosts("https://api.aallyn.net", "https://trove.aallyn.net");
    hosts.sort();
    assert_eq!(c["api_side_hosts"], serde_json::json!(hosts));
    for (raw, want) in c["parse_origins"].as_object().unwrap() {
        assert_eq!(serde_json::json!(security::parse_origins(raw)), *want, "{raw:?}");
    }
    for k in c["idem_keys"].as_array().unwrap() {
        let s = |i: usize| k[i].as_str().unwrap();
        assert_eq!(idempotency::storage_key(s(0), s(1), s(2), s(3)), s(4));
    }
}

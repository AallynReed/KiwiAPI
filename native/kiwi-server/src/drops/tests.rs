//! Checks against `testdata/drops/golden.json`
//! (`native/port/parity/gen_drops_fixtures.py`), plus store tests that need
//! a throwaway Mongo.

use std::{
    sync::{Arc, OnceLock},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use axum::{
    Router,
    body::Body,
    http::{HeaderMap, HeaderValue, Method, Request, StatusCode},
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Utc};
use hyper_util::{client::legacy::Client, rt::TokioExecutor};
use kiwi_core::drops::{RangeError, parse_range_header, safe_filename};
use mongodb::bson::{Bson, Document};
use serde_json::Value;
use tower::ServiceExt;

use super::{
    DropUpload, FileDrop, download, drop_view, public_view,
    service::{DropCreate, DropUpdate, extended},
    upload::write_stream,
    upload_view,
};
use crate::{
    config::Settings,
    db::Db,
    error::ApiError,
    middleware::{self, EmbedSource, Mw, MwConfig, forwarded::TrustedHosts},
    pyjson, request_id,
    state::{AppState, Inner},
    validation::{read_body, validate},
};

fn golden() -> &'static Value {
    static G: OnceLock<Value> = OnceLock::new();
    G.get_or_init(|| serde_json::from_str(include_str!("../../testdata/drops/golden.json")).expect("fixture"))
}

fn s(v: &Value) -> &str {
    v.as_str().unwrap_or_else(|| panic!("string: {v}"))
}

fn arr<'a>(v: &'a Value, key: &str) -> &'a Vec<Value> {
    v[key].as_array().unwrap_or_else(|| panic!("section {key}"))
}

fn latin1(s: &str) -> Vec<u8> {
    s.chars().map(|c| c as u32 as u8).collect()
}

fn settings() -> Settings {
    let mut vars: Vec<(String, String)> =
        golden()["env"].as_object().unwrap().iter().map(|(k, v)| (k.clone(), s(v).to_owned())).collect();
    vars.push(("PY_UPSTREAM".into(), "http://py:8000".into()));
    Settings::from_vars(vars).expect("settings")
}

fn state_with(mongo: Option<mongodb::Database>) -> AppState {
    let upstream = Client::builder(TokioExecutor::new()).build_http();
    AppState { inner: Arc::new(Inner { settings: settings(), db: Db { mongo, redis: None, pg: None }, upstream }) }
}

fn app(state: AppState) -> Router {
    let mw =
        Mw::new(MwConfig::from_lookup(&|_| None), None, EmbedSource::Fixed(vec![]), TrustedHosts::parse("127.0.0.1"));
    let inner = middleware::wrap(super::router(), Arc::new(mw), None)
        .fallback(|| async { StatusCode::IM_A_TEAPOT })
        .with_state(state);
    middleware::outer(inner)
}

async fn parts(resp: Response) -> (u16, HeaderMap, Vec<u8>) {
    let status = resp.status().as_u16();
    let headers = resp.headers().clone();
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.expect("body");
    (status, headers, body.to_vec())
}

#[test]
fn filenames() {
    for pair in arr(golden(), "filenames") {
        assert_eq!(safe_filename(pair[0].as_str()), s(&pair[1]), "{}", pair[0]);
    }
}

#[test]
fn ranges() {
    for c in arr(golden(), "ranges") {
        let got = parse_range_header(s(&c["range"]), 300);
        match (&got, c.get("ok")) {
            (Ok(r), Some(want)) => {
                let want: Vec<(u64, u64)> =
                    want.as_array().unwrap().iter().map(|p| (p[0].as_u64().unwrap(), p[1].as_u64().unwrap())).collect();
                assert_eq!(r, &want, "{}", c["range"]);
            }
            (Err(RangeError::Malformed(msg)), None) => {
                assert_eq!(s(&c["error"]), "MalformedRangeHeader");
                assert_eq!(*msg, s(&c["content"]), "{}", c["range"]);
            }
            (Err(RangeError::NotSatisfiable), None) => assert_eq!(s(&c["error"]), "RangeNotSatisfiable"),
            _ => panic!("{}: got {got:?} want {c}", c["range"]),
        }
    }
}

#[test]
fn stat_headers() {
    for c in arr(golden(), "stat_headers") {
        let (etag, lm) = download::stat_headers(c["mtime"].as_f64().unwrap(), c["size"].as_u64().unwrap());
        assert_eq!((etag.as_str(), lm.as_str()), (s(&c["etag"]), s(&c["last_modified"])), "{c}");
    }
}

#[tokio::test]
async fn downloads() {
    let d = &golden()["downloads"];
    let dir = std::env::temp_dir().join(format!("kiwi-drops-test-{}", mongodb::bson::oid::ObjectId::new().to_hex()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("stored.bin");
    std::fs::write(&path, latin1(s(&d["data"]))).unwrap();
    let ns = d["mtime_ns"].as_u64().unwrap();
    let f = std::fs::File::options().write(true).open(&path).unwrap();
    f.set_modified(UNIX_EPOCH + Duration::from_nanos(ns)).unwrap();
    drop(f);
    let mut bad = Vec::new();
    for c in arr(d, "cases") {
        let mut req = HeaderMap::new();
        for pair in c["headers"].as_array().unwrap() {
            req.append(
                axum::http::HeaderName::from_bytes(s(&pair[0]).as_bytes()).unwrap(),
                HeaderValue::from_bytes(&latin1(s(&pair[1]))).unwrap(),
            );
        }
        let resp = download::file_response(&path, s(&c["media_type"]), s(&c["filename"]), &req)
            .await
            .unwrap_or_else(IntoResponse::into_response);
        let (status, headers, body) = parts(resp).await;
        let mut got_headers: Vec<(String, String)> = headers
            .iter()
            .map(|(k, v)| (k.as_str().to_owned(), String::from_utf8_lossy(v.as_bytes()).into_owned()))
            .collect();
        let mut body: String = body.iter().map(|&b| b as char).collect();
        let boundary = got_headers
            .iter()
            .find(|(k, _)| k == "content-type")
            .and_then(|(_, v)| v.split("boundary=").nth(1))
            .map(str::to_owned);
        if let Some(b) = boundary {
            body = body.replace(&b, "BOUNDARY");
            for (_, v) in got_headers.iter_mut() {
                *v = v.replace(&b, "BOUNDARY");
            }
        }
        let want: Vec<(String, String)> =
            c["resp_headers"].as_array().unwrap().iter().map(|p| (s(&p[0]).to_owned(), s(&p[1]).to_owned())).collect();
        if u64::from(status) != c["status"].as_u64().unwrap() || got_headers != want || body != s(&c["body"]) {
            bad.push(format!("{}: {status} {got_headers:?}\n  want {} {want:?}", c["name"], c["status"]));
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

fn to_doc(v: &Value) -> Document {
    match Bson::try_from(v.clone()).expect("extjson") {
        Bson::Document(d) => d,
        other => panic!("not a document: {other:?}"),
    }
}

#[test]
fn views() {
    let v = &golden()["views"];
    let now: DateTime<Utc> = DateTime::parse_from_rfc3339(s(&v["now"])).unwrap().with_timezone(&Utc);
    let app_url = &settings().app_url;
    for c in arr(v, "cases") {
        let drop = FileDrop::from_doc(&to_doc(&c["drop"])).unwrap();
        let uploads: Vec<DropUpload> =
            c["uploads"].as_array().unwrap().iter().map(|u| DropUpload::from_doc(&to_doc(u)).unwrap()).collect();
        let view = pyjson::pydantic(&Value::Object(drop_view(&drop, &uploads, app_url, now)));
        assert_eq!(view, s(&c["view"]));
        assert_eq!(pyjson::pydantic(&public_view(&drop)), s(&c["public"]));
        let ups: Vec<String> = uploads.iter().map(|u| pyjson::pydantic(&upload_view(u))).collect();
        let want: Vec<String> = c["uploads_view"].as_array().unwrap().iter().map(|x| s(x).to_owned()).collect();
        assert_eq!(ups, want);
        assert_eq!(drop.is_open(now), c["open"].as_bool().unwrap());
        assert_eq!(drop.to_doc(), to_doc(&c["drop"]));
        for (u, raw) in uploads.iter().zip(c["uploads"].as_array().unwrap()) {
            assert_eq!(u.to_doc(), to_doc(raw));
        }
    }
    let first = &arr(v, "cases")[0];
    let drop = FileDrop::from_doc(&to_doc(&first["drop"])).unwrap();
    let mut created = drop_view(&drop, &[], app_url, now);
    created.insert("pin".into(), "0042".into());
    assert_eq!(pyjson::pydantic(&Value::Object(created)), s(&v["created"]));
}

/// The uploader routes up to the point Python's reply is settled without a
/// store, and the admin bodies' validation.
#[tokio::test]
async fn http_replay() {
    let app = app(state_with(None));
    let mut bad = Vec::new();
    for c in arr(golden(), "http") {
        let name = s(&c["name"]);
        let mut headers = HeaderMap::new();
        if let Some(ct) = c["content_type"].as_str() {
            headers.insert("content-type", HeaderValue::from_str(ct).unwrap());
        }
        headers.insert("x-request-id", HeaderValue::from_str(s(&c["request_id"])).unwrap());
        let body = latin1(s(&c["body"]));
        let (status, got_headers, out) = if s(&c["url"]).starts_with("/site/") {
            let mut req = Request::builder()
                .method(Method::from_bytes(s(&c["method"]).as_bytes()).unwrap())
                .uri(s(&c["url"]))
                .body(Body::from(body))
                .unwrap();
            *req.headers_mut() = headers;
            parts(app.clone().oneshot(req).await.unwrap()).await
        } else {
            let resp = request_id::scope(s(&c["request_id"]).to_owned(), async {
                let input = match read_body(&headers, Body::from(body)).await {
                    Ok(i) => i,
                    Err(e) => return e.into_response(),
                };
                let res = if s(&c["method"]) == "POST" {
                    validate::<DropCreate>(&input).map(|_| ())
                } else {
                    validate::<DropUpdate>(&input).map(|_| ())
                };
                match res {
                    Ok(()) if name.ends_with("_500") => ApiError::internal().into_response(),
                    Ok(()) => StatusCode::OK.into_response(),
                    Err(e) => e.into_response(),
                }
            })
            .await;
            parts(resp).await
        };
        let ct = got_headers.get("content-type").map(|v| v.to_str().unwrap().to_owned());
        let want_ct =
            c["resp_headers"].as_array().unwrap().iter().find(|h| h[0] == "content-type").map(|h| s(&h[1]).to_owned());
        let out = String::from_utf8(out).unwrap();
        if u64::from(status) != c["status"].as_u64().unwrap() || out != s(&c["resp"]) || ct != want_ct {
            bad.push(format!("{name}: {status} {out}\n  want {} {}", c["status"], s(&c["resp"])));
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

#[test]
fn extend_from_now_when_expired() {
    let now = Utc::now();
    let past = now - chrono::TimeDelta::hours(5);
    assert_eq!(extended(past, now, 2), now + chrono::TimeDelta::hours(2));
    let future = now + chrono::TimeDelta::hours(5);
    assert_eq!(extended(future, now, 2), future + chrono::TimeDelta::hours(2));
}

#[test]
fn stream_writes_and_caps() {
    let dir = std::env::temp_dir().join(format!("kiwi-drops-write-{}", mongodb::bson::oid::ObjectId::new().to_hex()));
    let dest = dir.join("slug").join("f.bin");
    let data = vec![7u8; 3 * 1024 * 1024 + 5];
    let (size, sha) = write_stream(&data, &dest, 4 * 1024 * 1024).unwrap();
    assert_eq!(size, data.len() as u64);
    use sha2::Digest;
    assert_eq!(sha, hex::encode(sha2::Sha256::digest(&data)));
    assert_eq!(std::fs::read(&dest).unwrap(), data);
    assert!(matches!(write_stream(&data, &dest, 3 * 1024 * 1024), Err(super::upload::WriteError::OverLimit)));
    assert_eq!(write_stream(&[], &dest, 10).unwrap().0, 0);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn st_mtime_is_python_float() {
    let t = UNIX_EPOCH + Duration::from_nanos(1_761_130_800_123_456_700);
    assert_eq!(crate::pyfloat::repr(download::st_mtime(t)), "1761130800.1234567");
    assert!(download::st_mtime(SystemTime::now()) > 0.0);
}

/// The full uploader and admin flow against a real Mongo.
#[tokio::test]
#[ignore = "needs MONGO_URI pointing at a throwaway mongo (creates and drops a scratch database)"]
async fn store_flow() {
    let uri = std::env::var("MONGO_URI").expect("MONGO_URI");
    let client = mongodb::Client::with_uri_str(&uri).await.unwrap();
    let db = client.database(&format!("kiwi_drops_test_{}", mongodb::bson::oid::ObjectId::new().to_hex()));
    let state = state_with(Some(db.clone()));
    let created = super::service::create(
        &state,
        DropCreate { label: "  Logs ".into(), pin: "2468".into(), max_uploads: 1, expires_in_hours: 1, max_file_mb: 1 },
    )
    .await
    .unwrap();
    let slug = s(&created["slug"]).to_owned();
    assert_eq!(created["label"], "Logs");
    assert_eq!(created["pin"], "2468");
    let drop = super::service::by_slug(&state, &slug).await.unwrap();
    assert!(super::service::check_pin(&drop, "2468").await.is_ok());
    assert!(super::service::check_pin(&drop, "1111").await.is_err());
    assert!(super::service::claim_slot(&state, &drop).await.unwrap());
    assert!(!super::service::claim_slot(&state, &drop).await.unwrap());
    super::service::release_slot(&state, &drop).await.unwrap();
    let id = s(&created["id"]).to_owned();
    let updated = super::service::update(
        &state,
        &id,
        DropUpdate { label: Some(" New ".into()), max_uploads: Some(2), extend_hours: None, revoked: Some(true) },
    )
    .await
    .unwrap();
    assert_eq!((updated["label"].as_str(), updated["open"].as_bool()), (Some("New"), Some(false)));
    assert!(super::service::by_slug(&state, &slug).await.is_err());
    let listed = super::service::list(&state).await.unwrap();
    assert_eq!(listed.as_array().unwrap().len(), 1);
    super::service::delete(&state, &id).await.unwrap();
    assert_eq!(super::service::list(&state).await.unwrap(), Value::Array(vec![]));
    db.drop().await.unwrap();
}

/// Starlette's matching: `uploads` is a valid `{drop_id}`, and a wrong method
/// names the first route of the path in `Allow`.
#[tokio::test]
async fn routing_like_starlette() {
    let app = app(state_with(None));
    let cases = [
        ("PATCH", "/admin/drops/uploads", 401, None),
        ("GET", "/admin/drops/uploads/abc", 405, Some("DELETE")),
        ("PUT", "/admin/drops", 405, Some("GET")),
        ("GET", "/admin/drops/abc", 405, Some("PATCH")),
        ("DELETE", "/site/drops/abc", 405, Some("GET")),
        ("GET", "/site/drops/abc/upload", 405, Some("POST")),
        ("GET", "/admin/drops/", 418, None),
    ];
    for (method, uri, status, allow) in cases {
        let req = Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            .body(Body::from("{}"))
            .unwrap();
        let (got, headers, _) = parts(app.clone().oneshot(req).await.unwrap()).await;
        assert_eq!(got, status, "{method} {uri}");
        assert_eq!(headers.get("allow").map(|v| v.to_str().unwrap()), allow, "{method} {uri}");
    }
}

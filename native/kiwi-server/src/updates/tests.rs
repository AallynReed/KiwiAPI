//! Checks against `testdata/updates/golden.json`, captured from the real Python
//! routes, models and helpers by `native/port/parity/gen_updates_fixtures.py`.
//! Each HTTP case replays the exact Mongo calls Beanie / pymongo made (filter,
//! projection, sort, skip, limit) and their results; blobs come from a temp CAS
//! laid out with the fixture's files and mtime.

use std::{
    collections::VecDeque,
    panic::AssertUnwindSafe,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, UNIX_EPOCH},
};

use axum::{
    Router,
    body::Body,
    http::{HeaderMap, HeaderName, HeaderValue, Method, Request, StatusCode, Uri},
    response::{IntoResponse, Response},
};
use base64::Engine;
use futures::FutureExt;
use hyper_util::{client::legacy::Client, rt::TokioExecutor};
use kiwi_core::updates::{compare, difflib, pkfx, tree};
use mongodb::bson::{Bson, Document};
use serde_json::{Value, json};
use tower::ServiceExt;

use super::{
    ChangesQ, CompareQ, Flavour, PageQ, PathQ, SearchQ, TreeQ, VfxListQ, branch_of, branches, changes, history, meta,
    reply, search, site_json, tree as tree_route, v1_json, versions, vfx, vfx_asset, view,
};
use crate::{
    config::Settings,
    db::Db,
    error::ApiError,
    middleware::{self, EmbedSource, Mw, MwConfig, forwarded::TrustedHosts},
    ratelimit::RateLimitInfo,
    request_id,
    routes::store::service::{Fetch, Find},
    state::{AppState, Inner},
    validation::{FromParams, Params},
};

fn golden() -> &'static Value {
    static G: OnceLock<Value> = OnceLock::new();
    G.get_or_init(|| serde_json::from_str(include_str!("../../testdata/updates/golden.json")).expect("fixture"))
}

fn s(v: &Value) -> &str {
    v.as_str().unwrap_or_else(|| panic!("string: {v}"))
}

fn arr<'a>(v: &'a Value, key: &str) -> &'a Vec<Value> {
    v[key].as_array().unwrap_or_else(|| panic!("fixture section {key}"))
}

fn b64(v: &Value) -> Vec<u8> {
    base64::engine::general_purpose::STANDARD.decode(s(v)).expect("b64")
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array().expect("list").iter().map(|x| s(x).to_owned()).collect()
}

// --- pure helpers ------------------------------------------------------------------

fn hunks_json(h: &[compare::Hunk<'_>]) -> Value {
    Value::Array(
        h.iter()
            .map(|h| {
                let lines: Vec<Value> = h
                    .lines
                    .iter()
                    .map(|l| json!({"kind": l.kind.as_str(), "left": l.left, "right": l.right, "text": l.text}))
                    .collect();
                json!({"left_start": h.left_start, "right_start": h.right_start, "lines": lines})
            })
            .collect(),
    )
}

#[test]
fn difflib_and_hunks_match_python() {
    let cases = arr(golden(), "hunks");
    assert!(cases.len() > 100);
    for (n, case) in cases.iter().enumerate() {
        let (a, b) = (strings(&case["a"]), strings(&case["b"]));
        let ops: Vec<Value> =
            difflib::opcodes(&a, &b).iter().map(|o| json!([o.tag.as_str(), o.i1, o.i2, o.j1, o.j2])).collect();
        assert_eq!(Value::Array(ops), case["opcodes"], "opcodes of case {n}");
        assert_eq!(hunks_json(&compare::make_hunks(&a, &b, compare::HUNK_CONTEXT)), case["hunks"], "hunks of case {n}");
    }
}

#[test]
fn decode_blob_matches_python() {
    for case in arr(golden(), "decode") {
        let d = if let Some(n) = case.get("len") {
            let n = n.as_u64().unwrap() as usize;
            let mut data = b"x\n".repeat(n / 2);
            data.extend(std::iter::repeat_n(b'x', n % 2));
            let d = compare::decode_blob(Some(&data));
            assert_eq!(d.lines.len() as u64, case["line_count"].as_u64().unwrap());
            d
        } else {
            let data = (!case["data"].is_null()).then(|| b64(&case["data"]));
            let d = compare::decode_blob(data.as_deref());
            assert_eq!(d.lines, strings(&case["lines"]), "{case}");
            d
        };
        assert_eq!(d.is_text, case["is_text"].as_bool().unwrap());
        assert_eq!(d.reason.map(Value::from).unwrap_or(Value::Null), case["reason"]);
    }
}

#[test]
fn lossy_utf8_matches_python_replace() {
    for case in arr(golden(), "lossy") {
        assert_eq!(String::from_utf8_lossy(&b64(&case["data"])), s(&case["text"]));
    }
}

#[test]
fn pkfx_parser_matches_python() {
    for case in arr(golden(), "pkfx") {
        assert_eq!(pkfx::extract_refs(s(&case["text"])), strings(&case["refs"]), "{}", case["text"]);
    }
    for case in arr(golden(), "media") {
        let p = s(&case["path"]);
        assert_eq!(pkfx::basename(p), s(&case["basename"]), "{p}");
        assert_eq!(pkfx::media_type_for(p), s(&case["media"]), "{p}");
        assert_eq!(pkfx::is_asset(p), case["asset"].as_bool().unwrap(), "{p}");
    }
}

#[test]
fn directory_listing_matches_python() {
    for case in arr(golden(), "listing") {
        let rows: Vec<tree::Row> = arr(case, "rows")
            .iter()
            .map(|r| tree::Row {
                path: s(&r["path"]).to_owned(),
                size: r["size"].as_i64().unwrap_or(0),
                last_ordinal: r.get("last_ordinal").and_then(Value::as_i64).unwrap_or(0),
            })
            .collect();
        let ours: Vec<Value> = tree::directory_listing(&rows, s(&case["prefix"]))
            .iter()
            .map(|c| {
                json!({"name": c.name, "path": c.path, "is_dir": c.is_dir, "file_count": c.file_count,
                       "size": c.size, "last_ordinal": c.last_ordinal})
            })
            .collect();
        assert_eq!(Value::Array(ours), case["listing"], "prefix {}", case["prefix"]);
    }
}

#[test]
fn view_kinds_match_python() {
    for case in arr(golden(), "view_kinds") {
        let kind = tree::view_kind(s(&case["path"]), case["size"].as_i64().unwrap()).unwrap_or("missing");
        assert_eq!(kind, s(&case["kind"]), "{case}");
    }
}

// --- HTTP replay -------------------------------------------------------------------

fn to_doc(v: &Value) -> Document {
    match Bson::try_from(v.clone()).unwrap_or_else(|e| panic!("extjson {v}: {e}")) {
        Bson::Document(d) => d,
        other => panic!("not a document: {other:?}"),
    }
}

fn ext(d: &Document) -> String {
    serde_json::to_string(&Bson::Document(d.clone()).into_canonical_extjson()).expect("json")
}

/// The CAS of the fixture, written once with the recorded mtime.
fn store_dir() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let root = std::env::temp_dir().join(format!("kiwi-updates-golden-{}", std::process::id()));
        let mtime = UNIX_EPOCH + Duration::from_nanos(golden()["mtime_ns"].as_u64().unwrap());
        let mut blobs: Vec<(String, Vec<u8>)> =
            golden()["blobs"].as_object().unwrap().iter().map(|(k, v)| (k.clone(), b64(v))).collect();
        for (sha, spec) in golden()["generated_blobs"].as_object().unwrap() {
            let unit = s(&spec["repeat"]).as_bytes();
            let n = spec["len"].as_u64().unwrap() as usize;
            blobs.push((sha.clone(), unit.iter().copied().cycle().take(n).collect()));
        }
        for (sha, data) in blobs {
            let p = root.join("objects").join(&sha[..2]).join(&sha);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, data).unwrap();
            std::fs::File::options().write(true).open(&p).unwrap().set_modified(mtime).unwrap();
        }
        root
    })
}

struct Replay {
    name: String,
    calls: Mutex<VecDeque<Value>>,
    bad: Mutex<Vec<String>>,
}

impl Replay {
    fn new(case: &Value) -> Self {
        let calls = case["calls"].as_array().cloned().unwrap_or_default();
        Replay { name: s(&case["name"]).to_owned(), calls: Mutex::new(calls.into()), bad: Mutex::new(Vec::new()) }
    }

    fn next(&self, op: &str, coll: &str, fields: &[(&str, String)]) -> Value {
        let Some(call) = self.calls.lock().unwrap().pop_front() else {
            panic!("{}: {op} on {coll}: Python raised before this call", self.name);
        };
        if call["op"] != op || call["coll"] != coll {
            self.bad
                .lock()
                .unwrap()
                .push(format!("{}: call {op} {coll}, recorded {} {}", self.name, call["op"], call["coll"]));
        }
        for (key, ours) in fields {
            let want = serde_json::to_string(&call[*key]).expect("json");
            if *ours != want {
                self.bad
                    .lock()
                    .unwrap()
                    .push(format!("{}: {op} {coll} {key}:\n  ours {ours}\n  want {want}", self.name));
            }
        }
        call
    }

    fn finish(&self) -> Vec<String> {
        let mut bad = std::mem::take(&mut *self.bad.lock().unwrap());
        let left = self.calls.lock().unwrap().len();
        if left > 0 {
            bad.push(format!("{}: {left} recorded calls never made", self.name));
        }
        bad
    }
}

impl Fetch for Replay {
    async fn find_one(&self, coll: &'static str, filter: Document, projection: Document) -> Option<Document> {
        let call = self.next("find_one", coll, &[("filter", ext(&filter)), ("projection", ext(&projection))]);
        (!call["result"].is_null()).then(|| to_doc(&call["result"]))
    }

    async fn find(&self, coll: &'static str, find: Find) -> Vec<Document> {
        let sort: Vec<Value> = find.sort.iter().map(|(k, d)| json!([k, d])).collect();
        let call = self.next(
            "find",
            coll,
            &[
                ("filter", ext(&find.filter)),
                ("projection", ext(&find.projection)),
                ("sort", serde_json::to_string(&sort).unwrap()),
                ("skip", find.skip.to_string()),
                ("limit", find.limit.to_string()),
            ],
        );
        arr(&call, "result").iter().map(to_doc).collect()
    }

    async fn count(&self, coll: &'static str, filter: Document) -> u64 {
        self.next("count", coll, &[("filter", ext(&filter))])["result"].as_u64().expect("count")
    }

    async fn delete_all(&self, _coll: &'static str) -> u64 {
        unreachable!("the read API never deletes")
    }

    async fn insert(&self, _coll: &'static str, _doc: Document) -> Result<(), String> {
        unreachable!("the read API never writes")
    }

    async fn blob(&self, sha: &str) -> Option<Vec<u8>> {
        let p = store_dir().join("objects").join(sha.get(..2).unwrap_or(sha)).join(sha);
        std::fs::metadata(&p).ok().filter(|m| m.is_file())?;
        Some(std::fs::read(p).unwrap())
    }
}

const RATE: RateLimitInfo = RateLimitInfo { limit: 600, remaining: 599, reset: 1_767_323_045 };

fn params<T: FromParams>(query: Option<&str>) -> Result<T, ApiError> {
    let headers = HeaderMap::new();
    let mut p = Params::new(Vec::new(), query, &headers);
    let value = T::from_params(&mut p);
    p.v.finish(value).map_err(ApiError::from)
}

fn request_headers(case: &Value) -> HeaderMap {
    let mut h = HeaderMap::new();
    for p in arr(case, "headers") {
        h.append(HeaderName::from_bytes(s(&p[0]).as_bytes()).unwrap(), HeaderValue::from_str(s(&p[1])).unwrap());
    }
    h
}

async fn dispatch<F: Fetch>(case: &Value, f: &F, cache: &vfx::Cache) -> Response {
    let uri: Uri = s(&case["url"]).parse().expect("uri");
    let query = uri.query();
    let headers = request_headers(case);
    let branch = branch_of(&uri);
    let segs: Vec<&str> = uri.path().split('/').collect();
    let (fl, rl) = if segs[1] == "v1" { (Flavour::V1, Some(RATE)) } else { (Flavour::Site, None) };
    let route = if segs[3] == "branches" { "branches".to_owned() } else { segs[4..].join("/") };
    macro_rules! q {
        ($t:ty) => {
            match params::<$t>(query) {
                Ok(v) => v,
                Err(e) => return e.into_response(),
            }
        };
    }
    let site_age = |a: u32| if fl == Flavour::Site { a } else { 0 };
    match route.as_str() {
        "branches" => match fl {
            Flavour::V1 => v1_json(rl, branches(f, fl).await),
            Flavour::Site => site_json(30, branches(f, fl).await),
        },
        "versions" => {
            let q = q!(PageQ);
            reply(fl, rl, site_age(30), versions(f, &branch, &q, fl).await)
        }
        "changes" if fl == Flavour::V1 => {
            let q = q!(ChangesQ<100, 1000>);
            reply(fl, rl, 0, changes(f, &branch, &q, fl).await)
        }
        "changes" => {
            let q = q!(ChangesQ<200, 2000>);
            reply(fl, rl, 30, changes(f, &branch, &q, fl).await)
        }
        "tree" => {
            let TreeQ(prefix) = q!(TreeQ);
            reply(fl, rl, site_age(60), tree_route(f, &branch, &prefix, fl).await)
        }
        "search" => {
            let q = q!(SearchQ);
            reply(fl, rl, 30, search(f, &branch, &q).await)
        }
        "file/meta" => {
            let PathQ(p) = q!(PathQ);
            reply(fl, rl, site_age(60), meta(f, &branch, &p).await)
        }
        "file" => {
            let PathQ(p) = q!(PathQ);
            let dir = store_dir().to_string_lossy().into_owned();
            match super::file(f, &dir, &branch, &p, &headers).await {
                Ok(r) => r,
                Err(e) => e.into_response(),
            }
        }
        "file/view" => {
            let PathQ(p) = q!(PathQ);
            reply(fl, rl, site_age(60), view(f, &branch, &p, fl).await)
        }
        "file/history" => {
            let PathQ(p) = q!(PathQ);
            reply(fl, rl, site_age(30), history(f, &branch, &p, fl).await)
        }
        "file/compare" => {
            let q = q!(CompareQ);
            reply(fl, rl, site_age(60), super::compare(f, &branch, &q, fl).await)
        }
        "vfx" => {
            let q = q!(VfxListQ);
            if let Err(e) = super::check_branch(&branch) {
                return e.into_response();
            }
            site_json(120, vfx::list_effects(cache, f, &branch, &q.q, q.limit, q.offset).await)
        }
        "vfx/manifest" => {
            let PathQ(p) = q!(PathQ);
            if let Err(e) = super::check_branch(&branch) {
                return e.into_response();
            }
            match vfx::manifest(cache, f, &branch, &p).await {
                Some(v) => site_json(300, v),
                None => ApiError::not_found(format!("No effect '{p}'")).into_response(),
            }
        }
        "vfx/asset" => {
            let PathQ(p) = q!(PathQ);
            vfx_asset(cache, f, &branch, &p, &headers).await
        }
        other => panic!("unrouted {other}"),
    }
}

async fn parts(resp: Response) -> (u16, HeaderMap, Vec<u8>) {
    let status = resp.status().as_u16();
    let headers = resp.headers().clone();
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.expect("body");
    (status, headers, body.to_vec())
}

fn header(h: &HeaderMap, name: &str) -> Option<String> {
    h.get(name).map(|v| String::from_utf8_lossy(v.as_bytes()).into_owned())
}

fn rid(case: &Value) -> String {
    arr(case, "headers").iter().find(|p| p[0] == "x-request-id").map(|p| s(&p[1]).to_owned()).expect("rid")
}

#[tokio::test]
async fn http_replay() {
    let cases = arr(golden(), "http");
    let mut bad = Vec::new();
    for case in cases {
        let name = s(&case["name"]);
        let replay = Replay::new(case);
        let cache = vfx::Cache::default();
        let run = AssertUnwindSafe(request_id::scope(rid(case), dispatch(case, &replay, &cache))).catch_unwind();
        let resp = match run.await {
            Ok(resp) => resp,
            Err(_) => request_id::scope(rid(case), async { ApiError::internal().into_response() }).await,
        };
        let (status, headers, body) = parts(resp).await;
        bad.extend(replay.finish());
        if u64::from(status) != case["status"].as_u64().unwrap() {
            bad.push(format!("{name}: status {status} want {}", case["status"]));
        }
        let want = &case["resp_headers"];
        let mut compared = vec![
            "content-type",
            "etag",
            "content-disposition",
            "last-modified",
            "accept-ranges",
            "content-range",
            "x-ratelimit-limit",
            "x-ratelimit-remaining",
            "x-ratelimit-reset",
        ];
        if !arr(case, "headers").iter().any(|p| p[0] == "authorization") {
            compared.push("cache-control");
        }
        if name.starts_with("v1_file") && status < 300 {
            compared.push("content-length");
        }
        for h in compared {
            let theirs = if h == "content-length" {
                case["content_length"].as_str().map(str::to_owned)
            } else {
                want.get(h).map(|v| s(v).to_owned())
            };
            if header(&headers, h) != theirs {
                bad.push(format!("{name}: {h} {:?} want {theirs:?}", header(&headers, h)));
            }
        }
        let binary = name.starts_with("v1_file") || name.starts_with("site_vfx_asset");
        let want_body = if binary { b64(&case["resp"]) } else { s(&case["resp"]).as_bytes().to_vec() };
        if body != want_body {
            bad.push(format!(
                "{name}:\n  got  {}\n  want {}",
                String::from_utf8_lossy(&body),
                String::from_utf8_lossy(&want_body)
            ));
        }
    }
    assert!(cases.len() > 120, "http cases missing");
    assert!(bad.is_empty(), "{} problems over {} cases:\n{}", bad.len(), cases.len(), bad.join("\n"));
}

/// The VFX index is reused until the branch's ordinal moves.
#[tokio::test]
async fn vfx_index_cached_per_ordinal() {
    let case = arr(golden(), "http").iter().find(|c| c["name"] == "site_vfx").expect("site_vfx");
    let calls = arr(case, "calls");
    let cache = vfx::Cache::default();
    let first = Replay::new(case);
    let a = vfx::list_effects(&cache, &first, "live-us", "", 300, 0).await;
    assert!(first.finish().is_empty());
    let again = Replay::new(&json!({"name": "again", "calls": [calls[0].clone()]}));
    let b = vfx::list_effects(&cache, &again, "live-us", "", 300, 0).await;
    assert!(again.finish().is_empty(), "a cached index must not rescan the tree");
    assert_eq!(a, b);
    let mut moved = calls[0].clone();
    moved["result"]["current_ordinal"] = json!({"$numberInt": "4"});
    let rebuild = Replay::new(&json!({"name": "rebuild", "calls": [moved, calls[1].clone()]}));
    vfx::list_effects(&cache, &rebuild, "live-us", "", 300, 0).await;
    assert!(rebuild.finish().is_empty(), "a new ordinal rebuilds the index");
}

fn state() -> AppState {
    let upstream = Client::builder(TokioExecutor::new()).build_http();
    let settings = Settings::from_vars([("PY_UPSTREAM", "http://py:8000"), ("SECRET_KEY", "updates-fixture-secret")])
        .expect("settings");
    AppState { inner: Arc::new(Inner { settings, db: Db { mongo: None, redis: None, pg: None }, upstream }) }
}

fn app() -> Router {
    let mw =
        Mw::new(MwConfig::from_lookup(&|_| None), None, EmbedSource::Fixed(vec![]), TrustedHosts::parse("127.0.0.1"));
    let inner = middleware::wrap(super::router(), Arc::new(mw), None)
        .fallback(|| async { StatusCode::IM_A_TEAPOT })
        .with_state(state());
    middleware::outer(inner)
}

/// The cases that need no store: 405s and the missing-credential 401s, which
/// also pin the gate -> auth -> query validation order.
#[tokio::test]
async fn gated_without_store() {
    let app = app();
    let mut bad = Vec::new();
    let mut n = 0;
    for case in arr(golden(), "gated") {
        let name = s(&case["name"]);
        if !(name.ends_with("_405") || name.ends_with("_noauth")) {
            continue;
        }
        n += 1;
        let mut req = Request::builder()
            .method(Method::from_bytes(s(&case["method"]).as_bytes()).unwrap())
            .uri(s(&case["url"]))
            .body(Body::empty())
            .unwrap();
        *req.headers_mut() = request_headers(case);
        let (status, headers, body) = parts(app.clone().oneshot(req).await.expect("infallible")).await;
        if u64::from(status) != case["status"].as_u64().unwrap() {
            bad.push(format!("{name}: status {status} want {}", case["status"]));
        }
        for h in ["content-type", "allow", "www-authenticate", "cache-control"] {
            let theirs = case["resp_headers"].get(h).map(|v| s(v).to_owned());
            if header(&headers, h) != theirs {
                bad.push(format!("{name}: {h} {:?} want {theirs:?}", header(&headers, h)));
            }
        }
        if body != s(&case["resp"]).as_bytes() {
            bad.push(format!("{name}:\n  got  {}\n  want {}", String::from_utf8_lossy(&body), s(&case["resp"])));
        }
    }
    assert!(n >= 6, "gated cases missing");
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

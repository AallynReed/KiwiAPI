//! Test support for `testdata/mods_creators_issues/golden.json` (from
//! `native/port/parity/gen_mods_creators_issues_fixtures.py`).

use std::{
    collections::HashMap,
    sync::{Arc, OnceLock},
};

use axum::{
    body::Body,
    http::{HeaderMap, HeaderName, HeaderValue},
    response::Response,
};
use mongodb::bson::{Bson, Document, oid::ObjectId};
use serde::de::DeserializeOwned;
use serde_json::Value;

use super::{mem::MemDb, store::Store};
use crate::{
    error::ApiError,
    outbound::testkit::{header, parts, to_doc},
    validation::{BodyInput, read_body},
};

pub use crate::outbound::testkit::{arr, s};

pub const COLLS: [&str; 7] =
    ["site_users", "users", "mod_projects", "mod_releases", "mod_issues", "mod_issue_events", "mod_creator_links"];

pub fn golden() -> &'static Value {
    static G: OnceLock<Value> = OnceLock::new();
    G.get_or_init(|| {
        serde_json::from_str(include_str!("../../testdata/mods_creators_issues/golden.json")).expect("fixture")
    })
}

pub fn now() -> chrono::DateTime<chrono::Utc> {
    crate::dt::parse_utc(s(&golden()["now"])).expect("now")
}

/// The case's starting database, in memory.
pub fn mem(case: &Value) -> Arc<MemDb> {
    let db = &golden()["dbs"][s(&case["db"])];
    let colls: HashMap<String, Vec<Document>> = COLLS
        .iter()
        .map(|c| ((*c).to_owned(), db[*c].as_array().map(|a| a.iter().map(to_doc).collect()).unwrap_or_default()))
        .collect();
    let mut m = MemDb::new(colls, now(), s(&golden()["minted"]));
    m.deny = case["deny"].as_array().map(|a| a.iter().map(|v| s(v).to_owned()).collect()).unwrap_or_default();
    Arc::new(m)
}

pub fn store(m: &Arc<MemDb>) -> Store {
    Store::Mem(m.clone())
}

/// A document of the starting database as its model.
pub fn model<T: DeserializeOwned>(m: &MemDb, coll: &str, id: &Value) -> Option<T> {
    let id = ObjectId::parse_str(id.as_str()?).ok()?;
    let d = m.snapshot(coll).into_iter().find(|d| d.get_object_id("_id").ok() == Some(id))?;
    Some(mongodb::bson::from_document(d).expect("model"))
}

fn norm(mut d: Document) -> String {
    if let Some(Bson::Array(ids)) = d.get_mut("participant_ids") {
        ids.sort_by_key(|b| b.as_object_id().map(|o| o.to_hex()).unwrap_or_default());
    }
    serde_json::to_string(&Bson::Document(d).into_canonical_extjson()).expect("json")
}

/// Through BSON, so both sides print numbers the same way (field order kept).
fn want(v: &Value) -> String {
    norm(to_doc(v))
}

/// Every collection against what the case left behind.
pub fn check_after(case: &Value, m: &MemDb) -> Vec<String> {
    let name = s(&case["name"]);
    let before = &golden()["dbs"][s(&case["db"])];
    let mut bad = Vec::new();
    for c in COLLS {
        let theirs = case["after"].get(c).unwrap_or(&before[c]);
        let theirs: Vec<String> = theirs.as_array().map(|a| a.iter().map(want).collect()).unwrap_or_default();
        let ours: Vec<String> = m.snapshot(c).into_iter().map(norm).collect();
        if ours != theirs {
            bad.push(format!(
                "{name}: {c} differs\n  ours {}\n  want {}",
                ours.join("\n       "),
                theirs.join("\n       ")
            ));
        }
    }
    bad
}

pub const HEADERS: [&str; 8] = [
    "content-type",
    "cache-control",
    "allow",
    "www-authenticate",
    "retry-after",
    "x-ratelimit-limit",
    "x-ratelimit-remaining",
    "x-ratelimit-reset",
];

/// Status, the compared headers and body bytes.
pub async fn check_response(case: &Value, resp: Response) -> Vec<String> {
    let name = s(&case["name"]);
    let (status, headers, body) = parts(resp).await;
    let mut bad = Vec::new();
    if u64::from(status) != case["status"].as_u64().unwrap() {
        bad.push(format!("{name}: status {status} want {}", case["status"]));
    }
    for h in HEADERS {
        let theirs = case["resp_headers"].get(h).map(|v| s(v).to_owned());
        if header(&headers, h) != theirs {
            bad.push(format!("{name}: {h} {:?} want {theirs:?}", header(&headers, h)));
        }
    }
    if body != s(&case["resp"]).as_bytes() {
        bad.push(format!("{name}:\n  got  {}\n  want {}", String::from_utf8_lossy(&body), s(&case["resp"])));
    }
    bad
}

pub fn rid(case: &Value) -> String {
    format!("fx-{}", s(&case["name"]))
}

pub fn request_headers(case: &Value) -> HeaderMap {
    let mut h = HeaderMap::new();
    for p in arr(case, "headers") {
        h.append(HeaderName::from_bytes(s(&p[0]).as_bytes()).unwrap(), HeaderValue::from_str(s(&p[1])).unwrap());
    }
    h
}

/// The body as FastAPI's handler reads it.
pub async fn body(case: &Value) -> Result<BodyInput, ApiError> {
    let bytes = match (case.get("raw"), case.get("body")) {
        (Some(r), _) => s(r).to_owned(),
        (_, Some(b)) => crate::pyjson::dumps_spaced(b),
        _ => String::new(),
    };
    read_body(&request_headers(case), Body::from(bytes)).await
}

pub type PathParams = Vec<(String, String)>;

/// Path parameters of `url` under `template` (Starlette-decoded), and the query.
pub fn url_params(url: &str, template: &str) -> Option<(PathParams, Option<String>)> {
    let (path, query) = match url.split_once('?') {
        Some((p, q)) => (p, Some(q.to_owned())),
        None => (url, None),
    };
    let segs: Vec<&str> = path.split('/').collect();
    let tpl: Vec<&str> = template.split('/').collect();
    if segs.len() != tpl.len() {
        return None;
    }
    let mut params = Vec::new();
    for (t, v) in tpl.iter().zip(&segs) {
        match t.strip_prefix('{').and_then(|n| n.strip_suffix('}')) {
            Some(n) => {
                params.push((n.to_owned(), percent_encoding::percent_decode_str(v).decode_utf8_lossy().into_owned()))
            }
            None if t == v => {}
            None => return None,
        }
    }
    Some((params, query))
}

pub fn limits(m: &MemDb) -> Vec<String> {
    m.limits.lock().unwrap().clone()
}

pub fn want_limits(case: &Value) -> Vec<String> {
    case["limits"].as_array().map(|a| a.iter().map(|l| s(&l[0]).to_owned()).collect()).unwrap_or_default()
}

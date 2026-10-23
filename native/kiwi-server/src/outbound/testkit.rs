//! Test support for the webhooks / DM-subscription goldens
//! (`testdata/webhooks/golden.json`, from `gen_webhooks_fixtures.py`).

use std::{
    collections::VecDeque,
    sync::{Arc, Mutex, OnceLock},
};

use axum::{
    Router,
    http::{HeaderMap, StatusCode},
    response::Response,
};
use mongodb::bson::{Bson, Document, oid::ObjectId};
use serde_json::Value;

use super::{
    Outcome,
    discord::{Payload, Reply, Transport},
    store::Coll,
};
use crate::{
    config::Settings,
    db::Db,
    middleware::{self, EmbedSource, Mw, MwConfig, forwarded::TrustedHosts},
    state::{AppState, Inner},
};

pub fn golden() -> &'static Value {
    static G: OnceLock<Value> = OnceLock::new();
    G.get_or_init(|| serde_json::from_str(include_str!("../../testdata/webhooks/golden.json")).expect("fixture"))
}

pub fn s(v: &Value) -> &str {
    v.as_str().unwrap_or_else(|| panic!("string: {v}"))
}

pub fn arr<'a>(v: &'a Value, key: &str) -> &'a Vec<Value> {
    v[key].as_array().unwrap_or_else(|| panic!("array {key} in {v}"))
}

pub fn to_doc(v: &Value) -> Document {
    match Bson::try_from(v.clone()).unwrap_or_else(|e| panic!("extjson {v}: {e}")) {
        Bson::Document(d) => d,
        other => panic!("not a document: {other:?}"),
    }
}

/// Canonical extended JSON, with the clock-dependent stamps blanked when asked.
pub fn ext(d: &Document, mask_clock: bool) -> String {
    let mut d = d.clone();
    if mask_clock {
        for key in ["updated_at", "last_delivered_at"] {
            if matches!(d.get(key), Some(Bson::DateTime(_))) {
                d.insert(key, "<now>");
            }
        }
    }
    serde_json::to_string(&Bson::Document(d).into_canonical_extjson()).expect("json")
}

pub fn docs(case: &Value, section: &str, coll: &str) -> Vec<Document> {
    case[section][coll].as_array().map(|a| a.iter().map(to_doc).collect()).unwrap_or_default()
}

/// The id the Python store gave the inserted document, if any.
pub fn new_id(case: &Value, coll: &str) -> Option<ObjectId> {
    let before: Vec<ObjectId> = docs(case, "before", coll).iter().filter_map(|d| d.get_object_id("_id").ok()).collect();
    docs(case, "after", coll).iter().filter_map(|d| d.get_object_id("_id").ok()).find(|id| !before.contains(id))
}

/// Differences between a collection's documents and the recorded ones.
pub fn diff_docs(name: &str, ours: &[Document], want: &[Document], mask_clock: bool) -> Vec<String> {
    let a: Vec<String> = ours.iter().map(|d| ext(d, mask_clock)).collect();
    let b: Vec<String> = want.iter().map(|d| ext(d, mask_clock)).collect();
    if a == b {
        return Vec::new();
    }
    vec![format!("{name}: stored documents differ\n  ours {}\n  want {}", a.join("\n       "), b.join("\n       "))]
}

/// An in-memory collection with the Python fake's semantics.
pub struct FakeColl {
    pub docs: Mutex<Vec<Document>>,
    next_id: Option<ObjectId>,
}

impl FakeColl {
    pub fn new(docs: Vec<Document>, next_id: Option<ObjectId>) -> Self {
        FakeColl { docs: Mutex::new(docs), next_id }
    }

    pub fn snapshot(&self) -> Vec<Document> {
        self.docs.lock().unwrap().clone()
    }
}

fn has(d: &Document, key: &str, v: &Bson) -> bool {
    match d.get(key) {
        Some(Bson::Array(a)) => a.contains(v),
        Some(x) => x == v,
        None => false,
    }
}

impl Coll for FakeColl {
    async fn list_owned(&self, owner: ObjectId) -> anyhow::Result<Vec<Document>> {
        let mut out: Vec<Document> =
            self.docs.lock().unwrap().iter().filter(|d| has(d, "owner_id", &Bson::ObjectId(owner))).cloned().collect();
        out.sort_by(|a, b| b.get_datetime("created_at").ok().cmp(&a.get_datetime("created_at").ok()));
        Ok(out)
    }

    async fn count_owned(&self, owner: ObjectId) -> anyhow::Result<u64> {
        Ok(self.docs.lock().unwrap().iter().filter(|d| has(d, "owner_id", &Bson::ObjectId(owner))).count() as u64)
    }

    async fn get(&self, id: ObjectId) -> anyhow::Result<Option<Document>> {
        Ok(self.docs.lock().unwrap().iter().find(|d| has(d, "_id", &Bson::ObjectId(id))).cloned())
    }

    async fn insert(&self, doc: Document) -> anyhow::Result<ObjectId> {
        let id = self.next_id.unwrap_or_default();
        self.docs.lock().unwrap().push(super::store::with_id(id, doc));
        Ok(id)
    }

    async fn save(&self, set: Document) -> anyhow::Result<()> {
        let id = set.get("_id").cloned().unwrap_or(Bson::Null);
        let mut docs = self.docs.lock().unwrap();
        match docs.iter_mut().find(|d| d.get("_id") == Some(&id)) {
            Some(d) => {
                for (k, v) in set {
                    d.insert(k, v);
                }
            }
            None => docs.push(super::store::with_id(id.as_object_id().unwrap_or_default(), set)),
        }
        Ok(())
    }

    async fn delete(&self, id: ObjectId) -> anyhow::Result<()> {
        let mut docs = self.docs.lock().unwrap();
        if let Some(i) = docs.iter().position(|d| has(d, "_id", &Bson::ObjectId(id))) {
            docs.remove(i);
        }
        Ok(())
    }

    async fn active_for(&self, event: &str) -> anyhow::Result<Vec<Document>> {
        Ok(self
            .docs
            .lock()
            .unwrap()
            .iter()
            .filter(|d| has(d, "active", &Bson::Boolean(true)) && has(d, "events", &Bson::String(event.into())))
            .cloned()
            .collect())
    }
}

pub fn outcome(v: &Value) -> Outcome {
    Outcome { ok: v[0].as_bool().unwrap(), status: v[1].as_i64(), error: v[2].as_str().map(str::to_owned) }
}

/// Canned delivery outcomes in order, then `fallback`; every send is recorded.
pub struct Canned {
    pub outcomes: Mutex<VecDeque<Outcome>>,
    pub fallback: Outcome,
    pub sends: Mutex<Vec<Value>>,
}

impl Canned {
    pub fn new(outcomes: &[Value], fallback: Outcome) -> Self {
        Canned { outcomes: Mutex::new(outcomes.iter().map(outcome).collect()), fallback, sends: Mutex::new(Vec::new()) }
    }

    pub fn next(&self, sent: Value) -> Outcome {
        self.sends.lock().unwrap().push(sent);
        self.outcomes.lock().unwrap().pop_front().unwrap_or_else(|| self.fallback.clone())
    }
}

/// A transport replaying a scripted list of httpx replies.
pub struct Scripted {
    pub replies: Mutex<VecDeque<Value>>,
    pub requests: Mutex<Vec<Value>>,
    pub sleeps: Mutex<Vec<f64>>,
}

impl Scripted {
    pub fn new(script: &[Value]) -> Self {
        Scripted {
            replies: Mutex::new(script.iter().cloned().collect()),
            requests: Mutex::new(Vec::new()),
            sleeps: Mutex::new(Vec::new()),
        }
    }
}

impl Transport for Scripted {
    async fn post(&self, url: &str, auth: Option<&str>, payload: Payload) -> Result<Reply, String> {
        let (raw, payload_json, multipart) = match &payload {
            Payload::Json(s) => (Some(s.clone()), None, false),
            Payload::Multipart { payload_json, .. } => (None, Some(payload_json.clone()), true),
        };
        self.requests.lock().unwrap().push(serde_json::json!({
            "url": url, "auth": auth, "raw": raw, "multipart": multipart, "payload_json": payload_json,
        }));
        let r = self.replies.lock().unwrap().pop_front().expect("scripted reply");
        if let Some(e) = r.get("error").and_then(Value::as_str) {
            return Err(e.to_owned());
        }
        Ok(Reply {
            status: r["status"].as_u64().unwrap() as u16,
            retry_after: r.get("retry_after").and_then(Value::as_str).map(str::to_owned),
            body: r.get("text").and_then(Value::as_str).unwrap_or("").as_bytes().to_vec(),
        })
    }

    async fn sleep(&self, secs: f64) {
        self.sleeps.lock().unwrap().push(secs);
    }
}

pub fn state() -> AppState {
    let settings = Settings::from_vars([("SECRET_KEY", "webhooks-fixture-secret")]).expect("settings");
    AppState { inner: Arc::new(Inner { settings, db: Db { mongo: None, redis: None, pg: None } }) }
}

/// `router` under the real middleware stack, without stores.
pub fn app(router: Router<AppState>) -> Router {
    let mw =
        Mw::new(MwConfig::from_lookup(&|_| None), None, EmbedSource::Fixed(vec![]), TrustedHosts::parse("127.0.0.1"));
    let inner =
        middleware::wrap(router, Arc::new(mw), None).fallback(|| async { StatusCode::IM_A_TEAPOT }).with_state(state());
    middleware::outer(inner)
}

pub async fn parts(resp: Response) -> (u16, HeaderMap, Vec<u8>) {
    let status = resp.status().as_u16();
    let headers = resp.headers().clone();
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.expect("body");
    (status, headers, body.to_vec())
}

pub fn header(h: &HeaderMap, name: &str) -> Option<String> {
    h.get(name).map(|v| String::from_utf8_lossy(v.as_bytes()).into_owned())
}

pub fn rid(case: &Value) -> String {
    arr(case, "headers").iter().find(|p| p[0] == "x-request-id").map(|p| s(&p[1]).to_owned()).expect("rid")
}

/// Status, headers and body of a recorded case against ours.
pub fn check_response(case: &Value, status: u16, headers: &HeaderMap, body: &[u8], extra: &[&str]) -> Vec<String> {
    let name = s(&case["name"]);
    let mut bad = Vec::new();
    if u64::from(status) != case["status"].as_u64().unwrap() {
        bad.push(format!("{name}: status {status} want {}", case["status"]));
    }
    for h in ["content-type"].iter().chain(extra) {
        let theirs = case["resp_headers"].get(*h).map(|v| s(v).to_owned());
        if header(headers, h) != theirs {
            bad.push(format!("{name}: {h} {:?} want {theirs:?}", header(headers, h)));
        }
    }
    if body != s(&case["resp"]).as_bytes() {
        bad.push(format!("{name}:\n  got  {}\n  want {}", String::from_utf8_lossy(body), s(&case["resp"])));
    }
    bad
}

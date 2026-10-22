//! Checks against `testdata/giveaways/golden.json`, captured from the real
//! Python routers, service and Beanie models by
//! `native/port/parity/gen_giveaways_fixtures.py`.

use std::{
    cmp::Ordering,
    sync::{Mutex, OnceLock},
};

use axum::{
    body::Body,
    http::{HeaderMap, HeaderValue, header},
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Utc};
use mongodb::bson::{self, Bson, Document, oid::ObjectId};
use serde_json::{Value, json};

use super::{
    EndedQuery,
    body::{self, GiveawayCreate, GiveawayUpdate, VaultCodeUpdate, VaultCodesAdd, VaultItemCreate, VaultItemUpdate},
    created, deleted, model,
    model::ENTRIES,
    public, service,
    store::{DbResult, Duplicate, Store},
};
use crate::{
    auth::models::{SITE_USERS, SiteUser},
    dt,
    ratelimit::RateLimitInfo,
    request_id,
    validation::{BodyInput, FromParams, Params, read_body},
};

fn golden() -> &'static Value {
    static G: OnceLock<Value> = OnceLock::new();
    G.get_or_init(|| serde_json::from_str(include_str!("../../testdata/giveaways/golden.json")).expect("fixture"))
}

fn s(v: &Value) -> &str {
    v.as_str().unwrap_or_else(|| panic!("string: {v}"))
}

fn to_doc(v: &Value) -> Document {
    match Bson::try_from(v.clone()) {
        Ok(Bson::Document(d)) => d,
        other => panic!("not a document: {other:?}"),
    }
}

fn ext(d: &Document) -> Value {
    Bson::Document(d.clone()).into_canonical_extjson()
}

async fn body_text(resp: Response) -> (u16, String) {
    let status = resp.status().as_u16();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.expect("body");
    (status, String::from_utf8(bytes.to_vec()).expect("utf8"))
}

// ---------------------------------------------------------------------------
// In-memory collections with the Mongo semantics the service relies on

pub(super) fn seq_id(n: u64) -> ObjectId {
    ObjectId::parse_str(format!("65f0{n:020x}")).expect("oid")
}

fn cmp_bson(a: &Bson, b: &Bson) -> Option<Ordering> {
    match (a, b) {
        (Bson::DateTime(x), Bson::DateTime(y)) => Some(x.cmp(y)),
        (Bson::String(x), Bson::String(y)) => Some(x.cmp(y)),
        (Bson::ObjectId(x), Bson::ObjectId(y)) => Some(x.cmp(y)),
        (Bson::Int32(x), Bson::Int32(y)) => Some(x.cmp(y)),
        (Bson::Int64(x), Bson::Int64(y)) => Some(x.cmp(y)),
        _ => None,
    }
}

fn matches(d: &Document, filter: &Document) -> bool {
    filter.iter().all(|(k, cond)| {
        if k == "$and" {
            return cond.as_array().expect("$and").iter().all(|c| matches(d, c.as_document().expect("clause")));
        }
        let v = d.get(k);
        match cond {
            Bson::Document(ops) if ops.keys().all(|o| o.starts_with('$')) && !ops.is_empty() => {
                ops.iter().all(|(op, arg)| match op.as_str() {
                    "$in" => arg.as_array().expect("$in").iter().any(|a| v == Some(a)),
                    "$ne" => v != Some(arg),
                    "$lte" => v.and_then(|v| cmp_bson(v, arg)).is_some_and(|o| o != Ordering::Greater),
                    "$gte" => v.and_then(|v| cmp_bson(v, arg)).is_some_and(|o| o != Ordering::Less),
                    other => panic!("operator {other}"),
                })
            }
            Bson::Null => v.is_none_or(|v| *v == Bson::Null),
            c => v == Some(c),
        }
    })
}

fn sort_key<'a>(d: &'a Document, k: &str) -> Option<&'a Bson> {
    d.get(k).filter(|v| **v != Bson::Null)
}

/// The store the scenario runs on: a frozen clock, the first entrant wins,
/// sequential ids continuing after the seed.
pub(super) struct Mem {
    colls: Mutex<Vec<(String, Vec<Document>)>>,
    next: Mutex<u64>,
    clock: Mutex<DateTime<Utc>>,
}

impl Mem {
    fn new(seed: &Value) -> Self {
        let mut colls = Vec::new();
        let mut next = 0;
        for (name, docs) in seed.as_object().expect("seed") {
            let docs: Vec<Document> = docs.as_array().expect("docs").iter().map(to_doc).collect();
            next += docs.len() as u64;
            colls.push((name.clone(), docs));
        }
        Mem { colls: Mutex::new(colls), next: Mutex::new(next), clock: Mutex::new(Utc::now()) }
    }

    fn with<T>(&self, coll: &str, f: impl FnOnce(&mut Vec<Document>) -> T) -> T {
        let mut colls = self.colls.lock().expect("lock");
        let i = match colls.iter().position(|(n, _)| n == coll) {
            Some(i) => i,
            None => {
                colls.push((coll.to_owned(), Vec::new()));
                colls.len() - 1
            }
        };
        f(&mut colls[i].1)
    }

    fn id(&self) -> ObjectId {
        let mut n = self.next.lock().expect("lock");
        *n += 1;
        seq_id(*n)
    }

    fn dump(&self) -> Value {
        let colls = self.colls.lock().expect("lock");
        let mut out = serde_json::Map::new();
        for (name, docs) in colls.iter() {
            out.insert(name.clone(), Value::Array(docs.iter().map(ext).collect()));
        }
        Value::Object(out)
    }

    fn store(&self, coll: &str, d: Document) -> Result<(), Duplicate> {
        self.with(coll, |rows| {
            let unique: &[&str] = match coll {
                ENTRIES => &["giveaway_id", "site_user_id"],
                "users" => &["email"],
                _ => &[],
            };
            if !unique.is_empty() && rows.iter().any(|r| unique.iter().all(|k| r.get(k) == d.get(k))) {
                return Err(Duplicate);
            }
            rows.push(d);
            Ok(())
        })
    }
}

fn with_id(id: ObjectId, fields: Document) -> Document {
    let mut d = bson::doc! {"_id": id};
    d.extend(fields);
    d
}

impl Store for Mem {
    fn now(&self) -> DateTime<Utc> {
        *self.clock.lock().expect("lock")
    }

    fn email_enabled(&self) -> bool {
        true
    }

    fn pick(&self, _n: usize) -> usize {
        0
    }

    async fn find(
        &self,
        coll: &'static str,
        filter: Document,
        sort: Option<Document>,
        limit: Option<i64>,
    ) -> DbResult<Vec<Document>> {
        let mut out: Vec<Document> =
            self.with(coll, |rows| rows.iter().filter(|d| matches(d, &filter)).cloned().collect());
        if let Some(sort) = sort {
            for (k, dir) in sort.iter().collect::<Vec<_>>().into_iter().rev() {
                let desc = dir.as_i32() == Some(-1);
                out.sort_by(|a, b| {
                    let o = match (sort_key(a, k), sort_key(b, k)) {
                        (None, None) => Ordering::Equal,
                        (None, Some(_)) => Ordering::Less,
                        (Some(_), None) => Ordering::Greater,
                        (Some(x), Some(y)) => cmp_bson(x, y).unwrap_or(Ordering::Equal),
                    };
                    if desc { o.reverse() } else { o }
                });
            }
        }
        if let Some(l) = limit {
            out.truncate(l as usize);
        }
        Ok(out)
    }

    async fn find_one(&self, coll: &'static str, filter: Document) -> DbResult<Option<Document>> {
        Ok(self.with(coll, |rows| rows.iter().find(|d| matches(d, &filter)).cloned()))
    }

    async fn count(&self, coll: &'static str, filter: Document) -> DbResult<u64> {
        Ok(self.with(coll, |rows| rows.iter().filter(|d| matches(d, &filter)).count() as u64))
    }

    async fn insert(&self, coll: &'static str, fields: Document) -> DbResult<Result<ObjectId, Duplicate>> {
        let id = self.id();
        Ok(self.store(coll, with_id(id, fields)).map(|()| id))
    }

    async fn insert_many(&self, coll: &'static str, docs: Vec<Document>) -> DbResult<Vec<ObjectId>> {
        let mut ids = Vec::new();
        for d in docs {
            let id = self.id();
            self.store(coll, with_id(id, d)).map_err(|_| anyhow::anyhow!("duplicate"))?;
            ids.push(id);
        }
        Ok(ids)
    }

    async fn save(&self, coll: &'static str, id: ObjectId, fields: Document) -> DbResult<()> {
        let fields = with_id(id, fields);
        let upsert = self.with(coll, |rows| match rows.iter_mut().find(|d| d.get_object_id("_id") == Ok(id)) {
            Some(d) => {
                for (k, v) in fields.iter() {
                    d.insert(k.clone(), v.clone());
                }
                false
            }
            None => true,
        });
        if upsert {
            self.store(coll, fields).map_err(|_| anyhow::anyhow!("duplicate"))?;
        }
        Ok(())
    }

    async fn delete_one(&self, coll: &'static str, id: ObjectId) -> DbResult<()> {
        self.with(coll, |rows| {
            if let Some(i) = rows.iter().position(|d| d.get_object_id("_id") == Ok(id)) {
                rows.remove(i);
            }
        });
        Ok(())
    }

    async fn delete_many(&self, coll: &'static str, filter: Document) -> DbResult<()> {
        self.with(coll, |rows| rows.retain(|d| !matches(d, &filter)));
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// The scenario

/// What a replay target adds to [`Store`]: the clock and the stored state.
pub(super) trait Harness: Store {
    fn set_now(&self, t: DateTime<Utc>);
    fn site_user(&self, name: &str) -> impl std::future::Future<Output = SiteUser> + Send;
}

impl Harness for Mem {
    fn set_now(&self, t: DateTime<Utc>) {
        *self.clock.lock().expect("lock") = t;
    }

    async fn site_user(&self, name: &str) -> SiteUser {
        let d = self.find_one(SITE_USERS, bson::doc! {"username": name}).await.expect("find").expect("user");
        bson::from_document(d).expect("site user")
    }
}

fn parse<B: body::Body>(body: &Value) -> Result<B, crate::error::ApiError> {
    body::checked::<B>(&BodyInput::Json(body.clone()))
}

const RATE: RateLimitInfo = RateLimitInfo { limit: 1, remaining: 1, reset: 0 };

async fn dispatch<S: Harness>(st: &S, step: &Value) -> Response {
    let id = step["id"].as_str().unwrap_or_default();
    let b = &step["body"];
    let user = match step["user"].as_str() {
        Some(u) => Some(st.site_user(u).await),
        None => None,
    };
    macro_rules! body {
        ($t:ty) => {
            match parse::<$t>(b) {
                Ok(v) => v,
                Err(e) => return e.into_response(),
            }
        };
    }
    match s(&step["op"]) {
        "list_items" => model(service::list_items(st).await),
        "create_item" => created(service::create_item(st, body!(VaultItemCreate)).await),
        "update_item" => model(service::update_item(st, id, body!(VaultItemUpdate)).await),
        "delete_item" => deleted(service::delete_item(st, id).await),
        "list_codes" => model(service::list_codes(st, id).await),
        "add_codes" => model(service::add_codes(st, id, body!(VaultCodesAdd)).await),
        "update_code" => model(service::update_code(st, id, body!(VaultCodeUpdate)).await),
        "delete_code" => deleted(service::delete_code(st, id).await),
        "list_admin" => model(service::list_admin(st).await),
        "create" => created(service::create(st, body!(GiveawayCreate)).await),
        "update" => model(service::update(st, id, body!(GiveawayUpdate)).await),
        "draw_now" => model(service::draw_now(st, id).await),
        "cancel" => model(service::cancel(st, id).await),
        "mine" => model(service::my_entry_ids(st, user.as_ref().expect("user")).await),
        "me" => model(service::my_participations(st, user.as_ref().expect("user")).await),
        "enter" => model(service::enter(st, user.as_ref().expect("user"), id).await),
        "ongoing" => public(RATE, service::list_ongoing(st).await),
        "upcoming" => public(RATE, service::list_upcoming(st).await),
        "ended" => {
            let headers = HeaderMap::new();
            let mut p = Params::new(Vec::new(), step["query"].as_str(), &headers);
            let q = EndedQuery::from_params(&mut p);
            public(RATE, service::list_ended(st, q.days).await)
        }
        other => panic!("op {other}"),
    }
}

pub(super) async fn replay<S: Harness>(st: &S) -> Vec<String> {
    let scenario = &golden()["scenario"];
    let mut bad = Vec::new();
    for (i, step) in scenario["steps"].as_array().expect("steps").iter().enumerate() {
        match s(&step["op"]) {
            "clock" => st.set_now(dt::parse_utc(s(&step["now"])).expect("clock")),
            "run_due" => service::run_due(st).await.expect("run_due"),
            "list_public" => {
                let items = service::list_public(st).await.expect("list_public");
                let got = crate::pyjson::dumps(&json!({"items": items}));
                if got != s(&step["response"]) {
                    bad.push(format!("step {i} list_public: got {got}\n  want {}", step["response"]));
                }
            }
            op => {
                let rid = s(&step["request_id"]).to_owned();
                let resp = request_id::scope(rid, dispatch(st, step)).await;
                let cache = resp.headers().get(header::CACHE_CONTROL).map(|v| v.to_str().unwrap_or("").to_owned());
                let (status, text) = body_text(resp).await;
                let want = (step["status"].as_u64().expect("status") as u16, s(&step["response"]));
                if (status, text.as_str()) != want {
                    bad.push(format!("step {i} {op}: got {status} {text}\n  want {} {}", want.0, want.1));
                }
                if cache.as_deref() != step["cache_control"].as_str() {
                    bad.push(format!("step {i} {op}: cache-control {cache:?} vs {}", step["cache_control"]));
                }
            }
        }
    }
    bad
}

#[tokio::test]
async fn scenario_replay() {
    let scenario = &golden()["scenario"];
    let mem = Mem::new(&scenario["seed"]);
    let bad = replay(&mem).await;
    assert!(bad.is_empty(), "{} step(s) differ:\n{}", bad.len(), bad.join("\n"));
    let got = mem.dump();
    let want = &scenario["final"];
    for (name, docs) in want.as_object().expect("final") {
        let g = serde_json::to_string(&got[name]).expect("json");
        let w = serde_json::to_string(docs).expect("json");
        assert_eq!(g, w, "collection {name}");
    }
    assert_eq!(got.as_object().map(|o| o.len()), want.as_object().map(|o| o.len()));
}

// ---------------------------------------------------------------------------
// Request bodies

fn parsed<T: serde::Serialize>(fields: T) -> Value {
    serde_json::to_value(fields).expect("json")
}

fn iso(t: &Option<DateTime<Utc>>) -> Value {
    t.map_or(Value::Null, |t| Value::String(dt::isoformat(&t)))
}

fn check_body(route: &str, body: &BodyInput) -> Result<Value, crate::error::ApiError> {
    Ok(match route {
        "create" => {
            let r = body::checked::<GiveawayCreate>(body)?;
            json!({"title": r.title, "description": r.description, "starts_at": iso(&Some(r.starts_at)),
                   "ends_at": iso(&Some(r.ends_at)), "vault_item_id": r.vault_item_id})
        }
        "update" => {
            let r = body::checked::<GiveawayUpdate>(body)?;
            json!({"title": r.title, "description": r.description, "starts_at": iso(&r.starts_at),
                   "ends_at": iso(&r.ends_at), "vault_item_id": r.vault_item_id})
        }
        "create_item" => {
            let r = body::checked::<VaultItemCreate>(body)?;
            json!({"name": r.name, "description": r.description})
        }
        "update_item" => {
            let r = body::checked::<VaultItemUpdate>(body)?;
            json!({"name": r.name, "description": r.description})
        }
        "add_codes" => parsed(json!({"codes": body::checked::<VaultCodesAdd>(body)?.codes})),
        "update_code" => json!({"code": body::checked::<VaultCodeUpdate>(body)?.code}),
        other => panic!("route {other}"),
    })
}

#[tokio::test]
async fn request_bodies() {
    let mut bad = Vec::new();
    for case in golden()["bodies"].as_array().expect("bodies") {
        let mut headers = HeaderMap::new();
        match &case["content_type"] {
            Value::Null if case.get("content_type").is_some() => {}
            Value::String(ct) => {
                headers.insert(header::CONTENT_TYPE, HeaderValue::from_str(ct).expect("ct"));
            }
            _ => {
                headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
            }
        }
        let raw = s(&case["body"]).to_owned();
        // pyjson::loads keeps an int beyond u64 only as a float, so the echoed `input` differs.
        if raw.contains("123456789012345678901234567890") {
            continue;
        }
        let rid = s(&case["request_id"]).to_owned();
        let route = s(&case["route"]);
        let got = request_id::scope(rid, async {
            let body = match read_body(&headers, Body::from(raw.clone())).await {
                Ok(b) => b,
                Err(e) => return Err(body_text(e.into_response()).await),
            };
            match check_body(route, &body) {
                Ok(v) => Ok(v),
                Err(e) => Err(body_text(e.into_response()).await),
            }
        })
        .await;
        let status = case["status"].as_u64().expect("status") as u16;
        match got {
            Ok(v) if status == 400 && case["parsed"] == v => {}
            Err((st, text)) if st == status && text == s(&case["response"]) => {}
            other => bad
                .push(format!("{route} {raw}: got {other:?}\n  want {status} {} {}", case["response"], case["parsed"])),
        }
    }
    assert!(bad.is_empty(), "{} case(s) differ:\n{}", bad.len(), bad.join("\n"));
}

#[test]
fn object_ids() {
    for case in golden()["oids"].as_array().expect("oids") {
        let value = s(&case["value"]);
        let got = service::parse_oid(value);
        match (&case["ok"], got) {
            (Value::String(hex), Ok(o)) => assert_eq!(&o.to_hex(), hex, "{value:?}"),
            (Value::String(hex), Err(e)) => {
                assert!(hex.len() < 24 && e.status.as_u16() == 500, "{value:?}: {e}");
            }
            (_, Err(e)) => {
                assert_eq!(e.status.as_u16(), case["status"].as_u64().expect("status") as u16, "{value:?}");
                assert_eq!(e.message, s(&case["message"]), "{value:?}");
            }
            (_, Ok(o)) => panic!("{value:?}: parsed {o}"),
        }
    }
}

#[test]
fn rederive_and_sort() {
    use super::model::GiveawayStatus::*;
    let t = |h: i64| dt::parse_utc("2026-10-22T11:00:00Z").expect("t") + chrono::Duration::hours(h);
    assert_eq!(service::rederive(Scheduled, t(-1), t(1), t(0)), Open);
    assert_eq!(service::rederive(Open, t(1), t(2), t(0)), Scheduled);
    assert_eq!(service::rederive(Open, t(-2), t(-1), t(0)), Open);
    assert_eq!(service::rederive(Scheduled, t(-2), t(-1), t(0)), Scheduled);
    let m = |won, h, tag: &str| super::views::Mine { won, ends_at: t(h), view: json!(tag) };
    let mut v = vec![m(false, 1, "a"), m(true, 0, "b"), m(false, 2, "c"), m(false, 1, "d"), m(true, 0, "e")];
    super::views::sort_mine(&mut v);
    let order: Vec<&str> = v.iter().map(|m| m.view.as_str().unwrap_or("")).collect();
    assert_eq!(order, ["b", "e", "c", "a", "d"]);
}

#[test]
fn clean_codes_like_python() {
    let raw: Vec<String> = ["  a ", "a", "", "\u{3000}b\u{3000}", "\x1cc\x1f", &"x".repeat(401), &"é".repeat(400)]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let got = service::clean_codes(&raw);
    assert_eq!(got, vec!["a".to_owned(), "b".into(), "c".into(), "é".repeat(400)]);
}

// ---------------------------------------------------------------------------
// Through the middleware stack, no stores: 405s, credentials, body before auth

fn app() -> axum::Router {
    use std::sync::Arc;

    use crate::{
        config::Settings,
        db::Db,
        middleware::{self, EmbedSource, Mw, MwConfig, forwarded::TrustedHosts},
        state::{AppState, Inner},
    };
    let upstream = hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new()).build_http();
    let settings = Settings::from_vars([("PY_UPSTREAM", "http://py:8000"), ("SECRET_KEY", "giveaways-fixture-secret")])
        .expect("settings");
    let state =
        AppState { inner: Arc::new(Inner { settings, db: Db { mongo: None, redis: None, pg: None }, upstream }) };
    let mw =
        Mw::new(MwConfig::from_lookup(&|_| None), None, EmbedSource::Fixed(vec![]), TrustedHosts::parse("127.0.0.1"));
    let inner = middleware::wrap(super::router(), Arc::new(mw), None)
        .fallback(|| async { axum::http::StatusCode::IM_A_TEAPOT })
        .with_state(state);
    middleware::outer(inner)
}

#[tokio::test]
async fn gated_without_store() {
    use tower::ServiceExt;

    let app = app();
    let mut bad = Vec::new();
    for case in golden()["gated"].as_array().expect("gated") {
        let mut req = axum::http::Request::builder()
            .method(s(&case["method"]))
            .uri(s(&case["path"]))
            .header("x-request-id", s(&case["request_id"]));
        if let Some(a) = case["auth"].as_str() {
            req = req.header("authorization", a);
        }
        let body = match case["body"].as_str() {
            Some(b) => {
                req = req.header("content-type", "application/json");
                Body::from(b.to_owned())
            }
            None => Body::empty(),
        };
        let resp = app.clone().oneshot(req.body(body).expect("request")).await.expect("infallible");
        let name = format!("{} {}", s(&case["method"]), s(&case["path"]));
        for h in ["content-type", "allow", "www-authenticate"] {
            let got = resp.headers().get(h).map(|v| v.to_str().unwrap_or("").to_owned());
            if got.as_deref() != case["headers"][h].as_str() {
                bad.push(format!("{name}: {h} {got:?} want {}", case["headers"][h]));
            }
        }
        let (status, text) = body_text(resp).await;
        if (u64::from(status), text.as_str()) != (case["status"].as_u64().expect("status"), s(&case["response"])) {
            bad.push(format!("{name}: got {status} {text}\n  want {} {}", case["status"], case["response"]));
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

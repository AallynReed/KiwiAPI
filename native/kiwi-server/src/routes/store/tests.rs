//! Checks against `testdata/store/golden.json`, captured from the real Python
//! parser, service and routes by `native/port/parity/gen_store_fixtures.py`.
//! Each HTTP case replays the exact Mongo calls Beanie made (filter,
//! projection, sort, skip, limit) and their results.

use std::{
    collections::{BTreeMap, VecDeque},
    panic::AssertUnwindSafe,
    sync::{Arc, Mutex, OnceLock},
};

use axum::{
    Router,
    body::Body,
    http::{HeaderMap, HeaderName, HeaderValue, Method, Request, StatusCode, Uri},
    response::{IntoResponse, Response},
};
use base64::Engine;
use futures::FutureExt;
use kiwi_core::store::{
    Category, NO_STORE_DATA, Product, State, Summary, ingest_category, ingest_product, ingest_state,
    parser::{ParsedProduct, ParsedStoreDump, parse_dump},
};
use mongodb::bson::{self, Bson, Document};
use serde_json::{Value, json};
use tower::ServiceExt;

use super::{
    ProductsParams, SITE_PRODUCT, TextureParams, TimelineParams, V1_PRODUCT, code_of, model, product_response,
    reset_response, service, site_json, texture_response, v1_json,
};
use crate::{
    auth::{
        IngestAuth,
        models::{ApiToken, User},
    },
    config::Settings,
    db::Db,
    error::ApiError,
    middleware::{self, EmbedSource, Mw, MwConfig, forwarded::TrustedHosts},
    pyfloat,
    ratelimit::RateLimitInfo,
    request_id,
    state::{AppState, Inner},
    validation::{FromParams, Params},
};

pub(super) fn golden() -> &'static Value {
    static G: OnceLock<Value> = OnceLock::new();
    G.get_or_init(|| serde_json::from_str(include_str!("../../../testdata/store/golden.json")).expect("fixture"))
}

pub(super) fn s(v: &Value) -> &str {
    v.as_str().unwrap_or_else(|| panic!("string: {v}"))
}

pub(super) fn arr<'a>(v: &'a Value, key: &str) -> &'a Vec<Value> {
    v[key].as_array().unwrap_or_else(|| panic!("fixture section {key}"))
}

fn to_bson(v: &Value) -> Bson {
    Bson::try_from(v.clone()).unwrap_or_else(|e| panic!("extjson {v}: {e}"))
}

pub(super) fn to_doc(v: &Value) -> Document {
    match to_bson(v) {
        Bson::Document(d) => d,
        other => panic!("not a document: {other:?}"),
    }
}

fn ext(d: &Document) -> String {
    serde_json::to_string(&Bson::Document(d.clone()).into_canonical_extjson()).expect("json")
}

/// The recorded calls of one case, served in order.
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

    fn fail(&self, msg: String) {
        self.bad.lock().unwrap().push(format!("{}: {msg}", self.name));
    }

    /// The next call, checked against what Rust is about to send. A call
    /// Python never got to make (it raised first) is a store error here.
    fn next(&self, op: &str, coll: &str, fields: &[(&str, String)]) -> Value {
        let Some(call) = self.calls.lock().unwrap().pop_front() else {
            service::unhandled(format!("{op} on {coll}: Python raised before this call"));
        };
        if call["op"] != op || call.get("coll").and_then(Value::as_str).is_some_and(|c| c != coll) {
            self.fail(format!("call {op} {coll}, recorded {} {}", call["op"], call["coll"]));
        }
        for (key, ours) in fields {
            let want = serde_json::to_string(&call[*key]).expect("json");
            if *ours != want {
                self.fail(format!("{op} {coll} {key}:\n  ours {ours}\n  want {want}"));
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

impl service::Fetch for Replay {
    async fn find_one(&self, coll: &'static str, filter: Document, projection: Document) -> Option<Document> {
        let call = self.next("find_one", coll, &[("filter", ext(&filter)), ("projection", ext(&projection))]);
        (!call["result"].is_null()).then(|| to_doc(&call["result"]))
    }

    async fn find(&self, coll: &'static str, find: service::Find) -> Vec<Document> {
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
        let call = self.next("count", coll, &[("filter", ext(&filter))]);
        call["result"].as_u64().expect("count")
    }

    async fn delete_all(&self, coll: &'static str) -> u64 {
        let call = self.next("delete_many", coll, &[("filter", "{}".into())]);
        call["result"].as_u64().expect("deleted")
    }

    async fn insert(&self, coll: &'static str, mut doc: Document) -> Result<(), String> {
        let call = self.next("insert_one", coll, &[]);
        let mut want = to_doc(&call["doc"]);
        for key in ["_id", "timestamp"] {
            let (a, b) = (doc.remove(key), want.remove(key));
            if std::mem::discriminant(&a) != std::mem::discriminant(&b) {
                self.fail(format!("insert {key}: {a:?} vs {b:?}"));
            }
        }
        let keys = |d: &Document| d.keys().cloned().collect::<Vec<_>>();
        if keys(&doc) != keys(&want) || ext(&doc) != ext(&want) {
            self.fail(format!("insert doc:\n  ours {}\n  want {}", ext(&doc), ext(&want)));
        }
        Ok(())
    }

    async fn blob(&self, sha: &str) -> Option<Vec<u8>> {
        let call = self.next("blob", "", &[("sha", serde_json::to_string(sha).unwrap())]);
        call["result"].as_str().map(|b| base64::engine::general_purpose::STANDARD.decode(b).expect("b64"))
    }
}

const RATE: RateLimitInfo = RateLimitInfo { limit: 600, remaining: 599, reset: 1_767_323_045 };

fn params<T: FromParams>(query: Option<&str>) -> Result<T, ApiError> {
    let headers = HeaderMap::new();
    let mut p = Params::new(Vec::new(), query, &headers);
    let value = T::from_params(&mut p);
    p.v.finish(value).map_err(ApiError::from)
}

fn master() -> IngestAuth {
    let user: User = bson::from_document(bson::doc! {
        "_id": bson::oid::ObjectId::parse_str("65a1b2c3d4e5f60718293a4b").unwrap(),
        "email": "master@aallyn.net", "is_superuser": true,
    })
    .expect("user");
    let token: ApiToken = bson::from_document(bson::doc! {
        "_id": bson::oid::ObjectId::parse_str("65a1b2c3d4e5f60718293a51").unwrap(),
        "user_id": user.id, "name": "Store bot",
    })
    .expect("token");
    IngestAuth { user, token: Some(token), rate_limit: Some(RATE) }
}

pub(super) async fn dispatch<F: service::Fetch>(case: &Value, f: &F, now: i64) -> Response {
    let uri: Uri = s(&case["url"]).parse().expect("uri");
    let (path, query) = (uri.path(), uri.query());
    let rl = Some(RATE);
    macro_rules! q {
        ($t:ty) => {
            match params::<$t>(query) {
                Ok(v) => v,
                Err(e) => return e.into_response(),
            }
        };
    }
    match path {
        "/v1/store/reset" => reset_response(f, &master(), chrono::Utc::now()).await,
        "/v1/store/products" => {
            let ProductsParams(q) = q!(ProductsParams<100>);
            v1_json(rl, service::products_page(f, &q, model::Flavour::V1).await)
        }
        "/site/store/products" => {
            let ProductsParams(q) = q!(ProductsParams<500>);
            site_json(service::products_page(f, &q, model::Flavour::Site).await)
        }
        "/v1/store/categories" => v1_json(rl, service::categories(f).await),
        "/site/store/categories" => site_json(service::categories(f).await),
        "/v1/store/timeline" => {
            let q = q!(TimelineParams);
            v1_json(rl, service::timeline(f, q.kind.as_deref(), q.limit, now).await)
        }
        "/site/store/timeline" => {
            let q = q!(TimelineParams);
            site_json(service::timeline(f, q.kind.as_deref(), q.limit, now).await)
        }
        "/site/store/texture" => {
            let TextureParams(p) = q!(TextureParams);
            texture_response(f, &p).await
        }
        p if p.starts_with(V1_PRODUCT) => {
            product_response(f, &code_of(&uri, V1_PRODUCT), now, model::Flavour::V1, rl).await
        }
        p if p.starts_with(SITE_PRODUCT) => {
            product_response(f, &code_of(&uri, SITE_PRODUCT), now, model::Flavour::Site, None).await
        }
        other => panic!("unrouted {other}"),
    }
}

pub(super) async fn parts(resp: Response) -> (u16, HeaderMap, Vec<u8>) {
    let status = resp.status().as_u16();
    let headers = resp.headers().clone();
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.expect("body");
    (status, headers, body.to_vec())
}

pub(super) fn header(h: &HeaderMap, name: &str) -> Option<String> {
    h.get(name).map(|v| String::from_utf8_lossy(v.as_bytes()).into_owned())
}

pub(super) fn rid(case: &Value) -> String {
    arr(case, "headers").iter().find(|p| p[0] == "x-request-id").map(|p| s(&p[1]).to_owned()).expect("rid")
}

#[tokio::test]
async fn http_replay() {
    let now = golden()["now"].as_i64().expect("now");
    let cases = arr(golden(), "http");
    let mut bad = Vec::new();
    for case in cases {
        let name = s(&case["name"]);
        let replay = Replay::new(case);
        let run = AssertUnwindSafe(request_id::scope(rid(case), dispatch(case, &replay, now))).catch_unwind();
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
        let mut compared = vec!["content-type", "x-ratelimit-limit", "x-ratelimit-remaining", "x-ratelimit-reset"];
        if !arr(case, "headers").iter().any(|p| p[0] == "authorization") && status < 500 {
            compared.push("cache-control");
        }
        for h in compared {
            let theirs = want.get(h).map(|v| s(v).to_owned());
            if header(&headers, h) != theirs {
                bad.push(format!("{name}: {h} {:?} want {theirs:?}", header(&headers, h)));
            }
        }
        let want_body = if name.starts_with("texture") {
            base64::engine::general_purpose::STANDARD.decode(s(&case["resp"])).expect("b64")
        } else {
            s(&case["resp"]).as_bytes().to_vec()
        };
        if body != want_body {
            bad.push(format!(
                "{name}:\n  got  {}\n  want {}",
                String::from_utf8_lossy(&body),
                String::from_utf8_lossy(&want_body)
            ));
        }
    }
    assert!(bad.is_empty(), "{} problems over {} cases:\n{}", bad.len(), cases.len(), bad.join("\n"));
}

fn state() -> AppState {
    let settings = Settings::from_vars([("SECRET_KEY", "store-fixture-secret")]).expect("settings");
    AppState { inner: Arc::new(Inner { settings, db: Db { mongo: None, redis: None, pg: None } }) }
}

/// The store routes under the real middleware stack, without stores.
fn app() -> Router {
    let mw =
        Mw::new(MwConfig::from_lookup(&|_| None), None, EmbedSource::Fixed(vec![]), TrustedHosts::parse("127.0.0.1"));
    let inner = middleware::wrap(super::router(), Arc::new(mw), None)
        .fallback(|| async { StatusCode::IM_A_TEAPOT })
        .with_state(state());
    middleware::outer(inner)
}

/// The gated cases that need no store: 405s and the missing credential 401s.
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
        for p in arr(case, "headers") {
            req.headers_mut()
                .append(HeaderName::from_bytes(s(&p[0]).as_bytes()).unwrap(), HeaderValue::from_str(s(&p[1])).unwrap());
        }
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

fn frepr(x: Option<f64>) -> Value {
    x.map_or(Value::Null, |x| Value::String(pyfloat::repr(x)))
}

fn product_value(p: &ParsedProduct) -> Value {
    json!({
        "code": p.code, "kind": p.kind, "name": p.name, "image": p.image, "owned": p.owned,
        "tradable": p.tradable, "informational": p.informational, "info": p.info,
        "prices": p.prices.iter().map(|pr| json!({
            "currency": pr.currency, "cost": frepr(Some(pr.cost)), "can_purchase": pr.can_purchase,
            "monthly": pr.monthly, "sale": pr.sale,
        })).collect::<Vec<_>>(),
        "price_string": p.price_string, "price_string_currency": p.price_string_currency,
        "price_string_sale": p.price_string_sale, "promo": p.promo, "deal_seconds": frepr(p.deal_seconds),
        "interact_label": p.interact_label, "interact_enabled": p.interact_enabled, "trial_limits": p.trial_limits,
        "class_level": p.class_level, "class_power_rank": p.class_power_rank,
        "class_shield_frame": p.class_shield_frame, "class_sub_name": p.class_sub_name, "class_icon": p.class_icon,
        "textures": p.textures.iter().map(|t| json!({
            "texture": t.texture, "x": t.x, "y": t.y, "text": t.text, "overlay": t.overlay,
        })).collect::<Vec<_>>(),
        "loot_title": p.loot_title, "loot_body": p.loot_body, "categories": p.categories,
    })
}

fn dump_value(d: &ParsedStoreDump) -> Value {
    json!({
        "categories": d.categories.iter().map(|c| json!({
            "index": c.index, "label": c.label, "icon": c.icon, "codes": c.codes,
        })).collect::<Vec<_>>(),
        "products": d.products.iter().map(product_value).collect::<Vec<_>>(),
        "balance": d.balance.map_or(Value::Null, |(a, b)| json!([pyfloat::repr(a), pyfloat::repr(b)])),
        "title": d.title, "instructions": d.instructions, "done": d.done,
    })
}

#[test]
fn parse_matches_python() {
    let mut bad = Vec::new();
    for case in arr(golden(), "parse") {
        let ours = dump_value(&parse_dump(s(&case["text"])));
        if ours != case["result"] {
            bad.push(format!("{}:\n  ours {ours}\n  want {}", s(&case["name"]), case["result"]));
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

fn products_of(v: &Value) -> BTreeMap<String, Product> {
    arr(v, "products")
        .iter()
        .map(|d| model::product(&to_doc(d)).expect("product"))
        .map(|p| (p.code.clone(), p))
        .collect()
}

/// Same values, floats compared bit for bit.
fn same(a: &Product, b: &Product) -> bool {
    format!("{a:?}") == format!("{b:?}")
}

#[test]
fn ingest_matches_python() {
    let mut bad = Vec::new();
    for scenario in arr(golden(), "ingest") {
        let name = s(&scenario["name"]);
        let mut products: BTreeMap<String, Product> = scenario["seed"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| model::product(&to_doc(d)).expect("seed"))
            .map(|p| (p.code.clone(), p))
            .collect();
        let mut categories: BTreeMap<i64, Category> = BTreeMap::new();
        let mut state: Option<State> = None;
        for (i, step) in arr(scenario, "steps").iter().enumerate() {
            let anchor = step["anchor"].as_i64().unwrap();
            let dump = parse_dump(s(&step["text"]));
            if dump.products.is_empty() && dump.categories.is_empty() {
                if step["error"] != NO_STORE_DATA {
                    bad.push(format!("{name}[{i}]: Python did not reject an empty dump"));
                }
                continue;
            }
            let prev = state.as_ref().and_then(|s| s.last_anchor);
            let (mut created, mut changed) = (0, 0);
            for p in &dump.products {
                let (doc, c, ch) = ingest_product(products.get(&p.code).cloned(), p, anchor, prev).expect("ingest");
                created += usize::from(c);
                changed += usize::from(ch);
                products.insert(p.code.clone(), doc);
            }
            for c in &dump.categories {
                let cat = ingest_category(categories.get(&c.index).cloned(), c, anchor);
                categories.insert(c.index, cat);
            }
            state = Some(ingest_state(state.unwrap_or_default(), &dump, anchor));
            let summary = Summary {
                products: dump.products.len(),
                categories: dump.categories.len(),
                created,
                price_changes: changed,
                anchor,
                done_marker: dump.done,
            };
            let want = &step["summary"];
            let ours = json!({
                "products": summary.products, "categories": summary.categories, "created": summary.created,
                "price_changes": summary.price_changes, "anchor": summary.anchor, "done_marker": summary.done_marker,
            });
            if &ours != want {
                bad.push(format!("{name}[{i}] summary {ours} want {want}"));
            }
            let want_products = products_of(step);
            if want_products.len() != products.len()
                || want_products.iter().any(|(k, w)| products.get(k).is_none_or(|p| !same(p, w)))
            {
                bad.push(format!("{name}[{i}] products:\n  ours {:?}\n  want {:?}", products, want_products));
            }
            let want_cats: Vec<Category> =
                arr(step, "categories").iter().map(|d| model::category(&to_doc(d)).expect("cat")).collect();
            if want_cats != categories.values().cloned().collect::<Vec<_>>() {
                bad.push(format!("{name}[{i}] categories {categories:?} want {want_cats:?}"));
            }
            let want_state: Vec<State> =
                arr(step, "state").iter().map(|d| model::state(&to_doc(d)).expect("state")).collect();
            if want_state != state.iter().cloned().collect::<Vec<_>>() {
                bad.push(format!("{name}[{i}] state {state:?} want {want_state:?}"));
            }
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

#[test]
fn texture_regex_escape_matches_python() {
    assert_eq!(service::re_escape("ui/(x)+[y].jpg"), r"ui/\(x\)\+\[y\]\.jpg");
    assert_eq!(service::re_escape("a b#&~-é"), r"a\ b\#\&\~\-é");
}

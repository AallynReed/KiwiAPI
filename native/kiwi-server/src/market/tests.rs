//! Golden checks against `native/port/parity/gen_market_fixtures.py`: the pure
//! functions, and every route case replayed through the middleware stack with
//! a fixture [`Store`] that answers from the same canned rows and must make
//! the same asyncpg calls (method, SQL text, arguments) Python made.

use std::{
    collections::{BTreeSet, VecDeque},
    sync::{Arc, Mutex, OnceLock},
};

use axum::{
    Router,
    body::Body,
    extract::{Request, State},
    http::{HeaderName, HeaderValue, Method, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, patch, post, put},
};
use chrono::{TimeZone, Utc};
use hyper_util::{client::legacy::Client, rt::TokioExecutor};
use kiwi_core::market::{
    self as core,
    parser::{created_at, parse_dump, uuid_str},
};
use mongodb::bson::oid::ObjectId;
use serde_json::{Value, json};
use tower::ServiceExt;

use super::{
    Segment, admin,
    pg::{Arg, PgValue, Row},
    reads::{self, DaysQuery, DealsQuery, HistoryQuery, ListingsQuery, TimelineQuery, WindowQuery, site},
    store::{self, Category, Store},
    tomes,
};
use crate::{
    config::Settings,
    db::Db,
    error::{ApiError, ApiResult},
    features,
    middleware::{EmbedSource, Mw, MwConfig, forwarded::TrustedHosts, outer, wrap},
    pyjson::PydanticJson,
    ratelimit::RateLimitInfo,
    state::{AppState, Inner},
    validation::{Field, FromBody, Query, Validator, read_body},
};

fn golden() -> &'static Value {
    static G: OnceLock<Value> = OnceLock::new();
    G.get_or_init(|| serde_json::from_str(include_str!("../../testdata/market/golden.json")).expect("fixture parses"))
}

fn cases(key: &str) -> &'static Vec<Value> {
    golden()[key].as_array().unwrap_or_else(|| panic!("fixture section {key}"))
}

fn s(v: &Value) -> &str {
    v.as_str().expect("string")
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array().expect("array").iter().map(|x| s(x).to_owned()).collect()
}

fn now() -> i64 {
    golden()["now"].as_i64().unwrap()
}

fn uuid_int(text: &str) -> u128 {
    u128::from_str_radix(&text.replace('-', ""), 16).unwrap()
}

// --- pure functions -------------------------------------------------------------------

#[test]
fn order_by_tokens() {
    for c in cases("order_by") {
        assert_eq!(core::order_by(s(&c[0])), s(&c[1]), "{c}");
    }
}

#[test]
fn uuid_timestamps() {
    for c in cases("created_at") {
        let id = uuid_int(s(&c[0]));
        assert_eq!(uuid_str(id), s(&c[0]));
        assert_eq!(created_at(id), c[1].as_i64().unwrap(), "{c}");
    }
}

#[test]
fn dump_parser() {
    for c in cases("parse") {
        let rows: Vec<Value> = parse_dump(s(&c["text"]))
            .into_iter()
            .map(|r| json!([uuid_str(r.id), r.name, r.kind, r.stack, r.price, r.price_each, r.created_at]))
            .collect();
        assert_eq!(Value::Array(rows), c["rows"], "{}", c["text"]);
    }
}

#[test]
fn price_outliers() {
    for c in cases("outliers") {
        let prices: Vec<f64> = c["prices"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
        let got = core::price_outliers(&prices).map(|v| json!(v)).unwrap_or(Value::Null);
        assert_eq!(got, c["dropped"], "{}", c["prices"]);
    }
}

#[test]
fn merchant_bands() {
    for c in cases("bands") {
        let got: Vec<Value> = core::event_bands(c["start"].as_i64().unwrap(), c["end"].as_i64().unwrap())
            .into_iter()
            .map(|b| json!({"name": b.name, "kind": b.kind, "starts_at": b.starts_at, "ends_at": b.ends_at}))
            .collect();
        assert_eq!(Value::Array(got), c["bands"], "{c}");
    }
}

#[test]
fn name_lists() {
    for (key, f) in [
        ("sorted_lower", core::sorted_lower as fn(&[String]) -> Vec<String>),
        ("clean_names", core::clean_names),
        ("clean_items", core::clean_items),
    ] {
        for c in cases(key) {
            assert_eq!(json!(f(&strings(&c["in"]))), c["out"], "{key}: {}", c["in"]);
        }
    }
}

#[test]
fn reorder_saves() {
    for c in cases("reorder") {
        let docs: Vec<(ObjectId, i64, String)> = c["docs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| (ObjectId::parse_str(s(&d[0])).unwrap(), d[1].as_i64().unwrap(), s(&d[2]).to_owned()))
            .collect();
        let keys: Vec<(ObjectId, i64, &str)> = docs.iter().map(|d| (d.0, d.1, d.2.as_str())).collect();
        let ids: Vec<Option<ObjectId>> = strings(&c["ids"]).iter().map(|i| ObjectId::parse_str(i).ok()).collect();
        let saves: Vec<Value> =
            core::reorder(&keys, &ids).into_iter().map(|(i, pos)| json!([docs[i].0.to_hex(), pos])).collect();
        assert_eq!(Value::Array(saves.clone()), c["saves"], "{}", c["ids"]);
        assert_eq!(saves.len() as u64, c["touched"].as_u64().unwrap());
    }
}

#[test]
fn role_ranks() {
    for c in cases("role_rank") {
        assert_eq!(core::item_role_rank(c[0].as_str()) as u64, c[1].as_u64().unwrap(), "{c}");
    }
}

// --- the fixture store ----------------------------------------------------------------

fn pg_value(v: &Value) -> PgValue {
    match v {
        Value::Null => PgValue::Null,
        Value::Bool(b) => PgValue::Bool(*b),
        Value::Number(n) if n.is_f64() => PgValue::Float(n.as_f64().unwrap()),
        Value::Number(n) => PgValue::Int(n.as_i64().unwrap()),
        Value::String(t) => PgValue::Text(t.clone()),
        Value::Object(o) if o.contains_key("$uuid") => PgValue::Uuid(uuid_int(s(&o["$uuid"]))),
        Value::Object(o) if o.contains_key("$numeric") => PgValue::Numeric(s(&o["$numeric"]).to_owned()),
        other => panic!("no column value for {other}"),
    }
}

fn row(v: &Value) -> Row {
    Row(v.as_object().expect("row").iter().map(|(k, v)| (k.clone(), pg_value(v))).collect())
}

fn arg_json(a: &Arg) -> Value {
    match a {
        Arg::Int(i) => json!(i),
        Arg::Float(f) => json!(f),
        Arg::Text(t) => json!(t),
        Arg::TextList(v) => {
            let mut v = v.clone();
            v.sort();
            json!(v)
        }
    }
}

struct Fixture {
    replies: Mutex<VecDeque<Value>>,
    calls: Mutex<Vec<Value>>,
    pg: bool,
    store: bool,
}

impl Fixture {
    fn new(case: &Value) -> Arc<Self> {
        Arc::new(Fixture {
            replies: Mutex::new(case["replies"].as_array().unwrap().iter().cloned().collect()),
            calls: Mutex::new(Vec::new()),
            pg: case["pg_enabled"] == true,
            store: case["store"] == true,
        })
    }
}

impl Store for Arc<Fixture> {
    fn now(&self) -> i64 {
        now()
    }

    fn now_us(&self) -> i64 {
        now() * 1_000_000
    }

    fn pg_enabled(&self) -> bool {
        self.pg
    }

    async fn run(&self, method: &'static str, sql: &str, args: Vec<Arg>) -> ApiResult<Vec<Row>> {
        let args: Vec<Value> = args.iter().map(arg_json).collect();
        self.calls.lock().unwrap().push(json!({"method": method, "sql": sql, "args": args}));
        let reply = self.replies.lock().unwrap().pop_front().unwrap_or_else(|| panic!("no canned reply for {sql}"));
        if reply.get("$error").is_some() {
            return Err(ApiError::internal());
        }
        Ok(match method {
            "fetch" => reply.as_array().unwrap().iter().map(row).collect(),
            "fetchrow" if reply.is_null() => vec![],
            "fetchrow" => vec![row(&reply)],
            "fetchval" => vec![Row(vec![("?column?".into(), pg_value(&reply))])],
            _ => vec![],
        })
    }

    async fn interest(&self) -> ApiResult<BTreeSet<String>> {
        let items = if self.store { strings(&golden()["store"]["interest"]) } else { vec![] };
        Ok(items.into_iter().collect())
    }

    async fn categories(&self) -> ApiResult<Vec<Category>> {
        if !self.store {
            return Ok(vec![]);
        }
        let at = Utc.timestamp_opt(now(), 0).unwrap();
        Ok(golden()["store"]["categories"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| Category {
                id: ObjectId::parse_str(s(&c["id"])).unwrap(),
                name: s(&c["name"]).to_owned(),
                order: c["order"].as_i64().unwrap(),
                items: strings(&c["items"]),
                created_by: None,
                created_at: at,
            })
            .collect())
    }
}

// --- the fixture app ------------------------------------------------------------------

/// A store-less state: enough for routing, 405s, rejected auth and the
/// admin answers that never reach Mongo.
fn state() -> AppState {
    let settings = Settings::from_vars([("PY_UPSTREAM", "http://py:8000")]).unwrap();
    let upstream = Client::builder(TokioExecutor::new()).build_http();
    AppState { inner: Arc::new(Inner { settings, db: Db { mongo: None, redis: None, pg: None }, upstream }) }
}

fn stack(routes: Router<AppState>) -> Router {
    let mw = Arc::new(Mw::new(
        MwConfig::from_lookup(&|_| None),
        None,
        EmbedSource::Fixed(Vec::new()),
        TrustedHosts::parse("127.0.0.1"),
    ));
    outer(wrap(routes, mw, None).fallback(|| async { (StatusCode::IM_A_TEAPOT, "proxied") }).with_state(state()))
}

/// The body without the superuser check (the fixture overrides it).
async fn body_of<T: FromBody>(req: Request) -> Result<T, Response> {
    let (parts, body) = req.into_parts();
    let body = read_body(&parts.headers, body).await.map_err(IntoResponse::into_response)?;
    let mut v = Validator::default();
    let value = T::from_body(Field::new(&mut v, vec!["body".into()], body.input()));
    match v.finish(value) {
        Ok(Some(t)) => Ok(t),
        Ok(None) => Err(ApiError::internal().into_response()),
        Err(e) => Err(e.into_response()),
    }
}

type Fx = State<Arc<Fixture>>;

fn json_or(r: ApiResult<Value>) -> Response {
    r.map(PydanticJson).into_response()
}

fn archive_info() -> RateLimitInfo {
    RateLimitInfo { limit: 10, remaining: 9, reset: Utc.with_ymd_and_hms(2026, 10, 22, 12, 0, 0).unwrap().timestamp() }
}

const ADMIN: &str = "65a1b2c3d4e5f60718293a4b";

/// The production routes with auth overridden and the fixture as the store.
fn fixture_app(fx: Arc<Fixture>) -> Router {
    let admin_id = ObjectId::parse_str(ADMIN).unwrap();
    let routes: Router<Arc<Fixture>> = Router::new()
        .route(
            "/site/market/listings",
            get(|State(fx): Fx, Query(q): Query<ListingsQuery<500>>| async move {
                let body = match q.check_range() {
                    Ok(()) => reads::listings_core(&fx, &q).await,
                    Err(e) => Err(e),
                };
                site(body, 30)
            }),
        )
        .route(
            "/v1/market/listings",
            get(|State(fx): Fx, Query(q): Query<ListingsQuery<1000>>| async move {
                if let Err(e) = q.check_range() {
                    return e.into_response();
                }
                let archive = if q.hide_expired { Default::default() } else { reads::archive_headers(archive_info()) };
                match reads::listings_core(&fx, &q).await {
                    Ok(v) => (archive, PydanticJson(v)).into_response(),
                    Err(e) => e.into_response(),
                }
            }),
        )
        .route(
            "/v1/market/items",
            get(|State(fx): Fx| async move {
                json_or(store::distinct_items(&fx).await.map(|i| json!({"items": i, "count": i.len()})))
            }),
        )
        .route(
            "/v1/market/items/{name}/summary",
            get(|State(fx): Fx, Segment(n): Segment<0>| async move { json_or(reads::summary_core(&fx, &n).await) }),
        )
        .route(
            "/v1/market/reset",
            post(|State(fx): Fx| async move {
                json_or(store::reset(&fx).await.map(|n| json!({"reset": true, "removed": n})))
            }),
        )
        .route(
            "/site/market/items/{name}/summary",
            get(|State(fx): Fx, Segment(n): Segment<1>| async move { site(reads::summary_core(&fx, &n).await, 30) }),
        )
        .route(
            "/site/market/items/{name}/history",
            get(|State(fx): Fx, Segment(n): Segment<2>, Query(q): Query<HistoryQuery>| async move {
                site(reads::history_core(&fx, &n, &q).await, 30)
            }),
        )
        .route("/site/market/items", get(|State(fx): Fx| async move { site(reads::items_core(&fx).await, 60) }))
        .route(
            "/site/market/item-images",
            get(|State(fx): Fx| async move { site(Ok(reads::item_images_core(&fx).await), 300) }),
        )
        .route(
            "/site/market/analytics/timeline",
            get(|State(fx): Fx, Query(q): Query<TimelineQuery>| async move {
                site(reads::timeline_core(&fx, &q).await, 120)
            }),
        )
        .route(
            "/site/market/analytics/deals",
            get(|State(fx): Fx, Query(q): Query<DealsQuery>| async move { site(reads::deals_core(&fx, &q).await, 60) }),
        )
        .route(
            "/site/market/analytics/movers",
            get(|State(fx): Fx, Query(q): Query<WindowQuery<7, 40>>| async move {
                site(reads::movers_core(&fx, q.days, q.limit).await, 120)
            }),
        )
        .route(
            "/site/market/analytics/overview",
            get(|State(fx): Fx, Query(q): Query<DaysQuery<1, 30, 7>>| async move {
                site(reads::overview_core(&fx, q.days).await, 120)
            }),
        )
        .route(
            "/site/market/analytics/signals",
            get(|State(fx): Fx, Query(q): Query<DaysQuery<10, 60, 21>>| async move {
                site(reads::signals_core(&fx, q.days).await, 600)
            }),
        )
        .route(
            "/site/market/analytics/liquidity",
            get(|State(fx): Fx, Query(q): Query<WindowQuery<14, 40>>| async move {
                site(reads::liquidity_core(&fx, q.days, q.limit).await, 120)
            }),
        )
        .route(
            "/site/market/analytics/volume",
            get(|State(fx): Fx, Query(q): Query<WindowQuery<14, 40>>| async move {
                site(reads::volume_core(&fx, q.days, q.limit).await, 120)
            }),
        )
        .route(
            "/site/tomes",
            get(|State(fx): Fx| async move {
                let raw = crate::gamedata::read_json(std::path::Path::new("../../assets/gamedata/tomes.json"))
                    .expect("tomes.json");
                let table = tomes::parse_table(&raw).expect("tome table");
                site(tomes::valued(&fx, &table).await, 120)
            }),
        )
        .route(
            "/admin/market/interest-items",
            post(move |req: Request| async move {
                match body_of::<admin::NameBody>(req).await {
                    Ok(b) => admin::add_interest_core(&state(), admin_id, &b.name)
                        .await
                        .unwrap_or_else(|e| e.into_response()),
                    Err(r) => r,
                }
            })
            .put(move |req: Request| async move {
                match body_of::<admin::ItemsBody>(req).await {
                    Ok(b) => json_or(admin::replace_interest_core(&state(), admin_id, &b.items).await),
                    Err(r) => r,
                }
            }),
        )
        .route(
            "/admin/market/categories",
            post(move |req: Request| async move {
                match body_of::<admin::NameBody>(req).await {
                    Ok(b) => admin::create_category_core(&state(), admin_id, &b.name)
                        .await
                        .unwrap_or_else(|e| e.into_response()),
                    Err(r) => r,
                }
            }),
        )
        .route(
            "/admin/market/categories/{category_id}",
            patch(|Segment(id): Segment<4>, req: Request| async move {
                match body_of::<admin::UpdateBody>(req).await {
                    Ok(b) => json_or(admin::update_category_core(&state(), &id, &b).await),
                    Err(r) => r,
                }
            })
            .delete(|Segment(id): Segment<4>| async move {
                admin::delete_category_core(&state(), &id).await.unwrap_or_else(|e| e.into_response())
            }),
        )
        .route(
            "/admin/market/categories/order",
            put(|req: Request| async move {
                match body_of::<admin::IdsBody>(req).await {
                    Ok(b) => json_or(admin::reorder_core(&state(), &b.ids).await.map(|n| json!({"reordered": n}))),
                    Err(r) => r,
                }
            })
            .patch(|Segment(id): Segment<4>, req: Request| async move {
                match body_of::<admin::UpdateBody>(req).await {
                    Ok(b) => json_or(admin::update_category_core(&state(), &id, &b).await),
                    Err(r) => r,
                }
            })
            .delete(|Segment(id): Segment<4>| async move {
                admin::delete_category_core(&state(), &id).await.unwrap_or_else(|e| e.into_response())
            }),
        );
    stack(routes.with_state(fx))
}

/// The gate answers for a disabled feature (`/site` plain 404, `/v1` the named one).
fn disabled_app(url: &str) -> Router {
    let path = url.split('?').next().unwrap().to_owned();
    let err = if path.starts_with("/v1/") {
        features::disabled("Market")
    } else {
        ApiError::http(StatusCode::NOT_FOUND, None)
    };
    let msg = err.message.clone();
    let routes = Router::new().route(
        &path,
        get(move || {
            let msg = msg.clone();
            async move { ApiError::new(StatusCode::NOT_FOUND, "not_found", msg).into_response() }
        }),
    );
    stack(routes)
}

#[tokio::test]
async fn routes_match_python() {
    for c in cases("http") {
        let name = s(&c["name"]);
        let fx = Fixture::new(c);
        let app = if c["real"] == true {
            stack(super::router())
        } else if !c["disabled"].is_null() {
            disabled_app(s(&c["url"]))
        } else {
            fixture_app(fx.clone())
        };
        let mut req =
            Request::builder().method(Method::from_bytes(s(&c["method"]).as_bytes()).unwrap()).uri(s(&c["url"]));
        for (k, v) in c["headers"].as_object().unwrap() {
            if k != "x-test-auth" {
                req = req.header(HeaderName::from_bytes(k.as_bytes()).unwrap(), HeaderValue::from_str(s(v)).unwrap());
            }
        }
        let body = c["body"].as_str().map_or_else(Body::empty, |b| Body::from(b.to_owned()));
        let req = req.header("host", "testserver").header("x-request-id", s(&c["request_id"])).body(body).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status().as_u16() as u64, c["status"].as_u64().unwrap(), "{name}: status");
        let expected = c["resp_headers"].as_object().unwrap();
        for h in [
            "content-type",
            "cache-control",
            "allow",
            "www-authenticate",
            "x-ratelimit-archive-limit",
            "x-ratelimit-archive-remaining",
            "x-ratelimit-archive-reset",
            "x-ratelimit-limit",
        ] {
            let got = resp.headers().get(h).map(|v| Value::from(v.to_str().unwrap()));
            assert_eq!(got.as_ref(), expected.get(h), "{name}: {h}");
        }
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        assert_eq!(String::from_utf8(bytes.to_vec()).unwrap(), s(&c["response"]), "{name}: body");
        if c["real"] != true && c["disabled"].is_null() {
            let calls = Value::Array(fx.calls.lock().unwrap().clone());
            assert_eq!(calls, c["calls"], "{name}: calls");
            assert!(fx.replies.lock().unwrap().is_empty(), "{name}: unused replies");
        }
    }
}

#[test]
fn numeric_wire_format() {
    let enc = |ndigits: u16, weight: i16, sign: u16, dscale: u16, digits: &[u16]| {
        let mut b = Vec::new();
        for w in [ndigits, weight as u16, sign, dscale] {
            b.extend(w.to_be_bytes());
        }
        for d in digits {
            b.extend(d.to_be_bytes());
        }
        super::pg::numeric_text(&b).unwrap()
    };
    assert_eq!(enc(0, 0, 0, 0, &[]), "0");
    assert_eq!(enc(3, 2, 0, 0, &[9, 8765, 4321]), "987654321");
    assert_eq!(enc(1, 1, 0, 0, &[12]), "120000");
    assert_eq!(enc(2, 0, 0x4000, 2, &[5, 2500]), "-5.25");
    assert_eq!(enc(1, -1, 0, 4, &[1]), "0.0001");
    assert_eq!(enc(1, -2, 0, 8, &[50]), "0.00000050");
}

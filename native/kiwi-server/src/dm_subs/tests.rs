//! DM subscriptions against Python (`testdata/webhooks/golden.json`): DM
//! bodies, `send_dm` over scripted replies, queue fan-out, the market
//! watchlist and every route past auth over an in-memory collection.

use axum::{
    body::Body,
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use mongodb::bson::oid::ObjectId;
use serde_json::{Value, json};

use super::{delivery, embeds, model::COLLECTION, service};
use crate::{
    dt,
    error::ApiError,
    outbound::{
        Outcome,
        discord::send_dm,
        testkit::{Canned, FakeColl, Scripted, arr, check_response, diff_docs, docs, golden, new_id, parts, rid, s},
    },
    pyjson::PydanticJson,
    request_id,
    validation::{BodyInput, FromBody, read_body, validate},
};

fn same(a: &Value, b: &Value) -> bool {
    serde_json::to_string(a).unwrap() == serde_json::to_string(b).unwrap()
}

#[test]
fn bodies_match_python() {
    let g = &golden()["dm"];
    let mut bad = Vec::new();
    for case in arr(g, "build") {
        let got = embeds::build(s(&case["event"]), &case["data"]);
        let want = &case["body"];
        match (got, want.get("ok")) {
            (Ok(v), Some(w)) => {
                let v = v.unwrap_or(Value::Null);
                if !same(&v, w) {
                    bad.push(format!("{} {}:\n  got  {v}\n  want {w}", case["event"], case["data"]));
                }
            }
            (Err(_), None) => {}
            (got, _) => bad.push(format!("{} {}: {got:?} want {want}", case["event"], case["data"])),
        }
    }
    assert!(same(&embeds::test_body(), &g["test_body"]), "test_body");
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

#[tokio::test]
async fn send_dm_matches_python() {
    let mut bad = Vec::new();
    let body = json!({"embeds": [{"title": "🥝 x"}]});
    for case in arr(golden(), "dm_send") {
        let t = Scripted::new(arr(case, "script"));
        let got = send_dm(&t, case["token"].as_str(), 312345678901234567, &body).await;
        let what = format!("{} {}", case["token"], case["script"]);
        match (got, case["result"].get("ok")) {
            (Ok(o), Some(w)) => {
                if !same(&json!([o.ok, o.status, o.error]), w) {
                    bad.push(format!("{what}: {o:?} want {w}"));
                }
            }
            (Err(_), None) => {}
            (got, _) => bad.push(format!("{what}: {got:?} want {}", case["result"])),
        }
        if !same(&json!(*t.sleeps.lock().unwrap()), &case["sleeps"]) {
            bad.push(format!("{what}: sleeps {:?}", t.sleeps.lock().unwrap()));
        }
        let want: Vec<Value> = arr(case, "requests")
            .iter()
            .map(|r| json!({"url": r["url"], "auth": r["auth"], "raw": r["raw"], "multipart": false, "payload_json": null}))
            .collect();
        if !same(&json!(*t.requests.lock().unwrap()), &json!(want)) {
            bad.push(format!("{what}: requests {:?}\n want {want:?}", t.requests.lock().unwrap()));
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

struct FakeDm(Canned);

impl delivery::Dm for FakeDm {
    async fn send(&self, discord_id: i64, body: &Value) -> anyhow::Result<Outcome> {
        Ok(self.0.next(json!({"discord_id": discord_id, "body": body})))
    }
}

fn dm(outcomes: &[Value]) -> FakeDm {
    FakeDm(Canned::new(outcomes, Outcome { ok: true, status: Some(200), error: None }))
}

#[tokio::test]
async fn queue_fanout_matches_python() {
    let mut bad = Vec::new();
    let mut n = 0;
    for case in arr(golden(), "queue").iter().filter(|c| c["feature"] == "dm_subs") {
        n += 1;
        let coll = FakeColl::new(docs(case, "before", COLLECTION), None);
        let d = dm(arr(case, "outcomes"));
        let res = delivery::deliver(&coll, &d, &case["job"]).await;
        let what = format!("job {}", case["job"]);
        if res.is_err() != !case["raises"].is_null() {
            bad.push(format!("{what}: {res:?} want raises {}", case["raises"]));
        }
        let sends = json!(*d.0.sends.lock().unwrap());
        if !same(&sends, &case["sends"]) {
            bad.push(format!("{what}: sends\n  got  {sends}\n  want {}", case["sends"]));
        }
        bad.extend(diff_docs(&what, &coll.snapshot(), &docs(case, "after", COLLECTION), true));
    }
    assert!(n >= 8, "queue cases missing");
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

#[tokio::test]
async fn market_watch_matches_python() {
    let mut bad = Vec::new();
    for case in arr(golden(), "market") {
        let coll = FakeColl::new(docs(case, "before", COLLECTION), None);
        let d = dm(arr(case, "outcomes"));
        let prices: Vec<(String, f64)> =
            case["prices"].as_object().unwrap().iter().map(|(k, v)| (k.clone(), v.as_f64().unwrap())).collect();
        if !prices.is_empty() {
            delivery::check_market_with(&coll, &d, &prices, case["now"].as_i64().unwrap()).await.unwrap();
        }
        let what = format!("prices {}", case["prices"]);
        let sends = json!(*d.0.sends.lock().unwrap());
        if !same(&sends, &case["sends"]) {
            bad.push(format!("{what}: sends\n  got  {sends}\n  want {}", case["sends"]));
        }
        bad.extend(diff_docs(&what, &coll.snapshot(), &docs(case, "after", COLLECTION), true));
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

fn now() -> chrono::DateTime<chrono::Utc> {
    dt::parse_utc(s(&golden()["now"])).expect("now")
}

async fn body_of<T: FromBody>(case: &Value) -> Result<T, ApiError> {
    let mut headers = HeaderMap::new();
    for p in arr(case, "headers") {
        headers.append(HeaderName::from_bytes(s(&p[0]).as_bytes()).unwrap(), HeaderValue::from_str(s(&p[1])).unwrap());
    }
    let bytes = match (case.get("body"), case.get("raw")) {
        (_, Some(raw)) => s(raw).as_bytes().to_vec(),
        (Some(b), _) => crate::pyjson::dumps_spaced(b).into_bytes(),
        _ => Vec::new(),
    };
    let input: BodyInput = read_body(&headers, Body::from(bytes)).await?;
    validate::<T>(&input)
}

fn reply(r: Result<Value, ApiError>, ok: StatusCode) -> Response {
    match r {
        Ok(v) => (ok, PydanticJson(v)).into_response(),
        Err(e) => e.into_response(),
    }
}

async fn dispatch(case: &Value, coll: &FakeColl, d: &FakeDm) -> Response {
    let path = s(&case["url"]);
    let method = s(&case["method"]);
    if path == "/v1/dm-subscriptions/events" && method == "GET" {
        return PydanticJson(
            json!({"events": super::model::EVENT_TYPES, "challenge_types": super::model::CHALLENGE_TYPES}),
        )
        .into_response();
    }
    let actor = ObjectId::parse_str(s(&case["user"]["id"])).unwrap();
    let discord = case["user"]["discord_id"].as_i64();
    let id = path.strip_prefix("/v1/dm-subscriptions/").map(|r| r.strip_suffix("/test").unwrap_or(r));
    match (method, id) {
        ("GET", None) => reply(service::list(coll, actor, discord).await, StatusCode::OK),
        ("POST", None) => match body_of::<service::DmSubCreate>(case).await {
            Ok(req) => reply(service::create(coll, actor, discord, &req, now()).await, StatusCode::CREATED),
            Err(e) => e.into_response(),
        },
        ("PATCH", Some(id)) => match body_of::<service::DmSubUpdate>(case).await {
            Ok(req) => reply(service::update(coll, actor, discord, id, &req, now()).await, StatusCode::OK),
            Err(e) => e.into_response(),
        },
        ("DELETE", Some(id)) => match service::delete(coll, actor, id).await {
            Ok(()) => StatusCode::NO_CONTENT.into_response(),
            Err(e) => e.into_response(),
        },
        ("POST", Some(id)) => reply(service::send_test(coll, d, actor, id).await, StatusCode::OK),
        other => panic!("unrouted {other:?}"),
    }
}

#[tokio::test]
async fn routes_match_python() {
    let mut bad = Vec::new();
    let cases: Vec<&Value> = arr(golden(), "http").iter().filter(|c| s(&c["name"]).starts_with("dm_")).collect();
    for case in &cases {
        let name = s(&case["name"]);
        let coll = FakeColl::new(docs(case, "before", COLLECTION), new_id(case, COLLECTION));
        let d = dm(arr(case, "outcomes"));
        let resp = request_id::scope(rid(case), dispatch(case, &coll, &d)).await;
        let (status, headers, body) = parts(resp).await;
        bad.extend(check_response(case, status, &headers, &body, &[]));
        bad.extend(diff_docs(name, &coll.snapshot(), &docs(case, "after", COLLECTION), false));
        let sends = json!(*d.0.sends.lock().unwrap());
        if !same(&sends, &case["sends"]) {
            bad.push(format!("{name}: sends\n  got  {sends}\n  want {}", case["sends"]));
        }
    }
    assert!(cases.len() > 20, "route cases missing");
    assert!(bad.is_empty(), "{} problems:\n{}", bad.len(), bad.join("\n"));
}

#[test]
fn flux_rounds_half_to_even() {
    assert_eq!(embeds::flux(Some(&json!(2.5))).unwrap(), "2");
    assert_eq!(embeds::flux(Some(&json!(3.5))).unwrap(), "4");
    assert_eq!(embeds::flux(Some(&json!(1_500_000.0))).unwrap(), "1.5M");
    assert_eq!(embeds::flux(Some(&json!("x"))).unwrap(), "?");
}

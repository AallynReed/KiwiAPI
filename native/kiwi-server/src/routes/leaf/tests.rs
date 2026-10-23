//! Byte-for-byte checks against `testdata/leaf/golden.json`, captured from the
//! real Python app and services by `native/port/parity/gen_leaf_fixtures.py`.

use std::sync::{Arc, OnceLock};

use axum::{
    Router,
    body::Body,
    http::{HeaderMap, HeaderName, HeaderValue, Method, Request, StatusCode, Uri},
    response::{IntoResponse, Response},
};
use chrono::{DateTime, TimeZone, Utc};
use kiwi_core::leaf::{clean_supporter_names, normalize_handle, py_strip};
use mongodb::bson::{Bson, Document, oid::ObjectId};
use serde_json::{Value, json};
use tower::ServiceExt;

use super::{
    design_feedback::{Days, DesignVote, count_of, since_day, summary_body, tally_docs},
    meta,
    moderation::{reachable, report_doc, target_query},
    supporters::{self, AddRequest, REPLACE_EMPTY, ReplaceRequest, Supporter},
};
use crate::{
    config::Settings,
    db::Db,
    error::ApiError,
    middleware::{self, EmbedSource, Mw, MwConfig, forwarded::TrustedHosts},
    pyjson::{self, PydanticJson},
    ratelimit::RateLimitInfo,
    request_id,
    state::{AppState, Inner},
    validation::{FromParams, Params, read_body, validate},
};

fn golden() -> &'static Value {
    static G: OnceLock<Value> = OnceLock::new();
    G.get_or_init(|| serde_json::from_str(include_str!("../../../testdata/leaf/golden.json")).expect("fixture"))
}

fn s(v: &Value) -> &str {
    v.as_str().unwrap_or_else(|| panic!("string: {v}"))
}

fn arr<'a>(v: &'a Value, key: &str) -> &'a Vec<Value> {
    v[key].as_array().unwrap_or_else(|| panic!("fixture section {key}"))
}

fn latin1(s: &str) -> Vec<u8> {
    s.chars().map(|c| c as u32 as u8).collect()
}

fn settings() -> Settings {
    let env = golden()["env"].as_object().expect("env");
    let vars: Vec<(String, String)> = env.iter().map(|(k, v)| (k.clone(), s(v).to_owned())).collect();
    Settings::from_vars(vars).expect("settings")
}

fn state() -> AppState {
    let db = Db { mongo: None, redis: None, pg: None };
    AppState { inner: Arc::new(Inner { settings: settings(), db }) }
}

/// The leaf routes under the real middleware stack, without stores.
fn app() -> Router {
    let mw =
        Mw::new(MwConfig::from_lookup(&|_| None), None, EmbedSource::Fixed(vec![]), TrustedHosts::parse("127.0.0.1"));
    let inner = middleware::wrap(super::router(), Arc::new(mw), None)
        .fallback(|| async { StatusCode::IM_A_TEAPOT })
        .with_state(state());
    middleware::outer(inner)
}

fn headers_of(case: &Value) -> HeaderMap {
    let mut h = HeaderMap::new();
    for pair in arr(case, "headers") {
        let name = HeaderName::from_bytes(s(&pair[0]).as_bytes()).expect("name");
        h.append(name, HeaderValue::from_bytes(&latin1(s(&pair[1]))).expect("value"));
    }
    h
}

async fn parts(resp: Response) -> (u16, HeaderMap, String) {
    let status = resp.status().as_u16();
    let headers = resp.headers().clone();
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.expect("body");
    (status, headers, String::from_utf8(body.to_vec()).expect("utf-8"))
}

const COMPARED: [&str; 7] = [
    "content-type",
    "allow",
    "www-authenticate",
    "cache-control",
    "x-ratelimit-limit",
    "x-ratelimit-remaining",
    "x-ratelimit-reset",
];

fn check(bad: &mut Vec<String>, case: &Value, status: u16, headers: &HeaderMap, body: &str) {
    let name = s(&case["name"]);
    if u64::from(status) != case["status"].as_u64().expect("status") {
        bad.push(format!("{name}: status {status} want {}", case["status"]));
    }
    for h in COMPARED {
        let ours = headers.get(h).map(|v| String::from_utf8_lossy(v.as_bytes()).into_owned());
        let want = case["resp_headers"].get(h).map(|v| s(v).to_owned());
        if ours != want {
            bad.push(format!("{name}: {h} {ours:?} want {want:?}"));
        }
    }
    if body != s(&case["resp"]) {
        bad.push(format!("{name}:\n  got  {body}\n  want {}", s(&case["resp"])));
    }
}

fn report(section: &str, n: usize, bad: Vec<String>) {
    assert!(bad.is_empty(), "{section}: {} of {n} differ:\n{}", bad.len(), bad.join("\n"));
}

#[tokio::test]
async fn http_replay() {
    let app = app();
    let cases = arr(golden(), "http");
    let mut bad = Vec::new();
    for case in cases {
        let mut req = Request::builder().method(Method::from_bytes(s(&case["method"]).as_bytes()).expect("method"));
        req = req.uri(s(&case["url"]));
        let mut req = req.body(Body::from(latin1(s(&case["body"])))).expect("request");
        *req.headers_mut() = headers_of(case);
        let (status, headers, body) = parts(app.clone().oneshot(req).await.expect("infallible")).await;
        check(&mut bad, case, status, &headers, &body);
    }
    report("http", cases.len(), bad);
}

#[tokio::test]
async fn docs_405_allow() {
    let req = Request::builder().method("POST").uri("/docs").body(Body::empty()).unwrap();
    let (status, headers, _) = parts(app().oneshot(req).await.unwrap()).await;
    assert_eq!(status, 405);
    assert_eq!(headers["allow"], "GET, HEAD");
}

fn rid_of(case: &Value) -> String {
    let h = headers_of(case);
    String::from_utf8(h["x-request-id"].as_bytes().to_vec()).expect("ascii id")
}

fn ok_body() -> String {
    pyjson::pydantic(&json!({"ok": true}))
}

#[tokio::test]
async fn validation_after_dependencies() {
    let cases = arr(golden(), "validation");
    let mut bad = Vec::new();
    let mut votes = Vec::new();
    for case in cases {
        let name = s(&case["name"]);
        let headers = headers_of(case);
        let body = Body::from(latin1(s(&case["body"])));
        let resp = request_id::scope(rid_of(case), async {
            if let Some(q) = name.strip_prefix("days_").map(|_| s(&case["url"]).split_once('?').map(|x| x.1)) {
                let mut p = Params::new(vec![], q, &headers);
                let v = Days::from_params(&mut p);
                return p.v.finish(v).map(|_| StatusCode::OK.into_response()).unwrap_or_else(|e| e.into_response());
            }
            let input = match read_body(&headers, body).await {
                Ok(i) => i,
                Err(e) => return e.into_response(),
            };
            let res: Result<Response, ApiError> = if name.starts_with("vote_") {
                validate::<DesignVote>(&input).map(|v| {
                    votes.push(vec![v.design, v.theme, v.vote]);
                    (StatusCode::ACCEPTED, PydanticJson(json!({"ok": true}))).into_response()
                })
            } else if name.starts_with("add_") {
                validate::<AddRequest>(&input).map(|_| StatusCode::OK.into_response())
            } else {
                validate::<ReplaceRequest>(&input).map(|_| StatusCode::OK.into_response())
            };
            res.unwrap_or_else(IntoResponse::into_response)
        })
        .await;
        let (status, headers, body) = parts(resp).await;
        check(&mut bad, case, status, &headers, &body);
    }
    report("validation", cases.len(), bad);
    let recorded: Vec<Vec<String>> = serde_json::from_value(golden()["recorded_votes"].clone()).unwrap();
    assert_eq!(votes, recorded[..votes.len()]);
}

fn response_case(name: &str) -> &'static Value {
    arr(golden(), "responses").iter().find(|c| c["name"] == name).unwrap_or_else(|| panic!("case {name}"))
}

async fn check_response(name: &str, build: impl FnOnce() -> Response) {
    let case = response_case(name);
    let resp = request_id::scope(rid_of(case), async { build() }).await;
    let (status, headers, body) = parts(resp).await;
    let mut bad = Vec::new();
    check(&mut bad, case, status, &headers, &body);
    report(name, 1, bad);
}

fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32, sec: u32, us: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(y, mo, d, h, mi, sec).unwrap() + chrono::Duration::microseconds(us.into())
}

const ADMIN: &str = "65a1b2c3d4e5f60718293a4b";

#[tokio::test]
async fn success_shapes() {
    let names: Vec<String> =
        ["IINikstarII", "Nao373", "Wahoo", "børyzje", "nz \"q\"", "Tues\u{2028}", "\u{1F600}"].map(String::from).into();
    let reset = Utc.with_ymd_and_hms(2026, 1, 2, 3, 4, 5).unwrap().timestamp();
    check_response("public_list", || {
        supporters::public_response(&names, RateLimitInfo { limit: 30, remaining: 29, reset })
    })
    .await;

    let created = utc(2025, 3, 4, 5, 6, 7, 123_000);
    let rows = [
        Supporter { name: "IINikstarII".into(), added_by: None, created_at: created },
        Supporter {
            name: "ünï".into(),
            added_by: Some(ObjectId::parse_str(ADMIN).unwrap()),
            created_at: utc(2025, 3, 4, 5, 6, 7, 0),
        },
    ];
    check_response("admin_list", || PydanticJson(supporters::admin_list_body(&rows)).into_response()).await;

    let added = Supporter {
        name: py_strip("  Néw  ").into(),
        added_by: Some(ObjectId::parse_str(ADMIN).unwrap()),
        created_at: utc(2025, 3, 4, 5, 6, 7, 123_456),
    };
    check_response("admin_add", || (StatusCode::CREATED, PydanticJson(added.admin_view())).into_response()).await;
    check_response("admin_replace", || PydanticJson(json!({"removed": 7, "added": 2})).into_response()).await;
    check_response("admin_delete", supporters::no_content).await;

    let uri: Uri = "/admin/supporters/n%C3%A9'%22x".parse().unwrap();
    let name = supporters::name_from_path(&uri).unwrap();
    check_response("admin_delete_missing", || {
        ApiError::not_found(format!("No supporter named '{name}'")).into_response()
    })
    .await;
    assert_eq!(supporters::name_from_path(&"/admin/supporters/a%2Fb".parse().unwrap()), None);
    assert_eq!(supporters::name_from_path(&"/admin/supporters/%20nz".parse().unwrap()).as_deref(), Some(" nz"));

    let body = ok_body();
    assert_eq!(s(&response_case("df_post")["resp"]), body);
    let recorded = &golden()["recorded_votes"][2];
    assert_eq!(recorded, &json!(["voxel", "auto", "up"]));
}

#[test]
fn supporter_rows_and_cleanup() {
    for case in arr(golden(), "clean") {
        let names: Vec<String> = serde_json::from_value(case["names"].clone()).unwrap();
        let got = clean_supporter_names(&names);
        match case["out"].as_array() {
            Some(want) => {
                let want: Vec<&str> = want.iter().map(s).collect();
                assert_eq!(got, want, "{names:?}");
            }
            None => {
                assert!(got.is_empty(), "{names:?}");
                assert_eq!(s(&case["error"]), REPLACE_EMPTY);
            }
        }
    }
    for case in arr(golden(), "add_errors") {
        let name = py_strip(s(&case["name"]));
        let lookup: Vec<&str> = arr(case, "lookup").iter().map(s).collect();
        if name.is_empty() {
            assert!(lookup.is_empty());
            assert_eq!(s(&case["error"]), "Supporter name cannot be empty");
            continue;
        }
        assert_eq!(lookup, [name]);
        if let Some(err) = case["error"].as_str() {
            assert_eq!(err, format!("Supporter '{name}' already exists"));
        } else {
            assert_eq!(s(&case["stored"]), name);
        }
    }
    let row = Supporter { name: "x".into(), added_by: None, created_at: utc(2025, 1, 1, 0, 0, 0, 1500) };
    let doc = row.to_doc(ObjectId::new());
    let keys: Vec<&str> = doc.keys().skip(1).map(String::as_str).collect();
    let want: Vec<&str> = arr(&golden()["models"], "supporter").iter().map(s).collect();
    assert_eq!(keys, want);
    assert_eq!(doc.get("added_by"), Some(&Bson::Null));
    assert_eq!(doc.get_datetime("created_at").unwrap().timestamp_millis(), 1_735_689_600_001);
    assert_eq!(Supporter::from_doc(&doc).unwrap().created_at, utc(2025, 1, 1, 0, 0, 0, 1000));
}

fn bson_of(v: &Value) -> Bson {
    match v {
        Value::Null => Bson::Null,
        Value::Bool(b) => Bson::Boolean(*b),
        Value::Number(n) => n.as_i64().map(Bson::Int64).unwrap_or_else(|| Bson::Double(n.as_f64().unwrap())),
        Value::String(t) if t == "__nan__" => Bson::Double(f64::NAN),
        Value::String(t) => Bson::String(t.clone()),
        other => panic!("row value {other}"),
    }
}

#[test]
fn design_feedback_summary() {
    let sum = &golden()["summary"];
    let now = crate::dt::parse_utc(s(&sum["now"])).unwrap();
    for case in arr(sum, "since") {
        assert_eq!(since_day(now, case["days"].as_i64().unwrap()), s(&case["since"]));
    }
    for case in arr(sum, "tallies") {
        let rows: Vec<Document> = arr(case, "rows")
            .iter()
            .map(|r| r.as_object().unwrap().iter().map(|(k, v)| (k.clone(), bson_of(v))).collect())
            .collect();
        let got = tally_docs(&rows).map(|t| pyjson::pydantic(&summary_body(30, &t)));
        assert_eq!(got.as_deref(), case["out"].as_str(), "{rows:?}");
    }
    assert_eq!(count_of(Some(&Bson::Double(-0.0))), Some(0));
    assert_eq!(count_of(Some(&Bson::String(" 1_2 ".into()))), Some(12));
    assert_eq!(count_of(Some(&Bson::Array(vec![Bson::Int32(1)]))), None);
}

#[test]
fn vote_upsert_shape() {
    let v = DesignVote { design: "voxel".into(), theme: "auto".into(), vote: "up".into() };
    let (filter, update) = super::design_feedback::vote_upsert(&v, utc(2026, 3, 1, 23, 59, 59, 999_999));
    assert_eq!(filter.get_str("_id").unwrap(), "2026-03-01|voxel|auto|up");
    assert_eq!(update.get_document("$inc").unwrap().get("count"), Some(&Bson::Int32(1)));
    let set = update.get_document("$set").unwrap();
    let keys: Vec<&str> = set.keys().map(String::as_str).collect();
    assert_eq!(keys, ["day", "design", "theme", "vote", "updated_at"]);
    assert_eq!(set.get_datetime("updated_at").unwrap().timestamp_millis() % 1000, 999);
}

fn pairs(filter: &Document) -> Vec<(String, String)> {
    let flat = |d: &Document| d.iter().map(|(k, v)| (k.clone(), v.as_str().unwrap().to_owned())).collect::<Vec<_>>();
    match filter.get_array("$and") {
        Ok(items) => items.iter().flat_map(|d| flat(d.as_document().unwrap())).collect(),
        Err(_) => flat(filter),
    }
}

#[test]
fn moderation_targets() {
    for case in arr(golden(), "resolve") {
        let (kind, handle, slug) = (s(&case["kind"]), s(&case["handle"]), s(&case["slug"]));
        let (_, filter, url) = target_query(kind, handle, slug);
        let want: Vec<(String, String)> =
            arr(case, "filter").iter().map(|p| (s(&p[0]).into(), s(&p[1]).into())).collect();
        assert_eq!(pairs(&filter), want, "{kind} {handle:?} {slug:?}");
        let got = match case["doc"].as_object() {
            Some(fields) => {
                let mut d = Document::new();
                for (k, v) in fields {
                    match k.as_str() {
                        "id" => d.insert("_id", ObjectId::parse_str(s(v)).unwrap()),
                        _ => d.insert(k.clone(), s(v)),
                    };
                }
                reachable(kind, &d, &normalize_handle(handle)).unwrap()
            }
            None => None,
        };
        let got = got.map(|(id, label)| json!([id.to_hex(), label, url]));
        assert_eq!(got.unwrap_or(Value::Null), case["result"], "{kind} {handle:?} {slug:?}");
    }
    let mut bad = Document::new();
    bad.insert("_id", ObjectId::new());
    bad.insert("title", "t");
    bad.insert("visibility", "weird");
    assert!(reachable("mod", &bad, "h").is_err());
    bad.insert("visibility", "hidden");
    assert!(reachable("modpack", &bad, "h").is_err());
    assert_eq!(reachable("mod", &bad, "h").unwrap(), None);
}

#[tokio::test]
async fn moderation_report_rows() {
    let captured = &arr(golden(), "report_docs")[0];
    let target = ObjectId::parse_str(s(&captured["target_id"])).unwrap();
    let reason = py_strip("  spam\n ");
    let doc = report_doc(ObjectId::new(), "mod", target, "", "/mods/h/s", reason, utc(2026, 1, 1, 0, 0, 0, 0));
    let keys: Vec<&str> = doc.keys().skip(1).map(String::as_str).collect();
    let want: Vec<&str> = arr(&golden()["models"], "content_report").iter().map(s).collect();
    assert_eq!(keys, want);
    for (k, v) in captured.as_object().unwrap() {
        let ours = match doc.get(k).unwrap() {
            Bson::ObjectId(o) => o.to_hex(),
            Bson::String(t) => t.clone(),
            other => panic!("{k}: {other:?}"),
        };
        assert_eq!(ours, s(v), "{k}");
    }
    assert_eq!(doc.get("resolved"), Some(&Bson::Boolean(false)));

    let cases = arr(golden(), "reports");
    let ok = || (StatusCode::ACCEPTED, PydanticJson(json!({"status": "received"}))).into_response();
    let missing = || ApiError::not_found("That content couldn't be found.").into_response();
    let builders: [&dyn Fn() -> Response; 2] = [&ok, &missing];
    let mut bad = Vec::new();
    for (case, build) in cases.iter().zip(builders) {
        let resp = request_id::scope(rid_of(case), async { build() }).await;
        let (status, headers, body) = parts(resp).await;
        check(&mut bad, case, status, &headers, &body);
    }
    report("reports", cases.len(), bad);
}

#[test]
fn config_and_docs_follow_settings() {
    let defaults = Settings::default();
    let body = pyjson::pydantic(&meta::config_body(&defaults));
    assert!(body.starts_with(r#"{"app_name":"Kiwi API","api_url":"https://api.aallyn.net","captcha_provider":"turnstile","captcha_sitekey":null,"require_verified_for_tokens":true,"#));
    assert!(body.ends_with(r#""github_oauth_enabled":false,"discord_oauth_enabled":false}"#));
    assert!(meta::swagger_html("Kiwi API").contains("<title>Kiwi API - Swagger UI</title>"));
    assert!(meta::redoc_html("Kiwi API").contains("<title>Kiwi API - ReDoc</title>"));
    assert!(meta::api_info_html(&defaults).contains(r#"<a href="https://docs.aallyn.net">Documentation</a>"#));
}

async fn call(app: &Router, method: &str, uri: &str, bearer: Option<&str>, body: Option<Value>) -> (u16, String) {
    let mut req = Request::builder().method(method).uri(uri).header("x-request-id", "leaf-store");
    if let Some(b) = bearer {
        req = req.header("authorization", format!("Bearer {b}"));
    }
    let req = match body {
        Some(v) => req.header("content-type", "application/json").body(Body::from(v.to_string())),
        None => req.body(Body::empty()),
    };
    let (status, _, body) = parts(app.clone().oneshot(req.unwrap()).await.unwrap()).await;
    (status, body)
}

/// Every route end to end. Writes: point MONGO_URI / MONGO_DB / REDIS_URL at
/// throwaway stores and run `cargo test -p kiwi-server leaf_store -- --ignored`.
#[tokio::test]
#[ignore = "writes to MONGO_URI / REDIS_URL"]
async fn leaf_store_round_trip() {
    use crate::auth::sessions::{Audience, create_access_token};
    use mongodb::bson::doc;

    let state = AppState::new(Settings::from_env().unwrap()).await.unwrap();
    let db = state.mongo().clone();
    let user_id = ObjectId::new();
    let now = mongodb::bson::DateTime::now();
    db.collection::<Document>("users")
        .insert_one(doc! {"_id": user_id, "email": format!("{user_id}@example.test"), "hashed_password": "x",
        "is_active": true, "is_superuser": true, "token_version": 0, "created_at": now, "updated_at": now})
        .await
        .unwrap();
    let jwt = create_access_token(Audience::Portal, &user_id.to_hex(), 0, None);
    let mw = Mw::new(MwConfig::from_lookup(&|_| None), None, EmbedSource::Fixed(vec![]), TrustedHosts::parse("*"));
    let app = middleware::outer(
        middleware::wrap(super::router(), Arc::new(mw), None)
            .fallback(|| async { StatusCode::IM_A_TEAPOT })
            .with_state(state),
    );
    let tok = Some(jwt.as_str());

    let (st, body) = call(&app, "POST", "/admin/supporters", tok, Some(json!({"name": " Zed "}))).await;
    assert_eq!(st, 201, "{body}");
    assert!(body.starts_with(&format!(r#"{{"name":"Zed","added_by":"{}","created_at":""#, user_id.to_hex())));
    let (st, body) = call(&app, "POST", "/admin/supporters", tok, Some(json!({"name": "Zed"}))).await;
    assert_eq!((st, body.contains("Supporter 'Zed' already exists")), (409, true));
    let (st, body) = call(&app, "GET", "/v1/misc/supporters", None, None).await;
    assert_eq!(st, 200);
    assert!(body.contains("\"Zed\""), "{body}");
    let (st, body) = call(&app, "PUT", "/admin/supporters", tok, Some(json!({"names": ["a", " A", "b"]}))).await;
    assert_eq!(st, 200);
    assert!(body.ends_with(r#","added":2}"#), "{body}");
    assert_eq!(call(&app, "DELETE", "/admin/supporters/%20a", tok, None).await.0, 204);
    let (st, body) = call(&app, "DELETE", "/admin/supporters/a", tok, None).await;
    assert_eq!((st, body.contains("No supporter named 'a'")), (404, true));

    let vote = json!({"design": "classic", "theme": "light", "vote": "down"});
    assert_eq!(call(&app, "POST", "/site/design-feedback", None, Some(vote)).await.0, 202);
    let (st, body) = call(&app, "GET", "/admin/design-feedback?days=1", tok, None).await;
    assert_eq!(st, 200);
    assert!(body.contains(r#""light":{"up":"#), "{body}");

    let mod_id = ObjectId::new();
    let handle = format!("leaf{}", user_id.to_hex());
    db.collection::<Document>("mod_projects")
        .insert_one(doc! {"_id": mod_id, "slug": "m", "title": "M", "owner_handle": &handle, "visibility": "public"})
        .await
        .unwrap();
    let report = json!({"target_type": "mod", "handle": handle.to_uppercase(), "slug": " M ", "reason": " bad "});
    assert_eq!(call(&app, "POST", "/v1/moderation/report", None, Some(report)).await.0, 202);
    let row = db.collection::<Document>("content_reports").find_one(doc! {"target_id": mod_id}).await.unwrap().unwrap();
    assert_eq!(row.get_str("target_url").unwrap(), format!("/mods/{handle}/m"));
    assert_eq!(row.get_str("reason").unwrap(), "bad");
    let missing = json!({"target_type": "modpack", "handle": handle, "slug": "m", "reason": "bad"});
    assert_eq!(call(&app, "POST", "/v1/moderation/report", None, Some(missing)).await.0, 404);

    db.collection::<Document>("users").delete_one(doc! {"_id": user_id}).await.unwrap();
    db.collection::<Document>("mod_projects").delete_one(doc! {"_id": mod_id}).await.unwrap();
    db.collection::<Document>("content_reports").delete_many(doc! {"target_id": mod_id}).await.unwrap();
}

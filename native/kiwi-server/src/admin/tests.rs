//! Golden checks against `native/port/parity/gen_admin_fixtures.py`: every
//! response body rebuilt from the same BSON Python read, the pipelines and
//! filters it sent, the parameter and body errors, and the unauthenticated
//! routing (401s, 405s, the body read before auth) through the middleware.

use std::{
    collections::HashMap,
    sync::{Arc, OnceLock},
};

use axum::{
    Router,
    body::Body,
    extract::Request,
    http::{HeaderMap, Method, StatusCode, Uri},
};
use chrono::{DateTime, Duration, Utc};
use hyper_util::{client::legacy::Client, rt::TokioExecutor};
use mongodb::bson::{self, Bson, Document, doc, oid::ObjectId};
use serde_json::{Value, json};
use tower::ServiceExt;

use super::{
    config, discord, docs, pageviews, pending, site_users,
    users::{self, EventsQuery, UsageRow},
};
use crate::{
    auth::models::{ApiToken, SiteUser, User},
    config::Settings,
    db::Db,
    dt,
    error::ApiError,
    middleware::{EmbedSource, Mw, MwConfig, forwarded::TrustedHosts, outer, pyurl::scope_path, wrap},
    pyjson,
    state::{AppState, Inner},
    validation::{BodyInput, FromParams, Params, validate},
};

fn golden() -> &'static Value {
    static G: OnceLock<Value> = OnceLock::new();
    G.get_or_init(|| serde_json::from_str(include_str!("../../testdata/admin/golden.json")).expect("fixture parses"))
}

fn s(v: &Value) -> &str {
    v.as_str().expect("string")
}

fn ext_doc(v: &Value) -> Document {
    match Bson::try_from(v.clone()).expect("extended JSON") {
        Bson::Document(d) => d,
        other => panic!("not a document: {other}"),
    }
}

fn ext_docs(v: &Value) -> Vec<Document> {
    v.as_array().expect("array").iter().map(ext_doc).collect()
}

fn load<T: for<'de> serde::Deserialize<'de>>(v: &Value) -> Vec<T> {
    ext_docs(v).into_iter().map(|d| bson::from_document(d).expect("model loads")).collect()
}

/// Canonical extended JSON text, key order kept.
fn ext_text(b: Bson) -> String {
    serde_json::to_string(&b.into_canonical_extjson()).unwrap()
}

fn pipeline_text(p: &[Document]) -> String {
    ext_text(Bson::Array(p.iter().cloned().map(Bson::Document).collect()))
}

fn now() -> DateTime<Utc> {
    dt::parse_utc(s(&golden()["now"])).expect("now")
}

fn body_of(case: &Value) -> &str {
    s(&case["body"])
}

fn assert_body(v: &Value, case: &Value, what: &str) {
    assert_eq!(pyjson::pydantic(v), body_of(case), "{what}");
}

/// The status, code, message and details of an error envelope.
fn assert_error(e: &ApiError, case: &Value, what: &str) {
    let want: Value = serde_json::from_str(body_of(case)).unwrap();
    assert_eq!(u64::from(e.status.as_u16()), case["status"].as_u64().unwrap(), "{what}: status");
    assert_eq!(e.code, s(&want["error"]["code"]), "{what}: code");
    assert_eq!(e.message, s(&want["error"]["message"]), "{what}: message");
    assert_eq!(e.details.clone().unwrap_or(Value::Null), want["error"]["details"], "{what}: details");
}

fn oid(s: &str) -> ObjectId {
    ObjectId::parse_str(s).unwrap()
}

// --- users -----------------------------------------------------------------------------

#[test]
fn user_views() {
    let u = &golden()["users"];
    let users: Vec<User> = load(&u["users"]);
    let tokens: Vec<ApiToken> = load(&u["tokens"]);
    let list = users::user_summaries(&users, &ext_docs(&u["token_rows"]), &ext_docs(&u["rl_rows"])).unwrap();
    assert_body(&list, &u["list"], "list");
    assert_body(&users::single_summary(&users[0], &tokens), &u["get"], "get");
    assert_body(&users::single_summary(&users[1], &[]), &u["get_no_tokens"], "get without tokens");
    assert_body(&Value::Array(tokens.iter().map(users::token_view).collect()), &u["tokens_list"], "tokens");
    assert_error(&ApiError::not_found("User not found"), &u["get_missing"], "missing user");
}

#[test]
fn user_activity() {
    let u = &golden()["users"];
    let rows: Vec<Vec<Document>> = u["activity_rows"].as_array().unwrap().iter().map(ext_docs).collect();
    let rows = [rows[0].clone(), rows[1].clone(), rows[2].clone()];
    assert_body(&users::activity_summary(3, &rows).unwrap(), &u["activity"], "activity");
    assert_body(&users::activity_summary(7, &[vec![], vec![], vec![]]).unwrap(), &u["activity_empty"], "empty");
    let since = dt::to_bson(now() - Duration::days(3));
    let built = users::activity_pipelines(doc! {"user_id": oid("65a000000000000000000a01")}, since);
    for (i, p) in built.iter().enumerate() {
        assert_eq!(pipeline_text(p), serde_json::to_string(&u["activity_pipelines"][i]).unwrap(), "pipeline {i}");
    }
}

#[test]
fn usage_feed() {
    let u = &golden()["users"];
    let rows: Vec<UsageRow> = load(&u["events"]);
    assert_body(&users::events_page(rows.clone(), 2), &u["events_more"], "page with more");
    assert_body(&users::events_page(rows, 3), &u["events_all"], "last page");
    let q = EventsQuery {
        user_id: Some(oid("65a000000000000000000a01")),
        status_code: Some(429),
        cursor: Some("65a000000000000000000e09".into()),
        limit: 2,
    };
    let f = users::events_filter(&q).unwrap();
    assert_eq!(ext_text(Bson::Document(f)), serde_json::to_string(&u["events_filters"][0]).unwrap());
    let all = EventsQuery { user_id: None, status_code: None, cursor: None, limit: 3 };
    assert_eq!(ext_text(Bson::Document(users::events_filter(&all).unwrap())), "{}");
    let bad = EventsQuery { cursor: Some("nope".into()), ..all };
    assert_error(&users::events_filter(&bad).unwrap_err(), &u["events_bad_cursor"], "bad cursor");
}

#[test]
fn activity_overview() {
    let u = &golden()["users"];
    let rows: Vec<Vec<Document>> = u["overview_rows"].as_array().unwrap().iter().map(ext_docs).collect();
    let users: Vec<User> = load(&u["users"]);
    let emails: HashMap<ObjectId, String> = users.into_iter().take(1).map(|u| (u.id, u.email)).collect();
    assert_body(&users::overview(2, &rows[0], &rows[1], &emails).unwrap(), &u["overview"], "overview");
    assert_body(&users::overview(7, &[], &[], &HashMap::new()).unwrap(), &u["overview_empty"], "empty");
    let built = users::overview_pipelines(dt::to_bson(now() - Duration::days(2)), 5);
    for (i, p) in built.iter().enumerate() {
        assert_eq!(pipeline_text(p), serde_json::to_string(&u["overview_pipelines"][i]).unwrap(), "pipeline {i}");
    }
}

// --- pending ---------------------------------------------------------------------------

#[test]
fn pending_badges() {
    let p = &golden()["pending"];
    let tabs: Vec<Value> = pending::SOURCES.iter().map(|s| json!([s.tab, s.one, s.many])).collect();
    assert_eq!(Value::Array(tabs), p["tabs"]);
    for c in p["cases"].as_array().unwrap() {
        let counts: Vec<Option<i64>> = c["counts"].as_array().unwrap().iter().map(Value::as_i64).collect();
        assert_body(&pending::fold(&counts), c, &c["counts"].to_string());
    }
}

#[test]
fn failing_decoder_rule() {
    let t = Utc::now();
    let run = |ok: Option<bool>, finished: bool, age: i64| -> pending::DecoderRun {
        let started = dt::to_bson(t - Duration::seconds(age));
        let mut d = doc! {"name": "fish", "trigger": "patch", "ordinal": 3, "code_version": "x", "started_at": started};
        d.insert("ok", ok.map_or(Bson::Null, Bson::Boolean));
        d.insert("finished_at", if finished { Bson::DateTime(started) } else { Bson::Null });
        bson::from_document(d).unwrap()
    };
    assert!(pending::is_failing(&run(Some(false), true, 10), t));
    assert!(!pending::is_failing(&run(Some(true), true, 99_999), t));
    assert!(!pending::is_failing(&run(None, false, 1799), t));
    assert!(pending::is_failing(&run(None, false, 1801), t));
}

// --- runtime config ----------------------------------------------------------------------

fn state() -> AppState {
    let settings = Settings::from_vars([("PY_UPSTREAM", "http://py:8000")]).unwrap();
    let upstream = Client::builder(TokioExecutor::new()).build_http();
    AppState { inner: Arc::new(Inner { settings, db: Db { mongo: None, redis: None, pg: None }, upstream }) }
}

#[test]
fn config_list() {
    let c = &golden()["config"];
    let st = state();
    let mut overrides = HashMap::new();
    for d in ext_docs(&c["overrides"]) {
        let (k, o) = config::Override::from_doc(&d).unwrap();
        overrides.insert(k, o);
    }
    let items = config::items(&st, &overrides);
    let n = items.len();
    assert_body(&json!({"items": items, "count": n}), &c["list"], "with overrides");
    let plain = config::items(&st, &HashMap::new());
    let n = plain.len();
    assert_body(&json!({"items": plain, "count": n}), &c["list_plain"], "defaults");
}

fn body_input(text: &str) -> Result<BodyInput, ApiError> {
    if text.is_empty() {
        return Ok(BodyInput::Missing);
    }
    match pyjson::loads(text.as_bytes()) {
        Ok(Value::Null) => Ok(BodyInput::Missing),
        Ok(v) => Ok(BodyInput::Json(v)),
        Err(pyjson::LoadsError::Syntax { msg, pos }) => {
            Err(crate::validation::ValidationError(vec![crate::validation::ErrorItem::json_invalid(pos, msg)]).into())
        }
        Err(e) => panic!("{e:?}"),
    }
}

#[test]
fn config_write_errors() {
    let c = &golden()["config"];
    for e in c["errors"].as_array().unwrap() {
        let name = s(&e["name"]);
        let uri: Uri = s(&e["url"]).parse().unwrap();
        let key = scope_path(&uri).trim_start_matches("/admin/config/").to_owned();
        let err = if s(&e["method"]) == "DELETE" {
            crate::runtime_config::spec(&key)
                .map(|_| ())
                .ok_or_else(|| ApiError::not_found(format!("Unknown setting key '{key}'.")))
                .unwrap_err()
        } else {
            match body_input(s(&e["sent"])).and_then(|b| validate::<config::UpdateBody>(&b)) {
                Err(err) => err,
                Ok(body) => config::validate(&key, &body.value).map(|_| ()).unwrap_err(),
            }
        };
        assert_error(&err, e, name);
    }
    for v in c["valid"].as_array().unwrap() {
        let value = match &v["value"] {
            Value::Object(m) if m.contains_key("$numberLong") => json!(s(&m["$numberLong"]).parse::<i64>().unwrap()),
            other => other.clone(),
        };
        let (_, stored) = config::validate(s(&v["key"]), &value).unwrap();
        assert_eq!(crate::runtime_config::encode_value(&stored), s(&v["stored"]), "{v}");
    }
}

// --- site users ----------------------------------------------------------------------------

#[test]
fn site_user_views() {
    let st = &golden()["site"];
    let users: Vec<SiteUser> = load(&st["site_users"]);
    for (u, case) in users.iter().zip(st["get"].as_array().unwrap()) {
        assert_body(&site_users::site_user_dto(u, 0, 0), case, &u.username);
    }
    let counts = [(3, 1), (0, 0), (1, 0)];
    let items: Vec<Value> = users.iter().zip(counts).map(|(u, (m, p))| site_users::site_user_dto(u, m, p)).collect();
    assert_body(&json!({"items": items, "count": 3, "total": 41}), &st["list"], "list");
    assert_body(&json!({"items": [], "count": 0, "total": 0}), &st["list_empty"], "empty list");
    let filters = st["list_filters"].as_array().unwrap();
    let f = site_users::site_users_filter(Some(" Kiwi.Fan+(1) "));
    assert_eq!(ext_text(Bson::Document(f)), serde_json::to_string(&filters[0]).unwrap());
    assert_eq!(ext_text(Bson::Document(site_users::site_users_filter(Some("  ")))), "{}");
    assert_eq!(ext_text(Bson::Document(site_users::claims_filter(true))), serde_json::to_string(&filters[4]).unwrap());
    assert_eq!(ext_text(Bson::Document(site_users::claims_filter(false))), serde_json::to_string(&filters[5]).unwrap());

    let claims: Vec<Value> = users.iter().take(2).map(site_users::claim_view).collect();
    assert_body(&json!({"items": claims, "count": 2}), &st["claims"], "claims");
    let reqs: Vec<docs::UsernameRequest> =
        ext_docs(&st["requests"]).into_iter().map(|d| docs::UsernameRequest::from_doc(d).unwrap()).collect();
    let dtos: Vec<Value> = reqs.iter().map(site_users::request_dto).collect();
    assert_body(&json!({"items": dtos}), &st["requests_list"], "requests");
}

#[test]
fn beanie_save_shapes() {
    let saves = &golden()["saves"];
    let check = |name: &str, f: &dyn Fn(Document) -> Document| {
        for c in saves[name].as_array().unwrap() {
            let got = ext_text(Bson::Document(f(ext_doc(&c["raw"]))));
            assert_eq!(got, serde_json::to_string(&c["saved"]).unwrap(), "{name}");
        }
    };
    check("users", &|d| docs::user_doc(&bson::from_document(d).unwrap()));
    check("tokens", &|d| docs::token_doc(&bson::from_document(d).unwrap()));
    check("site_users", &|d| docs::site_user_doc(&bson::from_document(d).unwrap()).unwrap());
    check("requests", &|d| docs::UsernameRequest::from_doc(d).unwrap().to_doc());
}

#[test]
fn username_rules() {
    for c in golden()["site"]["normalize"].as_array().unwrap() {
        match site_users::normalize_username(s(&c["raw"])) {
            Ok(name) => assert_eq!(Some(name.as_str()), c["name"].as_str(), "{c}"),
            Err(e) => {
                assert_eq!(u64::from(e.status.as_u16()), c["status"].as_u64().unwrap(), "{c}");
                assert_eq!((e.code, e.message.as_str()), (s(&c["code"]), s(&c["message"])), "{c}");
            }
        }
    }
}

// --- discord, bot ----------------------------------------------------------------------------

#[test]
fn discord_and_bot() {
    let m = &golden()["misc"];
    assert_body(&discord::commands_body(), &m["commands"], "commands");
    for name in ["bot_stats", "bot_stats_empty"] {
        let c = &m[name];
        let presence = ext_docs(&c["presence"]);
        let body = discord::bot_stats_body(presence.first(), &ext_docs(&c["commands"])).unwrap();
        assert_body(&body, c, name);
    }
    assert_body(
        &super::embed_stats_body(2, 401),
        &json!({"body": "{\"live\":2,\"bytes\":300,\"redis\":true}"}),
        "embed",
    );
}

// --- pageviews -------------------------------------------------------------------------------

fn params_from(url: &str) -> (Vec<(String, String)>, Option<String>) {
    let uri: Uri = url.parse().unwrap();
    (Vec::new(), uri.query().map(str::to_owned))
}

#[test]
fn pageviews_report() {
    let pv = &golden()["pageviews"];
    let now = dt::parse_utc(s(&pv["now"])).unwrap();
    for c in pv["cases"].as_array().unwrap() {
        let name = s(&c["name"]);
        let (path, query) = params_from(s(&c["url"]));
        let headers = HeaderMap::new();
        let mut p = Params::new(path, query.as_deref(), &headers);
        let q = pageviews::ReportQuery::from_params(&mut p);
        assert!(p.v.is_ok(), "{name}: params");
        let chosen = q.chosen();
        let started = match &c["started"] {
            Value::Null => None,
            v => match Bson::try_from(v.clone()).unwrap() {
                Bson::DateTime(d) => Some(dt::from_bson(d)),
                other => panic!("{other}"),
            },
        };
        let (w, prev_labels, current, previous) = pageviews::report_pipelines(q.days, now, started, &chosen);
        let logged = c["pipelines"].as_array().unwrap();
        assert_eq!(pipeline_text(&current), serde_json::to_string(&logged[0]).unwrap(), "{name}: current pipeline");
        assert_eq!(previous.is_some(), logged.len() == 2, "{name}: previous window");
        if let Some(p) = &previous {
            assert_eq!(pipeline_text(p), serde_json::to_string(&logged[1]).unwrap(), "{name}: previous pipeline");
        }
        let facets: Vec<Document> = c["facets"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| if f.is_null() { Document::new() } else { ext_doc(f) })
            .collect();
        let prev = previous.as_ref().map(|_| &facets[1]);
        let body = pageviews::build_report(q.days, &w, &prev_labels, started, &chosen, &facets[0], prev).unwrap();
        assert_body(&body, c, name);
    }
    let live = &pv["live"];
    assert_eq!(pipeline_text(&pageviews::live_pipeline(now)), serde_json::to_string(&live["pipeline"]).unwrap());
    assert_body(&pageviews::build_live(&ext_doc(&live["facet"])).unwrap(), live, "live");
}

// --- parameters and bodies ---------------------------------------------------------------------

/// Path parameters of `url` against the route templates the admin router uses.
fn path_values(url: &str) -> (String, Vec<(String, String)>, Option<String>) {
    let uri: Uri = url.parse().unwrap();
    let path = scope_path(&uri);
    let templates = [
        "/admin/users/{user_id}/activity",
        "/admin/users/{user_id}/tokens",
        "/admin/users/{user_id}/terminate",
        "/admin/users/{user_id}",
        "/admin/tokens/{token_id}/revoke",
    ];
    for t in templates {
        let (tp, pp): (Vec<&str>, Vec<&str>) = (t.split('/').collect(), path.split('/').collect());
        if tp.len() == pp.len() && tp.iter().zip(&pp).all(|(a, b)| a.starts_with('{') || a == b) {
            let vals = tp
                .iter()
                .zip(&pp)
                .filter(|(a, _)| a.starts_with('{'))
                .map(|(a, b)| (a.trim_matches(['{', '}']).to_owned(), (*b).to_owned()))
                .collect();
            return (t.to_owned(), vals, uri.query().map(str::to_owned));
        }
    }
    (path, Vec::new(), uri.query().map(str::to_owned))
}

fn check<T: FromParams>(path: Vec<(String, String)>, query: Option<&str>) -> Option<ApiError> {
    let headers = HeaderMap::new();
    let mut p = Params::new(path, query, &headers);
    let v = T::from_params(&mut p);
    p.v.finish(v).err().map(Into::into)
}

#[test]
fn parameter_errors() {
    use super::{discord::Guild, pageviews::ReportQuery, site_users::*, users::*};
    for c in golden()["params"].as_array().unwrap() {
        if c["status"] != 422 {
            continue;
        }
        let url = s(&c["url"]);
        let what = format!("{} {url}", s(&c["method"]));
        if let Some(text) = c["sent"].as_str() {
            let b = body_input(text).unwrap();
            let err = if url.ends_with("/username") {
                validate::<UsernameBody>(&b).err()
            } else if url.ends_with("/claimed-name") {
                validate::<ClaimedNameBody>(&b).err()
            } else {
                validate::<ReasonBody>(&b).err()
            };
            assert_error(&err.expect("body error"), c, &what);
            continue;
        }
        let (route, path, query) = path_values(url);
        let q = query.as_deref();
        let err = match route.as_str() {
            "/admin/users" => check::<Days<30, 365>>(path, q),
            "/admin/users/{user_id}" | "/admin/users/{user_id}/tokens" | "/admin/users/{user_id}/terminate" => {
                check::<UserPath>(path, q)
            }
            "/admin/users/{user_id}/activity" => check::<UserDays>(path, q),
            "/admin/tokens/{token_id}/revoke" => check::<TokenPath>(path, q),
            "/admin/events" => check::<EventsQuery>(path, q),
            "/admin/activity" => check::<OverviewQuery>(path, q),
            "/admin/pageviews" => check::<ReportQuery>(path, q),
            "/admin/site-users" => check::<SiteUsersQuery>(path, q),
            "/admin/site-claims" => check::<PendingOnly>(path, q),
            "/admin/discord/clear-guild-commands" => check::<Guild>(path, q),
            other => panic!("no extractor for {other}"),
        };
        assert_error(&err.expect("parameter error"), c, &what);
    }
}

// --- routing through the middleware ----------------------------------------------------------------

fn stack() -> Router {
    let mw = Arc::new(Mw::new(
        MwConfig::from_lookup(&|_| None),
        None,
        EmbedSource::Fixed(Vec::new()),
        TrustedHosts::parse("127.0.0.1"),
    ));
    outer(
        wrap(super::router(), mw, None).fallback(|| async { (StatusCode::IM_A_TEAPOT, "proxied") }).with_state(state()),
    )
}

async fn send(method: &str, url: &str, rid: &str, body: Option<&str>) -> (u16, HeaderMap, String) {
    let mut req = Request::builder()
        .method(Method::from_bytes(method.as_bytes()).unwrap())
        .uri(url)
        .header("host", "testserver")
        .header("x-request-id", rid);
    if body.is_some() {
        req = req.header("content-type", "application/json");
    }
    let req = req.body(body.map_or_else(Body::empty, |b| Body::from(b.to_owned()))).unwrap();
    let resp = stack().oneshot(req).await.unwrap();
    let status = resp.status().as_u16();
    let headers = resp.headers().clone();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    (status, headers, String::from_utf8(bytes.to_vec()).unwrap())
}

#[tokio::test]
async fn unauthenticated_routing() {
    let cases = golden()["unauth"]
        .as_array()
        .unwrap()
        .iter()
        .chain(golden()["params"].as_array().unwrap().iter().filter(|c| c["status"] == 405));
    for (i, c) in cases.enumerate() {
        let (method, url) = (s(&c["method"]), s(&c["url"]));
        let want: Value = serde_json::from_str(body_of(c)).unwrap();
        let rid = s(&want["error"]["request_id"]);
        let (status, headers, body) = send(method, url, rid, c["body_sent"].as_str()).await;
        let what = format!("#{i} {method} {url}");
        assert_eq!(u64::from(status), c["status"].as_u64().unwrap(), "{what}: status");
        for h in ["allow", "www-authenticate"] {
            assert_eq!(headers.get(h).map(|v| v.to_str().unwrap()), c[h].as_str(), "{what}: {h}");
        }
        if method != "HEAD" {
            assert_eq!(body, body_of(c), "{what}: body");
        }
    }
}

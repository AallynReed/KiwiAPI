//! Golden checks against fixtures generated from the Python code
//! (`native/port/parity/gen_lb_ingest_fixtures.py`): the parser, anchors,
//! guard and `insert_dump` composition, the follow-up effects of an ingest,
//! the ingest-log documents, and every route case.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock},
};

use axum::{
    Router,
    body::Body,
    extract::Request,
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
};
use hyper_util::{client::legacy::Client, rt::TokioExecutor};
use kiwi_core::lbparse::{self, Board, Verdict};
use mongodb::bson::{self, Document, doc, oid::ObjectId};
use serde_json::{Map, Value, json};
use tower::ServiceExt;

use super::{
    admin, backlog,
    ingest::{self, Effect, IngestStore, Outcome},
    routes, signals,
    store::{self, BoardRow, Partition, ResetTally, Written},
};
use crate::{
    auth::models::{ApiToken, User},
    config::Settings,
    db::Db,
    error::ApiError,
    middleware::{EmbedSource, Mw, MwConfig, forwarded::TrustedHosts, outer, wrap},
    pyfloat, pyjson, request_id,
    state::{AppState, Inner},
    validation::{BodyInput, FromParams, Params, read_body},
};

fn golden() -> &'static Value {
    static G: OnceLock<Value> = OnceLock::new();
    G.get_or_init(|| {
        serde_json::from_str(include_str!("../../testdata/lb_ingest/golden.json")).expect("fixture parses")
    })
}

fn cases(key: &str) -> &'static Vec<Value> {
    golden()[key].as_array().unwrap_or_else(|| panic!("fixture section {key}"))
}

fn s(v: &Value) -> &str {
    v.as_str().expect("string")
}

fn i(v: &Value) -> i64 {
    v.as_i64().expect("int")
}

fn hex_of(d: &Document) -> String {
    let mut out = Vec::new();
    d.to_writer(&mut out).expect("bson encodes");
    hex::encode(out)
}

fn doc_of(hex_: &str) -> Document {
    Document::from_reader(&hex::decode(hex_).unwrap()[..]).unwrap()
}

/// An `ApiError` body as the request `rid` renders it.
async fn err_body(rid: &str, e: ApiError) -> (u16, String) {
    let status = e.status.as_u16();
    (status, request_id::scope(rid.to_owned(), async move { e.body() }).await)
}

fn board_json(b: &Board<'_>) -> Value {
    json!({
        "name_id": b.name_id, "category_id": b.category_id, "uuid": b.uuid, "name": b.name,
        "category": b.category, "contest": b.contest,
        "entries": b.entries.iter().map(|(r, p, sc)| json!([r, p, pyfloat::repr(*sc)])).collect::<Vec<_>>(),
    })
}

/// Python ints past `i64` (any of which fails the `int4` write) saturate here.
fn saturate(v: &Value) -> Value {
    match v {
        Value::Number(n) if n.as_i64().is_none() => json!(i64::MAX),
        Value::Array(a) => Value::Array(a.iter().map(saturate).collect()),
        Value::Object(m) => Value::Object(m.iter().map(|(k, v)| (k.clone(), saturate(v))).collect()),
        other => other.clone(),
    }
}

#[test]
fn parser_matches_python() {
    for c in cases("parse") {
        let text = s(&c["text"]);
        match lbparse::parse_dump(text) {
            Ok(boards) => {
                let got: Vec<Value> = boards.iter().map(board_json).collect();
                assert_eq!(Value::Array(got), saturate(&c["boards"]), "{text:?}");
            }
            Err(e) => assert_eq!(e, s(&c["error"]), "{text:?}"),
        }
    }
}

#[test]
fn timestamps_and_boundaries() {
    for c in cases("normalize") {
        let now = i(&c["now"]);
        if let Some(a) = c.get("anchor") {
            assert_eq!(lbparse::trove_day_start(now), i(a));
            continue;
        }
        let Some(ts) = c["ts"].as_i64() else { continue };
        let got = lbparse::normalize_timestamp(ts, now, c["backfill"].as_bool().unwrap());
        assert_eq!(got.map(Value::from).unwrap_or(Value::Null), c["result"], "{c}");
    }
    for c in cases("boundaries") {
        let got = lbparse::reset_boundaries(s(&c["kind"]), i(&c["start"]), i(&c["end"]));
        assert_eq!(json!(got), c["result"], "{c}");
    }
}

fn counts(v: &Value) -> HashMap<i64, i64> {
    v.as_array().unwrap().iter().map(|p| (i(&p[0]), i(&p[1]))).collect()
}

fn verdict_json(v: &Verdict) -> Value {
    json!({"reason": v.reason, "missing_boards": v.missing_boards, "collapsed_boards": v.collapsed_boards})
}

#[test]
fn completeness_verdicts() {
    for c in cases("verdicts") {
        let got = lbparse::completeness_verdict(
            &counts(&c["cur"]),
            &counts(&c["prev"]),
            i(&c["max_missing"]),
            c["collapse_frac"].as_f64().unwrap(),
        );
        assert_eq!(got.as_ref().map(verdict_json).unwrap_or(Value::Null), c["result"], "{c}");
    }
}

/// The fake stores `insert_section` gives `insert_dump`, recording each call.
struct FakeStore {
    case: &'static Value,
    calls: Mutex<Vec<Value>>,
}

impl IngestStore for FakeStore {
    async fn setting(&self, key: &str) -> Result<Value, String> {
        Ok(self.case["settings"][key].clone())
    }
    async fn previous_anchor_before(&self, anchor: i64) -> Result<Option<i64>, String> {
        self.calls.lock().unwrap().push(json!(["previous_anchor_before", anchor]));
        Ok(self.case["prev_anchor"].as_i64())
    }
    async fn board_counts_at(&self, anchor: i64) -> Result<HashMap<i64, i64>, String> {
        self.calls.lock().unwrap().push(json!(["board_counts_at", anchor]));
        Ok(counts(&self.case["prev_counts"]))
    }
    async fn board_kinds(&self, uuids: &[i64]) -> Result<HashMap<i64, String>, String> {
        let mut sorted = uuids.to_vec();
        sorted.sort_unstable();
        self.calls.lock().unwrap().push(json!(["board_kinds", sorted]));
        let over: HashMap<i64, String> =
            self.case["overrides"].as_array().unwrap().iter().map(|p| (i(&p[0]), s(&p[1]).to_owned())).collect();
        Ok(uuids
            .iter()
            .map(|&u| (u, kiwi_core::lbmodels::effective_reset_kind(over.get(&u).map(String::as_str), u).to_owned()))
            .collect())
    }
    async fn write_snapshot(&self, boards: &[Board<'_>], anchor: i64) -> Result<Written, String> {
        let uuids: Vec<i64> = boards.iter().map(|b| b.uuid).collect();
        self.calls.lock().unwrap().push(json!(["write_snapshot", anchor, uuids]));
        Ok(Written { boards: boards.len(), entries: boards.iter().map(|b| b.entries.len()).sum(), cleared: 5 })
    }
}

/// `insert_dump`'s return value in Python's dict shape.
fn outcome_json(o: &Outcome) -> Value {
    match o {
        Outcome::Empty => json!({"boards": 0, "entries": 0, "created_at": null}),
        Outcome::Stored { created_at, written, rejected_timestamp } => {
            let mut m = json!({
                "boards": written.boards, "entries": written.entries, "created_at": created_at,
                "cleared_before_insert": written.cleared, "archived_old": 0,
            });
            if let Some(r) = rejected_timestamp {
                m["rejected_timestamp"] = json!(r);
            }
            m
        }
        Outcome::Skipped { created_at, boards, entries, verdict, prev_anchor, prev_boards, cur_boards } => json!({
            "skipped": true, "created_at": created_at, "boards": boards, "entries": entries,
            "reason": verdict.reason, "missing_boards": verdict.missing_boards,
            "collapsed_boards": verdict.collapsed_boards, "prev_anchor": prev_anchor,
            "prev_boards": prev_boards, "cur_boards": cur_boards,
        }),
    }
}

/// The Python dict back into an [`Outcome`].
fn outcome_of(d: &Value) -> Outcome {
    if d.get("skipped").is_some() {
        let ints = |k: &str| d[k].as_array().unwrap().iter().map(i).collect();
        return Outcome::Skipped {
            created_at: i(&d["created_at"]),
            boards: i(&d["boards"]) as usize,
            entries: i(&d["entries"]) as usize,
            verdict: Verdict {
                reason: s(&d["reason"]).to_owned(),
                missing_boards: ints("missing_boards"),
                collapsed_boards: ints("collapsed_boards"),
            },
            prev_anchor: i(&d["prev_anchor"]),
            prev_boards: i(&d["prev_boards"]) as usize,
            cur_boards: i(&d["cur_boards"]) as usize,
        };
    }
    match d["created_at"].as_i64() {
        None => Outcome::Empty,
        Some(created_at) => Outcome::Stored {
            created_at,
            written: Written {
                boards: i(&d["boards"]) as usize,
                entries: i(&d["entries"]) as usize,
                cleared: i(&d["cleared_before_insert"]) as u64,
            },
            rejected_timestamp: d.get("rejected_timestamp").and_then(Value::as_i64),
        },
    }
}

#[tokio::test]
async fn insert_dump_composition() {
    for c in cases("insert_dump") {
        let st = FakeStore { case: c, calls: Mutex::new(Vec::new()) };
        let got =
            ingest::insert_dump(&st, s(&c["text"]), c["timestamp"].as_i64(), c["backfill"].as_bool().unwrap(), &|| {
                i(&c["now"])
            })
            .await;
        match got {
            Ok(o) => assert_eq!(outcome_json(&o), c["summary"], "{}", c["name"]),
            Err(e) => assert_eq!(e, s(&c["error"]), "{}", c["name"]),
        }
        assert_eq!(Value::Array(st.calls.into_inner().unwrap()), c["calls"], "{}", c["name"]);
    }
}

fn effect_json(e: &Effect) -> Value {
    match e {
        Effect::Log { summary, error } => {
            json!(["log", ingest::ENDPOINT, error.is_none(), error, hex_of(&doc! {"summary": summary.clone()})])
        }
        Effect::Invalidate(a) => json!(["invalidate", a]),
        Effect::Save(a) => json!(["save", a]),
        Effect::Publish { signature, data } => json!(["publish", "leaderboard", signature, data]),
        Effect::Warm => json!(["warm"]),
    }
}

#[test]
fn ingest_follow_up() {
    for c in cases("process") {
        let result = match c["error"].as_str() {
            Some(e) => Err(e.to_owned()),
            None => Ok(outcome_of(&c["result"])),
        };
        let got: Vec<Value> =
            ingest::effects(&result, i(&c["nbytes"]) as usize, c["timestamp"].as_i64(), c["warm"].as_bool().unwrap())
                .iter()
                .map(effect_json)
                .collect();
        assert_eq!(Value::Array(got), c["calls"], "{}", c["name"]);
        let returned = result.ok().map(|o| outcome_json(&o)).unwrap_or(Value::Null);
        assert_eq!(returned, c["returned"], "{}", c["name"]);
    }
}

#[test]
fn ingest_log_documents() {
    let user: User = bson::from_document(doc! {"_id": ObjectId::parse_str("64b000000000000000000001").unwrap(),
    "email": "master@aallyn.net"})
    .unwrap();
    let id = ObjectId::parse_str("64b0000000000000000000aa").unwrap();
    let now = chrono::DateTime::from_timestamp(1_761_130_923, 456_789_000).unwrap();
    for c in cases("log_docs") {
        let token: Option<ApiToken> = c["token_name"]
            .as_str()
            .map(|n| bson::from_document(doc! {"_id": ObjectId::new(), "user_id": user.id, "name": n}).unwrap());
        let summary = doc_of(s(&c["summary"])).get_document("summary").unwrap().clone();
        let d = signals::log_doc(id, ingest::ENDPOINT, now, &user, token.as_ref(), summary, c["error"].as_str());
        assert_eq!(c["success"].as_bool().unwrap(), c["error"].is_null());
        assert_eq!(hex_of(&d), s(&c["bson"]), "{c}");
    }
}

#[test]
fn backlog_names_dirs_and_events() {
    let b = &golden()["backlog"];
    for p in b["anchor_of"].as_array().unwrap() {
        assert_eq!(backlog::anchor_of(s(&p[0])).map(Value::from).unwrap_or(Value::Null), p[1], "{p}");
    }
    for p in b["dir"].as_array().unwrap() {
        let settings = Settings::from_vars([("PY_UPSTREAM", "http://py:8000"), ("BACKLOG_DIR", s(&p[0]))]).unwrap();
        assert_eq!(backlog::dir_text(&settings), s(&p[1]), "{p}");
    }
    for c in cases("events") {
        assert_eq!(signals::event_payload("leaderboard", c["data"].clone(), i(&c["ts"])), s(&c["payload"]));
    }
}

#[tokio::test]
async fn backlog_save_and_list() {
    let root = std::env::temp_dir().join(format!("lb_backlog_{}", ObjectId::new().to_hex()));
    let root_s = root.to_string_lossy().replace('\\', "/");
    let settings = Settings::from_vars([("PY_UPSTREAM", "http://py:8000"), ("BACKLOG_DIR", root_s.as_str())]).unwrap();
    let upstream = Client::builder(TokioExecutor::new()).build_http();
    let state =
        AppState { inner: Arc::new(Inner { settings, db: Db { mongo: None, redis: None, pg: None }, upstream }) };
    let text = Arc::new("A$B$1 = N$C##1;Zoë;1\n".repeat(50));
    backlog::save(&state, 1_700_000_000, text.clone()).await;
    backlog::save(&state, 0, text.clone()).await;
    std::fs::write(root.join("leaderboards").join("1600000000.cfg"), "x").unwrap();
    std::fs::write(root.join("leaderboards").join(".1500000000.cfg"), "x").unwrap();
    let files: Vec<i64> = backlog::list_files(state.settings()).into_iter().map(|(a, _)| a).collect();
    assert_eq!(files, [1_600_000_000, 1_700_000_000]);
    let gz = std::fs::read(root.join("leaderboards").join("1700000000.cfg.gz")).unwrap();
    assert_eq!(&gz[..4], &[0x1f, 0x8b, 8, 8]);
    assert_eq!(&gz[10..33], b".1700000000.cfg.gz.tmp\0");
    let info = backlog::info(state.settings());
    assert_eq!(info["backlog_files"], 2);
    assert_eq!(info["backlog_dir_exists"], true);
    std::fs::remove_dir_all(root).unwrap();
}

// --- routes ------------------------------------------------------------------------

fn headers(ctype: Option<&str>) -> HeaderMap {
    let mut h = HeaderMap::new();
    if let Some(ct) = ctype {
        h.insert(header::CONTENT_TYPE, HeaderValue::from_str(ct).unwrap());
    }
    h
}

fn b64(v: &Value) -> Vec<u8> {
    base64::Engine::decode(&base64::engine::general_purpose::STANDARD, s(v)).unwrap()
}

/// `insert_leaderboards` up to the response, with the gate, auth and the
/// background processing as the fixture faked them.
async fn insert_case(c: &Value) -> (u16, String) {
    let rid = s(&c["request_id"]);
    let ctype = c["content_type"].as_str();
    let body = b64(&c["body_b64"]);
    let items = match kiwi_core::form::parse_form(ctype.map(str::as_bytes), &body) {
        Ok(items) => items,
        Err(kiwi_core::form::FormError::Http(m)) => {
            return err_body(rid, ApiError::http(StatusCode::BAD_REQUEST, Some(&m))).await;
        }
    };
    let query = s(&c["query"]).strip_prefix('?');
    let q = match routes::validate_insert(query, &headers(ctype), items) {
        Ok(q) => q,
        Err(e) => return err_body(rid, e.into()).await,
    };
    if q.file.is_empty() {
        return err_body(rid, ApiError::bad_request(routes::EMPTY_UPLOAD)).await;
    }
    let text = String::from_utf8_lossy(&q.file).into_owned();
    let warm = q.warm && !q.backfill;
    let seen = json!([[text, q.timestamp, q.file.len(), q.backfill, warm]]);
    assert_eq!(seen, c["seen"], "{}", c["name"]);
    let (status, accepted, message) = if q.sync {
        let outcome = match &c["summary"] {
            Value::String(f) if f == "fail" => None,
            d => Some(outcome_of(d)),
        };
        let (a, m) = ingest::sync_reply(outcome.as_ref());
        (200, a, m)
    } else {
        (202, true, ingest::ACCEPTED_MESSAGE.to_owned())
    };
    (status, pyjson::pydantic(&routes::insert_body(accepted, q.file.len(), &message)))
}

#[tokio::test]
async fn insert_route_cases() {
    for c in golden()["http"]["insert"].as_array().unwrap() {
        let (status, body) = insert_case(c).await;
        assert_eq!(status, c["status"].as_u64().unwrap() as u16, "{}", c["name"]);
        assert_eq!(body, s(&c["response"]), "{}", c["name"]);
    }
}

fn other(kind: &str) -> impl Iterator<Item = &'static Value> {
    golden()["http"]["other"].as_array().unwrap().iter().filter(move |c| c["kind"] == kind)
}

fn rid(c: &Value) -> String {
    let body: Value = serde_json::from_str(s(&c["response"])).unwrap_or(Value::Null);
    body["error"]["request_id"].as_str().unwrap_or("-").to_owned()
}

#[tokio::test]
async fn reset_and_status_routes() {
    for c in other("reset") {
        let h = HeaderMap::new();
        let mut p = Params::new(Vec::new(), s(&c["query"]).strip_prefix('?'), &h);
        let q = routes::ResetQuery::from_params(&mut p);
        if let Err(e) = p.v.finish(()) {
            assert_eq!(err_body(&rid(c), e.into()).await, (422, s(&c["response"]).to_owned()));
            continue;
        }
        let f = &c["fake"];
        let tally = ResetTally {
            entries: i(&f["hot_entries_deleted"]),
            estimates: i(&f["activity_estimates_deleted"]),
            boards: i(&f["boards_deleted"]),
        };
        assert_eq!(q.drop_boards, f["dropped_boards"].as_bool().unwrap());
        let summary = routes::reset_summary(&tally, q.drop_boards, i(&f["redis_keys_cleared"]));
        let mut body = Map::new();
        body.insert("reset".into(), true.into());
        body.extend(summary);
        assert_eq!(pyjson::pydantic(&Value::Object(body)), s(&c["response"]));
        let log = routes::reset_log(&tally, q.drop_boards, i(&f["redis_keys_cleared"]));
        assert_eq!(json!([hex_of(&doc! {"summary": log})]), c["logged"]);
    }
    for c in other("reingest_status") {
        let raw = c["redis"].as_str().map(str::as_bytes);
        let Value::Object(mut st) = backlog::status_from(raw) else { panic!("{c}") };
        let info = json!({"backlog_dir": "/data/backlog/leaderboards", "backlog_dir_exists": true, "backlog_files": 4});
        st.extend(info.as_object().unwrap().clone());
        assert_eq!(pyjson::pydantic(&Value::Object(st)), s(&c["response"]), "{}", c["name"]);
    }
}

fn board_rows(rows: &Value) -> Vec<BoardRow> {
    rows.as_array()
        .unwrap()
        .iter()
        .map(|r| BoardRow {
            uuid: i(&r["uuid"]),
            name: s(&r["name"]).into(),
            name_id: s(&r["name_id"]).into(),
            category: s(&r["category"]).into(),
            category_id: s(&r["category_id"]).into(),
            reset_kind_override: r["reset_kind_override"].as_str().map(str::to_owned),
        })
        .collect()
}

#[tokio::test]
async fn admin_board_routes() {
    let c = other("admin_boards").next().unwrap();
    let rows = board_rows(&c["rows"]);
    assert_eq!(pyjson::pydantic(&admin::board_list(rows.clone())), s(&c["response"]));
    for c in other("patch") {
        let ctype = c["content_type"].as_str();
        let rid = rid(c);
        let want = (c["status"].as_u64().unwrap() as u16, s(&c["response"]).to_owned());
        let body: BodyInput = match read_body(&headers(ctype), Body::from(b64(&c["body_b64"]))).await {
            Ok(b) => b,
            Err(e) => {
                assert_eq!(err_body(&rid, e).await, want, "{}", c["name"]);
                continue;
            }
        };
        let raw = s(&c["uuid"]);
        let decoded = percent_encoding::percent_decode_str(raw).decode_utf8_lossy().into_owned();
        let (uuid, value) = match admin::validate_patch(vec![("uuid".into(), decoded)], &body) {
            Ok(x) => x,
            Err(e) => {
                assert_eq!(err_body(&rid, e.into()).await, want, "{}", c["name"]);
                continue;
            }
        };
        if let Some(v) = value.as_deref()
            && !kiwi_core::lbmodels::RESET_KIND_VALUES.contains(&v)
        {
            assert_eq!(err_body(&rid, admin::bad_override(v)).await, want, "{}", c["name"]);
            continue;
        }
        let Some(row) = rows.iter().find(|r| r.uuid == uuid) else {
            let e = ApiError::not_found(format!("No leaderboard with uuid={uuid}"));
            assert_eq!(err_body(&rid, e).await, want, "{}", c["name"]);
            continue;
        };
        let row = BoardRow { reset_kind_override: value, ..row.clone() };
        assert_eq!((200, pyjson::pydantic(&admin::board_view(&row))), want, "{}", c["name"]);
    }
}

#[tokio::test]
async fn ingest_log_route() {
    for c in other("ingest_log") {
        let h = HeaderMap::new();
        let mut p = Params::new(Vec::new(), s(&c["query"]).strip_prefix('?'), &h);
        let q = admin::LogQuery::from_params(&mut p);
        if let Err(e) = p.v.finish(()) {
            assert_eq!(err_body(&rid(c), e.into()).await, (422, s(&c["response"]).to_owned()), "{}", c["name"]);
            continue;
        }
        let filter = admin::log_filter(q.endpoint.clone());
        let want_filter: Document = bson::to_document(&c["filter"][0]).unwrap();
        assert_eq!(filter, want_filter, "{}", c["name"]);
        let docs: Vec<Document> = c["docs"].as_array().unwrap().iter().map(|d| doc_of(s(d))).collect();
        let rows: Vec<Value> =
            docs.iter().take(q.limit.clamp(1, 200) as usize).map(|d| admin::log_row(d).unwrap()).collect();
        assert_eq!(pyjson::pydantic(&Value::Array(rows)), s(&c["response"]), "{}", c["name"]);
    }
}

#[tokio::test]
async fn job_status_routes() {
    let mut statuses = other("agg_status");
    assert_eq!(pyjson::pydantic(&admin::AggStatus::default().to_json()), s(&statuses.next().unwrap()["response"]));
    let c = statuses.next().unwrap();
    let st = &c["job"];
    let last = admin::AggStatus {
        running: st["running"].as_bool().unwrap(),
        captures: st["captures"].as_u64().unwrap(),
        first_anchor: st["first_anchor"].as_i64(),
        anchor: st["anchor"].as_i64(),
        rows: st["rows"].as_i64(),
        started_at: st["started_at"].as_i64(),
        finished_at: st["finished_at"].as_i64(),
        error: st["error"].as_str().map(str::to_owned),
    };
    assert_eq!(pyjson::pydantic(&last.to_json()), s(&c["response"]));
    let busy: Vec<&str> = other("busy").map(|c| s(&c["response"])).collect();
    let want: Vec<String> = [admin::REBUILD_BUSY, admin::LAST_RISE_BUSY, admin::COLD_BUSY]
        .iter()
        .map(|m| pyjson::pydantic(&json!({"started": false, "message": m})))
        .collect();
    assert_eq!(busy, want);
    for c in other("no_pg") {
        let e = ApiError::bad_request("Postgres backend is disabled");
        assert_eq!(err_body(&rid(c), e).await, (400, s(&c["response"]).to_owned()));
    }
    let last_run = admin::ColdStatus {
        running: false,
        moved: Some(vec!["entry_p_1".into()]),
        moved_bytes: Some(10),
        finished_at: Some(7),
        error: None,
    };
    for c in other("tier_status") {
        let parts: Vec<Partition> = c["parts"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|p| {
                Some(Partition {
                    oid: i(&p["oid"]) as u32,
                    name: s(&p["relname"]).into(),
                    lo: store::partition_lo(s(&p["relname"]))?,
                    tablespace: s(&p["tablespace"]).into(),
                    bytes: i(&p["bytes"]),
                })
            })
            .collect();
        let cold = c["cold"].as_bool().unwrap();
        let layout = admin::tier_layout(cold.then_some(&parts[..]), i(&c["after_days"]), i(&c["now"]));
        let body = json!({"last_run": last_run.to_json(), "layout": layout});
        assert_eq!(pyjson::pydantic(&body), s(&c["response"]), "{}", c["name"]);
    }
    let c = other("tier_no_pg").next().unwrap();
    assert_eq!(pyjson::pydantic(&json!({"last_run": last_run.to_json()})), s(&c["response"]));
}

/// A store-less state: routing, 405s and the form errors raised before the gate.
fn stack() -> Router {
    let settings = Settings::from_vars([("PY_UPSTREAM", "http://py:8000")]).unwrap();
    let upstream = Client::builder(TokioExecutor::new()).build_http();
    let state =
        AppState { inner: Arc::new(Inner { settings, db: Db { mongo: None, redis: None, pg: None }, upstream }) };
    let mw = Arc::new(Mw::new(
        MwConfig::from_lookup(&|_| None),
        None,
        EmbedSource::Fixed(Vec::new()),
        TrustedHosts::parse("127.0.0.1"),
    ));
    outer(wrap(super::router(), mw, None).fallback(|| async { (StatusCode::IM_A_TEAPOT, "proxied") }).with_state(state))
}

async fn send(
    app: &Router,
    method: &str,
    path: &str,
    rid: &str,
    ctype: Option<&str>,
    body: Vec<u8>,
) -> (u16, Option<String>, String) {
    let mut req = Request::builder()
        .method(Method::from_bytes(method.as_bytes()).unwrap())
        .uri(path)
        .header("x-request-id", rid)
        .header("content-length", body.len().to_string());
    if let Some(ct) = ctype {
        req = req.header("content-type", ct);
    }
    let res = app.clone().oneshot(req.body(Body::from(body)).unwrap()).await.unwrap();
    let allow = res.headers().get(header::ALLOW).map(|v| v.to_str().unwrap().to_owned());
    let status = res.status().as_u16();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
    (status, allow, String::from_utf8(bytes.to_vec()).unwrap())
}

#[tokio::test]
async fn routing_through_the_stack() {
    let app = stack();
    for c in other("method") {
        let (status, allow, body) = send(&app, s(&c["method"]), s(&c["path"]), &rid(c), None, Vec::new()).await;
        assert_eq!(status, c["status"].as_u64().unwrap() as u16, "{}", c["name"]);
        assert_eq!(allow.as_deref(), c["allow"].as_str(), "{}", c["name"]);
        if c["method"] != "HEAD" {
            assert_eq!(body, s(&c["response"]), "{}", c["name"]);
        }
    }
    for c in golden()["http"]["insert"].as_array().unwrap().iter().filter(|c| c["status"] == 400) {
        if c["name"] == "empty file" {
            continue;
        }
        let path = format!("/v1/leaderboards/insert{}", s(&c["query"]));
        let got = send(&app, "POST", &path, s(&c["request_id"]), c["content_type"].as_str(), b64(&c["body_b64"])).await;
        assert_eq!((got.0, got.2), (400, s(&c["response"]).to_owned()), "{}", c["name"]);
    }
}

#[test]
fn partition_names_and_fold_sql() {
    assert_eq!(store::partition_lo("entry_p_1761044400"), Some(1_761_044_400));
    assert_eq!(store::partition_lo("entry_p_x"), None);
    assert_eq!(store::partition_lo("entry_default"), None);
    assert_eq!(
        store::partition_ddl(1_761_130_000),
        "CREATE TABLE IF NOT EXISTS entry_p_1761044400 PARTITION OF entry FOR VALUES FROM (1761044400) TO (1761130800)"
    );
    assert!(store::fold_sql("player_board_agg_rebuild").starts_with("INSERT INTO player_board_agg_rebuild AS a"));
}

// --- stores (throwaway databases only) -------------------------------------------------

fn live_state(pg: Option<deadpool_postgres::Pool>) -> AppState {
    let settings = Settings::from_vars([("PY_UPSTREAM", "http://127.0.0.1:9")]).unwrap();
    let upstream = Client::builder(TokioExecutor::new()).build_http();
    AppState { inner: Arc::new(Inner { settings, db: Db { mongo: None, redis: None, pg }, upstream }) }
}

fn scratch_pool() -> deadpool_postgres::Pool {
    let dsn = std::env::var("POSTGRES_DSN").expect("POSTGRES_DSN");
    let cfg: tokio_postgres::Config = dsn.parse().unwrap();
    let mgr = deadpool_postgres::Manager::new(cfg, tokio_postgres::NoTls);
    deadpool_postgres::Pool::builder(mgr).max_size(4).build().unwrap()
}

/// A full write against the leaderboards schema, twice (the second replaces
/// the anchor), then the aggregate jobs over it.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs POSTGRES_DSN pointing at a throwaway postgres:17 with the leaderboards schema (writes and truncates)"]
async fn postgres_write_and_jobs() {
    let pool = scratch_pool();
    let st = live_state(Some(pool.clone()));
    let text = "N$Leaderboard_Category_Stats$1 = Trove Mastery$STATS##1;Bob;100|2;bob;90|3;Zoë;7\n\
                N$Leaderboard_Category_Contests$1 = Trove Mastery$CONTESTS##1;Bob;100\n";
    let boards = lbparse::parse_dump(text).unwrap();
    let anchor = 1_761_130_800;
    let first = store::write_snapshot(&pool, &boards, anchor).await.unwrap();
    assert_eq!((first.boards, first.entries), (1, 3));
    let again = store::write_snapshot(&pool, &boards, anchor).await.unwrap();
    assert_eq!(again.cleared, 3);
    assert_eq!(store::previous_anchor_before(&pool, anchor + 1).await.unwrap(), Some(anchor));
    assert_eq!(store::board_counts_at(&pool, anchor).await.unwrap(), HashMap::from([(1, 3)]));
    let rebuilt = store::rebuild_player_board_agg(&pool, &|_, _, _| {}).await.unwrap();
    assert_eq!(rebuilt, 2);
    store::backfill_last_rise(&pool, &|_, _, _| {}).await.unwrap();
    let out = ingest::insert_dump(&ingest::Live(&st), text, Some(anchor), true, &|| anchor).await.unwrap();
    assert!(matches!(out, Outcome::Stored { .. }));
}

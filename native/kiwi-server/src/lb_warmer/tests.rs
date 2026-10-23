//! Golden checks against `native/port/parity/gen_lb_warmer_fixtures.py`: the
//! pass orchestration, the boot probe, the loop cadence, the backlog replay
//! and the routes, each replayed against fakes that record what Python's did.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
};

use axum::{
    Router,
    body::Body,
    extract::Request,
    http::{HeaderMap, Method, StatusCode, header},
};
use base64::Engine;
use mongodb::bson::{Document, doc};
use serde_json::{Value, json};
use tower::ServiceExt;

use super::{
    pass::{self, Driver, Ops, Probe},
    reingest::{self, Runner},
    routes::{self, DeleteQuery, Maint, ReingestQuery},
};
use crate::{
    config::Settings,
    db::Db,
    error::ApiError,
    middleware::{EmbedSource, Mw, MwConfig, forwarded::TrustedHosts, outer, wrap},
    pyfloat, pyjson, request_id,
    state::{AppState, Inner},
    validation::{FromParams, Params},
};

fn golden() -> &'static Value {
    static G: OnceLock<Value> = OnceLock::new();
    G.get_or_init(|| {
        serde_json::from_str(include_str!("../../testdata/lb_warmer/golden.json")).expect("fixture parses")
    })
}

fn cases(key: &str) -> &'static Vec<Value> {
    golden()[key].as_array().unwrap_or_else(|| panic!("fixture section {key}"))
}

fn s(v: &Value) -> &str {
    v.as_str().expect("string")
}

fn fails(c: &Value, step: &str) -> bool {
    c["fail"].as_array().is_some_and(|f| f.iter().any(|x| x == step))
}

type Trace = Mutex<Vec<String>>;

fn push(t: &Trace, line: impl Into<String>) {
    t.lock().unwrap().push(line.into());
}

// --- the pass ------------------------------------------------------------------------

struct FakeOps {
    c: &'static Value,
    now: f64,
    trace: Trace,
}

impl FakeOps {
    fn step(&self, name: &str) -> anyhow::Result<()> {
        push(&self.trace, name);
        if fails(self.c, name) { Err(anyhow::anyhow!("{name}")) } else { Ok(()) }
    }
}

impl Ops for FakeOps {
    fn now(&self) -> f64 {
        self.now
    }
    async fn detect(&self) -> anyhow::Result<Arc<Value>> {
        self.step("detect")?;
        Ok(Arc::new(self.c.get("detect").cloned().unwrap_or(json!({"anchor": 1}))))
    }
    async fn activity(&self) -> anyhow::Result<()> {
        self.step("activity")
    }
    async fn class_activity(&self) -> anyhow::Result<()> {
        self.step("class_activity")
    }
    async fn renames(&self) -> anyhow::Result<()> {
        self.step("renames")
    }
    async fn duplicates(&self) -> anyhow::Result<()> {
        self.step("duplicates")
    }
    async fn warm_cache(&self) -> anyhow::Result<()> {
        self.step("cache.warm")
    }
    async fn latest(&self) -> anyhow::Result<Option<i64>> {
        push(&self.trace, "latest 1 False");
        if fails(self.c, "latest") {
            anyhow::bail!("latest");
        }
        Ok(self.c["latest"].as_array().and_then(|l| l.first()).and_then(Value::as_i64))
    }
    async fn set_ready_anchor(&self, anchor: i64) {
        push(&self.trace, format!("set_ready_anchor {anchor}"));
    }
    async fn warm_board_histories(&self, anchor: i64) -> anyhow::Result<usize> {
        push(&self.trace, format!("warm_board_histories {anchor}"));
        if fails(self.c, "warm_board_histories") { Err(anyhow::anyhow!("warm_board_histories")) } else { Ok(4) }
    }
    async fn warm_days(&self) -> anyhow::Result<()> {
        self.step("warm_days")
    }
    fn postgres_enabled(&self) -> bool {
        self.c["pg"].as_bool().unwrap_or(true)
    }
    async fn tier_after_days(&self) -> anyhow::Result<Value> {
        push(&self.trace, "setting leaderboards_pg_tier_after_days");
        if fails(self.c, "tier_setting") {
            anyhow::bail!("tier_setting");
        }
        Ok(self.c.get("after_days").cloned().unwrap_or(json!(7)))
    }
    async fn tier_cold(&self, after_days: i64, now: i64, limit: usize) -> anyhow::Result<Vec<String>> {
        push(&self.trace, format!("tier_cold {after_days} {now} {limit}"));
        if fails(self.c, "tier_cold") {
            anyhow::bail!("tier_cold");
        }
        let moved = self.c["tier"]["moved"].as_array().cloned().unwrap_or_default();
        Ok(moved.iter().map(|m| s(m).to_owned()).collect())
    }
}

#[tokio::test]
async fn warm_all_matches_python() {
    for c in cases("warm_all") {
        let ops = FakeOps { c, now: c["now"].as_f64().unwrap(), trace: Mutex::new(Vec::new()) };
        let mut last_cold = c["last_cold"].as_f64().unwrap_or(0.0);
        let res = pass::warm_all(&ops, &mut last_cold).await;
        let name = s(&c["name"]);
        assert_eq!(json!(*ops.trace.lock().unwrap()), c["trace"], "{name}");
        assert_eq!(res.err().map(|e| e.to_string()), c["raised"].as_str().map(str::to_owned), "{name}");
        assert_eq!(last_cold, c["last_cold_after"].as_f64().unwrap(), "{name}");
    }
}

// --- the boot probe -----------------------------------------------------------------

struct FakeProbe {
    c: &'static Value,
    trace: Trace,
}

impl Probe for FakeProbe {
    fn now(&self) -> f64 {
        self.c["now"].as_f64().unwrap()
    }
    async fn ready_anchor(&self) -> Option<i64> {
        push(&self.trace, "ready");
        self.c["ready"].as_i64()
    }
    async fn persisted(&self, anchor: i64) -> Option<Value> {
        push(&self.trace, format!("persisted {anchor}"));
        self.c.get("persisted").filter(|v| !v.is_null()).cloned()
    }
    async fn timestamps(&self) -> anyhow::Result<Vec<i64>> {
        push(&self.trace, "latest 1 False");
        match &self.c["latest"] {
            Value::String(_) => anyhow::bail!("latest"),
            Value::Array(l) => Ok(l.iter().filter_map(Value::as_i64).collect()),
            _ => Ok(Vec::new()),
        }
    }
    async fn ttl(&self) -> anyhow::Result<Value> {
        push(&self.trace, "setting cheaters_cache_ttl_seconds");
        match self.c.get("ttl") {
            Some(Value::String(f)) if f == "fail" => anyhow::bail!("ttl"),
            Some(v) => Ok(v.clone()),
            None => Ok(json!(1800)),
        }
    }
}

#[tokio::test]
async fn boot_probe_matches_python() {
    for c in cases("probe") {
        let p = FakeProbe { c, trace: Mutex::new(Vec::new()) };
        let got = pass::persisted_pass_age(&p).await;
        let name = s(&c["name"]);
        assert_eq!(json!(*p.trace.lock().unwrap()), c["trace"], "{name}");
        assert_eq!(got.map(pyfloat::repr), c["result"].as_str().map(str::to_owned), "{name}");
    }
}

// --- the loop -----------------------------------------------------------------------

struct FakeDriver {
    it: &'static Value,
    clock: Mutex<f64>,
    trace: Trace,
}

impl Driver for FakeDriver {
    fn now(&self) -> f64 {
        *self.clock.lock().unwrap()
    }
    async fn resume_age(&self) -> Option<f64> {
        push(&self.trace, "probe");
        self.it["probe"].as_f64()
    }
    async fn pass(&self) -> anyhow::Result<()> {
        push(&self.trace, "pass");
        *self.clock.lock().unwrap() += self.it["work"].as_f64().unwrap_or(0.0);
        if self.it["fail"].as_bool() == Some(true) { Err(anyhow::anyhow!("pass")) } else { Ok(()) }
    }
    async fn ttl(&self) -> anyhow::Result<Value> {
        push(&self.trace, "ttl");
        match self.it.get("ttl") {
            Some(Value::String(f)) if f == "fail" => anyhow::bail!("ttl"),
            Some(v) => Ok(v.clone()),
            None => Ok(json!(1800)),
        }
    }
}

#[tokio::test]
async fn loop_cadence_matches_python() {
    for c in cases("loop") {
        let mut trace = vec!["sleep 2.0".to_owned()];
        let mut clock = 1_761_130_000.5;
        for (i, it) in c["iterations"].as_array().unwrap().iter().enumerate() {
            let d = FakeDriver { it, clock: Mutex::new(clock), trace: Mutex::new(Vec::new()) };
            let secs = pass::iteration(&d, i == 0).await;
            trace.extend(d.trace.into_inner().unwrap());
            trace.push(format!("wait {}", pyfloat::repr(secs)));
            clock = d.clock.into_inner().unwrap();
        }
        assert_eq!(json!(trace), c["trace"], "{}", s(&c["name"]));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn off_executor_drives_timers_and_tasks() {
    let got = super::off_executor(|| async {
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        tokio::spawn(async { 7 }).await.unwrap()
    })
    .await
    .unwrap();
    assert_eq!(got, 7);
    let err = super::off_executor(|| async { super::guarded(async { panic!("boom") }).await.map(|()| 0) })
        .await
        .unwrap()
        .unwrap_err();
    assert_eq!(err.to_string(), "panicked: boom");
}

// --- the backlog --------------------------------------------------------------------

fn b64(v: &Value) -> Vec<u8> {
    base64::engine::general_purpose::STANDARD.decode(s(v)).unwrap()
}

#[test]
fn read_text_matches_python() {
    for c in cases("read_text") {
        let got = reingest::decode_text(s(&c["name"]), &b64(&c["data_b64"])).unwrap();
        assert_eq!(got.as_bytes(), b64(&c["text_b64"]), "{}", s(&c["name"]));
    }
    assert_eq!(reingest::decode_text("x.gz", b"nope").unwrap_err(), "Not a gzipped file (b'no')");
    let missing = Path::new("/nonexistent/kiwi/1700000000.cfg");
    let e = std::fs::read(missing).unwrap_err();
    let text = reingest::os_error(&e, missing);
    assert!(text.starts_with("[Errno ") && text.ends_with(": '/nonexistent/kiwi/1700000000.cfg'"), "{text}");
    #[cfg(unix)]
    assert_eq!(text, "[Errno 2] No such file or directory: '/nonexistent/kiwi/1700000000.cfg'");
}

struct FakeRunner {
    c: &'static Value,
    trace: Trace,
    local: Mutex<Option<Value>>,
}

impl FakeRunner {
    fn file(&self, anchor: i64) -> &'static Value {
        self.c["files"].as_array().unwrap().iter().find(|f| f[0] == anchor).expect("scripted file")
    }
}

impl Runner for FakeRunner {
    fn has_redis(&self) -> bool {
        self.c["redis"].as_bool().unwrap_or(true)
    }
    async fn lock(&self) -> anyhow::Result<bool> {
        push(&self.trace, format!("SET lb:reingest:running 1 NX EX {}", reingest::LOCK_TTL));
        Ok(self.c["lock"].as_bool().unwrap_or(true))
    }
    async fn put_status(&self, json: &str) {
        push(&self.trace, format!("SET lb:reingest:status {json} EX {}", reingest::STATUS_TTL));
    }
    fn keep_local(&self, status: &Value) {
        *self.local.lock().unwrap() = Some(status.clone());
    }
    async fn heartbeat(&self) {
        push(&self.trace, format!("EXPIRE lb:reingest:running {}", reingest::LOCK_TTL));
    }
    async fn unlock(&self) {
        push(&self.trace, "DEL lb:reingest:running");
    }
    fn files(&self) -> Vec<(i64, PathBuf)> {
        let files = self.c["files"].as_array().unwrap();
        files
            .iter()
            .map(|f| {
                let a = f[0].as_i64().unwrap();
                (a, PathBuf::from(format!("/data/backlog/leaderboards/{a}.cfg.gz")))
            })
            .collect()
    }
    fn now(&self) -> i64 {
        1_761_130_000
    }
    async fn reset_all(&self) -> anyhow::Result<()> {
        push(&self.trace, "reset_all False");
        if self.c["reset_fails"].as_bool() == Some(true) { Err(anyhow::anyhow!("reset")) } else { Ok(()) }
    }
    async fn read_text(&self, path: &Path) -> Result<String, String> {
        let name = path.file_name().unwrap().to_string_lossy();
        let a: i64 = name.split('.').next().unwrap().parse().unwrap();
        push(&self.trace, format!("read {a}"));
        let f = self.file(a);
        if f[1] == "read_fail" { Err(s(&f[2]).to_owned()) } else { Ok(format!("dump {a}")) }
    }
    async fn insert(&self, text: String, anchor: i64) -> Result<Option<i64>, String> {
        push(&self.trace, format!("insert {} {anchor} True", kiwi_core::pystr::repr(&text)));
        let f = self.file(anchor);
        if f[1] == "fail" { Err(s(&f[2]).to_owned()) } else { Ok(f[2].as_i64()) }
    }
    async fn backfill_activity(&self) -> anyhow::Result<()> {
        push(&self.trace, format!("backfill_activity {} True False", reingest::BACKFILL_DAYS));
        if self.c["activity_fails"].as_bool() == Some(true) { Err(anyhow::anyhow!("activity")) } else { Ok(()) }
    }
    async fn backfill_class(&self) -> anyhow::Result<()> {
        push(&self.trace, format!("backfill_class {} True False", reingest::BACKFILL_DAYS));
        if self.c["class_fails"].as_bool() == Some(true) { Err(anyhow::anyhow!("class")) } else { Ok(()) }
    }
    fn trigger(&self) {
        push(&self.trace, "trigger");
    }
}

#[tokio::test]
async fn reingest_matches_python() {
    for c in cases("reingest") {
        let r = FakeRunner { c, trace: Mutex::new(Vec::new()), local: Mutex::new(None) };
        let res = reingest::run(&r, c["clear_first"].as_bool().unwrap_or(false)).await;
        let name = s(&c["name"]);
        assert_eq!(json!(*r.trace.lock().unwrap()), c["trace"], "{name}");
        assert_eq!(res.err().map(|e| e.to_string()), c["raised"].as_str().map(str::to_owned), "{name}");
        let local = r.local.into_inner().unwrap().unwrap_or(json!({"running": false}));
        assert_eq!(pyjson::dumps_spaced(&local), s(&c["local"]), "{name}");
    }
}

// --- routes -------------------------------------------------------------------------

fn http(key: &str) -> &'static Vec<Value> {
    golden()["http"][key].as_array().unwrap()
}

fn rid(c: &Value) -> String {
    let body: Value = serde_json::from_str(s(&c["response"])).unwrap_or(Value::Null);
    body["error"]["request_id"].as_str().unwrap_or("-").to_owned()
}

async fn err_body(rid: &str, e: ApiError) -> String {
    request_id::scope(rid.to_owned(), async move { e.body() }).await
}

fn hex_of(d: &Document) -> String {
    let mut out = Vec::new();
    d.to_writer(&mut out).expect("bson encodes");
    hex::encode(out)
}

struct FakeMaint {
    deleted: i64,
    estimate_fails: bool,
    trace: Trace,
    backfills: Mutex<usize>,
}

impl Maint for FakeMaint {
    async fn delete_anchor(&self, anchor: i64) -> anyhow::Result<i64> {
        push(&self.trace, format!("delete_anchor {anchor}"));
        Ok(self.deleted)
    }
    async fn delete_estimate(&self, anchor: i64) -> anyhow::Result<()> {
        push(&self.trace, format!("delete_estimate {anchor}"));
        if self.estimate_fails { Err(anyhow::anyhow!("estimate")) } else { Ok(()) }
    }
    async fn invalidate_anchor(&self, anchor: i64) {
        push(&self.trace, format!("invalidate_anchor {anchor}"));
    }
    async fn log(&self, summary: Document) {
        push(&self.trace, format!("log {} {}", routes::DELETE_ENDPOINT, hex_of(&doc! {"summary": summary})));
    }
    fn reset_detection(&self) {
        push(&self.trace, "reset");
    }
    fn trigger_warmer(&self) {
        push(&self.trace, "trigger");
    }
    fn backfill(&self) {
        *self.backfills.lock().unwrap() += 1;
    }
}

fn params<'a>(query: &'a str, h: &'a HeaderMap) -> Params<'a> {
    Params::new(Vec::new(), query.strip_prefix('?'), h)
}

#[tokio::test]
async fn route_work_matches_python() {
    let warm = &http("warm")[0];
    assert_eq!((warm["status"].as_u64(), s(&warm["response"])), (Some(202), r#"{"warming":true}"#));
    let h = HeaderMap::new();
    for c in http("delete") {
        let name = s(&c["name"]);
        let mut p = params(s(&c["query"]), &h);
        let q = DeleteQuery::from_params(&mut p);
        if let Err(e) = p.v.finish(()) {
            assert_eq!(c["status"], 422, "{name}");
            assert_eq!(err_body(&rid(c), e.into()).await, s(&c["response"]), "{name}");
            continue;
        }
        let m = FakeMaint {
            deleted: c["deleted"].as_i64().unwrap_or(0),
            estimate_fails: c["estimate_fails"].as_bool() == Some(true),
            trace: Mutex::new(Vec::new()),
            backfills: Mutex::new(0),
        };
        let body = routes::delete_anchor_with(&m, q.anchor, q.recompute).await.unwrap();
        assert_eq!(c["status"], 200, "{name}");
        assert_eq!(pyjson::pydantic(&body), s(&c["response"]), "{name}");
        assert_eq!(json!(*m.trace.lock().unwrap()), c["trace"], "{name}");
        let want: Vec<Value> = c["backfills"].as_array().unwrap().clone();
        assert_eq!(*m.backfills.lock().unwrap(), want.len(), "{name}");
        assert!(want.iter().all(|b| *b == json!({"reset": true, "total_days": 0})), "{name}");
    }
    for c in http("reingest") {
        let name = s(&c["name"]);
        let mut p = params(s(&c["query"]), &h);
        let q = ReingestQuery::from_params(&mut p);
        if let Err(e) = p.v.finish(()) {
            assert_eq!(c["status"], 422, "{name}");
            assert_eq!(err_body(&rid(c), e.into()).await, s(&c["response"]), "{name}");
            continue;
        }
        let files = c["files"].as_u64().unwrap() as usize;
        assert_eq!(c["status"], 202, "{name}");
        assert_eq!(pyjson::pydantic(&routes::reingest_reply(files, q.clear_first)), s(&c["response"]), "{name}");
        let started = c["started"].as_array().unwrap();
        assert_eq!(started.len(), usize::from(files > 0), "{name}");
        assert!(started.iter().all(|k| k["clear_first"] == q.clear_first), "{name}");
    }
}

fn stack() -> Router {
    let settings = Settings::default();
    let state = AppState { inner: Arc::new(Inner { settings, db: Db { mongo: None, redis: None, pg: None } }) };
    let mw = Arc::new(Mw::new(
        MwConfig::from_lookup(&|_| None),
        None,
        EmbedSource::Fixed(Vec::new()),
        TrustedHosts::parse("127.0.0.1"),
    ));
    outer(
        wrap(super::router(), mw, None).fallback(|| async { (StatusCode::IM_A_TEAPOT, "unrouted") }).with_state(state),
    )
}

#[tokio::test]
async fn wrong_methods_are_405() {
    let app = stack();
    for c in http("method") {
        let req = Request::builder()
            .method(Method::from_bytes(s(&c["method"]).as_bytes()).unwrap())
            .uri(s(&c["path"]))
            .header("x-request-id", rid(c))
            .body(Body::empty())
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        let allow = res.headers().get(header::ALLOW).map(|v| v.to_str().unwrap().to_owned());
        assert_eq!(res.status().as_u16(), c["status"].as_u64().unwrap() as u16, "{c}");
        assert_eq!(allow.as_deref(), c["allow"].as_str(), "{c}");
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
        if c["method"] != "HEAD" {
            assert_eq!(std::str::from_utf8(&bytes).unwrap(), s(&c["response"]), "{c}");
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs POSTGRES_DSN pointing at a throwaway postgres:17 with the leaderboards schema (writes and deletes)"]
async fn postgres_deletes() {
    let cfg: tokio_postgres::Config = std::env::var("POSTGRES_DSN").expect("POSTGRES_DSN").parse().unwrap();
    let pool = deadpool_postgres::Pool::builder(deadpool_postgres::Manager::new(cfg, tokio_postgres::NoTls))
        .max_size(2)
        .build()
        .unwrap();
    let anchor = 1_761_130_800;
    let con = pool.get().await.unwrap();
    con.batch_execute(&crate::lb_ingest::store::partition_ddl(anchor)).await.unwrap();
    con.execute(
        "INSERT INTO entry (board_uuid, anchor, rank, score, player_id) VALUES (1, $1, 1, 5, 1), (1, $1, 2, 4, 2)",
        &[&anchor],
    )
    .await
    .unwrap();
    con.execute(
        "INSERT INTO activity_estimate (window_end, window_start, duration_hours, estimate, boards_analyzed, computed_at) \
         VALUES ($1, $1 - 3600, 1.0, 2, 1, $1) ON CONFLICT DO NOTHING",
        &[&anchor],
    )
    .await
    .unwrap();
    assert_eq!(super::store::delete_anchor(&pool, anchor).await.unwrap(), 2);
    assert_eq!(super::store::delete_anchor(&pool, anchor).await.unwrap(), 0);
    super::store::delete_estimate(&pool, anchor).await.unwrap();
    let left: i64 =
        con.query_one("SELECT count(*) FROM activity_estimate WHERE window_end = $1", &[&anchor]).await.unwrap().get(0);
    assert_eq!(left, 0);
}

#[test]
fn sleep_floor_and_values() {
    assert_eq!(pass::next_sleep(1800.0, 250.25, None), 1549.75);
    assert_eq!(pass::next_sleep(f64::NAN, 0.0, None), 30.0);
    assert_eq!(pass::next_sleep(f64::INFINITY, 1.0, None), f64::INFINITY);
    assert_eq!(pass::py_int(&json!(" 12 ")), Some(12));
    assert_eq!(pass::py_int(&json!("3.9")), None);
    assert_eq!(pass::py_float(&json!("1_000")), Some(1000.0));
}

//! Replays `testdata/lbread/fixtures.json`, captured from the real Python
//! routes by `native/port/parity/gen_lbread_fixtures.py`: every response, every
//! `pg_store` call (in order, with its arguments) and every Redis write must
//! match. Postgres answers are the ones Python got.

use std::{
    collections::{HashMap, VecDeque},
    net::SocketAddr,
    sync::{Arc, Mutex, OnceLock},
};

use axum::{
    Router,
    body::{Body, to_bytes},
    extract::{ConnectInfo, FromRequestParts, State},
    http::{Method, Request, StatusCode, Uri, request::Parts},
    response::{IntoResponse, Response},
    routing::get,
};
use kiwi_core::lbread::{self as core, HealthItem, Stamp};
use serde_json::{Map, Value, json};
use tower::ServiceExt;

use super::{
    cache::{Kv, py_int},
    defers, parse_uuid,
    routes::*,
    store::{Board, BoardRow, EntryRow, Res, SeriesRow, Source, TopEntry, TopMeta, TopSeries},
};
use crate::{auth::Rejection, error::ApiError, features, pyjson, ratelimit::RateLimitInfo, validation::Query};

fn fixtures() -> &'static Value {
    static F: OnceLock<Value> = OnceLock::new();
    F.get_or_init(|| pyjson::loads(include_bytes!("../../testdata/lbread/fixtures.json")).expect("fixtures"))
}

fn token() -> &'static str {
    fixtures()["token_id"].as_str().unwrap()
}

const ARCHIVE_RESET: i64 = 1_761_143_460;

struct Fake {
    label: String,
    calls: Mutex<VecDeque<Value>>,
    store: Mutex<HashMap<String, String>>,
    mode: String,
    sets: Mutex<Vec<Value>>,
    settings: Map<String, Value>,
    now_us: i64,
    archive_keys: Mutex<Vec<Value>>,
}

impl Fake {
    fn new(case: &Value) -> Fake {
        let store = case["redis_before"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_owned()))
            .collect();
        Fake {
            label: case["path"].as_str().unwrap().to_owned(),
            calls: Mutex::new(case["calls"].as_array().unwrap().iter().cloned().collect()),
            store: Mutex::new(store),
            mode: case["redis_mode"].as_str().unwrap().to_owned(),
            sets: Mutex::default(),
            settings: case["settings"].as_object().unwrap().clone(),
            now_us: case["now_us"].as_i64().unwrap(),
            archive_keys: Mutex::default(),
        }
    }

    fn take(&self, name: &str, args: Value) -> Value {
        let call = self.calls.lock().unwrap().pop_front();
        let call = call.unwrap_or_else(|| panic!("{}: unexpected {name}{args}", self.label));
        assert_eq!(call[0], name, "{}: call order", self.label);
        assert_eq!(call[1], args, "{}: {name} arguments", self.label);
        call[2].clone()
    }
}

fn ints(v: &Value) -> Vec<i64> {
    v.as_array().unwrap().iter().map(|x| x.as_i64().unwrap()).collect()
}

fn opt_str(v: &Value) -> Option<String> {
    v.as_str().map(str::to_owned)
}

fn board_row(v: &Value) -> BoardRow {
    BoardRow {
        uuid: v["uuid"].as_i64().unwrap(),
        name_id: v["name_id"].as_str().unwrap().into(),
        name: v["name"].as_str().unwrap().into(),
        category_id: v["category_id"].as_str().unwrap().into(),
        category: v["category"].as_str().unwrap().into(),
        reset_kind_override: opt_str(&v["reset_kind_override"]),
        contest_type: v.get("contest_type").and_then(opt_str),
    }
}

fn kind(s: &str) -> &'static str {
    match s {
        "daily" => "daily",
        "weekly" => "weekly",
        "none" => "none",
        _ => "default",
    }
}

impl Source for Fake {
    async fn list_timestamps(&self, limit: i64, since: Option<i64>) -> Res<Vec<i64>> {
        Ok(ints(&self.take("list_timestamps", json!([limit, since]))))
    }

    async fn list_days(&self, limit: i64) -> Res<Vec<i64>> {
        Ok(ints(&self.take("list_days", json!([limit]))))
    }

    async fn list_boards_at(&self, anchor: i64) -> Res<Vec<BoardRow>> {
        let ret = self.take("list_boards_at", json!([anchor]));
        let rows: Vec<BoardRow> = ret.as_array().unwrap().iter().map(board_row).collect();
        for (row, want) in rows.iter().zip(ret.as_array().unwrap()) {
            assert_eq!(pyjson::dumps(&row.to_value()), pyjson::dumps(want), "{}: board dict", self.label);
        }
        Ok(rows)
    }

    async fn get_board(&self, uuid: i32) -> Res<Option<Board>> {
        let ret = self.take("get_board", json!([uuid]));
        if ret.is_null() {
            return Ok(None);
        }
        let contests = ret["contests"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| (c["time"].as_i64().unwrap(), c["type"].as_str().unwrap().to_owned()))
            .collect();
        let row = board_row(&ret);
        assert_eq!(row.reset_kind(), ret["reset_kind"], "{}", self.label);
        Ok(Some(Board { row, contests }))
    }

    async fn latest_anchor_for_board(&self, uuid: i32) -> Res<Option<i64>> {
        Ok(self.take("latest_anchor_for_board", json!([uuid])).as_i64())
    }

    async fn top_entries_for_boards(&self, uuids: &[i32], anchor: i64) -> Res<HashMap<i64, TopEntry>> {
        let ret = self.take("top_entries_for_boards", json!([uuids, anchor]));
        Ok(ret
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| {
                let e = TopEntry {
                    player_name: v["player_name"].as_str().unwrap().into(),
                    score: v["score"].as_f64().unwrap(),
                    anchor: v["anchor"].as_i64().unwrap(),
                };
                (k.parse().unwrap(), e)
            })
            .collect())
    }

    async fn list_entries(&self, uuid: i32, anchor: i64, limit: i64, offset: i64) -> Res<(Vec<EntryRow>, i64)> {
        let ret = self.take("list_entries", json!([uuid, anchor, limit, offset]));
        let items = ret[0]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| EntryRow {
                player_name: r["player_name"].as_str().unwrap().into(),
                rank: r["rank"].as_i64().unwrap(),
                score: r["score"].as_f64().unwrap(),
            })
            .collect();
        Ok((items, ret[1].as_i64().unwrap()))
    }

    async fn previous_day_anchor(&self, uuid: i32, anchor: i64) -> Res<Option<i64>> {
        Ok(self.take("previous_day_anchor", json!([uuid, anchor])).as_i64())
    }

    async fn board_reset_kind(&self, uuid: i32) -> Res<&'static str> {
        Ok(kind(self.take("board_reset_kind", json!([uuid])).as_str().unwrap()))
    }

    async fn prev_rows_for_players(
        &self,
        uuid: i32,
        prev_anchor: i64,
        names: &[String],
    ) -> Res<HashMap<String, (i64, f64)>> {
        let ret = self.take("prev_rows_for_players", json!([uuid, prev_anchor, names]));
        Ok(ret
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), (v["rank"].as_i64().unwrap(), v["score"].as_f64().unwrap())))
            .collect())
    }

    async fn board_top_series(&self, uuid: i32, window_start: i64, top: i64) -> Res<TopSeries> {
        let ret = self.take("board_top_series", json!([uuid, window_start, top]));
        let top = ret[2]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| TopMeta {
                player_id: m["player_id"].as_i64().unwrap(),
                player_name: m["player_name"].as_str().unwrap().into(),
                rank: m["rank"].as_i64().unwrap(),
            })
            .collect();
        let rows = ret[3]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| SeriesRow {
                player_id: r["player_id"].as_i64().unwrap(),
                created_at: r["created_at"].as_i64().unwrap(),
                rank: r["rank"].as_i64().unwrap(),
                score: r["score"].as_f64().unwrap(),
            })
            .collect();
        Ok(TopSeries { anchors: ints(&ret[0]), top, rows })
    }
}

impl Kv for Fake {
    async fn get(&self, key: &str) -> Option<Value> {
        if self.mode != "up" {
            return None;
        }
        let raw = self.store.lock().unwrap().get(key).cloned()?;
        pyjson::loads(raw.as_bytes()).ok()
    }

    async fn set(&self, key: &str, value: &Value, ttl: i64) {
        if self.mode != "up" {
            return;
        }
        let raw = pyjson::dumps_spaced(value);
        self.sets.lock().unwrap().push(json!([key, raw, ttl]));
        self.store.lock().unwrap().insert(key.to_owned(), raw);
    }

    async fn retention_days(&self) -> i64 {
        match self.setting("leaderboards_hot_retention_days").await {
            Ok(v) => py_int(&v).map(|d| d.max(1)).unwrap_or(3),
            Err(_) => 3,
        }
    }

    fn configured(&self) -> bool {
        self.mode != "none"
    }
}

impl Env for Fake {
    async fn setting(&self, key: &str) -> anyhow::Result<Value> {
        match self.settings.get(key) {
            Some(v) if v.get("error").is_some() => anyhow::bail!("{key}: {}", v["error"]),
            Some(v) => Ok(v.clone()),
            None if key.starts_with("feature_") => Ok(Value::Bool(true)),
            None => anyhow::bail!("KeyError: {key}"),
        }
    }

    fn now_us(&self) -> i64 {
        self.now_us
    }

    async fn charge_archive(&self, token_id: &str) -> Result<RateLimitInfo, Rejection> {
        let (max, window) = (
            &self.settings["leaderboards_archive_rate_limit_max"],
            &self.settings["leaderboards_archive_rate_limit_window_seconds"],
        );
        self.archive_keys.lock().unwrap().push(json!([format!("lb_archive:{token_id}"), max, window]));
        Ok(RateLimitInfo { limit: 10, remaining: 7, reset: ARCHIVE_RESET })
    }
}

type St = State<Arc<Fake>>;

/// `require_leaderboards_enabled` over the fake settings.
struct V1On;

impl FromRequestParts<Arc<Fake>> for V1On {
    type Rejection = Response;

    async fn from_request_parts(_: &mut Parts, f: &Arc<Fake>) -> Result<Self, Response> {
        match f.setting(features::LEADERBOARDS_FLAG).await {
            Ok(v) if features::py_truthy(&v) => Ok(V1On),
            _ => Err(features::disabled("Leaderboards").into_response()),
        }
    }
}

/// `_resolve_feature_flags` over the fake settings.
struct SiteOn;

impl FromRequestParts<Arc<Fake>> for SiteOn {
    type Rejection = Response;

    async fn from_request_parts(_: &mut Parts, f: &Arc<Fake>) -> Result<Self, Response> {
        match f.setting(features::LEADERBOARDS_FLAG).await {
            Ok(v) if features::py_truthy(&v) => Ok(SiteOn),
            _ => Err(ApiError::http(StatusCode::NOT_FOUND, None).into_response()),
        }
    }
}

fn out(r: Reply) -> Response {
    r.unwrap_or_else(IntoResponse::into_response)
}

fn id(uri: &Uri) -> i32 {
    uuid_of(uri).expect("int segment")
}

/// The fixture app: the real routes with their scope dependency overridden.
fn app(fake: Arc<Fake>) -> Router {
    Router::new()
        .route("/v1/leaderboards", get(|_: V1On, State(f): St, Query(q): Query<CreatedAtQ>| async move {
            out(v1_boards(&*f, token(), q.created_at).await)
        }))
        .route("/v1/leaderboards/timestamps", get(|_: V1On, State(f): St, Query(q): Query<LimitQ>| async move {
            out(v1_timestamps(&*f, q.limit).await)
        }))
        .route("/v1/leaderboards/records", get(|_: V1On, State(f): St| async move { out(v1_records(&*f).await) }))
        .route("/v1/leaderboards/{uuid}", get(|_: V1On, State(f): St, uri: Uri| async move {
            out(v1_board(&*f, id(&uri)).await)
        }))
        .route(
            "/v1/leaderboards/{uuid}/entries",
            get(|_: V1On, State(f): St, uri: Uri, Query(q): Query<EntriesQ>| async move {
                out(v1_entries(&*f, token(), id(&uri), &q).await)
            }),
        )
        .route(
            "/v1/leaderboards/{uuid}/history",
            get(|_: V1On, State(f): St, uri: Uri, Query(q): Query<HistoryQ>| async move {
                out(v1_history(&*f, token(), id(&uri), &q).await)
            }),
        )
        .route("/v1/leaderboards/{uuid}/health", get(|_: V1On, State(f): St, uri: Uri| async move {
            out(v1_health(&*f, id(&uri)).await)
        }))
        .route("/site/leaderboards/config", get(|_: SiteOn, State(f): St| async move { out(site_config(&*f).await) }))
        .route("/site/leaderboards/timestamps", get(|_: SiteOn, State(f): St, Query(q): Query<LimitQ>| async move {
            out(site_timestamps(&*f, q.limit).await)
        }))
        .route("/site/leaderboards/days", get(|_: SiteOn, State(f): St| async move { out(site_days(&*f).await) }))
        .route("/site/leaderboards/boards", get(|_: SiteOn, State(f): St, Query(q): Query<CreatedAtQ>| async move {
            out(site_boards(&*f, q.created_at).await)
        }))
        .route("/site/leaderboards/records", get(|_: SiteOn, State(f): St| async move { out(site_records(&*f).await) }))
        .route(
            "/site/leaderboards/{uuid}/entries",
            get(|_: SiteOn, State(f): St, uri: Uri, Query(q): Query<EntriesQ>| async move {
                out(site_entries(&*f, id(&uri), &q).await)
            }),
        )
        .route(
            "/site/leaderboards/{uuid}/history",
            get(|_: SiteOn, State(f): St, uri: Uri, Query(q): Query<HistoryQ>| async move {
                out(site_history(&*f, id(&uri), &q).await)
            }),
        )
        .route("/site/leaderboards/{uuid}/health", get(|_: SiteOn, State(f): St, uri: Uri| async move {
            out(site_health(&*f, id(&uri)).await)
        }))
        .with_state(fake)
}

const KEEP_HEADERS: [&str; 5] = [
    "content-type",
    "cache-control",
    "x-ratelimit-archive-limit",
    "x-ratelimit-archive-remaining",
    "x-ratelimit-archive-reset",
];

async fn check_reply(resp: Response, case: &Value, label: &str) {
    assert_eq!(resp.status().as_u16(), case["status"].as_u64().unwrap() as u16, "{label} status");
    for name in KEEP_HEADERS {
        let got = resp.headers().get(name).map(|v| v.to_str().unwrap().to_owned());
        assert_eq!(got.as_deref(), case["headers"][name].as_str(), "{label} header {name}");
    }
    let png = case["headers"]["content-type"] == "image/png";
    let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let got = if png { hex::encode(&body) } else { String::from_utf8(body.to_vec()).unwrap() };
    assert_eq!(got, case["body"].as_str().unwrap(), "{label} body");
}

#[tokio::test]
async fn routes_match_python() {
    let cases = fixtures()["cases"].as_array().unwrap();
    assert!(cases.len() > 100);
    for case in cases {
        let path = case["path"].as_str().unwrap();
        let fake = Arc::new(Fake::new(case));
        let resp = app(fake.clone()).oneshot(Request::get(path).body(Body::empty()).unwrap()).await.unwrap();
        check_reply(resp, case, path).await;
        assert!(fake.calls.lock().unwrap().is_empty(), "{path}: calls Python made that Rust did not");
        assert_eq!(Value::Array(fake.sets.lock().unwrap().clone()), case["redis_sets"], "{path}: redis writes");
        assert_eq!(Value::Array(fake.archive_keys.lock().unwrap().clone()), case["archive_keys"], "{path}: archive");
    }
}

#[tokio::test]
async fn board_icons_match_python() {
    let dir = std::env::temp_dir().join(format!("kiwi-lbread-icons-{}", std::process::id()));
    let sha = format!("ab{}", "0".repeat(62));
    std::fs::create_dir_all(dir.join("objects").join("ab")).unwrap();
    let blob: Vec<u8> = (0..3).flat_map(|_| 0..=255u8).collect();
    std::fs::write(dir.join("objects").join("ab").join(&sha), blob).unwrap();
    let root = dir.to_str().unwrap();
    for case in fixtures()["icons"].as_array().unwrap() {
        let path = case["path"].as_str().unwrap();
        let uri: Uri = path.parse().unwrap();
        let calls = case["calls"].as_array().unwrap().clone();
        let meta = |p: String| async move {
            let call = calls.first().unwrap_or_else(|| panic!("{path}: unexpected meta lookup"));
            assert_eq!(call[1], json!([ICON_BRANCH, p]), "{path}");
            Ok(call[2].as_str().map(str::to_owned))
        };
        check_reply(out(board_icon(&icon_name_of(&uri), root, meta).await), case, path).await;
    }
    let _ = std::fs::remove_dir_all(&dir);
}

fn pure() -> &'static Value {
    &fixtures()["pure"]
}

#[test]
fn reset_boundaries_match_python() {
    for c in pure()["bounds"].as_array().unwrap() {
        let got =
            core::reset_boundaries_for_kind(c[0].as_str().unwrap(), c[1].as_i64().unwrap(), c[2].as_i64().unwrap());
        assert_eq!(got, ints(&c[3]), "{c}");
    }
}

#[test]
fn reset_zeros_match_python() {
    for c in pure()["zeros"].as_array().unwrap() {
        let pts: Vec<(i64, i64, f64)> = c[1]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| (p["created_at"].as_i64().unwrap(), p["rank"].as_i64().unwrap(), p["score"].as_f64().unwrap()))
            .collect();
        let got: Vec<Value> = core::inject_reset_zeros(&pts, c[0].as_str().unwrap())
            .iter()
            .map(|p| json!({"created_at": p.created_at, "rank": p.rank, "score": p.score, "synthetic": p.synthetic}))
            .collect();
        assert_eq!(pyjson::dumps(&Value::Array(got)), pyjson::dumps(&c[2]), "{c}");
    }
}

#[test]
fn timestamps_match_python() {
    let now = pure()["now"].as_i64().unwrap();
    for c in pure()["stamps"].as_array().unwrap() {
        let got = match core::normalize_timestamp(c[0].as_i64().unwrap(), now, c[1].as_bool().unwrap()) {
            Stamp::Anchor(a) => a,
            Stamp::Invalid => -1,
            Stamp::OutOfRange => panic!("{c}"),
        };
        assert_eq!(got, c[2].as_i64().unwrap(), "{c}");
    }
    assert_eq!(core::normalize_timestamp(core::PY_MAX_TIMESTAMP, now, false), Stamp::Invalid);
    assert_eq!(core::normalize_timestamp(core::PY_MAX_TIMESTAMP + 1, now, false), Stamp::OutOfRange);
}

#[test]
fn mastery_levels_match_python() {
    for c in pure()["levels"].as_array().unwrap() {
        let (l, i, n) = core::level_from_points(c[0].as_i64().unwrap());
        assert_eq!(json!([l, i, n]), c[1], "{c}");
    }
}

#[test]
fn board_health_matches_python() {
    for c in pure()["health"].as_array().unwrap() {
        let items: Vec<HealthItem> = c[0]
            .as_array()
            .unwrap()
            .iter()
            .map(|it| HealthItem {
                score: it["score"].as_f64().unwrap(),
                is_new: it["is_new"].as_bool().unwrap_or(false),
                score_delta: it["score_delta"].as_f64(),
                prev_score: it["prev_score"].as_f64(),
            })
            .collect();
        let h = core::compute_board_health(&items, c[1].as_bool().unwrap());
        let got = json!({
            "leader_share": h.leader_share, "p1_pn_ratio": h.p1_pn_ratio, "gini": h.gini,
            "turnover_rate": h.turnover_rate, "new_entrants": h.new_entrants,
            "median_score_gain": h.median_score_gain, "median_score_gain_pct": h.median_score_gain_pct,
        });
        assert_eq!(pyjson::dumps(&got), pyjson::dumps(&c[2]), "{c}");
    }
}

#[test]
fn latest_per_day_matches_python() {
    for c in pure()["per_day"].as_array().unwrap() {
        let got = core::latest_per_day(&ints(&c[0]), c[1].as_u64().unwrap() as usize);
        assert_eq!(got, ints(&c[2]), "{c}");
    }
}

#[test]
fn uuid_segments() {
    assert_eq!(parse_uuid("1000"), Some(1000));
    assert_eq!(parse_uuid("0004000"), Some(4000));
    assert_eq!(parse_uuid("2147483647"), Some(i32::MAX));
    for seg in ["2147483648", "", "-1", "+1", " 1", "1.0", "%31", "cheaters", "١"] {
        assert_eq!(parse_uuid(seg), None, "{seg:?}");
    }
    assert!(icon_name_ok("icon_paragon_knight") && icon_name_ok("abc\n"));
    assert!(!icon_name_ok("Icon") && !icon_name_ok("a-b") && !icon_name_ok("\n") && !icon_name_ok("a\n\n"));
}

#[test]
fn defer_rules() {
    let get = Method::GET;
    assert!(!defers("/v1/leaderboards/{uuid}", &get, "/v1/leaderboards/1000"));
    assert!(!defers("/v1/leaderboards/{uuid}", &Method::HEAD, "/v1/leaderboards/1000"));
    assert!(!defers("/site/leaderboards/{uuid}/entries", &get, "/site/leaderboards/7/entries"));
    assert!(defers("/v1/leaderboards/{uuid}", &Method::POST, "/v1/leaderboards/insert"));
    assert!(defers("/v1/leaderboards/{uuid}", &Method::POST, "/v1/leaderboards/1000"));
    assert!(defers("/v1/leaderboards/{uuid}", &get, "/v1/leaderboards/cheaters"));
    assert!(defers("/v1/leaderboards/{uuid}/history", &get, "/v1/leaderboards/renames/history"));
    assert!(defers("/site/leaderboards/{uuid}/history", &get, "/site/leaderboards/activity/history"));
    assert!(defers("/v1/leaderboards/{uuid}", &get, "/v1/leaderboards/9999999999"));
    assert!(!defers("/v1/leaderboards/timestamps", &Method::POST, "/v1/leaderboards/timestamps"));
}

/// A stand-in for api-py that answers every request with `py` and keeps its request lines.
async fn upstream() -> (SocketAddr, Arc<Mutex<Vec<String>>>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            let mut buf = vec![0u8; 8192];
            let n = sock.read(&mut buf).await.unwrap_or(0);
            let head = String::from_utf8_lossy(&buf[..n]);
            log.lock().unwrap().push(head.lines().next().unwrap_or("").to_owned());
            let _ = sock
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nx-upstream: py\r\nconnection: close\r\n\r\npy")
                .await;
        }
    });
    (addr, seen)
}

#[tokio::test]
async fn deferred_requests_reach_python_untouched() {
    use hyper_util::{client::legacy::Client, rt::TokioExecutor};
    let (addr, seen) = upstream().await;
    let settings = crate::config::Settings::from_vars([("PY_UPSTREAM", format!("http://{addr}"))]).unwrap();
    let upstream = Client::builder(TokioExecutor::new()).build_http();
    let db = crate::db::Db { mongo: None, redis: None, pg: None };
    let state = crate::state::AppState { inner: Arc::new(crate::state::Inner { settings, db, upstream }) };
    let app = crate::routes::app(state);
    let peer: SocketAddr = "203.0.113.9:5000".parse().unwrap();
    let send = |method: Method, path: &'static str| {
        let app = app.clone();
        async move {
            let req =
                Request::builder().method(method).uri(path).extension(ConnectInfo(peer)).body(Body::empty()).unwrap();
            app.oneshot(req).await.unwrap()
        }
    };
    for (method, path) in [
        (Method::POST, "/v1/leaderboards/insert"),
        (Method::POST, "/v1/leaderboards/1000"),
        (Method::GET, "/v1/leaderboards/cheaters"),
        (Method::HEAD, "/v1/leaderboards/renames"),
        (Method::GET, "/v1/leaderboards/%31"),
        (Method::GET, "/v1/leaderboards/2147483648"),
        (Method::GET, "/v1/leaderboards/renames/history"),
        (Method::GET, "/v1/leaderboards/players/Alpha/history"),
        (Method::GET, "/v1/leaderboards/"),
        (Method::GET, "/v1/leaderboards/1000/"),
        (Method::GET, "/site/leaderboards/activity/history"),
        (Method::GET, "/site/leaderboards/renames/health"),
        (Method::GET, "/site/leaderboards/players/Alpha/profile"),
    ] {
        let resp = send(method.clone(), path).await;
        assert_eq!(resp.headers().get("x-upstream").map(|v| v.to_str().unwrap()), Some("py"), "{method} {path}");
        assert!(resp.headers().get("x-content-type-options").is_none(), "{method} {path} went through the stack");
        let line = seen.lock().unwrap().last().cloned().unwrap();
        assert_eq!(line, format!("{method} {path} HTTP/1.1"));
    }
    let before = seen.lock().unwrap().len();
    for (path, status) in [
        ("/v1/leaderboards/1000", StatusCode::UNAUTHORIZED),
        ("/v1/leaderboards/0004000/entries", StatusCode::UNAUTHORIZED),
        ("/v1/leaderboards/timestamps", StatusCode::UNAUTHORIZED),
        ("/v1/leaderboards", StatusCode::UNAUTHORIZED),
        ("/site/leaderboards/1000/entries", StatusCode::UNPROCESSABLE_ENTITY),
        ("/site/leaderboards/timestamps?limit=0", StatusCode::UNPROCESSABLE_ENTITY),
    ] {
        let resp = send(Method::GET, path).await;
        assert_eq!(resp.status(), status, "{path}");
        assert_eq!(resp.headers()["x-content-type-options"], "nosniff", "{path}");
    }
    assert_eq!(seen.lock().unwrap().len(), before, "Rust-owned requests reached Python");
}

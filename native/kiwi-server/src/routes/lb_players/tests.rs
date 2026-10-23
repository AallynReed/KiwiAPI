//! Against `testdata/lb_players/golden.json` (native/port/parity/gen_lb_players_fixtures.py):
//! every route body byte for byte with the store answering exactly what it
//! answered Python (an unexpected call fails), plus the pure helpers.

use std::{collections::HashMap, path::Path, sync::OnceLock};

use axum::http::{HeaderMap, Uri};
use kiwi_core::lbplayers::{self as core, Point};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use super::{
    DaysQuery, SITE, SiteHistoryQuery, V1, V1HistoryQuery, player_name,
    store::{
        AggRow, BoardMeta, DuplicateRow, EntryRow, RenameEdge, Store, duplicate_evidence, duplicate_json, py_lower,
        rename_evidence,
    },
};
use crate::{
    error::{ApiError, ApiResult},
    gamedata::Gamedata,
    middleware::pyurl::scope_path,
    pyjson, request_id,
    validation::{FromParams, Params},
};

fn golden() -> &'static Value {
    static G: OnceLock<Value> = OnceLock::new();
    G.get_or_init(|| pyjson::loads(include_bytes!("../../../testdata/lb_players/golden.json")).unwrap())
}

fn core_cases(key: &str) -> &'static [Value] {
    golden()["core"][key].as_array().unwrap()
}

fn baseline() -> &'static Gamedata {
    static G: OnceLock<Gamedata> = OnceLock::new();
    G.get_or_init(|| Gamedata::new(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/gamedata"), None))
}

fn i(v: &Value) -> i64 {
    v.as_i64().unwrap_or_else(|| panic!("not an int: {v}"))
}

fn f(v: &Value) -> f64 {
    assert!(v.is_f64(), "not a float: {v}");
    v.as_f64().unwrap()
}

fn s(v: &Value) -> String {
    v.as_str().unwrap_or_else(|| panic!("not a str: {v}")).to_owned()
}

fn points(v: &Value) -> Vec<Point> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|p| Point { created_at: i(&p["created_at"]), rank: i(&p["rank"]), score: f(&p["score"]) })
        .collect()
}

#[test]
fn trove_days() {
    for c in core_cases("trove_day_start") {
        assert_eq!(core::trove_day_start(i(&c[0])), i(&c[1]), "{c}");
    }
    for c in core_cases("cold_keep_from") {
        assert_eq!(core::cold_keep_from(i(&c[0]), i(&c[1])), i(&c[2]), "{c}");
    }
}

#[test]
fn reset_boundaries() {
    for c in core_cases("reset_boundaries") {
        let got = core::reset_boundaries_for_kind(c[0].as_str().unwrap(), i(&c[1]), i(&c[2]));
        let want: Vec<i64> = c[3].as_array().unwrap().iter().map(i).collect();
        assert_eq!(got, want, "{c}");
    }
}

#[test]
fn reset_cliffs() {
    for c in core_cases("inject") {
        let got: Vec<Value> = core::inject_reset_zeros(&points(&c["points"]), c["kind"].as_str().unwrap())
            .into_iter()
            .map(|(p, syn)| json!({"created_at": p.created_at, "rank": p.rank, "score": p.score, "synthetic": syn}))
            .collect();
        assert_eq!(pyjson::dumps(&Value::Array(got)), pyjson::dumps(&c["out"]), "{c}");
    }
}

#[test]
fn split_series() {
    for c in core_cases("split") {
        let got: Vec<Value> = core::split_series(&points(&c["points"]))
            .into_iter()
            .map(|sp| json!({"slot": sp.slot, "idx": sp.points}))
            .collect();
        assert_eq!(Value::Array(got), c["out"], "{}", c["points"]);
    }
}

#[test]
fn mastery_levels() {
    for c in core_cases("level") {
        let (lvl, into, next) = core::level_from_points(c[0].as_i64().unwrap().into());
        assert_eq!(json!([lvl, into as i64, next as i64]), c[1], "{c}");
    }
    for c in core_cases("level_score") {
        assert_eq!(core::level_from_score(c[0].as_f64().unwrap()), Some(i(&c[1])), "{c}");
    }
}

#[test]
fn excluded_board_csv() {
    for c in core_cases("csv") {
        let got: Vec<i64> = core::parse_board_csv(c[0].as_str().unwrap()).into_iter().map(|v| v as i64).collect();
        assert_eq!(json!(got), c[1], "{c}");
    }
}

#[test]
fn jsonb_columns() {
    for c in core_cases("rename_evidence") {
        let got = rename_evidence(c[0].as_str().unwrap()).unwrap();
        assert_eq!(pyjson::dumps(&got), pyjson::dumps(&c[1]), "{c}");
    }
    for c in core_cases("duplicate_json") {
        let text = c[0].as_str().unwrap();
        assert_eq!(pyjson::dumps(&duplicate_json(text, json!([])).unwrap()), pyjson::dumps(&c[1]), "{c}");
        assert_eq!(pyjson::dumps(&duplicate_evidence(text).unwrap()), pyjson::dumps(&c[2]), "{c}");
    }
}

#[test]
fn python_lower() {
    for c in core_cases("lower") {
        assert_eq!(py_lower(kiwi_core::pystr::strip(c[0].as_str().unwrap())), s(&c[1]), "{c}");
    }
}

#[test]
fn reset_kinds() {
    for c in core_cases("effective_reset_kind") {
        assert_eq!(core::effective_reset_kind(c[0].as_str(), i(&c[1])), s(&c[2]), "{c}");
    }
}

// --- routes ------------------------------------------------------------------

/// The store answering from one case's recorded calls.
struct Fake {
    case: String,
    settings: Map<String, Value>,
    calls: Vec<&'static Value>,
}

impl Fake {
    fn new(case: &'static Value) -> Self {
        let answers = golden()["answers"].as_array().unwrap();
        Fake {
            case: s(&case["name"]),
            settings: case["settings"].as_object().unwrap().clone(),
            calls: case["calls"].as_array().unwrap().iter().map(|n| &answers[n.as_u64().unwrap() as usize]).collect(),
        }
    }

    fn answer(&self, func: &str, args: Value) -> Result<&'static Value, ()> {
        match self.calls.iter().find(|c| c["fn"] == func && c["args"] == args) {
            Some(c) if c.get("raise").is_some() => Err(()),
            Some(c) => Ok(&c["result"]),
            None => panic!("{}: call Python did not make: {func} {args}", self.case),
        }
    }

    fn api(&self, func: &str, args: Value) -> ApiResult<&'static Value> {
        self.answer(func, args).map_err(|()| ApiError::internal())
    }
}

fn entry_rows(v: &Value) -> Vec<EntryRow> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|r| EntryRow {
            player_name: s(&r["player_name"]),
            rank: i(&r["rank"]),
            score: f(&r["score"]),
            leaderboard: i(&r["leaderboard"]),
            created_at: i(&r["created_at"]),
        })
        .collect()
}

impl Store for Fake {
    fn now(&self) -> f64 {
        golden()["now"].as_f64().unwrap()
    }

    fn default_tier_days(&self) -> i64 {
        i(&golden()["tier_default"])
    }

    async fn setting(&self, key: &str) -> anyhow::Result<Value> {
        match self.settings.get(key) {
            Some(v) => Ok(v.clone()),
            None => panic!("{}: setting Python did not read: {key}", self.case),
        }
    }

    async fn player_rows(
        &self,
        name: &str,
        limit: i64,
        uuid: Option<i128>,
        window_start: Option<i64>,
    ) -> ApiResult<Vec<EntryRow>> {
        let args = json!([name, limit, uuid.map(|u| u as i64), window_start]);
        Ok(entry_rows(self.api("player_rows", args)?))
    }

    async fn player_rows_window(&self, name: &str, window_start: i64) -> ApiResult<Vec<EntryRow>> {
        Ok(entry_rows(self.api("player_rows_window", json!([name, window_start]))?))
    }

    async fn player_canonical_name(&self, name: &str) -> ApiResult<Option<String>> {
        Ok(self.api("player_canonical_name", json!([name]))?.as_str().map(str::to_owned))
    }

    async fn player_board_summary(&self, name: &str) -> ApiResult<Vec<AggRow>> {
        let rows = self.api("player_board_summary", json!([name]))?;
        Ok(rows
            .as_array()
            .unwrap()
            .iter()
            .map(|r| AggRow {
                leaderboard: i(&r["leaderboard"]),
                best_rank: i(&r["best_rank"]),
                appearances: i(&r["appearances"]),
                first_seen: i(&r["first_seen"]),
                last_seen: i(&r["last_seen"]),
                latest_rank: i(&r["latest_rank"]),
                latest_score: f(&r["latest_score"]),
            })
            .collect())
    }

    async fn player_last_played(&self, name: &str, excluded: &[i128]) -> ApiResult<Option<i64>> {
        let excluded: Vec<i64> = excluded.iter().map(|&v| v as i64).collect();
        Ok(self.api("player_last_played", json!([name, excluded]))?.as_i64())
    }

    async fn board_meta(&self, uuids: &[i64]) -> ApiResult<HashMap<i64, BoardMeta>> {
        let pairs = self.api("board_meta", json!([uuids]))?;
        Ok(pairs
            .as_array()
            .unwrap()
            .iter()
            .map(|p| {
                let m = &p[1];
                let meta = BoardMeta {
                    name: s(&m["name"]),
                    name_id: s(&m["name_id"]),
                    category: s(&m["category"]),
                    reset_kind: s(&m["reset_kind"]),
                };
                (i(&p[0]), meta)
            })
            .collect())
    }

    async fn board_kinds(&self, uuids: &[i64]) -> ApiResult<HashMap<i64, String>> {
        let pairs = self.api("board_kinds", json!([uuids]))?;
        Ok(pairs.as_array().unwrap().iter().map(|p| (i(&p[0]), s(&p[1]))).collect())
    }

    async fn renames_for_name(&self, name: &str) -> ApiResult<Vec<RenameEdge>> {
        let rows = self.api("renames_for_name", json!([name]))?;
        Ok(rows
            .as_array()
            .unwrap()
            .iter()
            .map(|e| RenameEdge {
                id: i(&e["id"]),
                from_name: s(&e["from_name"]),
                to_name: s(&e["to_name"]),
                from_anchor: i(&e["from_anchor"]),
                to_anchor: i(&e["to_anchor"]),
                confidence: f(&e["confidence"]),
                matched_boards: i(&e["matched_boards"]),
                evidence: e["evidence"].clone(),
                method_version: i(&e["method_version"]),
                created_at: i(&e["created_at"]),
            })
            .collect())
    }

    async fn duplicate_for_name(&self, name: &str) -> ApiResult<Option<DuplicateRow>> {
        let r = self.api("duplicate_for_name", json!([name]))?;
        if r.is_null() {
            return Ok(None);
        }
        Ok(Some(DuplicateRow {
            name: s(&r["name"]),
            kind: s(&r["kind"]),
            verdict: s(&r["verdict"]),
            boards: i(&r["boards"]),
            max_occurrences: i(&r["max_occurrences"]),
            spellings: r["spellings"].clone(),
            first_anchor: i(&r["first_anchor"]),
            last_anchor: i(&r["last_anchor"]),
            evidence: r["evidence"].clone(),
            method_version: i(&r["method_version"]),
            updated_at: i(&r["updated_at"]),
        }))
    }

    async fn latest_anchor(&self) -> ApiResult<Option<i64>> {
        Ok(self.api("list_timestamps", json!([1, null]))?.as_array().unwrap().first().map(i))
    }

    async fn claim_verified(&self, claimed_lower: &str) -> bool {
        self.answer("site_user_claim", json!([claimed_lower])).is_ok_and(|v| v.as_bool().unwrap())
    }

    async fn ready_anchor(&self) -> Option<i64> {
        self.answer("get_ready_anchor", json!([])).ok()?.as_i64()
    }

    async fn cheaters_payload(&self, anchor: i64) -> Option<Value> {
        let v = self.answer("get_cheaters", json!([anchor])).ok()?;
        (!v.is_null()).then(|| v.clone())
    }
}

fn query<T: FromParams>(uri: &Uri) -> ApiResult<T> {
    let headers = HeaderMap::new();
    let mut p = Params::new(Vec::new(), uri.query(), &headers);
    let value = T::from_params(&mut p);
    Ok(p.v.finish(value)?)
}

/// What the route answers once its dependencies passed, and its Cache-Control.
async fn route(fake: &Fake, uri: &Uri) -> (ApiResult<String>, Option<&'static str>) {
    let path = scope_path(uri);
    let gd = baseline();
    let (prefix, cache) = if path.starts_with(V1) { (V1, None) } else { (SITE, Some("public, max-age=30")) };
    let suffix = &path[path.rfind('/').unwrap()..];
    let name = player_name(uri, prefix, suffix);
    let body = match (prefix == V1, suffix) {
        (true, "/history") => match query::<V1HistoryQuery>(uri) {
            Ok(q) => super::v1_history_body(fake, &name, &q).await,
            Err(e) => Err(e),
        },
        (true, "/profile") => super::v1_profile_body(fake, gd, &name).await,
        (true, "/boards") => super::v1_boards_body(fake, &name).await,
        (true, "/series") => match query::<DaysQuery>(uri) {
            Ok(q) => {
                let days = super::v1_series_days(fake, &q).await;
                super::v1_series_body(fake, &name, days).await
            }
            Err(e) => Err(e),
        },
        (false, "/history") => match query::<SiteHistoryQuery>(uri) {
            Ok(q) => super::site_history_body(fake, &name, &q).await,
            Err(e) => Err(e),
        },
        (false, "/profile") => super::site_profile_body(fake, gd, &name).await,
        (false, "/series") => match query::<DaysQuery>(uri) {
            Ok(q) => super::site_series_body(fake, &name, &q).await,
            Err(e) => Err(e),
        },
        other => panic!("no route for {other:?}"),
    };
    (body, cache)
}

fn sha(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

#[tokio::test]
async fn routes_match_python() {
    let cases = golden()["cases"].as_array().unwrap();
    assert!(cases.len() > 80);
    let mut failures = Vec::new();
    for case in cases {
        let name = s(&case["name"]);
        let uri: Uri = case["path"].as_str().unwrap().parse().unwrap();
        let fake = Fake::new(case);
        let rid = s(&golden()["rid"]);
        let (status, body, cache) = request_id::scope(rid, async {
            let (res, cache) = route(&fake, &uri).await;
            match res {
                Ok(b) => (200, b, cache),
                Err(e) => (e.status.as_u16(), e.body(), None),
            }
        })
        .await;
        assert_eq!(u64::from(status), case["status"].as_u64().unwrap(), "{name}: status ({body})");
        assert_eq!(case["content_type"], "application/json", "{name}");
        if status == 200 {
            assert_eq!(case["cache_control"].as_str(), cache, "{name}: cache-control");
        }
        let same = match case["body_sha256"].as_str() {
            Some(h) => sha(body.as_bytes()) == h,
            None => body == s(&case["body"]),
        };
        if !same {
            let dir = std::env::temp_dir().join("kiwi_lb_players");
            let _ = std::fs::create_dir_all(&dir);
            let _ = std::fs::write(dir.join(format!("{name}.json")), &body);
            failures.push(name);
        }
    }
    assert!(failures.is_empty(), "bodies differ (ours written to the temp dir's kiwi_lb_players): {failures:?}");
}

#[test]
fn player_name_is_the_decoded_segment() {
    let uri: Uri = "/v1/leaderboards/players/%C3%9Cn%20x%3F/profile?y=1".parse().unwrap();
    assert_eq!(player_name(&uri, V1, "/profile"), "Ün x?");
    let uri: Uri = "/site/leaderboards/players/%FFa/series".parse().unwrap();
    assert_eq!(player_name(&uri, SITE, "/series"), "\u{fffd}a");
}

// --- routing -------------------------------------------------------------------

fn bare_state() -> crate::state::AppState {
    use crate::{config::Settings, db::Db, state::Inner};
    use hyper_util::{client::legacy::Client, rt::TokioExecutor};
    let settings = Settings::from_vars([("PY_UPSTREAM", "http://py:8000")]).unwrap();
    let upstream = Client::builder(TokioExecutor::new()).build_http();
    crate::state::AppState {
        inner: std::sync::Arc::new(Inner { settings, db: Db { mongo: None, redis: None, pg: None }, upstream }),
    }
}

async fn send(method: &str, uri: &str) -> (u16, Value) {
    use axum::body::Body;
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    let app = super::router().fallback(|| async { axum::http::StatusCode::IM_A_TEAPOT }).with_state(bare_state());
    let req = axum::http::Request::builder().method(method).uri(uri).body(Body::empty()).unwrap();
    let res = app.oneshot(req).await.unwrap();
    let status = res.status().as_u16();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

#[tokio::test]
async fn routing() {
    // Gate (flag defaults on), then the scope, before any query validation.
    let (status, body) = send("GET", "/v1/leaderboards/players/x/history?limit=0").await;
    assert_eq!((status, body["error"]["message"].as_str()), (401, Some("API token required")));
    let (status, body) = send("GET", "/v1/leaderboards/players/x/series?days=0").await;
    assert_eq!((status, body["error"]["code"].as_str()), (401, Some("not_authenticated")));
    let (status, body) = send("GET", "/site/leaderboards/players/x/history?limit=0&uuid=q").await;
    assert_eq!(status, 422);
    assert_eq!(body["error"]["details"][0]["loc"], json!(["query", "limit"]));
    assert_eq!(body["error"]["details"][1]["loc"], json!(["query", "uuid"]));
    assert_eq!(send("GET", "/site/leaderboards/players/x/series?days=31").await.0, 422);
    assert_eq!(send("POST", "/site/leaderboards/players/x/profile").await.0, 405);
    // Shapes Starlette doesn't route here go to Python untouched (an empty
    // segment does in `routes::app`, before this router).
    for uri in [
        "/v1/leaderboards/players/x/history/",
        "/v1/leaderboards/players/x/y/history",
        "/site/leaderboards/players/x/boards",
        "/site/leaderboards/players/history",
    ] {
        assert_eq!(send("GET", uri).await.0, 418, "{uri}");
    }
}

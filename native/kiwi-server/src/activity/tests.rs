//! Replays `testdata/activity/fixtures.json`, generated from the real Python
//! routes and compute by `native/port/parity/gen_activity_fixtures.py`.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::OnceLock,
};

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use kiwi_core::{
    activity::{Capture, ResetWindow, class_sets, pair},
    activity_calc::{
        self as calc, ACTIVE_RETENTION, CLASS_ACTIVE_RETENTION, Chain, ClassSets, DAY, ResetKind, RollupSource,
        Thresholds,
    },
};
use serde_json::{Value, json};
use tower::ServiceExt;

use super::{
    cache, class, models,
    player::{self, Window},
    routes::{BackfillQuery, ClassBackfillQuery, HistoryQuery, SiteHistoryQuery, backfill_body, site_blocked},
    store::{self, Board, ClassEstimate, ClassRollup, Estimate},
};
use crate::{
    auth::pyutil::int_of,
    error,
    pyjson::{self, PyJson, PydanticJson},
    validation::Query,
};

fn fixtures() -> &'static Value {
    static F: OnceLock<Value> = OnceLock::new();
    F.get_or_init(|| serde_json::from_str(include_str!("../../testdata/activity/fixtures.json")).unwrap())
}

fn arr(v: &Value) -> &Vec<Value> {
    v.as_array().unwrap()
}

fn i(v: &Value) -> i64 {
    v.as_i64().unwrap()
}

fn s(v: &Value) -> &str {
    v.as_str().unwrap()
}

fn now() -> i64 {
    i(&fixtures()["now"])
}

fn labels() -> Vec<(Value, Value)> {
    arr(&fixtures()["class_labels"]).iter().map(|l| (l[0].clone(), l[1].clone())).collect()
}

fn estimate(r: &Value) -> Estimate {
    Estimate {
        window_end: i(&r["window_end"]),
        window_start: i(&r["window_start"]),
        duration_hours: r["duration_hours"].as_f64().unwrap(),
        estimate: i(&r["estimate"]) as i32,
        boards_analyzed: i(&r["boards_analyzed"]) as i32,
        computed_at: i(&r["computed_at"]),
        estimate_24h: r["estimate_24h"].as_i64().map(|n| n as i32),
    }
}

fn class_estimate(r: &Value) -> ClassEstimate {
    let opt = |k: &str| r[k].as_i64().map(|n| n as i32);
    ClassEstimate {
        class_index: i(&r["class_index"]) as i32,
        window_end: i(&r["window_end"]),
        window_start: i(&r["window_start"]),
        duration_hours: r["duration_hours"].as_f64().unwrap(),
        estimate: i(&r["estimate"]) as i32,
        estimate_clean: opt("estimate_clean"),
        computed_at: i(&r["computed_at"]),
        estimate_24h: opt("estimate_24h"),
        estimate_24h_clean: opt("estimate_24h_clean"),
    }
}

/// Assert one recorded `{status, cache_control, body}` reply.
fn same_reply(want: &Value, status: u16, cc: Option<&str>, body: &str, label: &str) {
    assert_eq!(status as u64, want["status"].as_u64().unwrap(), "{label} status");
    assert_eq!(cc, want["cache_control"].as_str(), "{label} cache-control");
    assert_eq!(body, s(&want["body"]), "{label} body");
}

/// A `/v1` read and its `/site` twin over the same payload.
fn same_reads(payload: &Value, v1: &Value, site: &Value, model: fn(&Value) -> Option<Value>, label: &str) {
    let body = pyjson::pydantic(&model(payload).unwrap_or_else(|| panic!("{label}: model rejected")));
    same_reply(v1, 200, Some("public, max-age=1800"), &body, &format!("{label} v1"));
    same_reply(site, 200, Some("no-cache"), &pyjson::dumps(payload), &format!("{label} site"));
}

fn same_redis(writes: &Value, key: &str, payload: &Value, label: &str) {
    let w = &arr(writes)[0];
    assert_eq!(s(&w[0]), key, "{label} redis key");
    assert_eq!(s(&w[1]), pyjson::dumps_spaced(payload), "{label} redis value");
    assert_eq!(i(&w[2]), cache::SERIES_TTL, "{label} redis ttl");
}

async fn send(router: Router, method: &str, uri: &str) -> (u16, Option<String>, String) {
    let req = Request::builder().method(method).uri(uri).body(Body::empty()).unwrap();
    let resp = router.oneshot(req).await.unwrap();
    let status = resp.status().as_u16();
    let cc = resp.headers().get("cache-control").map(|v| v.to_str().unwrap().to_owned());
    let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    (status, cc, String::from_utf8(body.to_vec()).unwrap())
}

#[test]
fn series_matches_python() {
    for c in arr(&fixtures()["series"]) {
        let period = player::normalize_period(s(&c["period"]), &["7d", "1m"]);
        let (_, days, bucket) = player::PERIODS.iter().find(|p| p.0 == period).copied().unwrap();
        let ws = now() - days * 86_400;
        let rows: Vec<Estimate> = arr(&c["rows"]).iter().map(estimate).filter(|r| r.window_end >= ws).collect();
        let payload = player::series_payload(&period, bucket, ws, now(), &rows);
        let label = format!("series {}", c["period"]);
        same_redis(&c["redis"], &cache::series_key(&period), &payload, &label);
        same_reads(&payload, &c["v1"], &c["site"], models::series, &label);
    }
}

fn history_router(v1: bool, rows: Vec<Estimate>) -> Router {
    let reply = move |days: i64, rows: &[Estimate]| {
        let days = days.clamp(1, 30);
        let picked: Vec<Estimate> = rows.iter().filter(|r| r.window_end >= now() - days * 86_400).cloned().collect();
        player::history_payload(days, now(), &picked)
    };
    if v1 {
        Router::new().route(
            "/h",
            get(move |Query(q): Query<HistoryQuery>| async move {
                let p = reply(q.days, &rows);
                ([("cache-control", "public, max-age=1800")], PydanticJson(models::history(&p).unwrap()))
                    .into_response()
            }),
        )
    } else {
        Router::new().route(
            "/h",
            get(move |Query(q): Query<SiteHistoryQuery>| async move {
                ([("cache-control", "no-cache")], PyJson(reply(q.days, &rows))).into_response()
            }),
        )
    }
}

#[tokio::test]
async fn history_matches_python() {
    let h = &arr(&fixtures()["history"])[0];
    let rows: Vec<Estimate> = arr(&h["rows"]).iter().map(estimate).collect();
    for c in arr(&h["cases"]) {
        let q = s(&c["query"]);
        for (v1, key) in [(true, "v1"), (false, "site")] {
            let (status, cc, body) = send(history_router(v1, rows.clone()), "GET", &format!("/h{q}")).await;
            same_reply(&c[key], status, cc.as_deref(), &body, &format!("history {key} {q}"));
        }
    }
}

#[test]
fn current_matches_python() {
    for c in arr(&fixtures()["current"]) {
        let payload = match c["redis"].as_str() {
            Some(raw) => pyjson::loads(raw.as_bytes()).unwrap(),
            None => player::empty(now()),
        };
        same_reads(&payload, &c["v1"], &c["site"], models::activity, "current");
    }
}

/// Fixture score maps as captures, players numbered by first sight.
struct Ids(HashMap<String, i64>);

impl Ids {
    fn id(&mut self, name: &str) -> i64 {
        let next = self.0.len() as i64 + 1;
        *self.0.entry(name.to_owned()).or_insert(next)
    }

    fn capture(&mut self, maps: &Value, boards: &[i32]) -> Capture {
        let mut cap = Capture::default();
        for (u, m) in maps.as_object().unwrap() {
            let u: i32 = u.parse().unwrap();
            let m = m.as_object().unwrap();
            if !boards.contains(&u) || m.is_empty() {
                continue;
            }
            let board = m.iter().map(|(n, v)| (self.id(n), v.as_f64().unwrap())).collect();
            cap.boards.insert(u, board);
        }
        cap
    }

    fn names(&self, ids: &[i64]) -> HashSet<String> {
        let back: HashMap<i64, &String> = self.0.iter().map(|(n, i)| (*i, n)).collect();
        ids.iter().map(|i| back[i].clone()).collect()
    }
}

#[test]
fn compute_matches_python() {
    for c in arr(&fixtures()["compute"]) {
        let (late, early) = (i(&c["late"]), i(&c["early"]));
        let boards: Vec<Board> = arr(&c["boards"])
            .iter()
            .filter(|b| c["late_maps"].get(i(&b["uuid"]).to_string()).is_some())
            .map(|b| Board {
                uuid: i(&b["uuid"]) as i32,
                name: s(&b["name"]).into(),
                category: s(&b["category"]).into(),
            })
            .collect();
        let player_boards: Vec<&Board> = boards.iter().filter(|b| calc::is_player_board(b.uuid)).collect();
        let uuids: Vec<i32> = player_boards.iter().map(|b| b.uuid).collect();
        let mut ids = Ids(HashMap::new());
        let late_cap = ids.capture(&c["late_maps"], &uuids);
        let early_cap = ids.capture(&c["early_maps"], &uuids);
        let p = pair(&early_cap, &late_cap, &uuids);
        let names = ids.names(&p.active);
        let mut stamps_desc: Vec<i64> =
            arr(&c["stamps"]).iter().map(i).filter(|&t| t >= late - ACTIVE_RETENTION).collect();
        stamps_desc.sort_unstable_by(|a, b| b.cmp(a));
        let want = &c["payload"];
        let w = Window {
            late,
            early,
            boards: &player_boards,
            per_board: &p.per_board,
            estimate: names.len() as i64,
            early_24h: calc::pick_early_anchor(&stamps_desc, late, DAY),
            early_7d: calc::pick_early_anchor(&stamps_desc, late, 7 * DAY),
            estimate_24h: want["estimate_24h"].as_i64(),
            estimate_7d: want["estimate_7d"].as_i64(),
            now: now(),
        };
        assert_eq!(pyjson::dumps(&w.payload()), pyjson::dumps(want), "compute {late}");
        let writes = arr(&c["writes"]);
        let lowered = store::lowered(names.iter().map(String::as_str));
        assert_eq!(json!(["record_active_window", late, lowered]), writes[0]);
        assert_eq!(json!(["prune_active_windows", late - ACTIVE_RETENTION]), writes[1]);
        let r = w.row();
        let row = json!([
            "upsert_estimate",
            r.window_end,
            r.window_start,
            r.duration_hours,
            r.estimate,
            r.boards_analyzed,
            r.computed_at,
            r.estimate_24h
        ]);
        assert_eq!(pyjson::dumps(&row), pyjson::dumps(&writes[2]), "compute row {late}");
    }
}

fn thresholds(settings: &Value) -> Thresholds {
    let get = |k: &str, d: i64| int_of(&settings[k]).map(|v| v as i64).unwrap_or(d);
    Thresholds {
        power_rank: get("class_activity_power_rank_threshold", 25_000),
        effort: get("class_activity_effort_threshold", 50),
        xp: get("class_activity_xp_threshold", 2_000_000),
    }
}

#[test]
fn class_series_matches_python() {
    for c in arr(&fixtures()["class_series"]) {
        let known: Vec<&str> = class::PERIODS.iter().map(|p| p.0).collect();
        let period = player::normalize_period(s(&c["period"]), &known);
        let (_, days, bucket) = class::PERIODS.iter().find(|p| p.0 == period).copied().unwrap();
        let all: Vec<ClassEstimate> = arr(&c["rows"]).iter().map(class_estimate).collect();
        let (ws, rows) = match days {
            Some(d) => {
                let ws = now() - d * 86_400;
                (ws, all.into_iter().filter(|r| r.window_end >= ws).collect::<Vec<_>>())
            }
            None => (all.first().map_or(now(), |r| r.window_end), all),
        };
        let t = thresholds(&c["settings"]);
        let payload = class::series_payload(&period, bucket, ws, now(), &rows, &t, &labels());
        let label = format!("class series {}", c["period"]);
        same_redis(&c["redis"], &cache::class_series_key(&period), &payload, &label);
        same_reads(&payload, &c["v1"], &c["site"], models::class_series, &label);
    }
}

#[test]
fn donut_matches_python() {
    for c in arr(&fixtures()["donut"]) {
        let anchor = i(&c["anchor"]);
        let t = thresholds(&c["settings"]);
        let maps = &c["maps"];
        let mut ids = Ids(HashMap::new());
        let effort_boards = calc::effort_board_uuids();
        let effort = ids.capture(&maps[anchor.to_string()], &effort_boards);
        let gates = ids.capture(&maps[anchor.to_string()], &t.gate_uuids());
        let stamps: Vec<i64> = arr(&c["stamps"]).iter().map(i).collect();
        let payload = if calc::snapshot_counts(&effort, &gates, &t).is_empty() {
            class::empty(now())
        } else {
            let early = class::donut_early(&stamps, anchor).map(|e| ids.capture(&maps[e.to_string()], &effort_boards));
            class::donut_payload(anchor, &effort, &gates, early.as_ref(), &t, now(), &labels()).unwrap()
        };
        let label = format!("donut {anchor}");
        if !arr(&c["redis"]).is_empty() {
            same_redis(&c["redis"], cache::CLASS_CURRENT_KEY, &payload, &label);
        }
        same_reads(&payload, &c["v1"], &c["site"], models::class_current, &label);
    }
}

#[test]
fn class_compute_matches_python() {
    for c in arr(&fixtures()["class_compute"]) {
        let (late, early) = (i(&c["late"]), i(&c["early"]));
        let t = thresholds(&c["settings"]);
        let mut ids = Ids(HashMap::new());
        let boards = calc::effort_board_uuids();
        let maps = &c["maps"];
        let late_cap = ids.capture(&maps[late.to_string()], &boards);
        let early_cap = ids.capture(&maps[early.to_string()], &boards);
        let gates = ids.capture(&maps[late.to_string()], &t.gate_uuids());
        let crossed = calc::reset_crossed(ResetKind::Weekly, early, late);
        let short = crossed && late - early <= calc::SHORT_RESET_WINDOW;
        let mut sets = ClassSets::new();
        for (k, a, cl) in class_sets(&early_cap, &late_cap, &gates, &ResetWindow { crossed, short }, &t.floors()) {
            sets.insert(k, (ids.names(&a), cl.map(|cl| ids.names(&cl))));
        }
        let counts = class::counts_of(&sets);
        let writes = arr(&c["writes"]);
        let label = format!("class compute {late}");
        let rows_json = |rows: &[ClassEstimate]| {
            json!(
                rows.iter()
                    .map(|r| json!([
                        r.class_index,
                        r.window_end,
                        r.window_start,
                        r.duration_hours,
                        r.estimate,
                        r.estimate_clean,
                        r.computed_at,
                        r.estimate_24h,
                        r.estimate_24h_clean
                    ]))
                    .collect::<Vec<_>>()
            )
        };
        if counts.is_empty() {
            assert!(writes.is_empty(), "{label}");
        } else {
            let active: Vec<Value> =
                class::active_rows(&sets).into_iter().map(|(k, n, cl)| json!([k, n, cl])).collect();
            assert_eq!(writes[0], json!(["record_class_active_window", late, active]), "{label}");
            assert_eq!(writes[1], json!(["prune_class_active_windows", late - CLASS_ACTIVE_RETENTION]), "{label}");
            let first = class::rows(early, late, &counts, &BTreeMap::new(), now());
            assert_eq!(pyjson::dumps(&writes[2][1]), pyjson::dumps(&rows_json(&first)), "{label} first upsert");
            let stamps: Vec<i64> =
                arr(&c["stamps"]).iter().map(i).filter(|&t| t >= now() - CLASS_ACTIVE_RETENTION).collect();
            let span = class::rollup_span(&stamps, late);
            let rollups = match span {
                None => BTreeMap::new(),
                Some((e, n)) => {
                    let call = &writes[3];
                    assert_eq!((i(&call[1]), i(&call[2])), (e, late), "{label} rollup span");
                    let got: BTreeMap<i64, ClassRollup> = call[3]
                        .as_object()
                        .unwrap()
                        .iter()
                        .map(|(k, r)| {
                            let r = ClassRollup {
                                raw: i(&r["raw"]),
                                clean: i(&r["clean"]),
                                windows: i(&r["windows"]),
                                clean_windows: i(&r["clean_windows"]),
                            };
                            (k.parse().unwrap(), r)
                        })
                        .collect();
                    class::pick_rollups(n, &got)
                }
            };
            let second = class::rows(early, late, &counts, &rollups, now());
            let last = writes.last().unwrap();
            assert_eq!(pyjson::dumps(&last[1]), pyjson::dumps(&rows_json(&second)), "{label} second upsert");
        }
        let lab = if counts.is_empty() { Vec::new() } else { labels() };
        let duration = (late - early) as f64 / 3600.0;
        let payload =
            class::build_current(Some(early), Some(late), Some(duration), &counts, now(), &t, &lab, class::METHODOLOGY);
        assert_eq!(pyjson::dumps(&payload), pyjson::dumps(&c["payload"]), "{label} payload");
    }
}

fn chain_pairs<T>(chain: &Chain<T>) -> Value {
    json!(chain.iter().map(|c| [c.0, c.1]).collect::<Vec<_>>())
}

#[test]
fn replay_matches_python() {
    let r = &fixtures()["replay"];
    let stamps: Vec<i64> = arr(&r["stamps"]).iter().map(i).collect();
    let floor = i(&r["active_floor"]);
    let mut chain: Chain<HashSet<String>> = Chain::new();
    for step in arr(&r["player"]) {
        let (early, late) = (i(&step["early"]), i(&step["late"]));
        if step["reset"].as_bool().unwrap() {
            chain.clear();
        }
        let names: HashSet<String> = arr(&step["names"]).iter().map(|n| s(n).to_owned()).collect();
        calc::chain_push(&mut chain, early, late, names);
        let got = match calc::replay_source(&mut chain, &stamps, late, floor) {
            RollupSource::None => Value::Null,
            RollupSource::Chain => json!(calc::chain_union_len(&chain)),
            RollupSource::Table(e) => json!(1_000_000_000 + e),
        };
        assert_eq!(got, step["result"], "player replay {late}");
        assert_eq!(chain_pairs(&chain), step["chain"], "player chain {late}");
    }
    let mut chain: Chain<ClassSets> = Chain::new();
    for step in arr(&r["class"]) {
        let (early, late) = (i(&step["early"]), i(&step["late"]));
        if step["reset"].as_bool().unwrap() {
            chain.clear();
        }
        let set = |v: &Value| arr(v).iter().map(|n| s(n).to_owned()).collect::<HashSet<String>>();
        let sets: ClassSets = step["sets"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.parse().unwrap(), (set(&v[0]), (!v[1].is_null()).then(|| set(&v[1])))))
            .collect();
        calc::chain_push(&mut chain, early, late, sets);
        let got = match calc::replay_source(&mut chain, &stamps, late, floor) {
            RollupSource::None => json!({}),
            RollupSource::Chain => {
                let m: serde_json::Map<String, Value> = calc::chain_class_rollups(&chain)
                    .into_iter()
                    .map(|(k, (raw, clean))| (k.to_string(), json!([raw, clean])))
                    .collect();
                Value::Object(m)
            }
            RollupSource::Table(_) => json!({"table": [stamps[0], late]}),
        };
        assert_eq!(got, step["result"], "class replay {late}");
        assert_eq!(chain_pairs(&chain), step["chain"], "class chain {late}");
    }
}

#[test]
fn pure_helpers_match_python() {
    let p = &fixtures()["pure"];
    for c in arr(&p["resets"]) {
        let kind = match s(&c[0]) {
            "daily" => Some(ResetKind::Daily),
            "weekly" => Some(ResetKind::Weekly),
            _ => None,
        };
        let got = kind.is_some_and(|k| calc::reset_crossed(k, i(&c[1]), i(&c[2])));
        assert_eq!(got, c[3].as_bool().unwrap(), "{c}");
    }
    for c in arr(&p["picks"]) {
        let stamps: Vec<i64> = arr(&c[0]).iter().map(i).collect();
        assert_eq!(calc::pick_early_anchor(&stamps, i(&c[1]), i(&c[2])), c[3].as_i64(), "{c}");
    }
    for c in arr(&p["medians"]) {
        let vals: Vec<f64> = arr(&c[0]).iter().map(|v| v.as_f64().unwrap()).collect();
        assert_eq!(calc::median(&vals), c[1].as_f64(), "{c}");
    }
}

#[tokio::test]
async fn backfill_routes_match_python() {
    let router = Router::new()
        .route(
            "/v1/activity/backfill",
            post(|Query(q): Query<BackfillQuery>| async move {
                let spawned =
                    json!({"total_days": q.total_days, "chunk_days": q.chunk_days, "force": q.force, "reset": q.reset});
                let body = backfill_body(q.total_days, Some(q.chunk_days), q.force, q.reset, "activity");
                (StatusCode::ACCEPTED, [("x-spawned", pyjson::dumps(&spawned))], PydanticJson(body))
            }),
        )
        .route(
            "/v1/class-activity/backfill",
            post(|Query(q): Query<ClassBackfillQuery>| async move {
                let spawned = json!({"total_days": q.total_days, "force": q.force, "reset": q.reset});
                let body = backfill_body(q.total_days, None, q.force, q.reset, "class activity");
                (StatusCode::ACCEPTED, [("x-spawned", pyjson::dumps(&spawned))], PydanticJson(body))
            }),
        );
    for c in arr(&fixtures()["backfill"]) {
        let path = s(&c["path"]);
        let req = Request::builder().method("POST").uri(path).body(Body::empty()).unwrap();
        let resp: Response = router.clone().oneshot(req).await.unwrap();
        let spawned = resp.headers().get("x-spawned").map(|v| v.to_str().unwrap().to_owned());
        let status = resp.status().as_u16();
        let body = String::from_utf8(to_bytes(resp.into_body(), usize::MAX).await.unwrap().to_vec()).unwrap();
        same_reply(&c["reply"], status, None, &body, path);
        let want = arr(&c["spawned"]).first().map(pyjson::dumps);
        assert_eq!(spawned, want, "{path} spawned");
    }
}

#[test]
fn site_gate_matches_python() {
    for c in arr(&fixtures()["gate"]) {
        let flag = s(&c["flag"]);
        let path = s(&c["path"]).split('?').next().unwrap();
        let player_on = flag != crate::features::PLAYER_ACTIVITY_FLAG;
        let class_on = flag != crate::features::CLASS_ACTIVITY_FLAG;
        if site_blocked(path, player_on, class_on) {
            let mut headers = axum::http::HeaderMap::new();
            headers.insert("accept", "text/html".parse().unwrap());
            let resp = error::http_exception(StatusCode::NOT_FOUND, None, &axum::http::Method::GET, path, &headers);
            assert_eq!(resp.status().as_u16() as u64, c["reply"]["status"].as_u64().unwrap(), "{path}");
        } else {
            let period = if s(&c["path"]).ends_with("1m") { "1m" } else { "7d" };
            let (_, days, bucket) = player::PERIODS.iter().find(|p| p.0 == period).copied().unwrap();
            let ws = now() - days * 86_400;
            let rows: Vec<Estimate> = arr(&c["rows"]).iter().map(estimate).filter(|r| r.window_end >= ws).collect();
            let payload = player::series_payload(period, bucket, ws, now(), &rows);
            same_reply(&c["reply"], 200, Some("no-cache"), &pyjson::dumps(&payload), path);
        }
    }
}

#[test]
fn board_class_reads_the_classes_index() {
    let dir = std::env::temp_dir().join(format!("kiwi-activity-classes-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("classes.json"),
        r#"[{"qualified_name": "knight", "name": "Old"}, {"qualified_name": "gunslinger", "name": "Gunslinger"},
            {"qualified_name": "knight", "name": "Knight"}]"#,
    )
    .unwrap();
    let gd = crate::gamedata::Gamedata::new(&dir, None);
    let got = class::class_labels(&gd).unwrap();
    assert_eq!(got[0], (json!("Knight"), json!("/static/class-icons/knight.png")));
    assert_eq!(got[1], (json!("Gunslinger"), json!("/static/class-icons/gunslinger.png")));
    assert_eq!(got[2], (json!("Class 2"), Value::Null));
    std::fs::write(dir.join("classes.json"), r#"[{"name": "no key"}]"#).unwrap();
    let broken = crate::gamedata::Gamedata::new(&dir, None);
    assert!(class::class_labels(&broken).is_err());
}

/// Needs a throwaway Postgres with the leaderboards schema: `KIWI_TEST_PG`.
#[tokio::test]
#[ignore]
async fn store_round_trip() {
    let dsn = std::env::var("KIWI_TEST_PG").expect("KIWI_TEST_PG");
    let cfg: tokio_postgres::Config = dsn.parse().unwrap();
    let pool = deadpool_postgres::Pool::builder(deadpool_postgres::Manager::new(cfg, tokio_postgres::NoTls))
        .max_size(2)
        .build()
        .unwrap();
    store::delete_all_active_windows(&pool).await.unwrap();
    store::record_active_window(&pool, 100, &store::lowered(["Ab", "ab", "Ünï"])).await.unwrap();
    store::record_active_window(&pool, 200, &store::lowered(["AB", "cd"])).await.unwrap();
    assert_eq!(store::count_active_since(&pool, 0, 200).await.unwrap(), 3);
    assert_eq!(store::count_active_since(&pool, 100, 200).await.unwrap(), 2);
    let rows = [(1, "x".to_owned(), false), (1, "y".to_owned(), true), (2, "x".to_owned(), true)];
    store::record_class_active_window(&pool, 300, &rows).await.unwrap();
    store::upsert_class_estimates(
        &pool,
        &class::rows(250, 300, &class::counts_of(&ClassSets::new()), &BTreeMap::new(), 1),
    )
    .await
    .unwrap();
    assert!(store::class_rollup_since(&pool, 250, 300).await.unwrap().is_empty());
}

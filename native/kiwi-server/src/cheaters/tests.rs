//! Replays `testdata/cheaters/golden.json` from
//! `native/port/parity/gen_cheaters_fixtures.py`: the real Python pipeline,
//! cache module and routes over an in-memory capture database. [`Fake`]
//! implements the same database semantics as the generator's fakes.

use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
};

use kiwi_core::cheaters::{
    Board, Entry, Series, effective_reset_kind, fixed, fmt_num, g3, name_stem, parse_excluded, py_sum,
    reset_boundary_before, round_val,
};
use serde_json::Value;

use super::{
    cache_control, detect, reset,
    store::{BoardRow, Store, TopSeries, series_from_rows, trove_day_start},
    views, wire,
};
use crate::pyjson;

fn golden() -> &'static Value {
    static G: OnceLock<Value> = OnceLock::new();
    G.get_or_init(|| pyjson::loads(include_bytes!("../../testdata/cheaters/golden.json")).unwrap())
}

/// Detection keeps process-wide caches, like Python's module globals.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[derive(Clone)]
struct Row {
    uuid: i64,
    anchor: i64,
    name: String,
    rank: i64,
    score: f64,
}

/// Redis as a Python dict: `[(key, (value, ttl))]` in insertion order.
type Keys = Vec<(String, (String, i64))>;

struct Fake {
    boards: Vec<Value>,
    contests: Vec<(i64, i64, String)>,
    rows: Vec<Row>,
    settings: serde_json::Map<String, Value>,
    now: f64,
    hot: i64,
    redis: Option<Mutex<Keys>>,
    triggers: Mutex<usize>,
}

impl Fake {
    fn new(case: &Value, redis: Option<&Value>) -> Fake {
        let db = &golden()["dbs"][case["db"].as_str().unwrap()];
        let rows = db["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| Row {
                uuid: e[0].as_i64().unwrap(),
                anchor: e[1].as_i64().unwrap(),
                name: e[2].as_str().unwrap().into(),
                rank: e[3].as_i64().unwrap(),
                score: e[4].as_f64().unwrap(),
            })
            .collect();
        let contests = db["contests"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| (c[0].as_i64().unwrap(), c[1].as_i64().unwrap(), c[2].as_str().unwrap().into()))
            .collect();
        let redis = redis.filter(|r| !r.is_null()).map(|r| {
            Mutex::new(
                r.as_object()
                    .unwrap()
                    .iter()
                    .map(|(k, v)| (k.clone(), (v[0].as_str().unwrap().into(), v[1].as_i64().unwrap())))
                    .collect(),
            )
        });
        Fake {
            boards: db["boards"].as_array().unwrap().clone(),
            contests,
            rows,
            settings: case["settings"].as_object().unwrap().clone(),
            now: case["now"].as_f64().unwrap(),
            hot: case["hot_window_start"].as_i64().unwrap(),
            redis,
            triggers: Mutex::new(0),
        }
    }

    fn canonical(&self) -> HashMap<String, String> {
        let mut out = HashMap::new();
        for r in &self.rows {
            out.entry(r.name.to_lowercase()).or_insert_with(|| r.name.clone());
        }
        out
    }

    fn player_ids(&self) -> HashMap<String, i64> {
        let mut out = HashMap::new();
        for r in &self.rows {
            let n = out.len() as i64 + 1;
            out.entry(r.name.to_lowercase()).or_insert(n);
        }
        out
    }

    fn redis_state(&self) -> Option<Value> {
        let r = self.redis.as_ref()?.lock().unwrap();
        Some(Value::Object(r.iter().map(|(k, (v, t))| (k.clone(), serde_json::json!([v, t]))).collect()))
    }
}

fn s(v: &Value, k: &str) -> String {
    v[k].as_str().unwrap().to_string()
}

impl Store for Fake {
    async fn setting(&self, key: &str) -> anyhow::Result<Value> {
        self.settings.get(key).cloned().ok_or_else(|| anyhow::anyhow!("Python never read {key}"))
    }

    fn now(&self) -> f64 {
        self.now
    }

    fn trigger_warmer(&self) {
        *self.triggers.lock().unwrap() += 1;
    }

    async fn hot_window_start(&self) -> i64 {
        self.hot
    }

    fn has_redis(&self) -> bool {
        self.redis.is_some()
    }

    async fn redis_get(&self, key: &str) -> Option<String> {
        let r = self.redis.as_ref()?.lock().unwrap();
        r.iter().find(|(k, _)| k == key).map(|(_, (v, _))| v.clone())
    }

    async fn redis_set(&self, key: &str, value: String, ttl: i64) {
        let Some(r) = &self.redis else { return };
        let mut r = r.lock().unwrap();
        match r.iter_mut().find(|(k, _)| k == key) {
            Some((_, v)) => *v = (value, ttl),
            None => r.push((key.to_string(), (value, ttl))),
        }
    }

    async fn redis_scan(&self, pattern: &str) -> anyhow::Result<Vec<String>> {
        let prefix = pattern.strip_suffix('*').unwrap();
        let r = self.redis.as_ref().unwrap().lock().unwrap();
        Ok(r.iter().filter(|(k, _)| k.starts_with(prefix)).map(|(k, _)| k.clone()).collect())
    }

    async fn redis_del(&self, keys: &[String]) -> anyhow::Result<()> {
        self.redis.as_ref().unwrap().lock().unwrap().retain(|(k, _)| !keys.contains(k));
        Ok(())
    }

    async fn list_timestamps(&self, limit: i64) -> anyhow::Result<Vec<i64>> {
        let mut a: Vec<i64> = self.rows.iter().map(|r| r.anchor).collect();
        a.sort_unstable_by(|x, y| y.cmp(x));
        a.dedup();
        a.truncate(limit as usize);
        Ok(a)
    }

    async fn list_boards_at(&self, anchor: i64) -> anyhow::Result<Vec<BoardRow>> {
        let present: std::collections::HashSet<i64> =
            self.rows.iter().filter(|r| r.anchor == anchor).map(|r| r.uuid).collect();
        let mut rows: Vec<&Value> =
            self.boards.iter().filter(|b| present.contains(&b["uuid"].as_i64().unwrap())).collect();
        rows.sort_by_key(|b| (s(b, "category"), s(b, "name")));
        Ok(rows
            .into_iter()
            .map(|b| {
                let uuid = b["uuid"].as_i64().unwrap();
                let over = b["reset_kind_override"].as_str().map(str::to_string);
                BoardRow {
                    board: Board {
                        uuid,
                        name_id: s(b, "name_id"),
                        name: s(b, "name"),
                        category_id: s(b, "category_id"),
                        category: s(b, "category"),
                        contest_type: self.contests.iter().find(|c| c.0 == uuid && c.1 == anchor).map(|c| c.2.clone()),
                        reset_kind: effective_reset_kind(over.as_deref(), uuid),
                    },
                    reset_kind_override: over,
                }
            })
            .collect())
    }

    async fn entries_by_board_at(&self, anchor: i64, uuids: &[i64]) -> anyhow::Result<HashMap<i64, Vec<Entry>>> {
        let mut rows: Vec<&Row> = self.rows.iter().filter(|r| r.anchor == anchor && uuids.contains(&r.uuid)).collect();
        rows.sort_by_key(|r| (r.uuid, r.rank));
        let mut out: HashMap<i64, Vec<Entry>> = HashMap::new();
        for r in rows {
            out.entry(r.uuid).or_default().push(Entry { player_name: r.name.clone(), rank: r.rank, score: r.score });
        }
        Ok(out)
    }

    async fn previous_captures_bulk(
        &self,
        names: &[String],
        uuid: i64,
        anchor: i64,
        cycle_start: i64,
    ) -> anyhow::Result<HashMap<String, (f64, i64)>> {
        let lowered: Vec<String> = names.iter().map(|n| n.to_lowercase()).collect();
        let canon = self.canonical();
        let mut best: HashMap<String, (f64, i64)> = HashMap::new();
        for r in &self.rows {
            let lo = r.name.to_lowercase();
            if r.uuid != uuid || !lowered.contains(&lo) || !canon.contains_key(&lo) {
                continue;
            }
            if !(cycle_start <= r.anchor && r.anchor < anchor) {
                continue;
            }
            if best.get(&lo).is_none_or(|b| r.anchor > b.1) {
                best.insert(lo, (r.score, r.anchor));
            }
        }
        Ok(best.into_iter().map(|(lo, v)| (canon[&lo].clone(), v)).collect())
    }

    async fn comovement_series(&self, uuid: i64, names: &[String], since: i64, until: i64) -> anyhow::Result<Series> {
        let lowered: Vec<String> = names.iter().map(|n| n.to_lowercase()).collect();
        let canon = self.canonical();
        Ok(series_from_rows(
            self.rows
                .iter()
                .filter(|r| {
                    r.uuid == uuid && lowered.contains(&r.name.to_lowercase()) && since <= r.anchor && r.anchor <= until
                })
                .map(|r| (canon[&r.name.to_lowercase()].clone(), r.anchor, r.score)),
        ))
    }

    async fn list_entries(&self, uuid: i64, anchor: i64, limit: i64, offset: i64) -> anyhow::Result<(Vec<Entry>, i64)> {
        let mut rows: Vec<&Row> = self.rows.iter().filter(|r| r.uuid == uuid && r.anchor == anchor).collect();
        rows.sort_by_key(|r| r.rank);
        let canon = self.canonical();
        let items = rows
            .iter()
            .skip(offset as usize)
            .take(limit as usize)
            .map(|r| Entry { player_name: canon[&r.name.to_lowercase()].clone(), rank: r.rank, score: r.score })
            .collect();
        Ok((items, rows.len() as i64))
    }

    async fn previous_day_anchor(&self, uuid: i64, anchor: i64) -> anyhow::Result<Option<i64>> {
        let day = trove_day_start(anchor);
        Ok(self.rows.iter().filter(|r| r.uuid == uuid && r.anchor < day).map(|r| r.anchor).max())
    }

    async fn board_reset_kind(&self, uuid: i64) -> anyhow::Result<String> {
        let over = self
            .boards
            .iter()
            .find(|b| b["uuid"].as_i64() == Some(uuid))
            .and_then(|b| b["reset_kind_override"].as_str());
        Ok(effective_reset_kind(over, uuid))
    }

    async fn prev_rows_for_players(
        &self,
        uuid: i64,
        prev_anchor: i64,
        names: &[String],
    ) -> anyhow::Result<HashMap<String, (i64, f64)>> {
        let lowered: Vec<String> = names.iter().map(|n| n.to_lowercase()).collect();
        let mut out = HashMap::new();
        for r in &self.rows {
            let lo = r.name.to_lowercase();
            if r.uuid == uuid && r.anchor == prev_anchor && lowered.contains(&lo) {
                out.insert(lo, (r.rank, r.score));
            }
        }
        Ok(out)
    }

    async fn board_top_series(&self, uuid: i64, window_start: i64, top: i64) -> anyhow::Result<TopSeries> {
        let mut anchors: Vec<i64> =
            self.rows.iter().filter(|r| r.uuid == uuid && r.anchor >= window_start).map(|r| r.anchor).collect();
        anchors.sort_unstable();
        anchors.dedup();
        let Some(&latest) = anchors.last() else { return Ok((Vec::new(), Vec::new(), Vec::new())) };
        let (ids, canon) = (self.player_ids(), self.canonical());
        let mut at: Vec<&Row> = self.rows.iter().filter(|r| r.uuid == uuid && r.anchor == latest).collect();
        at.sort_by_key(|r| r.rank);
        at.truncate(top as usize);
        let meta: Vec<(i64, String, i64)> =
            at.iter().map(|r| (ids[&r.name.to_lowercase()], canon[&r.name.to_lowercase()].clone(), r.rank)).collect();
        let pids: Vec<i64> = meta.iter().map(|m| m.0).collect();
        let series = self
            .rows
            .iter()
            .filter(|r| r.uuid == uuid && r.anchor >= window_start && pids.contains(&ids[&r.name.to_lowercase()]))
            .map(|r| (ids[&r.name.to_lowercase()], r.anchor, r.rank, r.score))
            .collect();
        Ok((anchors, meta, series))
    }
}

fn cases(key: &str) -> &'static Vec<Value> {
    golden()[key].as_array().unwrap()
}

fn same_maps(got: Option<Value>, want: &Value, label: &str) {
    match (got, want) {
        (None, Value::Null) => {}
        (Some(Value::Object(g)), Value::Object(w)) => {
            let mut gk: Vec<_> = g.keys().collect();
            let mut wk: Vec<_> = w.keys().collect();
            gk.sort();
            wk.sort();
            assert_eq!(gk, wk, "{label}: redis keys");
            for (k, v) in w {
                assert_eq!(&g[k], v, "{label}: redis {k}");
            }
        }
        (g, w) => panic!("{label}: redis {g:?} vs {w}"),
    }
}

async fn check_reply(f: &Fake, payload: &Value, v1: &Value, site: &Value, label: &str) {
    let cc = cache_control(f).await.unwrap();
    match wire::v1_body(payload) {
        Ok(body) => {
            assert_eq!(v1["status"], 200, "{label}");
            assert_eq!(pyjson::pydantic(&body), s(v1, "body"), "{label}: /v1 body");
            assert_eq!(cc.to_str().unwrap(), s(v1, "cache_control"), "{label}: /v1 cache-control");
        }
        Err(e) => assert_eq!(v1["status"], 500, "{label}: {e}"),
    }
    assert_eq!(site["status"], 200, "{label}");
    assert_eq!(pyjson::dumps(payload), s(site, "body"), "{label}: /site body");
    assert_eq!(cc.to_str().unwrap(), s(site, "cache_control"), "{label}: /site cache-control");
}

#[tokio::test]
async fn compute_matches_python() {
    let _g = SERIAL.lock().await;
    for case in cases("compute") {
        let label = s(case, "name");
        reset();
        let f = Fake::new(case, Some(&serde_json::json!({})));
        let warmed = detect(&f, true).await.unwrap();
        let writes = &case["redis_writes"];
        let anchor = warmed["anchor"].as_i64();
        if let Some(raw) = anchor.and_then(|a| writes.get(format!("lb:cache:cheaters:{a}"))) {
            assert_eq!(pyjson::dumps_spaced(&warmed), raw[0].as_str().unwrap(), "{label}: payload");
        }
        same_maps(f.redis_state(), writes, &label);
        let served = detect(&f, false).await.unwrap();
        assert_eq!(served, warmed, "{label}: served");
        check_reply(&f, &served, &case["v1"], &case["site"], &label).await;
    }
}

#[tokio::test]
async fn serving_matches_python() {
    let _g = SERIAL.lock().await;
    for case in cases("serve") {
        let label = s(case, "name");
        reset();
        let f = Fake::new(case, Some(&case["redis"]));
        let first = detect(&f, false).await.unwrap();
        let second = detect(&f, false).await.unwrap();
        assert_eq!(first, second, "{label}");
        check_reply(&f, &first, &case["v1"], &case["site"], &label).await;
        let missing = matches!(label.as_str(), "missing_snapshot" | "null_snapshot" | "clusters_only_ready");
        assert_eq!(*f.triggers.lock().unwrap(), if missing { 2 } else { 0 }, "{label}: warmer wakes");
    }
}

#[tokio::test]
async fn views_recompute_matches_python() {
    let _g = SERIAL.lock().await;
    for case in cases("views") {
        let label = s(case, "name");
        let f = Fake::new(case, Some(&case["redis"]));
        let body = views::recompute(&f).await.unwrap();
        assert_eq!(case["reply"]["status"], 200, "{label}");
        assert_eq!(pyjson::pydantic(&body), s(&case["reply"], "body"), "{label}: body");
        same_maps(f.redis_state(), &case["redis_after"], &label);
    }
}

#[test]
fn helpers_match_python() {
    let u = &golden()["units"];
    for c in u["py_sum"].as_array().unwrap() {
        let xs: Vec<f64> = c[0].as_array().unwrap().iter().map(|x| x.as_f64().unwrap()).collect();
        assert_eq!(py_sum(xs.iter().copied()).to_bits(), c[1].as_f64().unwrap().to_bits(), "sum {xs:?}");
    }
    for c in u["fmt"].as_array().unwrap() {
        let v = c[0].as_f64().unwrap();
        if let Some(want) = c[1].as_str() {
            assert_eq!(fmt_num(v), want, "_fmt {v}");
        }
        assert_eq!(fixed(v, 0), c[2].as_str().unwrap(), ".0f {v}");
        assert_eq!(fixed(v, 1), c[3].as_str().unwrap(), ".1f {v}");
        assert_eq!(g3(v), c[4].as_str().unwrap(), ".3g {v}");
        if !c[5].is_null() {
            assert_eq!(round_val(v), c[5], "_round {v}");
        }
    }
    for c in u["stems"].as_array().unwrap() {
        assert_eq!(name_stem(c[0].as_str().unwrap()), c[1].as_str().unwrap(), "stem {}", c[0]);
    }
    for c in u["excluded"].as_array().unwrap() {
        let got: Vec<i64> = parse_excluded(c[0].as_str().unwrap()).into_iter().collect();
        let want: Vec<i64> = c[1].as_array().unwrap().iter().map(|x| x.as_i64().unwrap()).collect();
        assert_eq!(got, want, "excluded {}", c[0]);
    }
    for c in u["reset_before"].as_array().unwrap() {
        let (a, k) = (c[0].as_i64().unwrap(), c[1].as_str().unwrap());
        assert_eq!(reset_boundary_before(a, k), c[2].as_i64().unwrap(), "reset before {a} {k}");
    }
    for c in u["reset_boundaries"].as_array().unwrap() {
        let got = views::reset_boundaries(c[0].as_str().unwrap(), c[1].as_i64().unwrap(), c[2].as_i64().unwrap());
        let want: Vec<i64> = c[3].as_array().unwrap().iter().map(|x| x.as_i64().unwrap()).collect();
        assert_eq!(got, want, "boundaries {c}");
    }
}

//! Golden checks against `testdata/lb_names/golden.json`
//! (`native/port/parity/gen_lb_names_fixtures.py`): the same worlds run through
//! Python's detectors and through these, over an in-memory [`Store`] with the
//! SQL's semantics, must write the same rows, talk to Redis the same way and
//! answer the routes with the same bytes.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{LazyLock, Mutex};

use axum::http::{HeaderMap, StatusCode, Uri};
use kiwi_core::activity::Capture;
use kiwi_core::renames::{MatchCfg, Named};
use serde_json::{Map, Value, json};

use super::store::{
    BoardRow, DateWrite, DupGroup, DuplicateRow, DuplicateWrite, R, RenameRow, RenameWrite, Store, WindowRow,
};
use super::{
    ClearFirst, DupPage, Page, duplicate_lookup, duplicates, duplicates_list, duplicates_status, lower_key,
    rename_history, renames, renames_list, renames_status, started, tail,
};
use crate::{
    error::ApiError,
    features::{self, py_truthy},
    middleware::pyurl::scope_path,
    pyjson, request_id,
    validation::{FromParams, Params},
};

static GOLDEN: LazyLock<Value> = LazyLock::new(|| {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/testdata/lb_names/golden.json");
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
});
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn s(v: &Value) -> &str {
    v.as_str().unwrap()
}

fn i(v: &Value) -> i64 {
    v.as_i64().unwrap()
}

fn f(v: &Value) -> f64 {
    v.as_f64().unwrap()
}

fn arr(v: &Value) -> &Vec<Value> {
    v.as_array().unwrap()
}

/// A binary COPY of the given rows (each field a big-endian int4/int8/float8).
enum Field {
    I4(i32),
    I8(i64),
    F8(f64),
}

fn copy(rows: &[Vec<Field>]) -> Vec<u8> {
    let mut b = b"PGCOPY\n\xff\r\n\0".to_vec();
    b.extend(0i32.to_be_bytes());
    b.extend(0i32.to_be_bytes());
    for row in rows {
        b.extend((row.len() as i16).to_be_bytes());
        for field in row {
            match field {
                Field::I4(v) => {
                    b.extend(4i32.to_be_bytes());
                    b.extend(v.to_be_bytes());
                }
                Field::I8(v) => {
                    b.extend(8i32.to_be_bytes());
                    b.extend(v.to_be_bytes());
                }
                Field::F8(v) => {
                    b.extend(8i32.to_be_bytes());
                    b.extend(v.to_be_bytes());
                }
            }
        }
    }
    b.extend((-1i16).to_be_bytes());
    b
}

#[derive(Default)]
struct Mem {
    settings: Map<String, Value>,
    boards: Vec<BoardRow>,
    players: Vec<(i64, String)>,
    entries: Vec<(i32, i64, i32, f64, i64)>,
    fail: HashSet<i64>,
    fail_anchor_list: bool,
    deleted: (i64, i64),
    redis: Option<Mutex<HashMap<String, String>>>,
    writes: Mutex<Vec<Value>>,
    ops: Mutex<Vec<Value>>,
    renames: Vec<RenameRow>,
    dups: Vec<DuplicateRow>,
}

impl Mem {
    fn world(w: &Value) -> Mem {
        let boards = arr(&w["boards"])
            .iter()
            .map(|b| BoardRow {
                uuid: i(&b[0]) as i32,
                name_id: s(&b[1]).into(),
                name: s(&b[2]).into(),
                reset_kind_override: b[3].as_str().map(str::to_owned),
            })
            .collect();
        Mem {
            boards,
            players: arr(&w["players"]).iter().map(|p| (i(&p[0]), s(&p[1]).to_owned())).collect(),
            entries: arr(&w["entries"])
                .iter()
                .map(|e| (i(&e[0]) as i32, i(&e[1]), i(&e[2]) as i32, f(&e[3]), i(&e[4])))
                .collect(),
            fail: arr(&w["fail_anchors"]).iter().map(i).collect(),
            fail_anchor_list: w["fail_anchor_list"].as_bool().unwrap(),
            deleted: (i(&w["deleted"]["delete_renames"]), i(&w["deleted"]["delete_duplicates"])),
            ..Default::default()
        }
    }

    fn anchors(&self) -> Vec<i64> {
        let mut a: Vec<i64> = self.entries.iter().map(|e| e.1).collect();
        a.sort_unstable();
        a.dedup();
        a
    }

    fn write(&self, op: &str, rows: Vec<Value>) {
        self.writes.lock().unwrap().push(json!({"op": op, "rows": rows}));
    }

    fn op(&self, v: Value) {
        self.ops.lock().unwrap().push(v);
    }

    fn name_of(&self, id: i64) -> Option<&String> {
        self.players.iter().find(|p| p.0 == id).map(|p| &p.1)
    }
}

impl Store for Mem {
    async fn flag(&self, flag: &str) -> R<bool> {
        Ok(py_truthy(&self.setting(flag).await?))
    }

    async fn setting(&self, key: &str) -> R<Value> {
        self.settings.get(key).cloned().ok_or_else(|| anyhow::anyhow!("no setting {key}"))
    }

    async fn all_anchors_asc(&self) -> R<Vec<i64>> {
        if self.fail_anchor_list {
            anyhow::bail!("anchor scan timed out");
        }
        Ok(self.anchors())
    }

    async fn latest_anchors(&self, limit: i64) -> R<Vec<i64>> {
        Ok(self.anchors().into_iter().rev().take(limit as usize).collect())
    }

    async fn boards_at(&self, anchor: i64) -> R<Vec<BoardRow>> {
        let present: HashSet<i32> = self.entries.iter().filter(|e| e.1 == anchor).map(|e| e.0).collect();
        Ok(self.boards.iter().filter(|b| present.contains(&b.uuid)).cloned().collect())
    }

    async fn capture(&self, anchor: i64, uuids: &[i32]) -> R<Capture> {
        if self.fail.contains(&anchor) {
            anyhow::bail!("capture {anchor} failed");
        }
        let rows: Vec<Vec<Field>> = self
            .entries
            .iter()
            .filter(|e| e.1 == anchor && uuids.contains(&e.0))
            .map(|e| vec![Field::I4(e.0), Field::I8(e.4), Field::F8(e.3)])
            .collect();
        Ok(kiwi_core::activity::decode_copy(&copy(&rows)).unwrap())
    }

    async fn player_names(&self, ids: &[i64]) -> R<Vec<(i64, String)>> {
        Ok(ids.iter().filter_map(|id| self.name_of(*id).map(|n| (*id, n.clone()))).collect())
    }

    async fn upsert_renames(&self, rows: &[RenameWrite]) -> R<()> {
        if !rows.is_empty() {
            self.write("upsert_renames", rows.iter().map(rename_write_row).collect());
        }
        Ok(())
    }

    async fn list_renames(&self, limit: i64, offset: i64) -> R<(Vec<RenameRow>, i64)> {
        let mut rows = self.renames.clone();
        rows.sort_by(|a, b| {
            b.to_anchor.cmp(&a.to_anchor).then(b.confidence.total_cmp(&a.confidence)).then(b.id.cmp(&a.id))
        });
        let page = rows.into_iter().skip(offset as usize).take(limit as usize).collect();
        Ok((page, self.renames.len() as i64))
    }

    async fn renames_for_name(&self, lowered: &str) -> R<Vec<RenameRow>> {
        let mut rows: Vec<RenameRow> = self
            .renames
            .iter()
            .filter(|r| lower_key(&r.from_name) == lowered || lower_key(&r.to_name) == lowered)
            .cloned()
            .collect();
        rows.sort_by(|a, b| b.to_anchor.cmp(&a.to_anchor).then(b.id.cmp(&a.id)));
        Ok(rows)
    }

    async fn count_renames(&self) -> R<i64> {
        Ok(self.renames.len() as i64)
    }

    async fn delete_all_renames(&self) -> R<i64> {
        self.writes.lock().unwrap().push(json!({"op": "delete_renames"}));
        Ok(self.deleted.0)
    }

    async fn duplicate_groups_at(&self, anchor: i64) -> R<Vec<DupGroup>> {
        if self.fail.contains(&anchor) {
            anyhow::bail!("grouping {anchor} failed");
        }
        let rows: Vec<Vec<Field>> = self
            .entries
            .iter()
            .filter(|e| e.1 == anchor)
            .map(|e| vec![Field::I4(e.0), Field::I8(e.4), Field::I4(e.2), Field::F8(e.3)])
            .collect();
        let found = kiwi_core::activity::duplicate_groups(&copy(&rows)).unwrap();
        Ok(found
            .into_iter()
            .map(|(pid, board_uuid, occurrences)| DupGroup {
                name: self.name_of(pid).cloned().unwrap_or_else(|| pid.to_string()),
                board_uuid,
                occurrences,
            })
            .collect())
    }

    async fn player_rows_window(&self, name: &str, start: i64, end: i64) -> R<Vec<WindowRow>> {
        let Some(&(pid, _)) = self.players.iter().find(|p| p.1.to_lowercase() == lower_key(name)) else {
            return Ok(Vec::new());
        };
        // Postgres gives no order here; the reverse of Python's proves it doesn't matter.
        Ok(self
            .entries
            .iter()
            .rev()
            .filter(|e| e.4 == pid && e.1 >= start && e.1 <= end && super::store::is_player_board(e.0))
            .map(|e| WindowRow { leaderboard: e.0, created_at: e.1, rank: e.2, score: e.3 })
            .collect())
    }

    async fn upsert_duplicates(&self, rows: &[DuplicateWrite]) -> R<()> {
        if !rows.is_empty() {
            self.write("upsert_duplicates", rows.iter().map(dup_write_row).collect());
        }
        Ok(())
    }

    async fn upsert_duplicate_dates(&self, rows: &[DateWrite]) -> R<()> {
        if !rows.is_empty() {
            self.write("upsert_duplicate_dates", rows.iter().map(date_write_row).collect());
        }
        Ok(())
    }

    async fn list_duplicates(&self, limit: i64, offset: i64, kind: Option<&str>) -> R<(Vec<DuplicateRow>, i64, i64)> {
        let rows: Vec<&DuplicateRow> =
            self.dups.iter().filter(|r| kind.is_none_or(|k| r.kind == k || r.kind == "both")).collect();
        let latest = self.dups.iter().map(|r| r.last_anchor).max();
        let current = rows.iter().filter(|r| Some(r.last_anchor) == latest).count() as i64;
        let mut page: Vec<DuplicateRow> = rows.iter().map(|r| (*r).clone()).collect();
        page.sort_by(|a, b| {
            b.last_anchor
                .cmp(&a.last_anchor)
                .then(b.boards.cmp(&a.boards))
                .then(lower_key(&a.name).cmp(&lower_key(&b.name)))
        });
        let total = rows.len() as i64;
        Ok((page.into_iter().skip(offset as usize).take(limit as usize).collect(), total, current))
    }

    async fn latest_duplicate_anchor(&self) -> R<Option<i64>> {
        Ok(self.dups.iter().map(|r| r.last_anchor).max())
    }

    async fn duplicate_for_name(&self, name: &str) -> R<Option<DuplicateRow>> {
        Ok(self.dups.iter().find(|r| lower_key(&r.name) == lower_key(name)).cloned())
    }

    async fn count_duplicates(&self) -> R<i64> {
        Ok(self.dups.len() as i64)
    }

    async fn delete_all_duplicates(&self) -> R<i64> {
        self.writes.lock().unwrap().push(json!({"op": "delete_duplicates"}));
        Ok(self.deleted.1)
    }

    async fn lock(&self, key: &str) -> Option<bool> {
        let redis = self.redis.as_ref()?;
        self.op(json!(["set_nx", key]));
        let mut data = redis.lock().unwrap();
        if data.contains_key(key) {
            return Some(false);
        }
        data.insert(key.into(), "1".into());
        Some(true)
    }

    async fn heartbeat(&self, key: &str) {
        if self.redis.is_some() {
            self.op(json!(["expire", key, 900]));
        }
    }

    async fn unlock(&self, key: &str) {
        if let Some(redis) = &self.redis {
            self.op(json!(["delete", key]));
            redis.lock().unwrap().remove(key);
        }
    }

    async fn save_status(&self, key: &str, json: &str) {
        if let Some(redis) = &self.redis {
            self.op(json!(["set", key, json, 86400]));
            redis.lock().unwrap().insert(key.into(), json.into());
        }
    }

    async fn load_status(&self, key: &str) -> Option<String> {
        let redis = self.redis.as_ref()?;
        self.op(json!(["get", key]));
        redis.lock().unwrap().get(key).cloned()
    }
}

fn rename_write_row(r: &RenameWrite) -> Value {
    json!([
        r.from_name,
        r.from_name_lower,
        r.to_name,
        r.to_name_lower,
        r.from_anchor,
        r.to_anchor,
        r.confidence,
        r.matched_boards,
        r.evidence,
        r.method_version,
        r.created_at
    ])
}

fn dup_write_row(r: &DuplicateWrite) -> Value {
    json!([
        r.name_lower,
        r.name,
        r.kind,
        r.verdict,
        r.boards,
        r.max_occurrences,
        r.spellings,
        r.first_anchor,
        r.last_anchor,
        r.evidence,
        r.method_version,
        r.updated_at
    ])
}

fn date_write_row(r: &DateWrite) -> Value {
    json!([r.name_lower, r.name, r.boards, r.max_occurrences, r.first_anchor, r.last_anchor, r.updated_at])
}

fn fixed_clock() -> f64 {
    1_760_000_000.75
}

/// `executemany` row order inside a dates flush follows a Python set, so compare those sorted.
fn normalized_writes(writes: &[Value]) -> Vec<Value> {
    writes
        .iter()
        .map(|w| {
            let mut w = w.clone();
            if w["op"] == "upsert_duplicate_dates" {
                let mut rows = w["rows"].as_array().unwrap().clone();
                rows.sort_by_key(|r| r.to_string());
                w["rows"] = Value::Array(rows);
            }
            w
        })
        .collect()
}

/// The status text Redis gets, with the exception text (Python's type name +
/// message, ours the error chain) masked.
fn masked_op(op: &Value) -> Value {
    let mut op = op.clone();
    if op[0] == "set"
        && let Ok(Value::Object(mut m)) = serde_json::from_str::<Value>(s(&op[2]))
        && m.contains_key("error")
    {
        m.insert("error".into(), "<error>".into());
        op[2] = Value::String(pyjson::dumps_spaced(&Value::Object(m)));
    }
    op
}

#[tokio::test]
async fn scenarios_match_python() {
    let _serial = SERIAL.lock().await;
    let g = &*GOLDEN;
    assert_eq!(f(&arr(&g["scenarios"])[0]["clock"]), fixed_clock());
    for sc in arr(&g["scenarios"]) {
        let name = s(&sc["name"]);
        let mut mem = Mem::world(&g["worlds"][s(&sc["world"])]);
        mem.settings = sc["settings"].as_object().unwrap().clone();
        if sc["redis"].as_bool().unwrap() {
            let held = arr(&sc["redis_held"]).iter().map(|k| (s(k).to_owned(), "1".to_owned())).collect();
            mem.redis = Some(Mutex::new(held));
        }
        let keys = Mutex::new(renames::Keys::default());
        let pair_guard = Mutex::new(None);
        let anchor_guard = Mutex::new(None);
        for (n, args) in arr(&sc["args"]).iter().enumerate() {
            let force = args.get("force").and_then(Value::as_bool).unwrap_or(false);
            let clear_first = args.get("clear_first").and_then(Value::as_bool).unwrap_or(false);
            let got = match s(&sc["call"]) {
                "renames.backfill" => {
                    renames::backfill(&mem, &keys, fixed_clock, clear_first).await;
                    Value::Null
                }
                "duplicates.backfill" => {
                    duplicates::backfill(&mem, fixed_clock, clear_first).await;
                    Value::Null
                }
                "renames.detect_latest" => {
                    renames::detect_latest(&mem, &keys, &pair_guard, fixed_clock, force).await.unwrap()
                }
                "duplicates.detect_latest" => {
                    duplicates::detect_latest(&mem, &anchor_guard, fixed_clock, force).await.unwrap()
                }
                other => panic!("{other}"),
            };
            assert_eq!(pyjson::dumps(&got), pyjson::dumps(&sc["results"][n]), "{name} result {n}");
        }
        let want = normalized_writes(arr(&sc["writes"]));
        let got = normalized_writes(&mem.writes.lock().unwrap());
        assert_eq!(got.len(), want.len(), "{name}: write count");
        for (k, (g, w)) in got.iter().zip(&want).enumerate() {
            assert_eq!(g["op"], w["op"], "{name} write {k}");
            let (grows, wrows) = (g["rows"].as_array(), w["rows"].as_array());
            assert_eq!(grows.map(Vec::len), wrows.map(Vec::len), "{name} write {k} rows");
            for (j, (gr, wr)) in grows.into_iter().flatten().zip(wrows.into_iter().flatten()).enumerate() {
                assert_eq!(gr, wr, "{name} write {k} row {j}");
            }
        }
        let ops: Vec<Value> = mem.ops.lock().unwrap().iter().map(masked_op).collect();
        let want_ops: Vec<Value> = arr(&sc["redis_ops"]).iter().map(masked_op).collect();
        assert_eq!(ops, want_ops, "{name}: redis traffic");
    }
}

fn named(v: &Value) -> BTreeMap<String, Named> {
    v.as_object()
        .unwrap()
        .iter()
        .map(|(k, fp)| {
            let boards = arr(&fp["boards"]).iter().map(|b| (i(&b[0]) as i32, f(&b[1]))).collect();
            (k.clone(), Named { display: s(&fp["display"]).into(), boards })
        })
        .collect()
}

#[test]
fn matcher_matches_python() {
    for (n, c) in arr(&GOLDEN["match"]).iter().enumerate() {
        let cfg = MatchCfg {
            min_boards: i(&c["cfg"]["min_boards"]),
            drift: f(&c["cfg"]["drift"]),
            min_conf: f(&c["cfg"]["min_conf"]),
        };
        let names: HashMap<i32, String> =
            arr(&c["cfg"]["board_names"]).iter().map(|b| (i(&b[0]) as i32, s(&b[1]).to_owned())).collect();
        let found = kiwi_core::renames::match_pair(&named(&c["fp_a"]), &named(&c["fp_b"]), &cfg);
        let rows: Vec<Value> =
            renames::to_writes(found, i(&c["from_anchor"]), i(&c["to_anchor"]), &names, i(&c["now"]))
                .iter()
                .map(rename_write_row)
                .collect();
        assert_eq!(&Value::Array(rows), &c["rows"], "match case {n}");
    }
}

#[test]
fn splitter_matches_python() {
    use kiwi_core::duplicates as core;
    for (n, c) in arr(&GOLDEN["split"]).iter().enumerate() {
        let ta = arr(&c["text_args"]);
        assert_eq!(core::summary_text(s(&ta[0]), i(&ta[1]), s(&ta[2]), i(&ta[3])), s(&c["text"]), "text {n}");
        let Some(points) = c.get("points") else { continue };
        let pts: Vec<core::Point> =
            arr(points).iter().map(|p| core::Point { created_at: i(&p[0]), rank: i(&p[1]), score: f(&p[2]) }).collect();
        let split = core::split_series(&pts);
        let got: Vec<Value> = split
            .iter()
            .map(|sr| {
                json!([sr.slot, sr.points.iter().map(|p| json!([p.created_at, p.rank, p.score])).collect::<Vec<_>>()])
            })
            .collect();
        assert_eq!(&Value::Array(got), &c["split"], "split {n}");
        let summ = core::summarise_series(&split, i(&c["frozen_min_captures"]));
        let text = pyjson::dumps_spaced(&Value::Array(summ.iter().map(duplicates::series_value).collect()));
        assert_eq!(text, s(&c["summary"]), "summary {n}");
        assert_eq!(core::verdict([summ.as_slice()]), s(&c["verdict"]), "verdict {n}");
    }
}

fn route_mem(rt: &Value) -> Mem {
    let rows = &GOLDEN["route_rows"];
    let renames = arr(&rows["renames"])
        .iter()
        .map(|r| RenameRow {
            id: i(&r[0]),
            from_name: s(&r[1]).into(),
            to_name: s(&r[2]).into(),
            from_anchor: i(&r[3]),
            to_anchor: i(&r[4]),
            confidence: f(&r[5]),
            matched_boards: i(&r[6]) as i32,
            evidence: s(&r[7]).into(),
            method_version: i(&r[8]) as i32,
            created_at: i(&r[9]),
        })
        .collect();
    let dups = arr(&rows["duplicates"])
        .iter()
        .map(|r| DuplicateRow {
            name: s(&r[0]).into(),
            kind: s(&r[1]).into(),
            verdict: s(&r[2]).into(),
            boards: i(&r[3]) as i32,
            max_occurrences: i(&r[4]) as i32,
            spellings: s(&r[5]).into(),
            first_anchor: i(&r[6]),
            last_anchor: i(&r[7]),
            evidence: s(&r[8]).into(),
            method_version: i(&r[9]) as i32,
            updated_at: i(&r[10]),
        })
        .collect();
    let mut redis = HashMap::new();
    if let Some(m) = rt["redis_status"].as_object() {
        redis = m.iter().map(|(k, v)| (k.clone(), s(v).to_owned())).collect();
    }
    Mem {
        settings: rt["settings"].as_object().unwrap().clone(),
        renames,
        dups,
        redis: Some(Mutex::new(redis)),
        ..Default::default()
    }
}

/// One fixture request through the same gates, validation and cores the
/// handlers chain (auth is stubbed on both sides).
async fn answer(mem: &Mem, method: &str, path: &str) -> (u16, String, Option<String>) {
    let uri: Uri = path.parse().unwrap();
    let decoded = scope_path(&uri);
    let headers = HeaderMap::new();
    let mut params = Params::new(Vec::new(), uri.query(), &headers);
    let err = |e: ApiError| (e.status.as_u16(), e.body(), None);
    let (v1, rest) = match decoded.strip_prefix("/v1/leaderboards/") {
        Some(rest) => (true, rest.to_owned()),
        None => (false, decoded.strip_prefix("/site/leaderboards/").unwrap().to_owned()),
    };
    if !mem.flag(features::LEADERBOARDS_FLAG).await.unwrap() {
        return err(if v1 { features::disabled("Leaderboards") } else { ApiError::http(StatusCode::NOT_FOUND, None) });
    }
    let gate = |flag: &'static str, label: &'static str| async move {
        if v1 && !mem.flag(flag).await.unwrap() { Err(features::disabled(label)) } else { Ok(()) }
    };
    let renames_gate = || gate(features::RENAMES_FLAG, "Rename detection");
    let dup_gate = || gate(features::DUPLICATES_FLAG, "Duplicate-name detection");
    let prefix = if v1 { "/v1/leaderboards/" } else { "/site/leaderboards/" };
    let res = match (method, rest.as_str()) {
        ("GET", "renames") => match renames_gate().await {
            Err(e) => Err(e),
            Ok(()) => {
                let q = Page::from_params(&mut params);
                match std::mem::take(&mut params.v).finish(q) {
                    Err(e) => Err(e.into()),
                    Ok(q) => renames_list(mem, &q, v1).await.map(|r| (200, r.body, Some(r.cache))),
                }
            }
        },
        ("GET", r) if r.starts_with("renames/") => match renames_gate().await {
            Err(e) => Err(e),
            Ok(()) => rename_history(mem, &tail(&uri, &format!("{prefix}renames/")), v1)
                .await
                .map(|r| (200, r.body, Some(r.cache))),
        },
        ("GET", "duplicates") => match dup_gate().await {
            Err(e) => Err(e),
            Ok(()) => {
                let q = DupPage::from_params(&mut params);
                match std::mem::take(&mut params.v).finish(q) {
                    Err(e) => Err(e.into()),
                    Ok(q) => duplicates_list(mem, &q, v1).await.map(|r| (200, r.body, Some(r.cache))),
                }
            }
        },
        ("GET", r) if r.starts_with("duplicates/") => match dup_gate().await {
            Err(e) => Err(e),
            Ok(()) => duplicate_lookup(mem, &tail(&uri, &format!("{prefix}duplicates/")), v1)
                .await
                .map(|r| (200, r.body, Some(r.cache))),
        },
        ("GET", "renames-backfill-status") => renames_status(mem).await.map(|b| (200, b, None)),
        ("GET", "duplicates-backfill-status") => duplicates_status(mem).await.map(|b| (200, b, None)),
        ("POST", "renames-backfill" | "duplicates-backfill") => {
            let q = ClearFirst::from_params(&mut params);
            match std::mem::take(&mut params.v).finish(q) {
                Err(e) => Err(e.into()),
                Ok(ClearFirst(cf)) => Ok((202, started(cf), None)),
            }
        }
        other => panic!("unrouted {other:?}"),
    };
    res.unwrap_or_else(err)
}

#[tokio::test]
async fn routes_match_python() {
    let _serial = SERIAL.lock().await;
    let rid = s(&GOLDEN["rid"]).to_owned();
    for rt in arr(&GOLDEN["routes"]) {
        renames::reset_local_status();
        duplicates::reset_local_status();
        let mem = route_mem(rt);
        let (status, body, cache) =
            request_id::scope(rid.clone(), answer(&mem, s(&rt["method"]), s(&rt["path"]))).await;
        let name = s(&rt["name"]);
        assert_eq!(status, i(&rt["status"]) as u16, "{name}: status ({body})");
        assert_eq!(body, s(&rt["body"]), "{name}: body");
        assert_eq!(cache.as_deref(), rt["cache_control"].as_str(), "{name}: cache-control");
        assert_eq!(s(&rt["content_type"]), "application/json", "{name}");
    }
}

#[test]
fn python_slicing_and_keys() {
    assert_eq!(super::py_slice_end(5, 2), 2);
    assert_eq!(super::py_slice_end(5, 9), 5);
    assert_eq!(super::py_slice_end(5, -1), 4);
    assert_eq!(super::py_slice_end(2, -9), 0);
    assert_eq!(lower_key("\u{3000} ΣΟΦΟΣ "), "σοφος");
    assert_eq!(super::int_secs(1_760_000_000.75), 1_760_000_000);
}

//! Store access for detection and the page-view caches: the Postgres reads of
//! `app/trove/leaderboards/pg_store.py` (same SQL) and the `lb:cache:*` keys
//! of `cache.py`. Every Redis failure degrades like Python's (`None` / no-op).

use std::collections::HashMap;

use anyhow::Context;
use kiwi_core::cheaters::{Board, Entry, Series, effective_reset_kind};
use serde_json::Value;

use crate::{jobs, pyjson, runtime_config, state::AppState};

pub const PREFIX: &str = "lb:cache:";
pub const READY_KEY: &str = "lb:cache:ready_anchor";
pub const TS_KEY: &str = "lb:cache:timestamps";
pub const VIEW_TTL: i64 = 6 * 3600;

pub fn cheaters_key(anchor: i64) -> String {
    format!("{PREFIX}cheaters:{anchor}")
}

/// A `list_boards_at` row with the fields only the page snapshot carries.
#[derive(Clone, Debug, PartialEq)]
pub struct BoardRow {
    pub board: Board,
    pub reset_kind_override: Option<String>,
}

/// `board_top_series`: (anchors, top rows `(player_id, name, rank)`, series
/// rows `(player_id, anchor, rank, score)`).
pub type TopSeries = (Vec<i64>, Vec<(i64, String, i64)>, Vec<(i64, i64, i64, f64)>);

/// Everything the detection pass and the views warm read or write.
#[allow(async_fn_in_trait)]
pub trait Store: Sync {
    async fn setting(&self, key: &str) -> anyhow::Result<Value>;
    fn now(&self) -> f64;
    fn trigger_warmer(&self);
    async fn hot_window_start(&self) -> i64;

    fn has_redis(&self) -> bool;
    async fn redis_get(&self, key: &str) -> Option<String>;
    async fn redis_set(&self, key: &str, value: String, ttl: i64);
    async fn redis_scan(&self, pattern: &str) -> anyhow::Result<Vec<String>>;
    async fn redis_del(&self, keys: &[String]) -> anyhow::Result<()>;

    async fn list_timestamps(&self, limit: i64) -> anyhow::Result<Vec<i64>>;
    async fn list_boards_at(&self, anchor: i64) -> anyhow::Result<Vec<BoardRow>>;
    async fn entries_by_board_at(&self, anchor: i64, uuids: &[i64]) -> anyhow::Result<HashMap<i64, Vec<Entry>>>;
    async fn previous_captures_bulk(
        &self,
        names: &[String],
        uuid: i64,
        anchor: i64,
        cycle_start: i64,
    ) -> anyhow::Result<HashMap<String, (f64, i64)>>;
    async fn comovement_series(&self, uuid: i64, names: &[String], since: i64, until: i64) -> anyhow::Result<Series>;
    async fn list_entries(&self, uuid: i64, anchor: i64, limit: i64, offset: i64) -> anyhow::Result<(Vec<Entry>, i64)>;
    async fn previous_day_anchor(&self, uuid: i64, anchor: i64) -> anyhow::Result<Option<i64>>;
    async fn board_reset_kind(&self, uuid: i64) -> anyhow::Result<String>;
    async fn prev_rows_for_players(
        &self,
        uuid: i64,
        prev_anchor: i64,
        names: &[String],
    ) -> anyhow::Result<HashMap<String, (i64, f64)>>;
    async fn board_top_series(&self, uuid: i64, window_start: i64, top: i64) -> anyhow::Result<TopSeries>;
}

/// `cache._rget`: `json.loads` of a str value, `None` on any failure.
pub async fn rget<S: Store>(s: &S, key: &str) -> Option<Value> {
    let raw = s.redis_get(key).await?;
    if raw.starts_with('\u{feff}') {
        return None;
    }
    pyjson::loads(raw.as_bytes()).ok().filter(|v| !v.is_null())
}

/// `cache._rset`: `json.dumps(value)` with an expiry.
pub async fn rset<S: Store>(s: &S, key: &str, value: &Value, ttl: i64) {
    if s.has_redis() {
        s.redis_set(key, pyjson::dumps_spaced(value), ttl).await;
    }
}

/// `cache.get_ready_anchor`.
pub async fn ready_anchor<S: Store>(s: &S) -> Option<i64> {
    match rget(s, READY_KEY).await? {
        Value::Number(n) => n.as_i64(),
        Value::Bool(b) => Some(i64::from(b)),
        _ => None,
    }
}

/// `cache._retention_days`.
pub async fn retention_days<S: Store>(s: &S) -> i64 {
    match s.setting("leaderboards_hot_retention_days").await.ok().as_ref().and_then(py_int) {
        Some(d) => d.max(1),
        None => 3,
    }
}

/// `lb_service.hot_window_start`: the tierer's hot cutoff.
pub async fn hot_window_start_from<S: Store>(s: &S, default_days: i64) -> i64 {
    let days =
        s.setting("leaderboards_pg_tier_after_days").await.ok().as_ref().and_then(py_int).unwrap_or(default_days);
    let now = s.now() as i64;
    trove_day_start(now) - (days.max(1) - 1) * 86400
}

pub fn trove_day_start(anchor: i64) -> i64 {
    (anchor - 11 * 3600).div_euclid(86400) * 86400 + 11 * 3600
}

/// Python `int(v)` of a setting value.
pub fn py_int(v: &Value) -> Option<i64> {
    match v {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().filter(|f| f.is_finite()).map(|f| f.trunc() as i64)),
        Value::Bool(b) => Some(i64::from(*b)),
        Value::String(s) => kiwi_core::cheaters::py_int(kiwi_core::pystr::strip(s)),
        _ => None,
    }
}

/// Python `float(v)` of a setting value.
pub fn py_float(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::Bool(b) => Some(f64::from(u8::from(*b))),
        Value::String(s) => kiwi_core::pystr::strip(s).parse().ok(),
        _ => None,
    }
}

/// Python `str(v)` of a setting value.
pub fn py_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "None".into(),
        Value::Bool(b) => if *b { "True" } else { "False" }.into(),
        Value::Number(n) if n.is_f64() => crate::pyfloat::repr(n.as_f64().unwrap_or(f64::NAN)),
        other => other.to_string(),
    }
}

pub fn unix_now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

// ---------------------------------------------------------------------------
// Production store

/// The live stores.
pub struct Live<'a>(pub &'a AppState);

static WAKE: tokio::sync::Notify = tokio::sync::Notify::const_new();

/// What the warmer loop awaits between passes (woken by [`Store::trigger_warmer`]).
pub fn warmer_wake() -> &'static tokio::sync::Notify {
    &WAKE
}

impl Live<'_> {
    async fn pg(&self) -> anyhow::Result<deadpool_postgres::Object> {
        let pool = self.0.inner.db.pg.as_ref().context("POSTGRES_DSN not configured")?;
        Ok(pool.get().await?)
    }

    fn redis(&self) -> Option<redis::aio::ConnectionManager> {
        self.0.inner.db.redis.clone()
    }
}

/// Wakes one Python worker's warmer while Python still runs the loop: its
/// serving path calls `trigger_warmer()` on the same miss this server just saw.
fn wake_python(state: &AppState) {
    let state = state.clone();
    tokio::spawn(async move {
        let uri = format!("{}/site/leaderboards/cheaters", state.settings().py_upstream);
        let Ok(req) = axum::http::Request::get(uri).body(axum::body::Body::empty()) else { return };
        match state.inner.upstream.request(req).await {
            Ok(res) => {
                let _ = http_body_util::BodyExt::collect(res.into_body()).await;
            }
            Err(e) => tracing::warn!("cheaters: waking the Python warmer failed: {e}"),
        }
    });
}

fn i32_of(uuid: i64) -> anyhow::Result<i32> {
    i32::try_from(uuid).context("board uuid out of int4 range")
}

impl Store for Live<'_> {
    async fn setting(&self, key: &str) -> anyhow::Result<Value> {
        runtime_config::get_setting(self.0, key).await
    }

    fn now(&self) -> f64 {
        unix_now()
    }

    async fn hot_window_start(&self) -> i64 {
        hot_window_start_from(self, self.0.settings().leaderboards_pg_tier_after_days).await
    }

    fn trigger_warmer(&self) {
        if jobs::owns("leaderboards_warmer") {
            WAKE.notify_one();
        } else {
            wake_python(self.0);
        }
    }

    fn has_redis(&self) -> bool {
        self.0.inner.db.redis.is_some()
    }

    async fn redis_get(&self, key: &str) -> Option<String> {
        let mut conn = self.redis()?;
        match redis::cmd("GET").arg(key).query_async::<Option<Vec<u8>>>(&mut conn).await {
            Ok(raw) => String::from_utf8(raw?).ok(),
            Err(e) => {
                tracing::warn!("lb cache: get {key} failed: {e}");
                None
            }
        }
    }

    async fn redis_set(&self, key: &str, value: String, ttl: i64) {
        let Some(mut conn) = self.redis() else { return };
        let r: redis::RedisResult<()> =
            redis::cmd("SET").arg(key).arg(value).arg("EX").arg(ttl).query_async(&mut conn).await;
        if let Err(e) = r {
            tracing::warn!("lb cache: set {key} failed: {e}");
        }
    }

    async fn redis_scan(&self, pattern: &str) -> anyhow::Result<Vec<String>> {
        let mut conn = self.redis().context("no redis")?;
        let mut out = Vec::new();
        let mut cursor: u64 = 0;
        loop {
            let (next, keys): (u64, Vec<String>) =
                redis::cmd("SCAN").arg(cursor).arg("MATCH").arg(pattern).query_async(&mut conn).await?;
            out.extend(keys);
            if next == 0 {
                return Ok(out);
            }
            cursor = next;
        }
    }

    async fn redis_del(&self, keys: &[String]) -> anyhow::Result<()> {
        let mut conn = self.redis().context("no redis")?;
        let _: i64 = redis::cmd("DEL").arg(keys).query_async(&mut conn).await?;
        Ok(())
    }

    async fn list_timestamps(&self, limit: i64) -> anyhow::Result<Vec<i64>> {
        let rows = self
            .pg()
            .await?
            .query(
                "
                WITH RECURSIVE a AS (
                    SELECT (SELECT max(anchor) FROM entry) AS anchor
                    UNION ALL
                    SELECT (SELECT max(e.anchor) FROM entry e WHERE e.anchor < a.anchor)
                    FROM a WHERE a.anchor IS NOT NULL
                )
                SELECT anchor FROM a WHERE anchor IS NOT NULL LIMIT $1
                ",
                &[&limit],
            )
            .await?;
        Ok(rows.iter().map(|r| r.get::<_, i64>(0)).collect())
    }

    async fn list_boards_at(&self, anchor: i64) -> anyhow::Result<Vec<BoardRow>> {
        let rows = self
            .pg()
            .await?
            .query(
                "SELECT b.uuid, b.name_id, b.name, b.category_id, b.category, \
                        b.reset_kind_override, bc.type AS contest_type \
                 FROM (SELECT DISTINCT board_uuid FROM entry WHERE anchor = $1) e \
                 JOIN board b ON b.uuid = e.board_uuid \
                 LEFT JOIN board_contest bc ON bc.board_uuid = b.uuid AND bc.anchor = $1 \
                 ORDER BY b.category, b.name",
                &[&anchor],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| {
                let uuid = i64::from(r.get::<_, i32>("uuid"));
                let over: Option<String> = r.get("reset_kind_override");
                BoardRow {
                    board: Board {
                        uuid,
                        name_id: r.get("name_id"),
                        name: r.get("name"),
                        category_id: r.get("category_id"),
                        category: r.get("category"),
                        contest_type: r.get("contest_type"),
                        reset_kind: effective_reset_kind(over.as_deref(), uuid),
                    },
                    reset_kind_override: over,
                }
            })
            .collect())
    }

    async fn entries_by_board_at(&self, anchor: i64, uuids: &[i64]) -> anyhow::Result<HashMap<i64, Vec<Entry>>> {
        let ids: Vec<i32> = uuids.iter().map(|u| i32_of(*u)).collect::<anyhow::Result<_>>()?;
        let rows = self
            .pg()
            .await?
            .query(
                "SELECT e.board_uuid, p.name AS player_name, e.rank, e.score \
                 FROM entry e JOIN player p ON p.id = e.player_id WHERE e.anchor = $1 \
                 AND e.board_uuid = ANY($2) ORDER BY e.board_uuid, e.rank",
                &[&anchor, &ids],
            )
            .await?;
        let mut out: HashMap<i64, Vec<Entry>> = HashMap::new();
        for r in &rows {
            out.entry(i64::from(r.get::<_, i32>(0))).or_default().push(Entry {
                player_name: r.get(1),
                rank: i64::from(r.get::<_, i32>(2)),
                score: r.get(3),
            });
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
        if names.is_empty() {
            return Ok(HashMap::new());
        }
        let lowered: Vec<String> = names.iter().map(|n| n.to_lowercase()).collect();
        let con = self.pg().await?;
        let who = con.query("SELECT id, name FROM player WHERE name_lower = ANY($1)", &[&lowered]).await?;
        if who.is_empty() {
            return Ok(HashMap::new());
        }
        let by_id: HashMap<i64, String> = who.iter().map(|r| (r.get::<_, i64>(0), r.get::<_, String>(1))).collect();
        let ids: Vec<i64> = who.iter().map(|r| r.get::<_, i64>(0)).collect();
        let rows = con
            .query(
                "SELECT DISTINCT ON (player_id) player_id, score, anchor FROM entry \
                 WHERE board_uuid = $1 AND player_id = ANY($2::bigint[]) \
                 AND anchor < $3 AND anchor >= $4 \
                 ORDER BY player_id, anchor DESC",
                &[&i32_of(uuid)?, &ids, &anchor, &cycle_start],
            )
            .await?;
        Ok(rows
            .iter()
            .filter_map(|r| {
                let name = by_id.get(&r.get::<_, i64>(0))?.clone();
                Some((name, (r.get::<_, f64>(1), r.get::<_, i64>(2))))
            })
            .collect())
    }

    async fn comovement_series(&self, uuid: i64, names: &[String], since: i64, until: i64) -> anyhow::Result<Series> {
        if names.is_empty() {
            return Ok(Vec::new());
        }
        let lowered: Vec<String> = names.iter().map(|n| n.to_lowercase()).collect();
        let rows = self
            .pg()
            .await?
            .query(
                "SELECT p.name AS player_name, e.anchor, e.score \
                 FROM entry e JOIN player p ON p.id = e.player_id \
                 WHERE e.board_uuid = $1 AND p.name_lower = ANY($2) \
                 AND e.anchor >= $3 AND e.anchor <= $4",
                &[&i32_of(uuid)?, &lowered, &since, &until],
            )
            .await?;
        Ok(series_from_rows(rows.iter().map(|r| (r.get::<_, String>(0), r.get::<_, i64>(1), r.get::<_, f64>(2)))))
    }

    async fn list_entries(&self, uuid: i64, anchor: i64, limit: i64, offset: i64) -> anyhow::Result<(Vec<Entry>, i64)> {
        let con = self.pg().await?;
        let uuid = i32_of(uuid)?;
        let total: i64 = con
            .query_one("SELECT count(*) FROM entry WHERE board_uuid = $1 AND anchor = $2", &[&uuid, &anchor])
            .await?
            .get(0);
        let rows = con
            .query(
                "SELECT p.name AS player_name, e.rank, e.score \
                 FROM entry e JOIN player p ON p.id = e.player_id \
                 WHERE e.board_uuid = $1 AND e.anchor = $2 \
                 ORDER BY e.rank LIMIT $3 OFFSET $4",
                &[&uuid, &anchor, &limit, &offset],
            )
            .await?;
        let items = rows
            .iter()
            .map(|r| Entry { player_name: r.get(0), rank: i64::from(r.get::<_, i32>(1)), score: r.get(2) })
            .collect();
        Ok((items, total))
    }

    async fn previous_day_anchor(&self, uuid: i64, anchor: i64) -> anyhow::Result<Option<i64>> {
        let row = self
            .pg()
            .await?
            .query_one(
                "SELECT MAX(anchor) FROM entry WHERE board_uuid = $1 AND anchor < $2",
                &[&i32_of(uuid)?, &trove_day_start(anchor)],
            )
            .await?;
        Ok(row.get(0))
    }

    async fn board_reset_kind(&self, uuid: i64) -> anyhow::Result<String> {
        let row = self
            .pg()
            .await?
            .query_opt("SELECT reset_kind_override FROM board WHERE uuid = $1", &[&i32_of(uuid)?])
            .await?;
        let over: Option<String> = row.and_then(|r| r.get(0));
        Ok(effective_reset_kind(over.as_deref(), uuid))
    }

    async fn prev_rows_for_players(
        &self,
        uuid: i64,
        prev_anchor: i64,
        names: &[String],
    ) -> anyhow::Result<HashMap<String, (i64, f64)>> {
        if names.is_empty() {
            return Ok(HashMap::new());
        }
        let lowered: Vec<String> = names.iter().map(|n| n.to_lowercase()).collect();
        let rows = self
            .pg()
            .await?
            .query(
                "SELECT p.name_lower, e.rank, e.score \
                 FROM entry e JOIN player p ON p.id = e.player_id \
                 WHERE e.board_uuid = $1 AND e.anchor = $2 AND p.name_lower = ANY($3)",
                &[&i32_of(uuid)?, &prev_anchor, &lowered],
            )
            .await?;
        Ok(rows.iter().map(|r| (r.get::<_, String>(0), (i64::from(r.get::<_, i32>(1)), r.get::<_, f64>(2)))).collect())
    }

    async fn board_top_series(&self, uuid: i64, window_start: i64, top: i64) -> anyhow::Result<TopSeries> {
        let con = self.pg().await?;
        let id = i32_of(uuid)?;
        let anchors: Vec<i64> = con
            .query(
                "SELECT DISTINCT anchor FROM entry WHERE board_uuid = $1 AND anchor >= $2 ORDER BY anchor",
                &[&id, &window_start],
            )
            .await?
            .iter()
            .map(|r| r.get(0))
            .collect();
        let Some(&latest) = anchors.last() else { return Ok((Vec::new(), Vec::new(), Vec::new())) };
        let top_rows: Vec<(i64, String, i64)> = con
            .query(
                "SELECT e.player_id, p.name AS player_name, e.rank \
                 FROM entry e JOIN player p ON p.id = e.player_id \
                 WHERE e.board_uuid = $1 AND e.anchor = $2 \
                 ORDER BY e.rank LIMIT $3",
                &[&id, &latest, &top],
            )
            .await?
            .iter()
            .map(|r| (r.get(0), r.get(1), i64::from(r.get::<_, i32>(2))))
            .collect();
        if top_rows.is_empty() {
            return Ok((anchors, Vec::new(), Vec::new()));
        }
        let ids: Vec<i64> = top_rows.iter().map(|t| t.0).collect();
        let series = con
            .query(
                "SELECT e.player_id, e.anchor AS created_at, e.rank, e.score \
                 FROM entry e \
                 WHERE e.board_uuid = $1 AND e.anchor >= $2 AND e.player_id = ANY($3)",
                &[&id, &window_start, &ids],
            )
            .await?
            .iter()
            .map(|r| (r.get(0), r.get(1), i64::from(r.get::<_, i32>(2)), r.get(3)))
            .collect();
        Ok((anchors, top_rows, series))
    }
}

/// `{name: {anchor: score}}` built row by row, as Python's dicts do.
pub fn series_from_rows(rows: impl Iterator<Item = (String, i64, f64)>) -> Series {
    let mut out: Series = Vec::new();
    let mut at: HashMap<String, usize> = HashMap::new();
    for (name, anchor, score) in rows {
        let i = *at.entry(name.clone()).or_insert_with(|| {
            out.push((name, Vec::new()));
            out.len() - 1
        });
        let pts = &mut out[i].1;
        match pts.iter_mut().find(|(a, _)| *a == anchor) {
            Some(p) => p.1 = score,
            None => pts.push((anchor, score)),
        }
    }
    out
}

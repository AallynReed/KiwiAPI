//! What the player reads touch: the `pg_store` functions they call (same SQL,
//! same row dicts), the Redis snapshot keys, the claim lookup in `site_users`
//! and runtime settings.

use std::collections::HashMap;

use kiwi_core::{lbplayers as core, pystr};
use mongodb::bson::{Document, doc};
use serde_json::{Value, json};
use tokio_postgres::types::{FromSql, Type};

use crate::{
    auth::models::SITE_USERS,
    error::{ApiError, ApiResult},
    features::py_truthy,
    pyjson, runtime_config,
    state::AppState,
};

pub(crate) fn internal(err: impl std::fmt::Display) -> ApiError {
    tracing::error!("lb players: {err}");
    ApiError::internal()
}

/// A `pg_store.player_rows` / `player_rows_window` row.
#[derive(Clone, Debug, PartialEq)]
pub struct EntryRow {
    pub player_name: String,
    pub rank: i64,
    pub score: f64,
    pub leaderboard: i64,
    pub created_at: i64,
}

/// A `pg_store.player_board_summary` row.
#[derive(Clone, Debug, PartialEq)]
pub struct AggRow {
    pub leaderboard: i64,
    pub best_rank: i64,
    pub appearances: i64,
    pub first_seen: i64,
    pub last_seen: i64,
    pub latest_rank: i64,
    pub latest_score: f64,
}

/// One `pg_store.board_meta` value.
#[derive(Clone, Debug, PartialEq)]
pub struct BoardMeta {
    pub name: String,
    pub name_id: String,
    pub category: String,
    pub reset_kind: String,
}

/// `pg_store._rename_row`.
#[derive(Clone, Debug, PartialEq)]
pub struct RenameEdge {
    pub id: i64,
    pub from_name: String,
    pub to_name: String,
    pub from_anchor: i64,
    pub to_anchor: i64,
    pub confidence: f64,
    pub matched_boards: i64,
    pub evidence: Value,
    pub method_version: i64,
    pub created_at: i64,
}

/// `pg_store._duplicate_row`.
#[derive(Clone, Debug, PartialEq)]
pub struct DuplicateRow {
    pub name: String,
    pub kind: String,
    pub verdict: String,
    pub boards: i64,
    pub max_occurrences: i64,
    pub spellings: Value,
    pub first_anchor: i64,
    pub last_anchor: i64,
    pub evidence: Value,
    pub method_version: i64,
    pub updated_at: i64,
}

/// The stores behind the player reads: Postgres/Redis/Mongo live, recorded
/// answers in tests. Names are passed exactly as Python passes them.
pub(crate) trait Store: Sync {
    /// `time.time()`.
    fn now(&self) -> f64;
    /// `settings.leaderboards_pg_tier_after_days`, the fallback of `hot_window_days`.
    fn default_tier_days(&self) -> i64;
    /// `runtime_config.get_setting(key)`.
    async fn setting(&self, key: &str) -> anyhow::Result<Value>;
    async fn player_rows(
        &self,
        name: &str,
        limit: i64,
        uuid: Option<i128>,
        window_start: Option<i64>,
    ) -> ApiResult<Vec<EntryRow>>;
    async fn player_rows_window(&self, name: &str, window_start: i64) -> ApiResult<Vec<EntryRow>>;
    async fn player_canonical_name(&self, name: &str) -> ApiResult<Option<String>>;
    async fn player_board_summary(&self, name: &str) -> ApiResult<Vec<AggRow>>;
    /// `excluded` is the parsed CSV, sorted.
    async fn player_last_played(&self, name: &str, excluded: &[i128]) -> ApiResult<Option<i64>>;
    async fn board_meta(&self, uuids: &[i64]) -> ApiResult<HashMap<i64, BoardMeta>>;
    async fn board_kinds(&self, uuids: &[i64]) -> ApiResult<HashMap<i64, String>>;
    async fn renames_for_name(&self, name: &str) -> ApiResult<Vec<RenameEdge>>;
    async fn duplicate_for_name(&self, name: &str) -> ApiResult<Option<DuplicateRow>>;
    /// `list_timestamps(limit=1)`.
    async fn latest_anchor(&self) -> ApiResult<Option<i64>>;
    /// `SiteUser.find_one({claimed_trove_name, claim_verified: True})` found
    /// something; any failure is `false`.
    async fn claim_verified(&self, claimed_lower: &str) -> bool;
    /// `cache.get_ready_anchor()`.
    async fn ready_anchor(&self) -> Option<i64>;
    /// `cache.get_cheaters(anchor)`.
    async fn cheaters_payload(&self, anchor: i64) -> Option<Value>;
}

/// Python `str.lower()`.
pub fn py_lower(s: &str) -> String {
    s.to_lowercase()
}

/// asyncpg's jsonb codec (`json.loads` of the text form); also takes TEXT so
/// a `::text` column reads the same.
struct JsonbText(String);

impl<'a> FromSql<'a> for JsonbText {
    fn from_sql(ty: &Type, raw: &'a [u8]) -> Result<Self, Box<dyn std::error::Error + Sync + Send>> {
        let body = if *ty == Type::JSONB {
            match raw.split_first() {
                Some((1, rest)) => rest,
                _ => return Err("unsupported jsonb version".into()),
            }
        } else {
            raw
        };
        Ok(JsonbText(std::str::from_utf8(body)?.to_owned()))
    }

    fn accepts(ty: &Type) -> bool {
        matches!(*ty, Type::JSONB | Type::JSON | Type::TEXT)
    }
}

/// Any integer column as asyncpg hands it over (a Python int).
struct Int(i64);

impl<'a> FromSql<'a> for Int {
    fn from_sql(ty: &Type, raw: &'a [u8]) -> Result<Self, Box<dyn std::error::Error + Sync + Send>> {
        Ok(Int(match *ty {
            Type::INT2 => i16::from_sql(ty, raw)?.into(),
            Type::INT4 => i32::from_sql(ty, raw)?.into(),
            _ => i64::from_sql(ty, raw)?,
        }))
    }

    fn accepts(ty: &Type) -> bool {
        matches!(*ty, Type::INT2 | Type::INT4 | Type::INT8)
    }
}

fn int(r: &tokio_postgres::Row, col: &str) -> ApiResult<i64> {
    r.try_get::<_, Int>(col).map(|v| v.0).map_err(internal)
}

fn opt_int(r: &tokio_postgres::Row, col: &str) -> ApiResult<Option<i64>> {
    r.try_get::<_, Option<Int>>(col).map(|v| v.map(|i| i.0)).map_err(internal)
}

fn col<'a, T: FromSql<'a>>(r: &'a tokio_postgres::Row, col: &str) -> ApiResult<T> {
    r.try_get(col).map_err(internal)
}

fn jsonb(r: &tokio_postgres::Row, name: &str) -> ApiResult<String> {
    col::<JsonbText>(r, name).map(|j| j.0)
}

fn codec(text: &str) -> ApiResult<Value> {
    pyjson::loads(text.as_bytes()).map_err(|e| internal(format!("jsonb decode: {e:?}")))
}

/// `_rename_row`'s evidence: a double-encoded string is decoded once more
/// (`{}` if that fails), then `ev or {}`.
pub fn rename_evidence(jsonb: &str) -> ApiResult<Value> {
    let ev = match codec(jsonb)? {
        Value::String(s) => pyjson::loads(s.as_bytes()).unwrap_or_else(|_| json!({})),
        other => other,
    };
    Ok(if py_truthy(&ev) { ev } else { json!({}) })
}

/// `_duplicate_row`'s `_json(val, fallback)`.
pub fn duplicate_json(jsonb: &str, fallback: Value) -> ApiResult<Value> {
    Ok(match codec(jsonb)? {
        Value::String(s) => pyjson::loads(s.as_bytes()).unwrap_or(fallback),
        Value::Null => fallback,
        other => other,
    })
}

/// `_duplicate_row`'s evidence: `_json(val, {}) or {}`.
pub fn duplicate_evidence(jsonb: &str) -> ApiResult<Value> {
    let ev = duplicate_json(jsonb, json!({}))?;
    Ok(if py_truthy(&ev) { ev } else { json!({}) })
}

/// Postgres, Redis and Mongo as the Python app reaches them.
pub(crate) struct Live<'a>(pub &'a AppState);

const RENAME_COLS: &str =
    "id, from_name, to_name, from_anchor, to_anchor, confidence, matched_boards, evidence, method_version, created_at";
const DUP_COLS: &str = "name, kind, verdict, boards, max_occurrences, spellings, first_anchor, last_anchor, evidence, method_version, updated_at";

const COMMAND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

fn non_player_boards() -> Vec<i32> {
    core::NON_PLAYER_BOARDS.iter().map(|&b| b as i32).collect()
}

/// An int parameter asyncpg would refuse (`value out of int32 range`).
fn int4(v: i128) -> ApiResult<i32> {
    i32::try_from(v).map_err(|_| internal(format!("value out of int32 range: {v}")))
}

fn entry_rows(name: &str, rows: &[tokio_postgres::Row]) -> ApiResult<Vec<EntryRow>> {
    rows.iter()
        .map(|r| {
            Ok(EntryRow {
                player_name: name.to_owned(),
                rank: int(r, "rank")?,
                score: col(r, "score")?,
                leaderboard: int(r, "leaderboard")?,
                created_at: int(r, "created_at")?,
            })
        })
        .collect()
}

impl Live<'_> {
    async fn client(&self) -> ApiResult<deadpool_postgres::Object> {
        let pool = self.0.inner.db.pg.as_ref().ok_or_else(|| internal("Postgres pool not initialized"))?;
        pool.get().await.map_err(internal)
    }

    async fn query(
        &self,
        sql: &str,
        params: &[&(dyn tokio_postgres::types::ToSql + Sync)],
    ) -> ApiResult<Vec<tokio_postgres::Row>> {
        let client = self.client().await?;
        let stmt = client.prepare_cached(sql).await.map_err(internal)?;
        // asyncpg's command_timeout: cancel server-side, then fail the request.
        match tokio::time::timeout(COMMAND_TIMEOUT, client.query(&stmt, params)).await {
            Ok(rows) => rows.map_err(internal),
            Err(_) => {
                let _ = client.cancel_token().cancel_query(tokio_postgres::NoTls).await;
                Err(internal(format!("query timed out after {COMMAND_TIMEOUT:?}")))
            }
        }
    }

    async fn who(&self, name: &str) -> ApiResult<Option<(i64, String)>> {
        let rows =
            self.query("SELECT id, name FROM player WHERE name_lower = lower($1)", &[&pystr::strip(name)]).await?;
        rows.first().map(|r| Ok((int(r, "id")?, col(r, "name")?))).transpose()
    }

    async fn redis_json(&self, key: &str) -> Option<Value> {
        let mut conn = self.0.inner.db.redis.clone()?;
        let raw: Option<Vec<u8>> = match redis::cmd("GET").arg(key).query_async(&mut conn).await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("lb cache: get {key} failed: {e}");
                return None;
            }
        };
        pyjson::loads(&raw?).ok()
    }
}

impl Store for Live<'_> {
    fn now(&self) -> f64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
    }

    fn default_tier_days(&self) -> i64 {
        self.0.settings().leaderboards_pg_tier_after_days
    }

    async fn setting(&self, key: &str) -> anyhow::Result<Value> {
        runtime_config::get_setting(self.0, key).await
    }

    async fn player_rows(
        &self,
        name: &str,
        limit: i64,
        uuid: Option<i128>,
        window_start: Option<i64>,
    ) -> ApiResult<Vec<EntryRow>> {
        let Some((id, pname)) = self.who(name).await? else { return Ok(Vec::new()) };
        let boards = non_player_boards();
        let uuid = uuid.map(int4).transpose()?;
        let mut sql = String::from(
            "SELECT board_uuid AS leaderboard, anchor AS created_at, rank, score \
             FROM entry WHERE player_id = $1 AND NOT (board_uuid = ANY($2::int[]))",
        );
        let mut params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = vec![&id, &boards];
        if let Some(w) = &window_start {
            params.push(w);
            sql += &format!(" AND anchor >= ${}", params.len());
        }
        if let Some(u) = &uuid {
            params.push(u);
            sql += &format!(" AND board_uuid = ${}", params.len());
        }
        params.push(&limit);
        sql += &format!(" ORDER BY anchor DESC LIMIT ${}", params.len());
        entry_rows(&pname, &self.query(&sql, &params).await?)
    }

    async fn player_rows_window(&self, name: &str, window_start: i64) -> ApiResult<Vec<EntryRow>> {
        let Some((id, pname)) = self.who(name).await? else { return Ok(Vec::new()) };
        let rows = self
            .query(
                "SELECT board_uuid AS leaderboard, anchor AS created_at, rank, score \
                 FROM entry WHERE player_id = $1 AND anchor >= $2 \
                 AND NOT (board_uuid = ANY($3::int[]))",
                &[&id, &window_start, &non_player_boards()],
            )
            .await?;
        entry_rows(&pname, &rows)
    }

    async fn player_canonical_name(&self, name: &str) -> ApiResult<Option<String>> {
        let rows = self.query("SELECT name FROM player WHERE name_lower = lower($1)", &[&pystr::strip(name)]).await?;
        rows.first().map(|r| col(r, "name")).transpose()
    }

    async fn player_board_summary(&self, name: &str) -> ApiResult<Vec<AggRow>> {
        let rows = self
            .query(
                "SELECT a.board_uuid AS leaderboard, a.best_rank, a.appearances::int AS appearances,\
                        a.first_seen, a.last_seen, a.latest_rank, a.latest_score \
                 FROM player_board_agg a JOIN player p ON p.id = a.player_id \
                 WHERE p.name_lower = lower($1) AND NOT (a.board_uuid = ANY($2::int[])) \
                 ORDER BY a.best_rank ASC, a.last_seen DESC",
                &[&pystr::strip(name), &non_player_boards()],
            )
            .await?;
        rows.iter()
            .map(|r| {
                Ok(AggRow {
                    leaderboard: int(r, "leaderboard")?,
                    best_rank: int(r, "best_rank")?,
                    appearances: int(r, "appearances")?,
                    first_seen: int(r, "first_seen")?,
                    last_seen: int(r, "last_seen")?,
                    latest_rank: int(r, "latest_rank")?,
                    latest_score: col(r, "latest_score")?,
                })
            })
            .collect()
    }

    async fn player_last_played(&self, name: &str, excluded: &[i128]) -> ApiResult<Option<i64>> {
        let mut all: Vec<i128> = excluded.iter().copied().chain(core::NON_PLAYER_BOARDS.map(i128::from)).collect();
        all.sort_unstable();
        all.dedup();
        let all = all.into_iter().map(int4).collect::<ApiResult<Vec<i32>>>()?;
        let rows = self
            .query(
                "SELECT max(a.last_rise) FROM player_board_agg a \
                 JOIN player p ON p.id = a.player_id \
                 WHERE p.name_lower = lower($1) AND NOT (a.board_uuid = ANY($2::int[]))",
                &[&pystr::strip(name), &all],
            )
            .await?;
        Ok(rows.first().map(|r| opt_int(r, "max")).transpose()?.flatten())
    }

    async fn board_meta(&self, uuids: &[i64]) -> ApiResult<HashMap<i64, BoardMeta>> {
        if uuids.is_empty() {
            return Ok(HashMap::new());
        }
        let ids = uuids.iter().map(|&u| int4(u.into())).collect::<ApiResult<Vec<i32>>>()?;
        let rows = self
            .query(
                "SELECT uuid, name, name_id, category, reset_kind_override \
                 FROM board WHERE uuid = ANY($1)",
                &[&ids],
            )
            .await?;
        rows.iter()
            .map(|r| {
                let uuid = int(r, "uuid")?;
                let ov: Option<String> = col(r, "reset_kind_override")?;
                let meta = BoardMeta {
                    name: col(r, "name")?,
                    name_id: col(r, "name_id")?,
                    category: col(r, "category")?,
                    reset_kind: core::effective_reset_kind(ov.as_deref(), uuid),
                };
                Ok((uuid, meta))
            })
            .collect()
    }

    async fn board_kinds(&self, uuids: &[i64]) -> ApiResult<HashMap<i64, String>> {
        if uuids.is_empty() {
            return Ok(HashMap::new());
        }
        let ids = uuids.iter().map(|&u| int4(u.into())).collect::<ApiResult<Vec<i32>>>()?;
        let rows = self.query("SELECT uuid, reset_kind_override FROM board WHERE uuid = ANY($1)", &[&ids]).await?;
        rows.iter()
            .map(|r| {
                let uuid = int(r, "uuid")?;
                let ov: Option<String> = col(r, "reset_kind_override")?;
                Ok((uuid, core::effective_reset_kind(ov.as_deref(), uuid)))
            })
            .collect()
    }

    async fn renames_for_name(&self, name: &str) -> ApiResult<Vec<RenameEdge>> {
        let lowered = py_lower(pystr::strip(name));
        let sql = format!(
            "SELECT {RENAME_COLS} FROM player_rename \
             WHERE from_name_lower = $1 OR to_name_lower = $1 \
             ORDER BY to_anchor DESC, id DESC"
        );
        let rows = self.query(&sql, &[&lowered]).await?;
        rows.iter()
            .map(|r| {
                Ok(RenameEdge {
                    id: int(r, "id")?,
                    from_name: col(r, "from_name")?,
                    to_name: col(r, "to_name")?,
                    from_anchor: int(r, "from_anchor")?,
                    to_anchor: int(r, "to_anchor")?,
                    confidence: col(r, "confidence")?,
                    matched_boards: int(r, "matched_boards")?,
                    evidence: rename_evidence(&jsonb(r, "evidence")?)?,
                    method_version: int(r, "method_version")?,
                    created_at: int(r, "created_at")?,
                })
            })
            .collect()
    }

    async fn duplicate_for_name(&self, name: &str) -> ApiResult<Option<DuplicateRow>> {
        let sql = format!("SELECT {DUP_COLS} FROM player_duplicate WHERE name_lower = lower($1)");
        let rows = self.query(&sql, &[&pystr::strip(name)]).await?;
        let Some(r) = rows.first() else { return Ok(None) };
        Ok(Some(DuplicateRow {
            name: col(r, "name")?,
            kind: col(r, "kind")?,
            verdict: col(r, "verdict")?,
            boards: int(r, "boards")?,
            max_occurrences: int(r, "max_occurrences")?,
            spellings: duplicate_json(&jsonb(r, "spellings")?, json!([]))?,
            first_anchor: int(r, "first_anchor")?,
            last_anchor: int(r, "last_anchor")?,
            evidence: duplicate_evidence(&jsonb(r, "evidence")?)?,
            method_version: int(r, "method_version")?,
            updated_at: int(r, "updated_at")?,
        }))
    }

    async fn latest_anchor(&self) -> ApiResult<Option<i64>> {
        let rows = self
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
                &[&1i64],
            )
            .await?;
        rows.first().map(|r| int(r, "anchor")).transpose()
    }

    async fn claim_verified(&self, claimed_lower: &str) -> bool {
        let Some(db) = self.0.inner.db.mongo.as_ref() else { return false };
        db.collection::<Document>(SITE_USERS)
            .find_one(doc! { "claimed_trove_name": claimed_lower, "claim_verified": true })
            .await
            .is_ok_and(|d| d.is_some())
    }

    async fn ready_anchor(&self) -> Option<i64> {
        match self.redis_json("lb:cache:ready_anchor").await? {
            Value::Number(n) => n.as_i64(),
            Value::Bool(b) => Some(b.into()),
            _ => None,
        }
    }

    async fn cheaters_payload(&self, anchor: i64) -> Option<Value> {
        self.redis_json(&format!("lb:cache:cheaters:{anchor}")).await
    }
}

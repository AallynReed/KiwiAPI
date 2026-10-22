//! What rename and duplicate detection read and write: the `pg_store.py`
//! queries, the Redis job keys and the runtime settings. [`Pg`] is production;
//! tests drive the same code through an in-memory [`Store`].

use std::collections::HashMap;

use anyhow::{Context, anyhow};
use futures::TryStreamExt;
use kiwi_core::activity::{Capture, decode_copy, duplicate_groups};
use serde_json::Value;

use crate::{features, feeds::pyval, runtime_config, state::AppState};

/// `board` columns the detectors need, for the boards present at one anchor.
#[derive(Clone, Debug, PartialEq)]
pub struct BoardRow {
    pub uuid: i32,
    pub name_id: String,
    pub name: String,
    pub reset_kind_override: Option<String>,
}

/// A `player_rename` row; `evidence` is the jsonb's text.
#[derive(Clone, Debug, PartialEq)]
pub struct RenameRow {
    pub id: i64,
    pub from_name: String,
    pub to_name: String,
    pub from_anchor: i64,
    pub to_anchor: i64,
    pub confidence: f64,
    pub matched_boards: i32,
    pub evidence: String,
    pub method_version: i32,
    pub created_at: i64,
}

/// One `upsert_renames` parameter row (`evidence` is `json.dumps(evidence)`).
#[derive(Clone, Debug, PartialEq)]
pub struct RenameWrite {
    pub from_name: String,
    pub from_name_lower: String,
    pub to_name: String,
    pub to_name_lower: String,
    pub from_anchor: i64,
    pub to_anchor: i64,
    pub confidence: f64,
    pub matched_boards: i32,
    pub evidence: String,
    pub method_version: i32,
    pub created_at: i64,
}

/// A `player_duplicate` row; `spellings` and `evidence` are the jsonb's text.
#[derive(Clone, Debug, PartialEq)]
pub struct DuplicateRow {
    pub name: String,
    pub kind: String,
    pub verdict: String,
    pub boards: i32,
    pub max_occurrences: i32,
    pub spellings: String,
    pub first_anchor: i64,
    pub last_anchor: i64,
    pub evidence: String,
    pub method_version: i32,
    pub updated_at: i64,
}

/// One `upsert_duplicates` parameter row (json columns as `json.dumps` text).
#[derive(Clone, Debug, PartialEq)]
pub struct DuplicateWrite {
    pub name_lower: String,
    pub name: String,
    pub kind: String,
    pub verdict: String,
    pub boards: i32,
    pub max_occurrences: i32,
    pub spellings: String,
    pub first_anchor: i64,
    pub last_anchor: i64,
    pub evidence: String,
    pub method_version: i32,
    pub updated_at: i64,
}

/// One `upsert_duplicate_dates` parameter row.
#[derive(Clone, Debug, PartialEq)]
pub struct DateWrite {
    pub name_lower: String,
    pub name: String,
    pub boards: i32,
    pub max_occurrences: i32,
    pub first_anchor: i64,
    pub last_anchor: i64,
    pub updated_at: i64,
}

/// `duplicate_groups_at`: a (name, board) with more than one distinct (rank, score).
#[derive(Clone, Debug, PartialEq)]
pub struct DupGroup {
    pub name: String,
    pub board_uuid: i32,
    pub occurrences: i32,
}

/// `player_rows_window`: one of the player's entry rows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindowRow {
    pub leaderboard: i32,
    pub created_at: i64,
    pub rank: i32,
    pub score: f64,
}

pub type R<T> = anyhow::Result<T>;

#[allow(async_fn_in_trait)]
pub trait Store: Sync {
    async fn flag(&self, flag: &str) -> R<bool>;
    async fn setting(&self, key: &str) -> R<Value>;

    async fn all_anchors_asc(&self) -> R<Vec<i64>>;
    /// `list_timestamps(limit)`: newest first.
    async fn latest_anchors(&self, limit: i64) -> R<Vec<i64>>;
    async fn boards_at(&self, anchor: i64) -> R<Vec<BoardRow>>;

    /// `anchor_capture`: the boards' (player id, score) rows, highest score kept.
    async fn capture(&self, anchor: i64, uuids: &[i32]) -> R<Capture>;
    async fn player_names(&self, ids: &[i64]) -> R<Vec<(i64, String)>>;
    async fn upsert_renames(&self, rows: &[RenameWrite]) -> R<()>;
    async fn list_renames(&self, limit: i64, offset: i64) -> R<(Vec<RenameRow>, i64)>;
    /// `renames_for_name` with the name already `strip().lower()`ed.
    async fn renames_for_name(&self, lowered: &str) -> R<Vec<RenameRow>>;
    async fn count_renames(&self) -> R<i64>;
    async fn delete_all_renames(&self) -> R<i64>;

    async fn duplicate_groups_at(&self, anchor: i64) -> R<Vec<DupGroup>>;
    async fn player_rows_window(&self, name: &str, start: i64, end: i64) -> R<Vec<WindowRow>>;
    async fn upsert_duplicates(&self, rows: &[DuplicateWrite]) -> R<()>;
    async fn upsert_duplicate_dates(&self, rows: &[DateWrite]) -> R<()>;
    async fn list_duplicates(&self, limit: i64, offset: i64, kind: Option<&str>) -> R<(Vec<DuplicateRow>, i64, i64)>;
    async fn latest_duplicate_anchor(&self) -> R<Option<i64>>;
    async fn duplicate_for_name(&self, name: &str) -> R<Option<DuplicateRow>>;
    async fn count_duplicates(&self) -> R<i64>;
    async fn delete_all_duplicates(&self) -> R<i64>;

    /// `SET key 1 NX EX 900`: `None` without Redis, else whether it was taken.
    async fn lock(&self, key: &str) -> Option<bool>;
    /// `EXPIRE key 900`; a no-op without Redis.
    async fn heartbeat(&self, key: &str);
    async fn unlock(&self, key: &str);
    /// `SET key json EX 86400`; failures are swallowed.
    async fn save_status(&self, key: &str, json: &str);
    async fn load_status(&self, key: &str) -> Option<String>;
}

/// `int(get_setting(key))`.
pub async fn int_setting<S: Store>(s: &S, key: &str) -> R<i64> {
    let v = s.setting(key).await?;
    pyval::py_int(&v).and_then(|i| i64::try_from(i).ok()).ok_or_else(|| anyhow!("int({v}) failed for {key}"))
}

/// `float(get_setting(key))`.
pub async fn float_setting<S: Store>(s: &S, key: &str) -> R<f64> {
    let v = s.setting(key).await?;
    match &v {
        Value::Bool(b) => Some(f64::from(u8::from(*b))),
        Value::Number(n) => n.as_f64(),
        Value::String(t) => kiwi_core::pystr::strip(t).replace('_', "").parse().ok(),
        _ => None,
    }
    .ok_or_else(|| anyhow!("float({v}) failed for {key}"))
}

const NON_PLAYER_BOARDS: [i32; 2] = [1100, 21012];
const DAILY: [i32; 1] = [32000];
#[rustfmt::skip]
const WEEKLY: &[i32] = &[
    2001, 2002, 2004, 2011, 2013, 2014, 2021, 2024,
    2300, 2301, 2302, 2303, 2304, 2305, 2306, 2307, 2308, 2309, 2310, 2311, 2312, 2313, 2314, 2315, 2316, 2317,
    2400, 2401, 2402, 2403, 2404, 2405, 2406, 2407, 2408, 2409, 2410, 2411, 2412, 2413, 2414, 2415, 2416, 2417,
    4000, 4001, 4002, 4003, 4004, 4005, 4006, 4007, 4008, 4009, 4010, 4011, 4012, 4013, 4014, 4015, 4016, 4017,
    5000, 5001, 5002, 5003, 5004, 5005, 5006, 5007, 5008, 5009, 5010, 5011, 5012, 5013, 5014, 5015, 5016, 5017,
    10004, 10009, 10012, 10019, 21004, 21005, 21012, 30001, 30002, 30003, 30004, 30005, 33001, 33002, 50000,
];

/// `models.is_player_board`.
pub fn is_player_board(uuid: i32) -> bool {
    !NON_PLAYER_BOARDS.contains(&uuid)
}

/// `pg_store._effective_reset_kind`: a valid admin override, else `models.reset_kind`.
pub fn reset_kind(over: Option<&str>, uuid: i32) -> &str {
    match over {
        Some(o @ ("daily" | "weekly" | "none")) => o,
        _ if DAILY.contains(&uuid) => "daily",
        _ if WEEKLY.contains(&uuid) => "weekly",
        _ => "default",
    }
}

/// `models.is_lifetime_kind`.
pub fn is_lifetime_kind(rk: &str) -> bool {
    matches!(rk, "default" | "none")
}

/// `b.get("name") or b.get("name_id") or str(uuid)`.
pub fn board_label(b: &BoardRow) -> String {
    if !b.name.is_empty() {
        b.name.clone()
    } else if !b.name_id.is_empty() {
        b.name_id.clone()
    } else {
        b.uuid.to_string()
    }
}

// ---------------------------------------------------------------------------
// Production

pub struct Pg<'a>(pub &'a AppState);

const RENAME_COLS: &str = "id, from_name, to_name, from_anchor, to_anchor, confidence, matched_boards, \
                           evidence::text, method_version, created_at";
const DUP_COLS: &str = "name, kind, verdict, boards, max_occurrences, spellings::text, first_anchor, \
                        last_anchor, evidence::text, method_version, updated_at";

const UPSERT_RENAME: &str = "INSERT INTO player_rename (from_name, from_name_lower, to_name, \
     to_name_lower, from_anchor, to_anchor, confidence, matched_boards, \
     evidence, method_version, created_at) \
     VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9::text::jsonb, $10, $11) \
     ON CONFLICT (from_name_lower, to_name_lower, to_anchor) DO UPDATE SET \
     from_name = EXCLUDED.from_name, to_name = EXCLUDED.to_name, \
     from_anchor = EXCLUDED.from_anchor, confidence = EXCLUDED.confidence, \
     matched_boards = EXCLUDED.matched_boards, evidence = EXCLUDED.evidence, \
     method_version = EXCLUDED.method_version, created_at = EXCLUDED.created_at";

const UPSERT_DUPLICATE: &str = "INSERT INTO player_duplicate (name_lower, name, kind, verdict, boards, \
       max_occurrences, spellings, first_anchor, last_anchor, evidence, \
       method_version, updated_at) \
     VALUES ($1, $2, $3, $4, $5, $6, $7::text::jsonb, $8, $9, $10::text::jsonb, $11, $12) \
     ON CONFLICT (name_lower) DO UPDATE SET \
       name = EXCLUDED.name, \
       kind = CASE WHEN player_duplicate.kind IN ('case', 'both') \
                   THEN 'both' ELSE EXCLUDED.kind END, \
       verdict = EXCLUDED.verdict, boards = EXCLUDED.boards, \
       max_occurrences = EXCLUDED.max_occurrences, \
       first_anchor = LEAST(player_duplicate.first_anchor, EXCLUDED.first_anchor), \
       last_anchor = GREATEST(player_duplicate.last_anchor, EXCLUDED.last_anchor), \
       evidence = EXCLUDED.evidence, \
       method_version = EXCLUDED.method_version, \
       updated_at = EXCLUDED.updated_at";

const UPSERT_DATES: &str = "INSERT INTO player_duplicate (name_lower, name, kind, verdict, boards, \
       max_occurrences, first_anchor, last_anchor, updated_at) \
     VALUES ($1, $2, 'same_name', 'not_analysed', $3, $4, $5, $6, $7) \
     ON CONFLICT (name_lower) DO UPDATE SET \
       name = EXCLUDED.name, \
       kind = CASE WHEN player_duplicate.kind IN ('case', 'both') \
                   THEN 'both' ELSE 'same_name' END, \
       boards = EXCLUDED.boards, \
       max_occurrences = EXCLUDED.max_occurrences, \
       verdict = CASE WHEN player_duplicate.verdict = 'all_idle' \
                       AND player_duplicate.evidence = '{}'::jsonb \
                      THEN 'not_analysed' ELSE player_duplicate.verdict END, \
       first_anchor = LEAST(player_duplicate.first_anchor, EXCLUDED.first_anchor), \
       last_anchor = GREATEST(player_duplicate.last_anchor, EXCLUDED.last_anchor)";

/// The skip scan `list_timestamps` / `all_anchors_asc` share.
const ANCHORS: &str = "WITH RECURSIVE a AS ( \
         SELECT (SELECT max(anchor) FROM entry) AS anchor \
         UNION ALL \
         SELECT (SELECT max(e.anchor) FROM entry e WHERE e.anchor < a.anchor) \
         FROM a WHERE a.anchor IS NOT NULL \
     ) \
     SELECT anchor FROM a WHERE anchor IS NOT NULL";

fn rename_row(r: &tokio_postgres::Row) -> RenameRow {
    RenameRow {
        id: r.get(0),
        from_name: r.get(1),
        to_name: r.get(2),
        from_anchor: r.get(3),
        to_anchor: r.get(4),
        confidence: r.get(5),
        matched_boards: r.get(6),
        evidence: r.get(7),
        method_version: r.get(8),
        created_at: r.get(9),
    }
}

fn duplicate_row(r: &tokio_postgres::Row) -> DuplicateRow {
    DuplicateRow {
        name: r.get(0),
        kind: r.get(1),
        verdict: r.get(2),
        boards: r.get(3),
        max_occurrences: r.get(4),
        spellings: r.get(5),
        first_anchor: r.get(6),
        last_anchor: r.get(7),
        evidence: r.get(8),
        method_version: r.get(9),
        updated_at: r.get(10),
    }
}

/// An int array literal as asyncpg's `copy_from_query` interpolates it.
fn int_array(v: &[i32]) -> String {
    let items: Vec<String> = v.iter().map(i32::to_string).collect();
    format!("'{{{}}}'::int[]", items.join(","))
}

impl Pg<'_> {
    async fn client(&self) -> R<deadpool_postgres::Object> {
        let pool = self.0.inner.db.pg.as_ref().ok_or_else(|| anyhow!("Postgres pool not initialized"))?;
        pool.get().await.context("postgres pool")
    }

    async fn copy_bytes(&self, sql: &str) -> R<Vec<u8>> {
        let client = self.client().await?;
        let stream = client.copy_out(sql).await?;
        let chunks: Vec<bytes::Bytes> = stream.try_collect().await?;
        Ok(chunks.concat())
    }

    async fn scalar_i64(&self, sql: &str) -> R<i64> {
        Ok(self.client().await?.query_one(sql, &[]).await?.get(0))
    }

    fn redis(&self) -> Option<redis::aio::ConnectionManager> {
        self.0.inner.db.redis.clone()
    }
}

impl Store for Pg<'_> {
    async fn flag(&self, flag: &str) -> R<bool> {
        features::is_enabled(self.0, flag).await
    }

    async fn setting(&self, key: &str) -> R<Value> {
        runtime_config::get_setting(self.0, key).await
    }

    async fn all_anchors_asc(&self) -> R<Vec<i64>> {
        let rows = self.client().await?.query(&format!("{ANCHORS} ORDER BY anchor ASC"), &[]).await?;
        Ok(rows.iter().map(|r| r.get(0)).collect())
    }

    async fn latest_anchors(&self, limit: i64) -> R<Vec<i64>> {
        let rows = self.client().await?.query(&format!("{ANCHORS} LIMIT $1"), &[&limit]).await?;
        Ok(rows.iter().map(|r| r.get(0)).collect())
    }

    async fn boards_at(&self, anchor: i64) -> R<Vec<BoardRow>> {
        let rows = self
            .client()
            .await?
            .query(
                "SELECT b.uuid, b.name_id, b.name, b.reset_kind_override \
                 FROM (SELECT DISTINCT board_uuid FROM entry WHERE anchor = $1) e \
                 JOIN board b ON b.uuid = e.board_uuid",
                &[&anchor],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| BoardRow { uuid: r.get(0), name_id: r.get(1), name: r.get(2), reset_kind_override: r.get(3) })
            .collect())
    }

    async fn capture(&self, anchor: i64, uuids: &[i32]) -> R<Capture> {
        let sql = format!(
            "COPY (SELECT board_uuid, player_id, score FROM entry WHERE anchor = {anchor} AND board_uuid = ANY({})) \
             TO STDOUT (FORMAT binary)",
            int_array(uuids)
        );
        decode_copy(&self.copy_bytes(&sql).await?).ok_or_else(|| anyhow!("malformed binary COPY at {anchor}"))
    }

    async fn player_names(&self, ids: &[i64]) -> R<Vec<(i64, String)>> {
        let rows =
            self.client().await?.query("SELECT id, name FROM player WHERE id = ANY($1::bigint[])", &[&ids]).await?;
        Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    async fn upsert_renames(&self, rows: &[RenameWrite]) -> R<()> {
        if rows.is_empty() {
            return Ok(());
        }
        let mut client = self.client().await?;
        let tx = client.transaction().await?;
        let stmt = tx.prepare(UPSERT_RENAME).await?;
        for r in rows {
            let jsonb = pyjson_string(&r.evidence);
            tx.execute(
                &stmt,
                &[
                    &r.from_name,
                    &r.from_name_lower,
                    &r.to_name,
                    &r.to_name_lower,
                    &r.from_anchor,
                    &r.to_anchor,
                    &r.confidence,
                    &r.matched_boards,
                    &jsonb,
                    &r.method_version,
                    &r.created_at,
                ],
            )
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    async fn list_renames(&self, limit: i64, offset: i64) -> R<(Vec<RenameRow>, i64)> {
        let client = self.client().await?;
        let total: i64 = client.query_one("SELECT count(*) FROM player_rename", &[]).await?.get(0);
        let rows = client
            .query(
                &format!(
                    "SELECT {RENAME_COLS} FROM player_rename \
                     ORDER BY to_anchor DESC, confidence DESC, id DESC LIMIT $1 OFFSET $2"
                ),
                &[&limit, &offset],
            )
            .await?;
        Ok((rows.iter().map(rename_row).collect(), total))
    }

    async fn renames_for_name(&self, lowered: &str) -> R<Vec<RenameRow>> {
        let rows = self
            .client()
            .await?
            .query(
                &format!(
                    "SELECT {RENAME_COLS} FROM player_rename \
                     WHERE from_name_lower = $1 OR to_name_lower = $1 \
                     ORDER BY to_anchor DESC, id DESC"
                ),
                &[&lowered],
            )
            .await?;
        Ok(rows.iter().map(rename_row).collect())
    }

    async fn count_renames(&self) -> R<i64> {
        self.scalar_i64("SELECT count(*) FROM player_rename").await
    }

    async fn delete_all_renames(&self) -> R<i64> {
        Ok(self.client().await?.execute("DELETE FROM player_rename", &[]).await? as i64)
    }

    async fn duplicate_groups_at(&self, anchor: i64) -> R<Vec<DupGroup>> {
        let sql = format!(
            "COPY (SELECT board_uuid, player_id, rank, score FROM entry WHERE anchor = {anchor}) TO STDOUT (FORMAT binary)"
        );
        let client = self.client().await?;
        let found = match duplicate_groups(&self.copy_bytes(&sql).await?) {
            Some(found) => found,
            None => client
                .query(
                    "SELECT player_id, board_uuid, \
                            count(DISTINCT (rank, score))::int AS occurrences \
                     FROM entry WHERE anchor = $1 \
                     GROUP BY player_id, board_uuid \
                     HAVING count(DISTINCT (rank, score)) > 1 \
                     ORDER BY player_id, board_uuid",
                    &[&anchor],
                )
                .await?
                .iter()
                .map(|r| (r.get(0), r.get(1), r.get(2)))
                .collect(),
        };
        if found.is_empty() {
            return Ok(Vec::new());
        }
        let mut ids: Vec<i64> = found.iter().map(|g| g.0).collect();
        ids.sort_unstable();
        ids.dedup();
        let names: HashMap<i64, String> = client
            .query("SELECT id, name FROM player WHERE id = ANY($1::bigint[])", &[&ids])
            .await?
            .iter()
            .map(|r| (r.get(0), r.get(1)))
            .collect();
        Ok(found
            .into_iter()
            .map(|(pid, board_uuid, occurrences)| DupGroup {
                name: names.get(&pid).cloned().unwrap_or_else(|| pid.to_string()),
                board_uuid,
                occurrences,
            })
            .collect())
    }

    async fn player_rows_window(&self, name: &str, start: i64, end: i64) -> R<Vec<WindowRow>> {
        let client = self.client().await?;
        let who = client
            .query_opt("SELECT id, name FROM player WHERE name_lower = lower($1)", &[&kiwi_core::pystr::strip(name)])
            .await?;
        let Some(who) = who else { return Ok(Vec::new()) };
        let id: i64 = who.get(0);
        let rows = client
            .query(
                "SELECT board_uuid AS leaderboard, anchor AS created_at, rank, score \
                 FROM entry WHERE player_id = $1 AND anchor >= $2 \
                 AND NOT (board_uuid = ANY($3::int[])) AND anchor <= $4",
                &[&id, &start, &NON_PLAYER_BOARDS.to_vec(), &end],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| WindowRow { leaderboard: r.get(0), created_at: r.get(1), rank: r.get(2), score: r.get(3) })
            .collect())
    }

    async fn upsert_duplicates(&self, rows: &[DuplicateWrite]) -> R<()> {
        if rows.is_empty() {
            return Ok(());
        }
        let mut client = self.client().await?;
        let tx = client.transaction().await?;
        let stmt = tx.prepare(UPSERT_DUPLICATE).await?;
        for r in rows {
            let (spellings, evidence) = (pyjson_string(&r.spellings), pyjson_string(&r.evidence));
            tx.execute(
                &stmt,
                &[
                    &r.name_lower,
                    &r.name,
                    &r.kind,
                    &r.verdict,
                    &r.boards,
                    &r.max_occurrences,
                    &spellings,
                    &r.first_anchor,
                    &r.last_anchor,
                    &evidence,
                    &r.method_version,
                    &r.updated_at,
                ],
            )
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    async fn upsert_duplicate_dates(&self, rows: &[DateWrite]) -> R<()> {
        if rows.is_empty() {
            return Ok(());
        }
        let mut client = self.client().await?;
        let tx = client.transaction().await?;
        let stmt = tx.prepare(UPSERT_DATES).await?;
        for r in rows {
            tx.execute(
                &stmt,
                &[
                    &r.name_lower,
                    &r.name,
                    &r.boards,
                    &r.max_occurrences,
                    &r.first_anchor,
                    &r.last_anchor,
                    &r.updated_at,
                ],
            )
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    async fn list_duplicates(&self, limit: i64, offset: i64, kind: Option<&str>) -> R<(Vec<DuplicateRow>, i64, i64)> {
        let client = self.client().await?;
        let latest = "last_anchor = (SELECT MAX(last_anchor) FROM player_duplicate)";
        let order = "ORDER BY last_anchor DESC, boards DESC, name_lower ASC";
        let (total, current, rows) = match kind {
            Some(kind) => {
                let w = " WHERE (kind = $1 OR kind = 'both')";
                let total: i64 =
                    client.query_one(&format!("SELECT count(*) FROM player_duplicate{w}"), &[&kind]).await?.get(0);
                let current: i64 = client
                    .query_one(&format!("SELECT count(*) FROM player_duplicate{w} AND {latest}"), &[&kind])
                    .await?
                    .get(0);
                let sql = format!("SELECT {DUP_COLS} FROM player_duplicate{w} {order} LIMIT $2 OFFSET $3");
                (total, current, client.query(&sql, &[&kind, &limit, &offset]).await?)
            }
            None => {
                let total: i64 = client.query_one("SELECT count(*) FROM player_duplicate", &[]).await?.get(0);
                let current: i64 = client
                    .query_one(&format!("SELECT count(*) FROM player_duplicate WHERE {latest}"), &[])
                    .await?
                    .get(0);
                let sql = format!("SELECT {DUP_COLS} FROM player_duplicate {order} LIMIT $1 OFFSET $2");
                (total, current, client.query(&sql, &[&limit, &offset]).await?)
            }
        };
        Ok((rows.iter().map(duplicate_row).collect(), total, current))
    }

    async fn latest_duplicate_anchor(&self) -> R<Option<i64>> {
        Ok(self.client().await?.query_one("SELECT MAX(last_anchor) FROM player_duplicate", &[]).await?.get(0))
    }

    async fn duplicate_for_name(&self, name: &str) -> R<Option<DuplicateRow>> {
        let row = self
            .client()
            .await?
            .query_opt(
                &format!("SELECT {DUP_COLS} FROM player_duplicate WHERE name_lower = lower($1)"),
                &[&kiwi_core::pystr::strip(name)],
            )
            .await?;
        Ok(row.as_ref().map(duplicate_row))
    }

    async fn count_duplicates(&self) -> R<i64> {
        self.scalar_i64("SELECT count(*) FROM player_duplicate").await
    }

    async fn delete_all_duplicates(&self) -> R<i64> {
        Ok(self.client().await?.execute("DELETE FROM player_duplicate", &[]).await? as i64)
    }

    async fn lock(&self, key: &str) -> Option<bool> {
        let mut conn = self.redis()?;
        let got: redis::RedisResult<Option<String>> =
            redis::cmd("SET").arg(key).arg("1").arg("NX").arg("EX").arg(900).query_async(&mut conn).await;
        // redis-py raises here; the job never starts.
        Some(matches!(got, Ok(Some(_))))
    }

    async fn heartbeat(&self, key: &str) {
        if let Some(mut conn) = self.redis() {
            let _: redis::RedisResult<i64> = redis::cmd("EXPIRE").arg(key).arg(900).query_async(&mut conn).await;
        }
    }

    async fn unlock(&self, key: &str) {
        if let Some(mut conn) = self.redis() {
            let _: redis::RedisResult<i64> = redis::cmd("DEL").arg(key).query_async(&mut conn).await;
        }
    }

    async fn save_status(&self, key: &str, json: &str) {
        if let Some(mut conn) = self.redis() {
            let _: redis::RedisResult<()> =
                redis::cmd("SET").arg(key).arg(json).arg("EX").arg(86400).query_async(&mut conn).await;
        }
    }

    async fn load_status(&self, key: &str) -> Option<String> {
        let mut conn = self.redis()?;
        redis::cmd("GET").arg(key).query_async::<Option<String>>(&mut conn).await.ok().flatten()
    }
}

/// What asyncpg's jsonb codec (`json.dumps`) sends for an already-dumped
/// string: a JSON string literal, so the row stores a jsonb string.
fn pyjson_string(dumped: &str) -> String {
    crate::pyjson::dumps_spaced(&Value::String(dumped.to_owned()))
}

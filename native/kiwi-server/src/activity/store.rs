//! The Postgres reads and writes of `app/trove/leaderboards/pg_store.py` that
//! the activity estimates use, with the same SQL.

use std::collections::{BTreeMap, HashMap};

use anyhow::Context;
use deadpool_postgres::Pool;
use futures::TryStreamExt;
use kiwi_core::activity::{Capture, decode_copy};

/// `list_timestamps(limit, since=)`: distinct anchors, newest first.
pub async fn list_timestamps(pg: &Pool, limit: i64, since: Option<i64>) -> anyhow::Result<Vec<i64>> {
    let client = pg.get().await?;
    let rows = match since {
        None => {
            client
                .query(
                    "WITH RECURSIVE a AS (
                        SELECT (SELECT max(anchor) FROM entry) AS anchor
                        UNION ALL
                        SELECT (SELECT max(e.anchor) FROM entry e WHERE e.anchor < a.anchor)
                        FROM a WHERE a.anchor IS NOT NULL
                    )
                    SELECT anchor FROM a WHERE anchor IS NOT NULL LIMIT $1",
                    &[&limit],
                )
                .await?
        }
        Some(since) => {
            client
                .query(
                    "WITH RECURSIVE a AS (
                        SELECT (SELECT max(anchor) FROM entry WHERE anchor >= $2) AS anchor
                        UNION ALL
                        SELECT (SELECT max(e.anchor) FROM entry e
                                WHERE e.anchor < a.anchor AND e.anchor >= $2)
                        FROM a WHERE a.anchor IS NOT NULL
                    )
                    SELECT anchor FROM a WHERE anchor IS NOT NULL LIMIT $1",
                    &[&limit, &since],
                )
                .await?
        }
    };
    Ok(rows.iter().map(|r| r.get(0)).collect())
}

/// A board as `list_boards_at` returns it (the fields activity reads).
#[derive(Clone, Debug)]
pub struct Board {
    pub uuid: i32,
    pub name: String,
    pub category: String,
}

/// `list_boards_at(anchor)` in its `ORDER BY category, name` order.
pub async fn list_boards_at(pg: &Pool, anchor: i64) -> anyhow::Result<Vec<Board>> {
    let rows = crate::lbread::store::board_rows_at(&pg.get().await?, anchor).await?;
    Ok(rows.iter().map(|r| Board { uuid: r.get("uuid"), name: r.get("name"), category: r.get("category") }).collect())
}

/// Every board uuid (`all_boards()`, table order).
pub async fn all_board_uuids(pg: &Pool) -> anyhow::Result<Vec<i32>> {
    let client = pg.get().await?;
    let rows = client.query("SELECT uuid, name, category, reset_kind_override FROM board", &[]).await?;
    Ok(rows.iter().map(|r| r.get(0)).collect())
}

/// `anchor_capture`: one capture's `(board, player id, score)` rows over a
/// binary COPY. asyncpg inlines the arguments the same way.
pub async fn capture(pg: &Pool, anchor: i64, boards: &[i32]) -> anyhow::Result<Capture> {
    let list = boards.iter().map(i32::to_string).collect::<Vec<_>>().join(",");
    let sql = format!(
        "COPY (SELECT board_uuid, player_id, score FROM entry \
         WHERE anchor = {anchor} AND board_uuid = ANY('{{{list}}}'::int[])) TO STDOUT (FORMAT binary)"
    );
    let client = pg.get().await?;
    let stream = client.copy_out(sql.as_str()).await?;
    futures::pin_mut!(stream);
    let mut buf = Vec::new();
    while let Some(chunk) = stream.try_next().await? {
        buf.extend_from_slice(&chunk);
    }
    decode_copy(&buf).context("entry COPY did not decode")
}

/// `player_names(ids)`: `{id: name}`.
pub async fn player_names(pg: &Pool, ids: &[i64]) -> anyhow::Result<HashMap<i64, String>> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let client = pg.get().await?;
    let rows = client.query("SELECT id, name FROM player WHERE id = ANY($1::bigint[])", &[&ids]).await?;
    Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
}

/// Python `str.lower()` of the non-empty names, deduplicated and sorted.
pub fn lowered<'a>(names: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut out: Vec<String> = names.into_iter().filter(|n| !n.is_empty()).map(str::to_lowercase).collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// `record_active_window`: replace one window's active players.
pub async fn record_active_window(pg: &Pool, window_end: i64, names: &[String]) -> anyhow::Result<()> {
    let mut client = pg.get().await?;
    let tx = client.transaction().await?;
    tx.execute("DELETE FROM activity_active WHERE window_end = $1", &[&window_end]).await?;
    if !names.is_empty() {
        tx.execute(
            "INSERT INTO activity_active (window_end, player_lower) \
             SELECT $1::bigint, n FROM unnest($2::text[]) AS n ON CONFLICT DO NOTHING",
            &[&window_end, &names],
        )
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// `count_active_since(early, late)`.
pub async fn count_active_since(pg: &Pool, early: i64, late: i64) -> anyhow::Result<i64> {
    let client = pg.get().await?;
    let row = client
        .query_one(
            "SELECT COUNT(DISTINCT player_lower) FROM activity_active \
             WHERE window_end > $1 AND window_end <= $2",
            &[&early, &late],
        )
        .await?;
    Ok(row.get::<_, Option<i64>>(0).unwrap_or(0))
}

async fn exec(pg: &Pool, sql: &str, params: &[&(dyn tokio_postgres::types::ToSql + Sync)]) -> anyhow::Result<u64> {
    let client = pg.get().await?;
    Ok(client.execute(sql, params).await?)
}

pub async fn prune_active_windows(pg: &Pool, cutoff: i64) -> anyhow::Result<u64> {
    exec(pg, "DELETE FROM activity_active WHERE window_end < $1", &[&cutoff]).await
}

pub async fn delete_all_active_windows(pg: &Pool) -> anyhow::Result<u64> {
    exec(pg, "DELETE FROM activity_active", &[]).await
}

/// One `activity_estimate` row.
#[derive(Clone, Debug, PartialEq)]
pub struct Estimate {
    pub window_end: i64,
    pub window_start: i64,
    pub duration_hours: f64,
    pub estimate: i32,
    pub boards_analyzed: i32,
    pub computed_at: i64,
    pub estimate_24h: Option<i32>,
}

pub async fn upsert_estimate(pg: &Pool, e: &Estimate) -> anyhow::Result<()> {
    exec(
        pg,
        "INSERT INTO activity_estimate \
         (window_end, window_start, duration_hours, estimate, boards_analyzed, computed_at, \
          estimate_24h) \
         VALUES ($1, $2, $3, $4, $5, $6, $7) \
         ON CONFLICT (window_end) DO UPDATE SET \
         window_start = EXCLUDED.window_start, duration_hours = EXCLUDED.duration_hours, \
         estimate = EXCLUDED.estimate, boards_analyzed = EXCLUDED.boards_analyzed, \
         computed_at = EXCLUDED.computed_at, estimate_24h = EXCLUDED.estimate_24h",
        &[
            &e.window_end,
            &e.window_start,
            &e.duration_hours,
            &e.estimate,
            &e.boards_analyzed,
            &e.computed_at,
            &e.estimate_24h,
        ],
    )
    .await?;
    Ok(())
}

/// `get_estimates(window_start)`, ascending by window end.
pub async fn get_estimates(pg: &Pool, window_start: Option<i64>) -> anyhow::Result<Vec<Estimate>> {
    let client = pg.get().await?;
    let rows = match window_start {
        None => {
            client
                .query(
                    "SELECT window_end, window_start, duration_hours, estimate, boards_analyzed, \
                     computed_at, estimate_24h FROM activity_estimate ORDER BY window_end",
                    &[],
                )
                .await?
        }
        Some(ws) => {
            client
                .query(
                    "SELECT window_end, window_start, duration_hours, estimate, boards_analyzed, \
                     computed_at, estimate_24h FROM activity_estimate WHERE window_end >= $1 \
                     ORDER BY window_end",
                    &[&ws],
                )
                .await?
        }
    };
    Ok(rows
        .iter()
        .map(|r| Estimate {
            window_end: r.get(0),
            window_start: r.get(1),
            duration_hours: r.get(2),
            estimate: r.get(3),
            boards_analyzed: r.get(4),
            computed_at: r.get(5),
            estimate_24h: r.get(6),
        })
        .collect())
}

pub async fn delete_all_estimates(pg: &Pool) -> anyhow::Result<u64> {
    exec(pg, "DELETE FROM activity_estimate", &[]).await
}

/// One `class_activity_estimate` row.
#[derive(Clone, Debug, PartialEq)]
pub struct ClassEstimate {
    pub class_index: i32,
    pub window_end: i64,
    pub window_start: i64,
    pub duration_hours: f64,
    pub estimate: i32,
    pub estimate_clean: Option<i32>,
    pub computed_at: i64,
    pub estimate_24h: Option<i32>,
    pub estimate_24h_clean: Option<i32>,
}

/// `upsert_class_estimates(rows)`.
pub async fn upsert_class_estimates(pg: &Pool, rows: &[ClassEstimate]) -> anyhow::Result<()> {
    if rows.is_empty() {
        return Ok(());
    }
    let mut client = pg.get().await?;
    let tx = client.transaction().await?;
    let stmt = tx
        .prepare(
            "INSERT INTO class_activity_estimate \
             (class_index, window_end, window_start, duration_hours, estimate, \
              estimate_clean, computed_at, estimate_24h, estimate_24h_clean) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) \
             ON CONFLICT (class_index, window_end) DO UPDATE SET \
             window_start = EXCLUDED.window_start, duration_hours = EXCLUDED.duration_hours, \
             estimate = EXCLUDED.estimate, estimate_clean = EXCLUDED.estimate_clean, \
             computed_at = EXCLUDED.computed_at, estimate_24h = EXCLUDED.estimate_24h, \
             estimate_24h_clean = EXCLUDED.estimate_24h_clean",
        )
        .await?;
    for r in rows {
        tx.execute(
            &stmt,
            &[
                &r.class_index,
                &r.window_end,
                &r.window_start,
                &r.duration_hours,
                &r.estimate,
                &r.estimate_clean,
                &r.computed_at,
                &r.estimate_24h,
                &r.estimate_24h_clean,
            ],
        )
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// `record_class_active_window`: `rows` are `(class, player_lower, clean)`
/// with each `(class, player_lower)` once (clean OR-ed, as the upsert does).
pub async fn record_class_active_window(
    pg: &Pool,
    window_end: i64,
    rows: &[(i32, String, bool)],
) -> anyhow::Result<()> {
    let mut client = pg.get().await?;
    let tx = client.transaction().await?;
    tx.execute("DELETE FROM class_activity_active WHERE window_end = $1", &[&window_end]).await?;
    if !rows.is_empty() {
        let classes: Vec<i32> = rows.iter().map(|r| r.0).collect();
        let names: Vec<&str> = rows.iter().map(|r| r.1.as_str()).collect();
        let clean: Vec<bool> = rows.iter().map(|r| r.2).collect();
        tx.execute(
            "INSERT INTO class_activity_active (window_end, class_index, player_lower, clean) \
             SELECT $1::bigint, c, n, k FROM unnest($2::int[], $3::text[], $4::bool[]) AS u(c, n, k) \
             ON CONFLICT (window_end, class_index, player_lower) \
             DO UPDATE SET clean = class_activity_active.clean OR EXCLUDED.clean",
            &[&window_end, &classes, &names, &clean],
        )
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Per class over `(early, late]`: distinct raw/clean players and how many
/// windows were measurable (`class_rollup_since`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClassRollup {
    pub raw: i64,
    pub clean: i64,
    pub windows: i64,
    pub clean_windows: i64,
}

pub async fn class_rollup_since(pg: &Pool, early: i64, late: i64) -> anyhow::Result<BTreeMap<i64, ClassRollup>> {
    let client = pg.get().await?;
    let sets = client
        .query(
            "SELECT class_index, COUNT(DISTINCT player_lower) AS raw, \
                    COUNT(DISTINCT player_lower) FILTER (WHERE clean) AS clean \
             FROM class_activity_active WHERE window_end > $1 AND window_end <= $2 \
             GROUP BY class_index",
            &[&early, &late],
        )
        .await?;
    let windows = client
        .query(
            "SELECT class_index, count(*) AS windows, count(estimate_clean) AS clean_windows \
             FROM class_activity_estimate WHERE window_end > $1 AND window_end <= $2 \
             GROUP BY class_index",
            &[&early, &late],
        )
        .await?;
    let mut out: BTreeMap<i64, ClassRollup> = windows
        .iter()
        .map(|r| {
            let i: i32 = r.get(0);
            (i64::from(i), ClassRollup { raw: 0, clean: 0, windows: r.get(1), clean_windows: r.get(2) })
        })
        .collect();
    for r in &sets {
        let i: i32 = r.get(0);
        if let Some(o) = out.get_mut(&i64::from(i)) {
            o.raw = r.get(1);
            o.clean = r.get(2);
        }
    }
    Ok(out)
}

pub async fn prune_class_active_windows(pg: &Pool, cutoff: i64) -> anyhow::Result<u64> {
    exec(pg, "DELETE FROM class_activity_active WHERE window_end < $1", &[&cutoff]).await
}

pub async fn delete_all_class_active_windows(pg: &Pool) -> anyhow::Result<u64> {
    exec(pg, "DELETE FROM class_activity_active", &[]).await
}

/// `get_class_estimates(window_start)`, by window end then class.
pub async fn get_class_estimates(pg: &Pool, window_start: Option<i64>) -> anyhow::Result<Vec<ClassEstimate>> {
    const COLS: &str = "class_index, window_end, window_start, duration_hours, estimate, \
                        estimate_clean, computed_at, estimate_24h, estimate_24h_clean";
    let client = pg.get().await?;
    let rows = match window_start {
        None => {
            client
                .query(&format!("SELECT {COLS} FROM class_activity_estimate ORDER BY window_end, class_index"), &[])
                .await?
        }
        Some(ws) => {
            client
                .query(
                    &format!(
                        "SELECT {COLS} FROM class_activity_estimate WHERE window_end >= $1 \
                         ORDER BY window_end, class_index"
                    ),
                    &[&ws],
                )
                .await?
        }
    };
    Ok(rows
        .iter()
        .map(|r| ClassEstimate {
            class_index: r.get(0),
            window_end: r.get(1),
            window_start: r.get(2),
            duration_hours: r.get(3),
            estimate: r.get(4),
            estimate_clean: r.get(5),
            computed_at: r.get(6),
            estimate_24h: r.get(7),
            estimate_24h_clean: r.get(8),
        })
        .collect())
}

pub async fn delete_class_estimate(pg: &Pool, window_end: i64) -> anyhow::Result<u64> {
    exec(pg, "DELETE FROM class_activity_estimate WHERE window_end = $1", &[&window_end]).await
}

pub async fn delete_all_class_estimates(pg: &Pool) -> anyhow::Result<u64> {
    exec(pg, "DELETE FROM class_activity_estimate", &[]).await
}

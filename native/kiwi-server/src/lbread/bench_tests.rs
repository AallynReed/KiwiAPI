//! The warmer's board-list and chart reads against the queries they replaced,
//! on a synthetic history of 6-hourly captures: same rows, then timings and
//! buffer counts. Opt-in and DESTRUCTIVE (drops and reseeds the leaderboards
//! tables), so only ever against a throwaway postgres:17:
//!
//!   docker run --rm -d --name kp-bench -p 127.0.0.1:5439:5432 -e POSTGRES_PASSWORD=bench postgres:17
//!   POSTGRES_DSN=postgresql://postgres:bench@127.0.0.1:5439/postgres KIWI_BENCH_SCRATCH=1 \
//!     cargo test -p kiwi-server --release lbread_warmer_bench -- --ignored --nocapture
//!
//! KIWI_BENCH_SCALE (default 1, ~3.6M rows) multiplies the board sizes; each
//! capture is stored player by player, or board by board with KIWI_BENCH_LAYOUT=board.

use std::time::Instant;

use futures::{StreamExt, stream};
use kiwi_core::lbread::{DAY, trove_day_start};
use serde_json::Value;

use super::{
    Live, service,
    store::{self, SeriesRow, TopMeta, TopSeries},
};
use crate::{config::Settings, lb_ingest::store::partition_ddl, state::AppState};

const OLD_ANCHORS: &str = "SELECT DISTINCT anchor FROM entry WHERE board_uuid = $1 AND anchor >= $2 ORDER BY anchor";
const BOARDS: i32 = 90;
const DAYS: i64 = 10;
const CAPTURE_OFFSETS: [i64; 4] = [21_120, 42_960, 64_320, 85_860];

type Client = deadpool_postgres::Client;

async fn anchors(c: &Client, sql: &str, uuid: i32, ws: i64) -> Vec<i64> {
    c.query(sql, &[&uuid, &ws]).await.unwrap().iter().map(|r| r.get(0)).collect()
}

/// `board_top_series` with the given anchors query (the rest is the store's SQL).
async fn top_series(c: &Client, anchors_sql: &str, uuid: i32, ws: i64, top: i64) -> TopSeries {
    let anchors = anchors(c, anchors_sql, uuid, ws).await;
    let Some(&latest) = anchors.last() else { return TopSeries::default() };
    let top: Vec<TopMeta> = c
        .query(store::SERIES_TOP, &[&uuid, &latest, &top])
        .await
        .unwrap()
        .iter()
        .map(|r| TopMeta { player_id: r.get(0), player_name: r.get(1), rank: i64::from(r.get::<_, i32>(2)) })
        .collect();
    let ids: Vec<i64> = top.iter().map(|m| m.player_id).collect();
    let rows = c
        .query(store::SERIES_ROWS, &[&uuid, &ws, &ids])
        .await
        .unwrap()
        .iter()
        .map(|r| SeriesRow {
            player_id: r.get(0),
            created_at: r.get(1),
            rank: i64::from(r.get::<_, i32>(2)),
            score: r.get(3),
        })
        .collect();
    TopSeries { anchors, top, rows }
}

async fn board_list(c: &Client, anchor: i64) -> Vec<Vec<String>> {
    let fmt = |r: &tokio_postgres::Row| {
        vec![
            r.get::<_, i32>("uuid").to_string(),
            r.get("name"),
            r.get("category"),
            format!("{:?}", r.get::<_, Option<String>>("contest_type")),
        ]
    };
    let old: Vec<Vec<String>> = c.query(store::LIST_BOARDS_AT, &[&anchor]).await.unwrap().iter().map(fmt).collect();
    let new: Vec<Vec<String>> = store::board_rows_at(c, anchor).await.unwrap().iter().map(fmt).collect();
    assert_eq!(old, new, "board list at {anchor}");
    new
}

/// Shared buffers (hit + read) the statement touches, from EXPLAIN ANALYZE.
async fn buffers(c: &Client, sql: &str, params: &[i64]) -> i64 {
    let mut sql = sql.to_owned();
    for (i, v) in params.iter().enumerate().rev() {
        sql = sql.replace(&format!("${}", i + 1), &v.to_string());
    }
    let msgs = c.simple_query(&format!("EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) {sql}")).await.unwrap();
    let text = msgs
        .iter()
        .find_map(|m| match m {
            tokio_postgres::SimpleQueryMessage::Row(r) => r.get(0).map(str::to_owned),
            _ => None,
        })
        .unwrap();
    let plan: Value = serde_json::from_str(&text).unwrap();
    let p = &plan[0]["Plan"];
    p["Shared Hit Blocks"].as_i64().unwrap_or(0) + p["Shared Read Blocks"].as_i64().unwrap_or(0)
}

async fn seed(c: &Client, now: i64, scale: i64, board_major: bool) -> Vec<i64> {
    c.batch_execute("DROP TABLE IF EXISTS entry, board, board_contest, player CASCADE").await.unwrap();
    c.batch_execute(include_str!("../startup/leaderboards.sql")).await.unwrap();
    let today = trove_day_start(now);
    let mut anchors = Vec::new();
    for d in (0..DAYS).rev() {
        let lo = today - d * DAY;
        c.batch_execute(&partition_ddl(lo)).await.unwrap();
        anchors.extend(CAPTURE_OFFSETS.iter().map(|o| lo + o).filter(|&a| a < now));
    }
    let players = 5000 * scale;
    let sql = format!(
        "INSERT INTO board (uuid, name_id, name, category_id, category)
         SELECT i, 'LB_' || i, 'BOARD ' || lpad(i::text, 3, '0'), 'CAT_' || (i % 7),
                (ARRAY['Mastery','Stats','Delves','Daily','Power Rank','Effort','Clubs'])[i % 7 + 1]
         FROM generate_series(1, {BOARDS}) i;
         INSERT INTO player (name, name_lower) SELECT 'Bench' || g, 'bench' || g FROM generate_series(1, {players}) g;
         CREATE TEMP TABLE bench_anchors AS
           SELECT k::int AS k, a AS anchor FROM unnest('{{{list}}}'::bigint[]) WITH ORDINALITY AS u(a, k);
         INSERT INTO entry (board_uuid, anchor, rank, score, player_id)
         SELECT b, a.anchor,
                CASE WHEN b = 7 AND k <= 20 THEN 1 ELSE k END,
                CASE WHEN b = 7 AND k <= 20 THEN 1e9 ELSE (s.sz - k) * 1000.0 + ((a.k * 13 + k * 7) % 997) END,
                ((k + b * 211 + a.k * 3) % {players}) + 1
         FROM bench_anchors a
         CROSS JOIN generate_series(1, {BOARDS}) b
         CROSS JOIN LATERAL (SELECT (200 + (b * 97) % 1600) * {scale} AS sz) s
         CROSS JOIN LATERAL generate_series(1, s.sz) k
         ORDER BY a.anchor, {order};
         INSERT INTO entry (board_uuid, anchor, rank, score, player_id)
         SELECT 3, a.anchor, (200 + 291) * {scale} + 1, 0, ((2 + 3 * 211 + a.k * 3) % {players}) + 1
         FROM bench_anchors a;
         INSERT INTO board_contest (board_uuid, anchor, type) SELECT 5, max(anchor), 'weekly' FROM bench_anchors;
         DROP TABLE bench_anchors;",
        list = anchors.iter().map(i64::to_string).collect::<Vec<_>>().join(","),
        order = if board_major { "1, 3" } else { "5" },
    );
    c.batch_execute(&sql).await.unwrap();
    c.batch_execute("VACUUM ANALYZE entry").await.unwrap();
    c.batch_execute("ANALYZE board; ANALYZE player; ANALYZE board_contest").await.unwrap();
    anchors
}

async fn histories(state: &AppState, now: i64, warm: bool) -> (usize, f64) {
    let live = Live(state);
    let t = Instant::now();
    let n = stream::iter(1..=BOARDS)
        .map(|uuid| {
            let live = &live;
            async move {
                if warm {
                    service::warm_board_history(live, uuid, 7, 5, now).await.unwrap()
                } else {
                    service::board_history(live, uuid, 7, 5, now).await.unwrap()
                }
            }
        })
        .buffer_unordered(6)
        .count()
        .await;
    (n, t.elapsed().as_secs_f64() * 1000.0)
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "drops and reseeds the leaderboards tables of POSTGRES_DSN"]
async fn lbread_warmer_bench() {
    assert_eq!(std::env::var("KIWI_BENCH_SCRATCH").as_deref(), Ok("1"), "set KIWI_BENCH_SCRATCH=1 on a throwaway db");
    if std::env::var("PY_UPSTREAM").is_err() {
        // SAFETY: only this ignored test runs in its process invocation.
        unsafe { std::env::set_var("PY_UPSTREAM", "http://127.0.0.1:9") };
    }
    let scale: i64 = std::env::var("KIWI_BENCH_SCALE").ok().and_then(|v| v.parse().ok()).unwrap_or(1);
    let state = AppState::new(Settings::from_env().unwrap()).await.unwrap();
    let c = state.pg().get().await.unwrap();
    let db: String = c.query_one("SELECT current_database()", &[]).await.unwrap().get(0);
    assert_ne!(db, "kiwi_lb", "refusing to reseed what looks like the production database");

    let now = chrono::Utc::now().timestamp();
    let t = Instant::now();
    let board_major = std::env::var("KIWI_BENCH_LAYOUT").as_deref() == Ok("board");
    let caps = seed(&c, now, scale, board_major).await;
    let rows: i64 = c.query_one("SELECT count(*) FROM entry", &[]).await.unwrap().get(0);
    println!("seeded {rows} entries, {} captures, {BOARDS} boards in {:.1}s", caps.len(), t.elapsed().as_secs_f64());

    let ws = now - 7 * DAY;
    for uuid in 1..=BOARDS {
        assert_eq!(anchors(&c, OLD_ANCHORS, uuid, ws).await, anchors(&c, store::SERIES_ANCHORS, uuid, ws).await);
        let old = top_series(&c, OLD_ANCHORS, uuid, ws, 5).await;
        assert_eq!(old, top_series(&c, store::SERIES_ANCHORS, uuid, ws, 5).await, "board {uuid}");
        assert!(old.anchors.len() >= 27 && old.top.len() == 5, "board {uuid}: {old:?}");
    }
    let newest = *caps.last().unwrap();
    for &anchor in [newest, caps[caps.len() - 5], caps[0]].iter() {
        assert_eq!(board_list(&c, anchor).await.len(), BOARDS as usize);
    }
    assert_eq!(board_list(&c, newest + 1).await.len(), 0);
    c.batch_execute(&format!(
        "INSERT INTO board SELECT 91, 'LB_TIE', name, category_id, category, NULL FROM board WHERE uuid = 1;
         INSERT INTO entry (board_uuid, anchor, rank, score, player_id) VALUES (91, {newest}, 1, 5, 1)"
    ))
    .await
    .unwrap();
    assert_eq!(board_list(&c, newest).await.len(), BOARDS as usize + 1);
    c.batch_execute("DELETE FROM entry WHERE board_uuid = 91; DELETE FROM board WHERE uuid = 91").await.unwrap();
    println!("same anchors, charts and board lists for every board");

    let big: i32 = (1..=BOARDS).max_by_key(|b| (200 + (b * 97) % 1600, -b)).unwrap();
    for (what, sql) in [("old", OLD_ANCHORS), ("new", store::SERIES_ANCHORS)] {
        anchors(&c, sql, 1, ws).await;
        let t = Instant::now();
        for uuid in 1..=BOARDS {
            anchors(&c, sql, uuid, ws).await;
        }
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        let buf = buffers(&c, sql, &[i64::from(big), ws]).await;
        println!("chart anchors, {what}: {ms:.1} ms for {BOARDS} boards; board {big}: {buf} buffers");
    }
    for (what, sql) in [("old", store::LIST_BOARDS_AT), ("new", store::LIST_BOARDS_AT_PROBE)] {
        c.query(sql, &[&newest]).await.unwrap();
        let t = Instant::now();
        c.query(sql, &[&newest]).await.unwrap();
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        let buf = buffers(&c, sql, &[newest]).await;
        println!("board list, {what}: {ms:.1} ms, {buf} buffers");
    }

    let ids: Vec<i64> =
        top_series(&c, store::SERIES_ANCHORS, big, ws, 5).await.top.iter().map(|m| m.player_id).collect();
    let plan = c
        .simple_query(&format!(
            "EXPLAIN {}",
            store::SERIES_ROWS.replace("$1", &big.to_string()).replace("$2", &ws.to_string()).replace(
                "$3",
                &format!("ARRAY[{}]::bigint[]", ids.iter().map(i64::to_string).collect::<Vec<_>>().join(","))
            )
        ))
        .await
        .unwrap();
    println!("chart rows plan, board {big}:");
    for m in &plan {
        if let tokio_postgres::SimpleQueryMessage::Row(r) = m {
            println!("  {}", r.get(0).unwrap_or(""));
        }
    }

    let shared = state.clone();
    let custom = state.with_pg(crate::db::pg_pool(8, Some("-c plan_cache_mode=force_custom_plan")).unwrap().unwrap());
    let mode: String = custom.pg().get().await.unwrap().query_one("SHOW plan_cache_mode", &[]).await.unwrap().get(0);
    assert_eq!(mode, "force_custom_plan");
    for (what, s) in [("shared pool", &shared), ("warmer pool", &custom)] {
        histories(s, now, false).await;
        let (n, ms) = histories(s, now, false).await;
        println!("board_history x{n} (6 at a time), {what}: {ms:.1} ms");
    }

    store::reset_charts();
    let (n, first) = histories(&custom, now, true).await;
    let (_, again) = histories(&custom, now + 1800, true).await;
    println!("warmer charts x{n}: {first:.1} ms reading every board, {again:.1} ms half an hour later");
    let live = Live(&custom);
    let later = now + 6 * 3600;
    for uuid in 1..=BOARDS {
        let reused = service::warm_board_history(&live, uuid, 7, 5, later).await.unwrap();
        assert_eq!(reused, service::board_history(&live, uuid, 7, 5, later).await.unwrap(), "board {uuid}");
    }
    let edited = caps[caps.len() - 3];
    c.batch_execute(&format!("UPDATE entry SET score = score + 1 WHERE board_uuid = 1 AND anchor = {edited}"))
        .await
        .unwrap();
    let stale = service::warm_board_history(&live, 1, 7, 5, later).await.unwrap();
    assert_ne!(stale, service::board_history(&live, 1, 7, 5, later).await.unwrap(), "the edit is not in the chart");
    store::entries_written(Some(edited));
    let fresh = service::warm_board_history(&live, 1, 7, 5, later).await.unwrap();
    assert_eq!(fresh, service::board_history(&live, 1, 7, 5, later).await.unwrap());
    println!("reused charts match fresh reads; a recorded write forces a fresh read");

    histories(&custom, later, true).await;
    let next = newest + 6 * 3600;
    c.batch_execute(&partition_ddl(next)).await.unwrap();
    c.batch_execute(&format!(
        "INSERT INTO entry (board_uuid, anchor, rank, score, player_id)
         SELECT b, {next}, k, (s.sz - k) * 1000.0 + k % 997, ((k + b * 211 + {kn} * 3) % {players}) + 1
         FROM generate_series(1, {BOARDS}) b
         CROSS JOIN LATERAL (SELECT (200 + (b * 97) % 1600) * {scale} AS sz) s
         CROSS JOIN LATERAL generate_series(1, s.sz) k",
        kn = caps.len() + 1,
        players = 5000 * scale,
    ))
    .await
    .unwrap();
    store::entries_written(Some(next));
    let t = Instant::now();
    let (_, appended) = histories(&custom, next + 60, true).await;
    for uuid in 1..=BOARDS {
        let warm = service::warm_board_history(&live, uuid, 7, 5, next + 60).await.unwrap();
        assert_eq!(warm, service::board_history(&live, uuid, 7, 5, next + 60).await.unwrap(), "board {uuid}");
    }
    println!(
        "after a new capture: charts in {appended:.1} ms, all equal to fresh reads ({:.1}s)",
        t.elapsed().as_secs_f64()
    );
}

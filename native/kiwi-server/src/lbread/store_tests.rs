//! The live Postgres and Redis paths, opt-in: point POSTGRES_DSN at a scratch
//! database (the parity seed adds the leaderboards tables and rows) and
//! REDIS_URL at a throwaway server, then run
//! `cargo test -p kiwi-server lbread_store -- --ignored`.

use serde_json::json;

use super::{Live, cache, service, store::Source};
use crate::{config::Settings, pyjson, state::AppState};

const SEED: &str = include_str!("../../../port/parity/lbread.seed.sql");
const TUE_13: i64 = 1_894_107_600;
const TUE_11: i64 = 1_894_100_400;
const MON_18: i64 = 1_894_039_200;
const MON_11: i64 = 1_894_014_000;

#[tokio::test]
#[ignore = "writes to POSTGRES_DSN / REDIS_URL"]
async fn lbread_store_reads() {
    if std::env::var("PY_UPSTREAM").is_err() {
        // SAFETY: only this ignored test runs in its process invocation.
        unsafe { std::env::set_var("PY_UPSTREAM", "http://127.0.0.1:9") };
    }
    let state = AppState::new(Settings::from_env().unwrap()).await.unwrap();
    state.pg().get().await.unwrap().batch_execute(SEED).await.unwrap();
    let live = Live(&state);

    let ts = live.list_timestamps(365, None).await.unwrap();
    assert_eq!(ts[..3], [TUE_13, TUE_11, MON_18]);
    assert_eq!(live.list_timestamps(10, Some(MON_11)).await.unwrap(), [TUE_13, TUE_11, MON_18, MON_11]);
    assert_eq!(live.list_days(10).await.unwrap()[..4], [TUE_13, MON_18, 1_893_942_000, 1_893_841_200]);

    let boards = live.list_boards_at(TUE_13).await.unwrap();
    assert!(boards.iter().any(|b| b.uuid == 1000 && b.contest_type.is_none()));
    let at_tue = live.list_boards_at(TUE_11).await.unwrap();
    assert_eq!(at_tue.iter().find(|b| b.uuid == 1).unwrap().contest_type.as_deref(), Some("weekly"));

    let board = live.get_board(1000).await.unwrap().unwrap();
    assert_eq!(board.contests, [(1_893_927_600, "daily".to_owned())]);
    assert!(live.get_board(424_242).await.unwrap().is_none());
    assert_eq!(live.latest_anchor_for_board(1001).await.unwrap(), Some(TUE_13));
    assert_eq!(live.latest_anchor_for_board(777).await.unwrap(), None);

    let (rows, total) = live.list_entries(1000, TUE_13, 3, 0).await.unwrap();
    assert_eq!(total, 9);
    assert_eq!(rows.iter().map(|r| r.rank).collect::<Vec<_>>(), [1, 2, 3]);
    assert_eq!(live.previous_day_anchor(1000, TUE_13).await.unwrap(), Some(MON_18));
    assert_eq!(live.board_reset_kind(2001).await.unwrap(), "none");
    assert_eq!(live.board_reset_kind(50000).await.unwrap(), "weekly");
    assert_eq!(live.board_reset_kind(32000).await.unwrap(), "daily");
    let names: Vec<String> = rows.iter().map(|r| r.player_name.clone()).collect();
    assert!(!live.prev_rows_for_players(1000, MON_18, &names).await.unwrap().is_empty());
    assert_eq!(live.top_entries_for_boards(&[1, 20, 1000, 4242], TUE_13).await.unwrap().len(), 3);
    let series = live.board_top_series(1000, 1_893_841_200, 3).await.unwrap();
    assert_eq!((series.anchors.len(), series.top.len()), (7, 3));
    assert!(series.rows.iter().all(|r| series.top.iter().any(|m| m.player_id == r.player_id)));

    let page = service::list_entries_with_deltas(&live, 4000, MON_11, 5, 0).await.unwrap();
    assert_eq!(page.comparison.reason, "crossed_reset");
    assert!(service::board_health(&live, 1000).await.unwrap().is_some());
    assert!(service::board_health(&live, 777).await.unwrap().is_none());

    if state.inner.db.redis.is_none() {
        return;
    }
    let mut conn = state.redis();
    let keys = [cache::TS_KEY, cache::READY_KEY];
    let _: i64 = redis::cmd("DEL").arg(&keys).query_async(&mut conn).await.unwrap();
    assert_eq!(cache::get_timestamps(&live, &live, 2).await.unwrap(), [TUE_13, TUE_11]);
    let raw: String = redis::cmd("GET").arg(cache::TS_KEY).query_async(&mut conn).await.unwrap();
    assert_eq!(raw, pyjson::dumps_spaced(&json!(ts)));
    let ttl: i64 = redis::cmd("TTL").arg(cache::TS_KEY).query_async(&mut conn).await.unwrap();
    assert!((cache::TTL_SECONDS - 5..=cache::TTL_SECONDS).contains(&ttl));
    cache::set_ready_anchor(&live, TUE_11).await;
    assert_eq!(cache::get_timestamps(&live, &live, 2).await.unwrap(), [TUE_11, MON_18]);
    let _: i64 = redis::cmd("DEL").arg(&keys).query_async(&mut conn).await.unwrap();
}

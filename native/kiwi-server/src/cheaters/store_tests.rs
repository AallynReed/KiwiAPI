//! Opt-in: POSTGRES_DSN at a throwaway database with the leaderboards schema
//! (api-py creates it at startup); `cargo test -p kiwi-server cheaters_store -- --ignored`.

use kiwi_core::cheaters::Entry;

use super::store::{Live, Store, trove_day_start};
use crate::{config::Settings, state::AppState};

const BOARD: i32 = 990_401;

#[tokio::test]
#[ignore = "writes to POSTGRES_DSN"]
async fn cheaters_store_reads_like_pg_store() {
    if std::env::var("PY_UPSTREAM").is_err() {
        // SAFETY: set before any other thread reads the environment.
        unsafe { std::env::set_var("PY_UPSTREAM", "http://127.0.0.1:9") };
    }
    let state = AppState::new(Settings::from_env().unwrap()).await.unwrap();
    let anchor: i64 = 2_000_000_000 / 86400 * 86400 + 11 * 3600 + 7200;
    let day = trove_day_start(anchor);
    let pg = state.pg().get().await.unwrap();
    pg.batch_execute(&format!(
        "CREATE TABLE IF NOT EXISTS entry_p_{day} PARTITION OF entry FOR VALUES FROM ({day}) TO ({});
         INSERT INTO board VALUES ({BOARD}, 'LB_T', 'Parity Board', 'Cat_T', 'Parity', 'none') ON CONFLICT DO NOTHING;
         INSERT INTO player (name, name_lower) VALUES ('ParityÄ', 'parityä'), ('ParityB', 'parityb')
           ON CONFLICT (name_lower) DO NOTHING;",
        day + 86400
    ))
    .await
    .unwrap();
    for (name, back, rank, score) in [("ParityÄ", 1, 1, 10.0), ("ParityB", 1, 2, 5.0), ("ParityÄ", 0, 1, 30.0)] {
        pg.execute(
            "INSERT INTO entry (board_uuid, anchor, rank, score, player_id) \
             SELECT $1, $2, $3, $4, id FROM player WHERE name_lower = lower($5)",
            &[&BOARD, &(anchor - back * 3600), &rank, &score, &name],
        )
        .await
        .unwrap();
    }
    let s = Live(&state);
    let boards = s.list_boards_at(anchor).await.unwrap();
    let b = boards.iter().find(|b| b.board.uuid == i64::from(BOARD)).unwrap();
    assert_eq!(b.board.reset_kind, "none");
    let by = s.entries_by_board_at(anchor, &[i64::from(BOARD)]).await.unwrap();
    assert_eq!(by[&i64::from(BOARD)], vec![Entry { player_name: "ParityÄ".into(), rank: 1, score: 30.0 }]);
    let prev = s.previous_captures_bulk(&["parityä".into()], i64::from(BOARD), anchor, 0).await.unwrap();
    assert_eq!(prev["ParityÄ"], (10.0, anchor - 3600));
    let series = s.comovement_series(i64::from(BOARD), &["PARITYÄ".into(), "parityb".into()], 0, anchor).await.unwrap();
    assert_eq!(series.len(), 2);
    let (items, total) = s.list_entries(i64::from(BOARD), anchor - 3600, 100, 0).await.unwrap();
    assert_eq!((items.len(), total), (2, 2));
    assert_eq!(s.board_reset_kind(i64::from(BOARD)).await.unwrap(), "none");
    let (anchors, top, rows) = s.board_top_series(i64::from(BOARD), 0, 5).await.unwrap();
    assert_eq!((anchors.len(), top.len(), rows.len()), (2, 1, 2));
    pg.batch_execute(&format!("DELETE FROM entry WHERE board_uuid = {BOARD}; DELETE FROM board WHERE uuid = {BOARD};"))
        .await
        .unwrap();
}

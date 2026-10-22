//! The live store against real Postgres/Redis/Mongo, opt-in because it
//! writes: point POSTGRES_DSN (with the leaderboards schema, or an empty
//! database), REDIS_URL and MONGO_URI at throwaway stores and run
//! `cargo test -p kiwi-server lb_players_live -- --ignored`.

use mongodb::bson::{Document, doc};
use serde_json::json;

use super::store::{Live, Store};
use crate::{config::Settings, state::AppState};

const DDL: &str = "
CREATE TABLE IF NOT EXISTS board (uuid INTEGER PRIMARY KEY, name_id TEXT NOT NULL, name TEXT NOT NULL,
    category_id TEXT NOT NULL, category TEXT NOT NULL, reset_kind_override TEXT);
CREATE TABLE IF NOT EXISTS player (id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY, name TEXT NOT NULL,
    name_lower TEXT NOT NULL UNIQUE);
CREATE TABLE IF NOT EXISTS entry (board_uuid INTEGER NOT NULL, anchor BIGINT NOT NULL, rank INTEGER NOT NULL,
    score DOUBLE PRECISION NOT NULL, player_id BIGINT NOT NULL) PARTITION BY RANGE (anchor);
CREATE INDEX IF NOT EXISTS entry_pa ON entry (player_id, anchor);
CREATE TABLE IF NOT EXISTS entry_p_lbp_test PARTITION OF entry FOR VALUES FROM (4102444800) TO (4102531200);
CREATE TABLE IF NOT EXISTS player_board_agg (player_id BIGINT NOT NULL, board_uuid INTEGER NOT NULL,
    best_rank INTEGER NOT NULL, appearances BIGINT NOT NULL, first_seen BIGINT NOT NULL, last_seen BIGINT NOT NULL,
    latest_rank INTEGER NOT NULL, latest_score DOUBLE PRECISION NOT NULL, last_folded_anchor BIGINT NOT NULL,
    PRIMARY KEY (player_id, board_uuid));
ALTER TABLE player_board_agg ADD COLUMN IF NOT EXISTS last_rise BIGINT;
CREATE TABLE IF NOT EXISTS player_rename (id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    from_name TEXT NOT NULL, from_name_lower TEXT NOT NULL, to_name TEXT NOT NULL, to_name_lower TEXT NOT NULL,
    from_anchor BIGINT NOT NULL, to_anchor BIGINT NOT NULL, confidence DOUBLE PRECISION NOT NULL,
    matched_boards INTEGER NOT NULL, evidence JSONB NOT NULL DEFAULT '{}'::jsonb,
    method_version INTEGER NOT NULL DEFAULT 1, created_at BIGINT NOT NULL,
    UNIQUE (from_name_lower, to_name_lower, to_anchor));
CREATE TABLE IF NOT EXISTS player_duplicate (name_lower TEXT PRIMARY KEY, name TEXT NOT NULL,
    kind TEXT NOT NULL DEFAULT 'same_name', verdict TEXT NOT NULL DEFAULT 'all_idle',
    boards INTEGER NOT NULL DEFAULT 0, max_occurrences INTEGER NOT NULL DEFAULT 2,
    spellings JSONB NOT NULL DEFAULT '[]'::jsonb, first_anchor BIGINT NOT NULL, last_anchor BIGINT NOT NULL,
    evidence JSONB NOT NULL DEFAULT '{}'::jsonb, method_version INTEGER NOT NULL DEFAULT 1,
    updated_at BIGINT NOT NULL);
";

const A: i64 = 4_102_444_800;

#[tokio::test]
#[ignore = "creates and drops rows in POSTGRES_DSN, keys in REDIS_URL and a doc in MONGO_URI"]
async fn lb_players_live() {
    if std::env::var("PY_UPSTREAM").is_err() {
        // SAFETY: only this ignored test runs in its process invocation.
        unsafe { std::env::set_var("PY_UPSTREAM", "http://127.0.0.1:9") };
    }
    let state = AppState::new(Settings::from_env().unwrap()).await.unwrap();
    let pg = state.inner.db.pg.clone().expect("POSTGRES_DSN").get().await.unwrap();
    pg.batch_execute(DDL).await.unwrap();
    let name = format!("LbpTest{}", std::process::id());
    let lower = name.to_lowercase();
    let id: i64 = pg
        .query_one("INSERT INTO player (name, name_lower) VALUES ($1, $2) RETURNING id", &[&name, &lower])
        .await
        .unwrap()
        .get(0);
    pg.batch_execute(
        "INSERT INTO board VALUES (990001, 'n1', 'Board One', 'c', 'Cat', 'weekly'), \
         (990002, 'n2', 'Board Two', 'c', 'Cat', 'bogus'), (1100, 'club', 'Club PR', 'c', 'Clubs', NULL) \
         ON CONFLICT (uuid) DO NOTHING",
    )
    .await
    .unwrap();
    for (board, anchor, rank, score) in [
        (990001i32, A, 5i32, 10.0f64),
        (990002, A, 1, 2.5),
        (1100, A, 1, 99.0),
        (990001, A + 3600, 4, 20.0),
        (990002, A + 3600, 2, 1e16),
    ] {
        pg.execute(
            "INSERT INTO entry (board_uuid, anchor, rank, score, player_id) VALUES ($1, $2, $3, $4, $5)",
            &[&board, &anchor, &rank, &score, &id],
        )
        .await
        .unwrap();
    }
    pg.execute(
        "INSERT INTO player_board_agg VALUES ($1, 990001, 4, 2, $2, $3, 4, 20.0, $3, $3), \
         ($1, 990002, 1, 2, $2, $3, 2, 1e16, $3, NULL), ($1, 1100, 1, 1, $2, $2, 1, 99.0, $2, $3)",
        &[&id, &A, &(A + 3600)],
    )
    .await
    .unwrap();
    pg.execute(
        "INSERT INTO player_rename (from_name, from_name_lower, to_name, to_name_lower, from_anchor, to_anchor, \
         confidence, matched_boards, evidence, created_at) VALUES \
         ('Old', 'old', $1, $2, $3, $3, 0.5, 2, to_jsonb('{\"z\": 1, \"a\": 2.0}'::text), 1), \
         ($1, $2, 'Newer', 'newer', $3, $3 + 1, 1, 3, '{\"z\": 1, \"a\": [1]}'::jsonb, 2)",
        &[&name, &lower, &A],
    )
    .await
    .unwrap();
    pg.execute(
        "INSERT INTO player_duplicate (name_lower, name, spellings, first_anchor, last_anchor, evidence, updated_at) \
         VALUES ($1, $2, to_jsonb('[\"x\"]'::text), 1, 2, '{\"summary\": \"s\"}'::jsonb, 3)",
        &[&lower, &name],
    )
    .await
    .unwrap();

    let s = Live(&state);
    let padded = format!("  {}  ", name.to_uppercase());
    let rows = s.player_rows(&padded, 10, None, None).await.unwrap();
    assert_eq!(rows.len(), 4, "club board left out");
    assert!(rows.iter().all(|r| r.player_name == name));
    assert_eq!(rows[0].created_at, A + 3600);
    let one = s.player_rows(&name, 1, Some(990001), Some(A)).await.unwrap();
    assert_eq!((one.len(), one[0].rank, one[0].score), (1, 4, 20.0));
    assert!(s.player_rows(&name, 1, Some(1i128 << 40), None).await.is_err(), "int4 overflow is a 500");
    assert!(s.player_rows("nobody-lbp", 1, Some(1i128 << 40), None).await.unwrap().is_empty());
    assert_eq!(s.player_rows_window(&name, A + 1).await.unwrap().len(), 2);
    assert_eq!(s.player_canonical_name(&padded).await.unwrap().as_deref(), Some(name.as_str()));
    let agg = s.player_board_summary(&name).await.unwrap();
    assert_eq!(agg.iter().map(|a| a.leaderboard).collect::<Vec<_>>(), vec![990002, 990001]);
    assert_eq!(agg[0].latest_score, 1e16);
    assert_eq!(s.player_last_played(&name, &[]).await.unwrap(), Some(A + 3600));
    assert_eq!(s.player_last_played(&name, &[990001]).await.unwrap(), None);
    assert!(s.player_last_played(&name, &[1i128 << 40]).await.is_err());
    let meta = s.board_meta(&[990001, 990002, 990003]).await.unwrap();
    assert_eq!((meta[&990001].reset_kind.as_str(), meta[&990002].reset_kind.as_str()), ("weekly", "default"));
    assert_eq!(meta.len(), 2);
    assert_eq!(s.board_kinds(&[990001]).await.unwrap()[&990001], "weekly");
    let edges = s.renames_for_name(&format!(" {} ", name.to_uppercase())).await.unwrap();
    assert_eq!(edges.len(), 2);
    assert_eq!(edges[0].to_name, "Newer", "to_anchor DESC");
    assert_eq!(crate::pyjson::dumps(&edges[0].evidence), r#"{"a":[1],"z":1}"#, "jsonb key order");
    assert_eq!(crate::pyjson::dumps(&edges[1].evidence), r#"{"z":1,"a":2.0}"#, "double-encoded keeps its order");
    let dup = s.duplicate_for_name(&padded).await.unwrap().unwrap();
    assert_eq!((dup.spellings, dup.evidence), (json!(["x"]), json!({"summary": "s"})));
    assert!(s.latest_anchor().await.unwrap().is_some_and(|a| a >= A + 3600));

    if let Some(mut redis) = state.inner.db.redis.clone() {
        let key = format!("lb:cache:cheaters:{A}");
        let _: () =
            redis::cmd("SET").arg(&key).arg(r#"{"clusters": [{"x": 1.0}]}"#).query_async(&mut redis).await.unwrap();
        assert_eq!(s.cheaters_payload(A).await, Some(json!({"clusters": [{"x": 1.0}]})));
        let _: () = redis::cmd("DEL").arg(&key).query_async(&mut redis).await.unwrap();
        assert_eq!(s.cheaters_payload(A).await, None);
    }
    if let Some(db) = state.inner.db.mongo.clone() {
        let users = db.collection::<Document>("site_users");
        users.insert_one(doc! { "claimed_trove_name": &lower, "claim_verified": true }).await.unwrap();
        assert!(s.claim_verified(&lower).await);
        assert!(!s.claim_verified(&name).await);
        users.delete_many(doc! { "claimed_trove_name": &lower }).await.unwrap();
    }

    pg.execute("DELETE FROM player_duplicate WHERE name_lower = $1", &[&lower]).await.unwrap();
    pg.execute("DELETE FROM player_rename WHERE from_name_lower = $1 OR to_name_lower = $1", &[&lower]).await.unwrap();
    pg.execute("DELETE FROM player_board_agg WHERE player_id = $1", &[&id]).await.unwrap();
    pg.execute("DELETE FROM entry WHERE player_id = $1", &[&id]).await.unwrap();
    pg.execute("DELETE FROM player WHERE id = $1", &[&id]).await.unwrap();
    pg.batch_execute("DELETE FROM board WHERE uuid IN (990001, 990002); DROP TABLE IF EXISTS entry_p_lbp_test")
        .await
        .unwrap();
}

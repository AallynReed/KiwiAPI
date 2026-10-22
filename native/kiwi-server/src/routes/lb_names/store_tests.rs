//! Postgres-backed checks of the `pg_store.py` SQL, opt-in because they write:
//! point POSTGRES_DSN (and optionally REDIS_URL) at a throwaway postgres:17 /
//! redis:7 and run `cargo test -p kiwi-server lb_names_store -- --ignored`.
//! Everything lives in a scratch schema that is dropped at the end.

use std::sync::Arc;

use hyper_util::{client::legacy::Client, rt::TokioExecutor};
use serde_json::json;

use super::store::{DateWrite, DuplicateWrite, Pg, RenameWrite, Store};
use super::{duplicates, renames};
use crate::{
    config::Settings,
    db::Db,
    pyjson,
    state::{AppState, Inner},
};

const SCHEMA: &str = "
CREATE TABLE board (uuid INTEGER PRIMARY KEY, name_id TEXT NOT NULL, name TEXT NOT NULL,
    category_id TEXT NOT NULL, category TEXT NOT NULL, reset_kind_override TEXT);
CREATE TABLE player (id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY, name TEXT NOT NULL,
    name_lower TEXT NOT NULL UNIQUE);
CREATE TABLE entry (board_uuid INTEGER NOT NULL, anchor BIGINT NOT NULL, rank INTEGER NOT NULL,
    score DOUBLE PRECISION NOT NULL, player_id BIGINT NOT NULL) PARTITION BY RANGE (anchor);
CREATE TABLE entry_all PARTITION OF entry FOR VALUES FROM (0) TO (4000000000);
CREATE TABLE player_rename (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY, from_name TEXT NOT NULL, from_name_lower TEXT NOT NULL,
    to_name TEXT NOT NULL, to_name_lower TEXT NOT NULL, from_anchor BIGINT NOT NULL, to_anchor BIGINT NOT NULL,
    confidence DOUBLE PRECISION NOT NULL, matched_boards INTEGER NOT NULL,
    evidence JSONB NOT NULL DEFAULT '{}'::jsonb, method_version INTEGER NOT NULL DEFAULT 1,
    created_at BIGINT NOT NULL, UNIQUE (from_name_lower, to_name_lower, to_anchor));
CREATE TABLE player_duplicate (
    name_lower TEXT PRIMARY KEY, name TEXT NOT NULL, kind TEXT NOT NULL DEFAULT 'same_name',
    verdict TEXT NOT NULL DEFAULT 'all_idle', boards INTEGER NOT NULL DEFAULT 0,
    max_occurrences INTEGER NOT NULL DEFAULT 2, spellings JSONB NOT NULL DEFAULT '[]'::jsonb,
    first_anchor BIGINT NOT NULL, last_anchor BIGINT NOT NULL, evidence JSONB NOT NULL DEFAULT '{}'::jsonb,
    method_version INTEGER NOT NULL DEFAULT 1, updated_at BIGINT NOT NULL);
INSERT INTO board VALUES (1, 'TM', 'Trove Mastery', 'c', 'c', NULL), (20, 'GM', '', 'c', 'c', NULL),
    (1100, 'CPR', 'Club', 'c', 'c', NULL), (4000, 'E0', 'Effort', 'c', 'c', 'none');
INSERT INTO player (name, name_lower) VALUES ('Alpha', 'alpha'), ('Beta', 'beta'), ('Gamma', 'gamma');
INSERT INTO entry VALUES
    (1, 100, 1, 5000.0, 1), (1, 100, 2, 4000.0, 2), (1, 100, 3, 4500.0, 2), (20, 100, 1, 9.5, 1),
    (1, 200, 1, 5100.0, 1), (1, 200, 4, 0.0, 3), (1, 200, 5, -0.0, 3), (1, 200, 6, 0.0, 3), (1, 200, 6, 0.0, 3),
    (1100, 200, 1, 77.0, 1), (1100, 200, 2, 70.0, 1), (20, 300, 1, 1.0, 2), (4000, 300, 1, 2.0, 99);
";

async fn state(schema: &str) -> AppState {
    let dsn = std::env::var("POSTGRES_DSN").expect("POSTGRES_DSN");
    let mut cfg: tokio_postgres::Config = dsn.parse().unwrap();
    cfg.options(format!("-c search_path={schema}"));
    let pool = deadpool_postgres::Pool::builder(deadpool_postgres::Manager::new(cfg, tokio_postgres::NoTls))
        .max_size(4)
        .build()
        .unwrap();
    let redis = match std::env::var("REDIS_URL") {
        Ok(url) => Some(redis::aio::ConnectionManager::new(redis::Client::open(url).unwrap()).await.unwrap()),
        Err(_) => None,
    };
    AppState {
        inner: Arc::new(Inner {
            settings: Settings::from_vars([("PY_UPSTREAM", "http://127.0.0.1:9")]).unwrap(),
            db: Db { mongo: None, redis, pg: Some(pool) },
            upstream: Client::builder(TokioExecutor::new()).build_http(),
        }),
    }
}

async fn admin() -> tokio_postgres::Client {
    let dsn = std::env::var("POSTGRES_DSN").expect("POSTGRES_DSN");
    let (client, conn) = tokio_postgres::connect(&dsn, tokio_postgres::NoTls).await.unwrap();
    tokio::spawn(conn);
    client
}

#[tokio::test]
#[ignore = "writes to POSTGRES_DSN (and REDIS_URL)"]
async fn lb_names_store_matches_pg_store() {
    let schema = format!("lbn_test_{}", std::process::id());
    let admin = admin().await;
    admin.batch_execute(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE; CREATE SCHEMA {schema};")).await.unwrap();
    admin.batch_execute(&format!("SET search_path = {schema};{SCHEMA}")).await.unwrap();
    let st = state(&schema).await;
    let pg = Pg(&st);

    assert_eq!(pg.all_anchors_asc().await.unwrap(), [100, 200, 300]);
    assert_eq!(pg.latest_anchors(2).await.unwrap(), [300, 200]);
    let boards = pg.boards_at(200).await.unwrap();
    assert_eq!(boards.iter().map(|b| b.uuid).collect::<std::collections::BTreeSet<_>>(), [1, 1100].into());
    let cap = pg.capture(100, &[1, 20]).await.unwrap();
    assert_eq!(cap.boards[&1][&2], 4500.0, "a doubled player keeps the higher score");
    assert_eq!(cap.boards[&20].len(), 1);
    assert_eq!(pg.player_names(&[1, 3, 42]).await.unwrap().len(), 2);

    // The COPY kernel and the SQL fallback agree, -0.0 and exact repeats included.
    let groups = pg.duplicate_groups_at(200).await.unwrap();
    let sql: Vec<(i64, i32, i32)> = admin
        .query(
            "SELECT player_id, board_uuid, count(DISTINCT (rank, score))::int FROM entry WHERE anchor = 200 \
             GROUP BY player_id, board_uuid HAVING count(DISTINCT (rank, score)) > 1 ORDER BY player_id, board_uuid",
            &[],
        )
        .await
        .unwrap()
        .iter()
        .map(|r| (r.get(0), r.get(1), r.get(2)))
        .collect();
    assert_eq!(
        groups.iter().map(|g| (g.board_uuid, g.occurrences)).collect::<Vec<_>>(),
        sql.iter().map(|g| (g.1, g.2)).collect::<Vec<_>>()
    );
    assert_eq!(groups.iter().map(|g| g.name.as_str()).collect::<Vec<_>>(), ["Alpha", "Gamma"]);
    let window = pg.player_rows_window(" ALPHA ", 100, 200).await.unwrap();
    assert_eq!(window.len(), 3, "club boards and out-of-window anchors are left out: {window:?}");

    // Renames: the evidence is stored double-encoded, as asyncpg's codec does.
    let evidence =
        json!({"gap_seconds": 100, "boards": [{"uuid": 1, "score_from": 1.0}], "summary": "\u{201c}x\u{201d}"});
    let row = RenameWrite {
        from_name: "Alpha".into(),
        from_name_lower: "alpha".into(),
        to_name: "Delta".into(),
        to_name_lower: "delta".into(),
        from_anchor: 100,
        to_anchor: 200,
        confidence: 0.875,
        matched_boards: 2,
        evidence: pyjson::dumps_spaced(&evidence),
        method_version: 1,
        created_at: 5,
    };
    pg.upsert_renames(&[row.clone(), RenameWrite { confidence: 0.9, ..row.clone() }]).await.unwrap();
    let stored = admin
        .query_one(
            &format!("SELECT jsonb_typeof(evidence), evidence #>> '{{}}', confidence FROM {schema}.player_rename"),
            &[],
        )
        .await
        .unwrap();
    assert_eq!(stored.get::<_, String>(0), "string");
    assert_eq!(stored.get::<_, String>(1), row.evidence);
    assert_eq!(stored.get::<_, f64>(2), 0.9, "the unique key upserts in place");
    let (rows, total) = pg.list_renames(10, 0).await.unwrap();
    assert_eq!(total, 1);
    assert_eq!(renames::row_value(&rows[0])["evidence"], evidence);
    assert_eq!(pg.renames_for_name("delta").await.unwrap().len(), 1);
    assert_eq!(pg.count_renames().await.unwrap(), 1);

    // Duplicates: merge rules, the self-heal and the reads.
    admin
        .batch_execute(&format!(
            "SET search_path = {schema};
             INSERT INTO player_duplicate (name_lower, name, kind, verdict, boards, max_occurrences, spellings,
                 first_anchor, last_anchor, evidence, method_version, updated_at)
             VALUES ('robot', 'Robot', 'case', 'case_only', 1, 2, to_jsonb(ARRAY['Robot','robot']), 150, 150, '{{}}'::jsonb, 1, 150),
                    ('idle', 'Idle', 'same_name', 'all_idle', 1, 2, '[]'::jsonb, 90, 90, '{{}}'::jsonb, 1, 90),
                    ('kept', 'Kept', 'same_name', 'all_idle', 1, 2, '\"[]\"'::jsonb, 90, 90, '\"{{}}\"'::jsonb, 1, 90);"
        ))
        .await
        .unwrap();
    let dup = DuplicateWrite {
        name_lower: "robot".into(),
        name: "Robot".into(),
        kind: "same_name".into(),
        verdict: "one_live".into(),
        boards: 2,
        max_occurrences: 3,
        spellings: "[]".into(),
        first_anchor: 200,
        last_anchor: 300,
        evidence: pyjson::dumps_spaced(&json!({"lookback_days": 7})),
        method_version: 1,
        updated_at: 9,
    };
    pg.upsert_duplicates(&[dup]).await.unwrap();
    let dates = |name: &str| DateWrite {
        name_lower: name.to_lowercase(),
        name: name.into(),
        boards: 1,
        max_occurrences: 2,
        first_anchor: 80,
        last_anchor: 95,
        updated_at: 11,
    };
    pg.upsert_duplicate_dates(&[dates("Idle"), dates("Kept"), dates("Fresh")]).await.unwrap();
    let verdicts: Vec<(String, String, String, i64)> = admin
        .query(
            &format!("SELECT name_lower, kind, verdict, first_anchor FROM {schema}.player_duplicate ORDER BY 1"),
            &[],
        )
        .await
        .unwrap()
        .iter()
        .map(|r| (r.get(0), r.get(1), r.get(2), r.get(3)))
        .collect();
    let v = |a: &str, b: &str, c: &str, d: i64| (a.to_owned(), b.to_owned(), c.to_owned(), d);
    assert_eq!(
        verdicts,
        [
            v("fresh", "same_name", "not_analysed", 80),
            v("idle", "same_name", "not_analysed", 80),
            v("kept", "same_name", "all_idle", 80),
            v("robot", "both", "one_live", 150),
        ]
    );
    let (rows, total, current) = pg.list_duplicates(10, 0, Some("case")).await.unwrap();
    assert_eq!((rows.len(), total, current), (1, 1, 1));
    let robot = duplicates::row_value(&rows[0]);
    assert_eq!(robot["spellings"], json!(["Robot", "robot"]), "upsert_duplicates leaves the case spellings alone");
    assert_eq!(robot["evidence"], json!({"lookback_days": 7}));
    let (_, total, current) = pg.list_duplicates(10, 0, None).await.unwrap();
    assert_eq!((total, current), (4, 1));
    assert_eq!(pg.latest_duplicate_anchor().await.unwrap(), Some(300));
    assert!(pg.duplicate_for_name(" ROBOT").await.unwrap().is_some());
    assert_eq!(pg.count_duplicates().await.unwrap(), 4);
    assert_eq!(pg.delete_all_duplicates().await.unwrap(), 4);
    assert_eq!(pg.delete_all_renames().await.unwrap(), 1);

    if st.inner.db.redis.is_some() {
        let key = format!("lb:test:{schema}");
        assert_eq!(pg.lock(&key).await, Some(true));
        assert_eq!(pg.lock(&key).await, Some(false));
        pg.unlock(&key).await;
        pg.save_status(&key, "{\"running\": false}").await;
        assert_eq!(pg.load_status(&key).await.as_deref(), Some("{\"running\": false}"));
        pg.unlock(&key).await;
    }
    admin.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
}

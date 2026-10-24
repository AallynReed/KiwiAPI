//! Live-store checks, opt-in because they write: point POSTGRES_DSN at a
//! throwaway postgres:17 and run
//! `cargo test -p kiwi-server codexes::store_tests -- --ignored`.

use std::{collections::HashMap, path::PathBuf};

use kiwi_core::codex::extract::Blobs;
use serde_json::json;

use super::{
    archive::{Archive, Change},
    indexer,
    pg::{self, Cell},
    reads, store,
};
use crate::{config::Settings, pyjson, state::AppState};

async fn state() -> AppState {
    AppState::new(Settings::from_env().unwrap()).await.unwrap()
}

fn seed() -> String {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata/codexes/seed.sql");
    let sql = std::fs::read_to_string(dir).unwrap();
    format!(
        "DROP TABLE IF EXISTS codex_entry, codex_stat, codex_ability, codex_link, codex_requirement, codex_upgrade, codex_meta, rig_binding;\n{sql}"
    )
}

#[derive(Clone)]
struct Mem(HashMap<String, Vec<u8>>);

impl Blobs for Mem {
    fn get(&self, sha: &str) -> Option<Vec<u8>> {
        self.0.get(sha).cloned()
    }
}

/// A tiny pts archive from the fixture prefabs; the sha is the path.
struct MemArchive {
    files: Mem,
    changes: Vec<Change>,
}

impl Archive for MemArchive {
    type Blobs = Mem;

    fn blobs(&self) -> Mem {
        self.files.clone()
    }

    async fn prefix(&self, prefix: &str) -> anyhow::Result<Vec<(String, Option<String>)>> {
        let mut v: Vec<(String, Option<String>)> =
            self.files.0.keys().filter(|p| p.starts_with(prefix)).map(|p| (p.clone(), Some(p.clone()))).collect();
        v.sort();
        Ok(v)
    }

    async fn regex(&self, pattern: &str, ci: bool) -> anyhow::Result<Vec<(String, Option<String>)>> {
        let re = regex::RegexBuilder::new(pattern).case_insensitive(ci).build()?;
        let mut v: Vec<(String, Option<String>)> =
            self.files.0.keys().filter(|p| re.is_match(p)).map(|p| (p.clone(), Some(p.clone()))).collect();
        v.sort();
        Ok(v)
    }

    async fn file_sha(&self, path: &str) -> anyhow::Result<Option<String>> {
        Ok(self.files.0.contains_key(path).then(|| path.to_owned()))
    }

    async fn changes(&self, _: i64) -> anyhow::Result<Vec<Change>> {
        Ok(self.changes.clone())
    }
}

fn fixture_archive() -> MemArchive {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata/codexes");
    let mut files = HashMap::new();
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with("prefabs__") || name.starts_with("languages__") {
            files.insert(name.replace("__", "/"), std::fs::read(e.path()).unwrap());
        }
    }
    MemArchive { files: Mem(files), changes: Vec::new() }
}

#[tokio::test]
#[ignore = "writes to POSTGRES_DSN"]
async fn reads_and_index() {
    let state = state().await;
    let pool = state.pg().clone();
    pool.get().await.unwrap().batch_execute(&seed()).await.unwrap();

    let (rows, total) =
        store::query_entries(&pool, "live-us", &store::Filter::default(), "-indexed_at", 50, 0).await.unwrap();
    assert_eq!(total, 6);
    assert_eq!(rows[0].text("path"), "prefabs/collections/boat/clipper.binfab");
    let dragon = store::get_entry(&pool, "live-us", "dragon", "prefabs/collections/mount/dragon_amperium.binfab")
        .await
        .unwrap()
        .unwrap();
    let v1 = pyjson::pydantic(&reads::entry_out(&dragon));
    assert!(v1.contains(r#""amount":0.00001,"is_percent":true,"stat":"$Stat_MagicFind","value":1e+16"#), "{v1}");
    assert!(
        v1.ends_with(r#""indexed_at":"2026-10-20T12:34:56.123450Z","stat_value":null,"stat_percent":null}"#),
        "{v1}"
    );
    let site = pyjson::dumps(&reads::site_row(&dragon));
    assert!(
        site.contains(r#""amount":1e-05"#) && site.ends_with(r#""indexed_at":"2026-10-20T12:34:56.123450+00:00"}"#),
        "{site}"
    );

    let f = store::Filter { stat: Some("$Stat_MagicFind".into()), ..Default::default() };
    let (rows, _) = store::query_entries(&pool, "live-us", &f, "-stat_value", 10, 0).await.unwrap();
    let values: Vec<Cell> = rows.iter().map(|r| r.get("stat_value").clone()).collect();
    assert_eq!(values, [Cell::Float(30.000001192092896), Cell::Float(25.0), Cell::Null]);
    let (holders, total) = store::entries_with_stat(&pool, "live-us", "$Stat_MagicFind", None, 50, 0).await.unwrap();
    assert_eq!(total, 3);
    assert_eq!(holders.last().unwrap().get("value"), &Cell::Null);
    let reqs = store::requirements_for(&pool, "live-us", "collections/badge/blocks_bronze").await.unwrap();
    assert_eq!(reqs.len(), 2);
    assert_eq!(reqs[1].get("amount"), &Cell::Int(9007199254740993));
    let sig = store::meta_signature(&pool, "live-us").await.unwrap();
    assert_eq!(sig.0, 27);

    // A forced rebuild of pts from the fixture prefabs.
    let a = fixture_archive();
    let counts = indexer::reindex(&pool, &a, "pts", true, "").await.unwrap();
    let c: HashMap<String, i64> = counts.0.iter().cloned().collect();
    assert_eq!(c["removed"], 2);
    assert!(c["indexed"] >= 9, "{counts:?}");
    assert!(c["requirements"] > 100 && c["upgrade_nodes"] > 0, "{counts:?}");
    let n = indexer::reindex_rigs(&pool, &a, "pts").await.unwrap();
    assert!(n > 0);
    assert_eq!(store::get_parser_version(&pool, "pts").await.unwrap(), indexer::CODEX_PARSER_VERSION);
    assert_eq!(store::get_rig_version(&pool, "pts").await.unwrap(), indexer::RIG_PARSER_VERSION);
    let client = pool.get().await.unwrap();
    let data = pg::fetchval(
        &client,
        "SELECT data FROM codex_entry WHERE branch = 'pts' AND path = 'prefabs/recipes/recipe_item_wings_lunacy.binfab'",
        &[],
    )
    .await
    .unwrap();
    assert_eq!(data.json()["recipe"]["mastery_source"], json!("prefab"));
    let stale = pg::fetchval(
        &client,
        "SELECT count(*) FROM codex_stat WHERE branch = 'pts' AND path = 'prefabs/item/gone.binfab'",
        &[],
    )
    .await
    .unwrap();
    assert_eq!(stale, Cell::Int(0));

    // A delta that removes one prefab and touches nothing shared.
    let mut a = fixture_archive();
    a.changes =
        vec![Change { path: "prefabs/collections/pet/dragonfly.binfab".into(), kind: "removed".into(), sha: None }];
    let counts = indexer::reindex_changes(&pool, &a, "pts", 2, "").await.unwrap();
    assert_eq!(counts.to_value(), json!({"indexed": 0, "removed": 1, "missing_blob": 0}));
    assert!(
        store::get_entry(&pool, "pts", "ally", "prefabs/collections/pet/dragonfly.binfab").await.unwrap().is_none()
    );
}

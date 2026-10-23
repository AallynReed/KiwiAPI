//! Live-store checks, opt-in because they write: point MONGO_URI / MONGO_DB
//! at a throwaway mongo:7 and run
//! `cargo test -p kiwi-server archiver::store_tests -- --ignored --test-threads=1`.
//! Every test works on its own branch name and removes it afterwards.

use std::collections::HashMap;

use bytes::Bytes;
use futures::TryStreamExt;
use kiwi_core::update_archive::{ManifestEntry, Pointer};
use mongodb::bson::{Bson, Document, doc};

use super::{
    Counts, SyncError,
    cas::ContentStore,
    cdn::Cdn,
    ingest::sync_branch,
    maintenance,
    repo::{self, MongoRepo, Repo},
};
use crate::{config::Settings, state::AppState};

async fn state() -> AppState {
    if std::env::var("PY_UPSTREAM").is_err() {
        // SAFETY: only these ignored tests run in their process invocation.
        unsafe { std::env::set_var("PY_UPSTREAM", "http://127.0.0.1:9") };
    }
    AppState::new(Settings::from_env().unwrap()).await.unwrap()
}

fn branch() -> String {
    let mut raw = [0u8; 4];
    getrandom::fill(&mut raw).unwrap();
    format!("test-{}", hex::encode(raw))
}

async fn all(state: &AppState, coll: &str, branch: &str) -> Vec<Document> {
    let c = state.mongo().collection::<Document>(coll);
    let mut v: Vec<Document> = c.find(doc! {"branch": branch}).await.unwrap().try_collect().await.unwrap();
    v.sort_by_key(|d| format!("{:?}{:?}", d.get("ordinal"), d.get("path")));
    v
}

async fn cleanup(state: &AppState, branch: &str) {
    for coll in [repo::BRANCHES, repo::VERSIONS, repo::CHANGES, repo::STATE, repo::MANIFEST] {
        state.mongo().collection::<Document>(coll).delete_many(doc! {"branch": branch}).await.unwrap();
    }
}

fn keys(d: &Document) -> Vec<&str> {
    d.keys().map(String::as_str).collect()
}

#[tokio::test]
#[ignore = "writes to MONGO_URI"]
async fn repo_writes_like_pymongo() {
    let state = state().await;
    let b = branch();
    let r = MongoRepo::new(state.clone());
    let p = Pointer { version: "V".into(), content_path: "cp".into(), motd: "m".into() };
    assert_eq!(r.begin_version(&b, "MV-1", &p).await.unwrap(), (1, false));
    assert_eq!(r.begin_version(&b, "MV-2", &p).await.unwrap(), (1, true));
    let versions = all(&state, repo::VERSIONS, &b).await;
    assert_eq!(
        keys(&versions[0]),
        [
            "_id",
            "branch",
            "ordinal",
            "version_tag",
            "status",
            "captured_at",
            "completed_at",
            "content_path",
            "motd",
            "files_added",
            "files_modified",
            "files_removed",
            "bytes_added"
        ]
    );
    let branches = all(&state, repo::BRANCHES, &b).await;
    assert_eq!(branches[0].get_str("status").unwrap(), "syncing");

    r.record_change(&b, 1, "a", "added", Some("s"), Some(4_000_000_000), 3).await.unwrap();
    r.upsert_state(&b, "a", "s", Some(4_000_000_000), 3, Some("d"), Some(0), 1).await.unwrap();
    // One flush runs the upsert before the delete, like pymongo's unordered bulk.
    r.upsert_state(&b, "z", "s", None, 1, None, None, 1).await.unwrap();
    r.remove_state(&b, "z").await.unwrap();
    r.set_manifest_entry(&b, "d/index.tfi", "sha1", 9).await.unwrap();
    let counts = Counts { added: 1, modified: 0, removed: 0, bytes_added: 3 };
    r.finish_version(&b, 1, "MV-1", &p, &counts).await.unwrap();

    let changes = all(&state, repo::CHANGES, &b).await;
    assert_eq!(keys(&changes[0]), ["_id", "branch", "ordinal", "path", "type", "content_sha256", "fnv_hash", "size"]);
    assert_eq!(changes[0].get("fnv_hash"), Some(&Bson::Int64(4_000_000_000)));
    assert_eq!(changes[0].get("ordinal"), Some(&Bson::Int32(1)));
    let st = all(&state, repo::STATE, &b).await;
    assert_eq!(st.len(), 1);
    assert_eq!(
        keys(&st[0]),
        ["_id", "branch", "path", "content_sha256", "fnv_hash", "size", "archive", "archive_index", "last_ordinal"]
    );
    let man = all(&state, repo::MANIFEST, &b).await;
    assert_eq!(keys(&man[0]), ["_id", "branch", "path", "sha1", "size"]);
    let v = &all(&state, repo::VERSIONS, &b).await[0];
    assert_eq!(v.get_str("status").unwrap(), "complete");
    assert_eq!(v.get("bytes_added"), Some(&Bson::Int32(3)));
    let br = &all(&state, repo::BRANCHES, &b).await[0];
    assert_eq!(
        keys(br),
        [
            "_id",
            "branch",
            "content_path",
            "current_version",
            "current_ordinal",
            "last_probe_at",
            "status",
            "last_error"
        ]
    );
    assert_eq!(br.get_str("status").unwrap(), "idle");
    r.mark_branch_error(&b, &"é".repeat(600)).await.unwrap();
    let br = &all(&state, repo::BRANCHES, &b).await[0];
    assert_eq!(br.get_str("last_error").unwrap().chars().count(), 500);
    cleanup(&state, &b).await;
}

struct Cdn1 {
    manifest: Vec<ManifestEntry>,
    files: HashMap<String, Bytes>,
}

impl Cdn for Cdn1 {
    async fn fetch_pointer(&self, _: &str) -> Result<Pointer, SyncError> {
        Ok(Pointer { version: "V1".into(), content_path: "cp".into(), motd: String::new() })
    }

    async fn fetch_manifest(&self, _: &str, _: &str) -> Result<(String, Vec<ManifestEntry>), SyncError> {
        Ok(("V1".into(), self.manifest.clone()))
    }

    async fn download_file(&self, _: &str, path: &str, _: &str, _: Option<i64>) -> Result<Bytes, SyncError> {
        Ok(self.files[path].clone())
    }
}

#[tokio::test]
#[ignore = "writes to MONGO_URI"]
async fn sync_into_mongo_and_backfill() {
    let state = state().await;
    let b = branch();
    let root = std::env::temp_dir().join(format!("kiwi-archiver-store-{b}"));
    let store = ContentStore::new(&root);
    let files: HashMap<String, Bytes> =
        [("a.txt", &b"aa"[..]), ("b.txt", b"bb")].into_iter().map(|(p, d)| (p.to_owned(), Bytes::from(d))).collect();
    let manifest = files
        .iter()
        .map(|(p, d)| ManifestEntry { path: p.clone(), sha1: format!("sha-{p}"), size: d.len() as i64 })
        .collect();
    let r = MongoRepo::new(state.clone());
    let s = sync_branch(&b, "x", &Cdn1 { manifest, files }, &store, &r, 2).await.unwrap();
    assert!(s.changed);
    assert_eq!(s.counts.added, 2);
    let again = sync_branch(&b, "x", &Cdn1 { manifest: Vec::new(), files: HashMap::new() }, &store, &r, 2).await;
    assert_eq!(again.unwrap().counts.removed, 2);

    let states = state.mongo().collection::<Document>(repo::STATE);
    states
        .insert_one(doc! {"branch": &b, "path": "x", "content_sha256": "s", "fnv_hash": null, "size": 1,
        "archive": null, "archive_index": null, "last_ordinal": 0})
        .await
        .unwrap();
    state
        .mongo()
        .collection::<Document>(repo::CHANGES)
        .insert_one(doc! {"branch": &b, "ordinal": 7_i64, "path": "x", "type": "added"})
        .await
        .unwrap();
    maintenance::backfill_last_ordinal(state.clone(), b.clone()).await;
    let st = maintenance::get_backfill_status(&b);
    assert!(st.done && !st.running && st.updated == 1, "{st:?}");
    let x = states.find_one(doc! {"branch": &b, "path": "x"}).await.unwrap().unwrap();
    assert_eq!(x.get("last_ordinal"), Some(&Bson::Int64(7)));
    assert!(states.find_one(doc! {"branch": &b, "path": "a.txt"}).await.unwrap().is_none());
    cleanup(&state, &b).await;
    let _ = std::fs::remove_dir_all(&root);
}

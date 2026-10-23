//! Golden checks against `native/port/parity/gen_archiver_fixtures.py`: the
//! parsers and diffs, every `sync_branch` scenario (state, change-log,
//! versions and CAS layout after each step), the documents and pymongo
//! operations the repo writes, the backfill batches and the admin routes.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
};

use axum::{
    Router,
    body::Body,
    extract::Request,
    http::{HeaderMap, Method, StatusCode},
};
use bytes::Bytes;
use chrono::{DateTime, Utc};
use hyper_util::{client::legacy::Client, rt::TokioExecutor};
use kiwi_core::update_archive::{self as ua, ManifestEntry, Ordered, Pointer, Sidecar};
use mongodb::bson::{Bson, Document, doc};
use serde_json::{Value, json};
use tower::ServiceExt;

use super::{
    Counts, Summary, SyncError,
    cas::ContentStore,
    cdn::{self, Cdn},
    ingest::sync_branch,
    maintenance::{self, BackfillStatus, BranchQuery},
    repo::{self, BranchDoc, BranchUpdate, Op, Repo, VersionDoc},
};
use crate::{
    config::Settings,
    db::Db,
    dt,
    error::ApiError,
    middleware::{EmbedSource, Mw, MwConfig, forwarded::TrustedHosts, outer, wrap},
    pyjson,
    state::{AppState, Inner},
    validation::{FromParams, Params},
};

fn golden() -> &'static Value {
    static G: OnceLock<Value> = OnceLock::new();
    G.get_or_init(|| serde_json::from_str(include_str!("../../testdata/archiver/golden.json")).expect("fixture parses"))
}

fn s(v: &Value) -> &str {
    v.as_str().unwrap_or_else(|| panic!("not a string: {v}"))
}

fn i(v: &Value) -> i64 {
    v.as_i64().unwrap_or_else(|| panic!("not an int: {v}"))
}

fn unhex(v: &Value) -> Vec<u8> {
    hex::decode(s(v)).expect("hex")
}

/// Canonical extended JSON (BSON types included) of a document.
fn ext(d: &Document) -> Value {
    Bson::Document(d.clone()).into_canonical_extjson()
}

// --- pure ---------------------------------------------------------------------------

#[test]
fn pointers() {
    for c in golden()["pure"]["pointers"].as_array().unwrap() {
        let got = ua::parse_pointer(s(&c["text"]));
        match (&got, c.get("ok")) {
            (Ok(p), Some(ok)) => {
                assert_eq!(p.version, s(&ok["version"]), "{c}");
                assert_eq!(p.content_path, s(&ok["content_path"]), "{c}");
                assert_eq!(p.motd, s(&ok["motd"]), "{c}");
            }
            (Err(e), None) => assert_eq!(e, s(&c["error"]), "{c}"),
            _ => panic!("{c}: got {got:?}"),
        }
    }
}

#[test]
fn manifests() {
    for c in golden()["pure"]["manifests"].as_array().unwrap() {
        let got = ua::parse_manifest(s(&c["text"]));
        match (&got, c.get("ok")) {
            (Ok((v, entries)), Some(ok)) => {
                assert_eq!(v, s(&ok[0]), "{c}");
                let want: Vec<ManifestEntry> = ok[1]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|e| ManifestEntry {
                        path: s(&e["path"]).into(),
                        sha1: s(&e["sha1"]).into(),
                        size: i(&e["size"]),
                    })
                    .collect();
                assert_eq!(entries, &want, "{c}");
            }
            (Err(e), None) => assert_eq!(e, s(&c["error"]), "{c}"),
            _ => panic!("{c}: got {got:?}"),
        }
    }
}

#[test]
fn urls() {
    for c in golden()["pure"]["urls"].as_array().unwrap() {
        let a: Vec<&str> = c["args"].as_array().unwrap().iter().map(s).collect();
        assert_eq!(ua::pointer_url(a[0], a[1], "kiwi-pts.txt"), s(&c["pointer"]));
        assert_eq!(ua::manifest_url(a[0], a[1], a[2], "V1"), s(&c["manifest"]));
        assert_eq!(ua::file_url(a[0], a[1], a[2], a[3], a[4]), s(&c["file"]));
    }
    let branches: Vec<(String, String)> = serde_json::from_value(golden()["pure"]["branches"].clone()).unwrap();
    let ours: Vec<(String, String)> = ua::BRANCHES.iter().map(|(a, b)| ((*a).into(), (*b).into())).collect();
    assert_eq!(ours, branches);
}

fn entries_json(es: &[ua::TfiEntry]) -> Value {
    json!(es.iter().map(|e| json!([e.name, e.archive_index, e.offset, e.size, e.fnv_hash])).collect::<Vec<_>>())
}

#[test]
fn tfi_index() {
    for c in golden()["pure"]["tfi"].as_array().unwrap() {
        match ua::parse_tfi(&unhex(&c["hex"])) {
            Ok(es) => assert_eq!(entries_json(&es), c["entries"], "{c}"),
            Err(e) => assert_eq!(e, s(&c["error"]), "{c}"),
        }
    }
    for c in golden()["pure"]["archive_size"].as_array().unwrap() {
        let es: Vec<ua::TfiEntry> = c[0]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| ua::TfiEntry {
                name: s(&e[0]).into(),
                archive_index: i(&e[1]) as u32,
                offset: i(&e[2]) as u32,
                size: i(&e[3]) as u32,
                fnv_hash: i(&e[4]) as u32,
            })
            .collect();
        assert_eq!(ua::archive_size(&es, i(&c[1]) as u32), i(&c[2]) as u64);
    }
}

#[test]
fn tfa_inflate() {
    use sha2::Digest;
    for c in golden()["pure"]["inflate"].as_array().unwrap() {
        let got = ua::decompress_tfa(&unhex(&c["hex"]), i(&c["hint"]) as u64);
        match (&got, c.get("error")) {
            (Ok(data), None) => {
                assert_eq!(data.len() as i64, i(&c["len"]), "{}", c["name"]);
                assert_eq!(hex::encode(sha2::Sha256::digest(data)), s(&c["sha"]), "{}", c["name"]);
            }
            (Err(e), Some(want)) => assert_eq!(e, s(want), "{}", c["name"]),
            _ => panic!("{}: got {got:?}", c["name"]),
        }
    }
}

#[test]
fn manifest_diff() {
    for c in golden()["pure"]["classify"].as_array().unwrap() {
        let old: Ordered<Sidecar> = c["old"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| (s(&r[0]).to_owned(), Sidecar { sha1: s(&r[1]).into(), size: i(&r[2]) }))
            .collect();
        let new: Vec<ManifestEntry> = c["new"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| ManifestEntry { path: s(&e["path"]).into(), sha1: s(&e["sha1"]).into(), size: i(&e["size"]) })
            .collect();
        let plan = ua::classify_manifest_diff(&old, &new);
        let want = &c["plan"];
        assert_eq!(json!(plan.changed_loose), want["changed_loose"]);
        assert_eq!(json!(plan.removed_loose), want["removed_loose"]);
        let dirs: Vec<Value> =
            plan.dir_work.iter().map(|w| json!([w.directory, w.tfi_path, w.changed_archives])).collect();
        assert_eq!(json!(dirs), want["dir_work"]);
        let map: Vec<Value> = plan.new_map.values().map(|e| json!([e.path, e.sha1, e.size])).collect();
        assert_eq!(json!(map), want["new_map"]);
        assert_eq!(json!(plan.any()), want["any"]);
    }
    for c in golden()["pure"]["diff_logical"].as_array().unwrap() {
        let old: Ordered<Option<i64>> =
            c["old"].as_array().unwrap().iter().map(|r| (s(&r[0]).to_owned(), r[1].as_i64())).collect();
        let new: Ordered<u32> =
            c["new"].as_array().unwrap().iter().map(|r| (s(&r[0]).to_owned(), i(&r[1]) as u32)).collect();
        let d = ua::diff_logical(&old, &new);
        assert_eq!(json!({"added": d.added, "modified": d.modified, "removed": d.removed}), c["out"]);
    }
    for c in golden()["pure"]["archive_index_of"].as_array().unwrap() {
        assert_eq!(json!(ua::archive_index_of(s(&c[0]))), c[1], "{c}");
    }
}

#[test]
fn cdn_texts() {
    for c in golden()["pure"]["status_messages"].as_array().unwrap() {
        let status = StatusCode::from_u16(i(&c["status"]) as u16).unwrap();
        let msg = cdn::status_message(status, s(&c["reason"]), s(&c["url"]), c["location"].as_str().map(str::to_owned));
        if status.is_success() {
            assert!(c["message"].is_null());
        } else {
            assert_eq!(msg, s(&c["message"]));
        }
    }
    for c in golden()["pure"]["text"].as_array().unwrap() {
        assert_eq!(cdn::text_of(&unhex(&c["hex"]), c["content_type"].as_str()), s(&c["text"]), "{c}");
    }
}

#[test]
fn os_error_text() {
    let e = std::io::Error::from_raw_os_error(2);
    let SyncError::Io(t) = SyncError::io(e) else { panic!() };
    assert!(t.starts_with("[Errno 2] "), "{t}");
    assert!(!t.contains("os error"), "{t}");
}

// --- sync_branch over fakes --------------------------------------------------------------

/// A Python dict: insertion order, `pop` + re-insert moves a key to the end.
#[derive(Clone)]
struct PyDict<V>(Vec<(String, V)>);

impl<V> Default for PyDict<V> {
    fn default() -> Self {
        PyDict(Vec::new())
    }
}

impl<V: Clone> PyDict<V> {
    fn set(&mut self, k: &str, v: V) {
        match self.0.iter_mut().find(|(key, _)| key == k) {
            Some(slot) => slot.1 = v,
            None => self.0.push((k.to_owned(), v)),
        }
    }
    fn get(&self, k: &str) -> Option<&V> {
        self.0.iter().find(|(key, _)| key == k).map(|(_, v)| v)
    }
    fn pop(&mut self, k: &str) {
        self.0.retain(|(key, _)| key != k);
    }
}

#[derive(Clone)]
struct StateRow {
    sha: String,
    fnv: Option<i64>,
    size: i64,
    archive: Option<String>,
    archive_index: Option<i64>,
    last_ordinal: i64,
}

#[derive(Default)]
struct FakeData {
    manifest: PyDict<Sidecar>,
    state: PyDict<StateRow>,
    changes: Vec<Value>,
    versions: Vec<serde_json::Map<String, Value>>,
    branch: serde_json::Map<String, Value>,
}

/// The ingest test's in-memory repo: writes applied at once.
#[derive(Default)]
struct FakeRepo(Mutex<FakeData>);

impl FakeRepo {
    fn snapshot(&self) -> Value {
        let d = self.0.lock().unwrap();
        let mut manifest: Vec<(String, Value)> =
            d.manifest.0.iter().map(|(p, v)| (p.clone(), json!([p, v.sha1, v.size]))).collect();
        manifest.sort_by(|a, b| a.0.cmp(&b.0));
        let mut state: Vec<(String, Value)> = d
            .state
            .0
            .iter()
            .map(|(p, r)| (p.clone(), json!([p, r.sha, r.fnv, r.size, r.archive, r.archive_index, r.last_ordinal])))
            .collect();
        state.sort_by(|a, b| a.0.cmp(&b.0));
        let mut changes = d.changes.clone();
        changes.sort_by(|a, b| (i(&a[0]), s(&a[1]), s(&a[2])).cmp(&(i(&b[0]), s(&b[1]), s(&b[2]))));
        json!({
            "manifest": manifest.into_iter().map(|(_, v)| v).collect::<Vec<_>>(),
            "state": state.into_iter().map(|(_, v)| v).collect::<Vec<_>>(),
            "changes": changes,
            "versions": d.versions,
            "branch": d.branch,
        })
    }
}

impl Repo for FakeRepo {
    async fn get_manifest_sidecar(&self, _: &str) -> Result<Ordered<Sidecar>, SyncError> {
        Ok(self.0.lock().unwrap().manifest.0.iter().cloned().collect())
    }

    async fn state_exists(&self, _: &str, path: &str) -> Result<bool, SyncError> {
        Ok(self.0.lock().unwrap().state.get(path).is_some())
    }

    async fn get_archive_state(&self, _: &str, directory: &str) -> Result<Ordered<Option<i64>>, SyncError> {
        let d = self.0.lock().unwrap();
        Ok(d.state
            .0
            .iter()
            .filter(|(_, r)| r.archive.as_deref() == Some(directory))
            .map(|(p, r)| (p.clone(), r.fnv))
            .collect())
    }

    async fn begin_version(&self, _: &str, version_tag: &str, pointer: &Pointer) -> Result<(i64, bool), SyncError> {
        let mut d = self.0.lock().unwrap();
        if let Some(v) = d.versions.iter().find(|v| v["status"] == "in_progress") {
            return Ok((i(&v["ordinal"]), true));
        }
        let ordinal = d.versions.iter().map(|v| i(&v["ordinal"])).max().unwrap_or(0) + 1;
        let v = json!({"ordinal": ordinal, "version_tag": version_tag, "status": "in_progress",
                       "content_path": pointer.content_path, "motd": pointer.motd});
        d.versions.push(v.as_object().unwrap().clone());
        Ok((ordinal, false))
    }

    async fn record_change(
        &self,
        _: &str,
        ordinal: i64,
        path: &str,
        change_type: &str,
        sha: Option<&str>,
        fnv: Option<i64>,
        size: i64,
    ) -> Result<(), SyncError> {
        self.0.lock().unwrap().changes.push(json!([ordinal, path, change_type, sha, fnv, size]));
        Ok(())
    }

    async fn upsert_state(
        &self,
        _: &str,
        path: &str,
        sha: &str,
        fnv: Option<i64>,
        size: i64,
        archive: Option<&str>,
        archive_index: Option<i64>,
        last_ordinal: i64,
    ) -> Result<(), SyncError> {
        let row =
            StateRow { sha: sha.into(), fnv, size, archive: archive.map(str::to_owned), archive_index, last_ordinal };
        self.0.lock().unwrap().state.set(path, row);
        Ok(())
    }

    async fn remove_state(&self, _: &str, path: &str) -> Result<(), SyncError> {
        self.0.lock().unwrap().state.pop(path);
        Ok(())
    }

    async fn set_manifest_entry(&self, _: &str, path: &str, sha1: &str, size: i64) -> Result<(), SyncError> {
        self.0.lock().unwrap().manifest.set(path, Sidecar { sha1: sha1.into(), size });
        Ok(())
    }

    async fn remove_manifest_entry(&self, _: &str, path: &str) -> Result<(), SyncError> {
        self.0.lock().unwrap().manifest.pop(path);
        Ok(())
    }

    async fn finish_version(
        &self,
        _: &str,
        ordinal: i64,
        version_tag: &str,
        pointer: &Pointer,
        c: &Counts,
    ) -> Result<(), SyncError> {
        let mut d = self.0.lock().unwrap();
        for v in d.versions.iter_mut().filter(|v| i(&v["ordinal"]) == ordinal) {
            v.insert("status".into(), json!("complete"));
            for (k, n) in
                [("added", c.added), ("modified", c.modified), ("removed", c.removed), ("bytes_added", c.bytes_added)]
            {
                v.insert(k.into(), json!(n));
            }
        }
        d.branch =
            json!({"current_version": version_tag, "current_ordinal": ordinal, "content_path": pointer.content_path})
                .as_object()
                .unwrap()
                .clone();
        Ok(())
    }

    async fn touch_probe(&self, _: &str, content_path: &str, version_tag: &str) -> Result<(), SyncError> {
        let mut d = self.0.lock().unwrap();
        d.branch.insert("current_version".into(), json!(version_tag));
        d.branch.insert("content_path".into(), json!(content_path));
        Ok(())
    }
}

struct FakeCdn {
    version: String,
    content_path: String,
    manifest: Vec<ManifestEntry>,
    files: std::collections::HashMap<String, Bytes>,
    fail: std::collections::HashMap<String, String>,
    pointer_error: Option<String>,
}

impl Cdn for FakeCdn {
    async fn fetch_pointer(&self, _: &str) -> Result<Pointer, SyncError> {
        if let Some(e) = &self.pointer_error {
            return Err(SyncError::Cdn(e.clone()));
        }
        Ok(Pointer {
            version: self.version.clone(),
            content_path: self.content_path.clone(),
            motd: format!("m-{}", self.version),
        })
    }

    async fn fetch_manifest(&self, _: &str, _: &str) -> Result<(String, Vec<ManifestEntry>), SyncError> {
        Ok((format!("M-{}", self.version), self.manifest.clone()))
    }

    async fn download_file(&self, _: &str, path: &str, _: &str, expected: Option<i64>) -> Result<Bytes, SyncError> {
        tokio::task::yield_now().await;
        if let Some(e) = self.fail.get(path) {
            return Err(SyncError::Cdn(e.clone()));
        }
        let data = self.files[path].clone();
        if let Some(want) = expected
            && data.len() as i64 != want
        {
            return Err(SyncError::Cdn(format!("size mismatch for {path}: got {}, manifest {want}", data.len())));
        }
        Ok(data)
    }
}

fn summary_json(sm: &Summary) -> Value {
    let mut m = serde_json::Map::new();
    m.insert("branch".into(), json!(sm.branch));
    m.insert("version".into(), json!(sm.version));
    m.insert("changed".into(), json!(sm.changed));
    if let Some(o) = sm.ordinal {
        m.insert("ordinal".into(), json!(o));
    }
    let c = &sm.counts;
    for (k, n) in [("added", c.added), ("modified", c.modified), ("removed", c.removed), ("bytes_added", c.bytes_added)]
    {
        m.insert(k.into(), json!(n));
    }
    Value::Object(m)
}

fn cas_listing(root: &Path) -> Value {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() { walk(&p, out) } else { out.push(p) }
        }
    }
    let mut files = Vec::new();
    walk(&root.join("objects"), &mut files);
    let mut rows: Vec<(String, u64)> = files
        .iter()
        .map(|p| {
            let rel =
                p.strip_prefix(root).unwrap().components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>();
            (rel.join("/"), std::fs::metadata(p).unwrap().len())
        })
        .collect();
    rows.sort();
    json!(rows.into_iter().map(|(p, n)| json!([p, n])).collect::<Vec<_>>())
}

fn temp_dir(tag: &str) -> PathBuf {
    let mut raw = [0u8; 6];
    getrandom::fill(&mut raw).unwrap();
    let p = std::env::temp_dir().join(format!("kiwi-archiver-{tag}-{}", hex::encode(raw)));
    std::fs::create_dir_all(&p).unwrap();
    p
}

#[tokio::test]
async fn ingest_scenarios() {
    for sc in golden()["ingest"].as_array().unwrap() {
        let name = s(&sc["name"]);
        let branch = s(&sc["branch"]);
        let pointer_file = ua::BRANCHES.iter().find(|(b, _)| *b == branch).unwrap().1;
        let root = temp_dir(name);
        let store = ContentStore::new(&root);
        let repo = FakeRepo::default();
        for (n, st) in sc["steps"].as_array().unwrap().iter().enumerate() {
            let what = format!("{name} step {n} ({})", s(&st["version"]));
            let cdn = FakeCdn {
                version: s(&st["version"]).into(),
                content_path: format!("cp-{branch}"),
                manifest: st["cdn_manifest"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|e| ManifestEntry {
                        path: s(&e["path"]).into(),
                        sha1: s(&e["sha1"]).into(),
                        size: i(&e["size"]),
                    })
                    .collect(),
                files: st["cdn_files"]
                    .as_object()
                    .unwrap()
                    .iter()
                    .map(|(k, v)| (k.clone(), Bytes::from(unhex(v))))
                    .collect(),
                fail: st["fail"].as_object().unwrap().iter().map(|(k, v)| (k.clone(), s(v).to_owned())).collect(),
                pointer_error: st["pointer_error"].as_str().map(str::to_owned),
            };
            match sync_branch(branch, pointer_file, &cdn, &store, &repo, 3).await {
                Ok(sm) => assert_eq!(summary_json(&sm), st["summary"], "{what}: summary"),
                Err(e) => assert_eq!(json!(e.to_string()), st["error"], "{what}: error"),
            }
            let snap = repo.snapshot();
            for k in ["manifest", "state", "changes", "versions", "branch"] {
                assert_eq!(snap[k], st[k], "{what}: {k}");
            }
            assert_eq!(cas_listing(&root), st["cas"], "{what}: cas");
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}

#[test]
fn cas_put_is_idempotent() {
    let root = temp_dir("cas");
    let store = ContentStore::new(&root);
    let (sha, created) = store.put(b"hello").unwrap();
    assert_eq!(sha, "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824");
    assert!(created);
    assert_eq!(store.put(b"hello").unwrap(), (sha.clone(), false));
    assert!(store.has(&sha));
    assert_eq!(store.get(&sha).as_deref(), Some(&b"hello"[..]));
    assert_eq!(store.path_for(&sha), root.join("objects").join("2c").join(&sha));
    let leftovers: Vec<_> = std::fs::read_dir(root.join("objects").join("2c")).unwrap().flatten().collect();
    assert_eq!(leftovers.len(), 1);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(store.path_for(&sha)).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    let _ = std::fs::remove_dir_all(&root);
}

// --- MongoUpdateRepo --------------------------------------------------------------------

fn op_json(op: &Op) -> Value {
    match op {
        Op::Update { filter, set, upsert } => json!({"update": ext(filter), "set": ext(set), "upsert": upsert}),
        Op::Delete { filter } => json!({"delete": ext(filter)}),
    }
}

fn doc_of(v: &Value) -> Document {
    match Bson::try_from(v.clone()).expect("extended JSON") {
        Bson::Document(d) => d,
        other => panic!("not a document: {other}"),
    }
}

fn repo_now() -> DateTime<Utc> {
    dt::parse_utc(s(&golden()["repo"]["now"])).unwrap()
}

fn pointer() -> Pointer {
    let p = &golden()["repo"]["pointer"];
    Pointer { version: s(&p["version"]).into(), content_path: s(&p["content_path"]).into(), motd: s(&p["motd"]).into() }
}

fn sample_ops() -> [(&'static str, Vec<Op>); 3] {
    let sha = |c: &str| c.repeat(32);
    let (ab, cd, ef, z1) = (sha("ab"), sha("cd"), sha("ef"), sha("01"));
    let changes = vec![
        repo::change_op("live-us", 3, "ui/a.bin", "added", Some(&ab), Some(4_000_000_000), 12),
        repo::change_op("live-us", 4, "ui/b.bin", "removed", None, None, 0),
        repo::change_op("pts", 1 << 31, "x", "modified", Some(&cd), Some(5), 1 << 33),
    ];
    let state = vec![
        repo::state_op("live-us", "ui/a.bin", &ab, Some(4_000_000_000), 12, Some("ui"), Some(0), 3),
        repo::state_op("live-us", "Trove.exe", &ef, None, 1 << 31, None, None, (1 << 31) - 1),
        repo::delete_op("live-us", "gone.bin"),
        repo::state_op("live-us", "gone.bin", &z1, Some(1), 1, Some("d"), Some(7), 9),
    ];
    let manifest = vec![
        repo::manifest_op("live-us", "ui/index.tfi", "s1", 1 << 40),
        repo::delete_op("live-us", "old.tfa"),
        repo::manifest_op("live-us", "ui/archive0.tfa", "s2", 3),
    ];
    [(repo::CHANGES, changes), (repo::STATE, state), (repo::MANIFEST, manifest)]
}

#[test]
fn repo_write_ops() {
    let want = golden()["repo"]["ops"].as_array().unwrap();
    for (n, (coll, ops)) in sample_ops().iter().enumerate() {
        assert_eq!(want[n]["coll"], *coll);
        assert_eq!(want[n]["ordered"], false);
        assert_eq!(json!(ops.iter().map(op_json).collect::<Vec<_>>()), want[n]["ops"], "{coll}");
    }
}

/// The filter and `$set` key order pymongo sends (Value equality ignores it).
/// What mongod stores from them: `_id`, the filter fields in its own hash
/// order, then the `$set` fields in lexicographic order.
#[test]
fn repo_write_ops_keep_pymongo_key_order() {
    fn keys(v: &Value) -> Vec<String> {
        v.as_object().unwrap().keys().cloned().collect()
    }
    let want = golden()["repo"]["ops"].as_array().unwrap();
    for (n, (coll, ops)) in sample_ops().iter().enumerate() {
        for (op, w) in ops.iter().zip(want[n]["ops"].as_array().unwrap()) {
            let got = op_json(op);
            assert_eq!(keys(&got), keys(w), "{coll}");
            for k in ["update", "set", "delete"] {
                if let Some(v) = w.get(k) {
                    assert_eq!(keys(&got[k]), keys(v), "{coll} {k}");
                }
            }
        }
        for cmd in repo::bulk_commands(coll, ops) {
            for u in cmd.get_array("updates").into_iter().flatten() {
                let u = u.as_document().unwrap();
                assert_eq!(u.keys().collect::<Vec<_>>(), ["q", "u", "multi", "upsert"]);
                assert_eq!(u.get_document("u").unwrap().keys().collect::<Vec<_>>(), ["$set"]);
            }
        }
    }
    let changes = &sample_ops()[0].1;
    let Op::Update { filter, set, .. } = &changes[0] else { panic!() };
    assert_eq!(filter.keys().collect::<Vec<_>>(), ["branch", "ordinal", "path"]);
    assert_eq!(set.keys().collect::<Vec<_>>(), ["type", "content_sha256", "fnv_hash", "size"]);
}

#[test]
fn unordered_bulk_runs_updates_before_deletes() {
    let ops = [
        repo::delete_op("b", "x"),
        repo::state_op("b", "x", "s", None, 1, None, None, 1),
        repo::delete_op("b", "y"),
        repo::manifest_op("b", "z", "q", 2),
    ];
    let cmds = repo::bulk_commands("update_state", &ops);
    assert_eq!(cmds.len(), 2);
    assert_eq!(cmds[0].get_str("update").unwrap(), "update_state");
    let ups = cmds[0].get_array("updates").unwrap();
    assert_eq!(ups.len(), 2);
    assert_eq!(ups[0].as_document().unwrap().get_document("q").unwrap(), &doc! {"branch": "b", "path": "x"});
    assert_eq!(cmds[1].get_str("delete").unwrap(), "update_state");
    let dels: Vec<&Document> = cmds[1].get_array("deletes").unwrap().iter().map(|d| d.as_document().unwrap()).collect();
    assert_eq!(dels[0], &doc! {"q": {"branch": "b", "path": "x"}, "limit": 1});
    assert_eq!(dels[1], &doc! {"q": {"branch": "b", "path": "y"}, "limit": 1});
    assert!(!cmds[0].get_bool("ordered").unwrap());
    assert!(repo::bulk_commands("c", &[]).is_empty());
}

#[test]
fn repo_documents() {
    let r = &golden()["repo"];
    let now = repo_now();
    let p = pointer();

    let last = VersionDoc::load(&doc_of(&r["begin"]["last"]), now).unwrap();
    assert_eq!(json!([last.ordinal + 1, false]), r["begin"]["result"]);
    let log = r["begin"]["log"].as_array().unwrap();
    let mut inserted = ext(&repo::new_version("live-us", last.ordinal + 1, "MV-7", &p, now).to_doc());
    assert!(inserted["captured_at"]["$date"].is_object());
    inserted["captured_at"] = json!("<now>");
    assert_eq!((log[0]["coll"].clone(), inserted), (json!(repo::VERSIONS), log[0]["insert"].clone()));
    let mut b = BranchDoc::new("live-us");
    BranchUpdate::syncing().apply(&mut b);
    assert_eq!((log[1]["coll"].clone(), ext(&b.to_doc())), (json!(repo::BRANCHES), log[1]["insert"].clone()));
    assert_eq!(r["resumed"]["result"], json!([9, true]));

    let f = &r["finish"];
    let mut v = VersionDoc::load(&doc_of(&f["version"]), now).unwrap();
    let counts = Counts { added: 3, modified: 1 << 31, removed: 0, bytes_added: 1 << 35 };
    repo::complete_version(&mut v, &counts, now);
    let flog = f["log"].as_array().unwrap();
    assert_eq!(ext(&v.to_doc()), flog[0]["save"]);
    let branch = BranchDoc::load(&doc_of(&f["branch"])).unwrap();
    let mut b = branch.clone();
    BranchUpdate::finished(&p, "MV-7", 7, now).apply(&mut b);
    assert_eq!(ext(&b.to_doc()), flog[1]["save"]);
    assert_eq!(flog[2], json!({"publish": "game_update"}));

    let mut b = branch.clone();
    BranchUpdate::probed("cp2", "MV-8", now).apply(&mut b);
    assert_eq!(ext(&b.to_doc()), r["touch"]["log"][0]["save"]);

    let mut b = branch.clone();
    BranchUpdate::error(s(&r["error"]["message"])).apply(&mut b);
    assert_eq!(ext(&b.to_doc()), r["error"]["log"][0]["save"]);

    let mut b = BranchDoc::new("pts");
    BranchUpdate::error("Server error '500 Internal Server Error' for url 'http://h'").apply(&mut b);
    assert_eq!(ext(&b.to_doc()), r["error_new"]["log"][0]["insert"]);
}

#[test]
fn model_validation() {
    assert!(VersionDoc::load(&doc! {"branch": "b", "version_tag": "v"}, Utc::now()).is_err());
    assert!(BranchDoc::load(&doc! {"branch": 5}).is_err());
    let b = BranchDoc::load(&doc! {"branch": "b"}).unwrap();
    assert_eq!((b.status.as_str(), b.current_ordinal, b.current_version), ("idle", 0, None));
    assert_eq!(
        repo::load_state(&doc! {"branch": "b", "path": "p", "content_sha256": "s", "fnv_hash": 7_i64}).unwrap(),
        ("p".into(), Some(7))
    );
    assert!(repo::load_state(&doc! {"branch": "b", "path": "p"}).is_err());
    assert!(repo::load_sidecar(&doc! {"branch": "b", "path": "p", "sha1": "s"}).is_err());
}

// --- backfill --------------------------------------------------------------------------

#[tokio::test]
async fn backfill_batches() {
    let g = &golden()["backfill"];
    assert_eq!(json!(maintenance::pipeline("pts").iter().map(ext).collect::<Vec<_>>()), g["log"][0]["pipeline"]);
    let rows: Vec<anyhow::Result<Document>> = (0..2003)
        .map(|n: i64| {
            let last = if n % 3 == 0 { Bson::Int64(n) } else { Bson::Int32((n % 11) as i32) };
            Ok(doc! {"_id": format!("p{n}"), "last": last})
        })
        .collect();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let rec = calls.clone();
    // Its own branch name: the status map is process-wide and the route test reads "pts".
    let updated = maintenance::apply_rows("bf-pts", futures::stream::iter(rows), move |ops| {
        let rec = rec.clone();
        async move {
            let n = ops.len() as i64;
            rec.lock().unwrap().push(ops);
            Ok(n - 1)
        }
    })
    .await
    .unwrap();
    assert_eq!(updated, i(&g["updated"]));
    let calls = calls.lock().unwrap();
    let want = g["calls"].as_array().unwrap();
    assert_eq!(calls.len(), want.len());
    for (ops, w) in calls.iter().zip(want) {
        assert_eq!(ops.len() as i64, i(&w["n"]));
        for (op, key) in [(&ops[0], "first"), (ops.last().unwrap(), "last")] {
            let Op::Update { filter, set, upsert } = op else { panic!() };
            assert!(!upsert);
            let mut want = w[key]["filter"].clone();
            want["branch"] = json!("bf-pts");
            assert_eq!(ext(filter), want);
            assert_eq!(ext(&doc! {"$set": set.clone()}), w[key]["update"]);
        }
    }
}

#[tokio::test]
async fn backfill_status_tracking() {
    let branch = "status-test";
    maintenance::tracked(branch, async { Ok(5) }).await;
    let st = maintenance::get_backfill_status(branch);
    assert!(!st.running && st.done && st.updated == 5 && st.error.is_none());
    assert!(st.started_at.is_some() && st.finished_at >= st.started_at);
    maintenance::tracked(branch, async { Err(anyhow::anyhow!("x".repeat(600))) }).await;
    let st = maintenance::get_backfill_status(branch);
    assert!(!st.running && !st.done && st.updated == 0);
    assert_eq!(st.error.as_deref(), Some(&*"x".repeat(500)));
    let want = &golden()["backfill"]["status"];
    assert_eq!(want["running"], false);
    assert_eq!(want["done"], true);
}

// --- routes ---------------------------------------------------------------------------

fn query_branch(url: &str) -> String {
    let query = url.split_once('?').map(|(_, q)| q);
    let headers = HeaderMap::new();
    let mut p = Params::new(Vec::new(), query, &headers);
    let v = BranchQuery::from_params(&mut p);
    p.v.finish(v).ok().unwrap().0
}

fn assert_reply(got: Result<Value, ApiError>, case: &Value) {
    let what = format!("{} {}", s(&case["method"]), s(&case["url"]));
    match got {
        Ok(v) => {
            assert_eq!(case["status"], 200, "{what}");
            assert_eq!(pyjson::pydantic(&v), s(&case["body"]), "{what}");
        }
        Err(e) => {
            let want: Value = serde_json::from_str(s(&case["body"])).unwrap();
            assert_eq!(i64::from(e.status.as_u16()), i(&case["status"]), "{what}");
            assert_eq!(e.code, s(&want["error"]["code"]), "{what}");
            assert_eq!(e.message, s(&want["error"]["message"]), "{what}");
            assert!(want["error"]["details"].is_null());
        }
    }
}

#[test]
fn admin_route_replies() {
    let now = repo_now();
    let cases = golden()["routes"]["authed"].as_array().unwrap();
    let mut started = Vec::new();
    for (n, c) in cases.iter().enumerate() {
        match n {
            10 => maintenance::with_status("pts", |st| {
                *st = BackfillStatus {
                    running: false,
                    updated: 2003,
                    done: true,
                    started_at: Some(now),
                    finished_at: Some(now.with_nanosecond_floor()),
                    error: None,
                }
            }),
            11 => maintenance::with_status("pts", |st| {
                *st = BackfillStatus {
                    running: false,
                    updated: 1000,
                    done: false,
                    started_at: Some(now.with_micros(1000)),
                    finished_at: Some(now),
                    error: Some("boom \u{e9} \"quoted\"\n".into()),
                }
            }),
            _ => {}
        }
        let branch = query_branch(s(&c["url"]));
        let got = if c["method"] == "GET" {
            maintenance::status_reply(&branch)
        } else {
            maintenance::start_reply(&branch).map(|(body, start)| {
                if start {
                    started.push(branch.clone());
                    maintenance::with_status(&branch, |st| {
                        *st = BackfillStatus { running: true, started_at: Some(now), ..Default::default() }
                    });
                }
                body
            })
        };
        assert_reply(got, c);
    }
    assert_eq!(json!(started), golden()["routes"]["started"]);
}

trait Micros {
    fn with_nanosecond_floor(self) -> Self;
    fn with_micros(self, us: u32) -> Self;
}

impl Micros for DateTime<Utc> {
    fn with_nanosecond_floor(self) -> Self {
        chrono::Timelike::with_nanosecond(&self, 0).unwrap()
    }
    fn with_micros(self, us: u32) -> Self {
        chrono::Timelike::with_nanosecond(&self, us * 1000).unwrap()
    }
}

fn state() -> AppState {
    let settings = Settings::from_vars([("PY_UPSTREAM", "http://py:8000")]).unwrap();
    let upstream = Client::builder(TokioExecutor::new()).build_http();
    AppState { inner: Arc::new(Inner { settings, db: Db { mongo: None, redis: None, pg: None }, upstream }) }
}

fn stack() -> Router {
    let mw = Arc::new(Mw::new(
        MwConfig::from_lookup(&|_| None),
        None,
        EmbedSource::Fixed(Vec::new()),
        TrustedHosts::parse("127.0.0.1"),
    ));
    outer(
        wrap(super::router(), mw, None).fallback(|| async { (StatusCode::IM_A_TEAPOT, "proxied") }).with_state(state()),
    )
}

#[tokio::test]
async fn unauthenticated_routing() {
    for c in golden()["routes"]["unauth"].as_array().unwrap() {
        let (method, url) = (s(&c["method"]), s(&c["url"]));
        let want: Value = serde_json::from_str(s(&c["body"])).unwrap();
        let req = Request::builder()
            .method(Method::from_bytes(method.as_bytes()).unwrap())
            .uri(url)
            .header("host", "testserver")
            .header("x-request-id", s(&want["error"]["request_id"]))
            .body(Body::empty())
            .unwrap();
        let resp = stack().oneshot(req).await.unwrap();
        let what = format!("{method} {url}");
        assert_eq!(i64::from(resp.status().as_u16()), i(&c["status"]), "{what}");
        for h in ["allow", "www-authenticate", "content-type"] {
            assert_eq!(resp.headers().get(h).map(|v| v.to_str().unwrap()), c[h].as_str(), "{what}: {h}");
        }
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        if method != "HEAD" {
            assert_eq!(String::from_utf8(body.to_vec()).unwrap(), s(&c["body"]), "{what}: body");
        }
    }
}

// --- the CDN client over a local socket ---------------------------------------------------

/// One canned HTTP/1.1 answer per request target; every request head is kept.
async fn fake_http(answers: Vec<(&'static str, Vec<u8>)>) -> (String, Arc<Mutex<Vec<String>>>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else { return };
            let mut buf = Vec::new();
            let mut chunk = [0u8; 4096];
            while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                match sock.read(&mut chunk).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => buf.extend_from_slice(&chunk[..n]),
                }
            }
            let head = String::from_utf8_lossy(&buf).into_owned();
            let target = head.split(' ').nth(1).unwrap_or("").to_owned();
            log.lock().unwrap().push(head);
            let raw = answers.iter().find(|(t, _)| *t == target).map(|(_, r)| r.clone()).unwrap_or_else(|| {
                b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec()
            });
            let _ = sock.write_all(&raw).await;
            let _ = sock.shutdown().await;
        }
    });
    (base, seen)
}

fn answer(status_line: &str, headers: &str, body: &[u8]) -> Vec<u8> {
    let mut out =
        format!("HTTP/1.1 {status_line}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n", body.len())
            .into_bytes();
    out.extend_from_slice(body);
    out
}

#[tokio::test]
async fn cdn_client_over_http() {
    let prefix = "/kiwi-live-client-patch/";
    let (base, seen) = fake_http(vec![
        (
            "/kiwi-live-client-patch//public/kiwi-live-us.txt",
            answer("200 OK", "Content-Type: text/plain\r\n", b"V1|content/cp|x|motd\r\n"),
        ),
        (
            "/kiwi-live-client-patch//content/cp/V1.manifest",
            answer("200 OK", "", b"version V1\r\na\\b c+d.bin:s1:3\r\n"),
        ),
        ("/kiwi-live-client-patch//content/cp/recovery/a/b%20c%2Bd.bin?sha1=s1", answer("200 OK", "", b"xyz")),
        ("/kiwi-live-client-patch//public/kiwi-pts.txt", answer("503 Busy Now", "", b"")),
        (
            "/kiwi-live-client-patch//content/cp/V2.manifest",
            answer("301 Moved Permanently", "Location: /elsewhere\r\n", b""),
        ),
    ])
    .await;
    let c = cdn::CdnClient::new(&base, prefix);
    let p = c.fetch_pointer("kiwi-live-us.txt").await.unwrap();
    assert_eq!((p.version.as_str(), p.content_path.as_str(), p.motd.as_str()), ("V1", "cp", "motd"));
    let (v, entries) = c.fetch_manifest("cp", "V1").await.unwrap();
    assert_eq!(v, "V1");
    let e = &entries[0];
    assert_eq!((e.path.as_str(), e.sha1.as_str(), e.size), ("a/b c+d.bin", "s1", 3));
    assert_eq!(&c.download_file("cp", &e.path, &e.sha1, Some(3)).await.unwrap()[..], b"xyz");
    let short = c.download_file("cp", &e.path, &e.sha1, Some(4)).await.unwrap_err();
    assert_eq!(short.to_string(), "size mismatch for a/b c+d.bin: got 3, manifest 4");

    let busy = c.fetch_pointer("kiwi-pts.txt").await.unwrap_err();
    let url = format!("{base}{prefix}/public/kiwi-pts.txt");
    let want = format!(
        "Server error '503 Busy Now' for url '{url}'\nFor more information check: https://developer.mozilla.org/en-US/docs/Web/HTTP/Status/503"
    );
    assert_eq!(busy, SyncError::Http { status: 503, message: want });
    let moved = c.fetch_manifest("cp", "V2").await.unwrap_err().to_string();
    assert!(
        moved.contains("'301 Moved Permanently'") && moved.contains("\nRedirect location: '/elsewhere'\n"),
        "{moved}"
    );
    let missing = c.download_file("cp", "nope", "x", None).await.unwrap_err();
    assert!(matches!(missing, SyncError::Http { status: 404, .. }));

    let heads = seen.lock().unwrap().clone();
    assert_eq!(heads.len(), 7);
    for h in &heads {
        assert!(h.to_ascii_lowercase().contains("\r\nuser-agent: kiwiapi/1.0\r\n"), "{h}");
    }
    assert!(heads[2].starts_with("GET /kiwi-live-client-patch//content/cp/recovery/a/b%20c%2Bd.bin?sha1=s1 HTTP/1.1"));

    let dead = cdn::CdnClient::new("http://127.0.0.1:9", prefix).fetch_pointer("kiwi-live-us.txt").await.unwrap_err();
    assert_eq!(dead, SyncError::Transport("All connection attempts failed".into()));
}

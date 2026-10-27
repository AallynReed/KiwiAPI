//! `app/trove/updates/repo.py` `MongoUpdateRepo`: Beanie reads and the
//! version/branch documents, and the per-file writes buffered into pymongo
//! `bulk_write(ordered=False)` batches of 1000 (changes, then state, then the
//! manifest sidecar, which is the resume marker).

use std::{future::Future, sync::Mutex};

use chrono::{DateTime, Utc};
use futures::TryStreamExt;
use kiwi_core::update_archive::{Ordered, Pointer, Sidecar};
use mongodb::{
    Database,
    bson::{Bson, DateTime as BsonDateTime, Document, doc, oid::ObjectId},
    options::FindOptions,
};

use super::{Counts, SyncError};
use crate::{auth::pyutil::bson_int, dt, state::AppState};

pub const BRANCHES: &str = "update_branches";
pub const VERSIONS: &str = "update_versions";
pub const CHANGES: &str = "update_changes";
pub const STATE: &str = "update_state";
pub const MANIFEST: &str = "update_manifest";

const FLUSH_THRESHOLD: usize = 1000;

/// What `sync_branch` needs from persistence (an in-memory fake in tests).
pub trait Repo: Sync {
    fn get_manifest_sidecar(&self, branch: &str) -> impl Future<Output = Result<Ordered<Sidecar>, SyncError>> + Send;
    /// `state_get(...)` is truthy.
    fn state_exists(&self, branch: &str, path: &str) -> impl Future<Output = Result<bool, SyncError>> + Send;
    /// The state row's `content_sha256`, if the path is in the tree.
    fn state_sha(&self, branch: &str, path: &str) -> impl Future<Output = Result<Option<String>, SyncError>> + Send;
    /// `{path: fnv_hash}` of the state rows from one archive directory.
    fn get_archive_state(
        &self,
        branch: &str,
        directory: &str,
    ) -> impl Future<Output = Result<Ordered<Option<i64>>, SyncError>> + Send;
    fn begin_version(
        &self,
        branch: &str,
        version_tag: &str,
        pointer: &Pointer,
    ) -> impl Future<Output = Result<(i64, bool), SyncError>> + Send;
    #[allow(clippy::too_many_arguments)]
    fn record_change(
        &self,
        branch: &str,
        ordinal: i64,
        path: &str,
        change_type: &str,
        content_sha256: Option<&str>,
        fnv_hash: Option<i64>,
        size: i64,
    ) -> impl Future<Output = Result<(), SyncError>> + Send;
    #[allow(clippy::too_many_arguments)]
    fn upsert_state(
        &self,
        branch: &str,
        path: &str,
        content_sha256: &str,
        fnv_hash: Option<i64>,
        size: i64,
        archive: Option<&str>,
        archive_index: Option<i64>,
        last_ordinal: i64,
    ) -> impl Future<Output = Result<(), SyncError>> + Send;
    fn remove_state(&self, branch: &str, path: &str) -> impl Future<Output = Result<(), SyncError>> + Send;
    fn set_manifest_entry(
        &self,
        branch: &str,
        path: &str,
        sha1: &str,
        size: i64,
    ) -> impl Future<Output = Result<(), SyncError>> + Send;
    fn remove_manifest_entry(&self, branch: &str, path: &str) -> impl Future<Output = Result<(), SyncError>> + Send;
    fn finish_version(
        &self,
        branch: &str,
        ordinal: i64,
        version_tag: &str,
        pointer: &Pointer,
        counts: &Counts,
    ) -> impl Future<Output = Result<(), SyncError>> + Send;
    fn touch_probe(
        &self,
        branch: &str,
        content_path: &str,
        version_tag: &str,
    ) -> impl Future<Output = Result<(), SyncError>> + Send;
}

// --- pymongo operations ------------------------------------------------------------

/// One `UpdateOne` (`$set`) or `DeleteOne`.
#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    Update { filter: Document, set: Document, upsert: bool },
    Delete { filter: Document },
}

fn opt_str(s: Option<&str>) -> Bson {
    s.map_or(Bson::Null, |s| Bson::String(s.to_owned()))
}

fn opt_int(n: Option<i64>) -> Bson {
    n.map_or(Bson::Null, bson_int)
}

pub fn change_op(
    branch: &str,
    ordinal: i64,
    path: &str,
    change_type: &str,
    sha: Option<&str>,
    fnv: Option<i64>,
    size: i64,
) -> Op {
    Op::Update {
        filter: doc! {"branch": branch, "ordinal": bson_int(ordinal), "path": path},
        set: doc! {"type": change_type, "content_sha256": opt_str(sha), "fnv_hash": opt_int(fnv), "size": bson_int(size)},
        upsert: true,
    }
}

#[allow(clippy::too_many_arguments)]
pub fn state_op(
    branch: &str,
    path: &str,
    sha: &str,
    fnv: Option<i64>,
    size: i64,
    archive: Option<&str>,
    archive_index: Option<i64>,
    last_ordinal: i64,
) -> Op {
    Op::Update {
        filter: doc! {"branch": branch, "path": path},
        set: doc! {
            "content_sha256": sha, "fnv_hash": opt_int(fnv), "size": bson_int(size),
            "archive": opt_str(archive), "archive_index": opt_int(archive_index),
            "last_ordinal": bson_int(last_ordinal),
        },
        upsert: true,
    }
}

pub fn manifest_op(branch: &str, path: &str, sha1: &str, size: i64) -> Op {
    Op::Update {
        filter: doc! {"branch": branch, "path": path},
        set: doc! {"sha1": sha1, "size": bson_int(size)},
        upsert: true,
    }
}

pub fn delete_op(branch: &str, path: &str) -> Op {
    Op::Delete { filter: doc! {"branch": branch, "path": path} }
}

/// The commands pymongo's unordered `bulk_write` sends: every update (in
/// order) in one `update`, then every delete in one `delete`.
pub fn bulk_commands(coll: &str, ops: &[Op]) -> Vec<Document> {
    let mut updates = Vec::new();
    let mut deletes = Vec::new();
    for op in ops {
        match op {
            Op::Update { filter, set, upsert } => updates.push(Bson::Document(doc! {
                "q": filter.clone(), "u": {"$set": set.clone()}, "multi": false, "upsert": *upsert,
            })),
            Op::Delete { filter } => deletes.push(Bson::Document(doc! {"q": filter.clone(), "limit": 1})),
        }
    }
    let mut out = Vec::new();
    if !updates.is_empty() {
        out.push(doc! {"update": coll, "updates": updates, "ordered": false});
    }
    if !deletes.is_empty() {
        out.push(doc! {"delete": coll, "deletes": deletes, "ordered": false});
    }
    out
}

/// `bulk_write(ops, ordered=False)`; the summed `nModified`. Like pymongo,
/// every batch runs before a write error is raised.
pub async fn bulk_write(db: &Database, coll: &str, ops: &[Op]) -> Result<i64, SyncError> {
    let mut modified = 0;
    let mut errors: Vec<Bson> = Vec::new();
    for cmd in bulk_commands(coll, ops) {
        let reply = db.run_command(cmd).await.map_err(SyncError::db)?;
        modified += reply.get("nModified").and_then(int_of).unwrap_or(0);
        if let Ok(errs) = reply.get_array("writeErrors") {
            errors.extend(errs.iter().cloned());
        }
        if let Some(wce) = reply.get("writeConcernError") {
            errors.push(wce.clone());
        }
    }
    if !errors.is_empty() {
        let shown = Bson::Array(errors).into_relaxed_extjson();
        return Err(SyncError::Db(format!("batch op errors occurred, full error: {{'writeErrors': {shown}}}")));
    }
    Ok(modified)
}

fn int_of(b: &Bson) -> Option<i64> {
    match b {
        Bson::Int32(n) => Some(i64::from(*n)),
        Bson::Int64(n) => Some(*n),
        Bson::Double(f) if f.fract() == 0.0 && f.is_finite() => Some(*f as i64),
        _ => None,
    }
}

// --- Beanie models -----------------------------------------------------------------

fn bad(model: &str, field: &str) -> SyncError {
    SyncError::Db(format!("1 validation error for {model}\n{field}"))
}

fn req_str(d: &Document, model: &str, key: &str) -> Result<String, SyncError> {
    match d.get(key) {
        Some(Bson::String(s)) => Ok(s.clone()),
        _ => Err(bad(model, key)),
    }
}

fn str_or(d: &Document, model: &str, key: &str, default: &str) -> Result<String, SyncError> {
    match d.get(key) {
        None => Ok(default.to_owned()),
        Some(Bson::String(s)) => Ok(s.clone()),
        _ => Err(bad(model, key)),
    }
}

fn opt_str_of(d: &Document, model: &str, key: &str) -> Result<Option<String>, SyncError> {
    match d.get(key) {
        None | Some(Bson::Null) => Ok(None),
        Some(Bson::String(s)) => Ok(Some(s.clone())),
        _ => Err(bad(model, key)),
    }
}

fn int_or(d: &Document, model: &str, key: &str, default: Option<i64>) -> Result<i64, SyncError> {
    match d.get(key) {
        None => default.ok_or_else(|| bad(model, key)),
        Some(b) => int_of(b).ok_or_else(|| bad(model, key)),
    }
}

fn opt_int_of(d: &Document, model: &str, key: &str) -> Result<Option<i64>, SyncError> {
    match d.get(key) {
        None | Some(Bson::Null) => Ok(None),
        Some(b) => int_of(b).map(Some).ok_or_else(|| bad(model, key)),
    }
}

fn opt_date_of(d: &Document, model: &str, key: &str) -> Result<Option<BsonDateTime>, SyncError> {
    match d.get(key) {
        None | Some(Bson::Null) => Ok(None),
        Some(Bson::DateTime(t)) => Ok(Some(*t)),
        _ => Err(bad(model, key)),
    }
}

fn opt_date(d: Option<BsonDateTime>) -> Bson {
    d.map_or(Bson::Null, Bson::DateTime)
}

/// `UpdateBranch`.
#[derive(Debug, Clone, PartialEq)]
pub struct BranchDoc {
    pub id: Option<ObjectId>,
    pub branch: String,
    pub content_path: String,
    pub current_version: Option<String>,
    pub current_ordinal: i64,
    pub last_probe_at: Option<BsonDateTime>,
    pub status: String,
    pub last_error: Option<String>,
}

impl BranchDoc {
    pub fn new(branch: &str) -> Self {
        BranchDoc {
            id: None,
            branch: branch.to_owned(),
            content_path: String::new(),
            current_version: None,
            current_ordinal: 0,
            last_probe_at: None,
            status: "idle".into(),
            last_error: None,
        }
    }

    pub fn load(d: &Document) -> Result<Self, SyncError> {
        const M: &str = "UpdateBranch";
        Ok(BranchDoc {
            id: d.get_object_id("_id").ok(),
            branch: req_str(d, M, "branch")?,
            content_path: str_or(d, M, "content_path", "")?,
            current_version: opt_str_of(d, M, "current_version")?,
            current_ordinal: int_or(d, M, "current_ordinal", Some(0))?,
            last_probe_at: opt_date_of(d, M, "last_probe_at")?,
            status: str_or(d, M, "status", "idle")?,
            last_error: opt_str_of(d, M, "last_error")?,
        })
    }

    /// `get_dict(..., to_db=True)`: `_id` first when set, every field, nulls kept.
    pub fn to_doc(&self) -> Document {
        let mut d = Document::new();
        if let Some(id) = self.id {
            d.insert("_id", id);
        }
        d.insert("branch", self.branch.as_str());
        d.insert("content_path", self.content_path.as_str());
        d.insert("current_version", opt_str(self.current_version.as_deref()));
        d.insert("current_ordinal", bson_int(self.current_ordinal));
        d.insert("last_probe_at", opt_date(self.last_probe_at));
        d.insert("status", self.status.as_str());
        d.insert("last_error", opt_str(self.last_error.as_deref()));
        d
    }
}

/// `UpdateVersion`.
#[derive(Debug, Clone, PartialEq)]
pub struct VersionDoc {
    pub id: Option<ObjectId>,
    pub branch: String,
    pub ordinal: i64,
    pub version_tag: String,
    pub status: String,
    pub captured_at: BsonDateTime,
    pub completed_at: Option<BsonDateTime>,
    pub content_path: String,
    pub motd: String,
    pub files_added: i64,
    pub files_modified: i64,
    pub files_removed: i64,
    pub bytes_added: i64,
}

impl VersionDoc {
    pub fn load(d: &Document, now: DateTime<Utc>) -> Result<Self, SyncError> {
        const M: &str = "UpdateVersion";
        Ok(VersionDoc {
            id: d.get_object_id("_id").ok(),
            branch: req_str(d, M, "branch")?,
            ordinal: int_or(d, M, "ordinal", None)?,
            version_tag: req_str(d, M, "version_tag")?,
            status: str_or(d, M, "status", "in_progress")?,
            captured_at: match d.get("captured_at") {
                None => dt::to_bson(now),
                Some(Bson::DateTime(t)) => *t,
                Some(_) => return Err(bad(M, "captured_at")),
            },
            completed_at: opt_date_of(d, M, "completed_at")?,
            content_path: str_or(d, M, "content_path", "")?,
            motd: str_or(d, M, "motd", "")?,
            files_added: int_or(d, M, "files_added", Some(0))?,
            files_modified: int_or(d, M, "files_modified", Some(0))?,
            files_removed: int_or(d, M, "files_removed", Some(0))?,
            bytes_added: int_or(d, M, "bytes_added", Some(0))?,
        })
    }

    pub fn to_doc(&self) -> Document {
        let mut d = Document::new();
        if let Some(id) = self.id {
            d.insert("_id", id);
        }
        d.insert("branch", self.branch.as_str());
        d.insert("ordinal", bson_int(self.ordinal));
        d.insert("version_tag", self.version_tag.as_str());
        d.insert("status", self.status.as_str());
        d.insert("captured_at", self.captured_at);
        d.insert("completed_at", opt_date(self.completed_at));
        d.insert("content_path", self.content_path.as_str());
        d.insert("motd", self.motd.as_str());
        d.insert("files_added", bson_int(self.files_added));
        d.insert("files_modified", bson_int(self.files_modified));
        d.insert("files_removed", bson_int(self.files_removed));
        d.insert("bytes_added", bson_int(self.bytes_added));
        d
    }
}

/// The `UpdateVersion` `begin_version` inserts.
pub fn new_version(branch: &str, ordinal: i64, version_tag: &str, pointer: &Pointer, now: DateTime<Utc>) -> VersionDoc {
    VersionDoc {
        id: None,
        branch: branch.to_owned(),
        ordinal,
        version_tag: version_tag.to_owned(),
        status: "in_progress".into(),
        captured_at: dt::to_bson(now),
        completed_at: None,
        content_path: pointer.content_path.clone(),
        motd: pointer.motd.clone(),
        files_added: 0,
        files_modified: 0,
        files_removed: 0,
        bytes_added: 0,
    }
}

/// Validation of an `UpdateState` row Beanie loads; `(path, fnv_hash)`.
pub fn load_state(d: &Document) -> Result<(String, Option<i64>), SyncError> {
    const M: &str = "UpdateState";
    req_str(d, M, "branch")?;
    let path = req_str(d, M, "path")?;
    req_str(d, M, "content_sha256")?;
    let fnv = opt_int_of(d, M, "fnv_hash")?;
    int_or(d, M, "size", Some(0))?;
    opt_str_of(d, M, "archive")?;
    opt_int_of(d, M, "archive_index")?;
    int_or(d, M, "last_ordinal", Some(0))?;
    Ok((path, fnv))
}

pub fn load_sidecar(d: &Document) -> Result<(String, Sidecar), SyncError> {
    const M: &str = "UpdateManifestEntry";
    req_str(d, M, "branch")?;
    let path = req_str(d, M, "path")?;
    let sha1 = req_str(d, M, "sha1")?;
    let size = int_or(d, M, "size", None)?;
    Ok((path, Sidecar { sha1, size }))
}

/// `finish_version`'s changes to the version it closes.
pub fn complete_version(v: &mut VersionDoc, counts: &Counts, now: DateTime<Utc>) {
    v.status = "complete".into();
    v.completed_at = Some(dt::to_bson(now));
    v.files_added = counts.added;
    v.files_modified = counts.modified;
    v.files_removed = counts.removed;
    v.bytes_added = counts.bytes_added;
}

/// Which fields an `_update_branch(**fields)` call sets.
#[derive(Debug, Clone, Default)]
pub struct BranchUpdate {
    pub content_path: Option<String>,
    pub current_version: Option<String>,
    pub current_ordinal: Option<i64>,
    pub last_probe_at: Option<DateTime<Utc>>,
    pub status: Option<&'static str>,
    pub last_error: Option<String>,
}

impl BranchUpdate {
    pub fn syncing() -> Self {
        BranchUpdate { status: Some("syncing"), ..Default::default() }
    }

    pub fn finished(pointer: &Pointer, version_tag: &str, ordinal: i64, now: DateTime<Utc>) -> Self {
        BranchUpdate {
            content_path: Some(pointer.content_path.clone()),
            current_version: Some(version_tag.to_owned()),
            current_ordinal: Some(ordinal),
            last_probe_at: Some(now),
            status: Some("idle"),
            last_error: None,
        }
    }

    pub fn probed(content_path: &str, version_tag: &str, now: DateTime<Utc>) -> Self {
        BranchUpdate {
            content_path: Some(content_path.to_owned()),
            current_version: Some(version_tag.to_owned()),
            last_probe_at: Some(now),
            status: Some("idle"),
            ..Default::default()
        }
    }

    /// `mark_branch_error`: `last_error=message[:500]`.
    pub fn error(message: &str) -> Self {
        let last_error = kiwi_core::pystr::prefix(message, 500).to_owned();
        BranchUpdate { status: Some("error"), last_error: Some(last_error), ..Default::default() }
    }

    pub fn apply(&self, b: &mut BranchDoc) {
        if let Some(v) = &self.content_path {
            b.content_path = v.clone();
        }
        if let Some(v) = &self.current_version {
            b.current_version = Some(v.clone());
        }
        if let Some(v) = self.current_ordinal {
            b.current_ordinal = v;
        }
        if let Some(v) = self.last_probe_at {
            b.last_probe_at = Some(dt::to_bson(v));
        }
        if let Some(v) = self.status {
            b.status = v.to_owned();
        }
        if let Some(v) = &self.last_error {
            b.last_error = Some(v.clone());
        }
    }
}

// --- the live repo ----------------------------------------------------------------------

#[derive(Default)]
struct Buffers {
    changes: Vec<Op>,
    state: Vec<Op>,
    manifest: Vec<Op>,
}

impl Buffers {
    fn full(&self) -> bool {
        self.changes.len() >= FLUSH_THRESHOLD
            || self.state.len() >= FLUSH_THRESHOLD
            || self.manifest.len() >= FLUSH_THRESHOLD
    }
}

/// One per loop, like Python's: buffered writes a failed sync left behind go
/// out with the next flush.
pub struct MongoRepo {
    state: AppState,
    buffers: Mutex<Buffers>,
}

enum Buf {
    Changes,
    State,
    Manifest,
}

impl MongoRepo {
    pub fn new(state: AppState) -> Self {
        MongoRepo { state, buffers: Mutex::new(Buffers::default()) }
    }

    fn db(&self) -> Result<&Database, SyncError> {
        self.state.inner.db.mongo.as_ref().ok_or_else(|| SyncError::Db("MONGO_URI not configured".into()))
    }

    async fn push(&self, which: Buf, op: Op) -> Result<(), SyncError> {
        let swapped = {
            let mut b = self.buffers.lock().expect("buffers");
            match which {
                Buf::Changes => b.changes.push(op),
                Buf::State => b.state.push(op),
                Buf::Manifest => b.manifest.push(op),
            }
            b.full().then(|| std::mem::take(&mut *b))
        };
        match swapped {
            Some(b) => self.write(b).await,
            None => Ok(()),
        }
    }

    async fn flush(&self) -> Result<(), SyncError> {
        let b = std::mem::take(&mut *self.buffers.lock().expect("buffers"));
        self.write(b).await
    }

    async fn write(&self, b: Buffers) -> Result<(), SyncError> {
        let db = self.db()?;
        if !b.changes.is_empty() {
            bulk_write(db, CHANGES, &b.changes).await?;
        }
        if !b.state.is_empty() {
            bulk_write(db, STATE, &b.state).await?;
        }
        if !b.manifest.is_empty() {
            bulk_write(db, MANIFEST, &b.manifest).await?;
        }
        Ok(())
    }

    async fn find(&self, coll: &str, filter: Document, opts: Option<FindOptions>) -> Result<Vec<Document>, SyncError> {
        let c = self.db()?.collection::<Document>(coll);
        let cur = c.find(filter).with_options(opts).await.map_err(SyncError::db)?;
        cur.try_collect().await.map_err(SyncError::db)
    }

    async fn find_one(&self, coll: &str, filter: Document) -> Result<Option<Document>, SyncError> {
        self.db()?.collection::<Document>(coll).find_one(filter).await.map_err(SyncError::db)
    }

    /// Beanie `save()`: `$set` of `_id` and every field, upserting.
    async fn save(&self, coll: &str, id: ObjectId, doc: Document) -> Result<(), SyncError> {
        let c = self.db()?.collection::<Document>(coll);
        c.update_one(doc! {"_id": id}, doc! {"$set": doc}).upsert(true).await.map_err(SyncError::db)?;
        Ok(())
    }

    async fn insert(&self, coll: &str, mut doc: Document) -> Result<ObjectId, SyncError> {
        let id = ObjectId::new();
        let mut full = doc! {"_id": id};
        full.extend(std::mem::take(&mut doc));
        self.db()?.collection::<Document>(coll).insert_one(full).await.map_err(SyncError::db)?;
        Ok(id)
    }

    /// `_update_branch(branch, **fields)`.
    pub async fn update_branch(&self, branch: &str, fields: BranchUpdate) -> Result<(), SyncError> {
        match self.find_one(BRANCHES, doc! {"branch": branch}).await? {
            None => {
                let mut b = BranchDoc::new(branch);
                fields.apply(&mut b);
                self.insert(BRANCHES, b.to_doc()).await?;
            }
            Some(d) => {
                let mut b = BranchDoc::load(&d)?;
                fields.apply(&mut b);
                let id = b.id.ok_or_else(|| bad("UpdateBranch", "_id"))?;
                self.save(BRANCHES, id, b.to_doc()).await?;
            }
        }
        Ok(())
    }

    pub async fn mark_branch_error(&self, branch: &str, message: &str) -> Result<(), SyncError> {
        self.update_branch(branch, BranchUpdate::error(message)).await
    }
}

impl Repo for MongoRepo {
    async fn get_manifest_sidecar(&self, branch: &str) -> Result<Ordered<Sidecar>, SyncError> {
        let docs = self.find(MANIFEST, doc! {"branch": branch}, None).await?;
        docs.iter().map(load_sidecar).collect()
    }

    async fn state_exists(&self, branch: &str, path: &str) -> Result<bool, SyncError> {
        match self.find_one(STATE, doc! {"branch": branch, "path": path}).await? {
            Some(d) => load_state(&d).map(|_| true),
            None => Ok(false),
        }
    }

    async fn state_sha(&self, branch: &str, path: &str) -> Result<Option<String>, SyncError> {
        match self.find_one(STATE, doc! {"branch": branch, "path": path}).await? {
            Some(d) => load_state(&d).map(|_| d.get_str("content_sha256").ok().map(str::to_owned)),
            None => Ok(None),
        }
    }

    async fn get_archive_state(&self, branch: &str, directory: &str) -> Result<Ordered<Option<i64>>, SyncError> {
        let docs = self.find(STATE, doc! {"branch": branch, "archive": directory}, None).await?;
        docs.iter().map(load_state).collect()
    }

    async fn begin_version(
        &self,
        branch: &str,
        version_tag: &str,
        pointer: &Pointer,
    ) -> Result<(i64, bool), SyncError> {
        if let Some(d) = self.find_one(VERSIONS, doc! {"branch": branch, "status": "in_progress"}).await? {
            return Ok((VersionDoc::load(&d, dt::now())?.ordinal, true));
        }
        let opts = FindOptions::builder().sort(doc! {"ordinal": -1}).limit(1).build();
        let last = self.find(VERSIONS, doc! {"branch": branch}, Some(opts)).await?;
        let ordinal = match last.first() {
            Some(d) => VersionDoc::load(d, dt::now())?.ordinal + 1,
            None => 1,
        };
        let v = new_version(branch, ordinal, version_tag, pointer, dt::now());
        self.insert(VERSIONS, v.to_doc()).await?;
        self.update_branch(branch, BranchUpdate::syncing()).await?;
        Ok((ordinal, false))
    }

    async fn record_change(
        &self,
        branch: &str,
        ordinal: i64,
        path: &str,
        change_type: &str,
        content_sha256: Option<&str>,
        fnv_hash: Option<i64>,
        size: i64,
    ) -> Result<(), SyncError> {
        self.push(Buf::Changes, change_op(branch, ordinal, path, change_type, content_sha256, fnv_hash, size)).await
    }

    async fn upsert_state(
        &self,
        branch: &str,
        path: &str,
        content_sha256: &str,
        fnv_hash: Option<i64>,
        size: i64,
        archive: Option<&str>,
        archive_index: Option<i64>,
        last_ordinal: i64,
    ) -> Result<(), SyncError> {
        let op = state_op(branch, path, content_sha256, fnv_hash, size, archive, archive_index, last_ordinal);
        self.push(Buf::State, op).await
    }

    async fn remove_state(&self, branch: &str, path: &str) -> Result<(), SyncError> {
        self.push(Buf::State, delete_op(branch, path)).await
    }

    async fn set_manifest_entry(&self, branch: &str, path: &str, sha1: &str, size: i64) -> Result<(), SyncError> {
        self.push(Buf::Manifest, manifest_op(branch, path, sha1, size)).await
    }

    async fn remove_manifest_entry(&self, branch: &str, path: &str) -> Result<(), SyncError> {
        self.push(Buf::Manifest, delete_op(branch, path)).await
    }

    async fn finish_version(
        &self,
        branch: &str,
        ordinal: i64,
        version_tag: &str,
        pointer: &Pointer,
        counts: &Counts,
    ) -> Result<(), SyncError> {
        self.flush().await?;
        if let Some(d) = self.find_one(VERSIONS, doc! {"branch": branch, "ordinal": bson_int(ordinal)}).await? {
            let mut v = VersionDoc::load(&d, dt::now())?;
            complete_version(&mut v, counts, dt::now());
            let id = v.id.ok_or_else(|| bad("UpdateVersion", "_id"))?;
            self.save(VERSIONS, id, v.to_doc()).await?;
        }
        self.update_branch(branch, BranchUpdate::finished(pointer, version_tag, ordinal, dt::now())).await?;
        if let Err(e) = crate::events::bus::publish_type(&self.state, "game_update").await {
            tracing::warn!("game_update event publish failed: {e:#}");
        }
        Ok(())
    }

    async fn touch_probe(&self, branch: &str, content_path: &str, version_tag: &str) -> Result<(), SyncError> {
        self.update_branch(branch, BranchUpdate::probed(content_path, version_tag, dt::now())).await
    }
}

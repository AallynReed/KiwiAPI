//! `app/trove/updates/read.py` over the store module's [`Fetch`], issuing the
//! same Mongo calls in the same order (Beanie finds with the model projection,
//! raw pymongo finds with their own), so the golden cases can replay them.
//!
//! A document the Beanie model rejects or a store error is an unhandled
//! exception in Python: it panics into the request-id boundary here.

use std::collections::{BTreeSet, HashMap};

use chrono::{DateTime, Utc};
use kiwi_core::updates::tree::{self, Child, Row};
use mongodb::bson::{Bson, Document, doc};
use serde_json::Value;

use crate::{
    auth::pyutil::bson_int,
    dt,
    routes::store::service::{Fetch, Find, projection, re_escape},
};

pub const BRANCHES: &str = "update_branches";
pub const VERSIONS: &str = "update_versions";
pub const CHANGES: &str = "update_changes";
pub const STATE: &str = "update_state";

/// `updates.cdn.BRANCHES`, in order.
pub const KNOWN: [&str; 2] = ["live-us", "pts"];

const BRANCH_FIELDS: &[&str] =
    &["branch", "content_path", "current_version", "current_ordinal", "last_probe_at", "status", "last_error"];
const VERSION_FIELDS: &[&str] = &[
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
    "bytes_added",
    "from_live_added",
    "from_live_modified",
    "from_live_removed",
];
const CHANGE_FIELDS: &[&str] =
    &["branch", "ordinal", "path", "type", "content_sha256", "fnv_hash", "size", "from_live"];
const STATE_FIELDS: &[&str] =
    &["branch", "path", "content_sha256", "fnv_hash", "size", "archive", "archive_index", "last_ordinal"];

pub fn unhandled(e: impl std::fmt::Display) -> ! {
    panic!("updates: {e}")
}

fn bad(key: &str, v: &Bson) -> ! {
    unhandled(format!("document failed validation: {key}: unexpected {v:?}"))
}

fn int_of(key: &str, v: &Bson) -> i64 {
    match v {
        Bson::Int32(i) => i64::from(*i),
        Bson::Int64(i) => *i,
        Bson::Boolean(b) => i64::from(*b),
        Bson::Double(f) if f.is_finite() && f.fract() == 0.0 && f.abs() < 9.2e18 => *f as i64,
        _ => bad(key, v),
    }
}

/// `x: int = default` (`None` default: required).
fn int(d: &Document, key: &str, default: Option<i64>) -> i64 {
    match (d.get(key), default) {
        (Some(v), _) => int_of(key, v),
        (None, Some(def)) => def,
        (None, None) => unhandled(format!("document failed validation: {key}: missing")),
    }
}

fn opt_int(d: &Document, key: &str) -> Option<i64> {
    match d.get(key) {
        None | Some(Bson::Null) => None,
        Some(v) => Some(int_of(key, v)),
    }
}

fn string(d: &Document, key: &str, default: Option<&str>) -> String {
    match (d.get(key), default) {
        (Some(Bson::String(s)), _) => s.clone(),
        (None, Some(def)) => def.to_owned(),
        (None, None) => unhandled(format!("document failed validation: {key}: missing")),
        (Some(v), _) => bad(key, v),
    }
}

fn opt_string(d: &Document, key: &str) -> Option<String> {
    match d.get(key) {
        None | Some(Bson::Null) => None,
        Some(Bson::String(s)) => Some(s.clone()),
        Some(v) => bad(key, v),
    }
}

fn opt_dt(d: &Document, key: &str) -> Option<DateTime<Utc>> {
    match d.get(key) {
        None | Some(Bson::Null) => None,
        Some(Bson::DateTime(t)) => Some(dt::from_bson(*t)),
        Some(v) => bad(key, v),
    }
}

/// A raw pymongo value as `json.dumps` would write it (scalars only).
pub fn raw_json(v: &Bson) -> Value {
    match v {
        Bson::Null => Value::Null,
        Bson::Int32(i) => Value::from(*i),
        Bson::Int64(i) => Value::from(*i),
        Bson::Double(f) if f.is_finite() => Value::from(*f),
        Bson::Boolean(b) => Value::Bool(*b),
        Bson::String(s) => Value::String(s.clone()),
        other => unhandled(format!("TypeError: Object of type {other:?} is not JSON serializable")),
    }
}

fn find(filter: Document, projection: Document, sort: Vec<(&'static str, i32)>, skip: u64, limit: i64) -> Find {
    Find { filter, projection, sort, skip, limit }
}

/// `UpdateBranch` plus the `file_count` `list_branches` adds.
#[derive(Clone, Debug, PartialEq)]
pub struct Branch {
    pub branch: String,
    pub current_version: Option<String>,
    pub current_ordinal: i64,
    pub last_probe_at: Option<DateTime<Utc>>,
    pub status: String,
    pub file_count: u64,
}

/// `UpdateVersion`, the fields the routes read.
#[derive(Clone, Debug, PartialEq)]
pub struct Version {
    pub branch: String,
    pub ordinal: i64,
    pub version_tag: String,
    pub captured_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub files_added: i64,
    pub files_modified: i64,
    pub files_removed: i64,
    pub bytes_added: i64,
    /// Added/modified/removed rows that only repeat Live; None until tagged.
    pub from_live: Option<[i64; 3]>,
}

fn version(d: &Document) -> Version {
    let captured_at = match d.get("captured_at") {
        None => dt::now(),
        Some(Bson::DateTime(t)) => dt::from_bson(*t),
        Some(v) => bad("captured_at", v),
    };
    Version {
        branch: string(d, "branch", None),
        ordinal: int(d, "ordinal", None),
        version_tag: string(d, "version_tag", None),
        captured_at,
        completed_at: opt_dt(d, "completed_at"),
        files_added: int(d, "files_added", Some(0)),
        files_modified: int(d, "files_modified", Some(0)),
        files_removed: int(d, "files_removed", Some(0)),
        bytes_added: int(d, "bytes_added", Some(0)),
        from_live: d
            .contains_key("from_live_added")
            .then(|| ["from_live_added", "from_live_modified", "from_live_removed"].map(|k| int(d, k, Some(0)))),
    }
}

/// `UpdateChange`.
#[derive(Clone, Debug, PartialEq)]
pub struct Change {
    pub ordinal: i64,
    pub path: String,
    pub kind: String,
    pub content_sha256: Option<String>,
    pub size: i64,
    pub from_live: bool,
}

fn change(d: &Document) -> Change {
    let _ = string(d, "branch", None);
    Change {
        ordinal: int(d, "ordinal", None),
        path: string(d, "path", None),
        kind: string(d, "type", None),
        content_sha256: opt_string(d, "content_sha256"),
        size: int(d, "size", Some(0)),
        from_live: d.get_bool("from_live").unwrap_or(false),
    }
}

/// `get_file_meta`'s dict.
#[derive(Clone, Debug, PartialEq)]
pub struct FileMeta {
    pub path: String,
    pub content_sha256: String,
    pub size: i64,
    pub archive: Option<String>,
    pub archive_index: Option<i64>,
    pub removed: bool,
    pub removed_ordinal: Option<i64>,
}

pub async fn list_branches<F: Fetch>(f: &F) -> Vec<Branch> {
    let docs = f.find(BRANCHES, find(doc! {}, projection(BRANCH_FIELDS), vec![("branch", 1)], 0, 0)).await;
    let mut out = Vec::with_capacity(docs.len());
    for d in &docs {
        let branch = string(d, "branch", None);
        let current_version = opt_string(d, "current_version");
        let current_ordinal = int(d, "current_ordinal", Some(0));
        let last_probe_at = opt_dt(d, "last_probe_at");
        let status = string(d, "status", Some("idle"));
        let file_count = f.count(STATE, doc! {"branch": branch.as_str()}).await;
        out.push(Branch { branch, current_version, current_ordinal, last_probe_at, status, file_count });
    }
    out
}

/// `list_versions` (complete versions, newest first) and the total.
pub async fn list_versions<F: Fetch>(f: &F, branch: &str, limit: i64, offset: i64) -> (Vec<Version>, u64) {
    let filter = doc! {"branch": branch, "status": "complete"};
    let total = f.count(VERSIONS, filter.clone()).await;
    let docs =
        f.find(VERSIONS, find(filter, projection(VERSION_FIELDS), vec![("ordinal", -1)], offset as u64, limit)).await;
    (docs.iter().map(version).collect(), total)
}

/// `resolve_version`: by tag, by ordinal, else the latest complete one.
pub async fn resolve_version<F: Fetch>(
    f: &F,
    branch: &str,
    ordinal: Option<i64>,
    version_tag: Option<&str>,
) -> Option<Version> {
    if let Some(tag) = version_tag.filter(|t| !t.is_empty()) {
        return f
            .find_one(VERSIONS, doc! {"branch": branch, "version_tag": tag}, projection(VERSION_FIELDS))
            .await
            .map(|d| version(&d));
    }
    if let Some(o) = ordinal {
        return f
            .find_one(VERSIONS, doc! {"branch": branch, "ordinal": bson_int(o)}, projection(VERSION_FIELDS))
            .await
            .map(|d| version(&d));
    }
    let filter = doc! {"branch": branch, "status": "complete"};
    f.find(VERSIONS, find(filter, projection(VERSION_FIELDS), vec![("ordinal", -1)], 0, 1)).await.first().map(version)
}

/// `list_changes`: one version's changes by type then path, and the total.
pub async fn list_changes<F: Fetch>(
    f: &F,
    branch: &str,
    ordinal: i64,
    kind: Option<&str>,
    with_live: bool,
    limit: i64,
    offset: i64,
) -> (Vec<Change>, u64) {
    let mut q = doc! {"branch": branch, "ordinal": bson_int(ordinal)};
    if let Some(k) = kind.filter(|k| !k.is_empty()) {
        q.insert("type", k);
    }
    if !with_live {
        q.insert("from_live", doc! {"$ne": true});
    }
    let total = f.count(CHANGES, q.clone()).await;
    let sort = vec![("type", 1), ("path", 1)];
    let docs = f.find(CHANGES, find(q, projection(CHANGE_FIELDS), sort, offset as u64, limit)).await;
    (docs.iter().map(change).collect(), total)
}

/// `_captured_at_by_ordinal`.
async fn captured_at_by_ordinal<F: Fetch>(f: &F, branch: &str) -> HashMap<i64, Option<DateTime<Utc>>> {
    let proj = doc! {"ordinal": 1, "captured_at": 1, "_id": 0};
    let docs = f.find(VERSIONS, find(doc! {"branch": branch}, proj, Vec::new(), 0, 0)).await;
    docs.iter()
        .map(|d| {
            let o = d.get("ordinal").unwrap_or_else(|| unhandled("KeyError: 'ordinal'"));
            (int_of("ordinal", o), opt_dt(d, "captured_at"))
        })
        .collect()
}

/// `e.get("last_ordinal", 0) or 0`.
fn last_ordinal_or_zero(d: &Document) -> i64 {
    match d.get("last_ordinal") {
        None | Some(Bson::Null) => 0,
        Some(v) => int_of("last_ordinal", v),
    }
}

/// One `list_directory` entry with its `last_modified_at`.
pub struct TreeEntry {
    pub child: Child,
    pub last_modified_at: Option<DateTime<Utc>>,
}

pub async fn list_directory<F: Fetch>(f: &F, branch: &str, prefix: &str) -> Vec<TreeEntry> {
    let mut query = doc! {"branch": branch};
    if !prefix.is_empty() {
        query.insert("path", doc! {"$gte": prefix, "$lt": format!("{prefix}\u{ffff}")});
    }
    let proj = doc! {"path": 1, "size": 1, "last_ordinal": 1, "_id": 0};
    let docs = f.find(STATE, find(query, proj, Vec::new(), 0, 0)).await;
    let rows: Vec<Row> = docs
        .iter()
        .map(|d| Row {
            path: match d.get("path") {
                Some(Bson::String(s)) => s.clone(),
                Some(v) => bad("path", v),
                None => unhandled("KeyError: 'path'"),
            },
            size: d.get("size").map_or(0, |v| int_of("size", v)),
            last_ordinal: last_ordinal_or_zero(d),
        })
        .collect();
    let listing = tree::directory_listing(&rows, prefix);
    let captured = captured_at_by_ordinal(f, branch).await;
    listing
        .into_iter()
        .map(|child| {
            let last_modified_at = captured.get(&child.last_ordinal).copied().flatten();
            TreeEntry { child, last_modified_at }
        })
        .collect()
}

/// One `search_paths` hit: raw `size` / `last_ordinal` as pymongo returned them.
pub struct SearchHit {
    pub path: String,
    pub size: Value,
    pub last_ordinal: Value,
    pub last_modified_at: Option<DateTime<Utc>>,
}

pub async fn search_paths<F: Fetch>(f: &F, branch: &str, needle: &str, limit: i64) -> (Vec<SearchHit>, u64) {
    let needle = kiwi_core::pystr::strip(needle);
    if needle.is_empty() {
        return (Vec::new(), 0);
    }
    let query = doc! {"branch": branch, "path": {"$regex": re_escape(needle), "$options": "i"}};
    let total = f.count(STATE, query.clone()).await;
    let proj = doc! {"path": 1, "size": 1, "last_ordinal": 1, "_id": 0};
    let docs = f.find(STATE, find(query, proj, vec![("path", 1)], 0, limit)).await;
    let mut hits: Vec<(SearchHit, i64)> = docs
        .iter()
        .map(|d| {
            let path = match d.get("path") {
                Some(Bson::String(s)) => s.clone(),
                Some(v) => bad("path", v),
                None => unhandled("KeyError: 'path'"),
            };
            let size = d.get("size").map_or(Value::from(0), raw_json);
            let last_ordinal = d.get("last_ordinal").map_or(Value::from(0), raw_json);
            let hit = SearchHit { path, size, last_ordinal, last_modified_at: None };
            (hit, last_ordinal_or_zero(d))
        })
        .collect();
    let captured = captured_at_by_ordinal(f, branch).await;
    for (hit, o) in &mut hits {
        hit.last_modified_at = captured.get(o).copied().flatten();
    }
    (hits.into_iter().map(|(h, _)| h).collect(), total)
}

/// `last_known_meta`: the last version of a path that has left the tree.
async fn last_known_meta<F: Fetch>(f: &F, branch: &str, path: &str) -> Option<FileMeta> {
    let filter = doc! {"branch": branch, "path": path};
    let last = f.find(CHANGES, find(filter, projection(CHANGE_FIELDS), vec![("ordinal", -1)], 0, 1)).await;
    let last = change(last.first()?);
    if last.kind != "removed" {
        return None;
    }
    let filter = doc! {
        "branch": branch, "path": path,
        "ordinal": {"$lt": bson_int(last.ordinal)}, "content_sha256": {"$ne": Bson::Null},
    };
    let prev = f.find(CHANGES, find(filter, projection(CHANGE_FIELDS), vec![("ordinal", -1)], 0, 1)).await;
    let prev = change(prev.first()?);
    Some(FileMeta {
        path: path.to_owned(),
        content_sha256: prev.content_sha256.unwrap_or_else(|| unhandled("content_sha256 is None")),
        size: prev.size,
        archive: None,
        archive_index: None,
        removed: true,
        removed_ordinal: Some(last.ordinal),
    })
}

/// `get_file_meta`: the current tree, else the last version before removal.
pub async fn get_file_meta<F: Fetch>(f: &F, branch: &str, path: &str) -> Option<FileMeta> {
    match f.find_one(STATE, doc! {"branch": branch, "path": path}, projection(STATE_FIELDS)).await {
        Some(d) => {
            let _ = string(&d, "branch", None);
            let _ = opt_int(&d, "fnv_hash");
            let _ = int(&d, "last_ordinal", Some(0));
            Some(FileMeta {
                path: string(&d, "path", None),
                content_sha256: string(&d, "content_sha256", None),
                size: int(&d, "size", Some(0)),
                archive: opt_string(&d, "archive"),
                archive_index: opt_int(&d, "archive_index"),
                removed: false,
                removed_ordinal: None,
            })
        }
        None => last_known_meta(f, branch, path).await,
    }
}

/// One `file_history` row.
pub struct HistoryRow {
    pub ordinal: i64,
    pub version_tag: String,
    pub captured_at: Option<DateTime<Utc>>,
    pub kind: String,
    pub content_sha256: Option<String>,
    pub size: i64,
}

pub async fn file_history<F: Fetch>(f: &F, branch: &str, path: &str) -> Vec<HistoryRow> {
    let filter = doc! {"branch": branch, "path": path};
    let docs = f.find(CHANGES, find(filter, projection(CHANGE_FIELDS), vec![("ordinal", -1)], 0, 0)).await;
    if docs.is_empty() {
        return Vec::new();
    }
    let changes: Vec<Change> = docs.iter().map(change).collect();
    let ordinals: BTreeSet<i64> = changes.iter().map(|c| c.ordinal).collect();
    let ins: Vec<Bson> = ordinals.iter().map(|&o| bson_int(o)).collect();
    let filter = doc! {"branch": branch, "ordinal": {"$in": ins}};
    let versions = f.find(VERSIONS, find(filter, projection(VERSION_FIELDS), Vec::new(), 0, 0)).await;
    let by_ord: HashMap<i64, Version> = versions.iter().map(version).map(|v| (v.ordinal, v)).collect();
    changes
        .into_iter()
        .map(|c| {
            let v = by_ord.get(&c.ordinal);
            HistoryRow {
                ordinal: c.ordinal,
                version_tag: v.map(|v| v.version_tag.clone()).unwrap_or_default(),
                captured_at: v.map(|v| v.captured_at),
                kind: c.kind,
                content_sha256: c.content_sha256,
                size: c.size,
            }
        })
        .collect()
}

/// `resolve_file_at_version`: `(content_sha256, size)` of the latest
/// non-removed change at or before `ordinal`.
pub async fn resolve_file_at_version<F: Fetch>(
    f: &F,
    branch: &str,
    path: &str,
    ordinal: i64,
) -> Option<(Option<String>, i64)> {
    let filter = doc! {"branch": branch, "path": path, "ordinal": {"$lte": bson_int(ordinal)}};
    let docs = f.find(CHANGES, find(filter, projection(CHANGE_FIELDS), vec![("ordinal", -1)], 0, 1)).await;
    let last = change(docs.first()?);
    (last.kind != "removed").then_some((last.content_sha256, last.size))
}

/// `read_file_text`'s classification: `kind` and, for `text`, the content.
pub struct View {
    pub meta: FileMeta,
    pub kind: &'static str,
    pub text: Option<String>,
}

pub async fn read_file_text<F: Fetch>(f: &F, branch: &str, path: &str) -> Option<View> {
    let meta = get_file_meta(f, branch, path).await?;
    if let Some(kind) = tree::view_kind(path, meta.size) {
        return Some(View { meta, kind, text: None });
    }
    let Some(data) = f.blob(&meta.content_sha256).await else {
        return Some(View { meta, kind: "missing", text: None });
    };
    match tree::view_text(&data) {
        Some(text) => Some(View { text: Some(text.to_owned()), meta, kind: "text" }),
        None => Some(View { meta, kind: "binary", text: None }),
    }
}

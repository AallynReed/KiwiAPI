//! The branch / commit / tree / compare / placement half of
//! `mods_hub/service.py`, with its DTOs and messages. Each read and write has
//! a blocking `*_core` over the repository folder; the async wrappers load
//! nothing but the game map and save nothing but the project.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use axum::http::StatusCode;
use futures::TryStreamExt;
use kiwi_core::pystr;
use mongodb::{
    bson::{Bson, Document, doc},
    options::{FindOptions, UpdateOptions},
};
use serde_json::{Value, json};

use super::{
    odb::{self, GitError},
    project::{self, COLLECTION, Project, STRAY_HANDLE},
    store::{self, Meta, StoreError},
};
use crate::{
    auth::models::{SITE_USERS, SiteUser},
    dt,
    error::{ApiError, ErrorCode},
    state::AppState,
};

pub const BLOCKED_COMMIT_MESSAGE: &str = "A built .tmod goes in a release, not in your files - use New release to upload it so people can download and install it.";
pub const LIVE_BRANCH: &str = "live-us";

pub fn internal(e: impl std::fmt::Display) -> ApiError {
    tracing::error!("mods git: {e}");
    ApiError::internal()
}

fn not_found(msg: impl Into<String>) -> ApiError {
    ApiError::not_found(msg)
}

fn mongo(state: &AppState) -> Result<&mongodb::Database, ApiError> {
    state.inner.db.mongo.as_ref().ok_or_else(|| internal("MONGO_URI not configured"))
}

/// A blocking store call off the runtime; any store error is Python's 500.
pub async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, GitError> + Send + 'static,
) -> Result<T, ApiError> {
    tokio::task::spawn_blocking(f).await.map_err(internal)?.map_err(internal)
}

async fn run<T: Send + 'static>(f: impl FnOnce() -> Result<T, ApiError> + Send + 'static) -> Result<T, ApiError> {
    tokio::task::spawn_blocking(f).await.map_err(internal)?
}

pub fn repo_dir(state: &AppState, p: &Project) -> PathBuf {
    store::repo_dir(&state.settings().mods_store_dir, &p.id.to_hex())
}

fn master(state: &AppState) -> &str {
    &state.settings().site_master_handle
}

/// `get_project(handle, slug)`.
pub async fn get_project(state: &AppState, handle: &str, slug: &str) -> Result<Option<Project>, ApiError> {
    let db = mongo(state)?;
    let handle = pystr::strip(handle).to_lowercase();
    let coll = db.collection::<Document>(COLLECTION);
    let raw = if handle == STRAY_HANDLE {
        coll.find_one(doc! {"is_stray": true, "slug": slug}).await.map_err(internal)?
    } else {
        let user = db
            .collection::<Document>(SITE_USERS)
            .find_one(doc! {"username": &handle})
            .projection(doc! {"_id": 1})
            .await
            .map_err(internal)?;
        let Some(oid) = user.and_then(|u| u.get_object_id("_id").ok()) else { return Ok(None) };
        coll.find_one(doc! {"owner_id": oid, "slug": slug}).await.map_err(internal)?
    };
    raw.map(|d| Project::from_doc(&d).map_err(|e| internal(e.0))).transpose()
}

/// `get_for_view`: 404, or `not_public` for a draft the viewer can't see.
pub async fn get_for_view(
    state: &AppState,
    handle: &str,
    slug: &str,
    viewer: Option<&SiteUser>,
) -> Result<Project, ApiError> {
    let Some(p) = get_project(state, handle, slug).await? else { return Err(not_found("Mod project not found")) };
    check_view(&p, viewer, master(state))?;
    Ok(p)
}

pub fn check_view(p: &Project, viewer: Option<&SiteUser>, master: &str) -> Result<(), ApiError> {
    if project::can_view(p, viewer, master, dt::now()) {
        return Ok(());
    }
    if p.str("visibility") == "draft" && !p.bool("taken_down") {
        return Err(ApiError::new(StatusCode::NOT_FOUND, ErrorCode::NotPublic, "This mod isn't public yet."));
    }
    Err(not_found("Mod project not found"))
}

pub fn source_visible(state: &AppState, p: &Project, viewer: Option<&SiteUser>) -> bool {
    project::source_visible(p, viewer, master(state), dt::now())
}

pub fn ensure_source_visible(state: &AppState, p: &Project, viewer: Option<&SiteUser>) -> Result<(), ApiError> {
    if source_visible(state, p, viewer) { Ok(()) } else { Err(not_found("Mod project not found")) }
}

/// The router's `_require_owned`: an editable project or a uniform 404.
pub async fn require_owned(state: &AppState, handle: &str, slug: &str, user: &SiteUser) -> Result<Project, ApiError> {
    match get_project(state, handle, slug).await? {
        Some(p) if project::can_edit(&p, Some(user)) => Ok(p),
        _ => Err(not_found("Mod project not found")),
    }
}

/// `project.updated_at = utcnow(); await project.save()`.
pub async fn touch(state: &AppState, p: &mut Project) -> Result<(), ApiError> {
    p.set_updated_now();
    mongo(state)?
        .collection::<Document>(COLLECTION)
        .update_one(doc! {"_id": p.id}, doc! {"$set": p.save_set()})
        .with_options(UpdateOptions::builder().upsert(true).build())
        .await
        .map_err(internal)?;
    Ok(())
}

fn require_owner(p: &Project, actor: &SiteUser) -> Result<(), ApiError> {
    if project::can_edit(p, Some(actor)) {
        Ok(())
    } else {
        Err(ApiError::forbidden("You don't have edit access to this mod project."))
    }
}

fn require_files_mode(p: &Project) -> Result<(), ApiError> {
    if p.str("mode") == "files" {
        Ok(())
    } else {
        Err(ApiError::bad_request("This mod is in releases-only mode - it has no file versioning."))
    }
}

/// `_norm_path`.
pub fn norm_path(raw: &str) -> Result<String, ApiError> {
    let p = pystr::strip(raw.replace('\\', "/").trim_start_matches('/').to_lowercase().as_str()).to_owned();
    let invalid = || ApiError::bad_request(format!("Invalid file path: {}", pystr::repr(raw)));
    if p.is_empty() || p.ends_with('/') {
        return Err(invalid());
    }
    if p.split('/').any(|s| matches!(s, "" | "." | "..")) {
        return Err(invalid());
    }
    if p.len() > 255 {
        return Err(ApiError::bad_request(format!("Path too long (>255 bytes): {}", pystr::repr(raw))));
    }
    Ok(p)
}

/// `_valid_branch_name`.
pub fn valid_branch_name(name: &str) -> Result<String, ApiError> {
    let name = pystr::strip(name);
    let ok = (1..=80).contains(&name.chars().count())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | '-'))
        && !name.starts_with('/')
        && !name.contains("..");
    if ok { Ok(name.to_owned()) } else { Err(ApiError::bad_request("Invalid branch name.")) }
}

/// `_safe_filename`.
pub fn safe_filename(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .filter(|&c| !matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' | '\0'..='\x1f'))
        .collect();
    let cleaned = pystr::strip(&cleaned).trim_matches('.');
    let cut: String = cleaned.chars().take(120).collect();
    if cut.is_empty() { "mod".into() } else { cut }
}

/// `datetime.fromtimestamp(t, timezone.utc).isoformat()`.
fn iso_ts(t: i64) -> Result<String, ApiError> {
    chrono::DateTime::from_timestamp(t, 0)
        .filter(|d| chrono::Datelike::year(d) <= 9999)
        .map(|d| dt::isoformat(&d))
        .ok_or_else(|| internal("commit time out of range"))
}

/// `_commit_dto`.
pub fn commit_dto(m: &Meta, branch: Option<&str>) -> Result<Value, ApiError> {
    Ok(json!({
        "id": m.sha,
        "short": m.sha.chars().take(7).collect::<String>(),
        "branch": branch,
        "parents": m.parents,
        "author_username": m.author,
        "message": m.message,
        "file_count": m.file_count,
        "created_at": iso_ts(m.time)?,
    }))
}

fn entries_json(entries: &[store::TreeEntryDto]) -> Value {
    Value::Array(entries.iter().map(|e| json!({"path": e.path, "blob_sha": e.blob_sha, "size": e.size})).collect())
}

pub fn actor_name(actor: &SiteUser) -> &str {
    match actor.display_name.as_deref() {
        Some(d) if !d.is_empty() => d,
        _ => &actor.username,
    }
}

// --- branches ---------------------------------------------------------------

pub async fn list_branches(state: &AppState, p: &Project) -> Result<Value, ApiError> {
    let (dir, default) = (repo_dir(state, p), p.default_branch().to_owned());
    run(move || list_branches_core(&dir, &default)).await
}

/// `list_branches` / `_branches_or_default`.
pub fn list_branches_core(dir: &Path, default_branch: &str) -> Result<Value, ApiError> {
    let branches = store::list_branches(dir).map_err(internal)?;
    if branches.is_empty() {
        return Ok(json!([{"name": default_branch, "head_commit_id": null}]));
    }
    Ok(Value::Array(branches.iter().map(|(n, h)| json!({"name": n, "head_commit_id": h})).collect()))
}

pub async fn create_branch(
    state: &AppState,
    p: &Project,
    actor: &SiteUser,
    name: &str,
    from_ref: Option<&str>,
) -> Result<Value, ApiError> {
    let (dir, p, actor, name, from_ref) =
        (repo_dir(state, p), p.clone(), actor.clone(), name.to_owned(), from_ref.map(str::to_owned));
    run(move || create_branch_core(&dir, &p, &actor, &name, from_ref.as_deref())).await
}

pub fn create_branch_core(
    dir: &Path,
    p: &Project,
    actor: &SiteUser,
    name: &str,
    from_ref: Option<&str>,
) -> Result<Value, ApiError> {
    require_owner(p, actor)?;
    require_files_mode(p)?;
    let name = valid_branch_name(name)?;
    let existing = store::list_branches(dir).map_err(internal)?;
    if existing.iter().any(|(n, _)| *n == name) {
        return Err(ApiError::conflict(format!("Branch '{name}' already exists.")));
    }
    let start = match from_ref {
        Some(r) if !r.is_empty() => r,
        _ => p.default_branch(),
    };
    match store::create_branch(dir, &name, start) {
        Ok(head) => Ok(json!({"name": name, "head_commit_id": head})),
        Err(StoreError::Refused(msg)) => Err(ApiError::bad_request(msg)),
        Err(e) => Err(internal(format!("{e:?}"))),
    }
}

pub async fn delete_branch(state: &AppState, p: &Project, actor: &SiteUser, name: &str) -> Result<(), ApiError> {
    let (dir, p, actor, name) = (repo_dir(state, p), p.clone(), actor.clone(), name.to_owned());
    run(move || delete_branch_core(&dir, &p, &actor, &name)).await
}

pub fn delete_branch_core(dir: &Path, p: &Project, actor: &SiteUser, name: &str) -> Result<(), ApiError> {
    require_owner(p, actor)?;
    if name == p.default_branch() {
        return Err(ApiError::bad_request("Can't delete the default branch."));
    }
    store::delete_branch(dir, name).map_err(internal)
}

// --- commits ------------------------------------------------------------------

fn resolve_hex(dir: &Path, r: &str) -> Result<Option<String>, ApiError> {
    let Some(repo) = odb::Repo::open(dir) else { return Ok(None) };
    Ok(store::resolve(&repo, r).map_err(internal)?.map(|o| odb::to_hex(&o)))
}

/// A validated web commit (every check `commit_files` makes before writing).
pub struct CommitRequest {
    pub branch: String,
    pub message: String,
    pub adds: Vec<(String, Vec<u8>)>,
    pub deletes: Vec<String>,
}

#[allow(clippy::too_many_arguments)]
pub fn check_commit(
    p: &Project,
    actor: &SiteUser,
    branch: &str,
    message: &str,
    adds: Vec<(String, Vec<u8>)>,
    deletes: &[String],
    max_files: i64,
    max_bytes: i64,
) -> Result<CommitRequest, ApiError> {
    require_owner(p, actor)?;
    require_files_mode(p)?;
    if pystr::strip(message).is_empty() {
        return Err(ApiError::bad_request("A commit needs a message."));
    }
    if adds.len() as i64 > max_files {
        return Err(ApiError::bad_request(format!("Too many files in one commit (max {max_files}).")));
    }
    let branch = valid_branch_name(branch)?;
    let mut norm = Vec::with_capacity(adds.len());
    for (raw, content) in adds {
        if content.len() as i64 > max_bytes {
            return Err(ApiError::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                ErrorCode::BadRequest,
                format!("File {} exceeds the {max_bytes}-byte limit.", pystr::repr(&raw)),
            ));
        }
        let path = norm_path(&raw)?;
        if path.ends_with(".tmod") {
            return Err(ApiError::bad_request(BLOCKED_COMMIT_MESSAGE));
        }
        norm.push((path, content));
    }
    let deletes = deletes.iter().map(|d| norm_path(d)).collect::<Result<Vec<_>, _>>()?;
    Ok(CommitRequest { branch, message: pystr::strip(message).to_owned(), adds: norm, deletes })
}

/// The write and DTO of `commit_files` (the project save comes after).
pub fn commit_core(dir: &Path, req: &CommitRequest, author: &[u8], ts: i64) -> Result<Value, ApiError> {
    let sha = match store::write_commit(dir, &req.branch, &req.adds, &req.deletes, author, &req.message, ts) {
        Ok(sha) => sha,
        Err(StoreError::NothingToCommit) => {
            return Err(ApiError::bad_request("Nothing to commit - the tree is unchanged."));
        }
        Err(e) => return Err(internal(format!("{e:?}"))),
    };
    match store::read_tree(dir, &sha).map_err(internal)? {
        Some((m, _)) => commit_dto(&m, Some(&req.branch)),
        None => Ok(json!({"id": sha, "branch": req.branch})),
    }
}

pub async fn commit_files(
    state: &AppState,
    p: &mut Project,
    actor: &SiteUser,
    req: CommitRequest,
) -> Result<Value, ApiError> {
    let dir = repo_dir(state, p);
    let author = store::author_ident(actor_name(actor));
    let ts = dt::now().timestamp();
    let dto = run(move || commit_core(&dir, &req, &author, ts)).await?;
    touch(state, p).await?;
    Ok(dto)
}

pub async fn list_commits(
    state: &AppState,
    p: &Project,
    branch: Option<&str>,
    limit: usize,
    offset: usize,
) -> Result<Value, ApiError> {
    let (dir, default, branch) = (repo_dir(state, p), p.default_branch().to_owned(), branch.map(str::to_owned));
    run(move || list_commits_core(&dir, &default, branch.as_deref(), limit, offset)).await
}

pub fn list_commits_core(
    dir: &Path,
    default_branch: &str,
    branch: Option<&str>,
    limit: usize,
    offset: usize,
) -> Result<Value, ApiError> {
    let r = match branch {
        Some(b) if !b.is_empty() => b,
        _ => default_branch,
    };
    let metas = store::list_commits(dir, r, offset.saturating_add(limit)).map_err(internal)?;
    let total = store::count_commits(dir, r).map_err(internal)?;
    let items = metas.iter().skip(offset).take(limit).map(|m| commit_dto(m, Some(r))).collect::<Result<Vec<_>, _>>()?;
    Ok(json!({"items": items, "count": items.len(), "total": total}))
}

pub async fn get_tree(state: &AppState, p: &Project, r: &str) -> Result<Value, ApiError> {
    let (dir, default, r) = (repo_dir(state, p), p.default_branch().to_owned(), r.to_owned());
    run(move || get_tree_core(&dir, &default, &r)).await
}

pub fn get_tree_core(dir: &Path, default_branch: &str, r: &str) -> Result<Value, ApiError> {
    let r = if r.is_empty() { default_branch } else { r };
    match store::read_tree(dir, r).map_err(internal)? {
        None => Ok(json!({"commit": null, "entries": []})),
        Some((m, entries)) => Ok(json!({"commit": commit_dto(&m, None)?, "entries": entries_json(&entries)})),
    }
}

pub async fn get_file_bytes(state: &AppState, p: &Project, commit_ref: &str, path: &str) -> Result<Vec<u8>, ApiError> {
    let (dir, r, path) = (repo_dir(state, p), commit_ref.to_owned(), path.to_owned());
    run(move || file_bytes_core(&dir, &r, &path)).await
}

pub fn file_bytes_core(dir: &Path, commit_ref: &str, path: &str) -> Result<Vec<u8>, ApiError> {
    let norm = norm_path(path)?;
    store::read_blob(dir, commit_ref, &norm)
        .map_err(internal)?
        .ok_or_else(|| not_found(format!("No file '{path}' in that commit")))
}

pub async fn source_archive(state: &AppState, p: &Project, r: &str) -> Result<(String, Vec<u8>), ApiError> {
    let (dir, p, r) = (repo_dir(state, p), p.clone(), r.to_owned());
    run(move || archive_core(&dir, &p, &r, None)).await
}

/// `source_archive`: `(filename, zip bytes)`; `clock` pins the zip times.
pub fn archive_core(dir: &Path, p: &Project, r: &str, clock: Option<[u16; 6]>) -> Result<(String, Vec<u8>), ApiError> {
    let target = if r.is_empty() { p.default_branch() } else { r };
    let Some(sha) = resolve_hex(dir, target)? else {
        return Err(not_found(format!("No branch or commit '{target}', or no commits yet.")));
    };
    let entries = match store::read_tree(dir, &sha).map_err(internal)? {
        Some((_, entries)) if !entries.is_empty() => entries,
        _ => return Err(not_found("That commit has no files to download.")),
    };
    let mut files = Vec::with_capacity(entries.len());
    for e in entries {
        match store::read_blob(dir, &sha, &e.path).map_err(internal)? {
            Some(d) => files.push((e.path, d)),
            None => {
                return Err(ApiError::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    ErrorCode::InternalError,
                    format!("Missing blob for {}", e.path),
                ));
            }
        }
    }
    let archive = super::archive::build_zip(&files, clock);
    let name = format!("{}-{}-{}", safe_filename(p.str("slug")), target, &sha[..7]);
    Ok((format!("{}.zip", safe_filename(&name)), archive))
}

pub async fn compare(state: &AppState, p: &Project, base: &str, head: &str) -> Result<Value, ApiError> {
    let (dir, base, head) = (repo_dir(state, p), base.to_owned(), head.to_owned());
    run(move || compare_core(&dir, &base, &head)).await
}

pub fn compare_core(dir: &Path, base: &str, head: &str) -> Result<Value, ApiError> {
    let b = store::read_tree(dir, base).map_err(internal)?;
    let h = store::read_tree(dir, head).map_err(internal)?;
    let (Some(b), Some(h)) = (b, h) else { return Err(not_found("One of the refs doesn't exist on this project.")) };
    let bm: HashMap<&str, &str> = b.1.iter().map(|e| (e.path.as_str(), e.blob_sha.as_str())).collect();
    let hm: HashMap<&str, &str> = h.1.iter().map(|e| (e.path.as_str(), e.blob_sha.as_str())).collect();
    let mut added: Vec<&str> = hm.keys().filter(|p| !bm.contains_key(*p)).copied().collect();
    let mut removed: Vec<&str> = bm.keys().filter(|p| !hm.contains_key(*p)).copied().collect();
    let mut modified: Vec<&str> =
        hm.iter().filter(|(p, s)| bm.get(*p).is_some_and(|x| x != *s)).map(|(p, _)| *p).collect();
    added.sort_unstable();
    removed.sort_unstable();
    modified.sort_unstable();
    Ok(json!({
        "base": commit_dto(&b.0, None)?, "head": commit_dto(&h.0, None)?,
        "added": added, "removed": removed, "modified": modified,
    }))
}

// --- Trove placement rules (trove_layout.py) -------------------------------

const TROVE_DIRECTORIES: [&str; 11] = [
    "audio",
    "blueprints",
    "fonts",
    "languages",
    "models",
    "movies",
    "particles",
    "prefabs",
    "shadersunified",
    "textures",
    "ui",
];
const COMPILE_IGNORED_EXTENSIONS: [&str; 14] =
    [".tfi", ".tfa", ".exe", ".dll", ".tmod", ".zip", ".cfg", ".txt", ".log", ".ini", ".toml", ".json", ".xml", ".dat"];

fn ext(path: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or(path);
    match name.rsplit_once('.') {
        Some((_, e)) => format!(".{}", e.to_lowercase()),
        None => String::new(),
    }
}

fn is_compilable(path: &str) -> bool {
    let parts: Vec<&str> = path.split('/').collect();
    parts.len() >= 2
        && TROVE_DIRECTORIES.contains(&parts[0].to_lowercase().as_str())
        && !COMPILE_IGNORED_EXTENSIONS.contains(&ext(path).as_str())
}

fn skip_reason(path: &str) -> String {
    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() < 2 {
        return "root file (only files inside a Trove folder are compiled)".into();
    }
    if !TROVE_DIRECTORIES.contains(&parts[0].to_lowercase().as_str()) {
        return format!("'{}' is not a Trove folder", parts[0]);
    }
    let e = ext(path);
    if COMPILE_IGNORED_EXTENSIONS.contains(&e.as_str()) {
        return format!("'{e}' files aren't included in a build");
    }
    "ignored".into()
}

pub type GameMap = HashMap<String, String>;

/// `find_misplaced`: `(path, expected)` for each file the game keeps elsewhere.
pub fn find_misplaced(paths: &[String], game: &GameMap) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for p in paths {
        let name = p.rsplit('/').next().unwrap_or(p).to_lowercase();
        if let Some(expected) = game.get(&name)
            && !expected.is_empty()
            && expected.to_lowercase() != p.to_lowercase()
        {
            out.push((p.clone(), expected.clone()));
        }
    }
    out
}

pub fn placement_json(meta: Option<&Meta>, paths: &[String], game: &GameMap) -> Result<Value, ApiError> {
    let compilable = paths.iter().filter(|p| is_compilable(p)).count();
    let misplaced = find_misplaced(paths, game);
    let skipped: Vec<Value> = paths
        .iter()
        .filter(|p| !is_compilable(p) && !misplaced.iter().any(|(m, _)| m == *p))
        .map(|p| json!({"path": p, "reason": skip_reason(p)}))
        .collect();
    Ok(json!({
        "commit": meta.map(|m| commit_dto(m, None)).transpose()?,
        "total": paths.len(),
        "compilable_count": compilable,
        "skipped": skipped,
        "misplaced": misplaced.iter().map(|(p, e)| json!({"path": p, "expected": e})).collect::<Vec<_>>(),
        "fix_available": !misplaced.is_empty(),
        "game_index_available": !game.is_empty(),
    }))
}

pub fn empty_placement() -> Value {
    json!({"total": 0, "compilable_count": 0, "skipped": [], "misplaced": [],
           "fix_available": false, "game_index_available": false, "commit": null})
}

static GAME_MAP: Mutex<Option<(i64, Arc<GameMap>)>> = Mutex::new(None);

/// `trove_layout.game_file_map()`: `{filename: game path}` from the live
/// branch's archived tree (first path wins), cached per build ordinal.
pub async fn game_file_map(state: &AppState) -> Result<Arc<GameMap>, ApiError> {
    let db = mongo(state)?;
    let ub = db
        .collection::<Document>(crate::archiver::repo::BRANCHES)
        .find_one(doc! {"branch": LIVE_BRANCH})
        .await
        .map_err(internal)?;
    let ordinal = ub.as_ref().and_then(|d| match d.get("current_ordinal") {
        Some(Bson::Int32(n)) => Some(*n as i64),
        Some(Bson::Int64(n)) => Some(*n),
        _ => None,
    });
    let Some(ordinal) = ordinal.filter(|o| *o != 0) else { return Ok(Arc::new(GameMap::new())) };
    if let Ok(g) = GAME_MAP.lock()
        && let Some((o, m)) = g.as_ref()
        && *o == ordinal
        && !m.is_empty()
    {
        return Ok(m.clone());
    }
    let mut cursor = db
        .collection::<Document>(crate::archiver::repo::STATE)
        .find(doc! {"branch": LIVE_BRANCH})
        .with_options(FindOptions::builder().projection(doc! {"path": 1, "_id": 0}).build())
        .await
        .map_err(internal)?;
    let mut map = GameMap::new();
    while let Some(d) = cursor.try_next().await.map_err(internal)? {
        let Ok(path) = d.get_str("path") else { continue };
        if path.is_empty() {
            continue;
        }
        let name = path.rsplit('/').next().unwrap_or(path).to_lowercase();
        map.entry(name).or_insert_with(|| path.to_owned());
    }
    let map = Arc::new(map);
    if let Ok(mut g) = GAME_MAP.lock() {
        *g = Some((ordinal, map.clone()));
    }
    Ok(map)
}

pub async fn placement_report(state: &AppState, p: &Project, r: &str) -> Result<Value, ApiError> {
    let (dir, default, r2) = (repo_dir(state, p), p.default_branch().to_owned(), r.to_owned());
    let tree = run(move || {
        let r = if r2.is_empty() { default.as_str() } else { r2.as_str() };
        store::read_tree(&dir, r).map_err(internal)
    })
    .await?;
    let Some((meta, entries)) = tree else { return Ok(empty_placement()) };
    let paths: Vec<String> = entries.into_iter().map(|e| e.path).collect();
    let game = game_file_map(state).await?;
    placement_json(Some(&meta), &paths, &game)
}

/// `placement_report` with the game map in hand.
pub fn placement_core(dir: &Path, default_branch: &str, r: &str, game: &GameMap) -> Result<Value, ApiError> {
    let r = if r.is_empty() { default_branch } else { r };
    let Some((meta, entries)) = store::read_tree(dir, r).map_err(internal)? else { return Ok(empty_placement()) };
    let paths: Vec<String> = entries.into_iter().map(|e| e.path).collect();
    placement_json(Some(&meta), &paths, game)
}

/// The checks of `fix_placement` before the game map is needed: the branch
/// to fix, its head, and the head's paths.
pub fn fix_prepare(
    dir: &Path,
    p: &Project,
    actor: &SiteUser,
    branch: &str,
) -> Result<(String, String, Vec<String>), ApiError> {
    require_owner(p, actor)?;
    require_files_mode(p)?;
    let branch = valid_branch_name(if branch.is_empty() { p.default_branch() } else { branch })?;
    let Some(sha) = resolve_hex(dir, &branch)? else {
        return Err(not_found(format!("Branch '{branch}' has no commits to fix.")));
    };
    let Some((_, entries)) = store::read_tree(dir, &sha).map_err(internal)? else {
        return Err(not_found("Nothing to fix."));
    };
    Ok((branch, sha, entries.into_iter().map(|e| e.path).collect()))
}

pub async fn fix_placement(
    state: &AppState,
    p: &mut Project,
    actor: &SiteUser,
    branch: &str,
) -> Result<Value, ApiError> {
    let dir = repo_dir(state, p);
    let (d, pc, a, b) = (dir.clone(), p.clone(), actor.clone(), branch.to_owned());
    let (branch, sha, paths) = run(move || fix_prepare(&d, &pc, &a, &b)).await?;
    let game = game_file_map(state).await?;
    let author = store::author_ident(actor_name(actor));
    let ts = dt::now().timestamp();
    let out = run(move || fix_core(&dir, &branch, &sha, &paths, &game, &author, ts)).await?;
    touch(state, p).await?;
    Ok(out)
}

/// The rest of `fix_placement`: misplaced files moved in one commit.
pub fn fix_core(
    dir: &Path,
    branch: &str,
    sha: &str,
    paths: &[String],
    game: &GameMap,
    author: &[u8],
    ts: i64,
) -> Result<Value, ApiError> {
    if game.is_empty() {
        return Err(ApiError::conflict(
            "The game file index isn't available right now, so placement can't be auto-fixed. Try again later.",
        ));
    }
    let misplaced = find_misplaced(paths, game);
    if misplaced.is_empty() {
        return Err(ApiError::bad_request("Nothing to fix - every file is already in its correct place."));
    }
    let mut adds = Vec::new();
    let mut deletes = Vec::new();
    for (path, expected) in &misplaced {
        let Some(data) = store::read_blob(dir, sha, path).map_err(internal)? else { continue };
        adds.push((norm_path(expected)?, data));
        deletes.push(path.clone());
    }
    if adds.is_empty() {
        return Err(ApiError::bad_request("Nothing to fix."));
    }
    let n = adds.len();
    let message = format!("Fix file placement ({n} file{})", if n != 1 { "s" } else { "" });
    let commit = match store::write_commit(dir, branch, &adds, &deletes, author, &message, ts) {
        Ok(c) => c,
        Err(StoreError::NothingToCommit) => return Err(ApiError::bad_request("Nothing to fix.")),
        Err(e) => return Err(internal(format!("{e:?}"))),
    };
    let commit_json = match store::read_tree(dir, &commit).map_err(internal)? {
        Some((m, _)) => commit_dto(&m, Some(branch))?,
        None => json!({"id": commit}),
    };
    Ok(json!({"fixed": n, "commit": commit_json}))
}

//! The update archive read API: `updates_router` of `app/trove/router.py`
//! (`updates:read`, behind `require_updates_enabled`) and the `/site/updates/*`
//! proxies of `app/site/router.py`. The `/site` blueprint, SWF and BNK views
//! stay in Python until `bp_cache`, the SWF extractor and the bank reader are
//! ported; they are simply not routed here.

pub mod read;
pub mod vfx;

#[cfg(test)]
mod tests;

use axum::{
    Router,
    extract::{FromRequestParts, State},
    http::{HeaderMap, HeaderValue, StatusCode, Uri, header, request::Parts},
    response::{IntoResponse, Response},
    routing::get,
};
use chrono::{DateTime, Utc};
use kiwi_core::updates::compare::{HUNK_CONTEXT, decode_blob, make_hunks};
use serde_json::{Value, json};

use self::read::{Branch, FileMeta, HistoryRow, TreeEntry, Version, View};
use crate::{
    auth::{AccessContext, Rejection, RequireScope, extract::public_scope, scopes},
    dt,
    error::{self, ApiError},
    features::{self, RequireFeature},
    middleware::pyurl::{first, scope_path},
    pyjson::{PyJson, PydanticJson},
    ratelimit::RateLimitInfo,
    routes::store::service::{Fetch, Mongo},
    state::AppState,
    validation::{FromParams, Params, Query},
};

type UpdRead = RequireScope<{ scopes::UPDATES_READ }>;
type Gate = RequireFeature<features::Updates>;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/updates/branches", get(v1_branches))
        .route("/v1/updates/{branch}/versions", get(v1_versions))
        .route("/v1/updates/{branch}/changes", get(v1_changes))
        .route("/v1/updates/{branch}/tree", get(v1_tree))
        .route("/v1/updates/{branch}/file/meta", get(v1_meta))
        .route("/v1/updates/{branch}/file", get(v1_file))
        .route("/v1/updates/{branch}/file/view", get(v1_view))
        .route("/v1/updates/{branch}/file/history", get(v1_history))
        .route("/v1/updates/{branch}/file/compare", get(v1_compare))
        .route("/site/updates/branches", get(site_branches))
        .route("/site/updates/{branch}/versions", get(site_versions))
        .route("/site/updates/{branch}/changes", get(site_changes))
        .route("/site/updates/{branch}/tree", get(site_tree))
        .route("/site/updates/{branch}/search", get(site_search))
        .route("/site/updates/{branch}/file/meta", get(site_meta))
        .route("/site/updates/{branch}/file/view", get(site_view))
        .route("/site/updates/{branch}/file/history", get(site_history))
        .route("/site/updates/{branch}/file/compare", get(site_compare))
        .route("/site/updates/{branch}/vfx", get(site_vfx_list))
        .route("/site/updates/{branch}/vfx/manifest", get(site_vfx_manifest))
        .route("/site/updates/{branch}/vfx/asset", get(site_vfx_asset))
}

/// The two serializers on the wire: pydantic response models on `/v1`,
/// hand-built dicts through `iso()` + `JSONResponse` on `/site`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flavour {
    V1,
    Site,
}

impl Flavour {
    fn date(self, d: Option<&DateTime<Utc>>) -> Value {
        match (d, self) {
            (None, _) => Value::Null,
            (Some(d), Flavour::V1) => dt::pydantic(d).into(),
            (Some(d), Flavour::Site) => dt::isoformat(d).into(),
        }
    }
}

/// The Starlette-decoded `{branch}` segment.
pub fn branch_of(uri: &Uri) -> String {
    scope_path(uri).split('/').nth(3).unwrap_or_default().to_owned()
}

/// `_check_branch` / `_site_check_branch` (the same envelope either way).
pub fn check_branch(branch: &str) -> Result<(), ApiError> {
    if read::KNOWN.contains(&branch) {
        return Ok(());
    }
    Err(ApiError::not_found(format!("Unknown branch '{branch}' (known: {})", read::KNOWN.join(", "))))
}

fn mongo(state: &AppState) -> Mongo<'_> {
    Mongo { db: state.mongo(), blob_root: std::path::PathBuf::from(&state.settings().trove_update_store_dir) }
}

pub fn v1_json(rate_limit: Option<RateLimitInfo>, body: Value) -> Response {
    (rate_limit, PydanticJson(body)).into_response()
}

pub fn site_json(max_age: u32, body: Value) -> Response {
    let cc = HeaderValue::from_str(&format!("public, max-age={max_age}")).expect("cache-control");
    ([(header::CACHE_CONTROL, cc)], PyJson(body)).into_response()
}

fn reply(fl: Flavour, rate_limit: Option<RateLimitInfo>, max_age: u32, r: Result<Value, ApiError>) -> Response {
    match (r, fl) {
        (Ok(v), Flavour::V1) => v1_json(rate_limit, v),
        (Ok(v), Flavour::Site) => site_json(max_age, v),
        (Err(e), _) => e.into_response(),
    }
}

// --- query parameters ----------------------------------------------------------

pub struct PageQ {
    pub limit: i64,
    pub offset: i64,
}

impl FromParams for PageQ {
    fn from_params(p: &mut Params<'_>) -> Self {
        PageQ {
            limit: p.query("limit").int().ge(1).le(200).default(50),
            offset: p.query("offset").int().ge(0).default(0),
        }
    }
}

/// `ordinal`, `version`, `type`, `limit`, `offset`; the limit differs by surface.
pub struct ChangesQ<const DEF: i64, const MAX: i64> {
    pub ordinal: Option<i64>,
    pub version: Option<String>,
    pub kind: Option<String>,
    pub limit: i64,
    pub offset: i64,
}

impl<const DEF: i64, const MAX: i64> FromParams for ChangesQ<DEF, MAX> {
    fn from_params(p: &mut Params<'_>) -> Self {
        ChangesQ {
            ordinal: p.query("ordinal").int().optional(),
            version: p.query("version").str().optional(),
            kind: p.query("type").str().optional(),
            limit: p.query("limit").int().ge(1).le(MAX).default(DEF),
            offset: p.query("offset").int().ge(0).default(0),
        }
    }
}

pub struct TreeQ(pub String);

impl FromParams for TreeQ {
    fn from_params(p: &mut Params<'_>) -> Self {
        TreeQ(p.query("prefix").str().default(String::new()))
    }
}

pub struct PathQ(pub String);

impl FromParams for PathQ {
    fn from_params(p: &mut Params<'_>) -> Self {
        PathQ(p.query("path").str().required())
    }
}

pub struct CompareQ {
    pub path: String,
    pub from: i64,
    pub to: i64,
}

impl FromParams for CompareQ {
    fn from_params(p: &mut Params<'_>) -> Self {
        CompareQ {
            path: p.query("path").str().required(),
            from: p.query("from").int().required(),
            to: p.query("to").int().required(),
        }
    }
}

pub struct SearchQ {
    pub q: String,
    pub limit: i64,
}

impl FromParams for SearchQ {
    fn from_params(p: &mut Params<'_>) -> Self {
        SearchQ {
            q: p.query("q").str().min_len(1).max_len(200).required(),
            limit: p.query("limit").int().ge(1).le(500).default(200),
        }
    }
}

pub struct VfxListQ {
    pub q: String,
    pub limit: i64,
    pub offset: i64,
}

impl FromParams for VfxListQ {
    fn from_params(p: &mut Params<'_>) -> Self {
        VfxListQ {
            q: p.query("q").str().max_len(200).default(String::new()),
            limit: p.query("limit").int().ge(1).le(1000).default(300),
            offset: p.query("offset").int().ge(0).saturating().default(0),
        }
    }
}

// --- bodies --------------------------------------------------------------------

fn branch_json(b: &Branch, fl: Flavour) -> Value {
    json!({
        "branch": b.branch, "current_version": b.current_version, "current_ordinal": b.current_ordinal,
        "last_probe_at": fl.date(b.last_probe_at.as_ref()), "status": b.status, "file_count": b.file_count,
    })
}

fn version_json(v: &Version, fl: Flavour) -> Value {
    json!({
        "branch": v.branch, "ordinal": v.ordinal, "version_tag": v.version_tag,
        "captured_at": fl.date(Some(&v.captured_at)), "completed_at": fl.date(v.completed_at.as_ref()),
        "files_added": v.files_added, "files_modified": v.files_modified,
        "files_removed": v.files_removed, "bytes_added": v.bytes_added,
    })
}

fn tree_entry_json(e: &TreeEntry, fl: Flavour) -> Value {
    let c = &e.child;
    json!({
        "name": c.name, "path": c.path, "is_dir": c.is_dir, "file_count": c.file_count, "size": c.size,
        "last_ordinal": c.last_ordinal, "last_modified_at": fl.date(e.last_modified_at.as_ref()),
    })
}

fn meta_json(branch: &str, m: &FileMeta) -> Value {
    json!({
        "branch": branch, "path": m.path, "content_sha256": m.content_sha256, "size": m.size,
        "archive": m.archive, "archive_index": m.archive_index,
        "removed": m.removed, "removed_ordinal": m.removed_ordinal,
    })
}

fn view_json(branch: &str, path: &str, v: &View, fl: Flavour) -> Value {
    let viewable = v.kind == "text";
    let reason = if viewable { Value::Null } else { Value::from(v.kind) };
    let m = &v.meta;
    match fl {
        Flavour::V1 => json!({
            "branch": branch, "path": path, "size": m.size, "content_sha256": m.content_sha256,
            "viewable": viewable, "kind": v.kind, "reason": reason, "truncated": false, "text": v.text,
            "removed": m.removed, "removed_ordinal": m.removed_ordinal,
        }),
        Flavour::Site => json!({
            "branch": branch, "path": path, "size": m.size, "content_sha256": m.content_sha256,
            "truncated": false, "text": v.text, "removed": m.removed, "removed_ordinal": m.removed_ordinal,
            "viewable": viewable, "kind": v.kind, "reason": reason,
        }),
    }
}

fn history_json(r: &HistoryRow, fl: Flavour) -> Value {
    json!({
        "ordinal": r.ordinal, "version_tag": r.version_tag, "captured_at": fl.date(r.captured_at.as_ref()),
        "type": r.kind, "content_sha256": r.content_sha256, "size": r.size,
    })
}

// --- route bodies, over any store -------------------------------------------

pub async fn branches<F: Fetch>(f: &F, fl: Flavour) -> Value {
    let items: Vec<Value> = read::list_branches(f).await.iter().map(|b| branch_json(b, fl)).collect();
    json!({"items": items, "count": items.len()})
}

pub async fn versions<F: Fetch>(f: &F, branch: &str, q: &PageQ, fl: Flavour) -> Result<Value, ApiError> {
    check_branch(branch)?;
    let (docs, total) = read::list_versions(f, branch, q.limit, q.offset).await;
    let items: Vec<Value> = docs.iter().map(|v| version_json(v, fl)).collect();
    Ok(json!({"items": items, "count": items.len(), "total": total}))
}

pub async fn changes<F: Fetch, const D: i64, const M: i64>(
    f: &F,
    branch: &str,
    q: &ChangesQ<D, M>,
    fl: Flavour,
) -> Result<Value, ApiError> {
    check_branch(branch)?;
    if let Some(t) = q.kind.as_deref().filter(|t| !["added", "modified", "removed"].contains(t)) {
        return Err(match fl {
            Flavour::V1 => ApiError::bad_request(format!("Invalid type '{t}' (allowed: added, modified, removed)")),
            Flavour::Site => ApiError::http(StatusCode::BAD_REQUEST, Some(&format!("Invalid type '{t}'"))),
        });
    }
    let ver = read::resolve_version(f, branch, q.ordinal, q.version.as_deref())
        .await
        .ok_or_else(|| ApiError::not_found("No matching version for that branch"))?;
    let (docs, total) = read::list_changes(f, branch, ver.ordinal, q.kind.as_deref(), q.limit, q.offset).await;
    let entries: Vec<Value> = docs
        .iter()
        .map(|d| json!({"path": d.path, "type": d.kind, "content_sha256": d.content_sha256, "size": d.size}))
        .collect();
    Ok(json!({
        "branch": branch, "ordinal": ver.ordinal, "version_tag": ver.version_tag,
        "entries": entries, "count": docs.len(), "total": total,
        "files_added": ver.files_added, "files_modified": ver.files_modified, "files_removed": ver.files_removed,
    }))
}

pub async fn tree<F: Fetch>(f: &F, branch: &str, prefix: &str, fl: Flavour) -> Result<Value, ApiError> {
    check_branch(branch)?;
    let mut prefix = prefix.to_owned();
    if !prefix.is_empty() && !prefix.ends_with('/') {
        prefix.push('/');
    }
    let entries: Vec<Value> =
        read::list_directory(f, branch, &prefix).await.iter().map(|e| tree_entry_json(e, fl)).collect();
    Ok(json!({"branch": branch, "prefix": prefix, "entries": entries, "count": entries.len()}))
}

pub async fn search<F: Fetch>(f: &F, branch: &str, q: &SearchQ) -> Result<Value, ApiError> {
    check_branch(branch)?;
    let (hits, total) = read::search_paths(f, branch, &q.q, q.limit).await;
    let entries: Vec<Value> = hits
        .iter()
        .map(|h| {
            json!({
                "path": h.path, "name": h.path.rsplit('/').next().unwrap_or_default(), "size": h.size,
                "is_dir": false, "last_ordinal": h.last_ordinal,
                "last_modified_at": Flavour::Site.date(h.last_modified_at.as_ref()),
            })
        })
        .collect();
    Ok(json!({"branch": branch, "query": q.q, "entries": entries, "count": entries.len(), "total": total}))
}

fn no_file(path: &str) -> ApiError {
    ApiError::not_found(format!("No file '{path}'"))
}

pub async fn meta<F: Fetch>(f: &F, branch: &str, path: &str) -> Result<Value, ApiError> {
    check_branch(branch)?;
    let m = read::get_file_meta(f, branch, path).await.ok_or_else(|| no_file(path))?;
    Ok(meta_json(branch, &m))
}

pub async fn view<F: Fetch>(f: &F, branch: &str, path: &str, fl: Flavour) -> Result<Value, ApiError> {
    check_branch(branch)?;
    let v = read::read_file_text(f, branch, path).await.ok_or_else(|| no_file(path))?;
    Ok(view_json(branch, path, &v, fl))
}

pub async fn history<F: Fetch>(f: &F, branch: &str, path: &str, fl: Flavour) -> Result<Value, ApiError> {
    check_branch(branch)?;
    let rows = read::file_history(f, branch, path).await;
    if rows.is_empty() {
        return Err(ApiError::not_found(format!("No history for '{path}' on branch '{branch}'")));
    }
    let items: Vec<Value> = rows.iter().map(|r| history_json(r, fl)).collect();
    Ok(json!({"branch": branch, "path": path, "items": items, "count": items.len()}))
}

pub async fn compare<F: Fetch>(f: &F, branch: &str, q: &CompareQ, fl: Flavour) -> Result<Value, ApiError> {
    check_branch(branch)?;
    let v_from = read::resolve_version(f, branch, Some(q.from), None).await;
    let v_to = read::resolve_version(f, branch, Some(q.to), None).await;
    let (Some(v_from), Some(v_to)) = (v_from, v_to) else {
        return Err(ApiError::not_found("One of the requested ordinals doesn't exist on this branch"));
    };
    let a = read::resolve_file_at_version(f, branch, &q.path, q.from).await;
    let b = read::resolve_file_at_version(f, branch, &q.path, q.to).await;
    let sha = |s: &Option<(Option<String>, i64)>| s.as_ref().and_then(|s| s.0.clone());
    let (a_sha, b_sha) = (sha(&a), sha(&b));
    let info = |v: &Version, sha: &Option<String>, side: &Option<(Option<String>, i64)>| {
        json!({
            "ordinal": v.ordinal, "version_tag": v.version_tag, "captured_at": fl.date(Some(&v.captured_at)),
            "content_sha256": sha, "size": side.as_ref().map_or(0, |s| s.1),
        })
    };
    let mut out =
        json!({"branch": branch, "path": q.path, "from": info(&v_from, &a_sha, &a), "to": info(&v_to, &b_sha, &b)});
    let o = out.as_object_mut().expect("object");
    let finish =
        |o: &mut serde_json::Map<String, Value>, identical: bool, is_text: bool, reason: Option<&str>, hunks: Value| {
            o.insert("identical".into(), identical.into());
            o.insert("is_text".into(), is_text.into());
            match fl {
                Flavour::V1 => {
                    o.insert("reason".into(), reason.into());
                    o.insert("hunks".into(), hunks);
                }
                Flavour::Site if !is_text => {
                    o.insert("reason".into(), reason.into());
                    o.insert("hunks".into(), hunks);
                }
                Flavour::Site => {
                    o.insert("hunks".into(), hunks);
                    if let Some(r) = reason {
                        o.insert("reason".into(), r.into());
                    }
                }
            }
        };
    if a_sha.is_some() && a_sha.as_deref().unwrap_or("") == b_sha.as_deref().unwrap_or("") {
        finish(o, true, true, None, json!([]));
        return Ok(out);
    }
    if a.is_none() && b.is_none() {
        finish(o, true, true, Some("file did not exist at either side"), json!([]));
        return Ok(out);
    }
    let blob = |s: &Option<(Option<String>, i64)>| {
        let s = s.as_ref().map(|s| s.0.clone().unwrap_or_else(|| read::unhandled("TypeError: content_sha256 is None")));
        async move {
            match s {
                Some(sha) => f.blob(&sha).await,
                None => None,
            }
        }
    };
    let a_bytes = blob(&a).await;
    let b_bytes = blob(&b).await;
    let a_dec = decode_blob(a_bytes.as_deref());
    let b_dec = decode_blob(b_bytes.as_deref());
    if !(a_dec.is_text && b_dec.is_text) {
        let reason = a_dec.reason.or(b_dec.reason).unwrap_or("binary");
        finish(o, false, false, Some(reason), json!([]));
        return Ok(out);
    }
    let hunks: Vec<Value> = make_hunks(&a_dec.lines, &b_dec.lines, HUNK_CONTEXT)
        .iter()
        .map(|h| {
            let lines: Vec<Value> = h
                .lines
                .iter()
                .map(|l| json!({"kind": l.kind.as_str(), "left": l.left, "right": l.right, "text": l.text}))
                .collect();
            json!({"left_start": h.left_start, "right_start": h.right_start, "lines": lines})
        })
        .collect();
    finish(o, false, true, None, Value::Array(hunks));
    Ok(out)
}

pub async fn vfx_asset<F: Fetch>(cache: &vfx::Cache, f: &F, branch: &str, path: &str, headers: &HeaderMap) -> Response {
    if let Err(e) = check_branch(branch) {
        return e.into_response();
    }
    let Some((data, media, sha)) = vfx::asset(cache, f, branch, path).await else {
        return ApiError::not_found(format!("No asset '{path}'")).into_response();
    };
    let etag = HeaderValue::from_str(&format!("\"{sha}\"")).expect("etag");
    if first(headers, "if-none-match").is_some_and(|v| v.as_bytes() == etag.as_bytes()) {
        return (StatusCode::NOT_MODIFIED, [(header::ETAG, etag)]).into_response();
    }
    (
        [
            (header::ETAG, etag),
            (header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=86400")),
            (header::CONTENT_TYPE, HeaderValue::from_static(media)),
        ],
        data,
    )
        .into_response()
}

// --- handlers --------------------------------------------------------------------

async fn v1_branches(State(state): State<AppState>, _: Gate, RequireScope(ctx): UpdRead) -> Response {
    v1_json(Some(ctx.rate_limit), branches(&mongo(&state), Flavour::V1).await)
}

async fn v1_versions(
    State(state): State<AppState>,
    _: Gate,
    RequireScope(ctx): UpdRead,
    uri: Uri,
    Query(q): Query<PageQ>,
) -> Response {
    reply(Flavour::V1, Some(ctx.rate_limit), 0, versions(&mongo(&state), &branch_of(&uri), &q, Flavour::V1).await)
}

async fn v1_changes(
    State(state): State<AppState>,
    _: Gate,
    RequireScope(ctx): UpdRead,
    uri: Uri,
    Query(q): Query<ChangesQ<100, 1000>>,
) -> Response {
    reply(Flavour::V1, Some(ctx.rate_limit), 0, changes(&mongo(&state), &branch_of(&uri), &q, Flavour::V1).await)
}

async fn v1_tree(
    State(state): State<AppState>,
    _: Gate,
    RequireScope(ctx): UpdRead,
    uri: Uri,
    Query(TreeQ(prefix)): Query<TreeQ>,
) -> Response {
    reply(Flavour::V1, Some(ctx.rate_limit), 0, tree(&mongo(&state), &branch_of(&uri), &prefix, Flavour::V1).await)
}

async fn v1_meta(
    State(state): State<AppState>,
    _: Gate,
    RequireScope(ctx): UpdRead,
    uri: Uri,
    Query(PathQ(path)): Query<PathQ>,
) -> Response {
    reply(Flavour::V1, Some(ctx.rate_limit), 0, meta(&mongo(&state), &branch_of(&uri), &path).await)
}

/// `public_scope("updates:read", rate_multiplier=settings.updates_file_rate_limit_multiplier)`.
pub struct FileScope;

impl FromRequestParts<AppState> for FileScope {
    type Rejection = Rejection;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Rejection> {
        let multiplier = state.settings().updates_file_rate_limit_multiplier;
        let _: AccessContext = public_scope(state, parts, scopes::UPDATES_READ, multiplier, None).await?;
        Ok(FileScope)
    }
}

/// `download_update_file`: Starlette `FileResponse` over the CAS blob.
pub async fn file<F: Fetch>(
    f: &F,
    store_dir: &str,
    branch: &str,
    path: &str,
    headers: &HeaderMap,
) -> Result<Response, ApiError> {
    check_branch(branch)?;
    let m = read::get_file_meta(f, branch, path).await.ok_or_else(|| no_file(path))?;
    let shard: String = m.content_sha256.chars().take(2).collect();
    let blob = std::path::Path::new(store_dir).join("objects").join(shard).join(&m.content_sha256);
    if !tokio::fs::metadata(&blob).await.is_ok_and(|md| md.is_file()) {
        return Err(ApiError::not_found("Blob missing from the store"));
    }
    let name = path.rsplit('/').next().filter(|n| !n.is_empty()).unwrap_or("file");
    crate::drops::download::file_response_cached(
        &blob,
        "application/octet-stream",
        name,
        "public, max-age=86400",
        headers,
    )
    .await
}

async fn v1_file(
    State(state): State<AppState>,
    _: Gate,
    _: FileScope,
    uri: Uri,
    headers: HeaderMap,
    Query(PathQ(path)): Query<PathQ>,
) -> Response {
    let store_dir = state.settings().trove_update_store_dir.clone();
    match file(&mongo(&state), &store_dir, &branch_of(&uri), &path, &headers).await {
        Ok(r) => r,
        Err(e) => e.into_response(),
    }
}

async fn v1_view(
    State(state): State<AppState>,
    _: Gate,
    RequireScope(ctx): UpdRead,
    uri: Uri,
    Query(PathQ(path)): Query<PathQ>,
) -> Response {
    reply(Flavour::V1, Some(ctx.rate_limit), 0, view(&mongo(&state), &branch_of(&uri), &path, Flavour::V1).await)
}

async fn v1_history(
    State(state): State<AppState>,
    _: Gate,
    RequireScope(ctx): UpdRead,
    uri: Uri,
    Query(PathQ(path)): Query<PathQ>,
) -> Response {
    reply(Flavour::V1, Some(ctx.rate_limit), 0, history(&mongo(&state), &branch_of(&uri), &path, Flavour::V1).await)
}

async fn v1_compare(
    State(state): State<AppState>,
    _: Gate,
    RequireScope(ctx): UpdRead,
    uri: Uri,
    Query(q): Query<CompareQ>,
) -> Response {
    reply(Flavour::V1, Some(ctx.rate_limit), 0, compare(&mongo(&state), &branch_of(&uri), &q, Flavour::V1).await)
}

/// The site router's `_resolve_feature_flags`: every site flag is read, then a
/// disabled updates archive is the framework 404.
pub struct SiteGate;

impl FromRequestParts<AppState> for SiteGate {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Response> {
        let mut updates = true;
        for flag in crate::activity::SITE_FLAGS {
            let on = features::is_enabled(state, flag).await.map_err(|e| {
                tracing::error!("site feature flag {flag}: {e:#}");
                ApiError::internal().into_response()
            })?;
            if flag == features::UPDATES_FLAG {
                updates = on;
            }
        }
        let path = crate::middleware::pyurl::url_path(&parts.uri);
        if !updates && path.starts_with("/site/updates/") {
            return Err(error::http_exception(StatusCode::NOT_FOUND, None, &parts.method, &path, &parts.headers));
        }
        Ok(SiteGate)
    }
}

async fn site_branches(State(state): State<AppState>, _: SiteGate) -> Response {
    site_json(30, branches(&mongo(&state), Flavour::Site).await)
}

async fn site_versions(State(state): State<AppState>, _: SiteGate, uri: Uri, Query(q): Query<PageQ>) -> Response {
    reply(Flavour::Site, None, 30, versions(&mongo(&state), &branch_of(&uri), &q, Flavour::Site).await)
}

async fn site_changes(
    State(state): State<AppState>,
    _: SiteGate,
    uri: Uri,
    Query(q): Query<ChangesQ<200, 2000>>,
) -> Response {
    reply(Flavour::Site, None, 30, changes(&mongo(&state), &branch_of(&uri), &q, Flavour::Site).await)
}

async fn site_tree(
    State(state): State<AppState>,
    _: SiteGate,
    uri: Uri,
    Query(TreeQ(prefix)): Query<TreeQ>,
) -> Response {
    reply(Flavour::Site, None, 60, tree(&mongo(&state), &branch_of(&uri), &prefix, Flavour::Site).await)
}

async fn site_search(State(state): State<AppState>, _: SiteGate, uri: Uri, Query(q): Query<SearchQ>) -> Response {
    reply(Flavour::Site, None, 30, search(&mongo(&state), &branch_of(&uri), &q).await)
}

async fn site_meta(State(state): State<AppState>, _: SiteGate, uri: Uri, Query(PathQ(path)): Query<PathQ>) -> Response {
    reply(Flavour::Site, None, 60, meta(&mongo(&state), &branch_of(&uri), &path).await)
}

async fn site_view(State(state): State<AppState>, _: SiteGate, uri: Uri, Query(PathQ(path)): Query<PathQ>) -> Response {
    reply(Flavour::Site, None, 60, view(&mongo(&state), &branch_of(&uri), &path, Flavour::Site).await)
}

async fn site_history(
    State(state): State<AppState>,
    _: SiteGate,
    uri: Uri,
    Query(PathQ(path)): Query<PathQ>,
) -> Response {
    reply(Flavour::Site, None, 30, history(&mongo(&state), &branch_of(&uri), &path, Flavour::Site).await)
}

async fn site_compare(State(state): State<AppState>, _: SiteGate, uri: Uri, Query(q): Query<CompareQ>) -> Response {
    reply(Flavour::Site, None, 60, compare(&mongo(&state), &branch_of(&uri), &q, Flavour::Site).await)
}

async fn site_vfx_list(State(state): State<AppState>, _: SiteGate, uri: Uri, Query(q): Query<VfxListQ>) -> Response {
    let branch = branch_of(&uri);
    if let Err(e) = check_branch(&branch) {
        return e.into_response();
    }
    site_json(120, vfx::list_effects(&vfx::CACHE, &mongo(&state), &branch, &q.q, q.limit, q.offset).await)
}

async fn site_vfx_manifest(
    State(state): State<AppState>,
    _: SiteGate,
    uri: Uri,
    Query(PathQ(path)): Query<PathQ>,
) -> Response {
    let branch = branch_of(&uri);
    if let Err(e) = check_branch(&branch) {
        return e.into_response();
    }
    match vfx::manifest(&vfx::CACHE, &mongo(&state), &branch, &path).await {
        Some(v) => site_json(300, v),
        None => ApiError::not_found(format!("No effect '{path}'")).into_response(),
    }
}

async fn site_vfx_asset(
    State(state): State<AppState>,
    _: SiteGate,
    uri: Uri,
    headers: HeaderMap,
    Query(PathQ(path)): Query<PathQ>,
) -> Response {
    vfx_asset(&vfx::CACHE, &mongo(&state), &branch_of(&uri), &path, &headers).await
}

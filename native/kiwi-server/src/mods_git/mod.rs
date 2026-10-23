//! Mods Hub file history: the per-project git store, the `/git/mods/*`
//! smart-HTTP endpoints, branch / commit / tree / compare / placement reads
//! and writes, git access tokens, and the `/site/mods/*` wrappers of those
//! reads (`app/trove/mods_hub/gitstore.py`, `git_http.py`, the matching parts
//! of `service.py` and `router.py`, and `app/site/router.py`).

#![allow(clippy::result_large_err)]

pub mod archive;
pub mod odb;
pub mod project;
pub mod refs;
pub mod service;
pub mod smart;
pub mod store;
pub mod tokens;
pub mod write_auth;

#[cfg(test)]
mod tests;

use axum::{
    Router,
    body::Body,
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, Uri, header, request::Parts},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
};
use kiwi_core::form::{self, FormError, FormValue};
use serde_json::{Value, json};

use crate::{
    auth::{
        Rejection,
        extract::{OptionalSiteUser, public_scope},
        models::SiteUser,
        scopes,
    },
    error::{self, ApiError},
    features::{self, ModsHub},
    middleware::pyurl::scope_path,
    pyjson::PyJson,
    ratelimit::RateLimitInfo,
    state::AppState,
    validation::{self, ErrorItem, Field, FromBody, LocItem, Params, ValidationError},
};
use project::Project;
use service::internal;
use smart::Service;

type Res<T = Response> = Result<T, Response>;

fn e<E: IntoResponse>(err: E) -> Response {
    err.into_response()
}

pub fn router() -> Router<AppState> {
    let na = |allow: &'static str| move || async move { error::method_not_allowed(allow) };
    const P: &str = "/v1/mods/hub/projects/{handle}/{slug}";
    const S: &str = "/site/mods/projects/{handle}/{slug}";
    Router::new()
        .route("/git/mods/{handle}/{slug}/info/refs", get(git_info_refs).fallback(na("GET")))
        .route("/git/mods/{handle}/{slug}/git-upload-pack", post(git_upload_pack).fallback(na("POST")))
        .route("/git/mods/{handle}/{slug}/git-receive-pack", post(git_receive_pack).fallback(na("POST")))
        .route(&format!("{P}/branches"), get(v1_branches).post(create_branch).fallback(na("GET")))
        .route(&format!("{P}/branches/{{name}}"), delete(delete_branch).fallback(na("DELETE")))
        .route(&format!("{P}/commits"), get(v1_commits).post(create_commit).fallback(na("GET")))
        .route(&format!("{P}/tree"), get(v1_tree).fallback(na("GET")))
        .route(&format!("{P}/raw/{{commit_ref}}/{{*path}}"), get(v1_raw).fallback(na("GET")))
        .route(&format!("{P}/archive"), get(v1_archive).fallback(na("GET")))
        .route(&format!("{P}/compare"), get(v1_compare).fallback(na("GET")))
        .route(&format!("{P}/placement"), get(v1_placement).fallback(na("GET")))
        .route(&format!("{P}/fix-placement"), post(fix_placement).fallback(na("POST")))
        .route("/v1/mods/hub/me/projects/{handle}/{slug}/commits", get(me_commits).fallback(na("GET")))
        .route("/v1/mods/hub/me/projects/{handle}/{slug}/placement", get(me_placement).fallback(na("GET")))
        .route("/v1/mods/hub/me/git-tokens", get(list_tokens).post(create_token).fallback(na("GET")))
        .route("/v1/mods/hub/me/git-tokens/{token_id}", delete(revoke_token).fallback(na("DELETE")))
        .route(&format!("{S}/branches"), get(site_branches).fallback(na("GET")))
        .route(&format!("{S}/commits"), get(site_commits).fallback(na("GET")))
        .route(&format!("{S}/tree"), get(site_tree).fallback(na("GET")))
        .route(&format!("{S}/placement"), get(site_placement).fallback(na("GET")))
        .route(&format!("{S}/raw/{{commit_ref}}/{{*path}}"), get(site_raw).fallback(na("GET")))
        .route(&format!("{S}/archive"), get(site_archive).fallback(na("GET")))
}

/// For `routes::defer_to_python`: Starlette's `{slug}.git` matches only a
/// segment ending in `.git` with something before it.
pub fn defers(matched: &str, uri: &Uri) -> bool {
    if !matched.starts_with("/git/mods/") {
        return false;
    }
    let path = scope_path(uri);
    let seg = path.split('/').nth(4).unwrap_or("");
    !(seg.len() > 4 && seg.ends_with(".git"))
}

fn segments(uri: &Uri) -> Vec<String> {
    scope_path(uri).split('/').map(str::to_owned).collect()
}

fn seg(uri: &Uri, i: usize) -> String {
    segments(uri).into_iter().nth(i).unwrap_or_default()
}

async fn gate(state: &AppState) -> Res<()> {
    features::check::<ModsHub>(state).await.map_err(e)
}

/// The site router's `_resolve_feature_flags`: every flag read, a bare 404
/// while the Mods Hub is off.
async fn site_gate(state: &AppState) -> Res<()> {
    let mut on = true;
    for flag in crate::activity::SITE_FLAGS {
        let v = features::is_enabled(state, flag).await.map_err(|err| e(internal(format!("site flags: {err:#}"))))?;
        if flag == features::MODS_HUB_FLAG {
            on = v;
        }
    }
    if on { Ok(()) } else { Err(e(ApiError::http(StatusCode::NOT_FOUND, None))) }
}

/// `public_scope("mods:read", ...)` in the hub's browse or asset bucket.
async fn hub_scope(state: &AppState, parts: &Parts, asset: bool) -> Res<RateLimitInfo> {
    let s = state.settings();
    let (mult, bucket) = if asset {
        (s.mods_asset_rate_limit_multiplier, "mods:asset")
    } else {
        (s.mods_rate_limit_multiplier, "mods:browse")
    };
    Ok(public_scope(state, parts, scopes::MODS_READ, mult, Some(bucket)).await.map_err(e)?.rate_limit)
}

async fn site_viewer(state: &AppState, parts: &mut Parts) -> Option<SiteUser> {
    use axum::extract::FromRequestParts;
    OptionalSiteUser::from_request_parts(parts, state).await.ok().and_then(|OptionalSiteUser(u)| u)
}

fn params<'a>(parts: &'a Parts) -> Params<'a> {
    Params::new(Vec::new(), parts.uri.query(), &parts.headers)
}

fn finish<T>(p: Params<'_>, v: T) -> Res<T> {
    p.v.finish(v).map_err(e)
}

struct CommitsQuery {
    branch: Option<String>,
    limit: usize,
    offset: usize,
}

fn commits_query(parts: &Parts) -> Res<CommitsQuery> {
    let mut p = params(parts);
    let branch = p.query("branch").str().optional();
    let limit = p.query("limit").int().ge(1).le(200).default(50);
    let offset = p.query("offset").int().ge(0).saturating().default(0);
    let q = CommitsQuery { branch, limit: limit as usize, offset: offset.clamp(0, i64::MAX) as usize };
    finish(p, q)
}

fn ref_query(parts: &Parts) -> Res<String> {
    let mut p = params(parts);
    let r = p.query("ref").str().default(String::new());
    finish(p, r)
}

fn with_rl(rl: Option<RateLimitInfo>, status: StatusCode, v: Value) -> Response {
    let mut r = (status, PyJson(v)).into_response();
    if let Some(rl) = rl {
        for (k, val) in rl.headers() {
            r.headers_mut().insert(k, val);
        }
    }
    r
}

fn no_cache(mut r: Response) -> Response {
    r.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    r
}

fn octet(data: Vec<u8>) -> Response {
    ([(header::CONTENT_TYPE, HeaderValue::from_static("application/octet-stream"))], data).into_response()
}

fn zip_response(name: &str, data: Vec<u8>) -> Res<Response> {
    let cd = crate::media::latin1_header(&format!("attachment; filename=\"{name}\"")).map_err(e)?;
    Ok(([(header::CONTENT_TYPE, HeaderValue::from_static("application/zip")), (header::CONTENT_DISPOSITION, cd)], data)
        .into_response())
}

fn collapse(r: Res) -> Response {
    r.unwrap_or_else(|x| x)
}

// --- /v1/mods/hub public reads ---------------------------------------------

async fn v1_project(state: &AppState, parts: &Parts) -> Res<Project> {
    let p = service::get_for_view(state, &seg(&parts.uri, 5), &seg(&parts.uri, 6), None).await.map_err(e)?;
    service::ensure_source_visible(state, &p, None).map_err(e)?;
    Ok(p)
}

async fn v1_branches(State(state): State<AppState>, req: Request) -> Response {
    let (parts, _) = req.into_parts();
    collapse(
        async {
            gate(&state).await?;
            let rl = hub_scope(&state, &parts, false).await?;
            let p = v1_project(&state, &parts).await?;
            let items = service::list_branches(&state, &p).await.map_err(e)?;
            Ok(with_rl(Some(rl), StatusCode::OK, json!({"items": items})))
        }
        .await,
    )
}

async fn v1_commits(State(state): State<AppState>, req: Request) -> Response {
    let (parts, _) = req.into_parts();
    collapse(
        async {
            gate(&state).await?;
            let rl = hub_scope(&state, &parts, false).await?;
            let q = commits_query(&parts)?;
            let p = v1_project(&state, &parts).await?;
            let v = service::list_commits(&state, &p, q.branch.as_deref(), q.limit, q.offset).await.map_err(e)?;
            Ok(with_rl(Some(rl), StatusCode::OK, v))
        }
        .await,
    )
}

async fn v1_tree(State(state): State<AppState>, req: Request) -> Response {
    let (parts, _) = req.into_parts();
    collapse(
        async {
            gate(&state).await?;
            let rl = hub_scope(&state, &parts, false).await?;
            let r = ref_query(&parts)?;
            let p = v1_project(&state, &parts).await?;
            let v = service::get_tree(&state, &p, &r).await.map_err(e)?;
            Ok(with_rl(Some(rl), StatusCode::OK, v))
        }
        .await,
    )
}

fn raw_path(uri: &Uri, at: usize) -> (String, String) {
    let s = segments(uri);
    let commit_ref = s.get(at).cloned().unwrap_or_default();
    let path = s.get(at + 1..).map(|r| r.join("/")).unwrap_or_default();
    (commit_ref, path)
}

async fn v1_raw(State(state): State<AppState>, req: Request) -> Response {
    let (parts, _) = req.into_parts();
    collapse(
        async {
            gate(&state).await?;
            hub_scope(&state, &parts, true).await?;
            let p = v1_project(&state, &parts).await?;
            let (r, path) = raw_path(&parts.uri, 8);
            let data = service::get_file_bytes(&state, &p, &r, &path).await.map_err(e)?;
            Ok(octet(data))
        }
        .await,
    )
}

async fn v1_archive(State(state): State<AppState>, req: Request) -> Response {
    let (parts, _) = req.into_parts();
    collapse(
        async {
            gate(&state).await?;
            hub_scope(&state, &parts, true).await?;
            let r = ref_query(&parts)?;
            let p = v1_project(&state, &parts).await?;
            let (name, data) = service::source_archive(&state, &p, &r).await.map_err(e)?;
            zip_response(&name, data)
        }
        .await,
    )
}

async fn v1_compare(State(state): State<AppState>, req: Request) -> Response {
    let (parts, _) = req.into_parts();
    collapse(
        async {
            gate(&state).await?;
            let rl = hub_scope(&state, &parts, false).await?;
            let mut q = params(&parts);
            let base = q.query("base").str().required();
            let head = q.query("head").str().required();
            let (base, head) = finish(q, (base, head))?;
            let p = v1_project(&state, &parts).await?;
            let v = service::compare(&state, &p, &base, &head).await.map_err(e)?;
            Ok(with_rl(Some(rl), StatusCode::OK, v))
        }
        .await,
    )
}

async fn v1_placement(State(state): State<AppState>, req: Request) -> Response {
    let (parts, _) = req.into_parts();
    collapse(
        async {
            gate(&state).await?;
            let rl = hub_scope(&state, &parts, false).await?;
            let r = ref_query(&parts)?;
            let p = v1_project(&state, &parts).await?;
            let v = service::placement_report(&state, &p, &r).await.map_err(e)?;
            Ok(with_rl(Some(rl), StatusCode::OK, v))
        }
        .await,
    )
}

// --- writes and the creator's own reads ------------------------------------

async fn writer(
    state: &AppState,
    parts: &Parts,
    hs: Option<(&str, &str)>,
    api_allowed: bool,
) -> Res<write_auth::WriteUser> {
    write_auth::write_user(state, parts, hs, api_allowed).await.map_err(e)
}

struct BranchBody {
    name: String,
    from_ref: Option<String>,
}

impl FromBody for BranchBody {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| {
            Some(BranchBody {
                name: m.field("name").str().min_len(1).max_len(80).required(),
                from_ref: m.field("from_ref").str().optional(),
            })
        })
        .required()
    }
}

async fn create_branch(State(state): State<AppState>, req: Request) -> Response {
    let (parts, body) = req.into_parts();
    collapse(
        async {
            let body = validation::read_body(&parts.headers, body).await.map_err(e)?;
            gate(&state).await?;
            let (h, s) = (seg(&parts.uri, 5), seg(&parts.uri, 6));
            let w = writer(&state, &parts, Some((&h, &s)), true).await?;
            let b: BranchBody = validation::validate(&body).map_err(e)?;
            let p = service::require_owned(&state, &h, &s, &w.user).await.map_err(e)?;
            let v = service::create_branch(&state, &p, &w.user, &b.name, b.from_ref.as_deref()).await.map_err(e)?;
            Ok(with_rl(w.rate_limit, StatusCode::CREATED, v))
        }
        .await,
    )
}

async fn delete_branch(State(state): State<AppState>, req: Request) -> Response {
    let (parts, _) = req.into_parts();
    collapse(
        async {
            gate(&state).await?;
            let (h, s) = (seg(&parts.uri, 5), seg(&parts.uri, 6));
            let w = writer(&state, &parts, Some((&h, &s)), true).await?;
            let p = service::require_owned(&state, &h, &s, &w.user).await.map_err(e)?;
            service::delete_branch(&state, &p, &w.user, &seg(&parts.uri, 8)).await.map_err(e)?;
            Ok(StatusCode::NO_CONTENT.into_response())
        }
        .await,
    )
}

fn item(kind: &'static str, loc: Vec<LocItem>, msg: &str, input: Value, ctx: Option<Value>) -> ErrorItem {
    ErrorItem {
        kind: kind.into(),
        loc,
        msg: msg.into(),
        input,
        ctx: ctx.and_then(|c| match c {
            Value::Object(m) => Some(m),
            _ => None,
        }),
    }
}

/// The web commit form after FastAPI's extraction: `branch`, `message`,
/// `files`, `paths`, `deletes`.
pub struct CommitForm {
    pub branch: String,
    pub message: String,
    pub files: Vec<form::UploadFile>,
    pub paths: Vec<String>,
    pub deletes: Vec<String>,
}

pub fn commit_form(items: &[(String, FormValue)]) -> Result<CommitForm, Vec<ErrorItem>> {
    use crate::routes::misc::feedback::{str_field, upload_json};
    let mut errors = Vec::new();
    let present = |name: &str| {
        items
            .iter()
            .rev()
            .find(|(k, _)| k == name)
            .is_some_and(|(_, v)| !matches!(v, FormValue::Text(t) if t.is_empty()))
    };
    let required = |name: &str, errors: &mut Vec<ErrorItem>| -> String {
        if !present(name) {
            errors.push(item("missing", vec!["body".into(), name.into()], "Field required", Value::Null, None));
        }
        str_field(items, name, None, None, errors).unwrap_or_default()
    };
    let branch = required("branch", &mut errors);
    let message = required("message", &mut errors);
    let mut files = Vec::new();
    for (i, (_, v)) in items.iter().filter(|(k, _)| k == "files").enumerate() {
        match v {
            FormValue::File(f) => files.push(f.clone()),
            FormValue::Text(t) => errors.push(item(
                "value_error",
                vec!["body".into(), "files".into(), i.into()],
                "Value error, Expected UploadFile, received: <class 'str'>",
                Value::String(t.clone()),
                Some(json!({"error": {}})),
            )),
        }
    }
    let strs = |name: &str, errors: &mut Vec<ErrorItem>| -> Vec<String> {
        let mut out = Vec::new();
        for (i, (_, v)) in items.iter().filter(|(k, _)| k == name).enumerate() {
            match v {
                FormValue::Text(t) => out.push(t.clone()),
                FormValue::File(f) => errors.push(item(
                    "string_type",
                    vec!["body".into(), name.into(), i.into()],
                    "Input should be a valid string",
                    upload_json(f),
                    None,
                )),
            }
        }
        out
    };
    let paths = strs("paths", &mut errors);
    let deletes = strs("deletes", &mut errors);
    if errors.is_empty() { Ok(CommitForm { branch, message, files, paths, deletes }) } else { Err(errors) }
}

/// The router's `(path, bytes)` pairs: each file's parallel `paths` entry,
/// else its upload name, else `file-<i>`.
pub fn commit_adds(f: &CommitForm) -> Vec<(String, Vec<u8>)> {
    f.files
        .iter()
        .enumerate()
        .map(|(i, file)| {
            let path = match f.paths.get(i) {
                Some(p) => p.clone(),
                None if !file.filename.is_empty() => file.filename.clone(),
                None => format!("file-{i}"),
            };
            (path, file.data.clone())
        })
        .collect()
}

async fn create_commit(State(state): State<AppState>, req: Request) -> Response {
    let (parts, body) = req.into_parts();
    collapse(
        async {
            let Ok(bytes) = axum::body::to_bytes(body, usize::MAX).await else {
                return Err(e(ApiError::http(StatusCode::BAD_REQUEST, Some("There was an error parsing the body"))));
            };
            let ct = parts.headers.get(header::CONTENT_TYPE).map(|v| v.as_bytes().to_vec());
            let items = tokio::task::spawn_blocking(move || form::parse_form(ct.as_deref(), &bytes))
                .await
                .map_err(|x| e(internal(x)))?
                .map_err(|FormError::Http(msg)| e(ApiError::http(StatusCode::BAD_REQUEST, Some(&msg))))?;
            gate(&state).await?;
            let (h, s) = (seg(&parts.uri, 5), seg(&parts.uri, 6));
            let w = writer(&state, &parts, Some((&h, &s)), true).await?;
            let f = commit_form(&items).map_err(|errs| e(ValidationError(errs)))?;
            drop(items);
            let mut p = service::require_owned(&state, &h, &s, &w.user).await.map_err(e)?;
            let settings = state.settings();
            let req = service::check_commit(
                &p,
                &w.user,
                &f.branch,
                &f.message,
                commit_adds(&f),
                &f.deletes,
                settings.mods_hub_max_files_per_commit,
                settings.mods_hub_max_file_bytes,
            )
            .map_err(e)?;
            drop(f);
            let v = service::commit_files(&state, &mut p, &w.user, req).await.map_err(e)?;
            Ok(with_rl(w.rate_limit, StatusCode::CREATED, v))
        }
        .await,
    )
}

async fn fix_placement(State(state): State<AppState>, req: Request) -> Response {
    let (parts, _) = req.into_parts();
    collapse(
        async {
            gate(&state).await?;
            let (h, s) = (seg(&parts.uri, 5), seg(&parts.uri, 6));
            let w = writer(&state, &parts, Some((&h, &s)), true).await?;
            let mut q = params(&parts);
            let branch = q.query("branch").str().default(String::new());
            let branch = finish(q, branch)?;
            let mut p = service::require_owned(&state, &h, &s, &w.user).await.map_err(e)?;
            let v = service::fix_placement(&state, &mut p, &w.user, &branch).await.map_err(e)?;
            Ok(with_rl(w.rate_limit, StatusCode::CREATED, v))
        }
        .await,
    )
}

async fn me_commits(State(state): State<AppState>, req: Request) -> Response {
    let (parts, _) = req.into_parts();
    collapse(
        async {
            gate(&state).await?;
            let (h, s) = (seg(&parts.uri, 6), seg(&parts.uri, 7));
            let w = writer(&state, &parts, Some((&h, &s)), true).await?;
            let q = commits_query(&parts)?;
            let p = service::require_owned(&state, &h, &s, &w.user).await.map_err(e)?;
            let v = service::list_commits(&state, &p, q.branch.as_deref(), q.limit, q.offset).await.map_err(e)?;
            Ok(with_rl(w.rate_limit, StatusCode::OK, v))
        }
        .await,
    )
}

async fn me_placement(State(state): State<AppState>, req: Request) -> Response {
    let (parts, _) = req.into_parts();
    collapse(
        async {
            gate(&state).await?;
            let (h, s) = (seg(&parts.uri, 6), seg(&parts.uri, 7));
            let w = writer(&state, &parts, Some((&h, &s)), true).await?;
            let r = ref_query(&parts)?;
            let p = service::require_owned(&state, &h, &s, &w.user).await.map_err(e)?;
            let v = service::placement_report(&state, &p, &r).await.map_err(e)?;
            Ok(with_rl(w.rate_limit, StatusCode::OK, v))
        }
        .await,
    )
}

struct TokenBody {
    name: String,
}

impl FromBody for TokenBody {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| Some(TokenBody { name: m.field("name").str().max_len(60).default(String::new()) })).required()
    }
}

async fn list_tokens(State(state): State<AppState>, req: Request) -> Response {
    let (parts, _) = req.into_parts();
    collapse(
        async {
            gate(&state).await?;
            let w = writer(&state, &parts, None, false).await?;
            let v = tokens::list(&state, &w.user).await.map_err(e)?;
            Ok(with_rl(w.rate_limit, StatusCode::OK, v))
        }
        .await,
    )
}

async fn create_token(State(state): State<AppState>, req: Request) -> Response {
    let (parts, body) = req.into_parts();
    collapse(
        async {
            let body = validation::read_body(&parts.headers, body).await.map_err(e)?;
            gate(&state).await?;
            let w = writer(&state, &parts, None, false).await?;
            let b: TokenBody = validation::validate(&body).map_err(e)?;
            let v = tokens::create(&state, &w.user, &b.name).await.map_err(e)?;
            Ok(with_rl(w.rate_limit, StatusCode::CREATED, v))
        }
        .await,
    )
}

async fn revoke_token(State(state): State<AppState>, req: Request) -> Response {
    let (parts, _) = req.into_parts();
    collapse(
        async {
            gate(&state).await?;
            let w = writer(&state, &parts, None, false).await?;
            tokens::revoke(&state, &w.user, &seg(&parts.uri, 6)).await.map_err(e)?;
            Ok(StatusCode::NO_CONTENT.into_response())
        }
        .await,
    )
}

// --- /site/mods wrappers ----------------------------------------------------

async fn site_project(state: &AppState, parts: &mut Parts) -> Res<(Project, Option<SiteUser>)> {
    let viewer = site_viewer(state, parts).await;
    Ok((site_load(state, parts, viewer.as_ref()).await?, viewer))
}

async fn site_load(state: &AppState, parts: &Parts, viewer: Option<&SiteUser>) -> Res<Project> {
    let p = service::get_for_view(state, &seg(&parts.uri, 4), &seg(&parts.uri, 5), viewer).await.map_err(e)?;
    service::ensure_source_visible(state, &p, viewer).map_err(e)?;
    Ok(p)
}

async fn site_branches(State(state): State<AppState>, req: Request) -> Response {
    let (mut parts, _) = req.into_parts();
    collapse(
        async {
            site_gate(&state).await?;
            let (p, _) = site_project(&state, &mut parts).await?;
            let items = service::list_branches(&state, &p).await.map_err(e)?;
            Ok(no_cache(PyJson(json!({"items": items})).into_response()))
        }
        .await,
    )
}

async fn site_commits(State(state): State<AppState>, req: Request) -> Response {
    let (mut parts, _) = req.into_parts();
    collapse(
        async {
            site_gate(&state).await?;
            let viewer = site_viewer(&state, &mut parts).await;
            let q = commits_query(&parts)?;
            let p = site_load(&state, &parts, viewer.as_ref()).await?;
            let v = service::list_commits(&state, &p, q.branch.as_deref(), q.limit, q.offset).await.map_err(e)?;
            Ok(no_cache(PyJson(v).into_response()))
        }
        .await,
    )
}

async fn site_tree(State(state): State<AppState>, req: Request) -> Response {
    let (mut parts, _) = req.into_parts();
    collapse(
        async {
            site_gate(&state).await?;
            let viewer = site_viewer(&state, &mut parts).await;
            let r = ref_query(&parts)?;
            let p = site_load(&state, &parts, viewer.as_ref()).await?;
            let v = service::get_tree(&state, &p, &r).await.map_err(e)?;
            Ok(no_cache(PyJson(v).into_response()))
        }
        .await,
    )
}

async fn site_placement(State(state): State<AppState>, req: Request) -> Response {
    let (mut parts, _) = req.into_parts();
    collapse(
        async {
            site_gate(&state).await?;
            let viewer = site_viewer(&state, &mut parts).await;
            let r = ref_query(&parts)?;
            let p = site_load(&state, &parts, viewer.as_ref()).await?;
            let v = service::placement_report(&state, &p, &r).await.map_err(e)?;
            Ok(no_cache(PyJson(v).into_response()))
        }
        .await,
    )
}

async fn site_raw(State(state): State<AppState>, req: Request) -> Response {
    let (mut parts, _) = req.into_parts();
    collapse(
        async {
            site_gate(&state).await?;
            let (p, _) = site_project(&state, &mut parts).await?;
            let (r, path) = raw_path(&parts.uri, 7);
            let data = service::get_file_bytes(&state, &p, &r, &path).await.map_err(e)?;
            Ok(no_cache(octet(data)))
        }
        .await,
    )
}

async fn site_archive(State(state): State<AppState>, req: Request) -> Response {
    let (mut parts, _) = req.into_parts();
    collapse(
        async {
            site_gate(&state).await?;
            let viewer = site_viewer(&state, &mut parts).await;
            let r = ref_query(&parts)?;
            let p = site_load(&state, &parts, viewer.as_ref()).await?;
            let (name, data) = service::source_archive(&state, &p, &r).await.map_err(e)?;
            Ok(no_cache(zip_response(&name, data)?))
        }
        .await,
    )
}

// --- /git/mods smart HTTP ----------------------------------------------------

const CHALLENGE: &str = "Basic realm=\"Kiwi Mods Hub git\"";

/// CPython 3.13.17's `binascii.a2b_base64(data, strict_mode=False)`
/// (`base64.b64decode`): `=` never ends the input, only completes the last
/// quad. `None` is its `binascii.Error`.
pub fn a2b_base64(data: &[u8]) -> Option<Vec<u8>> {
    fn value(c: u8) -> Option<u8> {
        match c {
            b'A'..=b'Z' => Some(c - b'A'),
            b'a'..=b'z' => Some(c - b'a' + 26),
            b'0'..=b'9' => Some(c - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let mut out = Vec::with_capacity(data.len() / 4 * 3 + 3);
    let (mut quad, mut left, mut pads) = (0u8, 0u8, 0usize);
    for &c in data {
        if c == b'=' {
            if quad >= 2 {
                pads += 1;
            }
            continue;
        }
        let Some(v) = value(c) else { continue };
        pads = 0;
        match quad {
            0 => {
                quad = 1;
                left = v;
            }
            1 => {
                quad = 2;
                out.push((left << 2) | (v >> 4));
                left = v & 0x0f;
            }
            2 => {
                quad = 3;
                out.push((left << 4) | (v >> 2));
                left = v & 0x03;
            }
            _ => {
                quad = 0;
                out.push((left << 6) | v);
                left = 0;
            }
        }
    }
    (quad == 0 || quad as usize + pads >= 4).then_some(out)
}

/// `_basic_token`: the password of an HTTP Basic header.
pub fn basic_token(headers: &HeaderMap) -> Option<String> {
    let value = crate::request_id::latin1(headers.get(header::AUTHORIZATION)?.as_bytes());
    let b64 = value.strip_prefix("Basic ")?;
    if !b64.is_ascii() {
        return None;
    }
    let raw = a2b_base64(b64.as_bytes())?;
    let text = String::from_utf8_lossy(&raw).into_owned();
    let pw = text.split_once(':').map(|(_, p)| p.to_owned()).unwrap_or_default();
    (!pw.is_empty()).then_some(pw)
}

fn challenge(message: &str) -> Response {
    let mut r = Rejection::not_authenticated(message).into_response();
    r.headers_mut().insert(header::WWW_AUTHENTICATE, HeaderValue::from_static(CHALLENGE));
    r
}

/// What `_authorize` decides once the project and the token's user are known.
pub fn authorize_decision(
    project: Option<&Project>,
    user: Option<&SiteUser>,
    push: bool,
    source_visible: bool,
) -> Result<(), Response> {
    let Some(p) = project else {
        return Err(match user {
            None => challenge("Authentication required"),
            Some(_) => e(ApiError::not_found("Repository not found")),
        });
    };
    if push {
        let Some(u) = user else { return Err(challenge("A git access token is required to push")) };
        if p.owner_id() != Some(u.id) {
            return Err(e(ApiError::forbidden("You can only push to your own mods")));
        }
    } else if !source_visible {
        return Err(match user {
            None => challenge("Authentication required"),
            Some(_) => e(ApiError::not_found("Repository not found")),
        });
    }
    Ok(())
}

async fn authorize(state: &AppState, parts: &Parts, push: bool) -> Res<Project> {
    let handle = seg(&parts.uri, 3);
    let slug_seg = seg(&parts.uri, 4);
    let slug = slug_seg.strip_suffix(".git").unwrap_or(&slug_seg);
    let project = service::get_project(state, &handle, slug).await.map_err(e)?;
    let user = match basic_token(&parts.headers) {
        Some(t) => tokens::authenticate(state, &t).await.map_err(e)?,
        None => None,
    };
    let visible = project.as_ref().is_some_and(|p| service::source_visible(state, p, user.as_ref()));
    authorize_decision(project.as_ref(), user.as_ref(), push, visible)?;
    project.ok_or_else(|| e(ApiError::internal()))
}

fn git_reply(body: Body, content_type: &str) -> Response {
    let mut r = Response::new(body);
    let h = r.headers_mut();
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache, max-age=0, must-revalidate"));
    h.insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
    if let Ok(v) = HeaderValue::from_str(content_type) {
        h.insert(header::CONTENT_TYPE, v);
    }
    r
}

async fn ensure_repo(state: &AppState, p: &Project) -> Res<std::path::PathBuf> {
    let dir = service::repo_dir(state, p);
    let d = dir.clone();
    service::blocking(move || odb::Repo::ensure(&d).map(|_| ())).await.map_err(e)?;
    Ok(dir)
}

fn gzipped(headers: &HeaderMap) -> bool {
    headers.get(header::CONTENT_ENCODING).is_some_and(|v| {
        let v = v.as_bytes();
        v.eq_ignore_ascii_case(b"gzip") || v.eq_ignore_ascii_case(b"x-gzip")
    })
}

async fn git_info_refs(State(state): State<AppState>, req: Request) -> Response {
    let (parts, _) = req.into_parts();
    collapse(
        async {
            gate(&state).await?;
            let mut q = params(&parts);
            let svc = q.query("service").str().required();
            let svc = finish(q, svc)?;
            let Some(svc) = Service::from_name(&svc) else { return Err(e(ApiError::forbidden("Unsupported service"))) };
            let p = authorize(&state, &parts, svc == Service::ReceivePack).await?;
            let dir = ensure_repo(&state, &p).await?;
            let body = smart::advertise(svc, &dir).await.map_err(|x| e(internal(x)))?;
            Ok(git_reply(Body::from(body), &format!("application/x-{}-advertisement", svc.name())))
        }
        .await,
    )
}

async fn git_upload_pack(State(state): State<AppState>, req: Request) -> Response {
    let (parts, body) = req.into_parts();
    collapse(
        async {
            gate(&state).await?;
            let p = authorize(&state, &parts, false).await?;
            let dir = ensure_repo(&state, &p).await?;
            let out = smart::upload_pack(&dir, body, gzipped(&parts.headers)).await.map_err(|x| e(internal(x)))?;
            Ok(git_reply(out, "application/x-git-upload-pack-result"))
        }
        .await,
    )
}

async fn git_receive_pack(State(state): State<AppState>, req: Request) -> Response {
    let (parts, body) = req.into_parts();
    collapse(
        async {
            gate(&state).await?;
            let mut p = authorize(&state, &parts, true).await?;
            let dir = ensure_repo(&state, &p).await?;
            let out = smart::receive_pack(&dir, body, gzipped(&parts.headers)).await.map_err(|x| e(internal(x)))?;
            service::touch(&state, &mut p).await.map_err(e)?;
            Ok(git_reply(Body::from(out), "application/x-git-receive-pack-result"))
        }
        .await,
    )
}

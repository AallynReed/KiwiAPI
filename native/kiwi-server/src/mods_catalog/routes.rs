//! Handlers. Extractors run in FastAPI's order: the feature gate, then the
//! auth dependency, then path/query, with JSON bodies read first and
//! validated last (`DepsThenBody`) and forms parsed before anything.

use axum::{
    extract::{FromRequestParts, Request, State},
    http::{HeaderValue, StatusCode, header, request::Parts},
    response::{IntoResponse, Response},
};
use kiwi_core::form::{self, FormError, FormValue};
use serde_json::{Value, json};

use super::{
    bodies::{
        ClaimBody, CollaboratorBody, CreateProject, Lookup, RemixClaimBody, RemixStatus, UpdateProfile, UpdateProject,
    },
    model::Project,
    service::{self, ListArgs},
    stats,
    write_auth::WriteUser,
    writes,
};
use crate::{
    auth::{AccessContext, OptionalSiteUser, Rejection, extract::public_scope, models::SiteUser, scopes},
    error::{self, ApiError},
    features::{self, ModsHub, RequireFeature},
    pyjson::{PyJson, PydanticJson},
    ratelimit::RateLimitInfo,
    state::AppState,
    validation::{DepsThenBody, Fail, FromParams, Params, Query, ValidationError, Validator},
};

type Gate = RequireFeature<ModsHub>;

/// `public_scope("mods:read", rate_multiplier=settings.mods_rate_limit_multiplier, bucket="mods:browse")`.
pub struct Browse(pub AccessContext);

impl FromRequestParts<AppState> for Browse {
    type Rejection = Rejection;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Rejection> {
        let m = state.settings().mods_rate_limit_multiplier;
        Ok(Browse(public_scope(state, parts, scopes::MODS_READ, m, Some("mods:browse")).await?))
    }
}

/// The site router's `_resolve_feature_flags` for `/site/mods/*`.
pub struct SiteGate;

impl FromRequestParts<AppState> for SiteGate {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Response> {
        let mut on = true;
        for flag in crate::activity::SITE_FLAGS {
            let v = features::is_enabled(state, flag).await.map_err(|e| {
                tracing::error!("site feature flag {flag}: {e:#}");
                ApiError::internal().into_response()
            })?;
            if flag == features::MODS_HUB_FLAG {
                on = v;
            }
        }
        if !on {
            return Err(error::http_exception(
                StatusCode::NOT_FOUND,
                None,
                &parts.method,
                parts.uri.path(),
                &parts.headers,
            ));
        }
        Ok(SiteGate)
    }
}

/// Starlette-decoded path parameters.
pub struct P(Vec<(String, String)>);

impl P {
    fn get(&self, k: &str) -> String {
        self.0.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone()).unwrap_or_default()
    }
}

impl<S: Send + Sync> FromRequestParts<S> for P {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        Ok(P(crate::admin::path_params(parts)))
    }
}

pub struct ListQ(ListArgs);

impl FromParams for ListQ {
    fn from_params(p: &mut Params<'_>) -> Self {
        ListQ(ListArgs {
            q: p.query("q").str().max_len(120).optional(),
            tag: p.query("tag").str().max_len(40).optional(),
            author: p.query("author").str().max_len(80).optional(),
            sort: p.query("sort").str().default("recent".into()),
            limit: p.query("limit").int().ge(1).le(100).default(30),
            offset: p.query("offset").int().ge(0).default(0),
        })
    }
}

pub struct PopularQ(i64);

impl FromParams for PopularQ {
    fn from_params(p: &mut Params<'_>) -> Self {
        PopularQ(p.query("limit").int().ge(1).le(25).default(25))
    }
}

pub struct StatsQ(i64);

impl FromParams for StatsQ {
    fn from_params(p: &mut Params<'_>) -> Self {
        StatsQ(p.query("days").int().ge(7).le(90).default(30))
    }
}

fn v1(rl: Option<RateLimitInfo>, r: Result<Value, ApiError>) -> Response {
    v1_status(StatusCode::OK, rl, r)
}

fn v1_status(status: StatusCode, rl: Option<RateLimitInfo>, r: Result<Value, ApiError>) -> Response {
    match r {
        Ok(v) => (status, rl, PydanticJson(v)).into_response(),
        Err(e) => e.into_response(),
    }
}

fn site(cache: &'static str, r: Result<Value, ApiError>) -> Response {
    match r {
        Ok(v) => ([(header::CACHE_CONTROL, HeaderValue::from_static(cache))], PyJson(v)).into_response(),
        Err(e) => e.into_response(),
    }
}

fn items(v: Vec<Value>) -> Value {
    json!({"items": v})
}

async fn hub_page(s: &AppState, a: &ListArgs) -> Result<Value, ApiError> {
    let (items, total) = service::list_public(s, a).await?;
    let count = items.len();
    Ok(json!({"items": items, "count": count, "total": total}))
}

/// `_require_owned(handle, slug, user)`: editors only, else a uniform 404.
async fn require_owned(state: &AppState, p: &P, user: &SiteUser) -> Result<Project, ApiError> {
    match service::get_project(state, &p.get("handle"), &p.get("slug")).await? {
        Some(proj) if service::can_edit(&proj, Some(user)) => Ok(proj),
        _ => Err(ApiError::not_found("Mod project not found")),
    }
}

async fn for_view(state: &AppState, p: &P, viewer: Option<&SiteUser>) -> Result<Project, ApiError> {
    service::get_for_view(state, &p.get("handle"), &p.get("slug"), viewer).await
}

// --- public hub reads ----------------------------------------------------------------

pub async fn hub_list(
    State(s): State<AppState>,
    _: Gate,
    Browse(ctx): Browse,
    Query(ListQ(a)): Query<ListQ>,
) -> Response {
    v1(Some(ctx.rate_limit), hub_page(&s, &a).await)
}

pub async fn hub_tags(State(s): State<AppState>, _: Gate, Browse(ctx): Browse) -> Response {
    v1(Some(ctx.rate_limit), service::tag_facets(&s).await)
}

pub async fn hub_profile(State(s): State<AppState>, _: Gate, Browse(ctx): Browse, p: P) -> Response {
    let r = service::profile_view(&s, &p.get("handle"), None)
        .await
        .and_then(|v| v.ok_or_else(|| ApiError::not_found("No such modder")));
    v1(Some(ctx.rate_limit), r)
}

pub async fn hub_project(State(s): State<AppState>, _: Gate, Browse(ctx): Browse, p: P) -> Response {
    let r = async { service::project_detail(&s, &for_view(&s, &p, None).await?, None).await }.await;
    v1(Some(ctx.rate_limit), r)
}

pub async fn hub_releases(State(s): State<AppState>, _: Gate, Browse(ctx): Browse, p: P) -> Response {
    let r = async { Ok(items(service::list_releases(&s, &for_view(&s, &p, None).await?, false, false).await?)) }.await;
    v1(Some(ctx.rate_limit), r)
}

pub async fn hub_forks(State(s): State<AppState>, _: Gate, Browse(ctx): Browse, p: P) -> Response {
    let r = async { Ok(items(service::list_forks(&s, &for_view(&s, &p, None).await?).await?)) }.await;
    v1(Some(ctx.rate_limit), r)
}

// --- the creator's own reads ----------------------------------------------------------

pub async fn me_projects(State(s): State<AppState>, _: Gate, w: WriteUser) -> Response {
    v1(w.rate_limit, service::list_owned(&s, &w.user).await.map(items))
}

pub async fn me_project(State(s): State<AppState>, _: Gate, w: WriteUser, p: P) -> Response {
    let r = async { service::project_detail(&s, &require_owned(&s, &p, &w.user).await?, Some(&w.user)).await }.await;
    v1(w.rate_limit, r)
}

pub async fn me_releases(State(s): State<AppState>, _: Gate, w: WriteUser, p: P) -> Response {
    let r = async { Ok(items(service::list_releases(&s, &require_owned(&s, &p, &w.user).await?, true, true).await?)) }
        .await;
    v1(w.rate_limit, r)
}

pub async fn me_starred(State(s): State<AppState>, _: Gate, w: WriteUser) -> Response {
    v1(w.rate_limit, service::list_starred(&s, &w.user).await.map(items))
}

pub async fn me_stats(
    State(s): State<AppState>,
    _: Gate,
    w: WriteUser,
    Query(StatsQ(days)): Query<StatsQ>,
) -> Response {
    let r = async {
        let mods = service::list_owned(&s, &w.user).await?;
        let report = stats::paths_report(&s, &stats::mod_paths(&mods), days).await?;
        Ok(stats::fold(&mods, &report))
    }
    .await;
    v1(w.rate_limit, r)
}

// --- writes --------------------------------------------------------------------------

pub async fn update_profile(
    State(s): State<AppState>,
    DepsThenBody((_, w), body): DepsThenBody<(Gate, WriteUser), UpdateProfile>,
) -> Response {
    v1(w.rate_limit, writes::update_profile(&s, &w.user, body).await)
}

pub async fn create_project(
    State(s): State<AppState>,
    DepsThenBody((_, w), body): DepsThenBody<(Gate, WriteUser), CreateProject>,
) -> Response {
    let r = async {
        let p = writes::create_project(&s, &w.user, body).await?;
        service::project_detail(&s, &p, Some(&w.user)).await
    }
    .await;
    v1_status(StatusCode::CREATED, w.rate_limit, r)
}

pub async fn fork_project(State(s): State<AppState>, _: Gate, w: WriteUser, p: P) -> Response {
    let r = async {
        let mut original = for_view(&s, &p, Some(&w.user)).await?;
        let fork = writes::fork_project(&s, &w.user, &mut original).await?;
        service::project_detail(&s, &fork, Some(&w.user)).await
    }
    .await;
    v1_status(StatusCode::CREATED, w.rate_limit, r)
}

pub async fn update_project(
    State(s): State<AppState>,
    p: P,
    DepsThenBody((_, w), body): DepsThenBody<(Gate, WriteUser), UpdateProject>,
) -> Response {
    let r = async {
        let proj = require_owned(&s, &p, &w.user).await?;
        let proj = writes::update_project(&s, proj, &w.user, body).await?;
        service::project_detail(&s, &proj, Some(&w.user)).await
    }
    .await;
    v1(w.rate_limit, r)
}

pub async fn delete_project(State(s): State<AppState>, _: Gate, w: WriteUser, p: P) -> Response {
    let r = async { writes::delete_project(&s, &require_owned(&s, &p, &w.user).await?, &w.user).await }.await;
    match r {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => e.into_response(),
    }
}

pub async fn list_remixes(State(s): State<AppState>, _: Gate, w: WriteUser, p: P) -> Response {
    let r = async {
        let proj = require_owned(&s, &p, &w.user).await?;
        Ok(items(writes::list_remixes_for_owner(&s, &proj, &w.user).await?))
    }
    .await;
    v1(w.rate_limit, r)
}

pub async fn set_remix_status(
    State(s): State<AppState>,
    p: P,
    DepsThenBody((_, w), body): DepsThenBody<(Gate, WriteUser), RemixStatus>,
) -> Response {
    let r = async {
        let proj = require_owned(&s, &p, &w.user).await?;
        writes::set_remix_status(&s, &proj, &w.user, &p.get("remix_id"), &body.status).await
    }
    .await;
    v1(w.rate_limit, r)
}

pub async fn remix_claim(
    State(s): State<AppState>,
    p: P,
    DepsThenBody((_, w), body): DepsThenBody<(Gate, WriteUser), RemixClaimBody>,
) -> Response {
    let r = async {
        let original = require_owned(&s, &p, &w.user).await?;
        let target = match service::get_project(&s, &body.target_handle, &body.target_slug).await? {
            Some(t) if service::can_view(&s, &t, Some(&w.user)) => t,
            _ => return Err(ApiError::not_found("That mod could not be found.")),
        };
        writes::create_remix_claim(&s, &w.user, &original, &target, &body.message).await
    }
    .await;
    v1(w.rate_limit, r)
}

pub async fn delete_banner(State(s): State<AppState>, _: Gate, w: WriteUser, p: P) -> Response {
    let r = async {
        let mut proj = require_owned(&s, &p, &w.user).await?;
        writes::clear_banner(&s, &mut proj, &w.user).await?;
        Ok(json!({"banner_sha": proj.banner_sha}))
    }
    .await;
    v1(w.rate_limit, r)
}

pub async fn delete_preview(State(s): State<AppState>, _: Gate, w: WriteUser, p: P) -> Response {
    let r = async {
        let mut proj = require_owned(&s, &p, &w.user).await?;
        writes::remove_preview(&s, &mut proj, &w.user, &p.get("sha")).await?;
        Ok(json!({"preview_shas": proj.preview_shas}))
    }
    .await;
    v1(w.rate_limit, r)
}

pub async fn star(State(s): State<AppState>, _: Gate, w: WriteUser, p: P) -> Response {
    let r = async { writes::star_project(&s, &w.user, &mut for_view(&s, &p, Some(&w.user)).await?).await }.await;
    v1(w.rate_limit, r)
}

pub async fn unstar(State(s): State<AppState>, _: Gate, w: WriteUser, p: P) -> Response {
    let r = async { writes::unstar_project(&s, &w.user, &mut for_view(&s, &p, Some(&w.user)).await?).await }.await;
    v1(w.rate_limit, r)
}

pub async fn add_collaborator(
    State(s): State<AppState>,
    p: P,
    DepsThenBody((_, w), body): DepsThenBody<(Gate, WriteUser), CollaboratorBody>,
) -> Response {
    let r = async {
        let proj = require_owned(&s, &p, &w.user).await?;
        writes::add_collaborator(&s, proj, &w.user, &body.username).await
    }
    .await;
    v1(w.rate_limit, r)
}

pub async fn remove_collaborator(State(s): State<AppState>, _: Gate, w: WriteUser, p: P) -> Response {
    let r = async {
        let proj = require_owned(&s, &p, &w.user).await?;
        writes::remove_collaborator(&s, proj, &w.user, &p.get("user_id")).await
    }
    .await;
    v1(w.rate_limit, r)
}

pub async fn claim(
    State(s): State<AppState>,
    p: P,
    DepsThenBody((_, w), body): DepsThenBody<(Gate, WriteUser), ClaimBody>,
) -> Response {
    let r =
        async { writes::create_claim(&s, &for_view(&s, &p, Some(&w.user)).await?, &w.user, &body.message).await }.await;
    v1_status(StatusCode::ACCEPTED, w.rate_limit, r)
}

// --- uploads (multipart) ---------------------------------------------------------------

fn parse_error(msg: &str) -> Response {
    ApiError::http(StatusCode::BAD_REQUEST, Some(msg)).into_response()
}

/// `request.form()`, then the gate and the write user, as FastAPI orders it.
async fn form_then_deps(
    state: &AppState,
    req: Request,
) -> Result<(Parts, Vec<(String, FormValue)>, WriteUser), Response> {
    let (mut parts, body) = req.into_parts();
    let Ok(bytes) = axum::body::to_bytes(body, usize::MAX).await else {
        return Err(parse_error("There was an error parsing the body"));
    };
    let items = match form::parse_form(parts.headers.get(header::CONTENT_TYPE).map(|v| v.as_bytes()), &bytes) {
        Ok(items) => items,
        Err(FormError::Http(msg)) => return Err(parse_error(&msg)),
    };
    drop(bytes);
    features::check::<ModsHub>(state).await.map_err(IntoResponse::into_response)?;
    let w = WriteUser::from_request_parts(&mut parts, state).await.map_err(IntoResponse::into_response)?;
    Ok((parts, items, w))
}

/// `files: list[UploadFile] = File(...)`.
pub fn upload_files(items: Vec<(String, FormValue)>) -> Result<Vec<Vec<u8>>, ValidationError> {
    let mut v = Validator::default();
    let values: Vec<FormValue> = items.into_iter().filter(|(k, _)| k == "files").map(|(_, v)| v).collect();
    if values.is_empty() {
        v.push(vec!["body".into(), "files".into()], Value::Null, Fail::missing());
        return v.finish(Vec::new());
    }
    let mut out = Vec::new();
    for (i, item) in values.into_iter().enumerate() {
        match item {
            FormValue::File(f) => out.push(f.data),
            FormValue::Text(t) => v.push(
                vec!["body".into(), "files".into(), i.into()],
                Value::String(t),
                Fail::value_error("Expected UploadFile, received: <class 'str'>"),
            ),
        }
    }
    v.finish(out)
}

async fn profile_image(state: AppState, req: Request, banner: bool) -> Response {
    let (_, items, w) = match form_then_deps(&state, req).await {
        Ok(x) => x,
        Err(r) => return r,
    };
    let data = match crate::images::upload_file(items) {
        Ok(d) => d,
        Err(e) => return e.into_response(),
    };
    let r = async {
        let sha = writes::store_image(&state, &w.user, data).await?;
        writes::set_profile_image(&state, &w.user, &sha, banner).await
    }
    .await;
    v1(w.rate_limit, r)
}

pub async fn upload_avatar(State(s): State<AppState>, req: Request) -> Response {
    profile_image(s, req, false).await
}

pub async fn upload_profile_banner(State(s): State<AppState>, req: Request) -> Response {
    profile_image(s, req, true).await
}

pub async fn upload_banner(State(s): State<AppState>, req: Request) -> Response {
    let (parts, items, w) = match form_then_deps(&s, req).await {
        Ok(x) => x,
        Err(r) => return r,
    };
    let data = match crate::images::upload_file(items) {
        Ok(d) => d,
        Err(e) => return e.into_response(),
    };
    let p = P(crate::admin::path_params(&parts));
    let r = async {
        let mut proj = require_owned(&s, &p, &w.user).await?;
        let sha = writes::store_image(&s, &w.user, data).await?;
        writes::set_banner(&s, &mut proj, &w.user, &sha).await?;
        Ok(json!({"banner_sha": proj.banner_sha}))
    }
    .await;
    v1(w.rate_limit, r)
}

pub async fn upload_previews(State(s): State<AppState>, req: Request) -> Response {
    let (parts, items, w) = match form_then_deps(&s, req).await {
        Ok(x) => x,
        Err(r) => return r,
    };
    let files = match upload_files(items) {
        Ok(f) => f,
        Err(e) => return e.into_response(),
    };
    let p = P(crate::admin::path_params(&parts));
    let r = async {
        let mut proj = require_owned(&s, &p, &w.user).await?;
        for data in files {
            let sha = writes::store_image(&s, &w.user, data).await?;
            writes::add_preview(&s, &mut proj, &w.user, &sha).await?;
        }
        Ok(json!({"preview_shas": proj.preview_shas}))
    }
    .await;
    v1(w.rate_limit, r)
}

// --- public catalog API (/v1/mods) ---------------------------------------------------

pub async fn public_list(
    State(s): State<AppState>,
    _: Gate,
    Browse(ctx): Browse,
    Query(ListQ(a)): Query<ListQ>,
) -> Response {
    let r = async {
        let (items, total) = service::public_list(&s, &a).await?;
        let count = items.len();
        Ok(json!({"items": items, "count": count, "total": total, "limit": a.limit, "offset": a.offset}))
    }
    .await;
    v1(Some(ctx.rate_limit), r)
}

pub async fn public_popular(
    State(s): State<AppState>,
    _: Gate,
    Browse(ctx): Browse,
    Query(PopularQ(limit)): Query<PopularQ>,
) -> Response {
    let r = service::public_popular(&s, limit).await.map(|items| {
        let count = items.len();
        json!({"items": items, "count": count})
    });
    v1(Some(ctx.rate_limit), r)
}

pub async fn lookup(
    State(s): State<AppState>,
    DepsThenBody((_, Browse(ctx)), body): DepsThenBody<(Gate, Browse), Lookup>,
) -> Response {
    v1(Some(ctx.rate_limit), service::lookup_by_hashes(&s, &body.hashes, body.mode()).await)
}

pub async fn categories(_: Gate, Browse(ctx): Browse) -> Response {
    let cats: Vec<Value> = kiwi_core::mod_categories::NAMES
        .iter()
        .enumerate()
        .map(|(i, n)| json!({"name": n, "bit": 1u64 << i}))
        .collect();
    v1(Some(ctx.rate_limit), Ok(json!({"categories": cats})))
}

pub async fn public_detail(State(s): State<AppState>, _: Gate, Browse(ctx): Browse, p: P) -> Response {
    v1(Some(ctx.rate_limit), service::public_detail(&s, &p.get("handle"), &p.get("slug")).await)
}

// --- /site/mods proxies --------------------------------------------------------------

pub async fn site_projects(State(s): State<AppState>, _: SiteGate, Query(ListQ(a)): Query<ListQ>) -> Response {
    site("public, max-age=30", hub_page(&s, &a).await)
}

pub async fn site_tags(State(s): State<AppState>, _: SiteGate) -> Response {
    site("public, max-age=60", service::tag_facets(&s).await)
}

pub async fn site_profile(
    State(s): State<AppState>,
    _: SiteGate,
    OptionalSiteUser(viewer): OptionalSiteUser,
    p: P,
) -> Response {
    let r = service::profile_view(&s, &p.get("handle"), viewer.as_ref())
        .await
        .and_then(|v| v.ok_or_else(|| ApiError::http(StatusCode::NOT_FOUND, Some("No such modder."))));
    site("no-cache", r)
}

pub async fn site_modders(State(s): State<AppState>, _: SiteGate) -> Response {
    site("public, max-age=300", service::public_modder_handles(&s).await.map(items))
}

pub async fn site_my_projects(
    State(s): State<AppState>,
    _: SiteGate,
    OptionalSiteUser(viewer): OptionalSiteUser,
) -> Response {
    let Some(viewer) = viewer else {
        return ApiError::http(StatusCode::UNAUTHORIZED, Some("Sign in to view your mods.")).into_response();
    };
    site("no-cache", service::list_owned(&s, &viewer).await.map(items))
}

pub async fn site_project(
    State(s): State<AppState>,
    _: SiteGate,
    OptionalSiteUser(viewer): OptionalSiteUser,
    p: P,
) -> Response {
    let r = async {
        let proj = for_view(&s, &p, viewer.as_ref()).await?;
        service::project_detail(&s, &proj, viewer.as_ref()).await
    }
    .await;
    site("no-cache", r)
}

pub async fn site_releases(
    State(s): State<AppState>,
    _: SiteGate,
    OptionalSiteUser(viewer): OptionalSiteUser,
    p: P,
) -> Response {
    let r = async {
        let proj = for_view(&s, &p, viewer.as_ref()).await?;
        let owner = viewer.as_ref().is_some_and(|v| proj.owner_id == Some(v.id));
        Ok(items(service::list_releases(&s, &proj, owner, owner).await?))
    }
    .await;
    site("no-cache", r)
}

pub async fn site_forks(
    State(s): State<AppState>,
    _: SiteGate,
    OptionalSiteUser(viewer): OptionalSiteUser,
    p: P,
) -> Response {
    let r = async { Ok(items(service::list_forks(&s, &for_view(&s, &p, viewer.as_ref()).await?).await?)) }.await;
    site("public, max-age=30", r)
}

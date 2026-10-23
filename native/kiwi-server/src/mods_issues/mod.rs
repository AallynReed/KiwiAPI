//! Issues and requests on a mod: public reads and session writes under
//! `/v1/mods/hub/projects/{handle}/{slug}/issues`, the notification bell at
//! `/v1/mods/hub/me/issue-notifications`, and the viewer-aware `/site/mods`
//! reads. Gates run in FastAPI's order: Mods Hub, mod issues, then auth.

pub mod service;

#[cfg(test)]
mod tests;

use axum::{
    Router,
    extract::{FromRequest, FromRequestParts, Request, State},
    http::{HeaderValue, StatusCode, header, request::Parts},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
};
use serde_json::{Value, json};

use crate::{
    admin::{Args, path_params},
    auth::{AccessContext, OptionalSiteUser, Rejection, extract::public_scope, models::SiteUser, scopes},
    error::{ApiError, method_not_allowed},
    features::{self, ModIssues, ModsHub, RequireFeature},
    mods_write_auth::{ModWriteAuth, project, site::SiteGate, store::Store},
    pyjson::{PyJson, PydanticJson},
    ratelimit::RateLimitInfo,
    state::AppState,
    validation::{BodyInput, Fail, Field, FromBody, FromParams, Input, Params, Validator, read_body, str_as_int},
};

const BASE: &str = "/v1/mods/hub/projects/{handle}/{slug}/issues";
const SITE_BASE: &str = "/site/mods/projects/{handle}/{slug}/issues";

pub fn router() -> Router<AppState> {
    Router::new()
        .route(BASE, get(v1_list).post(create).fallback(|| async { method_not_allowed("GET") }))
        .route(
            &format!("{BASE}/{{number}}"),
            get(v1_get).patch(set_status).delete(delete_issue).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(&format!("{BASE}/{{number}}/comments"), post(comment).fallback(|| async { method_not_allowed("POST") }))
        .route(
            &format!("{BASE}/{{number}}/comments/{{event_id}}"),
            delete(delete_comment).fallback(|| async { method_not_allowed("DELETE") }),
        )
        .route(
            "/v1/mods/hub/me/issue-notifications",
            get(my_notifications).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/v1/mods/hub/me/issue-notifications/seen",
            post(mark_seen).fallback(|| async { method_not_allowed("POST") }),
        )
        .route(SITE_BASE, get(site_list).fallback(|| async { method_not_allowed("GET") }))
        .route(&format!("{SITE_BASE}/{{number}}"), get(site_get).fallback(|| async { method_not_allowed("GET") }))
        .route("/site/mods/notifications", get(site_notifications).fallback(|| async { method_not_allowed("GET") }))
}

/// `public_scope("mods:read", rate_multiplier=mods_rate_limit_multiplier, bucket="mods:browse")`.
pub struct ModsBrowse(pub AccessContext);

impl FromRequestParts<AppState> for ModsBrowse {
    type Rejection = Rejection;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Rejection> {
        let m = state.settings().mods_rate_limit_multiplier;
        Ok(ModsBrowse(public_scope(state, parts, scopes::MODS_READ, m, Some("mods:browse")).await?))
    }
}

/// The site's `_issues_on` dependency (its own message, not the `/v1` gate's).
pub struct SiteIssuesOn;

pub const SITE_ISSUES_OFF: &str = "Mod issues are currently disabled.";

impl FromRequestParts<AppState> for SiteIssuesOn {
    type Rejection = ApiError;

    async fn from_request_parts(_: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        match features::is_enabled(state, features::MOD_ISSUES_FLAG).await {
            Ok(true) => Ok(SiteIssuesOn),
            Ok(false) => Err(ApiError::not_found(SITE_ISSUES_OFF)),
            Err(e) => {
                tracing::error!("mod issues flag: {e:#}");
                Err(ApiError::internal())
            }
        }
    }
}

/// A Python `int` from a path/query string; `None` past `i64`, which pydantic
/// takes and the Mongo query then fails on (a 500 in Python).
fn big_int(input: &Input<'_>) -> Result<Option<i64>, Fail> {
    Ok(i64::try_from(str_as_int(text(input)?)?).ok())
}

fn text<'a>(input: &'a Input<'_>) -> Result<&'a str, Fail> {
    match input {
        Input::Str(s) => Ok(s.as_ref()),
        _ => Err(Fail::int_type()),
    }
}

/// `offset: int = Query(0, ge=0)`.
fn offset(input: &Input<'_>) -> Result<Option<i64>, Fail> {
    let v = str_as_int(text(input)?)?;
    if v < 0 {
        return Err(Fail::greater_than_equal(0.into()));
    }
    Ok(i64::try_from(v).ok())
}

/// `handle, slug` (plain strings), and `number: int` / `event_id` where the route has them.
pub struct IssuePath {
    pub handle: String,
    pub slug: String,
    pub number: Option<i64>,
    pub event_id: String,
}

impl FromParams for IssuePath {
    fn from_params(p: &mut Params<'_>) -> Self {
        let handle = p.path("handle").str().required();
        let slug = p.path("slug").str().required();
        let number = if p.path("number").is_missing() { Some(0) } else { p.path("number").custom(big_int).required() };
        let event_id =
            if p.path("event_id").is_missing() { String::new() } else { p.path("event_id").str().required() };
        IssuePath { handle, slug, number, event_id }
    }
}

/// The issue list's path and query.
pub struct ListArgs {
    pub path: IssuePath,
    pub status: String,
    pub limit: i64,
    pub offset: Option<i64>,
}

impl FromParams for ListArgs {
    fn from_params(p: &mut Params<'_>) -> Self {
        let path = IssuePath::from_params(p);
        ListArgs {
            path,
            status: p.query("status").str().one_of_pattern(&["open", "closed", "all"]).default("open".into()),
            limit: p.query("limit").int().ge(1).le(100).default(30),
            offset: p.query("offset").custom(offset).default(Some(0)),
        }
    }
}

/// `CreateIssueRequest`.
pub struct CreateBody {
    pub kind: String,
    pub title: String,
    pub body: String,
}

impl FromBody for CreateBody {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| {
            Some(CreateBody {
                kind: m.field("kind").literal(&["issue", "request"]).default("issue".into()),
                title: m.field("title").str().min_len(1).max_len(140).required(),
                body: m.field("body").str().max_len(8000).default(String::new()),
            })
        })
        .required()
    }
}

/// `IssueCommentRequest`.
pub struct CommentBody {
    pub body: String,
}

impl FromBody for CommentBody {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| Some(CommentBody { body: m.field("body").str().min_len(1).max_len(8000).required() })).required()
    }
}

/// `IssueStatusRequest`.
pub struct StatusBody {
    pub status: String,
    pub comment: String,
}

impl FromBody for StatusBody {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| {
            Some(StatusBody {
                status: m.field("status").literal(&["open", "closed"]).required(),
                comment: m.field("comment").str().max_len(8000).default(String::new()),
            })
        })
        .required()
    }
}

pub fn with_cache(v: Value, cache: &'static str) -> Response {
    let mut resp = PyJson(v).into_response();
    resp.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    resp
}

/// A dict reply, with the auth's `X-RateLimit-*` when it set any.
fn dict(status: StatusCode, v: Value, rl: Option<RateLimitInfo>) -> Response {
    match rl {
        Some(info) => (status, info, PydanticJson(v)).into_response(),
        None => (status, PydanticJson(v)).into_response(),
    }
}

fn number(path: &IssuePath) -> Result<i64, ApiError> {
    path.number.ok_or_else(|| {
        tracing::error!("issue number past int64");
        ApiError::internal()
    })
}

type Gates = (RequireFeature<ModsHub>, RequireFeature<ModIssues>, ModWriteAuth);

/// Gates, the write caller, then the path, as FastAPI orders them.
pub struct WriteCall {
    pub auth: ModWriteAuth,
    pub path: IssuePath,
}

impl FromRequestParts<AppState> for WriteCall {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Response> {
        let (_, _, auth) = Gates::from_request_parts(parts, state).await.map_err(IntoResponse::into_response)?;
        let Args(path) =
            Args::<IssuePath>::from_request_parts(parts, state).await.map_err(IntoResponse::into_response)?;
        Ok(WriteCall { auth, path })
    }
}

/// [`WriteCall`] plus a JSON body: a syntax error first, then the gates and
/// auth, then path and body errors reported together.
pub struct WriteBody<B> {
    pub auth: ModWriteAuth,
    pub path: IssuePath,
    pub body: B,
}

impl<B: FromBody + Send> FromRequest<AppState> for WriteBody<B> {
    type Rejection = Response;

    async fn from_request(req: Request, state: &AppState) -> Result<Self, Response> {
        let (mut parts, body) = req.into_parts();
        let body = read_body(&parts.headers, body).await.map_err(IntoResponse::into_response)?;
        let (_, _, auth) = Gates::from_request_parts(&mut parts, state).await.map_err(IntoResponse::into_response)?;
        let query = parts.uri.query().map(str::to_owned);
        let (path, body) = validate_write::<B>(path_params(&parts), query.as_deref(), &parts.headers, &body)
            .map_err(IntoResponse::into_response)?;
        Ok(WriteBody { auth, path, body })
    }
}

/// Path then body validation, every error in one 422 (FastAPI's order).
pub fn validate_write<B: FromBody>(
    path: Vec<(String, String)>,
    query: Option<&str>,
    headers: &axum::http::HeaderMap,
    body: &BodyInput,
) -> Result<(IssuePath, B), ApiError> {
    let mut p = Params::new(path, query, headers);
    let issue = IssuePath::from_params(&mut p);
    let mut v: Validator = p.v;
    let value = B::from_body(Field::new(&mut v, vec!["body".into()], body.input()));
    match v.finish(value)? {
        Some(b) => Ok((issue, b)),
        None => Err(ApiError::internal()),
    }
}

/// Path and query validation for the read routes.
pub fn validate_params<T: FromParams>(
    path: Vec<(String, String)>,
    query: Option<&str>,
    headers: &axum::http::HeaderMap,
) -> Result<T, ApiError> {
    let mut p = Params::new(path, query, headers);
    let value = T::from_params(&mut p);
    Ok(p.v.finish(value)?)
}

async fn v1_list(
    State(state): State<AppState>,
    _: RequireFeature<ModsHub>,
    _: RequireFeature<ModIssues>,
    ModsBrowse(ctx): ModsBrowse,
    Args(q): Args<ListArgs>,
) -> Response {
    match list_op(&Store::live(&state), None, &q).await {
        Ok(v) => (ctx.rate_limit, PydanticJson(v)).into_response(),
        Err(e) => e.into_response(),
    }
}

async fn v1_get(
    State(state): State<AppState>,
    _: RequireFeature<ModsHub>,
    _: RequireFeature<ModIssues>,
    ModsBrowse(ctx): ModsBrowse,
    Args(q): Args<IssuePath>,
) -> Response {
    match get_op(&Store::live(&state), None, &q).await {
        Ok(v) => (ctx.rate_limit, PydanticJson(v)).into_response(),
        Err(e) => e.into_response(),
    }
}

/// A page of a mod's issues for `viewer`.
pub async fn list_op(store: &Store, viewer: Option<&SiteUser>, q: &ListArgs) -> Result<Value, ApiError> {
    let project = project::get_for_view(store, &q.path.handle, &q.path.slug, viewer).await?;
    service::ensure_issues_open(&project)?;
    let offset = q.offset.ok_or_else(|| {
        tracing::error!("issue offset past int64");
        ApiError::internal()
    })?;
    service::list_issues(store, &project, viewer, &q.status, q.limit, offset as u64).await
}

/// One thread with its timeline, for `viewer`.
pub async fn get_op(store: &Store, viewer: Option<&SiteUser>, q: &IssuePath) -> Result<Value, ApiError> {
    let project = project::get_for_view(store, &q.handle, &q.slug, viewer).await?;
    service::ensure_issues_open(&project)?;
    service::get_issue(store, &project, number(q)?, viewer).await
}

pub async fn create_op(store: &Store, auth: &ModWriteAuth, path: &IssuePath, req: &CreateBody) -> Response {
    let out = async {
        let project = project::get_for_view(store, &path.handle, &path.slug, Some(&auth.user)).await?;
        service::create_issue(store, &project, &auth.user, &req.kind, &req.title, &req.body).await
    };
    match out.await {
        Ok(v) => dict(StatusCode::CREATED, v, auth.rate_limit),
        Err(e) => e.into_response(),
    }
}

async fn create(State(state): State<AppState>, call: WriteBody<CreateBody>) -> Response {
    create_op(&Store::live(&state), &call.auth, &call.path, &call.body).await
}

pub async fn comment_op(store: &Store, auth: &ModWriteAuth, path: &IssuePath, req: &CommentBody) -> Response {
    let out = async {
        let project = project::get_for_view(store, &path.handle, &path.slug, Some(&auth.user)).await?;
        service::ensure_issues_open(&project)?;
        service::add_comment(store, &project, number(path)?, &auth.user, &req.body).await
    };
    match out.await {
        Ok(v) => dict(StatusCode::CREATED, v, auth.rate_limit),
        Err(e) => e.into_response(),
    }
}

async fn comment(State(state): State<AppState>, call: WriteBody<CommentBody>) -> Response {
    comment_op(&Store::live(&state), &call.auth, &call.path, &call.body).await
}

pub async fn status_op(store: &Store, auth: &ModWriteAuth, path: &IssuePath, req: &StatusBody) -> Response {
    let out = async {
        let project = project::get_for_view(store, &path.handle, &path.slug, Some(&auth.user)).await?;
        service::ensure_issues_open(&project)?;
        if !matches!(req.status.as_str(), "open" | "closed") {
            return Err(ApiError::bad_request("status must be 'open' or 'closed'.").into());
        }
        service::set_status(store, &project, number(path)?, &auth.user, &req.status, &req.comment).await
    };
    match out.await {
        Ok(v) => dict(StatusCode::OK, v, auth.rate_limit),
        Err(e) => e.into_response(),
    }
}

async fn set_status(State(state): State<AppState>, call: WriteBody<StatusBody>) -> Response {
    status_op(&Store::live(&state), &call.auth, &call.path, &call.body).await
}

pub async fn delete_issue_op(store: &Store, auth: &ModWriteAuth, path: &IssuePath) -> Response {
    let out = async {
        let project = project::get_for_view(store, &path.handle, &path.slug, Some(&auth.user)).await?;
        service::delete_issue(store, &project, number(path)?, &auth.user).await
    };
    match out.await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => e.into_response(),
    }
}

async fn delete_issue(State(state): State<AppState>, call: WriteCall) -> Response {
    delete_issue_op(&Store::live(&state), &call.auth, &call.path).await
}

pub async fn delete_comment_op(store: &Store, auth: &ModWriteAuth, path: &IssuePath) -> Response {
    let out = async {
        let project = project::get_for_view(store, &path.handle, &path.slug, Some(&auth.user)).await?;
        service::delete_comment(store, &project, number(path)?, &path.event_id, &auth.user).await
    };
    match out.await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => e.into_response(),
    }
}

async fn delete_comment(State(state): State<AppState>, call: WriteCall) -> Response {
    delete_comment_op(&Store::live(&state), &call.auth, &call.path).await
}

pub async fn notifications_op(store: &Store, auth: &ModWriteAuth) -> Response {
    match service::notifications(store, &auth.user).await {
        Ok(v) => dict(StatusCode::OK, v, auth.rate_limit),
        Err(e) => e.into_response(),
    }
}

async fn my_notifications(
    State(state): State<AppState>,
    _: RequireFeature<ModsHub>,
    _: RequireFeature<ModIssues>,
    auth: ModWriteAuth,
) -> Response {
    notifications_op(&Store::live(&state), &auth).await
}

pub async fn mark_seen_op(store: &Store, auth: &ModWriteAuth) -> Response {
    match service::mark_seen(store, &auth.user).await {
        Ok(v) => dict(StatusCode::OK, v, auth.rate_limit),
        Err(e) => e.into_response(),
    }
}

async fn mark_seen(
    State(state): State<AppState>,
    _: RequireFeature<ModsHub>,
    _: RequireFeature<ModIssues>,
    auth: ModWriteAuth,
) -> Response {
    mark_seen_op(&Store::live(&state), &auth).await
}

async fn site_list(
    State(state): State<AppState>,
    _: SiteGate,
    _: SiteIssuesOn,
    OptionalSiteUser(viewer): OptionalSiteUser,
    Args(q): Args<ListArgs>,
) -> Response {
    match list_op(&Store::live(&state), viewer.as_ref(), &q).await {
        Ok(v) => with_cache(v, "no-cache"),
        Err(e) => e.into_response(),
    }
}

async fn site_get(
    State(state): State<AppState>,
    _: SiteGate,
    _: SiteIssuesOn,
    OptionalSiteUser(viewer): OptionalSiteUser,
    Args(q): Args<IssuePath>,
) -> Response {
    match get_op(&Store::live(&state), viewer.as_ref(), &q).await {
        Ok(v) => with_cache(v, "no-cache"),
        Err(e) => e.into_response(),
    }
}

pub async fn site_notifications_op(store: &Store, viewer: Option<&SiteUser>) -> Response {
    let Some(viewer) = viewer else {
        return with_cache(json!({"items": [], "unread": 0, "seen_at": null}), "no-store");
    };
    match service::notifications(store, viewer).await {
        Ok(v) => with_cache(v, "no-store"),
        Err(e) => e.into_response(),
    }
}

async fn site_notifications(
    State(state): State<AppState>,
    _: SiteGate,
    _: SiteIssuesOn,
    OptionalSiteUser(viewer): OptionalSiteUser,
) -> Response {
    site_notifications_op(&Store::live(&state), viewer.as_ref()).await
}

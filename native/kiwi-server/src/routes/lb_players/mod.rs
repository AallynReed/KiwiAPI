//! Player reads of the leaderboards: `/v1/leaderboards/players/{name}/{history,
//! profile,boards,series}` (`app/trove/router.py`) and the `/site` copies of
//! history, profile and series (`app/site/router.py`).

pub mod render;
pub mod service;
pub mod store;
#[cfg(test)]
mod store_tests;
#[cfg(test)]
mod tests;

use axum::{
    Router,
    extract::{FromRequestParts, State},
    http::{HeaderName, HeaderValue, StatusCode, Uri, header, request::Parts},
    response::{IntoResponse, Response},
    routing::get,
};

use crate::{
    auth::{Public, Rejection, RequireScope, scopes::LEADERBOARDS_READ},
    error::{ApiError, ApiResult},
    features::{self, LEADERBOARDS_FLAG, Leaderboards, RequireFeature},
    gamedata::Gamedata,
    middleware::pyurl::scope_path,
    pyjson,
    ratelimit::{check_rate_limit, get_rate_limit},
    state::AppState,
    validation::{Fail, FromParams, Input, Params, Query, str_as_int},
};
use store::{Live, Store};

const V1: &str = "/v1/leaderboards/players/";
const SITE: &str = "/site/leaderboards/players/";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/leaderboards/players/{player_name}/history", get(v1_history))
        .route("/v1/leaderboards/players/{player_name}/profile", get(v1_profile))
        .route("/v1/leaderboards/players/{player_name}/boards", get(v1_boards))
        .route("/v1/leaderboards/players/{player_name}/series", get(v1_series))
        .route("/site/leaderboards/players/{player_name}/history", get(site_history))
        .route("/site/leaderboards/players/{player_name}/profile", get(site_profile))
        .route("/site/leaderboards/players/{player_name}/series", get(site_series))
}

/// The `{player_name}` Starlette matched: cut from the percent-decoded path.
fn player_name(uri: &Uri, prefix: &str, suffix: &str) -> String {
    let path = scope_path(uri);
    path.strip_prefix(prefix).and_then(|p| p.strip_suffix(suffix)).unwrap_or_default().to_owned()
}

/// `_resolve_feature_flags` for these paths: the leaderboards toggle, a bare
/// `HTTPException(404)` while it is off.
pub struct SiteLeaderboards;

impl FromRequestParts<AppState> for SiteLeaderboards {
    type Rejection = ApiError;

    async fn from_request_parts(_parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        match features::is_enabled(state, LEADERBOARDS_FLAG).await {
            Ok(true) => Ok(SiteLeaderboards),
            Ok(false) => Err(ApiError::http(StatusCode::NOT_FOUND, None)),
            Err(e) => Err(store::internal(e)),
        }
    }
}

/// `int | None = Query(default=None)`: pydantic accepts any size of int.
fn opt_big_int(f: crate::validation::Field<'_>) -> Option<i128> {
    f.custom(|i| match i {
        Input::Str(s) => str_as_int(s),
        _ => Err(Fail::int_type()),
    })
    .optional()
}

pub struct V1HistoryQuery {
    pub uuid: Option<i128>,
    pub limit: i64,
}

impl FromParams for V1HistoryQuery {
    fn from_params(p: &mut Params<'_>) -> Self {
        let uuid = opt_big_int(p.query("uuid"));
        V1HistoryQuery { uuid, limit: p.query("limit").int().ge(1).le(500).default(50) }
    }
}

pub struct SiteHistoryQuery {
    pub limit: i64,
    pub uuid: Option<i128>,
}

impl FromParams for SiteHistoryQuery {
    fn from_params(p: &mut Params<'_>) -> Self {
        let limit = p.query("limit").int().ge(1).le(500).default(50);
        SiteHistoryQuery { limit, uuid: opt_big_int(p.query("uuid")) }
    }
}

pub struct DaysQuery {
    pub days: i64,
}

impl FromParams for DaysQuery {
    fn from_params(p: &mut Params<'_>) -> Self {
        DaysQuery { days: p.query("days").int().ge(1).le(30).default(7) }
    }
}

fn gamedata(state: &AppState) -> &'static Gamedata {
    Gamedata::global(state.settings())
}

fn site_json(body: String) -> Response {
    let mut res = pyjson::json_response(body);
    res.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=30"));
    res
}

type Scoped = RequireScope<{ LEADERBOARDS_READ }>;

/// What each route answers once its dependencies passed.
pub(crate) async fn v1_history_body<S: Store>(s: &S, name: &str, q: &V1HistoryQuery) -> ApiResult<String> {
    let window_start = service::hot_window_start(s).await;
    let rows = service::player_history(s, name, q.limit, q.uuid, false, Some(window_start)).await?;
    Ok(render::v1_history(name, &rows))
}

pub(crate) async fn v1_profile_body<S: Store>(s: &S, gd: &Gamedata, name: &str) -> ApiResult<String> {
    render::v1_profile(&service::player_profile(s, gd, name, false).await?)
}

pub(crate) async fn v1_boards_body<S: Store>(s: &S, name: &str) -> ApiResult<String> {
    Ok(render::v1_boards(&service::player_boards(s, name).await?))
}

/// `days = min(days, hot_window_days())`, before the archive charge.
pub(crate) async fn v1_series_days<S: Store>(s: &S, q: &DaysQuery) -> i64 {
    q.days.min(service::hot_window_days(s).await)
}

pub(crate) async fn v1_series_body<S: Store>(s: &S, name: &str, days: i64) -> ApiResult<String> {
    Ok(render::v1_series(&service::player_history_series(s, name, days).await?))
}

pub(crate) async fn site_history_body<S: Store>(s: &S, name: &str, q: &SiteHistoryQuery) -> ApiResult<String> {
    let window_start = service::now_secs(s) - 7 * 86_400;
    let rows = service::player_history(s, name, q.limit, q.uuid, true, Some(window_start)).await?;
    Ok(render::site_history(name, &rows))
}

pub(crate) async fn site_profile_body<S: Store>(s: &S, gd: &Gamedata, name: &str) -> ApiResult<String> {
    Ok(render::site_profile(&service::player_profile(s, gd, name, true).await?))
}

pub(crate) async fn site_series_body<S: Store>(s: &S, name: &str, q: &DaysQuery) -> ApiResult<String> {
    Ok(render::site_series(&service::player_history_series(s, name, q.days).await?))
}

async fn v1_history(
    State(state): State<AppState>,
    _f: RequireFeature<Leaderboards>,
    RequireScope(ctx): Scoped,
    uri: Uri,
    Query(q): Query<V1HistoryQuery>,
) -> ApiResult<Response> {
    let body = v1_history_body(&Live(&state), &player_name(&uri, V1, "/history"), &q).await?;
    Ok((ctx.rate_limit, pyjson::json_response(body)).into_response())
}

async fn v1_profile(
    State(state): State<AppState>,
    _f: RequireFeature<Leaderboards>,
    Public(ctx): Public<{ LEADERBOARDS_READ }>,
    uri: Uri,
) -> ApiResult<Response> {
    let body = v1_profile_body(&Live(&state), gamedata(&state), &player_name(&uri, V1, "/profile")).await?;
    Ok((ctx.rate_limit, pyjson::json_response(body)).into_response())
}

async fn v1_boards(
    State(state): State<AppState>,
    _f: RequireFeature<Leaderboards>,
    Public(ctx): Public<{ LEADERBOARDS_READ }>,
    uri: Uri,
) -> ApiResult<Response> {
    let body = v1_boards_body(&Live(&state), &player_name(&uri, V1, "/boards")).await?;
    Ok((ctx.rate_limit, pyjson::json_response(body)).into_response())
}

/// `_enforce_lb_archive_limit`: the archive bucket for an old window, its
/// state as `X-RateLimit-Archive-*`.
async fn archive_limit(
    state: &AppState,
    s: &Live<'_>,
    ctx: &crate::auth::TokenContext,
    anchor: i64,
) -> Result<Vec<(HeaderName, HeaderValue)>, Response> {
    if !service::is_archive_query(s, anchor).await.map_err(IntoResponse::into_response)? {
        return Ok(Vec::new());
    }
    let (max, window) = get_rate_limit(state, "leaderboards_archive_rate_limit").await;
    let info = check_rate_limit(state, &format!("lb_archive:{}", ctx.token.id), max, window)
        .await
        .map_err(|r: Rejection| r.into_response())?;
    Ok(info
        .headers()
        .into_iter()
        .filter_map(|(name, value)| {
            let archive = name.as_str().replacen("x-ratelimit-", "x-ratelimit-archive-", 1);
            Some((HeaderName::from_bytes(archive.as_bytes()).ok()?, value))
        })
        .collect())
}

async fn v1_series(
    State(state): State<AppState>,
    _f: RequireFeature<Leaderboards>,
    RequireScope(ctx): Scoped,
    uri: Uri,
    Query(q): Query<DaysQuery>,
) -> Response {
    let s = Live(&state);
    let days = v1_series_days(&s, &q).await;
    let archive = match archive_limit(&state, &s, &ctx, service::now_secs(&s) - days * 86_400).await {
        Ok(h) => h,
        Err(r) => return r,
    };
    match v1_series_body(&s, &player_name(&uri, V1, "/series"), days).await {
        Ok(body) => {
            let mut res = (ctx.rate_limit, pyjson::json_response(body)).into_response();
            res.headers_mut().extend(archive);
            res
        }
        Err(e) => e.into_response(),
    }
}

async fn site_history(
    State(state): State<AppState>,
    _g: SiteLeaderboards,
    uri: Uri,
    Query(q): Query<SiteHistoryQuery>,
) -> ApiResult<Response> {
    Ok(site_json(site_history_body(&Live(&state), &player_name(&uri, SITE, "/history"), &q).await?))
}

async fn site_profile(State(state): State<AppState>, _g: SiteLeaderboards, uri: Uri) -> ApiResult<Response> {
    let name = player_name(&uri, SITE, "/profile");
    Ok(site_json(site_profile_body(&Live(&state), gamedata(&state), &name).await?))
}

async fn site_series(
    State(state): State<AppState>,
    _g: SiteLeaderboards,
    uri: Uri,
    Query(q): Query<DaysQuery>,
) -> ApiResult<Response> {
    Ok(site_json(site_series_body(&Live(&state), &player_name(&uri, SITE, "/series"), &q).await?))
}

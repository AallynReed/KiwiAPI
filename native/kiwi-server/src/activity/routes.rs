//! The route layer: `activity_router` / `class_activity_router` in
//! `app/trove/router.py` and the `/site/leaderboards/{activity,class-activity}`
//! proxies in `app/site/router.py`.

use axum::{
    extract::{FromRequestParts, State},
    http::{HeaderValue, StatusCode, header, request::Parts},
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};

use super::{class, models, player};
use crate::{
    auth::{AccessContext, MasterIngest, Public, pyutil::int_of, scopes::ACTIVITY_READ},
    error::{self, ApiError},
    features::{self, ClassActivity, PlayerActivity, RequireFeature},
    pyjson::{PyJson, PydanticJson},
    runtime_config,
    state::AppState,
    validation::{FromParams, Params, Query},
};

fn internal(e: anyhow::Error) -> ApiError {
    tracing::error!("activity: {e:#}");
    ApiError::internal()
}

/// `int(get_setting("cheaters_cache_ttl_seconds"))` as the `/v1` Cache-Control.
async fn v1_cache_control(state: &AppState) -> Result<HeaderValue, ApiError> {
    let v = runtime_config::get_setting(state, "cheaters_cache_ttl_seconds").await.map_err(internal)?;
    let ttl = int_of(&v).ok_or_else(|| internal(anyhow::anyhow!("cheaters_cache_ttl_seconds is {v}")))?;
    HeaderValue::from_str(&format!("public, max-age={ttl}")).map_err(|e| internal(e.into()))
}

/// A `/v1` read: the cache header, then `Model(**payload)` dumped.
async fn v1_reply(
    state: &AppState,
    ctx: AccessContext,
    payload: anyhow::Result<Value>,
    model: fn(&Value) -> Option<Value>,
) -> Result<Response, ApiError> {
    let payload = payload.map_err(internal)?;
    let cc = v1_cache_control(state).await?;
    let body = model(&payload).ok_or_else(|| internal(anyhow::anyhow!("response model rejected {payload}")))?;
    Ok((ctx.rate_limit, [(header::CACHE_CONTROL, cc)], PydanticJson(body)).into_response())
}

fn site_reply(payload: anyhow::Result<Value>) -> Result<Response, ApiError> {
    let payload = payload.map_err(internal)?;
    Ok(([(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"))], PyJson(payload)).into_response())
}

pub struct HistoryQuery {
    pub days: i64,
}

impl FromParams for HistoryQuery {
    fn from_params(p: &mut Params<'_>) -> Self {
        HistoryQuery { days: p.query("days").int().ge(1).le(30).default(7) }
    }
}

/// `days: int = 7` (the `/site` proxy clamps instead of bounding).
pub struct SiteHistoryQuery {
    pub days: i64,
}

impl FromParams for SiteHistoryQuery {
    fn from_params(p: &mut Params<'_>) -> Self {
        SiteHistoryQuery { days: p.query("days").int().saturating().default(7) }
    }
}

pub struct PeriodQuery {
    pub period: String,
}

impl FromParams for PeriodQuery {
    fn from_params(p: &mut Params<'_>) -> Self {
        PeriodQuery { period: p.query("period").str().default("7d".into()) }
    }
}

pub struct BackfillQuery {
    pub total_days: i64,
    pub chunk_days: i64,
    pub force: bool,
    pub reset: bool,
}

impl FromParams for BackfillQuery {
    fn from_params(p: &mut Params<'_>) -> Self {
        BackfillQuery {
            total_days: p.query("total_days").int().ge(0).le(1000).default(400),
            chunk_days: p.query("chunk_days").int().ge(1).le(60).default(14),
            force: p.query("force").bool().default(false),
            reset: p.query("reset").bool().default(false),
        }
    }
}

pub struct ClassBackfillQuery {
    pub total_days: i64,
    pub force: bool,
    pub reset: bool,
}

impl FromParams for ClassBackfillQuery {
    fn from_params(p: &mut Params<'_>) -> Self {
        ClassBackfillQuery {
            total_days: p.query("total_days").int().ge(0).le(1000).default(400),
            force: p.query("force").bool().default(false),
            reset: p.query("reset").bool().default(false),
        }
    }
}

pub async fn v1_current(
    _: RequireFeature<PlayerActivity>,
    Public(ctx): Public<{ ACTIVITY_READ }>,
    State(state): State<AppState>,
) -> Result<Response, ApiError> {
    let payload = player::estimate_active_players(&state, false).await;
    v1_reply(&state, ctx, payload, models::activity).await
}

pub async fn v1_history(
    _: RequireFeature<PlayerActivity>,
    Public(ctx): Public<{ ACTIVITY_READ }>,
    State(state): State<AppState>,
    Query(q): Query<HistoryQuery>,
) -> Result<Response, ApiError> {
    let payload = player::history(&state, q.days).await;
    v1_reply(&state, ctx, payload, models::history).await
}

pub async fn v1_series(
    _: RequireFeature<PlayerActivity>,
    Public(ctx): Public<{ ACTIVITY_READ }>,
    State(state): State<AppState>,
    Query(q): Query<PeriodQuery>,
) -> Result<Response, ApiError> {
    let payload = player::series(&state, &q.period).await;
    v1_reply(&state, ctx, payload, models::series).await
}

/// The 202 answer of the backfill routes (`chunk_days` only for players).
pub fn backfill_body(total_days: i64, chunk_days: Option<i64>, force: bool, reset: bool, what: &str) -> Value {
    let mut body = json!({"accepted": true, "total_days": total_days});
    if let (Some(c), Value::Object(o)) = (chunk_days, &mut body) {
        o.insert("chunk_days".into(), c.into());
    }
    if let Value::Object(o) = &mut body {
        o.insert("force".into(), (force || reset).into());
        o.insert("reset".into(), reset.into());
        let lead = if reset { "RESET + backfill" } else { "Backfill" };
        o.insert(
            "message".into(),
            format!(
                "{lead} started in the background; watch the api logs for '{what} backfill (chunked) done' with the tally."
            )
            .into(),
        );
    }
    body
}

pub async fn v1_backfill(
    _: RequireFeature<PlayerActivity>,
    MasterIngest(auth): MasterIngest,
    State(state): State<AppState>,
    Query(q): Query<BackfillQuery>,
) -> Response {
    let (total_days, force, reset) = (q.total_days, q.force, q.reset);
    tokio::spawn(async move {
        if let Err(e) = player::backfill_history_chunked(&state, total_days, force, reset).await {
            tracing::error!("activity backfill failed: {e:#}");
        }
    });
    let body = backfill_body(q.total_days, Some(q.chunk_days), q.force, q.reset, "activity");
    (StatusCode::ACCEPTED, auth.rate_limit, PydanticJson(body)).into_response()
}

pub async fn v1_class_current(
    _: RequireFeature<ClassActivity>,
    Public(ctx): Public<{ ACTIVITY_READ }>,
    State(state): State<AppState>,
) -> Result<Response, ApiError> {
    let payload = class::current(&state).await;
    v1_reply(&state, ctx, payload, models::class_current).await
}

pub async fn v1_class_series(
    _: RequireFeature<ClassActivity>,
    Public(ctx): Public<{ ACTIVITY_READ }>,
    State(state): State<AppState>,
    Query(q): Query<PeriodQuery>,
) -> Result<Response, ApiError> {
    let payload = class::series(&state, &q.period).await;
    v1_reply(&state, ctx, payload, models::class_series).await
}

pub async fn v1_class_backfill(
    _: RequireFeature<ClassActivity>,
    MasterIngest(auth): MasterIngest,
    State(state): State<AppState>,
    Query(q): Query<ClassBackfillQuery>,
) -> Response {
    let (total_days, force, reset) = (q.total_days, q.force, q.reset);
    tokio::spawn(async move {
        if let Err(e) = class::backfill_class_history_chunked(&state, total_days, force, reset).await {
            tracing::error!("class activity backfill failed: {e:#}");
        }
    });
    let body = backfill_body(q.total_days, None, q.force, q.reset, "class activity");
    (StatusCode::ACCEPTED, auth.rate_limit, PydanticJson(body)).into_response()
}

/// `feature_map.SITE_FEATURE_FLAGS`, in order: the site router resolves every
/// one of them on each request (`_resolve_feature_flags`).
pub const SITE_FLAGS: [&str; 42] = [
    features::MODS_HUB_FLAG,
    features::MARKET_FLAG,
    features::STORE_FLAG,
    features::LEADERBOARDS_FLAG,
    features::PLAYER_ACTIVITY_FLAG,
    features::CLASS_ACTIVITY_FLAG,
    features::CLUBS_FLAG,
    features::UPDATES_FLAG,
    features::CODEXES_FLAG,
    features::SERVER_STATUS_FLAG,
    features::GIVEAWAYS_FLAG,
    features::COMMANDS_FLAG,
    features::SERVER_TIME_FLAG,
    features::WEBHOOKS_FLAG,
    features::DM_SUBS_FLAG,
    features::IMAGE_STUDIO_FLAG,
    features::CALENDAR_FLAG,
    features::STREAMS_FLAG,
    features::BTT_RELEASES_FLAG,
    features::CLASSES_FLAG,
    features::STAR_CHART_FLAG,
    features::GEM_SIMULATOR_FLAG,
    features::GEM_EVALUATOR_FLAG,
    features::GEM_BUILDS_FLAG,
    features::CALCULATORS_FLAG,
    features::GEMS_GUIDE_FLAG,
    features::ABILITIES_FLAG,
    features::GUIDES_FLAG,
    features::ALLIES_FLAG,
    features::GEM_TOOLS_FLAG,
    features::FISHING_GUIDE_FLAG,
    features::GEAR_GUIDE_FLAG,
    features::LOOT_COLLECTOR_GUIDE_FLAG,
    features::DRESSING_ROOM_FLAG,
    features::DRESSING_ROOM_PAGE_FLAG,
    features::SOUND_STUDIO_FLAG,
    features::MOD_WORKSHOP_FLAG,
    features::BLUEPRINT_EDITOR_FLAG,
    features::TOMES_FLAG,
    features::UNLOCK_DEBUG_FLAG,
    features::FILE_DROPS_FLAG,
    features::COMMUNITY_SITES_FLAG,
];

/// `feature_blocks` for the paths served here.
pub fn site_blocked(path: &str, player_activity: bool, class_activity: bool) -> bool {
    (!player_activity && path.starts_with("/site/leaderboards/activity"))
        || (!class_activity && path.starts_with("/site/leaderboards/class-activity"))
}

/// The site router's `_resolve_feature_flags` dependency: a disabled feature's
/// proxy is a plain 404.
pub struct SiteGate;

impl FromRequestParts<AppState> for SiteGate {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Response> {
        let (mut player, mut class) = (true, true);
        for flag in SITE_FLAGS {
            let on = features::is_enabled(state, flag).await.map_err(|e| internal(e).into_response())?;
            if flag == features::PLAYER_ACTIVITY_FLAG {
                player = on;
            } else if flag == features::CLASS_ACTIVITY_FLAG {
                class = on;
            }
        }
        let path = parts.uri.path();
        if site_blocked(path, player, class) {
            return Err(error::http_exception(StatusCode::NOT_FOUND, None, &parts.method, path, &parts.headers));
        }
        Ok(SiteGate)
    }
}

pub async fn site_current(_: SiteGate, State(state): State<AppState>) -> Result<Response, ApiError> {
    site_reply(player::estimate_active_players(&state, false).await)
}

pub async fn site_history(
    _: SiteGate,
    State(state): State<AppState>,
    Query(q): Query<SiteHistoryQuery>,
) -> Result<Response, ApiError> {
    site_reply(player::history(&state, q.days).await)
}

pub async fn site_series(
    _: SiteGate,
    State(state): State<AppState>,
    Query(q): Query<PeriodQuery>,
) -> Result<Response, ApiError> {
    site_reply(player::series(&state, &q.period).await)
}

pub async fn site_class_current(_: SiteGate, State(state): State<AppState>) -> Result<Response, ApiError> {
    site_reply(class::current(&state).await)
}

pub async fn site_class_series(
    _: SiteGate,
    State(state): State<AppState>,
    Query(q): Query<PeriodQuery>,
) -> Result<Response, ApiError> {
    site_reply(class::series(&state, &q.period).await)
}

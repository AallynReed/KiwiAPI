//! `/v1/rotations/*` from `app/trove/router.py`: deterministic timers and the
//! bot-captured reads, tokenless under `rotations:read`, and the three
//! master-only capture inserts (`inserts`).

pub mod bodies;
pub mod chaos;
pub mod delves;
pub mod gamedata;
pub mod http;
pub mod inserts;
pub mod store;

#[cfg(test)]
mod inserts_tests;
#[cfg(test)]
mod store_tests;
#[cfg(test)]
mod tests;

use axum::{
    Router,
    extract::State,
    response::IntoResponse,
    routing::{get, post},
};
use kiwi_core::rotations as rot;
use serde_json::Value;

use crate::{
    auth::{Public, scopes},
    dt,
    error::ApiError,
    features::{self, RequireFeature},
    pyjson::PydanticJson,
    state::AppState,
    validation::{FromParams, Params, Query},
};
use gamedata::{Gamedata, Invalid};

type Rot = Public<{ scopes::ROTATIONS_READ }>;
type Reply = Result<axum::response::Response, ApiError>;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/rotations/server-time", get(server_time))
        .route("/v1/rotations/daily-buffs", get(daily_buffs))
        .route("/v1/rotations/weekly-buffs", get(weekly_buffs))
        .route("/v1/rotations/corruxion", get(corruxion))
        .route("/v1/rotations/fluxion", get(fluxion))
        .route("/v1/rotations/luxion", get(luxion))
        .route(inserts::LUXION_PATH, post(inserts::luxion_insert))
        .route("/v1/rotations/luxion/history", get(luxion_history))
        .route("/v1/rotations/gardening", get(gardening))
        .route("/v1/rotations/chaos-chest", get(chaos_chest))
        .route(inserts::CHAOS_PATH, post(inserts::chaos_insert))
        .route("/v1/rotations/chaos-chest/history", get(chaos_history))
        .route("/v1/rotations/challenge/current", get(challenge_current))
        .route(inserts::CHALLENGE_PATH, post(inserts::challenge_insert))
        .route("/v1/rotations/challenge/history", get(challenge_history))
        .route("/v1/rotations/calendar", get(calendar))
        .route("/v1/rotations/delves/weeks", get(delve_weeks))
        .route("/v1/rotations/delves", get(delve_rotation))
        .route("/v1/rotations/biomes", get(biomes))
        .route("/v1/rotations/wild-mana", get(wild_mana))
        .route("/v1/rotations/stampy", get(stampy))
}

/// The background loops this area owns (`chaos`, `delves`).
pub fn start_jobs(state: &AppState) {
    chaos::start(state);
    delves::start(state);
}

fn internal(err: impl std::fmt::Display) -> ApiError {
    tracing::error!("rotations: {err}");
    ApiError::internal()
}

impl From<Invalid> for ApiError {
    fn from(_: Invalid) -> Self {
        internal("response failed validation")
    }
}

impl From<store::StoreError> for ApiError {
    fn from(e: store::StoreError) -> Self {
        internal(e)
    }
}

fn reply(ctx: &Rot, body: Value) -> Reply {
    Ok((ctx.0.rate_limit, PydanticJson(body)).into_response())
}

fn now_us() -> i64 {
    dt::now().timestamp_micros()
}

async fn server_time(ctx: Rot) -> Reply {
    reply(&ctx, bodies::server_time(dt::now()))
}

async fn daily_buffs(ctx: Rot) -> Reply {
    reply(&ctx, bodies::daily_buffs(Gamedata::get(), now_us())?)
}

async fn weekly_buffs(ctx: Rot) -> Reply {
    reply(&ctx, bodies::weekly_buffs(Gamedata::get(), now_us())?)
}

async fn corruxion(ctx: Rot) -> Reply {
    reply(&ctx, bodies::corruxion(now_us()))
}

async fn fluxion(ctx: Rot) -> Reply {
    reply(&ctx, bodies::fluxion(now_us()))
}

async fn gardening(ctx: Rot) -> Reply {
    reply(&ctx, bodies::gardening(now_us()))
}

async fn biomes(ctx: Rot) -> Reply {
    reply(&ctx, bodies::biomes(Gamedata::get(), now_us())?)
}

async fn wild_mana(ctx: Rot) -> Reply {
    reply(&ctx, bodies::wild_mana(Gamedata::get(), now_us())?)
}

async fn stampy(ctx: Rot) -> Reply {
    reply(&ctx, bodies::stampy(Gamedata::get(), now_us())?)
}

async fn luxion(State(state): State<AppState>, ctx: Rot) -> Reply {
    let doc = store::latest_luxion(state.mongo()).await?;
    reply(&ctx, bodies::luxion(doc.as_ref(), now_us()))
}

async fn calendar(State(state): State<AppState>, ctx: Rot) -> Reply {
    let runs: Vec<rot::LuxionRun> = store::luxion_runs(state.mongo()).await?.iter().map(|d| d.run()).collect();
    reply(&ctx, bodies::calendar(Gamedata::get(), now_us(), &runs)?)
}

/// `limit: int = Query(50, ge=1, le=<max>)`, `offset: int = Query(0, ge=0)`.
struct Paging<const MAX: i64> {
    limit: i64,
    offset: i64,
}

impl<const MAX: i64> FromParams for Paging<MAX> {
    fn from_params(p: &mut Params<'_>) -> Self {
        Paging {
            limit: p.query("limit").int().ge(1).le(MAX).default(50),
            offset: p.query("offset").int().ge(0).default(0),
        }
    }
}

async fn luxion_history(State(state): State<AppState>, ctx: Rot, Query(q): Query<Paging<200>>) -> Reply {
    let (docs, total) = store::luxion_history(state.mongo(), q.limit, q.offset).await?;
    reply(&ctx, bodies::page(docs.iter().map(bodies::luxion_history_item).collect(), total))
}

async fn chaos_history(State(state): State<AppState>, ctx: Rot, Query(q): Query<Paging<200>>) -> Reply {
    let (docs, total) = store::chaos_history(state.mongo(), q.limit, q.offset).await?;
    let items = docs.iter().map(|d| bodies::chaos_history_item(d.week_anchor, &d.name, &d.captured_at)).collect();
    reply(&ctx, bodies::page(items, total))
}

async fn challenge_history(State(state): State<AppState>, ctx: Rot, Query(q): Query<Paging<500>>) -> Reply {
    let (docs, total) = store::challenge_history(state.mongo(), q.limit, q.offset).await?;
    let items = docs.iter().map(bodies::challenge_history_item).collect::<Result<Vec<_>, _>>()?;
    reply(&ctx, bodies::page(items, total))
}

async fn challenge_current(State(state): State<AppState>, ctx: Rot) -> Reply {
    let now = now_us();
    let window = rot::challenge_window(now);
    let doc = store::challenge_for_window(state.mongo(), window.starts_at).await?;
    reply(&ctx, bodies::challenge_current(now, doc.as_ref().map(|d| (d.name.as_str(), d.captured_at))))
}

async fn chaos_chest(State(state): State<AppState>, ctx: Rot) -> Reply {
    let now = now_us();
    let week = rot::chaos_chest_window(now);
    let db = state.mongo();
    let chest = match store::chaos_for_week(db, week.0).await? {
        Some(capture) => chaos::build(Some(&chaos::capture_item(&capture.name, week)), Some(capture.captured_at), now)?,
        None => match store::chaos_feed(db).await? {
            Some((cached, fetched_at)) => chaos::build(cached.as_ref(), Some(fetched_at), now)?,
            None => chaos::build(None, None, now)?,
        },
    };
    let resolved = match chaos::needs_blueprint(&chest) {
        Some(name) => chaos::resolve_blueprint(&state, name).await,
        None => None,
    };
    reply(&ctx, chaos::body(&chest, resolved, &state.settings().api_url))
}

/// `week: int | None = Query(None)`.
struct WeekQuery {
    week: Option<i64>,
}

impl FromParams for WeekQuery {
    fn from_params(p: &mut Params<'_>) -> Self {
        WeekQuery { week: p.query("week").int().optional() }
    }
}

async fn delve_weeks(_on: RequireFeature<features::Delves>, State(state): State<AppState>, ctx: Rot) -> Reply {
    let rows = store::delve_weeks(state.mongo()).await?;
    reply(&ctx, bodies::delve_week_list(rot::delve_week_id(now_us()), &rows))
}

async fn delve_rotation(
    _on: RequireFeature<features::Delves>,
    State(state): State<AppState>,
    ctx: Rot,
    Query(q): Query<WeekQuery>,
) -> Reply {
    let current = rot::delve_week_id(now_us());
    let doc = store::delve_week(state.mongo(), q.week.unwrap_or(current)).await?;
    reply(&ctx, bodies::delve_rotation(q.week, current, doc.as_ref()).map_err(ApiError::not_found)?)
}

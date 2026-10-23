//! `GET /site/trove-status` and `/history` (`app/site/router.py`): the same
//! snapshot and history as `/v1`, as plain `JSONResponse`s behind the site
//! router's feature gate. Python's copy reads the in-process prober state, so
//! these must answer here once the prober runs here.

use axum::{
    extract::State,
    http::{HeaderValue, header},
    response::{IntoResponse, Response},
};
use kiwi_core::status as core;

use super::{history_value, load_segments, shared_snapshot, unix_now, unknown_snapshot};
use crate::{
    error::ApiError,
    og::SiteGate,
    pyjson::PyJson,
    state::AppState,
    validation::{FromParams, Params, Query},
};

fn reply(cache_control: &'static str, body: serde_json::Value) -> Response {
    ([(header::CACHE_CONTROL, HeaderValue::from_static(cache_control))], PyJson(body)).into_response()
}

pub async fn current(_: SiteGate, State(state): State<AppState>) -> Response {
    reply("public, max-age=30", shared_snapshot(&state).await.unwrap_or_else(unknown_snapshot))
}

/// `env: str = "live", days: int = 7`.
pub struct SiteHistoryQuery {
    pub env: String,
    pub days: i64,
}

impl FromParams for SiteHistoryQuery {
    fn from_params(p: &mut Params<'_>) -> Self {
        SiteHistoryQuery {
            env: p.query("env").str().default("live".into()),
            days: p.query("days").int().saturating().default(7),
        }
    }
}

/// The route's own `env`/`days` normalization before `get_history`.
pub fn site_env(env: &str) -> &'static str {
    core::history_env(if env == "pts" { "pts" } else { "live" })
}

pub async fn history(
    _: SiteGate,
    State(state): State<AppState>,
    Query(q): Query<SiteHistoryQuery>,
) -> Result<Response, ApiError> {
    let now = unix_now();
    let env = site_env(&q.env);
    let rows = load_segments(&state, env).await.map_err(|e| {
        tracing::error!("site trove status history: {e:#}");
        ApiError::internal()
    })?;
    Ok(reply("public, max-age=60", history_value(&core::history(env, q.days.clamp(1, 90), now, &rows))))
}

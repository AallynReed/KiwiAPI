//! `app/trove/decode/router.py` (the dev panel's Game data tab) and the web
//! container's `GET /gamedata/{name}`.

use axum::{
    Router,
    extract::{Path, State},
    http::{HeaderMap, Method, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde_json::json;

use super::{registry, runner};
use crate::{
    admin::Args,
    auth::Superuser,
    error::{ApiError, ErrorCode},
    gamedata::Gamedata,
    pyjson::PydanticJson,
    state::AppState,
    validation::{FromParams, Params},
};

pub fn router() -> Router<AppState> {
    Router::new().route("/admin/gamedata", get(status)).route("/admin/gamedata/run", post(run))
}

fn internal(e: impl std::fmt::Display) -> ApiError {
    tracing::error!("gamedata admin: {e}");
    ApiError::internal()
}

async fn status(State(state): State<AppState>, _: Superuser) -> Result<PydanticJson, ApiError> {
    runner::status(&state).await.map(PydanticJson).map_err(internal)
}

pub struct RunQuery {
    name: Option<String>,
    allow_shrink: bool,
}

impl FromParams for RunQuery {
    fn from_params(p: &mut Params<'_>) -> Self {
        RunQuery { name: p.query("name").str().optional(), allow_shrink: p.query("allow_shrink").bool().default(false) }
    }
}

async fn run(State(state): State<AppState>, _: Superuser, Args(q): Args<RunQuery>) -> Result<PydanticJson, ApiError> {
    if !runner::enabled(state.settings()) {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            ErrorCode::Conflict,
            "GAMEDATA_DIR isn't set on this server, so rebuilds are off.",
        ));
    }
    if let Some(name) = &q.name
        && registry::get(name).is_none()
    {
        return Err(ApiError::new(StatusCode::NOT_FOUND, ErrorCode::NotFound, format!("No decoder called '{name}'.")));
    }
    if runner::is_running(&state).await.map_err(internal)? {
        return Ok(PydanticJson(json!({"started": false, "message": "A rebuild is already running."})));
    }
    let names: Vec<&'static str> = match &q.name {
        Some(n) => registry::get(n).map(|d| d.name).into_iter().collect(),
        None => registry::DECODERS.iter().map(|d| d.name).collect(),
    };
    let message = format!("Rebuilding {}.", names.join(", "));
    let allow_shrink = q.allow_shrink;
    tokio::spawn(async move {
        if let Err(e) = runner::run(&state, &names, "manual", allow_shrink).await {
            tracing::error!("gamedata run failed: {e:#}");
        }
    });
    Ok(PydanticJson(json!({"started": true, "message": message})))
}

const PUBLIC: [&str; 3] = ["pvp.json", "gem_upgrades.json", "gear.json"];

/// The web container's routes (`app/web/main.py`); not mounted on the API host.
pub fn web_router() -> Router<AppState> {
    Router::new().route("/gamedata/{name}", get(gamedata_file))
}

/// `GET /gamedata/{name}`: the rebuild when there is one, else the repo copy.
/// Any other name is the web app's `HTTPException(404)`: the themed page for a
/// browser, the envelope otherwise.
pub async fn gamedata_file(
    State(state): State<AppState>,
    Path(name): Path<String>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    if !PUBLIC.contains(&name.as_str()) {
        return crate::error::http_exception(StatusCode::NOT_FOUND, None, &method, uri.path(), &headers);
    }
    let path = Gamedata::global(state.settings()).path(&name);
    match crate::drops::download::file_response_with(&path, "application/json", None, "public, max-age=300", &headers)
        .await
    {
        Ok(r) => r,
        Err(e) => e.into_response(),
    }
}

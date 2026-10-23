//! `POST /admin/codexes/rebuild` and `GET /admin/codexes/status`. The rebuild
//! status is process-wide here (Python kept one per worker).

use axum::{
    extract::State,
    response::{IntoResponse, Response},
};
use serde_json::json;

use super::{
    BRANCHES, DEFAULT_BRANCH,
    archive::{Cas, MongoArchive},
    indexer, internal, store,
};
use crate::{
    auth::Superuser,
    error::{ApiError, ErrorCode},
    pyjson::PydanticJson,
    state::AppState,
    validation::{FromParams, Params, Query},
};

pub struct BranchQ {
    branch: String,
}

impl FromParams for BranchQ {
    fn from_params(p: &mut Params<'_>) -> Self {
        BranchQ { branch: p.query("branch").str().default(DEFAULT_BRANCH.into()) }
    }
}

/// The archive one branch is indexed from.
pub fn archive(state: &AppState, branch: &str) -> MongoArchive {
    MongoArchive {
        db: state.mongo().clone(),
        branch: branch.to_owned(),
        cas: Cas { root: state.settings().trove_update_store_dir.clone().into() },
    }
}

pub async fn rebuild(State(state): State<AppState>, _: Superuser, Query(q): Query<BranchQ>) -> Response {
    let branch = q.branch;
    if !BRANCHES.contains(&branch.as_str()) {
        return ApiError::bad_request(super::branch_message(&branch)).into_response();
    }
    let Some(pool) = state.inner.db.pg.clone() else {
        return ApiError::bad_request("Postgres backend is disabled").into_response();
    };
    if indexer::rebuild_status(&branch).get("running").and_then(|v| v.as_bool()).unwrap_or(false) {
        return PydanticJson(json!({"started": false, "branch": branch, "message": "A rebuild is already running."}))
            .into_response();
    }
    let started = indexer::mark_running(&branch);
    let a = archive(&state, &branch);
    let local = state.settings().trove_local_game_dir.clone();
    let b = branch.clone();
    tokio::spawn(async move { indexer::rebuild(&pool, &a, &b, started, &local).await });
    PydanticJson(
        json!({"started": true, "branch": branch, "message": "Codex rebuild started - poll /admin/codexes/status."}),
    )
    .into_response()
}

pub async fn status(State(state): State<AppState>, _: Superuser, Query(q): Query<BranchQ>) -> Response {
    let branch = q.branch;
    if !BRANCHES.contains(&branch.as_str()) {
        return ApiError::new(
            axum::http::StatusCode::BAD_REQUEST,
            ErrorCode::NotFound,
            format!("Unknown branch '{branch}'"),
        )
        .into_response();
    }
    let (count, version) = match state.inner.db.pg.as_ref() {
        Some(pool) => {
            let count = match store::branch_count(pool, &branch).await {
                Ok(c) => c,
                Err(e) => return internal(e).into_response(),
            };
            match store::get_parser_version(pool, &branch).await {
                Ok(v) => (count, v),
                Err(e) => return internal(e).into_response(),
            }
        }
        None => (0, 0),
    };
    PydanticJson(json!({
        "branch": branch,
        "entry_count": count,
        "parser_version": version,
        "current_parser_version": indexer::CODEX_PARSER_VERSION,
        "rebuild": indexer::rebuild_status(&branch),
    }))
    .into_response()
}

//! The master-only `/v1/leaderboards` write routes of `app/trove/router.py`:
//! `POST /insert`, `POST /reset` and `GET /reingest-status`.

use std::sync::Arc;

use axum::{
    extract::{FromRequestParts, Request, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use kiwi_core::form::{self, FormError, FormValue};
use mongodb::bson::doc;
use serde_json::{Map, Value, json};

use super::{
    backlog,
    ingest::{self, ACCEPTED_MESSAGE, Submitter},
    signals, store,
};
use crate::{
    auth::{IngestAuth, MasterIngest, pyutil::bson_int},
    error::ApiError,
    features::{self, RequireFeature},
    pyjson::PydanticJson,
    state::AppState,
    validation::{Fail, FromParams, Params, Query, ValidationError},
};

/// The parameters of `insert_leaderboards`, in signature order.
#[derive(Debug)]
pub struct InsertReq {
    pub file: Vec<u8>,
    pub timestamp: Option<i64>,
    pub backfill: bool,
    pub sync: bool,
    pub warm: bool,
}

/// Query + form validation of `/insert` (the 422 FastAPI raises once every
/// dependency has run): `file: UploadFile = File(...)`.
pub fn validate_insert(
    query: Option<&str>,
    headers: &HeaderMap,
    items: Vec<(String, FormValue)>,
) -> Result<InsertReq, ValidationError> {
    let mut p = Params::new(Vec::new(), query, headers);
    let timestamp = p.query("timestamp").int().optional();
    let backfill = p.query("backfill").bool().default(false);
    let sync = p.query("sync").bool().default(false);
    let warm = p.query("warm").bool().default(true);
    let mut v = p.v;
    let loc = || vec!["body".into(), "file".into()];
    let file = match items.into_iter().rev().find(|(k, _)| k == "file").map(|(_, v)| v) {
        Some(FormValue::File(f)) => f.data,
        Some(FormValue::Text(t)) if !t.is_empty() => {
            v.push(loc(), Value::String(t), Fail::value_error("Expected UploadFile, received: <class 'str'>"));
            Vec::new()
        }
        _ => {
            v.push(loc(), Value::Null, Fail::missing());
            Vec::new()
        }
    };
    v.finish(InsertReq { file, timestamp, backfill, sync, warm })
}

/// `LeaderboardInsertResponse`.
pub fn insert_body(accepted: bool, nbytes: usize, message: &str) -> Value {
    json!({"accepted": accepted, "bytes": nbytes, "message": message})
}

pub const EMPTY_UPLOAD: &str = "Empty upload - POST the raw cfg text as a multipart 'file' field.";

fn parse_error(msg: &str) -> Response {
    ApiError::http(StatusCode::BAD_REQUEST, Some(msg)).into_response()
}

/// `POST /v1/leaderboards/insert`.
pub async fn insert(State(state): State<AppState>, req: Request) -> Response {
    let (mut parts, body) = req.into_parts();
    let Ok(bytes) = axum::body::to_bytes(body, usize::MAX).await else {
        return parse_error("There was an error parsing the body");
    };
    let items = match form::parse_form(parts.headers.get(header::CONTENT_TYPE).map(|v| v.as_bytes()), &bytes) {
        Ok(items) => items,
        Err(FormError::Http(msg)) => return parse_error(&msg),
    };
    drop(bytes);
    if let Err(e) = features::check::<features::Leaderboards>(&state).await {
        return e.into_response();
    }
    let auth = match MasterIngest::from_request_parts(&mut parts, &state).await {
        Ok(MasterIngest(a)) => a,
        Err(r) => return r.into_response(),
    };
    let q = match validate_insert(parts.uri.query(), &parts.headers, items) {
        Ok(q) => q,
        Err(e) => return e.into_response(),
    };
    if q.file.is_empty() {
        return ApiError::bad_request(EMPTY_UPLOAD).into_response();
    }
    let nbytes = q.file.len();
    let text = Arc::new(match String::from_utf8(q.file) {
        Ok(s) => s,
        Err(e) => String::from_utf8_lossy(e.as_bytes()).into_owned(),
    });
    let warm = q.warm && !q.backfill;
    let IngestAuth { user, token, rate_limit } = auth;
    let who = Submitter { user, token };
    if q.sync {
        let outcome = ingest::process_dump(&state, text, q.timestamp, &who, nbytes, q.backfill, warm).await;
        let (accepted, message) = ingest::sync_reply(outcome.as_ref());
        return (StatusCode::OK, rate_limit, PydanticJson(insert_body(accepted, nbytes, &message))).into_response();
    }
    let (state2, timestamp, backfill) = (state.clone(), q.timestamp, q.backfill);
    tokio::spawn(async move {
        ingest::process_dump(&state2, text, timestamp, &who, nbytes, backfill, warm).await;
    });
    (StatusCode::ACCEPTED, rate_limit, PydanticJson(insert_body(true, nbytes, ACCEPTED_MESSAGE))).into_response()
}

pub struct ResetQuery {
    pub drop_boards: bool,
}

impl FromParams for ResetQuery {
    fn from_params(p: &mut Params<'_>) -> Self {
        ResetQuery { drop_boards: p.query("drop_boards").bool().default(false) }
    }
}

/// `service.reset_all`'s summary.
pub fn reset_summary(t: &store::ResetTally, drop_boards: bool, redis_keys: i64) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("hot_entries_deleted".into(), t.entries.into());
    m.insert("archive_entries_deleted".into(), 0.into());
    m.insert("activity_estimates_deleted".into(), t.estimates.into());
    m.insert("boards_deleted".into(), t.boards.into());
    m.insert("dropped_boards".into(), drop_boards.into());
    m.insert("redis_keys_cleared".into(), redis_keys.into());
    m
}

/// The reset summary as the ingest log stores it.
pub fn reset_log(t: &store::ResetTally, drop_boards: bool, redis_keys: i64) -> mongodb::bson::Document {
    doc! {
        "hot_entries_deleted": bson_int(t.entries),
        "archive_entries_deleted": bson_int(0),
        "activity_estimates_deleted": bson_int(t.estimates),
        "boards_deleted": bson_int(t.boards),
        "dropped_boards": drop_boards,
        "redis_keys_cleared": bson_int(redis_keys),
    }
}

/// `POST /v1/leaderboards/reset`. The Python workers' in-process detection,
/// activity, rename and duplicate memos are theirs to drop; everything they
/// read from is wiped here.
pub async fn reset(
    _on: RequireFeature<features::Leaderboards>,
    State(state): State<AppState>,
    MasterIngest(auth): MasterIngest,
    Query(q): Query<ResetQuery>,
) -> Result<Response, ApiError> {
    let pool = store::pool(&state).map_err(internal)?;
    let tally = store::reset_all(pool, q.drop_boards).await.map_err(internal)?;
    let cleared = signals::reset_cache(&state).await;
    let summary = reset_summary(&tally, q.drop_boards, cleared);
    tracing::warn!("leaderboards: FULL RESET {summary:?}");
    let log = reset_log(&tally, q.drop_boards, cleared);
    signals::record(&state, "/v1/leaderboards/reset", &auth.user, auth.token.as_ref(), log, None).await;
    let mut body = Map::new();
    body.insert("reset".into(), true.into());
    body.extend(summary);
    Ok((auth.rate_limit, PydanticJson(Value::Object(body))).into_response())
}

/// `GET /v1/leaderboards/reingest-status`: the progress Python's re-ingest
/// publishes, plus what the backlog folder holds.
pub async fn reingest_status(
    _on: RequireFeature<features::Leaderboards>,
    State(state): State<AppState>,
    MasterIngest(auth): MasterIngest,
) -> Result<Response, ApiError> {
    let status = backlog::get_status(&state).await;
    let Value::Object(mut status) = status else {
        return Err(internal("reingest status is not an object"));
    };
    let settings = state.settings().clone();
    let info = tokio::task::spawn_blocking(move || backlog::info(&settings)).await.map_err(internal)?;
    status.extend(info);
    Ok((auth.rate_limit, PydanticJson(Value::Object(status))).into_response())
}

pub fn internal(e: impl std::fmt::Display) -> ApiError {
    tracing::error!("leaderboards ingest route: {e}");
    ApiError::internal()
}

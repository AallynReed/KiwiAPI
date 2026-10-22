//! `POST /v1/leaderboards/warm`, `/delete-anchor` and `/reingest-backlog`
//! (`app/trove/router.py`).

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use mongodb::bson::{Document, doc};
use serde_json::{Value, json};

use super::{off_executor, reingest};
use crate::{
    activity,
    auth::{IngestAuth, MasterIngest, pyutil::bson_int},
    error::ApiError,
    features::{Leaderboards, RequireFeature},
    lb_ingest::{backlog, signals, store::pool},
    lbread,
    pyjson::PydanticJson,
    state::AppState,
    validation::{FromParams, Params, Query},
};

pub const DELETE_ENDPOINT: &str = "/v1/leaderboards/delete-anchor";
pub const EMPTY_BACKLOG: &str =
    "Backlog is empty - let the bot populate it, or drop <unix>.cfg files into the backlog folder.";

fn internal(e: impl std::fmt::Display) -> Response {
    tracing::error!("leaderboards warmer route: {e}");
    ApiError::internal().into_response()
}

/// `warm_leaderboards`.
pub async fn warm(_on: RequireFeature<Leaderboards>, MasterIngest(auth): MasterIngest) -> Response {
    super::trigger();
    (StatusCode::ACCEPTED, auth.rate_limit, PydanticJson(json!({"warming": true}))).into_response()
}

pub struct DeleteQuery {
    pub anchor: i64,
    pub recompute: bool,
}

impl FromParams for DeleteQuery {
    fn from_params(p: &mut Params<'_>) -> Self {
        DeleteQuery { anchor: p.query("anchor").int().required(), recompute: p.query("recompute").bool().default(true) }
    }
}

/// What `delete_leaderboard_anchor` touches besides its reply.
#[allow(async_fn_in_trait)]
pub trait Maint: Sync {
    async fn delete_anchor(&self, anchor: i64) -> anyhow::Result<i64>;
    async fn delete_estimate(&self, anchor: i64) -> anyhow::Result<()>;
    async fn invalidate_anchor(&self, anchor: i64);
    async fn log(&self, summary: Document);
    /// `detection.reset()`.
    fn reset_detection(&self);
    fn trigger_warmer(&self);
    /// The `backfill_history_chunked(reset=True, total_days=0)` background task.
    fn backfill(&self);
}

/// `delete_leaderboard_anchor`'s work and reply.
pub async fn delete_anchor_with<M: Maint>(m: &M, anchor: i64, recompute: bool) -> anyhow::Result<Value> {
    let deleted = m.delete_anchor(anchor).await?;
    if let Err(e) = m.delete_estimate(anchor).await {
        tracing::warn!("delete-anchor: estimate purge failed for {anchor}: {e:#}");
    }
    m.invalidate_anchor(anchor).await;
    m.log(doc! {"anchor": bson_int(anchor), "entries_deleted": bson_int(deleted), "recompute": recompute}).await;
    let rerun = recompute && deleted != 0;
    if rerun {
        m.reset_detection();
        m.trigger_warmer();
        m.backfill();
    }
    Ok(json!({"deleted": deleted != 0, "anchor": anchor, "entries_deleted": deleted, "recompute": rerun}))
}

struct LiveMaint<'a> {
    state: &'a AppState,
    auth: &'a IngestAuth,
}

impl Maint for LiveMaint<'_> {
    async fn delete_anchor(&self, anchor: i64) -> anyhow::Result<i64> {
        let pool = pool(self.state).map_err(anyhow::Error::msg)?;
        super::store::delete_anchor(pool, anchor).await.map_err(anyhow::Error::msg)
    }

    async fn delete_estimate(&self, anchor: i64) -> anyhow::Result<()> {
        let pool = pool(self.state).map_err(anyhow::Error::msg)?;
        super::store::delete_estimate(pool, anchor).await.map_err(anyhow::Error::msg)
    }

    async fn invalidate_anchor(&self, anchor: i64) {
        lbread::cache::invalidate_anchor(self.state, anchor).await;
    }

    async fn log(&self, summary: Document) {
        let a = self.auth;
        signals::record(self.state, DELETE_ENDPOINT, &a.user, a.token.as_ref(), summary, None).await;
    }

    fn reset_detection(&self) {
        crate::cheaters::reset();
    }

    fn trigger_warmer(&self) {
        super::trigger();
    }

    fn backfill(&self) {
        let state = self.state.clone();
        tokio::spawn(async move {
            let run = off_executor(move || async move {
                activity::player::backfill_history_chunked(&state, 0, false, true).await
            });
            match run.await {
                Ok(Ok(_)) => {}
                Ok(Err(e)) | Err(e) => tracing::error!("delete-anchor: activity backfill failed: {e:#}"),
            }
        });
    }
}

/// `delete_leaderboard_anchor`.
pub async fn delete_anchor(
    _on: RequireFeature<Leaderboards>,
    State(state): State<AppState>,
    MasterIngest(auth): MasterIngest,
    Query(q): Query<DeleteQuery>,
) -> Response {
    let m = LiveMaint { state: &state, auth: &auth };
    match delete_anchor_with(&m, q.anchor, q.recompute).await {
        Ok(body) => (auth.rate_limit, PydanticJson(body)).into_response(),
        Err(e) => internal(format!("{e:#}")),
    }
}

pub struct ReingestQuery {
    pub clear_first: bool,
}

impl FromParams for ReingestQuery {
    fn from_params(p: &mut Params<'_>) -> Self {
        ReingestQuery { clear_first: p.query("clear_first").bool().default(false) }
    }
}

/// `reingest_backlog`'s reply for a backlog of `files`.
pub fn reingest_reply(files: usize, clear_first: bool) -> Value {
    if files == 0 {
        json!({"started": false, "files": 0, "message": EMPTY_BACKLOG})
    } else {
        json!({"started": true, "files": files, "clear_first": clear_first})
    }
}

/// `reingest_backlog`.
pub async fn reingest_backlog(
    _on: RequireFeature<Leaderboards>,
    State(state): State<AppState>,
    MasterIngest(auth): MasterIngest,
    Query(q): Query<ReingestQuery>,
) -> Response {
    let settings = state.settings().clone();
    let files = match tokio::task::spawn_blocking(move || backlog::list_files(&settings).len()).await {
        Ok(n) => n,
        Err(e) => return internal(e),
    };
    if files > 0 {
        let (state, clear_first) = (state.clone(), q.clear_first);
        tokio::spawn(async move {
            let run = off_executor(move || async move { reingest::run(&reingest::Live(state), clear_first).await });
            match run.await {
                Ok(Ok(())) => {}
                Ok(Err(e)) | Err(e) => tracing::error!("backlog re-ingest failed: {e:#}"),
            }
        });
    }
    (StatusCode::ACCEPTED, auth.rate_limit, PydanticJson(reingest_reply(files, q.clear_first))).into_response()
}

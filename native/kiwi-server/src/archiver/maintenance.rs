//! `app/trove/updates/maintenance.py` and its two `/admin/updates/*` routes:
//! recompute `update_state.last_ordinal` from the change-log, in the
//! background, with the progress kept in this process.

use std::{collections::HashMap, future::Future, sync::Mutex};

use axum::{extract::State, http::StatusCode};
use chrono::{DateTime, Utc};
use futures::{Stream, TryStreamExt};
use mongodb::bson::{Bson, Document, doc};
use serde_json::{Value, json};

use super::repo::{CHANGES, Op, STATE, bulk_write};
use crate::{
    admin::{Args, Res},
    auth::Superuser,
    dt,
    error::ApiError,
    pyjson::PydanticJson,
    state::AppState,
    validation::{FromParams, Params},
};

pub const BATCH: usize = 1000;
pub const UPDATE_BRANCHES: [&str; 2] = ["live-us", "pts"];

#[derive(Debug, Clone, Default, PartialEq)]
pub struct BackfillStatus {
    pub running: bool,
    pub updated: i64,
    pub done: bool,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub error: Option<String>,
}

impl BackfillStatus {
    pub fn to_json(&self) -> Value {
        json!({
            "running": self.running,
            "updated": self.updated,
            "done": self.done,
            "started_at": self.started_at.as_ref().map(dt::pydantic),
            "finished_at": self.finished_at.as_ref().map(dt::pydantic),
            "error": self.error,
        })
    }
}

fn statuses() -> &'static Mutex<HashMap<String, BackfillStatus>> {
    static S: std::sync::OnceLock<Mutex<HashMap<String, BackfillStatus>>> = std::sync::OnceLock::new();
    S.get_or_init(Default::default)
}

/// `get_backfill_status(branch)`.
pub fn get_backfill_status(branch: &str) -> BackfillStatus {
    statuses().lock().expect("statuses").get(branch).cloned().unwrap_or_default()
}

pub fn with_status(branch: &str, f: impl FnOnce(&mut BackfillStatus)) {
    f(statuses().lock().expect("statuses").entry(branch.to_owned()).or_default());
}

pub fn pipeline(branch: &str) -> Vec<Document> {
    vec![doc! {"$match": {"branch": branch}}, doc! {"$group": {"_id": "$path", "last": {"$max": "$ordinal"}}}]
}

/// The non-upsert `UpdateOne` for one aggregate row.
pub fn row_op(branch: &str, row: &Document) -> Op {
    Op::Update {
        filter: doc! {"branch": branch, "path": row.get("_id").cloned().unwrap_or(Bson::Null)},
        set: doc! {"last_ordinal": row.get("last").cloned().unwrap_or(Bson::Null)},
        upsert: false,
    }
}

/// The batching loop over the aggregate rows: `bulk` writes 1000 at a time
/// and returns `modified_count`.
pub async fn apply_rows<S, F, Fut>(branch: &str, mut rows: S, mut bulk: F) -> anyhow::Result<i64>
where
    S: Stream<Item = anyhow::Result<Document>> + Unpin,
    F: FnMut(Vec<Op>) -> Fut,
    Fut: Future<Output = anyhow::Result<i64>>,
{
    let mut updated = 0;
    let mut ops = Vec::new();
    while let Some(row) = rows.try_next().await? {
        ops.push(row_op(branch, &row));
        if ops.len() >= BATCH {
            updated += bulk(std::mem::take(&mut ops)).await?;
            with_status(branch, |s| s.updated = updated);
            tokio::task::yield_now().await;
        }
    }
    if !ops.is_empty() {
        updated += bulk(ops).await?;
    }
    Ok(updated)
}

async fn run(state: &AppState, branch: &str) -> anyhow::Result<i64> {
    let db = state.inner.db.mongo.as_ref().ok_or_else(|| anyhow::anyhow!("MONGO_URI not configured"))?;
    let cursor = db.collection::<Document>(CHANGES).aggregate(pipeline(branch)).allow_disk_use(true).await?;
    apply_rows(branch, cursor.map_err(anyhow::Error::from), |ops| async move { Ok(bulk_write(db, STATE, &ops).await?) })
        .await
}

/// The status bookkeeping around one backfill run.
pub async fn tracked<F: Future<Output = anyhow::Result<i64>>>(branch: &str, work: F) {
    with_status(branch, |s| {
        *s = BackfillStatus { running: true, started_at: Some(dt::now()), ..Default::default() };
    });
    match work.await {
        Ok(updated) => {
            with_status(branch, |s| {
                s.updated = updated;
                s.done = true;
            });
            tracing::info!("updates[{branch}]: last_ordinal backfill updated {updated} files");
        }
        Err(e) => {
            let msg = e.to_string();
            with_status(branch, |s| s.error = Some(kiwi_core::pystr::prefix(&msg, 500).to_owned()));
            tracing::error!("updates[{branch}]: last_ordinal backfill failed: {e:#}");
        }
    }
    with_status(branch, |s| {
        s.running = false;
        s.finished_at = Some(dt::now());
    });
}

/// `backfill_last_ordinal(branch)`.
pub async fn backfill_last_ordinal(state: AppState, branch: String) {
    tracked(&branch, run(&state, &branch)).await;
}

/// `branch: str = Query(default="live-us")`.
pub struct BranchQuery(pub String);

impl FromParams for BranchQuery {
    fn from_params(p: &mut Params<'_>) -> Self {
        BranchQuery(p.query("branch").str().default("live-us".to_owned()))
    }
}

/// The POST's reply, and whether a backfill is to be started.
pub fn start_reply(branch: &str) -> Result<(Value, bool), ApiError> {
    if !UPDATE_BRANCHES.contains(&branch) {
        return Err(ApiError::bad_request(format!(
            "Unknown branch '{branch}' (known: {})",
            UPDATE_BRANCHES.join(", ")
        )));
    }
    if get_backfill_status(branch).running {
        return Ok((json!({"started": false, "branch": branch, "message": "A backfill is already running."}), false));
    }
    let body = json!({"started": true, "branch": branch,
                      "message": "Backfill started - poll /admin/updates/backfill-modified/status."});
    Ok((body, true))
}

pub fn status_reply(branch: &str) -> Result<Value, ApiError> {
    if !UPDATE_BRANCHES.contains(&branch) {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "not_found", format!("Unknown branch '{branch}'")));
    }
    Ok(json!({"branch": branch, "backfill": get_backfill_status(branch).to_json()}))
}

pub async fn backfill_modified(
    State(state): State<AppState>,
    _: Superuser,
    Args(BranchQuery(branch)): Args<BranchQuery>,
) -> Res {
    let (body, start) = start_reply(&branch)?;
    if start {
        tokio::spawn(backfill_last_ordinal(state, branch));
    }
    Ok(PydanticJson(body))
}

pub async fn backfill_modified_status(_: Superuser, Args(BranchQuery(branch)): Args<BranchQuery>) -> Res {
    Ok(PydanticJson(status_reply(&branch)?))
}

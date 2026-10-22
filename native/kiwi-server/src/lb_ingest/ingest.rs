//! `service.insert_dump` + the router's `_process_leaderboard_dump`: parse,
//! anchor, completeness guard, one-transaction write, then the ingest log,
//! cache invalidation, backlog copy, `leaderboard` event and warmer wake.

use std::{collections::HashMap, sync::Arc};

use kiwi_core::lbparse::{self, Board, Verdict};
use mongodb::bson::{Bson, Document, doc};
use serde_json::{Value, json};

use super::{backlog, signals, store};
use crate::{
    auth::{
        models::{ApiToken, User},
        pyutil::bson_int,
    },
    runtime_config::get_setting,
    state::AppState,
};

pub const ENDPOINT: &str = "/v1/leaderboards/insert";

/// What `insert_dump` returned.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    /// Nothing parsed: `{"boards": 0, "entries": 0, "created_at": None}`.
    Empty,
    /// Rejected by the completeness guard and not stored.
    Skipped {
        created_at: i64,
        boards: usize,
        entries: usize,
        verdict: Verdict,
        prev_anchor: i64,
        prev_boards: usize,
        cur_boards: usize,
    },
    Stored {
        created_at: i64,
        written: store::Written,
        rejected_timestamp: Option<i64>,
    },
}

/// What `insert_dump` reads and writes: Postgres and the runtime settings.
pub trait IngestStore {
    fn setting(&self, key: &str) -> impl Future<Output = Result<Value, String>> + Send;
    fn previous_anchor_before(&self, anchor: i64) -> impl Future<Output = Result<Option<i64>, String>> + Send;
    fn board_counts_at(&self, anchor: i64) -> impl Future<Output = Result<HashMap<i64, i64>, String>> + Send;
    fn board_kinds(&self, uuids: &[i64]) -> impl Future<Output = Result<HashMap<i64, String>, String>> + Send;
    fn write_snapshot(
        &self,
        boards: &[Board<'_>],
        anchor: i64,
    ) -> impl Future<Output = Result<store::Written, String>> + Send;
}

/// The production [`IngestStore`].
pub struct Live<'a>(pub &'a AppState);

impl IngestStore for Live<'_> {
    async fn setting(&self, key: &str) -> Result<Value, String> {
        get_setting(self.0, key).await.map_err(|e| e.to_string())
    }
    async fn previous_anchor_before(&self, anchor: i64) -> Result<Option<i64>, String> {
        store::previous_anchor_before(store::pool(self.0)?, anchor).await
    }
    async fn board_counts_at(&self, anchor: i64) -> Result<HashMap<i64, i64>, String> {
        store::board_counts_at(store::pool(self.0)?, anchor).await
    }
    async fn board_kinds(&self, uuids: &[i64]) -> Result<HashMap<i64, String>, String> {
        store::board_kinds(store::pool(self.0)?, uuids).await
    }
    async fn write_snapshot(&self, boards: &[Board<'_>], anchor: i64) -> Result<store::Written, String> {
        store::write_snapshot(store::pool(self.0)?, boards, anchor).await
    }
}

/// `int(v)` of a runtime setting value.
pub fn py_int(v: &Value) -> i64 {
    match v {
        Value::Bool(b) => i64::from(*b),
        Value::Number(n) => n.as_i64().unwrap_or_else(|| n.as_f64().map_or(0, |f| f.trunc() as i64)),
        Value::String(s) => kiwi_core::pynum::int(s).unwrap_or(0),
        _ => 0,
    }
}

/// `float(v)` of a runtime setting value.
pub fn py_float(v: &Value) -> f64 {
    match v {
        Value::Bool(b) => f64::from(u8::from(*b)),
        Value::Number(n) => n.as_f64().unwrap_or(0.0),
        Value::String(s) => kiwi_core::pynum::float(s).unwrap_or(0.0),
        _ => 0.0,
    }
}

/// The guard's reject verdict plus `(prev_anchor, prev_boards, cur_boards)`.
type Rejected = (Verdict, i64, usize, usize);

/// `_assess_capture_completeness`.
async fn assess(st: &impl IngestStore, boards: &[Board<'_>], anchor: i64) -> Result<Option<Rejected>, String> {
    if !crate::features::py_truthy(&st.setting("capture_completeness_enabled").await?) {
        return Ok(None);
    }
    let Some(prev) = st.previous_anchor_before(anchor).await? else { return Ok(None) };
    let mut prev_counts = st.board_counts_at(prev).await?;
    let min_prev = py_int(&st.setting("capture_min_prev_boards").await?);
    if (prev_counts.len() as i64) < min_prev {
        return Ok(None);
    }
    let max_missing = py_int(&st.setting("capture_max_missing_boards").await?);
    let collapse_frac = py_float(&st.setting("capture_collapse_frac").await?);
    let mut cur_counts: HashMap<i64, i64> = boards.iter().map(|b| (b.uuid, b.entries.len() as i64)).collect();
    if !lbparse::reset_boundaries("daily", prev, anchor).is_empty()
        || !lbparse::reset_boundaries("weekly", prev, anchor).is_empty()
    {
        let uuids: Vec<i64> = prev_counts.keys().copied().collect();
        let kinds = st.board_kinds(&uuids).await?;
        let none = |u: &i64| kinds.get(u).is_some_and(|k| k == "none");
        prev_counts.retain(|u, _| none(u));
        cur_counts.retain(|u, _| none(u));
        if prev_counts.is_empty() {
            return Ok(None);
        }
    }
    Ok(lbparse::completeness_verdict(&cur_counts, &prev_counts, max_missing, collapse_frac)
        .map(|v| (v, prev, prev_counts.len(), cur_counts.len())))
}

/// `insert_dump(text, timestamp=, allow_backfill=)` at `now`; Err is `str(exc)`.
pub async fn insert_dump(
    st: &impl IngestStore,
    text: &str,
    timestamp: Option<i64>,
    allow_backfill: bool,
    now: i64,
) -> Result<Outcome, String> {
    // Off the async workers (a full dump is ~700k rows), borrowing the text.
    let boards = match tokio::runtime::Handle::current().runtime_flavor() {
        tokio::runtime::RuntimeFlavor::MultiThread => tokio::task::block_in_place(|| lbparse::parse_dump(text)),
        _ => lbparse::parse_dump(text),
    }?;
    if boards.is_empty() {
        tracing::warn!("leaderboards: parsed 0 boards from {}-char dump", text.chars().count());
        return Ok(Outcome::Empty);
    }

    let mut rejected_timestamp = None;
    let created_at = match timestamp {
        Some(ts) if ts > 0 => match lbparse::normalize_timestamp(ts, now, allow_backfill)? {
            -1 => {
                rejected_timestamp = Some(ts);
                let fallback = lbparse::trove_day_start(now);
                tracing::warn!(
                    "leaderboards: client timestamp {ts} is {:+}s vs server now - out of range, falling back to \
                     day anchor {fallback}; this capture OVERWRITES the 11:00 UTC snapshot. Check the scraper \
                     host's clock.",
                    ts - now
                );
                fallback
            }
            a => a,
        },
        _ => lbparse::trove_day_start(now),
    };

    if !allow_backfill
        && let Some((verdict, prev_anchor, prev_boards, cur_boards)) = assess(st, &boards, created_at).await?
    {
        tracing::warn!(
            "leaderboards: REJECTED incomplete capture anchor={created_at} ({}); prev={prev_anchor} boards \
             {prev_boards}->{cur_boards}",
            verdict.reason
        );
        return Ok(Outcome::Skipped {
            created_at,
            boards: boards.len(),
            entries: boards.iter().map(|b| b.entries.len()).sum(),
            verdict,
            prev_anchor,
            prev_boards,
            cur_boards,
        });
    }

    let written = st.write_snapshot(&boards, created_at).await?;
    tracing::info!(
        "leaderboards: ingested anchor={created_at} boards={} entries={} cleared={}",
        written.boards,
        written.entries,
        written.cleared
    );
    Ok(Outcome::Stored { created_at, written, rejected_timestamp })
}

fn ints(v: &[i64]) -> Bson {
    Bson::Array(v.iter().map(|&n| bson_int(n)).collect())
}

fn opt_int(v: Option<i64>) -> Bson {
    v.map_or(Bson::Null, bson_int)
}

/// The ingest-log summary of a stored (or empty) dump.
pub fn stored_summary(outcome: &Outcome, nbytes: usize) -> Document {
    let (boards, entries, anchor, cleared, archived, rejected) = match outcome {
        Outcome::Stored { created_at, written, rejected_timestamp } => (
            bson_int(written.boards as i64),
            bson_int(written.entries as i64),
            bson_int(*created_at),
            bson_int(written.cleared as i64),
            bson_int(0),
            opt_int(*rejected_timestamp),
        ),
        _ => (bson_int(0), bson_int(0), Bson::Null, Bson::Null, Bson::Null, Bson::Null),
    };
    doc! {
        "boards": boards,
        "entries": entries,
        "anchor": anchor,
        "cleared_before_insert": cleared,
        "archived_old": archived,
        "rejected_timestamp": rejected,
        "bytes": bson_int(nbytes as i64),
    }
}

/// The ingest-log summary of a rejected capture.
pub fn skipped_summary(outcome: &Outcome, nbytes: usize) -> Document {
    let Outcome::Skipped { created_at, boards, entries, verdict, prev_anchor, .. } = outcome else {
        return Document::new();
    };
    doc! {
        "rejected": true,
        "anchor": bson_int(*created_at),
        "boards": bson_int(*boards as i64),
        "entries": bson_int(*entries as i64),
        "missing_boards": ints(&verdict.missing_boards),
        "collapsed_boards": ints(&verdict.collapsed_boards),
        "prev_anchor": bson_int(*prev_anchor),
        "bytes": bson_int(nbytes as i64),
    }
}

/// The ingest-log summary of a failed ingest (`timestamp` is the raw query value).
pub fn failed_summary(nbytes: usize, timestamp: Option<i64>) -> Document {
    doc! { "bytes": bson_int(nbytes as i64), "anchor": opt_int(timestamp) }
}

/// `str(exc)[:300]`.
pub fn clip_error(e: &str) -> String {
    kiwi_core::pystr::prefix(e, 300).to_owned()
}

/// Who submitted the dump.
#[derive(Clone)]
pub struct Submitter {
    pub user: User,
    pub token: Option<ApiToken>,
}

/// One thing `_process_leaderboard_dump` does after `insert_dump`, in order.
#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    Log { summary: Document, error: Option<String> },
    Invalidate(i64),
    Save(i64),
    Publish { signature: String, data: Value },
    Warm,
}

/// `_process_leaderboard_dump`'s follow-up for an `insert_dump` result.
pub fn effects(result: &Result<Outcome, String>, nbytes: usize, timestamp: Option<i64>, warm: bool) -> Vec<Effect> {
    let outcome = match result {
        Err(e) => return vec![Effect::Log { summary: failed_summary(nbytes, timestamp), error: Some(clip_error(e)) }],
        Ok(o) => o,
    };
    if let Outcome::Skipped { created_at, verdict, .. } = outcome {
        let mut out = vec![Effect::Log {
            summary: skipped_summary(outcome, nbytes),
            error: Some(format!("rejected incomplete capture: {}", verdict.reason)),
        }];
        if *created_at != 0 {
            out.push(Effect::Save(*created_at));
        }
        return out;
    }
    let mut out = vec![Effect::Log { summary: stored_summary(outcome, nbytes), error: None }];
    if let Outcome::Stored { created_at, written, .. } = outcome
        && *created_at != 0
    {
        out.push(Effect::Invalidate(*created_at));
        out.push(Effect::Save(*created_at));
        out.push(Effect::Publish {
            signature: created_at.to_string(),
            data: json!({"anchor": created_at, "boards": written.boards, "entries": written.entries}),
        });
    }
    if warm {
        out.push(Effect::Warm);
    }
    out
}

/// `_process_leaderboard_dump`: never raises; None when the ingest failed.
pub async fn process_dump(
    state: &AppState,
    text: Arc<String>,
    timestamp: Option<i64>,
    who: &Submitter,
    nbytes: usize,
    allow_backfill: bool,
    warm: bool,
) -> Option<Outcome> {
    let now = chrono::Utc::now().timestamp();
    let result = insert_dump(&Live(state), &text, timestamp, allow_backfill, now).await;
    match &result {
        Err(e) => tracing::error!("leaderboard ingest processing failed: {e}"),
        Ok(Outcome::Skipped { created_at, verdict, .. }) => {
            tracing::warn!("leaderboard ingest REJECTED (incomplete) anchor={created_at}: {}", verdict.reason)
        }
        Ok(_) => {}
    }
    for effect in effects(&result, nbytes, timestamp, warm) {
        match effect {
            Effect::Log { summary, error } => {
                signals::record(state, ENDPOINT, &who.user, who.token.as_ref(), summary, error.as_deref()).await
            }
            Effect::Invalidate(anchor) => signals::invalidate_anchor(state, anchor).await,
            Effect::Save(anchor) => backlog::save(state, anchor, text.clone()).await,
            Effect::Publish { signature, data } => {
                signals::publish_event(state, "leaderboard", &signature, data).await;
            }
            Effect::Warm => signals::trigger_warmer(state, &who.user),
        }
    }
    result.ok()
}

/// `LeaderboardInsertResponse` for `?sync=true` (`accepted`, `message`).
pub fn sync_reply(outcome: Option<&Outcome>) -> (bool, String) {
    match outcome {
        None => (false, "Ingest failed - see the master ingest log for the error.".into()),
        Some(Outcome::Skipped { created_at, verdict, .. }) => (
            false,
            format!(
                "Rejected incomplete capture for anchor {created_at}: {}. Not stored (kept in backlog for audit).",
                verdict.reason
            ),
        ),
        Some(Outcome::Empty) => (true, "Ingested anchor None: 0 boards, 0 entries.".into()),
        Some(Outcome::Stored { created_at, written, .. }) => {
            (true, format!("Ingested anchor {created_at}: {} boards, {} entries.", written.boards, written.entries))
        }
    }
}

pub const ACCEPTED_MESSAGE: &str = "Dump accepted - parsing and persisting in the background; \
    boards/entries counts land in the ingest log when done.";

//! `app/design_feedback`: the anonymous thumbs up/down on the site design
//! (`POST /site/design-feedback`, per-IP throttled, no feature gate) and the
//! master readout (`GET /admin/design-feedback`).

use axum::{
    Router,
    extract::{FromRequestParts, State},
    http::{StatusCode, request::Parts},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use chrono::{DateTime, Duration, Utc};
use futures::TryStreamExt;
use kiwi_core::leaf::{FeedbackRow, Tally, Votes, tally};
use mongodb::bson::{Bson, Document, doc};
use serde_json::{Map, Value, json};

use super::{body::DepsThenBody, internal};
use crate::{
    auth::{Rejection, Superuser, client::client_ip, pyutil::int_of},
    dt,
    error::ApiError,
    pyjson::PydanticJson,
    ratelimit::check_rate_limit,
    runtime_config,
    state::AppState,
    validation::{Field, FromBody, FromParams, Params, Query},
};

pub const COLLECTION: &str = "design_feedback";
const DESIGNS: &[&str] = &kiwi_core::leaf::DESIGNS;
const THEMES: &[&str] = &kiwi_core::leaf::THEMES;
const VOTES: &[&str] = &["up", "down"];

pub fn router() -> Router<AppState> {
    Router::new().route("/site/design-feedback", post(vote)).route("/admin/design-feedback", get(summary))
}

/// `_throttle`: the per-IP `design_feedback_rate_limit` bucket.
pub struct Throttle;

impl FromRequestParts<AppState> for Throttle {
    type Rejection = Rejection;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Rejection> {
        let (max, window) =
            runtime_config::get_rate_limit(state, "design_feedback_rate_limit").await.map_err(Rejection::internal)?;
        let ip = client_ip(parts).unwrap_or_else(|| "unknown".into());
        check_rate_limit(state, &format!("designfeedback:{ip}"), max, window).await?;
        Ok(Throttle)
    }
}

/// `DesignVote`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesignVote {
    pub design: String,
    pub theme: String,
    pub vote: String,
}

impl FromBody for DesignVote {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| {
            Some(DesignVote {
                design: m.field("design").literal(DESIGNS).required(),
                theme: m.field("theme").literal(THEMES).default("auto".into()),
                vote: m.field("vote").literal(VOTES).required(),
            })
        })
        .required()
    }
}

/// `record_vote`'s upsert: filter and update.
pub fn vote_upsert(v: &DesignVote, now: DateTime<Utc>) -> (Document, Document) {
    let day = now.format("%Y-%m-%d").to_string();
    let filter = doc! {"_id": format!("{day}|{}|{}|{}", v.design, v.theme, v.vote)};
    let update = doc! {
        "$inc": {"count": 1_i32},
        "$set": {"day": &day, "design": &v.design, "theme": &v.theme, "vote": &v.vote, "updated_at": dt::to_bson(now)},
    };
    (filter, update)
}

async fn vote(
    State(state): State<AppState>,
    DepsThenBody(Throttle, v): DepsThenBody<Throttle, DesignVote>,
) -> Result<Response, ApiError> {
    let (filter, update) = vote_upsert(&v, dt::now());
    state.mongo().collection::<Document>(COLLECTION).update_one(filter, update).upsert(true).await.map_err(internal)?;
    Ok((StatusCode::ACCEPTED, PydanticJson(json!({"ok": true}))).into_response())
}

pub struct Days(pub i64);

impl FromParams for Days {
    fn from_params(p: &mut Params<'_>) -> Self {
        Days(p.query("days").int().ge(1).le(365).default(30))
    }
}

/// `since` of `summary()`: the first UTC day of the window.
pub fn since_day(now: DateTime<Utc>, days: i64) -> String {
    (now - Duration::days(days - 1)).format("%Y-%m-%d").to_string()
}

/// `int(r.get("count") or 0)`; `None` where Python raises.
pub fn count_of(v: Option<&Bson>) -> Option<i64> {
    match v {
        None | Some(Bson::Null) => Some(0),
        Some(Bson::Boolean(b)) => Some(i64::from(*b)),
        Some(Bson::Int32(i)) => Some(i64::from(*i)),
        Some(Bson::Int64(i)) => Some(*i),
        Some(Bson::Double(f)) if *f == 0.0 => Some(0),
        Some(Bson::Double(f)) => f.is_finite().then(|| f.trunc() as i64),
        Some(Bson::String(s)) if s.is_empty() => Some(0),
        Some(Bson::String(s)) => int_of(&Value::String(s.clone())).and_then(|i| i64::try_from(i).ok()),
        Some(Bson::Array(a)) if a.is_empty() => Some(0),
        Some(Bson::Document(d)) if d.is_empty() => Some(0),
        Some(_) => None,
    }
}

/// `r.get(key) or "?"` for the text keys.
fn key_of<'a>(d: &'a Document, key: &str) -> &'a str {
    match d.get(key) {
        Some(Bson::String(s)) if !s.is_empty() => s,
        _ => "?",
    }
}

/// The tallies of the stored rows; `None` when a count is unreadable.
pub fn tally_docs(rows: &[Document]) -> Option<Tally> {
    let mut parsed = Vec::with_capacity(rows.len());
    for r in rows {
        let count = count_of(r.get("count"))?;
        let vote = match r.get("vote") {
            Some(Bson::String(s)) => Some(s.as_str()),
            _ => None,
        };
        parsed.push(FeedbackRow {
            vote,
            count,
            design: key_of(r, "design"),
            theme: key_of(r, "theme"),
            day: key_of(r, "day"),
        });
    }
    Some(tally(parsed))
}

fn votes(v: &Votes) -> Value {
    json!({"up": v.up, "down": v.down})
}

fn votes_map(list: &[(String, Votes)]) -> Value {
    Value::Object(list.iter().map(|(k, v)| (k.clone(), votes(v))).collect::<Map<_, _>>())
}

pub fn summary_body(days: i64, t: &Tally) -> Value {
    let series: Vec<Value> = t.series.iter().map(|(d, v)| json!({"day": d, "up": v.up, "down": v.down})).collect();
    json!({
        "days": days,
        "total": votes(&t.total),
        "by_design": votes_map(&t.by_design),
        "by_theme": votes_map(&t.by_theme),
        "series": series,
    })
}

async fn summary(
    State(state): State<AppState>,
    _: Superuser,
    Query(Days(days)): Query<Days>,
) -> Result<PydanticJson, ApiError> {
    let since = since_day(Utc::now(), days);
    let rows: Vec<Document> = state
        .mongo()
        .collection::<Document>(COLLECTION)
        .find(doc! {"day": {"$gte": since}})
        .await
        .map_err(internal)?
        .try_collect()
        .await
        .map_err(internal)?;
    let t = tally_docs(&rows).ok_or_else(|| internal("design_feedback: unreadable count"))?;
    Ok(PydanticJson(summary_body(days, &t)))
}

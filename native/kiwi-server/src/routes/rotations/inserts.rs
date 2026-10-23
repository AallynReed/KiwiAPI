//! The master-only bot capture inserts of `app/trove/router.py` and
//! `app/trove/captures.py`: `POST /v1/rotations/{luxion,chaos-chest,challenge}/insert`.
//! Each writes its capture the way Beanie does, records the ingest log and
//! publishes on the events bus (exactly once across processes).

use axum::{
    extract::State,
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Utc};
use kiwi_core::{pystr, rotations as rot};
use mongodb::{
    Collection, Database,
    bson::{Bson, Document, doc, oid::ObjectId},
};
use serde_json::json;

use super::{
    bodies::{ChallengeDoc, LuxionDoc},
    store::{self, ChaosCapture},
};
use crate::{
    auth::{IngestAuth, MasterIngest, pyutil::bson_int},
    dt,
    error::ApiError,
    lb_ingest::signals,
    pyjson::PydanticJson,
    state::AppState,
    validation::{DepsThenBody, Field, FromBody},
};

pub const LUXION_PATH: &str = "/v1/rotations/luxion/insert";
pub const CHAOS_PATH: &str = "/v1/rotations/chaos-chest/insert";
pub const CHALLENGE_PATH: &str = "/v1/rotations/challenge/insert";

/// `CaptureInsertRequest`.
pub struct CaptureInsert {
    pub name: String,
}

impl FromBody for CaptureInsert {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| Some(CaptureInsert { name: m.field("name").str().required() })).required()
    }
}

/// `body: LuxionInsertRequest | None = None` (`active: bool = True`).
pub struct LuxionInsert {
    pub active: bool,
}

impl FromBody for LuxionInsert {
    fn from_body(body: Field<'_>) -> Option<Self> {
        let parsed = body.model(|m| Some(m.field("active").bool().default(true))).optional();
        Some(LuxionInsert { active: parsed.flatten().unwrap_or(true) })
    }
}

fn internal(e: impl std::fmt::Display) -> ApiError {
    tracing::error!("rotation capture insert: {e}");
    ApiError::internal()
}

fn db(state: &AppState) -> Result<&Database, ApiError> {
    state.inner.db.mongo.as_ref().ok_or_else(|| internal("MONGO_URI not configured"))
}

fn coll(db: &Database, name: &str) -> Collection<Document> {
    db.collection::<Document>(name)
}

fn with_id(id: ObjectId, fields: Document) -> Document {
    let mut d = doc! { "_id": id };
    d.extend(fields);
    d
}

/// Beanie `insert()` (pymongo puts the new `_id` first).
async fn insert(c: &Collection<Document>, fields: Document) -> mongodb::error::Result<ObjectId> {
    let id = ObjectId::new();
    c.insert_one(with_id(id, fields)).await?;
    Ok(id)
}

/// Beanie `save()` of a loaded document: `$set` of `_id` and every field.
async fn save(c: &Collection<Document>, id: &Bson, fields: Document) -> mongodb::error::Result<()> {
    let mut set = doc! { "_id": id.clone() };
    set.extend(fields);
    c.update_one(doc! { "_id": id.clone() }, doc! { "$set": set }).upsert(true).await?;
    Ok(())
}

fn is_duplicate(e: &mongodb::error::Error) -> bool {
    matches!(
        e.kind.as_ref(),
        mongodb::error::ErrorKind::Write(mongodb::error::WriteFailure::WriteError(w)) if w.code == 11000
    )
}

fn bdt(t: DateTime<Utc>) -> Bson {
    Bson::DateTime(dt::to_bson(t))
}

fn reply(auth: &IngestAuth, anchor: i64, name: &str, refreshed: bool) -> Response {
    let body = PydanticJson(json!({"anchor": anchor, "name": name, "refreshed": refreshed}));
    (auth.rate_limit, body).into_response()
}

async fn log(state: &AppState, endpoint: &str, auth: &IngestAuth, summary: Document, error: Option<&str>) {
    signals::record(state, endpoint, &auth.user, auth.token.as_ref(), summary, error).await;
}

/// The rejected-capture branch: a failed log row, then the 400.
async fn rejected(state: &AppState, endpoint: &str, auth: &IngestAuth, raw: &str, msg: &str) -> Response {
    log(state, endpoint, auth, doc! { "name": raw }, Some(msg)).await;
    ApiError::bad_request(msg).into_response()
}

// --- chaos chest ------------------------------------------------------------------

pub fn chaos_fields(week_anchor: i64, name: &str, captured_at: DateTime<Utc>) -> Document {
    doc! { "week_anchor": bson_int(week_anchor), "name": name, "captured_at": bdt(captured_at) }
}

/// `captures.insert_chaos_chest`: `(week_anchor, was_new)`.
async fn insert_chaos(db: &Database, name: &str, now_us: i64) -> Result<(i64, bool), ApiError> {
    let week_anchor = rot::chaos_chest_window(now_us).0;
    let captured_at = dt::now();
    let c = coll(db, store::CHAOS);
    let found = c.find_one(doc! { "week_anchor": week_anchor }).await.map_err(internal)?;
    let Some(found) = found else {
        insert(&c, chaos_fields(week_anchor, name, captured_at)).await.map_err(internal)?;
        tracing::info!("chaos chest captured: week={week_anchor} name={} (new)", pystr::repr(name));
        return Ok((week_anchor, true));
    };
    let ChaosCapture { week_anchor: stored, .. } = store::chaos_doc(&found).map_err(|_| internal("invalid capture"))?;
    let id = found.get("_id").cloned().unwrap_or(Bson::Null);
    save(&c, &id, chaos_fields(stored, name, captured_at)).await.map_err(internal)?;
    tracing::info!("chaos chest captured: week={week_anchor} name={} (refreshed)", pystr::repr(name));
    Ok((stored, false))
}

pub async fn chaos_insert(
    State(state): State<AppState>,
    DepsThenBody(MasterIngest(auth), req): DepsThenBody<MasterIngest, CaptureInsert>,
) -> Result<Response, ApiError> {
    let name = pystr::strip(&req.name).to_owned();
    if name.is_empty() {
        return Ok(rejected(&state, CHAOS_PATH, &auth, &req.name, "name is empty").await);
    }
    let (anchor, was_new) = insert_chaos(db(&state)?, &name, dt::now().timestamp_micros()).await?;
    log(&state, CHAOS_PATH, &auth, doc! { "name": &name, "anchor": bson_int(anchor), "refreshed": !was_new }, None)
        .await;
    if let Err(e) = crate::events::publish_chaos(&state).await {
        tracing::warn!("chaos event publish failed: {e:#}");
    }
    Ok(reply(&auth, anchor, &name, !was_new))
}

// --- hourly challenge ---------------------------------------------------------------

pub fn challenge_fields(d: &ChallengeDoc) -> Document {
    doc! {
        "window_anchor": bson_int(d.window_anchor),
        "window_ends_at": bson_int(d.window_ends_at),
        "name": &d.name,
        "is_friday_window": d.is_friday_window,
        "captured_at": bdt(d.captured_at),
    }
}

/// `name.strip()` rejected when empty or `none` (any case).
pub fn challenge_name_ok(name: &str) -> bool {
    !name.is_empty() && name.to_lowercase() != "none"
}

/// `captures.insert_challenge`: `(window_anchor, was_new)`.
async fn insert_challenge(db: &Database, name: &str, now_us: i64) -> Result<(i64, bool), ApiError> {
    let window = rot::challenge_window(now_us);
    let captured_at = dt::now();
    let c = coll(db, store::CHALLENGE);
    let found = c.find_one(doc! { "window_anchor": window.starts_at }).await.map_err(internal)?;
    let Some(found) = found else {
        let d = ChallengeDoc {
            window_anchor: window.starts_at,
            window_ends_at: window.ends_at,
            name: name.to_owned(),
            is_friday_window: window.is_friday_window,
            captured_at,
        };
        insert(&c, challenge_fields(&d)).await.map_err(internal)?;
        tracing::info!("challenge captured: window={} name={} (new)", window.starts_at, pystr::repr(name));
        return Ok((window.starts_at, true));
    };
    let mut d = store::challenge_doc(&found).map_err(|_| internal("invalid capture"))?;
    d.name = name.to_owned();
    d.captured_at = captured_at;
    d.window_ends_at = window.ends_at;
    d.is_friday_window = window.is_friday_window;
    let id = found.get("_id").cloned().unwrap_or(Bson::Null);
    save(&c, &id, challenge_fields(&d)).await.map_err(internal)?;
    tracing::info!("challenge captured: window={} name={} (refreshed)", window.starts_at, pystr::repr(name));
    Ok((d.window_anchor, false))
}

pub async fn challenge_insert(
    State(state): State<AppState>,
    DepsThenBody(MasterIngest(auth), req): DepsThenBody<MasterIngest, CaptureInsert>,
) -> Result<Response, ApiError> {
    let name = pystr::strip(&req.name).to_owned();
    if !challenge_name_ok(&name) {
        let msg = "challenge name is empty or 'none'";
        return Ok(rejected(&state, CHALLENGE_PATH, &auth, &req.name, msg).await);
    }
    let (anchor, was_new) = insert_challenge(db(&state)?, &name, dt::now().timestamp_micros()).await?;
    log(&state, CHALLENGE_PATH, &auth, doc! { "name": &name, "anchor": bson_int(anchor), "refreshed": !was_new }, None)
        .await;
    if let Err(e) = crate::events::publish_challenge(&state).await {
        tracing::warn!("challenge event publish failed: {e:#}");
    }
    Ok(reply(&auth, anchor, &name, !was_new))
}

// --- Luxion -------------------------------------------------------------------------

/// `server_time.current_daily_reset(now)`: the trove-day start (11:00 UTC) at
/// or before `now` (µs), in Unix seconds.
pub fn current_daily_reset(now_us: i64) -> i64 {
    let today = now_us.div_euclid(rot::DAY) * rot::DAY + 11 * rot::HOUR;
    rot::unix(if now_us >= today { today } else { today - rot::DAY })
}

/// What `insert_luxion` does with the newest appearance.
#[derive(Debug, PartialEq, Eq)]
pub enum LuxionStep {
    /// `active=false` with no live run: nothing to close.
    NothingToClose,
    /// Stamp `ended_at` on the live run.
    Close,
    /// A re-sighting inside the live run.
    Refresh,
    /// A new appearance anchored to today's reset.
    Open,
}

/// The live-run test: not ended, and before `run_bounds(started_at, first_seen)[1]`.
pub fn luxion_step(latest: Option<&LuxionDoc>, now_ts: i64, active: bool) -> LuxionStep {
    let live = latest.is_some_and(|l| {
        l.ended_at.is_none()
            && now_ts < rot::run_bounds(l.started_at, Some(super::bodies::ts(&l.first_seen_at)), None).1
    });
    match (active, live) {
        (false, false) => LuxionStep::NothingToClose,
        (false, true) => LuxionStep::Close,
        (true, true) => LuxionStep::Refresh,
        (true, false) => LuxionStep::Open,
    }
}

pub fn luxion_fields(d: &LuxionDoc) -> Document {
    doc! {
        "started_at": bson_int(d.started_at),
        "first_seen_at": bdt(d.first_seen_at),
        "last_seen_at": bdt(d.last_seen_at),
        "ended_at": d.ended_at.map_or(Bson::Null, bdt),
    }
}

/// `captures.insert_luxion`: `(started_at of the run, was_new)`, `None` for a
/// no-op close.
async fn insert_luxion(db: &Database, active: bool, real: DateTime<Utc>) -> Result<(Option<i64>, bool), ApiError> {
    let now_us = real.timestamp_micros();
    let anchor = current_daily_reset(now_us);
    let c = coll(db, store::LUXION);
    let latest = c.find_one(doc! {}).sort(doc! { "started_at": -1 }).await.map_err(internal)?;
    let parsed = latest.as_ref().map(store::luxion_doc).transpose().map_err(|_| internal("invalid appearance"))?;
    let id = latest.as_ref().and_then(|d| d.get("_id").cloned()).unwrap_or(Bson::Null);
    match (luxion_step(parsed.as_ref(), rot::unix(now_us), active), parsed) {
        (LuxionStep::NothingToClose, _) => Ok((None, false)),
        (LuxionStep::Close, Some(mut live)) => {
            live.ended_at = Some(real);
            save(&c, &id, luxion_fields(&live)).await.map_err(internal)?;
            tracing::info!("luxion absent: run start={} closed", live.started_at);
            Ok((Some(live.started_at), false))
        }
        (LuxionStep::Refresh, Some(mut live)) => {
            live.last_seen_at = real;
            save(&c, &id, luxion_fields(&live)).await.map_err(internal)?;
            tracing::info!("luxion sighting: run start={} (refreshed)", live.started_at);
            Ok((Some(live.started_at), false))
        }
        _ => {
            let fresh = LuxionDoc { started_at: anchor, first_seen_at: real, last_seen_at: real, ended_at: None };
            match insert(&c, luxion_fields(&fresh)).await {
                Ok(_) => {
                    tracing::info!("luxion sighting: run start={anchor} (new appearance)");
                    Ok((Some(anchor), true))
                }
                Err(e) if is_duplicate(&e) => {
                    let found = c.find_one(doc! { "started_at": anchor }).await.map_err(internal)?;
                    let Some(found) = found else { return Err(internal(e)) };
                    let mut existing = store::luxion_doc(&found).map_err(|_| internal("invalid appearance"))?;
                    existing.last_seen_at = real;
                    existing.ended_at = None;
                    let id = found.get("_id").cloned().unwrap_or(Bson::Null);
                    save(&c, &id, luxion_fields(&existing)).await.map_err(internal)?;
                    tracing::info!("luxion sighting: run start={anchor} (raced, refreshed)");
                    Ok((Some(existing.started_at), false))
                }
                Err(e) => Err(internal(e)),
            }
        }
    }
}

pub async fn luxion_insert(
    State(state): State<AppState>,
    DepsThenBody(MasterIngest(auth), body): DepsThenBody<MasterIngest, LuxionInsert>,
) -> Result<Response, ApiError> {
    let active = body.active;
    let (started, was_new) = insert_luxion(db(&state)?, active, dt::now()).await?;
    let anchor = started.unwrap_or(0);
    let summary = doc! { "started_at": bson_int(anchor), "active": active, "refreshed": !was_new };
    log(&state, LUXION_PATH, &auth, summary, None).await;
    if let Err(e) = crate::events::publish_luxion(&state).await {
        tracing::warn!("luxion event publish failed: {e:#}");
    }
    Ok(reply(&auth, anchor, if active { "luxion" } else { "luxion:away" }, !was_new))
}

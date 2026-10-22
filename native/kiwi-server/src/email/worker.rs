//! The outbox delivery loop (`_worker_loop`): claim due messages one at a
//! time (at most 50 per wake), deliver, record the outcome, sleep
//! `email_worker_poll_seconds`. The claim is the same conditional update
//! Python uses, so both sides can run against one outbox without double sends.

use std::time::Duration;

use chrono::{DateTime, TimeDelta, Utc};
use mongodb::{
    Database,
    bson::{Document, doc},
    options::ReturnDocument,
};

use super::{OUTBOX, OutboxEmail, mailer};
use crate::{
    auth::{
        models::{USERS, User},
        pyutil::bson_int,
    },
    config::Settings,
    dt, jobs,
    state::AppState,
};

const RECLAIM_AFTER_MINUTES: i64 = 5;
const BURST: usize = 50;

pub fn start(state: &AppState) {
    jobs::spawn(state, "email_outbox", |state| async move {
        let poll = Duration::from_secs(state.settings().email_worker_poll_seconds.max(0) as u64);
        loop {
            if let Err(err) = run_once(&state).await {
                tracing::error!("Email worker iteration failed: {err:#}");
            }
            tokio::time::sleep(poll).await;
        }
    });
}

/// One wake of the loop: deliver due messages until none is left or the
/// burst cap is hit.
pub async fn run_once(state: &AppState) -> anyhow::Result<usize> {
    let db = state.mongo();
    let mut processed = 0;
    while let Some(msg) = claim_next(db).await? {
        process_one(state, msg).await?;
        processed += 1;
        if processed >= BURST {
            break;
        }
    }
    Ok(processed)
}

/// `_claim_next`: atomically flip the next due message to `sending`.
pub async fn claim_next(db: &Database) -> anyhow::Result<Option<OutboxEmail>> {
    let now = dt::now();
    let reclaim_before = now - TimeDelta::minutes(RECLAIM_AFTER_MINUTES);
    let found = db
        .collection::<Document>(OUTBOX)
        .find_one_and_update(
            doc! {"$or": [
                {"status": "pending", "next_attempt_at": {"$lte": dt::to_bson(now)}},
                {"status": "sending", "claimed_at": {"$lte": dt::to_bson(reclaim_before)}},
            ]},
            doc! {"$set": {"status": "sending", "claimed_at": dt::to_bson(now)}},
        )
        .sort(doc! {"next_attempt_at": 1})
        .return_document(ReturnDocument::After)
        .await?;
    let Some(found) = found else { return Ok(None) };
    OutboxEmail::from_doc(&found)
        .map(Some)
        .ok_or_else(|| anyhow::anyhow!("outbox document {:?} doesn't validate", found.get("_id")))
}

/// Beanie `save()`: `$set` of the whole model (upsert).
async fn save(db: &Database, msg: &OutboxEmail) -> anyhow::Result<()> {
    db.collection::<Document>(OUTBOX)
        .update_one(doc! {"_id": msg.id}, doc! {"$set": msg.to_doc()})
        .upsert(true)
        .await?;
    Ok(())
}

/// The record after a delivery outcome (`_process_one` / `_handle_failure`).
/// `Some(to)` asks for the recipient to be flagged as bounced.
pub fn apply_outcome(
    msg: &mut OutboxEmail,
    outcome: &Result<(), mailer::DeliveryError>,
    settings: &Settings,
    now: DateTime<Utc>,
) -> bool {
    msg.attempts += 1;
    let exc = match outcome {
        Ok(()) => {
            msg.status = "sent".into();
            msg.sent_at = Some(now);
            return false;
        }
        Err(e) => e,
    };
    msg.last_error = Some(kiwi_core::pystr::prefix(&exc.message, 500).to_owned());
    if exc.permanent {
        msg.status = "bounced".into();
        return true;
    }
    if msg.attempts >= settings.email_max_attempts {
        msg.status = "failed".into();
        return false;
    }
    msg.status = "pending".into();
    msg.next_attempt_at = now + backoff(settings.email_retry_base_seconds, msg.attempts);
    false
}

/// `_backoff`: base * 2^(attempts-1) seconds.
pub fn backoff(base: i64, attempts: i64) -> TimeDelta {
    let exp = u32::try_from(attempts - 1).unwrap_or(0).min(40);
    TimeDelta::seconds(base.saturating_mul(1i64 << exp))
}

async fn process_one(state: &AppState, mut msg: OutboxEmail) -> anyhow::Result<()> {
    let settings = state.settings();
    let outcome = mailer::deliver_email(settings, &msg.to, &msg.subject, &msg.text, msg.html.as_deref()).await?;
    let bounce = apply_outcome(&mut msg, &outcome, settings, dt::now());
    let db = state.mongo();
    save(db, &msg).await?;
    match &outcome {
        Ok(()) => {}
        Err(e) if bounce => {
            flag_bounced(db, &msg.to).await?;
            tracing::warn!("Email to {} bounced: {e}", msg.to);
        }
        Err(e) if msg.status == "failed" => {
            tracing::error!("Email to {} failed after {} attempts: {e}", msg.to, msg.attempts)
        }
        Err(e) => tracing::info!("Email to {} retry {} scheduled: {e}", msg.to, msg.attempts),
    }
    Ok(())
}

/// The `$set` Beanie's `User.save()` sends: every model field, model order.
pub fn user_set(u: &User) -> Document {
    doc! {
        "_id": u.id,
        "email": &u.email,
        "hashed_password": &u.hashed_password,
        "display_name": u.display_name.clone(),
        "is_active": u.is_active,
        "is_verified": u.is_verified,
        "is_superuser": u.is_superuser,
        "roles": u.roles.clone(),
        "token_version": bson_int(u.token_version),
        "github_id": u.github_id.map(bson_int),
        "email_bounced": u.email_bounced,
        "created_at": u.created_at,
        "updated_at": u.updated_at,
        "last_login_at": u.last_login_at,
    }
}

/// `_flag_bounced`: mark the matching account so nothing more is queued for it.
pub async fn flag_bounced(db: &Database, to: &str) -> anyhow::Result<()> {
    let users = db.collection::<User>(USERS);
    let Some(mut user) = users.find_one(doc! {"email": to.to_lowercase()}).await? else {
        return Ok(());
    };
    if user.email_bounced {
        return Ok(());
    }
    user.email_bounced = true;
    db.collection::<Document>(USERS)
        .update_one(doc! {"_id": user.id}, doc! {"$set": user_set(&user)})
        .upsert(true)
        .await?;
    Ok(())
}

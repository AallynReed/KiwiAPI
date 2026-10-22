//! The daily maintenance sweep (`app/core/maintenance.py`, RUST_OWNS
//! `maintenance`): auto-revoke inactive API tokens, warn owners of tokens
//! about to expire, and mail the admin a digest of rate-limit hits. Runs at
//! startup, then every 24 hours.

#[cfg(test)]
mod tests;

use std::{collections::HashMap, time::Duration};

use chrono::{DateTime, TimeDelta, Utc};
use futures::TryStreamExt;
use kiwi_core::mail::{self, DigestRow};
use mongodb::{
    Database,
    bson::{Bson, Document, doc, oid::ObjectId},
};
use serde_json::Value;

use crate::{
    auth::{
        models::{API_TOKENS, ApiToken, USERS, User},
        pyutil::bson_int,
    },
    dt, email, jobs, pyfloat, runtime_config,
    state::AppState,
};

const SWEEP_INTERVAL: Duration = Duration::from_secs(86400);
pub const STATE_COLLECTION: &str = "system_state";
pub const USAGE_EVENTS: &str = "usage_events";
pub const DIGEST_KEY: &str = "rate_limit_digest";

pub fn start(state: &AppState) {
    jobs::spawn(state, "maintenance", |state| async move {
        loop {
            if let Err(err) = sweep(&state).await {
                tracing::error!("Maintenance sweep failed: {err:#}");
            }
            tokio::time::sleep(SWEEP_INTERVAL).await;
        }
    });
}

pub async fn sweep(state: &AppState) -> anyhow::Result<()> {
    auto_revoke_inactive_tokens(state).await?;
    warn_expiring_tokens(state).await?;
    daily_rate_limit_digest(state).await?;
    Ok(())
}

/// The tokens `auto_revoke_inactive_tokens` revokes: unused since `cutoff`
/// (never-used tokens count from creation).
pub fn inactive_filter(cutoff: DateTime<Utc>) -> Document {
    let cutoff = dt::to_bson(cutoff);
    doc! {
        "revoked": false,
        "$or": [
            {"last_used_at": {"$ne": Bson::Null, "$lt": cutoff}},
            {"last_used_at": Bson::Null, "created_at": {"$lt": cutoff}},
        ],
    }
}

pub async fn auto_revoke_inactive_tokens(state: &AppState) -> anyhow::Result<u64> {
    let days = state.settings().token_inactivity_revoke_days;
    let query = inactive_filter(dt::now() - TimeDelta::days(days));
    let tokens = email::mongo(state)?.collection::<Document>(API_TOKENS);
    let count = tokens.count_documents(query.clone()).await?;
    if count > 0 {
        let reason = format!("Auto-revoked: inactive for {days} days");
        tokens
            .update_many(
                query,
                doc! {"$set": {"revoked": true, "revoked_at": dt::to_bson(dt::now()), "revoke_reason": reason}},
            )
            .await?;
        tracing::warn!("Auto-revoked {count} inactive token(s)");
    }
    Ok(count)
}

/// Beanie `ApiToken.save()`: `$set` of every model field, in model order.
pub fn token_set(t: &ApiToken) -> Document {
    doc! {
        "_id": t.id,
        "user_id": t.user_id,
        "name": &t.name,
        "prefix": &t.prefix,
        "hashed_token": &t.hashed_token,
        "scopes": bson_int(t.scopes),
        "ip_salt": t.ip_salt.clone(),
        "allowed_ip_hashes": t.allowed_ip_hashes.clone(),
        "revoked": t.revoked,
        "revoked_at": t.revoked_at,
        "revoke_reason": t.revoke_reason.clone(),
        "expires_at": t.expires_at,
        "expiry_warned": t.expiry_warned,
        "created_at": t.created_at,
        "last_used_at": t.last_used_at,
        "rotated_at": t.rotated_at,
        "request_count": bson_int(t.request_count),
    }
}

/// `(expires_at - now).days`, floored at 0.
pub fn days_left(expires_at: DateTime<Utc>, now: DateTime<Utc>) -> i64 {
    let us = (expires_at - now).num_microseconds().unwrap_or(i64::MAX);
    us.div_euclid(86_400_000_000).max(0)
}

pub async fn warn_expiring_tokens(state: &AppState) -> anyhow::Result<usize> {
    let settings = state.settings();
    if !settings.security_email_notifications {
        return Ok(0);
    }
    let db = email::mongo(state)?;
    let now = dt::now();
    let horizon = now + TimeDelta::days(settings.token_expiry_warning_days);
    let query = doc! {
        "revoked": false,
        "expiry_warned": false,
        "expires_at": {"$ne": Bson::Null, "$gte": dt::to_bson(now), "$lte": dt::to_bson(horizon)},
    };
    let tokens: Vec<ApiToken> = db.collection::<ApiToken>(API_TOKENS).find(query).await?.try_collect().await?;
    if tokens.is_empty() {
        return Ok(0);
    }
    let mut ids: Vec<ObjectId> = Vec::new();
    for t in &tokens {
        if !ids.contains(&t.user_id) {
            ids.push(t.user_id);
        }
    }
    let users: HashMap<ObjectId, User> = db
        .collection::<User>(USERS)
        .find(doc! {"_id": {"$in": ids}})
        .await?
        .try_collect::<Vec<User>>()
        .await?
        .into_iter()
        .map(|u| (u.id, u))
        .collect();
    let tokens_coll = db.collection::<Document>(API_TOKENS);
    let mut sent = 0;
    for mut token in tokens {
        if let (Some(user), Some(expires_at)) = (users.get(&token.user_id), token.expires_at) {
            let left = days_left(dt::from_bson(expires_at), now);
            let notice = mail::token_expiring(&settings.app_name, &settings.dev_url, &token.name, &token.prefix, left);
            email::queue_notice(state, &user.email, &notice).await?;
            sent += 1;
        }
        token.expiry_warned = true;
        tokens_coll.update_one(doc! {"_id": token.id}, doc! {"$set": token_set(&token)}).upsert(true).await?;
    }
    if sent > 0 {
        tracing::info!("Sent {sent} token-expiry warning(s)");
    }
    Ok(sent)
}

/// `timedelta * 0.9`: exact, rounded half to even like CPython.
pub fn times_09(us: i64) -> i64 {
    const NUM: i128 = 8_106_479_329_266_893;
    const DEN: i128 = 9_007_199_254_740_992;
    let p = i128::from(us) * NUM;
    let (q, r) = (p.div_euclid(DEN), p.rem_euclid(DEN));
    let q = if 2 * r > DEN || (2 * r == DEN && q % 2 != 0) { q + 1 } else { q };
    q as i64
}

/// `timedelta(hours=h)` in microseconds, for an int or float setting.
pub fn hours_us(hours: &Value) -> Option<i64> {
    if let Some(h) = hours.as_i64() {
        return h.checked_mul(3_600_000_000);
    }
    hours.as_f64().map(|h| pyfloat::round_int(h * 3_600_000_000.0))
}

/// The setting as `f"{hours}"` prints it.
pub fn py_num_text(v: &Value) -> String {
    match v {
        Value::Number(n) if n.is_i64() || n.is_u64() => n.to_string(),
        Value::Number(n) => pyfloat::repr(n.as_f64().unwrap_or(0.0)),
        Value::Bool(b) => if *b { "True" } else { "False" }.into(),
        other => other.to_string(),
    }
}

pub fn digest_pipeline(since: DateTime<Utc>) -> Vec<Document> {
    vec![
        doc! {"$match": {"created_at": {"$gte": dt::to_bson(since)}, "status_code": 429}},
        doc! {"$group": {"_id": "$token_id", "count": {"$sum": 1}, "user_id": {"$first": "$user_id"}}},
        doc! {"$sort": {"count": -1}},
        doc! {"$limit": 50},
    ]
}

fn int_of(b: Option<&Bson>) -> i64 {
    match b {
        Some(Bson::Int32(n)) => i64::from(*n),
        Some(Bson::Int64(n)) => *n,
        Some(Bson::Double(f)) => *f as i64,
        _ => 0,
    }
}

async fn state_time(db: &Database, key: &str) -> anyhow::Result<Option<DateTime<Utc>>> {
    let Some(d) = db.collection::<Document>(STATE_COLLECTION).find_one(doc! {"_id": key}).await? else {
        return Ok(None);
    };
    match d.get("value") {
        Some(Bson::DateTime(v)) => Ok(Some(dt::from_bson(*v))),
        Some(Bson::Null) | None => Ok(None),
        Some(other) => anyhow::bail!("system_state {key} value is not a datetime: {other:?}"),
    }
}

/// False while the last digest is younger than 90% of the window.
pub fn digest_due(now: DateTime<Utc>, last: Option<DateTime<Utc>>, window_us: i64) -> bool {
    last.is_none_or(|last| (now - last).num_microseconds().unwrap_or(i64::MAX) >= times_09(window_us))
}

/// The digest lines: the account and token of each row, or the stand-ins
/// Python prints for a missing one.
pub fn digest_rows(
    rows: &[Document],
    emails: &HashMap<ObjectId, String>,
    names: &HashMap<ObjectId, (String, String)>,
) -> Vec<DigestRow> {
    let oid = |b: Option<&Bson>| match b {
        Some(Bson::ObjectId(o)) => Some(*o),
        _ => None,
    };
    rows.iter()
        .map(|r| {
            let email = oid(r.get("user_id")).and_then(|u| emails.get(&u).cloned()).unwrap_or_else(|| "unknown".into());
            let (token_name, token_prefix) = oid(r.get("_id"))
                .and_then(|t| names.get(&t).cloned())
                .unwrap_or_else(|| ("(deleted token)".into(), String::new()));
            DigestRow { count: int_of(r.get("count")), email, token_name, token_prefix }
        })
        .collect()
}

pub async fn daily_rate_limit_digest(state: &AppState) -> anyhow::Result<bool> {
    let settings = state.settings();
    let Some(to) = settings.rate_limit_alert_email.as_deref().filter(|s| !s.is_empty()) else {
        return Ok(false);
    };
    let hours = runtime_config::get_setting(state, "rate_limit_digest_window_hours").await?;
    let threshold = runtime_config::get_setting(state, "rate_limit_alert_threshold").await?;
    let window_us = hours_us(&hours).ok_or_else(|| anyhow::anyhow!("bad rate_limit_digest_window_hours {hours}"))?;
    let db = email::mongo(state)?;
    let now = dt::now();
    if !digest_due(now, state_time(db, DIGEST_KEY).await?, window_us) {
        return Ok(false);
    }
    let since = now - TimeDelta::microseconds(window_us);
    let rows: Vec<Document> =
        db.collection::<Document>(USAGE_EVENTS).aggregate(digest_pipeline(since)).await?.try_collect().await?;
    db.collection::<Document>(STATE_COLLECTION)
        .update_one(doc! {"_id": DIGEST_KEY}, doc! {"$set": {"value": dt::to_bson(now)}})
        .upsert(true)
        .await?;
    let total: i64 = rows.iter().map(|r| int_of(r.get("count"))).sum();
    let below = match threshold.as_i64() {
        Some(t) => total < t,
        None => (total as f64) < threshold.as_f64().unwrap_or(f64::INFINITY),
    };
    if below {
        return Ok(false);
    }
    let mut user_ids: Vec<Bson> = Vec::new();
    for r in &rows {
        let u = r.get("user_id").cloned().unwrap_or(Bson::Null);
        if !user_ids.contains(&u) {
            user_ids.push(u);
        }
    }
    let token_ids: Vec<Bson> = rows.iter().map(|r| r.get("_id").cloned().unwrap_or(Bson::Null)).collect();
    let emails: HashMap<ObjectId, String> = db
        .collection::<User>(USERS)
        .find(doc! {"_id": {"$in": user_ids}})
        .await?
        .try_collect::<Vec<User>>()
        .await?
        .into_iter()
        .map(|u| (u.id, u.email))
        .collect();
    let names: HashMap<ObjectId, (String, String)> = db
        .collection::<ApiToken>(API_TOKENS)
        .find(doc! {"_id": {"$in": token_ids}})
        .await?
        .try_collect::<Vec<ApiToken>>()
        .await?
        .into_iter()
        .map(|t| (t.id, (t.name, t.prefix)))
        .collect();
    let digest = digest_rows(&rows, &emails, &names);
    let hours_text = py_num_text(&hours);
    let (subject, text, html) = mail::rate_limit_digest(&settings.app_name, &hours_text, total, &digest);
    email::queue_email(state, to, &subject, &text, Some(&html)).await?;
    tracing::warn!("Queued rate-limit digest ({total} hits) to {to}");
    Ok(true)
}

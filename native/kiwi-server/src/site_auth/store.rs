//! The site-account documents as Beanie reads and writes them: `save()` is a
//! `$set` of every model field (nulls kept, ints as pymongo sizes them).

use chrono::{DateTime, Utc};
use mongodb::{
    Collection, Database,
    bson::{self, Bson, Document, doc, oid::ObjectId},
};
use serde_json::{Value, json};

use crate::{
    auth::{
        models::{SITE_SESSIONS, SITE_USERS, SessionRow, SiteUser},
        pyutil::bson_int,
    },
    dt,
    error::ApiError,
    state::AppState,
};

pub const USERNAME_REQUESTS: &str = "username_change_requests";

pub fn internal(err: impl std::fmt::Display) -> ApiError {
    tracing::error!("site_auth: {err}");
    ApiError::internal()
}

pub fn db(state: &AppState) -> Result<&Database, ApiError> {
    state.inner.db.mongo.as_ref().ok_or_else(|| internal("MONGO_URI not configured"))
}

pub fn coll(state: &AppState, name: &str) -> Result<Collection<Document>, ApiError> {
    Ok(db(state)?.collection::<Document>(name))
}

pub fn date(t: DateTime<Utc>) -> Bson {
    Bson::DateTime(dt::to_bson(t))
}

fn opt_date(t: Option<bson::DateTime>) -> Bson {
    t.map_or(Bson::Null, Bson::DateTime)
}

fn opt_str(s: &Option<String>) -> Bson {
    s.as_ref().map_or(Bson::Null, |s| Bson::String(s.clone()))
}

/// `dict[str, float]` as pydantic coerces it before Beanie writes it back.
fn float_map(d: &Document) -> Document {
    d.iter()
        .map(|(k, v)| {
            let v = match v {
                Bson::Int32(i) => Bson::Double(f64::from(*i)),
                Bson::Int64(i) => Bson::Double(*i as f64),
                other => other.clone(),
            };
            (k.clone(), v)
        })
        .collect()
}

/// The `SiteUser` document in model order (`_id` first).
pub fn user_doc(u: &SiteUser) -> Document {
    doc! {
        "_id": u.id,
        "username": &u.username,
        "discord_handle": &u.discord_handle,
        "notify_email": opt_str(&u.notify_email),
        "display_name": opt_str(&u.display_name),
        "is_active": u.is_active,
        "is_verified": u.is_verified,
        "is_deleted": u.is_deleted,
        "discord_id": u.discord_id.map_or(Bson::Null, bson_int),
        "discord_avatar": opt_str(&u.discord_avatar),
        "token_version": bson_int(u.token_version),
        "claimed_trove_name": opt_str(&u.claimed_trove_name),
        "claimed_trove_display": opt_str(&u.claimed_trove_display),
        "claimed_at": opt_date(u.claimed_at),
        "claim_verified": u.claim_verified,
        "claim_baseline": float_map(&u.claim_baseline),
        "claim_verified_at": opt_date(u.claim_verified_at),
        "creator_token_hash": opt_str(&u.creator_token_hash),
        "creator_token_prefix": opt_str(&u.creator_token_prefix),
        "creator_token_at": opt_date(u.creator_token_at),
        "mod_issues_seen_at": opt_date(u.mod_issues_seen_at),
        "created_at": u.created_at,
        "updated_at": u.updated_at,
        "last_login_at": opt_date(u.last_login_at),
    }
}

/// Beanie's `save()` of a `SiteUser`.
pub async fn save_user(state: &AppState, u: &SiteUser) -> Result<(), ApiError> {
    coll(state, SITE_USERS)?
        .update_one(doc! { "_id": u.id }, doc! { "$set": user_doc(u) })
        .upsert(true)
        .await
        .map_err(internal)?;
    Ok(())
}

pub async fn find_user(state: &AppState, filter: Document) -> Result<Option<SiteUser>, ApiError> {
    db(state)?.collection::<SiteUser>(SITE_USERS).find_one(filter).await.map_err(internal)
}

/// `SiteUser.find_one(SiteUser.username == name) is not None`.
pub async fn username_taken(state: &AppState, name: &str) -> Result<Option<ObjectId>, ApiError> {
    Ok(find_user(state, doc! { "username": name }).await?.map(|u| u.id))
}

/// Beanie's `save()` of a `SiteSession`.
pub async fn save_session(state: &AppState, s: &SessionRow) -> Result<(), ApiError> {
    let set = doc! {
        "_id": s.id,
        "site_user_id": s.owner_id,
        "refresh_token_hash": &s.refresh_token_hash,
        "device": opt_str(&s.device),
        "created_at": s.created_at,
        "last_used_at": s.last_used_at,
        "expires_at": s.expires_at,
        "revoked": s.revoked,
    };
    coll(state, SITE_SESSIONS)?
        .update_one(doc! { "_id": s.id }, doc! { "$set": set })
        .upsert(true)
        .await
        .map_err(internal)?;
    Ok(())
}

fn pydantic_dt(t: Option<bson::DateTime>) -> Value {
    t.map_or(Value::Null, |t| Value::String(dt::pydantic(&dt::from_bson(t))))
}

/// `_discord_avatar_url`.
pub fn avatar_url(u: &SiteUser) -> Option<String> {
    let id = u.discord_id?;
    Some(match u.discord_avatar.as_deref().filter(|a| !a.is_empty()) {
        Some(hash) => {
            let ext = if hash.starts_with("a_") { "gif" } else { "png" };
            format!("https://cdn.discordapp.com/avatars/{id}/{hash}.{ext}?size=128")
        }
        None => format!("https://cdn.discordapp.com/embed/avatars/{}.png", (id >> 22).rem_euclid(6)),
    })
}

/// `_to_public(user)` as `SiteUserPublic`; `claimed_at` overrides the stored
/// value with one set during this request (microseconds, not yet rounded).
pub fn to_public(u: &SiteUser, claimed_at: Option<DateTime<Utc>>) -> Value {
    let claimed_at = match claimed_at {
        Some(t) => Value::String(dt::pydantic(&t)),
        None => pydantic_dt(u.claimed_at),
    };
    let handle = if u.discord_handle.is_empty() { &u.username } else { &u.discord_handle };
    json!({
        "id": u.id.to_hex(),
        "username": u.username,
        "discord_handle": handle,
        "notify_email": u.notify_email,
        "display_name": u.display_name,
        "avatar_url": avatar_url(u),
        "is_active": u.is_active,
        "is_verified": u.is_verified,
        "claimed_trove_name": u.claimed_trove_name,
        "claimed_trove_display": u.claimed_trove_display,
        "claimed_at": claimed_at,
        "claim_verified": u.claim_verified,
        "claim_verified_at": pydantic_dt(u.claim_verified_at),
        "claim_baseline_board_count": u.claim_baseline.len(),
        "created_at": dt::pydantic(&dt::from_bson(u.created_at)),
        "last_login_at": pydantic_dt(u.last_login_at),
    })
}

/// `iso(dt)`: `isoformat()` or null.
pub fn iso(t: Option<bson::DateTime>) -> Value {
    t.map_or(Value::Null, |t| Value::String(dt::isoformat(&dt::from_bson(t))))
}

/// Python truthiness of a JSON value.
pub fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64() != Some(0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// `secrets.token_hex(n)`.
pub fn token_hex(n: usize) -> String {
    let mut buf = vec![0u8; n];
    getrandom::fill(&mut buf).expect("OS RNG");
    hex::encode(buf)
}

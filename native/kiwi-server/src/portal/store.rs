//! Beanie `insert()` / `save()` of the portal documents: an insert writes every
//! field in model order (`_id` first, explicit nulls), a save `$set`s every
//! field of the in-memory model with upsert, so legacy rows gain missing
//! fields exactly as Python's save gives them.

use chrono::{DateTime, Utc};
use futures::TryStreamExt;
use mongodb::{
    Collection, Database,
    bson::{Bson, DateTime as BsonDateTime, Document, doc, oid::ObjectId},
};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::{
    auth::{
        Rejection,
        models::{API_TOKENS, ApiToken, SESSIONS, SessionRow, USERS, User},
        pyutil::bson_int,
        scopes,
    },
    dt,
    state::AppState,
};

pub fn db(state: &AppState) -> Result<&Database, Rejection> {
    state.inner.db.mongo.as_ref().ok_or_else(|| Rejection::internal("MONGO_URI not configured"))
}

pub fn coll(state: &AppState, name: &str) -> Result<Collection<Document>, Rejection> {
    Ok(db(state)?.collection(name))
}

pub async fn find_one<T: DeserializeOwned + Send + Sync>(
    state: &AppState,
    name: &str,
    filter: Document,
) -> Result<Option<T>, Rejection> {
    db(state)?.collection::<T>(name).find_one(filter).await.map_err(Rejection::internal)
}

pub async fn find_all<T: DeserializeOwned + Send + Sync>(
    state: &AppState,
    name: &str,
    filter: Document,
) -> Result<Vec<T>, Rejection> {
    let cur = db(state)?.collection::<T>(name).find(filter).await.map_err(Rejection::internal)?;
    cur.try_collect().await.map_err(Rejection::internal)
}

pub async fn user_by_id(state: &AppState, id: ObjectId) -> Result<Option<User>, Rejection> {
    find_one(state, USERS, doc! { "_id": id }).await
}

pub async fn user_by_email(state: &AppState, email: &str) -> Result<Option<User>, Rejection> {
    find_one(state, USERS, doc! { "email": email }).await
}

fn opt_date(d: Option<BsonDateTime>) -> Bson {
    d.map_or(Bson::Null, Bson::DateTime)
}

fn opt_str(s: &Option<String>) -> Bson {
    s.as_ref().map_or(Bson::Null, |s| Bson::String(s.clone()))
}

/// `User` fields in model order, without `_id`.
pub fn user_fields(u: &User) -> Document {
    doc! {
        "email": &u.email,
        "hashed_password": &u.hashed_password,
        "display_name": opt_str(&u.display_name),
        "is_active": u.is_active,
        "is_verified": u.is_verified,
        "is_superuser": u.is_superuser,
        "roles": u.roles.clone(),
        "token_version": bson_int(u.token_version),
        "github_id": u.github_id.map_or(Bson::Null, bson_int),
        "email_bounced": u.email_bounced,
        "created_at": u.created_at,
        "updated_at": u.updated_at,
        "last_login_at": opt_date(u.last_login_at),
    }
}

fn with_id(id: ObjectId, fields: Document) -> Document {
    let mut d = doc! { "_id": id };
    d.extend(fields);
    d
}

async fn insert(state: &AppState, name: &str, d: Document) -> Result<(), Rejection> {
    coll(state, name)?.insert_one(d).await.map_err(Rejection::internal)?;
    Ok(())
}

async fn save(state: &AppState, name: &str, id: ObjectId, fields: Document) -> Result<(), Rejection> {
    coll(state, name)?
        .update_one(doc! { "_id": id }, doc! { "$set": fields })
        .upsert(true)
        .await
        .map_err(Rejection::internal)?;
    Ok(())
}

pub async fn insert_user(state: &AppState, u: &User) -> Result<(), Rejection> {
    insert(state, USERS, with_id(u.id, user_fields(u))).await
}

pub async fn save_user(state: &AppState, u: &User) -> Result<(), Rejection> {
    save(state, USERS, u.id, user_fields(u)).await
}

/// `ApiToken` fields in model order, without `_id`.
pub fn token_fields(t: &ApiToken) -> Document {
    doc! {
        "user_id": t.user_id,
        "name": &t.name,
        "prefix": &t.prefix,
        "hashed_token": &t.hashed_token,
        "scopes": bson_int(t.scopes),
        "ip_salt": opt_str(&t.ip_salt),
        "allowed_ip_hashes": t.allowed_ip_hashes.clone(),
        "revoked": t.revoked,
        "revoked_at": opt_date(t.revoked_at),
        "revoke_reason": opt_str(&t.revoke_reason),
        "expires_at": opt_date(t.expires_at),
        "expiry_warned": t.expiry_warned,
        "created_at": t.created_at,
        "last_used_at": opt_date(t.last_used_at),
        "rotated_at": opt_date(t.rotated_at),
        "request_count": bson_int(t.request_count),
    }
}

pub async fn insert_token(state: &AppState, t: &ApiToken) -> Result<(), Rejection> {
    insert(state, API_TOKENS, with_id(t.id, token_fields(t))).await
}

pub async fn save_token(state: &AppState, t: &ApiToken) -> Result<(), Rejection> {
    save(state, API_TOKENS, t.id, token_fields(t)).await
}

/// `Session` fields in model order, without `_id`.
pub fn session_fields(s: &SessionRow) -> Document {
    doc! {
        "user_id": s.owner_id,
        "refresh_token_hash": &s.refresh_token_hash,
        "device": opt_str(&s.device),
        "created_at": s.created_at,
        "last_used_at": s.last_used_at,
        "expires_at": s.expires_at,
        "revoked": s.revoked,
    }
}

pub async fn save_session(state: &AppState, s: &SessionRow) -> Result<(), Rejection> {
    save(state, SESSIONS, s.id, session_fields(s)).await
}

pub fn bson_now(now: DateTime<Utc>) -> BsonDateTime {
    dt::to_bson(now)
}

pub fn pyd(d: BsonDateTime) -> String {
    dt::pydantic(&dt::from_bson(d))
}

pub fn pyd_opt(d: Option<BsonDateTime>) -> Value {
    d.map_or(Value::Null, |d| Value::String(pyd(d)))
}

/// `revoke_all_sessions(user)`: bump `token_version` (every access token dies),
/// save the whole user (so pending in-memory edits land too), then revoke
/// every live session.
pub async fn revoke_all_sessions(state: &AppState, user: &mut User) -> Result<(), Rejection> {
    user.token_version += 1;
    user.updated_at = bson_now(dt::now());
    save_user(state, user).await?;
    coll(state, SESSIONS)?
        .update_many(doc! { "user_id": user.id, "revoked": false }, doc! { "$set": { "revoked": true } })
        .await
        .map_err(Rejection::internal)?;
    Ok(())
}

/// `UserPublic`; `created_at` is passed in so a fresh insert keeps microseconds.
pub fn user_public(u: &User, created_at: DateTime<Utc>) -> Value {
    json!({
        "id": u.id.to_hex(),
        "email": u.email,
        "display_name": u.display_name,
        "is_active": u.is_active,
        "is_verified": u.is_verified,
        "is_superuser": u.is_superuser,
        "roles": u.roles,
        "created_at": dt::pydantic(&created_at),
        "last_login_at": pyd_opt(u.last_login_at),
    })
}

/// `TokenPublic`, with the datetimes given as text (fresh inserts keep microseconds).
pub struct TokenTimes {
    pub created_at: String,
    pub expires_at: Value,
}

impl TokenTimes {
    pub fn stored(t: &ApiToken) -> Self {
        TokenTimes { created_at: pyd(t.created_at), expires_at: pyd_opt(t.expires_at) }
    }
}

pub fn token_public(t: &ApiToken, times: TokenTimes) -> Value {
    json!({
        "id": t.id.to_hex(),
        "name": t.name,
        "prefix": t.prefix,
        "scopes": t.scopes,
        "scope_names": scopes::decode(t.scopes),
        "allowed_ip_count": t.allowed_ip_hashes.len(),
        "revoked": t.revoked,
        "revoked_at": pyd_opt(t.revoked_at),
        "revoke_reason": t.revoke_reason,
        "created_at": times.created_at,
        "last_used_at": pyd_opt(t.last_used_at),
        "rotated_at": pyd_opt(t.rotated_at),
        "expires_at": times.expires_at,
        "request_count": t.request_count,
    })
}

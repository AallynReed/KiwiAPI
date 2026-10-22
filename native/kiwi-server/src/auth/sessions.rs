//! Refresh-token sessions for both account systems: `app/auth/sessions.py`
//! (dev portal: `sessions` / `users`) and `app/site_auth/sessions.py` (site:
//! `site_sessions` / `site_users`, access tokens carry `kind: "site"`).

use chrono::{Duration, Utc};
use mongodb::bson::{doc, oid::ObjectId, DateTime, Document};
use serde::Serialize;

use super::{
    jwt,
    models::{SessionRow, SESSIONS, SITE_SESSIONS, SITE_USERS, USERS},
    pyutil::bson_int,
    tokens::{generate_refresh_token, hash_token},
    Rejection,
};
use crate::state::AppState;

pub const SITE_TOKEN_KIND: &str = "site";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Audience {
    Portal,
    Site,
}

impl Audience {
    pub fn sessions(self) -> &'static str {
        match self {
            Audience::Portal => SESSIONS,
            Audience::Site => SITE_SESSIONS,
        }
    }

    pub fn users(self) -> &'static str {
        match self {
            Audience::Portal => USERS,
            Audience::Site => SITE_USERS,
        }
    }

    pub fn owner_field(self) -> &'static str {
        match self {
            Audience::Portal => "user_id",
            Audience::Site => "site_user_id",
        }
    }
}

/// `TokenResponse` / `SiteTokenResponse`, fields in model order.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TokenPair {
    pub access_token: String,
    pub refresh_token: String,
    pub token_type: &'static str,
    pub expires_in: i64,
}

/// `create_access_token` / `create_site_access_token` at the current time.
pub fn create_access_token(aud: Audience, subject: &str, token_version: i64, session_id: Option<&str>) -> String {
    let s = super::auth_settings();
    let ttl = Duration::minutes(s.access_token_expire_minutes);
    match aud {
        Audience::Portal => {
            jwt::create_access_token_at(&s.secret_key, subject, token_version, session_id, ttl, Utc::now())
        }
        Audience::Site => {
            jwt::create_site_access_token_at(&s.secret_key, subject, token_version, session_id, ttl, Utc::now())
        }
    }
}

fn mongo(state: &AppState) -> Result<&mongodb::Database, Rejection> {
    state.inner.db.mongo.as_ref().ok_or_else(|| Rejection::internal("MONGO_URI not configured"))
}

/// `issue_tokens(user, request)`: insert a session row, mint access + refresh.
pub async fn issue_tokens(
    state: &AppState,
    aud: Audience,
    user_id: ObjectId,
    token_version: i64,
    device: Option<String>,
) -> Result<TokenPair, Rejection> {
    let s = super::auth_settings();
    let (refresh, refresh_hash) = generate_refresh_token();
    let now = Utc::now();
    let id = ObjectId::new();
    let row = doc! {
        "_id": id,
        aud.owner_field(): user_id,
        "refresh_token_hash": refresh_hash,
        "device": device,
        "created_at": DateTime::from_millis(now.timestamp_millis()),
        "last_used_at": DateTime::from_millis(now.timestamp_millis()),
        "expires_at": DateTime::from_millis((now + Duration::days(s.refresh_token_expire_days)).timestamp_millis()),
        "revoked": false,
    };
    mongo(state)?.collection::<Document>(aud.sessions()).insert_one(row).await.map_err(Rejection::internal)?;
    Ok(TokenPair {
        access_token: create_access_token(aud, &user_id.to_hex(), token_version, Some(&id.to_hex())),
        refresh_token: refresh,
        token_type: "bearer",
        expires_in: s.access_token_expire_minutes * 60,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RotateError {
    /// Unknown, revoked or expired refresh token.
    InvalidToken,
    /// The owning account is gone or inactive.
    InactiveUser,
}

/// `rotate` / the body of `POST /auth/refresh`: single-use refresh rotation.
pub async fn rotate(
    state: &AppState,
    aud: Audience,
    refresh_token: &str,
    device: Option<String>,
) -> Result<Result<TokenPair, RotateError>, Rejection> {
    let db = mongo(state)?;
    let sessions = db.collection::<SessionRow>(aud.sessions());
    let found = sessions
        .find_one(doc! { "refresh_token_hash": hash_token(refresh_token) })
        .await
        .map_err(Rejection::internal)?;
    let now = DateTime::now();
    let Some(session) = found.filter(|s| !s.revoked && s.expires_at >= now) else {
        return Ok(Err(RotateError::InvalidToken));
    };
    let user = db
        .collection::<Document>(aud.users())
        .find_one(doc! { "_id": session.owner_id })
        .await
        .map_err(Rejection::internal)?;
    let Some(user) = user.filter(|u| u.get_bool("is_active").unwrap_or(true)) else {
        return Ok(Err(RotateError::InactiveUser));
    };
    let token_version = user.get("token_version").and_then(bson_as_i64).unwrap_or(0);

    let (refresh, refresh_hash) = generate_refresh_token();
    sessions
        .update_one(
            doc! { "_id": session.id },
            doc! { "$set": { "refresh_token_hash": refresh_hash, "last_used_at": now, "device": device } },
        )
        .await
        .map_err(Rejection::internal)?;
    let s = super::auth_settings();
    Ok(Ok(TokenPair {
        access_token: create_access_token(aud, &session.owner_id.to_hex(), token_version, Some(&session.id.to_hex())),
        refresh_token: refresh,
        token_type: "bearer",
        expires_in: s.access_token_expire_minutes * 60,
    }))
}

/// `revoke_by_refresh_token` / `POST /auth/logout`: idempotent.
pub async fn revoke_by_refresh_token(state: &AppState, aud: Audience, refresh_token: &str) -> Result<(), Rejection> {
    if refresh_token.is_empty() {
        return Ok(());
    }
    mongo(state)?
        .collection::<Document>(aud.sessions())
        .update_one(
            doc! { "refresh_token_hash": hash_token(refresh_token), "revoked": false },
            doc! { "$set": { "revoked": true } },
        )
        .await
        .map_err(Rejection::internal)?;
    Ok(())
}

/// `revoke_all_sessions(user)`: bump `token_version` (kills every access
/// token at once) and revoke every live session row.
pub async fn revoke_all_sessions(
    state: &AppState,
    aud: Audience,
    user_id: ObjectId,
    current_version: i64,
) -> Result<i64, Rejection> {
    let db = mongo(state)?;
    let next = current_version + 1;
    db.collection::<Document>(aud.users())
        .update_one(
            doc! { "_id": user_id },
            doc! { "$set": { "token_version": bson_int(next), "updated_at": DateTime::now() } },
        )
        .await
        .map_err(Rejection::internal)?;
    db.collection::<Document>(aud.sessions())
        .update_many(doc! { aud.owner_field(): user_id, "revoked": false }, doc! { "$set": { "revoked": true } })
        .await
        .map_err(Rejection::internal)?;
    Ok(next)
}

fn bson_as_i64(b: &mongodb::bson::Bson) -> Option<i64> {
    b.as_i64().or_else(|| b.as_i32().map(i64::from))
}

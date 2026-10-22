//! The documents the admin routes load and save, the way Beanie does it: a
//! load validates (a document it can't validate is a 500), `save()` `$set`s
//! every model field in order (`_id` first, Python ints as int32 when they
//! fit, explicit nulls) and the in-memory copy then holds what was stored.

use chrono::Utc;
use futures::TryStreamExt;
use mongodb::{
    Collection,
    bson::{self, Bson, DateTime, Document, doc, oid::ObjectId},
};
use serde::Deserialize;

use super::internal;
use crate::{
    auth::{
        models::{ApiToken, SiteUser, User},
        pyutil::bson_int,
    },
    dt,
    error::ApiError,
};

pub const USERNAME_REQUESTS: &str = "username_change_requests";
pub const MOD_PROJECTS: &str = "mod_projects";
pub const MODPACK_PROJECTS: &str = "modpack_projects";

/// `utcnow()` as it reads back after a write.
pub fn now() -> DateTime {
    DateTime::from_millis(Utc::now().timestamp_millis())
}

pub fn load<T: for<'de> Deserialize<'de>>(d: Document) -> Result<T, ApiError> {
    bson::from_document(d).map_err(internal)
}

pub async fn find_all<T: for<'de> Deserialize<'de>>(
    c: &Collection<Document>,
    filter: Document,
    sort: Option<Document>,
) -> Result<Vec<T>, ApiError> {
    let mut find = c.find(filter);
    if let Some(s) = sort {
        find = find.sort(s);
    }
    let docs: Vec<Document> = find.await.map_err(internal)?.try_collect().await.map_err(internal)?;
    docs.into_iter().map(load).collect()
}

pub async fn find_one<T: for<'de> Deserialize<'de>>(
    c: &Collection<Document>,
    filter: Document,
) -> Result<Option<T>, ApiError> {
    match c.find_one(filter).await.map_err(internal)? {
        Some(d) => load(d).map(Some),
        None => Ok(None),
    }
}

/// Beanie's `save()`: `$set` of the whole document, upserting.
pub async fn save(c: &Collection<Document>, id: ObjectId, d: Document) -> Result<(), ApiError> {
    c.update_one(doc! {"_id": id}, doc! {"$set": d}).upsert(true).await.map_err(internal)?;
    Ok(())
}

/// pydantic `EmailStr`: email-validator's normalized form (domain lowercased).
pub fn normalize_email(email: &str) -> String {
    match email.rsplit_once('@') {
        Some((local, domain)) => format!("{local}@{}", domain.to_lowercase()),
        None => email.to_owned(),
    }
}

fn opt_str(v: &Option<String>) -> Bson {
    v.as_ref().map_or(Bson::Null, |s| Bson::String(s.clone()))
}

fn opt_dt(v: &Option<DateTime>) -> Bson {
    v.map_or(Bson::Null, Bson::DateTime)
}

fn opt_int(v: Option<i64>) -> Bson {
    v.map_or(Bson::Null, bson_int)
}

/// `app.auth.models.User` as `save()` writes it.
pub fn user_doc(u: &User) -> Document {
    doc! {
        "_id": u.id,
        "email": normalize_email(&u.email),
        "hashed_password": &u.hashed_password,
        "display_name": opt_str(&u.display_name),
        "is_active": u.is_active,
        "is_verified": u.is_verified,
        "is_superuser": u.is_superuser,
        "roles": u.roles.clone(),
        "token_version": bson_int(u.token_version),
        "github_id": opt_int(u.github_id),
        "email_bounced": u.email_bounced,
        "created_at": u.created_at,
        "updated_at": u.updated_at,
        "last_login_at": opt_dt(&u.last_login_at),
    }
}

/// `app.tokens.models.ApiToken` as `save()` writes it.
pub fn token_doc(t: &ApiToken) -> Document {
    doc! {
        "_id": t.id,
        "user_id": t.user_id,
        "name": &t.name,
        "prefix": &t.prefix,
        "hashed_token": &t.hashed_token,
        "scopes": bson_int(t.scopes),
        "ip_salt": opt_str(&t.ip_salt),
        "allowed_ip_hashes": t.allowed_ip_hashes.clone(),
        "revoked": t.revoked,
        "revoked_at": opt_dt(&t.revoked_at),
        "revoke_reason": opt_str(&t.revoke_reason),
        "expires_at": opt_dt(&t.expires_at),
        "expiry_warned": t.expiry_warned,
        "created_at": t.created_at,
        "last_used_at": opt_dt(&t.last_used_at),
        "rotated_at": opt_dt(&t.rotated_at),
        "request_count": bson_int(t.request_count),
    }
}

/// `claim_baseline: dict[str, float]`: pydantic's lax float of each value.
pub fn float_map(d: &Document) -> Result<Document, ApiError> {
    let mut out = Document::new();
    for (k, v) in d {
        let f = match v {
            Bson::Double(f) => *f,
            Bson::Int32(n) => f64::from(*n),
            Bson::Int64(n) => *n as f64,
            Bson::Boolean(b) => f64::from(u8::from(*b)),
            Bson::String(s) => kiwi_core::pystr::strip(s).parse::<f64>().map_err(internal)?,
            other => return Err(internal(format!("claim_baseline.{k}: {other}"))),
        };
        out.insert(k.clone(), f);
    }
    Ok(out)
}

/// `app.site_auth.models.SiteUser` as `save()` writes it.
pub fn site_user_doc(u: &SiteUser) -> Result<Document, ApiError> {
    Ok(doc! {
        "_id": u.id,
        "username": &u.username,
        "discord_handle": &u.discord_handle,
        "notify_email": opt_str(&u.notify_email),
        "display_name": opt_str(&u.display_name),
        "is_active": u.is_active,
        "is_verified": u.is_verified,
        "is_deleted": u.is_deleted,
        "discord_id": opt_int(u.discord_id),
        "discord_avatar": opt_str(&u.discord_avatar),
        "token_version": bson_int(u.token_version),
        "claimed_trove_name": opt_str(&u.claimed_trove_name),
        "claimed_trove_display": opt_str(&u.claimed_trove_display),
        "claimed_at": opt_dt(&u.claimed_at),
        "claim_verified": u.claim_verified,
        "claim_baseline": float_map(&u.claim_baseline)?,
        "claim_verified_at": opt_dt(&u.claim_verified_at),
        "creator_token_hash": opt_str(&u.creator_token_hash),
        "creator_token_prefix": opt_str(&u.creator_token_prefix),
        "creator_token_at": opt_dt(&u.creator_token_at),
        "mod_issues_seen_at": opt_dt(&u.mod_issues_seen_at),
        "created_at": u.created_at,
        "updated_at": u.updated_at,
        "last_login_at": opt_dt(&u.last_login_at),
    })
}

/// `SiteUser.clear_claim()`.
pub fn clear_claim(u: &mut SiteUser) {
    u.claimed_trove_name = None;
    u.claimed_trove_display = None;
    u.claimed_at = None;
    u.claim_verified = false;
    u.claim_verified_at = None;
    u.claim_baseline = Document::new();
}

fn pending() -> String {
    "pending".into()
}

/// `app.site_auth.models.UsernameChangeRequest`.
#[derive(Debug, Clone, Deserialize)]
pub struct UsernameRequest {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub site_user_id: ObjectId,
    pub current_username: String,
    pub requested_username: String,
    #[serde(default = "pending")]
    pub status: String,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub resolved_by: Option<ObjectId>,
    #[serde(default)]
    pub resolved_at: Option<DateTime>,
    #[serde(default = "now")]
    pub created_at: DateTime,
}

pub const REQUEST_STATUSES: [&str; 3] = ["pending", "approved", "rejected"];

impl UsernameRequest {
    /// Beanie's load: the `Literal` status is validated too.
    pub fn from_doc(d: Document) -> Result<Self, ApiError> {
        let r: UsernameRequest = load(d)?;
        if !REQUEST_STATUSES.contains(&r.status.as_str()) {
            return Err(internal(format!("username request {}: status {:?}", r.id, r.status)));
        }
        Ok(r)
    }

    pub fn to_doc(&self) -> Document {
        doc! {
            "_id": self.id,
            "site_user_id": self.site_user_id,
            "current_username": &self.current_username,
            "requested_username": &self.requested_username,
            "status": &self.status,
            "reason": &self.reason,
            "resolved_by": self.resolved_by.map_or(Bson::Null, Bson::ObjectId),
            "resolved_at": opt_dt(&self.resolved_at),
            "created_at": self.created_at,
        }
    }
}

/// A BSON date as the app hands it out: pydantic's `...Z` form.
pub fn pyd(d: &DateTime) -> String {
    dt::pydantic(&dt::from_bson(*d))
}

pub fn pyd_opt(d: &Option<DateTime>) -> serde_json::Value {
    d.as_ref().map_or(serde_json::Value::Null, |d| pyd(d).into())
}

/// `iso(dt)`: `datetime.isoformat()` (`+00:00`), `None` for a missing date.
pub fn iso_opt(d: &Option<DateTime>) -> serde_json::Value {
    d.as_ref().map_or(serde_json::Value::Null, |d| dt::isoformat(&dt::from_bson(*d)).into())
}

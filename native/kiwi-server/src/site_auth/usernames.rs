//! `app/site_auth/usernames.py`: the request to change the frozen Trove
//! username (a master reviews it in the admin area).

use axum::{extract::State, http::StatusCode};
use futures::TryStreamExt;
use kiwi_core::pystr;
use mongodb::bson::{self, Bson, Document, doc, oid::ObjectId};
use serde::Deserialize;
use serde_json::{Value, json};

use super::store::{USERNAME_REQUESTS, coll, internal, iso, username_taken};
use crate::{
    auth::{CurrentSiteUser, models::SiteUser},
    dt,
    error::{ApiError, ErrorCode},
    pyjson::PydanticJson,
    state::AppState,
    validation::{DepsThenBody, Field, FromBody},
};

pub const RESERVED: [&str; 20] = [
    "stray",
    "why",
    "admin",
    "administrator",
    "api",
    "me",
    "mods",
    "modpacks",
    "trovesaurus",
    "kiwi",
    "support",
    "login",
    "logout",
    "dashboard",
    "system",
    "moderator",
    "staff",
    "root",
    "null",
    "undefined",
];

const FORMAT: &str = "Username must be 3-24 characters using lowercase letters, numbers, underscores or periods \
                      - a period can't start it or repeat (\"..\").";

fn bad(status: StatusCode, message: &str) -> ApiError {
    ApiError::new(status, ErrorCode::BadRequest, message)
}

/// `normalize_username`.
pub fn normalize(raw: &str) -> Result<String, ApiError> {
    let name = pystr::strip(raw).to_lowercase();
    let n = name.chars().count();
    let charset = name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '.');
    if !(3..=24).contains(&n) || !charset || name.starts_with('.') || name.contains("..") {
        return Err(bad(StatusCode::BAD_REQUEST, FORMAT));
    }
    if RESERVED.contains(&name.as_str()) {
        return Err(bad(StatusCode::CONFLICT, "That username is reserved."));
    }
    Ok(name)
}

/// `UsernameChangeRequest` as Beanie loads it.
#[derive(Debug, Clone, Deserialize)]
pub struct Request {
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
    pub resolved_at: Option<bson::DateTime>,
    pub created_at: bson::DateTime,
}

fn pending() -> String {
    "pending".into()
}

impl Request {
    /// The document in model order (`_id` first), as insert and `save()` write it.
    pub fn to_doc(&self) -> Document {
        doc! {
            "_id": self.id,
            "site_user_id": self.site_user_id,
            "current_username": &self.current_username,
            "requested_username": &self.requested_username,
            "status": &self.status,
            "reason": &self.reason,
            "resolved_by": self.resolved_by.map_or(Bson::Null, Bson::ObjectId),
            "resolved_at": self.resolved_at.map_or(Bson::Null, Bson::DateTime),
            "created_at": self.created_at,
        }
    }
}

/// `username_request_dto`; `created_at` overrides a value set this request.
pub fn dto(r: &Request, created_at: Option<chrono::DateTime<chrono::Utc>>) -> Value {
    let created = match created_at {
        Some(t) => Value::String(dt::isoformat(&t)),
        None => iso(Some(r.created_at)),
    };
    json!({
        "id": r.id.to_hex(),
        "site_user_id": r.site_user_id.to_hex(),
        "current_username": r.current_username,
        "requested_username": r.requested_username,
        "status": r.status,
        "reason": r.reason,
        "created_at": created,
        "resolved_at": iso(r.resolved_at),
    })
}

async fn find_one(state: &AppState, filter: Document) -> Result<Option<Request>, ApiError> {
    match coll(state, USERNAME_REQUESTS)?.find_one(filter).await.map_err(internal)? {
        Some(d) => bson::from_document(d).map(Some).map_err(internal),
        None => Ok(None),
    }
}

async fn save(state: &AppState, r: &Request) -> Result<(), ApiError> {
    coll(state, USERNAME_REQUESTS)?
        .update_one(doc! { "_id": r.id }, doc! { "$set": r.to_doc() })
        .upsert(true)
        .await
        .map_err(internal)?;
    Ok(())
}

/// `request_change`: create or replace the caller's pending request.
async fn request_change(
    state: &AppState,
    user: &SiteUser,
    raw: &str,
) -> Result<(Request, chrono::DateTime<chrono::Utc>), ApiError> {
    let name = normalize(raw)?;
    if name == user.username {
        return Err(bad(StatusCode::BAD_REQUEST, "That's already your username."));
    }
    if username_taken(state, &name).await?.is_some_and(|id| id != user.id) {
        return Err(bad(StatusCode::CONFLICT, "That username is already taken."));
    }
    let other = find_one(state, doc! { "requested_username": &name, "status": "pending" }).await?;
    if other.is_some_and(|o| o.site_user_id != user.id) {
        return Err(bad(StatusCode::CONFLICT, "Someone else has a pending request for that username."));
    }
    let now = dt::now();
    if let Some(mut existing) = find_one(state, doc! { "site_user_id": user.id, "status": "pending" }).await? {
        existing.requested_username = name;
        existing.current_username = user.username.clone();
        existing.created_at = dt::to_bson(now);
        save(state, &existing).await?;
        return Ok((existing, now));
    }
    let req = Request {
        id: ObjectId::new(),
        site_user_id: user.id,
        current_username: user.username.clone(),
        requested_username: name,
        status: pending(),
        reason: String::new(),
        resolved_by: None,
        resolved_at: None,
        created_at: dt::to_bson(now),
    };
    coll(state, USERNAME_REQUESTS)?.insert_one(req.to_doc()).await.map_err(internal)?;
    Ok((req, now))
}

pub struct UsernameBody {
    pub username: String,
}

impl FromBody for UsernameBody {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| Some(UsernameBody { username: m.field("username").str().min_len(3).max_len(24).required() }))
            .required()
    }
}

/// `GET /v1/site-auth/me/username-request`.
pub async fn get_request(
    State(state): State<AppState>,
    CurrentSiteUser(user): CurrentSiteUser,
) -> Result<PydanticJson, ApiError> {
    let latest = coll(&state, USERNAME_REQUESTS)?
        .find(doc! { "site_user_id": user.id })
        .sort(doc! { "created_at": -1 })
        .limit(1)
        .await
        .map_err(internal)?
        .try_next()
        .await
        .map_err(internal)?;
    let request = match latest {
        Some(d) => dto(&bson::from_document(d).map_err(internal)?, None),
        None => Value::Null,
    };
    Ok(PydanticJson(json!({ "request": request })))
}

/// `POST /v1/site-auth/me/username-request`.
pub async fn post_request(
    State(state): State<AppState>,
    DepsThenBody(CurrentSiteUser(user), body): DepsThenBody<CurrentSiteUser, UsernameBody>,
) -> Result<PydanticJson, ApiError> {
    let (req, now) = request_change(&state, &user, &body.username).await?;
    Ok(PydanticJson(json!({ "request": dto(&req, Some(now)) })))
}

/// `DELETE /v1/site-auth/me/username-request`.
pub async fn delete_request(
    State(state): State<AppState>,
    CurrentSiteUser(user): CurrentSiteUser,
) -> Result<PydanticJson, ApiError> {
    if let Some(req) = find_one(&state, doc! { "site_user_id": user.id, "status": "pending" }).await? {
        coll(&state, USERNAME_REQUESTS)?.delete_one(doc! { "_id": req.id }).await.map_err(internal)?;
    }
    Ok(PydanticJson(json!({ "ok": true })))
}

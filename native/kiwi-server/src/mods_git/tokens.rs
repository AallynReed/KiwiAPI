//! Git access tokens (`ModGitToken`, collection `mod_git_tokens`): the HTTP
//! Basic password for clone/pull/push. Only the sha256 is stored.

use chrono::{DateTime, Utc};
use futures::TryStreamExt;
use mongodb::{
    bson::{Bson, Document, doc, oid::ObjectId},
    options::UpdateOptions,
};
use serde_json::{Value, json};

use super::service::internal;
use crate::{
    auth::{
        models::{SITE_USERS, SiteUser},
        tokens::{hash_token, token_urlsafe},
    },
    dt,
    error::ApiError,
    state::AppState,
};

pub const COLLECTION: &str = "mod_git_tokens";
pub const PREFIX: &str = "kgit_";

#[derive(Clone, Debug)]
pub struct GitToken {
    pub id: ObjectId,
    pub site_user_id: ObjectId,
    pub token_hash: String,
    pub prefix: String,
    pub name: String,
    pub revoked: bool,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
}

impl GitToken {
    pub fn from_doc(d: &Document) -> Option<GitToken> {
        let dtm = |k: &str| d.get_datetime(k).ok().map(|b| dt::from_bson(*b));
        Some(GitToken {
            id: d.get_object_id("_id").ok()?,
            site_user_id: d.get_object_id("site_user_id").ok()?,
            token_hash: d.get_str("token_hash").ok()?.to_owned(),
            prefix: d.get_str("prefix").ok()?.to_owned(),
            name: d.get_str("name").unwrap_or("").to_owned(),
            revoked: d.get_bool("revoked").unwrap_or(false),
            created_at: dtm("created_at").unwrap_or_else(dt::now),
            last_used_at: dtm("last_used_at"),
        })
    }

    /// The document as Beanie writes it (insert, and `save()`'s `$set`).
    pub fn to_doc(&self) -> Document {
        doc! {
            "_id": self.id,
            "site_user_id": self.site_user_id,
            "token_hash": &self.token_hash,
            "prefix": &self.prefix,
            "name": &self.name,
            "revoked": self.revoked,
            "created_at": dt::to_bson(self.created_at),
            "last_used_at": self.last_used_at.map(|t| Bson::DateTime(dt::to_bson(t))).unwrap_or(Bson::Null),
        }
    }

    /// `_git_token_dto`.
    pub fn dto(&self) -> Value {
        json!({
            "id": self.id.to_hex(),
            "name": self.name,
            "prefix": self.prefix,
            "created_at": dt::isoformat(&self.created_at),
            "last_used_at": self.last_used_at.map(|t| dt::isoformat(&t)),
        })
    }
}

fn coll(state: &AppState) -> Result<mongodb::Collection<Document>, ApiError> {
    Ok(state.inner.db.mongo.as_ref().ok_or_else(|| internal("MONGO_URI not configured"))?.collection(COLLECTION))
}

async fn save(state: &AppState, t: &GitToken) -> Result<(), ApiError> {
    coll(state)?
        .update_one(doc! {"_id": t.id}, doc! {"$set": t.to_doc()})
        .with_options(UpdateOptions::builder().upsert(true).build())
        .await
        .map_err(internal)?;
    Ok(())
}

/// A freshly minted token: the stored row and the plaintext.
pub fn mint(actor: &SiteUser, name: &str) -> (GitToken, String) {
    let raw = format!("{PREFIX}{}", token_urlsafe(24));
    let label: String = kiwi_core::pystr::strip(name).chars().take(60).collect();
    let t = GitToken {
        id: ObjectId::new(),
        site_user_id: actor.id,
        token_hash: hash_token(&raw),
        prefix: raw.chars().take(12).collect(),
        name: label,
        revoked: false,
        created_at: dt::now(),
        last_used_at: None,
    };
    (t, raw)
}

/// `create_git_token`: `{...dto, token}`.
pub async fn create(state: &AppState, actor: &SiteUser, name: &str) -> Result<Value, ApiError> {
    let (t, raw) = mint(actor, name);
    coll(state)?.insert_one(t.to_doc()).await.map_err(internal)?;
    let mut v = t.dto();
    v["token"] = Value::String(raw);
    Ok(v)
}

/// `list_git_tokens`: live tokens, newest first.
pub async fn list(state: &AppState, actor: &SiteUser) -> Result<Value, ApiError> {
    let docs: Vec<Document> = coll(state)?
        .find(doc! {"site_user_id": actor.id, "revoked": false})
        .sort(doc! {"created_at": -1})
        .await
        .map_err(internal)?
        .try_collect()
        .await
        .map_err(internal)?;
    let items: Vec<Value> = docs.iter().filter_map(GitToken::from_doc).map(|t| t.dto()).collect();
    Ok(json!({"items": items}))
}

/// `revoke_git_token`.
pub async fn revoke(state: &AppState, actor: &SiteUser, token_id: &str) -> Result<(), ApiError> {
    let found = match ObjectId::parse_str(token_id) {
        Ok(oid) => {
            coll(state)?.find_one(doc! {"_id": oid}).await.map_err(internal)?.and_then(|d| GitToken::from_doc(&d))
        }
        Err(_) => None,
    };
    let Some(mut t) = found.filter(|t| t.site_user_id == actor.id) else {
        return Err(ApiError::not_found("Token not found"));
    };
    t.revoked = true;
    save(state, &t).await
}

/// `authenticate_git`: the live token's active owner, its `last_used_at` bumped.
pub async fn authenticate(state: &AppState, token: &str) -> Result<Option<SiteUser>, ApiError> {
    if token.is_empty() {
        return Ok(None);
    }
    let Some(mut t) = coll(state)?
        .find_one(doc! {"token_hash": hash_token(token), "revoked": false})
        .await
        .map_err(internal)?
        .and_then(|d| GitToken::from_doc(&d))
    else {
        return Ok(None);
    };
    let db = state.inner.db.mongo.as_ref().ok_or_else(|| internal("MONGO_URI not configured"))?;
    let user = db.collection::<SiteUser>(SITE_USERS).find_one(doc! {"_id": t.site_user_id}).await.map_err(internal)?;
    let Some(user) = user.filter(|u| u.is_active) else { return Ok(None) };
    t.last_used_at = Some(dt::now());
    save(state, &t).await?;
    Ok(Some(user))
}

//! `app/drops/service.py`: the store side of drops. Every lookup, guard and
//! write is the one Python makes, so either side can serve a drop the other
//! created.

use std::path::PathBuf;

use chrono::{DateTime, TimeDelta, Utc};
use futures::TryStreamExt;
use mongodb::{
    Database,
    bson::{Document, doc, oid::ObjectId},
};
use serde_json::{Map, Value};

use super::{DROPS, DropUpload, FileDrop, UPLOADS, drop_dir, drop_view};
use crate::{
    auth::{passwords, tokens::token_urlsafe},
    dt,
    error::{ApiError, ApiResult},
    state::AppState,
    validation::{Field, FromBody},
};

pub const INACTIVE: &str =
    "This upload link isn't active - it may have expired, been used already, or been turned off.";
const MIB: i64 = 1024 * 1024;

pub fn internal(err: impl std::fmt::Display) -> ApiError {
    tracing::error!("drops: {err}");
    ApiError::internal()
}

fn db(state: &AppState) -> ApiResult<&Database> {
    state.inner.db.mongo.as_ref().ok_or_else(|| internal("MONGO_URI not configured"))
}

/// `DropCreate`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DropCreate {
    pub label: String,
    pub pin: String,
    pub max_uploads: i64,
    pub expires_in_hours: i64,
    pub max_file_mb: i64,
}

fn digits_only(v: &str) -> Result<(), crate::validation::Fail> {
    if v.is_empty() || !v.chars().all(|c| c.is_ascii_digit()) {
        return Err(crate::validation::Fail::value_error("The PIN must be digits only."));
    }
    Ok(())
}

impl FromBody for DropCreate {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| {
            Some(DropCreate {
                label: m.field("label").str().min_len(1).max_len(120).required(),
                pin: m.field("pin").str().min_len(4).max_len(12).check(|v| digits_only(v)).required(),
                max_uploads: m.field("max_uploads").int().ge(1).le(100).default(1),
                expires_in_hours: m.field("expires_in_hours").int().ge(1).le(24 * 90).default(24),
                max_file_mb: m.field("max_file_mb").int().ge(1).le(2048).default(256),
            })
        })
        .required()
    }
}

/// `DropUpdate`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DropUpdate {
    pub label: Option<String>,
    pub max_uploads: Option<i64>,
    pub extend_hours: Option<i64>,
    pub revoked: Option<bool>,
}

impl FromBody for DropUpdate {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| {
            Some(DropUpdate {
                label: m.field("label").str().min_len(1).max_len(120).optional(),
                max_uploads: m.field("max_uploads").int().ge(1).le(100).optional(),
                extend_hours: m.field("extend_hours").int().ge(1).le(24 * 90).optional(),
                revoked: m.field("revoked").bool().optional(),
            })
        })
        .required()
    }
}

/// `FileDrop.get(id)`: an id that isn't an ObjectId fails pydantic
/// validation inside the handler, which Python answers with a 500.
fn parse_id(id: &str) -> ApiResult<ObjectId> {
    ObjectId::parse_str(id).map_err(|_| internal(format!("not an ObjectId: {id:?}")))
}

async fn find_drop(db: &Database, filter: Document) -> ApiResult<Option<FileDrop>> {
    match db.collection::<Document>(DROPS).find_one(filter).await.map_err(internal)? {
        Some(d) => FileDrop::from_doc(&d).map(Some).map_err(internal),
        None => Ok(None),
    }
}

async fn uploads_of(db: &Database, filter: Document) -> ApiResult<Vec<DropUpload>> {
    let docs: Vec<Document> = db
        .collection::<Document>(UPLOADS)
        .find(filter)
        .sort(doc! {"uploaded_at": -1})
        .await
        .map_err(internal)?
        .try_collect()
        .await
        .map_err(internal)?;
    docs.iter().map(DropUpload::from_doc).collect::<Result<_, _>>().map_err(internal)
}

fn view(state: &AppState, d: &FileDrop, uploads: &[DropUpload]) -> Map<String, Value> {
    drop_view(d, uploads, &state.settings().app_url, dt::now())
}

/// `create`: mint a link; the reply is the only place the PIN ever appears.
pub async fn create(state: &AppState, req: DropCreate) -> ApiResult<Value> {
    let db = db(state)?;
    let slug = token_urlsafe(12);
    let label = kiwi_core::pystr::strip(&req.label).to_owned();
    let pin = req.pin.clone();
    let pin_hash = tokio::task::spawn_blocking(move || passwords::hash_password(&pin)).await.map_err(internal)?;
    let cap = state.settings().drops_max_request_body_bytes;
    let expires_at = dt::now() + TimeDelta::hours(req.expires_in_hours);
    let drop = FileDrop {
        id: ObjectId::new(),
        slug,
        label,
        pin_hash,
        max_uploads: req.max_uploads,
        upload_count: 0,
        max_file_bytes: req.max_file_mb.saturating_mul(MIB).min(cap),
        expires_at,
        revoked: false,
        created_at: dt::now(),
    };
    db.collection::<Document>(DROPS).insert_one(drop.to_doc()).await.map_err(internal)?;
    let mut out = view(state, &drop, &[]);
    out.insert("pin".into(), req.pin.into());
    Ok(Value::Object(out))
}

/// `list_drops`: newest first, each with its files (newest first).
pub async fn list(state: &AppState) -> ApiResult<Value> {
    let db = db(state)?;
    let docs: Vec<Document> = db
        .collection::<Document>(DROPS)
        .find(doc! {})
        .sort(doc! {"created_at": -1})
        .await
        .map_err(internal)?
        .try_collect()
        .await
        .map_err(internal)?;
    let drops: Vec<FileDrop> = docs.iter().map(FileDrop::from_doc).collect::<Result<_, _>>().map_err(internal)?;
    if drops.is_empty() {
        return Ok(Value::Array(Vec::new()));
    }
    let ids: Vec<ObjectId> = drops.iter().map(|d| d.id).collect();
    let uploads = uploads_of(db, doc! {"drop_id": {"$in": ids}}).await?;
    Ok(Value::Array(
        drops
            .iter()
            .map(|d| {
                let mine: Vec<DropUpload> = uploads.iter().filter(|u| u.drop_id == d.id).cloned().collect();
                Value::Object(view(state, d, &mine))
            })
            .collect(),
    ))
}

async fn get_drop(db: &Database, id: &str) -> ApiResult<FileDrop> {
    let oid = parse_id(id)?;
    find_drop(db, doc! {"_id": oid}).await?.ok_or_else(|| ApiError::not_found("That drop doesn't exist."))
}

/// The new deadline of `extend_hours`: from now when already expired.
pub fn extended(expires_at: DateTime<Utc>, now: DateTime<Utc>, hours: i64) -> DateTime<Utc> {
    expires_at.max(now) + TimeDelta::hours(hours)
}

pub async fn update(state: &AppState, id: &str, req: DropUpdate) -> ApiResult<Value> {
    let db = db(state)?;
    let mut drop = get_drop(db, id).await?;
    if let Some(label) = &req.label {
        drop.label = kiwi_core::pystr::strip(label).to_owned();
    }
    if let Some(n) = req.max_uploads {
        drop.max_uploads = n;
    }
    if let Some(h) = req.extend_hours {
        drop.expires_at = extended(drop.expires_at, dt::now(), h);
    }
    if let Some(r) = req.revoked {
        drop.revoked = r;
    }
    db.collection::<Document>(DROPS)
        .update_one(doc! {"_id": drop.id}, doc! {"$set": drop.to_doc()})
        .upsert(true)
        .await
        .map_err(internal)?;
    let uploads = uploads_of(db, doc! {"drop_id": drop.id}).await?;
    Ok(Value::Object(view(state, &drop, &uploads)))
}

/// `delete`: the drop, its upload rows and every file it received.
pub async fn delete(state: &AppState, id: &str) -> ApiResult<()> {
    let db = db(state)?;
    let drop = get_drop(db, id).await?;
    db.collection::<Document>(UPLOADS).delete_many(doc! {"drop_id": drop.id}).await.map_err(internal)?;
    let _ = tokio::fs::remove_dir_all(drop_dir(state.settings(), &drop.slug)).await;
    db.collection::<Document>(DROPS).delete_one(doc! {"_id": drop.id}).await.map_err(internal)?;
    Ok(())
}

async fn get_upload(db: &Database, id: &str) -> ApiResult<DropUpload> {
    let oid = parse_id(id)?;
    let found = db.collection::<Document>(UPLOADS).find_one(doc! {"_id": oid}).await.map_err(internal)?;
    match found {
        Some(d) => DropUpload::from_doc(&d).map_err(internal),
        None => Err(ApiError::not_found("That file doesn't exist.")),
    }
}

/// `delete_upload`: the file and its row; the drop's used-up count stays.
pub async fn delete_upload(state: &AppState, id: &str) -> ApiResult<()> {
    let db = db(state)?;
    let upload = get_upload(db, id).await?;
    if let Some(drop) = find_drop(db, doc! {"_id": upload.drop_id}).await? {
        let _ = tokio::fs::remove_file(drop_dir(state.settings(), &drop.slug).join(&upload.stored_name)).await;
    }
    db.collection::<Document>(UPLOADS).delete_one(doc! {"_id": upload.id}).await.map_err(internal)?;
    Ok(())
}

/// `upload_path`: the file behind an upload row, for the master download.
pub async fn upload_path(state: &AppState, id: &str) -> ApiResult<(DropUpload, PathBuf)> {
    let db = db(state)?;
    let upload = get_upload(db, id).await?;
    let drop = find_drop(db, doc! {"_id": upload.drop_id}).await?;
    let path = drop.map(|d| drop_dir(state.settings(), &d.slug).join(&upload.stored_name));
    match path {
        Some(p) if tokio::fs::metadata(&p).await.is_ok_and(|m| m.is_file()) => Ok((upload, p)),
        _ => Err(ApiError::not_found("That file is no longer on disk.")),
    }
}

/// `by_slug`: spent, revoked, expired and unknown links all answer the same 404.
pub async fn by_slug(state: &AppState, slug: &str) -> ApiResult<FileDrop> {
    let db = db(state)?;
    match find_drop(db, doc! {"slug": slug}).await? {
        Some(d) if d.is_open(dt::now()) => Ok(d),
        _ => Err(ApiError::not_found(INACTIVE)),
    }
}

/// `check_pin` (argon2 verify off the async threads).
pub async fn check_pin(drop: &FileDrop, pin: &str) -> ApiResult<()> {
    let (pin, hash) = (pin.to_owned(), drop.pin_hash.clone());
    let ok = !pin.is_empty()
        && tokio::task::spawn_blocking(move || passwords::verify_password(&pin, &hash)).await.map_err(internal)?;
    if !ok {
        return Err(ApiError::forbidden("That PIN isn't right."));
    }
    Ok(())
}

/// `_claim_slot`: take one upload atomically, guarded by the whole open test.
pub async fn claim_slot(state: &AppState, drop: &FileDrop) -> ApiResult<bool> {
    let found = db(state)?
        .collection::<Document>(DROPS)
        .find_one_and_update(
            doc! {
                "_id": drop.id,
                "revoked": false,
                "expires_at": {"$gt": dt::to_bson(dt::now())},
                "$expr": {"$lt": ["$upload_count", "$max_uploads"]},
            },
            doc! {"$inc": {"upload_count": 1_i32}},
        )
        .await
        .map_err(internal)?;
    Ok(found.is_some())
}

pub async fn release_slot(state: &AppState, drop: &FileDrop) -> ApiResult<()> {
    db(state)?
        .collection::<Document>(DROPS)
        .update_one(doc! {"_id": drop.id}, doc! {"$inc": {"upload_count": -1_i32}})
        .await
        .map_err(internal)?;
    Ok(())
}

pub async fn insert_upload(state: &AppState, upload: &DropUpload) -> ApiResult<()> {
    db(state)?.collection::<Document>(UPLOADS).insert_one(upload.to_doc()).await.map_err(internal)?;
    Ok(())
}

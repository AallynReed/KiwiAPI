//! `/admin/custom-art*` (`app/custom_art/admin.py` + the review half of
//! `service.py`): the portal's Modules -> Custom art page.

use axum::{
    extract::{FromRequestParts, State},
    http::{HeaderValue, StatusCode, header, request::Parts},
    response::{IntoResponse, Response},
};
use futures::TryStreamExt;
use kiwi_core::pystr;
use mongodb::{
    Collection,
    bson::{Bson, Document, doc, oid::ObjectId},
};
use serde_json::{Value, json};

use super::{ArtRequest, COLLECTION, clean_name, denied_mail, send, view};
use crate::{
    admin::{RequestSeg, Res, Seg, docs, internal, path_param},
    auth::Superuser,
    dt,
    error::ApiError,
    media::cas::ContentStore,
    pyjson::PydanticJson,
    state::AppState,
    validation::{DepsThenBody, Field, FromBody},
};

/// `ApproveBody`.
pub struct ApproveBody {
    pub name: Option<String>,
}

impl FromBody for ApproveBody {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| Some(ApproveBody { name: m.field("name").str().max_len(200).optional() })).required()
    }
}

/// `DenyBody`.
pub struct DenyBody {
    pub reason: String,
}

impl FromBody for DenyBody {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| Some(DenyBody { reason: m.field("reason").str().min_len(3).max_len(2000).required() }))
            .required()
    }
}

/// The `{slot}` path segment, decoded.
pub struct SlotSeg(pub String);

impl<S: Send + Sync> FromRequestParts<S> for SlotSeg {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        Ok(SlotSeg(path_param(parts, "slot")))
    }
}

fn coll(state: &AppState) -> Collection<Document> {
    state.mongo().collection(COLLECTION)
}

fn dto(r: &ArtRequest) -> Result<Value, ApiError> {
    view(r).ok_or_else(|| internal("custom art request without a picture to name"))
}

/// `_get`.
pub async fn load(state: &AppState, id: &str) -> Result<ArtRequest, ApiError> {
    let missing = || ApiError::not_found("That request doesn't exist.");
    let Ok(oid) = ObjectId::parse_str(id) else { return Err(missing()) };
    let d = coll(state).find_one(doc! {"_id": oid}).await.map_err(internal)?.ok_or_else(missing)?;
    ArtRequest::from_doc(&d).map_err(internal)
}

/// Beanie `save()`.
pub async fn save(state: &AppState, r: &ArtRequest) -> Result<(), ApiError> {
    docs::save(&coll(state), r.id, r.to_doc()).await
}

async fn update_many(state: &AppState, filter: Document, set: Document) -> Result<u64, ApiError> {
    let res = coll(state).update_many(filter, doc! {"$set": set}).await.map_err(internal)?;
    Ok(res.modified_count)
}

pub async fn list(State(state): State<AppState>, _: Superuser) -> Res {
    let rows: Vec<Document> = coll(&state)
        .find(doc! {})
        .sort(doc! {"created_at": -1})
        .limit(300)
        .await
        .map_err(internal)?
        .try_collect()
        .await
        .map_err(internal)?;
    let mut items = Vec::with_capacity(rows.len());
    for d in &rows {
        items.push(dto(&ArtRequest::from_doc(d).map_err(internal)?)?);
    }
    Ok(PydanticJson(json!({"items": items})))
}

/// Starlette `Response(content, media_type=...)`'s Content-Type.
fn media_type(ct: &str) -> String {
    if ct.starts_with("text/") && !ct.to_ascii_lowercase().contains("charset=") {
        format!("{ct}; charset=utf-8")
    } else {
        ct.to_owned()
    }
}

pub async fn image(
    State(state): State<AppState>,
    _: Superuser,
    Seg(id): RequestSeg,
    SlotSeg(slot): SlotSeg,
) -> Result<Response, ApiError> {
    let r = load(&state, &id).await?;
    let picture = r.pictures.iter().find(|(k, _)| *k == slot).map(|(_, p)| p.clone());
    let data = match &picture {
        Some(p) => {
            let store = ContentStore::new(&state.settings().mods_store_dir);
            let sha = p.sha.clone();
            tokio::task::spawn_blocking(move || store.get(&sha)).await.map_err(internal)?.map_err(internal)?
        }
        None => None,
    };
    let (Some(p), Some(data)) = (picture, data) else {
        return Err(ApiError::not_found("That picture isn't there."));
    };
    let ct = HeaderValue::from_str(&media_type(&p.content_type)).map_err(internal)?;
    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, ct), (header::CACHE_CONTROL, HeaderValue::from_static("no-store"))],
        data,
    )
        .into_response())
}

pub async fn release(State(state): State<AppState>, _: Superuser) -> Res {
    let n = update_many(&state, doc! {"status": "approved"}, doc! {"status": "queued", "log": Bson::Null}).await?;
    Ok(PydanticJson(json!({"queued": n})))
}

pub async fn steam_done(State(state): State<AppState>, _: Superuser) -> Res {
    let n = update_many(&state, doc! {"steam_pending": true}, doc! {"steam_pending": false}).await?;
    Ok(PydanticJson(json!({"cleared": n})))
}

pub async fn approve(
    State(state): State<AppState>,
    Seg(id): RequestSeg,
    DepsThenBody(_, body): DepsThenBody<Superuser, ApproveBody>,
) -> Res {
    let mut r = load(&state, &id).await?;
    if r.status != "pending" {
        return Err(ApiError::conflict("Only a pending request can be approved."));
    }
    if let Some(name) = &body.name {
        r.name = clean_name(name)?;
    }
    r.status = "approved".into();
    r.decided_at = Some(dt::now());
    save(&state, &r).await?;
    Ok(PydanticJson(dto(&r)?))
}

pub async fn deny(
    State(state): State<AppState>,
    Seg(id): RequestSeg,
    DepsThenBody(_, body): DepsThenBody<Superuser, DenyBody>,
) -> Res {
    let mut r = load(&state, &id).await?;
    if r.status != "pending" && r.status != "approved" {
        return Err(ApiError::conflict("That request is already past review."));
    }
    r.status = "denied".into();
    r.reason = Some(pystr::strip(&body.reason).to_owned());
    r.decided_at = Some(dt::now());
    save(&state, &r).await?;
    send(&state, &denied_mail(&r, &state.settings().app_url)).await;
    Ok(PydanticJson(dto(&r)?))
}

pub async fn retry(State(state): State<AppState>, _: Superuser, Seg(id): RequestSeg) -> Res {
    let r = load(&state, &id).await?;
    if r.status != "failed" {
        return Err(ApiError::conflict("Only a failed request can be retried."));
    }
    let batch = r.batch.as_ref().map_or(Bson::Null, |b| Bson::String(b.clone()));
    let n = update_many(&state, doc! {"status": "failed", "batch": batch}, doc! {"status": "queued"}).await?;
    Ok(PydanticJson(json!({"queued": n})))
}

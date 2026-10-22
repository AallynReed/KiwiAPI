//! `POST /v1/moderation/report` (`app/trove/moderation.py`): an open
//! notice-and-action report against a publicly reachable mod, modpack or
//! profile. No auth, no rate limit; nothing about the reporter is stored.

use axum::{
    Router,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
};
use chrono::{DateTime, Utc};
use kiwi_core::leaf::{normalize_handle, py_strip};
use mongodb::bson::{Bson, Document, doc, oid::ObjectId};
use serde_json::json;

use super::internal;
use crate::{
    dt,
    error::ApiError,
    pyjson::PydanticJson,
    state::AppState,
    validation::{Field, FromBody, Json},
};

pub const REPORTS: &str = "content_reports";
const TARGET_TYPES: &[&str] = &["mod", "modpack", "profile"];
const PUBLIC: [&str; 2] = ["public", "unlisted"];
const MOD_VISIBILITY: [&str; 5] = ["draft", "unlisted", "public", "public_website_only", "hidden"];
const MODPACK_VISIBILITY: [&str; 3] = ["draft", "unlisted", "public"];

pub fn router() -> Router<AppState> {
    Router::new().route("/v1/moderation/report", post(report))
}

/// `ReportBody`.
#[derive(Debug, Clone)]
pub struct ReportBody {
    pub target_type: String,
    pub handle: String,
    pub slug: String,
    pub reason: String,
}

impl FromBody for ReportBody {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| {
            Some(ReportBody {
                target_type: m.field("target_type").literal(TARGET_TYPES).required(),
                handle: m.field("handle").str().min_len(1).max_len(64).required(),
                slug: m.field("slug").str().max_len(140).default(String::new()),
                reason: m.field("reason").str().min_len(3).max_len(2000).required(),
            })
        })
        .required()
    }
}

/// Where `_resolve_target` looks: collection, Beanie's filter, and the
/// target URL, from the normalized handle and slug.
pub fn target_query(kind: &str, handle: &str, slug: &str) -> (&'static str, Document, String) {
    let (handle, slug) = (normalize_handle(handle), normalize_handle(slug));
    match kind {
        "mod" => ("mod_projects", pair_filter(&handle, &slug), format!("/mods/{handle}/{slug}")),
        "modpack" => ("modpack_projects", pair_filter(&handle, &slug), format!("/modpacks/{handle}/{slug}")),
        _ => ("mod_profiles", doc! {"handle": &handle}, format!("/mods/{handle}")),
    }
}

fn pair_filter(handle: &str, slug: &str) -> Document {
    doc! {"$and": [{"owner_handle": handle}, {"slug": slug}]}
}

fn str_field(d: &Document, key: &str, default: Option<&str>) -> Result<String, ApiError> {
    match (d.get(key), default) {
        (Some(Bson::String(s)), _) => Ok(s.clone()),
        (None, Some(dflt)) => Ok(dflt.to_owned()),
        (other, _) => Err(internal(format!("moderation target {key}: {other:?}"))),
    }
}

/// `(target_id, label)` of a found document, or `None` when a reporter
/// could not have reached it. `handle` is the normalized handle.
pub fn reachable(kind: &str, d: &Document, handle: &str) -> Result<Option<(ObjectId, String)>, ApiError> {
    let id = match d.get("_id") {
        Some(Bson::ObjectId(o)) => *o,
        other => return Err(internal(format!("moderation target _id: {other:?}"))),
    };
    if kind == "profile" {
        let name = str_field(d, "display_name", Some(""))?;
        return Ok(Some((id, if name.is_empty() { handle.to_owned() } else { name })));
    }
    let title = str_field(d, "title", None)?;
    let visibility = str_field(d, "visibility", Some("draft"))?;
    let allowed: &[&str] = if kind == "mod" { &MOD_VISIBILITY } else { &MODPACK_VISIBILITY };
    if !allowed.contains(&visibility.as_str()) {
        return Err(internal(format!("moderation target visibility {visibility:?}")));
    }
    Ok(PUBLIC.contains(&visibility.as_str()).then_some((id, title)))
}

/// The `ContentReport` row Beanie inserts (`_id` first, as pymongo sends it).
pub fn report_doc(
    id: ObjectId,
    kind: &str,
    target_id: ObjectId,
    label: &str,
    url: &str,
    reason: &str,
    now: DateTime<Utc>,
) -> Document {
    doc! {
        "_id": id,
        "target_type": kind,
        "target_id": target_id,
        "target_label": label,
        "target_url": url,
        "reason": reason,
        "resolved": false,
        "created_at": dt::to_bson(now),
    }
}

async fn report(State(state): State<AppState>, Json(body): Json<ReportBody>) -> Result<Response, ApiError> {
    let (coll, filter, url) = target_query(&body.target_type, &body.handle, &body.slug);
    let db = state.mongo();
    let found = db.collection::<Document>(coll).find_one(filter).await.map_err(internal)?;
    let handle = normalize_handle(&body.handle);
    let target = match found {
        Some(d) => reachable(&body.target_type, &d, &handle)?,
        None => None,
    };
    let Some((target_id, label)) = target else {
        return Err(ApiError::not_found("That content couldn't be found."));
    };
    let row =
        report_doc(ObjectId::new(), &body.target_type, target_id, &label, &url, py_strip(&body.reason), dt::now());
    db.collection::<Document>(REPORTS).insert_one(row).await.map_err(internal)?;
    Ok((StatusCode::ACCEPTED, PydanticJson(json!({"status": "received"}))).into_response())
}

//! File drops (`app/drops`): master-minted, PIN-protected upload links. The
//! uploader surface is `/site/drops/*` (behind `feature_file_drops_enabled`),
//! management is `/admin/drops*` (superuser). Files live under
//! `<drops_store_dir>/<slug>/` and only come back out through the master
//! download.

pub mod download;
pub mod routes;
pub mod service;
pub mod upload;

#[cfg(test)]
mod tests;

use std::path::PathBuf;

use axum::{
    Router,
    routing::{delete, get, post},
};
use chrono::{DateTime, Utc};
use mongodb::bson::{Bson, Document, doc, oid::ObjectId};
use serde_json::{Map, Value, json};

use crate::{auth::pyutil::bson_int, config::Settings, dt, error, state::AppState};

pub const DROPS: &str = "file_drops";
pub const UPLOADS: &str = "drop_uploads";

pub fn router() -> Router<AppState> {
    let not_allowed = |allow: &'static str| move || async move { error::method_not_allowed(allow) };
    Router::new()
        .route("/site/drops/{slug}", get(routes::meta).fallback(not_allowed("GET")))
        .route("/site/drops/{slug}/verify", post(routes::verify).fallback(not_allowed("POST")))
        .route("/site/drops/{slug}/upload", post(routes::upload).fallback(not_allowed("POST")))
        .route("/admin/drops", get(routes::list).post(routes::create).fallback(not_allowed("GET")))
        .route(
            "/admin/drops/{drop_id}",
            axum::routing::patch(routes::update).delete(routes::remove).fallback(not_allowed("PATCH")),
        )
        .route("/admin/drops/uploads/{upload_id}/download", get(routes::download).fallback(not_allowed("GET")))
        .route("/admin/drops/uploads/{upload_id}", delete(routes::remove_upload).fallback(not_allowed("DELETE")))
}

pub fn drop_dir(settings: &Settings, slug: &str) -> PathBuf {
    PathBuf::from(&settings.drops_store_dir).join(slug)
}

/// A stored int as pydantic reads it (lax: an integral float is fine).
fn int_field(d: &Document, key: &str) -> Result<Option<i64>, ()> {
    match d.get(key) {
        None => Ok(None),
        Some(Bson::Int32(n)) => Ok(Some(i64::from(*n))),
        Some(Bson::Int64(n)) => Ok(Some(*n)),
        Some(Bson::Double(f)) if f.fract() == 0.0 && f.is_finite() => Ok(Some(*f as i64)),
        Some(_) => Err(()),
    }
}

fn str_field(d: &Document, key: &str) -> Result<Option<String>, ()> {
    match d.get(key) {
        None | Some(Bson::Null) => Ok(None),
        Some(Bson::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(()),
    }
}

fn dt_field(d: &Document, key: &str) -> Result<Option<DateTime<Utc>>, ()> {
    match d.get(key) {
        None => Ok(None),
        Some(Bson::DateTime(b)) => Ok(Some(dt::from_bson(*b))),
        Some(_) => Err(()),
    }
}

fn opt_str(v: &Option<String>) -> Bson {
    v.as_ref().map_or(Bson::Null, |s| Bson::String(s.clone()))
}

/// `FileDrop`.
#[derive(Clone, Debug, PartialEq)]
pub struct FileDrop {
    pub id: ObjectId,
    pub slug: String,
    pub label: String,
    pub pin_hash: String,
    pub max_uploads: i64,
    pub upload_count: i64,
    pub max_file_bytes: i64,
    pub expires_at: DateTime<Utc>,
    pub revoked: bool,
    pub created_at: DateTime<Utc>,
}

impl FileDrop {
    pub fn to_doc(&self) -> Document {
        doc! {
            "_id": self.id,
            "slug": &self.slug,
            "label": &self.label,
            "pin_hash": &self.pin_hash,
            "max_uploads": bson_int(self.max_uploads),
            "upload_count": bson_int(self.upload_count),
            "max_file_bytes": bson_int(self.max_file_bytes),
            "expires_at": dt::to_bson(self.expires_at),
            "revoked": self.revoked,
            "created_at": dt::to_bson(self.created_at),
        }
    }

    /// A stored drop; `Err` where Beanie's validation raises (a 500).
    pub fn from_doc(d: &Document) -> Result<Self, String> {
        let bad = |k: &str| format!("file_drops document has a bad {k}");
        let req = |k: &str, v: Result<Option<String>, ()>| v.ok().flatten().ok_or_else(|| bad(k));
        Ok(FileDrop {
            id: d.get_object_id("_id").map_err(|_| bad("_id"))?,
            slug: req("slug", str_field(d, "slug"))?,
            label: req("label", str_field(d, "label"))?,
            pin_hash: req("pin_hash", str_field(d, "pin_hash"))?,
            max_uploads: int_field(d, "max_uploads").map_err(|_| bad("max_uploads"))?.unwrap_or(1),
            upload_count: int_field(d, "upload_count").map_err(|_| bad("upload_count"))?.unwrap_or(0),
            max_file_bytes: int_field(d, "max_file_bytes").ok().flatten().ok_or_else(|| bad("max_file_bytes"))?,
            expires_at: dt_field(d, "expires_at").ok().flatten().ok_or_else(|| bad("expires_at"))?,
            revoked: match d.get("revoked") {
                None => false,
                Some(Bson::Boolean(b)) => *b,
                Some(_) => return Err(bad("revoked")),
            },
            created_at: dt_field(d, "created_at").map_err(|_| bad("created_at"))?.unwrap_or_else(dt::now),
        })
    }

    /// `is_open`: not revoked, not expired, uploads left.
    pub fn is_open(&self, now: DateTime<Utc>) -> bool {
        !self.revoked && self.expires_at > now && self.upload_count < self.max_uploads
    }
}

/// `DropUpload`.
#[derive(Clone, Debug, PartialEq)]
pub struct DropUpload {
    pub id: ObjectId,
    pub drop_id: ObjectId,
    pub filename: String,
    pub stored_name: String,
    pub size: i64,
    pub content_type: Option<String>,
    pub sha256: String,
    pub note: Option<String>,
    pub uploaded_at: DateTime<Utc>,
}

impl DropUpload {
    pub fn to_doc(&self) -> Document {
        doc! {
            "_id": self.id,
            "drop_id": self.drop_id,
            "filename": &self.filename,
            "stored_name": &self.stored_name,
            "size": bson_int(self.size),
            "content_type": opt_str(&self.content_type),
            "sha256": &self.sha256,
            "note": opt_str(&self.note),
            "uploaded_at": dt::to_bson(self.uploaded_at),
        }
    }

    pub fn from_doc(d: &Document) -> Result<Self, String> {
        let bad = |k: &str| format!("drop_uploads document has a bad {k}");
        let req = |k: &str, v: Result<Option<String>, ()>| v.ok().flatten().ok_or_else(|| bad(k));
        Ok(DropUpload {
            id: d.get_object_id("_id").map_err(|_| bad("_id"))?,
            drop_id: match d.get("drop_id") {
                Some(Bson::ObjectId(o)) => *o,
                Some(Bson::String(s)) => ObjectId::parse_str(s).map_err(|_| bad("drop_id"))?,
                _ => return Err(bad("drop_id")),
            },
            filename: req("filename", str_field(d, "filename"))?,
            stored_name: req("stored_name", str_field(d, "stored_name"))?,
            size: int_field(d, "size").ok().flatten().ok_or_else(|| bad("size"))?,
            content_type: str_field(d, "content_type").map_err(|_| bad("content_type"))?,
            sha256: req("sha256", str_field(d, "sha256"))?,
            note: str_field(d, "note").map_err(|_| bad("note"))?,
            uploaded_at: dt_field(d, "uploaded_at").map_err(|_| bad("uploaded_at"))?.unwrap_or_else(dt::now),
        })
    }
}

/// `DropUploadView`.
pub fn upload_view(u: &DropUpload) -> Value {
    json!({
        "id": u.id.to_hex(),
        "filename": u.filename,
        "size": u.size,
        "content_type": u.content_type,
        "sha256": u.sha256,
        "note": u.note,
        "uploaded_at": dt::pydantic(&u.uploaded_at),
    })
}

/// `DropView` (the PIN is never part of it).
pub fn drop_view(d: &FileDrop, uploads: &[DropUpload], app_url: &str, now: DateTime<Utc>) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("id".into(), d.id.to_hex().into());
    m.insert("slug".into(), d.slug.clone().into());
    m.insert("url".into(), format!("{}/drop/{}", app_url.trim_end_matches('/'), d.slug).into());
    m.insert("label".into(), d.label.clone().into());
    m.insert("max_uploads".into(), d.max_uploads.into());
    m.insert("upload_count".into(), d.upload_count.into());
    m.insert("max_file_bytes".into(), d.max_file_bytes.into());
    m.insert("expires_at".into(), dt::pydantic(&d.expires_at).into());
    m.insert("revoked".into(), d.revoked.into());
    m.insert("open".into(), d.is_open(now).into());
    m.insert("created_at".into(), dt::pydantic(&d.created_at).into());
    m.insert("uploads".into(), Value::Array(uploads.iter().map(upload_view).collect()));
    m
}

/// `DropPublicView`: enough to render the upload form, nothing about the maker.
pub fn public_view(d: &FileDrop) -> Value {
    json!({
        "label": d.label,
        "max_file_bytes": d.max_file_bytes,
        "uploads_left": (d.max_uploads - d.upload_count).max(0),
        "expires_at": dt::pydantic(&d.expires_at),
    })
}

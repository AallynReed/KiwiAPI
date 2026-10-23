//! `ArtRequest` (`art_requests`) as Beanie loads and writes it: every field in
//! model order, explicit nulls, `_id` first.

use chrono::{DateTime, Utc};
use mongodb::bson::{Bson, Document, doc, oid::ObjectId};

use crate::{auth::pyutil::bson_int, dt};

#[derive(Debug, Clone, PartialEq)]
pub struct ArtPicture {
    pub sha: String,
    pub content_type: String,
    pub width: i64,
    pub height: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ArtRequest {
    pub id: ObjectId,
    pub kind: String,
    pub name: String,
    pub email: String,
    pub note: Option<String>,
    /// Slot -> picture, in stored order.
    pub pictures: Vec<(String, ArtPicture)>,
    pub status: String,
    pub reason: Option<String>,
    pub batch: Option<String>,
    pub log: Option<String>,
    pub versions: Vec<(String, String)>,
    pub steam_pending: bool,
    pub created_at: DateTime<Utc>,
    pub decided_at: Option<DateTime<Utc>>,
    pub released_at: Option<DateTime<Utc>>,
}

const STATUSES: [&str; 7] = ["pending", "approved", "queued", "building", "released", "failed", "denied"];

type R<T> = Result<T, String>;

fn req_str(d: &Document, k: &str) -> R<String> {
    match d.get(k) {
        Some(Bson::String(s)) => Ok(s.clone()),
        other => Err(format!("{k}: {other:?}")),
    }
}

fn opt_str(d: &Document, k: &str) -> R<Option<String>> {
    match d.get(k) {
        None | Some(Bson::Null) => Ok(None),
        Some(Bson::String(s)) => Ok(Some(s.clone())),
        other => Err(format!("{k}: {other:?}")),
    }
}

fn opt_dt(d: &Document, k: &str) -> R<Option<DateTime<Utc>>> {
    match d.get(k) {
        None | Some(Bson::Null) => Ok(None),
        Some(Bson::DateTime(t)) => Ok(Some(dt::from_bson(*t))),
        other => Err(format!("{k}: {other:?}")),
    }
}

/// pydantic's lax `int`.
pub fn int_of(b: &Bson) -> Option<i64> {
    match b {
        Bson::Int32(n) => Some(i64::from(*n)),
        Bson::Int64(n) => Some(*n),
        Bson::Double(f) if f.is_finite() && f.fract() == 0.0 => Some(*f as i64),
        Bson::Boolean(b) => Some(i64::from(*b)),
        _ => None,
    }
}

fn picture(b: &Bson) -> R<ArtPicture> {
    let Bson::Document(d) = b else { return Err(format!("picture: {b:?}")) };
    let int = |k: &str| d.get(k).and_then(int_of).ok_or_else(|| format!("picture.{k}"));
    Ok(ArtPicture {
        sha: req_str(d, "sha")?,
        content_type: req_str(d, "content_type")?,
        width: int("width")?,
        height: int("height")?,
    })
}

fn opt(v: &Option<String>) -> Bson {
    v.as_ref().map_or(Bson::Null, |s| Bson::String(s.clone()))
}

fn opt_date(v: &Option<DateTime<Utc>>) -> Bson {
    v.map_or(Bson::Null, |t| Bson::DateTime(dt::to_bson(t)))
}

impl ArtRequest {
    /// A stored document; Err where Beanie's validation would raise.
    pub fn from_doc(d: &Document) -> R<Self> {
        let id = d.get_object_id("_id").map_err(|e| format!("_id: {e}"))?;
        let kind = req_str(d, "kind")?;
        if kind != "player" && kind != "club" {
            return Err(format!("kind: {kind:?}"));
        }
        let pictures = match d.get("pictures") {
            Some(Bson::Document(p)) => p.iter().map(|(k, v)| Ok((k.clone(), picture(v)?))).collect::<R<Vec<_>>>()?,
            other => return Err(format!("pictures: {other:?}")),
        };
        let status = match d.get("status") {
            None => "pending".to_owned(),
            Some(_) => req_str(d, "status")?,
        };
        if !STATUSES.contains(&status.as_str()) {
            return Err(format!("status: {status:?}"));
        }
        let versions = match d.get("versions") {
            None => Vec::new(),
            Some(Bson::Document(v)) => v
                .iter()
                .map(|(k, v)| match v {
                    Bson::String(s) => Ok((k.clone(), s.clone())),
                    other => Err(format!("versions.{k}: {other:?}")),
                })
                .collect::<R<Vec<_>>>()?,
            other => return Err(format!("versions: {other:?}")),
        };
        let steam_pending = match d.get("steam_pending") {
            None => false,
            Some(Bson::Boolean(b)) => *b,
            Some(Bson::Int32(n @ (0 | 1))) => *n == 1,
            Some(Bson::Int64(n @ (0 | 1))) => *n == 1,
            other => return Err(format!("steam_pending: {other:?}")),
        };
        let created_at = match d.get("created_at") {
            None => dt::now(),
            Some(Bson::DateTime(t)) => dt::from_bson(*t),
            other => return Err(format!("created_at: {other:?}")),
        };
        Ok(ArtRequest {
            id,
            kind,
            name: req_str(d, "name")?,
            email: req_str(d, "email")?,
            note: opt_str(d, "note")?,
            pictures,
            status,
            reason: opt_str(d, "reason")?,
            batch: opt_str(d, "batch")?,
            log: opt_str(d, "log")?,
            versions,
            steam_pending,
            created_at,
            decided_at: opt_dt(d, "decided_at")?,
            released_at: opt_dt(d, "released_at")?,
        })
    }

    /// The whole document: what `insert()` stores and `save()` `$set`s.
    pub fn to_doc(&self) -> Document {
        let mut pictures = Document::new();
        for (slot, p) in &self.pictures {
            pictures.insert(
                slot.clone(),
                doc! {"sha": &p.sha, "content_type": &p.content_type, "width": bson_int(p.width), "height": bson_int(p.height)},
            );
        }
        let mut versions = Document::new();
        for (k, v) in &self.versions {
            versions.insert(k.clone(), v.clone());
        }
        doc! {
            "_id": self.id,
            "kind": &self.kind,
            "name": &self.name,
            "email": &self.email,
            "note": opt(&self.note),
            "pictures": pictures,
            "status": &self.status,
            "reason": opt(&self.reason),
            "batch": opt(&self.batch),
            "log": opt(&self.log),
            "versions": versions,
            "steam_pending": self.steam_pending,
            "created_at": dt::to_bson(self.created_at),
            "decided_at": opt_date(&self.decided_at),
            "released_at": opt_date(&self.released_at),
        }
    }

    pub fn version(&self, mod_: &str) -> Option<&str> {
        self.versions.iter().find(|(k, _)| k == mod_).map(|(_, v)| v.as_str())
    }

    /// `request.versions[mod] = version` (an existing key keeps its place).
    pub fn set_version(&mut self, mod_: &str, version: &str) {
        match self.versions.iter_mut().find(|(k, _)| k == mod_) {
            Some((_, v)) => *v = version.to_owned(),
            None => self.versions.push((mod_.to_owned(), version.to_owned())),
        }
    }
}

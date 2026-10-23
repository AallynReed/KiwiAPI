//! `app/trove/modpacks/models.py`: read the way Beanie validates a document
//! (model defaults for missing fields, a wrong type is a 500) and written
//! field by field in model order, as `insert()` / `save()` send it.

use chrono::{DateTime, Utc};
use mongodb::bson::{Bson, Document, doc, oid::ObjectId};

use crate::{auth::pyutil::bson_int, dt, error::ApiError};

pub const PACKS: &str = "modpack_projects";
pub const STARS: &str = "modpack_stars";
pub const DEFAULT_VARIANT: &str = "default";
pub const VISIBILITY: &[&str] = &["draft", "unlisted", "public"];

/// A stored document the model rejects (a 500 in Python).
#[derive(Debug)]
pub struct Invalid(pub String);

impl From<Invalid> for ApiError {
    fn from(e: Invalid) -> Self {
        tracing::error!("modpacks: document failed validation: {}", e.0);
        ApiError::internal()
    }
}

type R<T> = Result<T, Invalid>;

fn bad<T>(key: &str, v: &Bson) -> R<T> {
    Err(Invalid(format!("{key}: unexpected {v:?}")))
}

fn missing<T>(key: &str) -> R<T> {
    Err(Invalid(format!("{key}: missing")))
}

pub fn oid_of(key: &str, v: &Bson) -> R<ObjectId> {
    match v {
        Bson::ObjectId(o) => Ok(*o),
        Bson::String(s) => ObjectId::parse_str(s).map_err(|_| Invalid(format!("{key}: {s:?}"))),
        _ => bad(key, v),
    }
}

pub fn oid(d: &Document, key: &str) -> R<ObjectId> {
    d.get(key).map_or_else(|| missing(key), |v| oid_of(key, v))
}

pub fn opt_oid(d: &Document, key: &str) -> R<Option<ObjectId>> {
    match d.get(key) {
        None | Some(Bson::Null) => Ok(None),
        Some(v) => oid_of(key, v).map(Some),
    }
}

pub fn s(d: &Document, key: &str) -> R<String> {
    match d.get(key) {
        Some(Bson::String(v)) => Ok(v.clone()),
        Some(v) => bad(key, v),
        None => missing(key),
    }
}

pub fn s_or(d: &Document, key: &str, default: &str) -> R<String> {
    match d.get(key) {
        None => Ok(default.to_owned()),
        Some(Bson::String(v)) => Ok(v.clone()),
        Some(v) => bad(key, v),
    }
}

pub fn opt_s(d: &Document, key: &str) -> R<Option<String>> {
    match d.get(key) {
        None | Some(Bson::Null) => Ok(None),
        Some(Bson::String(v)) => Ok(Some(v.clone())),
        Some(v) => bad(key, v),
    }
}

pub fn strs(d: &Document, key: &str) -> R<Vec<String>> {
    match d.get(key) {
        None => Ok(Vec::new()),
        Some(Bson::Array(a)) => a
            .iter()
            .map(|v| match v {
                Bson::String(s) => Ok(s.clone()),
                _ => bad(key, v),
            })
            .collect(),
        Some(v) => bad(key, v),
    }
}

pub fn boolean(d: &Document, key: &str, default: bool) -> R<bool> {
    match d.get(key) {
        None => Ok(default),
        Some(Bson::Boolean(b)) => Ok(*b),
        Some(Bson::Int32(0)) | Some(Bson::Int64(0)) => Ok(false),
        Some(Bson::Int32(1)) | Some(Bson::Int64(1)) => Ok(true),
        Some(Bson::Double(f)) if *f == 0.0 || *f == 1.0 => Ok(*f == 1.0),
        Some(v) => bad(key, v),
    }
}

pub fn int(d: &Document, key: &str, default: i64) -> R<i64> {
    match d.get(key) {
        None => Ok(default),
        Some(Bson::Int32(i)) => Ok(i64::from(*i)),
        Some(Bson::Int64(i)) => Ok(*i),
        Some(Bson::Boolean(b)) => Ok(i64::from(*b)),
        Some(Bson::Double(f)) if f.is_finite() && f.fract() == 0.0 && f.abs() < 9.2e18 => Ok(*f as i64),
        Some(v) => bad(key, v),
    }
}

/// `datetime = Field(default_factory=utcnow)`.
pub fn datetime_or(d: &Document, key: &str, now: DateTime<Utc>) -> R<DateTime<Utc>> {
    match d.get(key) {
        None => Ok(now),
        Some(Bson::DateTime(b)) => Ok(dt::from_bson(*b)),
        Some(v) => bad(key, v),
    }
}

fn literal(d: &Document, key: &str, choices: &[&str], default: &str) -> R<String> {
    let v = s_or(d, key, default)?;
    if choices.contains(&v.as_str()) { Ok(v) } else { Err(Invalid(format!("{key}: {v:?}"))) }
}

pub fn opt_str(v: &Option<String>) -> Bson {
    v.as_ref().map_or(Bson::Null, |s| Bson::String(s.clone()))
}

pub fn bdt(t: DateTime<Utc>) -> Bson {
    Bson::DateTime(dt::to_bson(t))
}

fn sub_docs(d: &Document, key: &str) -> R<Vec<Document>> {
    match d.get(key) {
        None => Ok(Vec::new()),
        Some(Bson::Array(a)) => a
            .iter()
            .map(|v| match v {
                Bson::Document(x) => Ok(x.clone()),
                _ => bad(key, v),
            })
            .collect(),
        Some(v) => bad(key, v),
    }
}

/// `ModpackEntry`: a hub-mod reference or a custom uploaded `.tmod`.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub project_id: Option<ObjectId>,
    pub handle: String,
    pub slug: String,
    pub title: String,
    pub branch: String,
    pub version_locked: bool,
    pub locked_tag: Option<String>,
    pub custom_sha: Option<String>,
    pub custom_filename: Option<String>,
    pub author: String,
}

impl Default for Entry {
    fn default() -> Self {
        Entry {
            project_id: None,
            handle: String::new(),
            slug: String::new(),
            title: String::new(),
            branch: "main".into(),
            version_locked: false,
            locked_tag: None,
            custom_sha: None,
            custom_filename: None,
            author: String::new(),
        }
    }
}

impl Entry {
    fn from_doc(d: &Document) -> R<Self> {
        Ok(Entry {
            project_id: opt_oid(d, "project_id")?,
            handle: s_or(d, "handle", "")?,
            slug: s_or(d, "slug", "")?,
            title: s_or(d, "title", "")?,
            branch: s_or(d, "branch", "main")?,
            version_locked: boolean(d, "version_locked", false)?,
            locked_tag: opt_s(d, "locked_tag")?,
            custom_sha: opt_s(d, "custom_sha")?,
            custom_filename: opt_s(d, "custom_filename")?,
            author: s_or(d, "author", "")?,
        })
    }

    fn to_doc(&self) -> Document {
        doc! {
            "project_id": self.project_id.map_or(Bson::Null, Bson::ObjectId),
            "handle": &self.handle,
            "slug": &self.slug,
            "title": &self.title,
            "branch": &self.branch,
            "version_locked": self.version_locked,
            "locked_tag": opt_str(&self.locked_tag),
            "custom_sha": opt_str(&self.custom_sha),
            "custom_filename": opt_str(&self.custom_filename),
            "author": &self.author,
        }
    }

    /// The custom-upload sha when set (Python's truthiness: `""` is unset).
    pub fn custom(&self) -> Option<&str> {
        self.custom_sha.as_deref().filter(|s| !s.is_empty())
    }
}

/// `ModpackVariant`.
#[derive(Clone, Debug, PartialEq)]
pub struct Variant {
    pub name: String,
    pub label: String,
    pub entries: Vec<Entry>,
}

impl Variant {
    fn from_doc(d: &Document) -> R<Self> {
        Ok(Variant {
            name: s(d, "name")?,
            label: s_or(d, "label", "")?,
            entries: sub_docs(d, "entries")?.iter().map(Entry::from_doc).collect::<R<_>>()?,
        })
    }

    fn to_doc(&self) -> Document {
        let entries: Vec<Bson> = self.entries.iter().map(|e| Bson::Document(e.to_doc())).collect();
        doc! {"name": &self.name, "label": &self.label, "entries": entries}
    }

    /// `variant.label or variant.name`.
    pub fn display(&self) -> &str {
        if self.label.is_empty() { &self.name } else { &self.label }
    }
}

/// `mods_hub.models.Collaborator`.
#[derive(Clone, Debug, PartialEq)]
pub struct Collaborator {
    pub user_id: ObjectId,
    pub username: String,
}

impl Collaborator {
    fn from_doc(d: &Document) -> R<Self> {
        Ok(Collaborator { user_id: oid(d, "user_id")?, username: s(d, "username")? })
    }

    fn to_doc(&self) -> Document {
        doc! {"user_id": self.user_id, "username": &self.username}
    }
}

/// `ModpackProject`.
#[derive(Clone, Debug, PartialEq)]
pub struct Pack {
    pub id: ObjectId,
    pub slug: String,
    pub title: String,
    pub summary: String,
    pub description: String,
    pub warnings: String,
    pub tags: Vec<String>,
    pub owner_id: ObjectId,
    pub owner_username: String,
    pub owner_handle: String,
    pub collaborators: Vec<Collaborator>,
    pub visibility: String,
    pub banner_sha: Option<String>,
    pub preview_shas: Vec<String>,
    pub discord_url: Option<String>,
    pub website_url: Option<String>,
    pub donation_urls: Vec<String>,
    pub download_count: i64,
    pub star_count: i64,
    pub variants: Vec<Variant>,
    pub default_variant: String,
    pub taken_down: bool,
    pub takedown_reason: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Pack {
    pub fn from_doc(d: &Document) -> R<Self> {
        let now = dt::now();
        let id = match d.get("_id") {
            Some(v) => oid_of("_id", v)?,
            None => return missing("_id"),
        };
        Ok(Pack {
            id,
            slug: s(d, "slug")?,
            title: s(d, "title")?,
            summary: s_or(d, "summary", "")?,
            description: s_or(d, "description", "")?,
            warnings: s_or(d, "warnings", "")?,
            tags: strs(d, "tags")?,
            owner_id: oid(d, "owner_id")?,
            owner_username: s(d, "owner_username")?,
            owner_handle: s_or(d, "owner_handle", "")?,
            collaborators: sub_docs(d, "collaborators")?.iter().map(Collaborator::from_doc).collect::<R<_>>()?,
            visibility: literal(d, "visibility", VISIBILITY, "draft")?,
            banner_sha: opt_s(d, "banner_sha")?,
            preview_shas: strs(d, "preview_shas")?,
            discord_url: opt_s(d, "discord_url")?,
            website_url: opt_s(d, "website_url")?,
            donation_urls: strs(d, "donation_urls")?,
            download_count: int(d, "download_count", 0)?,
            star_count: int(d, "star_count", 0)?,
            variants: sub_docs(d, "variants")?.iter().map(Variant::from_doc).collect::<R<_>>()?,
            default_variant: s_or(d, "default_variant", DEFAULT_VARIANT)?,
            taken_down: boolean(d, "taken_down", false)?,
            takedown_reason: opt_s(d, "takedown_reason")?,
            created_at: datetime_or(d, "created_at", now)?,
            updated_at: datetime_or(d, "updated_at", now)?,
        })
    }

    /// Every model field after `_id`, in model order.
    pub fn fields(&self) -> Document {
        let collaborators: Vec<Bson> = self.collaborators.iter().map(|c| Bson::Document(c.to_doc())).collect();
        let variants: Vec<Bson> = self.variants.iter().map(|v| Bson::Document(v.to_doc())).collect();
        doc! {
            "slug": &self.slug,
            "title": &self.title,
            "summary": &self.summary,
            "description": &self.description,
            "warnings": &self.warnings,
            "tags": &self.tags,
            "owner_id": self.owner_id,
            "owner_username": &self.owner_username,
            "owner_handle": &self.owner_handle,
            "collaborators": collaborators,
            "visibility": &self.visibility,
            "banner_sha": opt_str(&self.banner_sha),
            "preview_shas": &self.preview_shas,
            "discord_url": opt_str(&self.discord_url),
            "website_url": opt_str(&self.website_url),
            "donation_urls": &self.donation_urls,
            "download_count": bson_int(self.download_count),
            "star_count": bson_int(self.star_count),
            "variants": variants,
            "default_variant": &self.default_variant,
            "taken_down": self.taken_down,
            "takedown_reason": opt_str(&self.takedown_reason),
            "created_at": bdt(self.created_at),
            "updated_at": bdt(self.updated_at),
        }
    }

    /// `_id` then the fields: what `insert()` and `save()` send.
    pub fn to_doc(&self) -> Document {
        let mut d = doc! {"_id": self.id};
        d.extend(self.fields());
        d
    }

    /// `_find_variant` without the error: the named one, else the default,
    /// else the first.
    pub fn find_variant(&self, name: Option<&str>) -> Option<usize> {
        if let Some(n) = name.filter(|n| !n.is_empty())
            && let Some(i) = self.variants.iter().position(|v| v.name == n)
        {
            return Some(i);
        }
        self.variants.iter().position(|v| v.name == self.default_variant).or(if self.variants.is_empty() {
            None
        } else {
            Some(0)
        })
    }

    pub fn variant_index(&self, name: &str) -> Option<usize> {
        self.variants.iter().position(|v| v.name == name)
    }

    /// The variant a card counts: the default, else the first.
    pub fn card_variant(&self) -> Option<&Variant> {
        self.variants.iter().find(|v| v.name == self.default_variant).or(self.variants.first())
    }

    pub fn can_edit(&self, actor: Option<ObjectId>) -> bool {
        actor.is_some_and(|a| a == self.owner_id || self.collaborators.iter().any(|c| c.user_id == a))
    }

    pub fn is_primary_owner(&self, actor: Option<ObjectId>) -> bool {
        actor == Some(self.owner_id)
    }

    pub fn can_view(&self, actor: Option<ObjectId>) -> bool {
        if self.can_edit(actor) {
            return true;
        }
        !self.taken_down && matches!(self.visibility.as_str(), "public" | "unlisted")
    }
}

/// `ModpackStar` as `insert()` writes it.
pub fn star_doc(modpack_id: ObjectId, site_user_id: ObjectId, now: DateTime<Utc>) -> Document {
    doc! {"_id": ObjectId::new(), "modpack_id": modpack_id, "site_user_id": site_user_id, "created_at": bdt(now)}
}

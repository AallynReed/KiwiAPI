//! The slice of `mods_hub/service.py` these routes stand on: resolving a mod
//! by `<handle>/<slug>`, edit rights and the visibility gate.

use chrono::{DateTime, Duration, Utc};
use mongodb::bson::{Bson, Document, doc, oid::ObjectId};

use super::{
    docs::{read_bool, read_int, read_oid, read_opt_date, read_str},
    store::{Res, Store},
};
use crate::{
    auth::models::{SITE_USERS, SiteUser},
    error::{ApiError, ErrorCode},
};

pub const PROJECTS: &str = "mod_projects";
pub const RELEASES: &str = "mod_releases";
pub const STRAY_HANDLE: &str = "stray";

const REACHABLE: [&str; 3] = ["public", "public_website_only", "unlisted"];

#[derive(Debug, Clone)]
pub struct Project {
    pub id: ObjectId,
    pub slug: String,
    pub title: String,
    pub owner_id: Option<ObjectId>,
    pub owner_handle: String,
    pub collaborators: Vec<ObjectId>,
    pub issues_enabled: bool,
    pub visibility: String,
    pub is_remix: bool,
    pub remix_status: String,
    pub remix_requested_at: Option<DateTime<Utc>>,
    pub taken_down: bool,
}

impl Project {
    pub fn from_doc(d: &Document) -> Self {
        let collaborators = d
            .get_array("collaborators")
            .map(|a| a.iter().filter_map(|c| c.as_document()?.get_object_id("user_id").ok()).collect())
            .unwrap_or_default();
        Project {
            id: read_oid(d, "_id").unwrap_or_else(|| ObjectId::from_bytes([0; 12])),
            slug: read_str(d, "slug"),
            title: read_str(d, "title"),
            owner_id: read_oid(d, "owner_id"),
            owner_handle: read_str(d, "owner_handle"),
            collaborators,
            issues_enabled: read_bool(d, "issues_enabled", true),
            visibility: d.get_str("visibility").unwrap_or("draft").to_owned(),
            is_remix: read_bool(d, "is_remix", false),
            remix_status: d.get_str("remix_status").unwrap_or("pending").to_owned(),
            remix_requested_at: read_opt_date(d, "remix_requested_at"),
            taken_down: read_bool(d, "taken_down", false),
        }
    }

    /// `int(...)` of a counter field, for callers that need one.
    pub fn counter(d: &Document, key: &str) -> i64 {
        read_int(d, key, 0)
    }
}

/// `can_edit(project, actor)`: the primary owner or a collaborator.
pub fn can_edit(project: &Project, actor: Option<&SiteUser>) -> bool {
    actor.is_some_and(|a| project.owner_id == Some(a.id) || project.collaborators.contains(&a.id))
}

pub fn is_site_master(store: &Store, actor: Option<&SiteUser>) -> bool {
    actor.is_some_and(|a| a.username == store.site_master_handle())
}

/// `effective_visibility(project)`.
pub fn effective_visibility(project: &Project, now: DateTime<Utc>) -> &str {
    if !project.is_remix || !matches!(project.visibility.as_str(), "public" | "public_website_only") {
        return &project.visibility;
    }
    if project.remix_status == "denied" {
        return "hidden";
    }
    if project.remix_status == "approved" || project.remix_requested_at.is_some_and(|t| now - t >= Duration::days(7)) {
        return "public_website_only";
    }
    "hidden"
}

/// `can_view(project, viewer)`.
pub fn can_view(store: &Store, project: &Project, viewer: Option<&SiteUser>) -> bool {
    if can_edit(project, viewer) {
        return true;
    }
    if project.taken_down && !is_site_master(store, viewer) {
        return false;
    }
    REACHABLE.contains(&effective_visibility(project, store.now()))
}

/// `get_project(handle, slug)`.
pub async fn get_project(store: &Store, handle: &str, slug: &str) -> Res<Option<Project>> {
    let handle = kiwi_core::pystr::strip(handle).to_lowercase();
    if handle == STRAY_HANDLE {
        let found = store.find_one(PROJECTS, doc! {"$and": [{"is_stray": true}, {"slug": slug}]}).await?;
        return Ok(found.as_ref().map(Project::from_doc));
    }
    let Some(user) = store.find_one(SITE_USERS, doc! {"username": &handle}).await? else {
        return Ok(None);
    };
    let Some(owner) = user.get("_id").cloned() else { return Ok(None) };
    let found = store.find_one(PROJECTS, doc! {"$and": [{"owner_id": owner}, {"slug": slug}]}).await?;
    Ok(found.as_ref().map(Project::from_doc))
}

pub async fn get_project_by_id(store: &Store, id: Bson) -> Res<Option<Project>> {
    Ok(store.find_one(PROJECTS, doc! {"_id": id}).await?.as_ref().map(Project::from_doc))
}

pub fn not_found() -> ApiError {
    ApiError::not_found("Mod project not found")
}

pub fn not_public() -> ApiError {
    ApiError::new(axum::http::StatusCode::NOT_FOUND, ErrorCode::NotPublic, "This mod isn't public yet.")
}

/// `get_for_view(handle, slug, viewer)`.
pub async fn get_for_view(store: &Store, handle: &str, slug: &str, viewer: Option<&SiteUser>) -> Res<Project> {
    let project = get_project(store, handle, slug).await?.ok_or_else(not_found)?;
    if !can_view(store, &project, viewer) {
        if project.visibility == "draft" && !project.taken_down {
            return Err(not_public());
        }
        return Err(not_found());
    }
    Ok(project)
}

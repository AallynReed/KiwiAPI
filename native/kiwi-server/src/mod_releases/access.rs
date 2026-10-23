//! Loading releases and projects, and the visibility rules of `service.py`
//! (`can_edit`, `can_view`, `effective_visibility`, `release_with_project`).

use chrono::Duration;
use mongodb::bson::{Bson, Document, doc, oid::ObjectId};

use super::model::{Doc, PROJECT, RELEASE};
use crate::{auth::models::SiteUser, dt, error::ApiError, state::AppState};

pub const PROJECTS: &str = "mod_projects";
pub const RELEASES: &str = "mod_releases";
pub const STRAY_HANDLE: &str = "stray";

pub fn internal(e: impl std::fmt::Display) -> ApiError {
    tracing::error!("mods hub: {e}");
    ApiError::internal()
}

fn db(state: &AppState) -> Result<&mongodb::Database, ApiError> {
    state.inner.db.mongo.as_ref().ok_or_else(|| internal("MONGO_URI not configured"))
}

pub async fn find_one(state: &AppState, coll: &str, filter: Document) -> Result<Option<Document>, ApiError> {
    db(state)?.collection::<Document>(coll).find_one(filter).await.map_err(internal)
}

fn load(spec: &[super::model::F], coll: &str, d: Option<Document>) -> Result<Option<Doc>, ApiError> {
    match d {
        None => Ok(None),
        Some(d) => Doc::load(spec, &d).map(Some).ok_or_else(|| internal(format!("{coll}: invalid document"))),
    }
}

pub async fn project_by(state: &AppState, filter: Document) -> Result<Option<Doc>, ApiError> {
    load(PROJECT, PROJECTS, find_one(state, PROJECTS, filter).await?)
}

pub async fn release_by(state: &AppState, filter: Document) -> Result<Option<Doc>, ApiError> {
    load(RELEASE, RELEASES, find_one(state, RELEASES, filter).await?)
}

/// `to_oid(value)`.
pub fn to_oid(s: &str) -> Option<ObjectId> {
    if s.len() != 24 {
        return None;
    }
    ObjectId::parse_str(s).ok()
}

/// `get_release(release_id)`.
pub async fn get_release(state: &AppState, release_id: &str) -> Result<Option<Doc>, ApiError> {
    match to_oid(release_id) {
        Some(oid) => release_by(state, doc! {"_id": oid}).await,
        None => Ok(None),
    }
}

pub async fn project_by_id(state: &AppState, id: Option<ObjectId>) -> Result<Option<Doc>, ApiError> {
    match id {
        Some(id) => project_by(state, doc! {"_id": id}).await,
        None => Ok(None),
    }
}

/// `get_project(handle, slug)`.
pub async fn get_project(state: &AppState, handle: &str, slug: &str) -> Result<Option<Doc>, ApiError> {
    let handle = kiwi_core::pystr::strip(handle).to_lowercase();
    if handle == STRAY_HANDLE {
        return project_by(state, doc! {"is_stray": true, "slug": slug}).await;
    }
    let Some(user) = find_one(state, crate::auth::models::SITE_USERS, doc! {"username": handle}).await? else {
        return Ok(None);
    };
    let Some(Bson::ObjectId(uid)) = user.get("_id") else { return Ok(None) };
    project_by(state, doc! {"owner_id": *uid, "slug": slug}).await
}

pub fn can_edit(project: &Doc, actor: Option<&SiteUser>) -> bool {
    let Some(actor) = actor else { return false };
    project.oid("owner_id") == Some(actor.id) || project.collab_ids().contains(&actor.id)
}

pub fn is_primary_owner(project: &Doc, actor: Option<&SiteUser>) -> bool {
    actor.is_some_and(|a| project.oid("owner_id") == Some(a.id))
}

pub fn is_site_master(state: &AppState, actor: Option<&SiteUser>) -> bool {
    actor.is_some_and(|a| a.username == state.settings().site_master_handle)
}

/// `effective_visibility(project)`.
pub fn effective_visibility(project: &Doc) -> String {
    let vis = project.str("visibility");
    if !project.bool("is_remix") || !matches!(vis, "public" | "public_website_only") {
        return vis.to_owned();
    }
    let status = project.str("remix_status");
    if status == "denied" {
        return "hidden".into();
    }
    let aged = project.dt("remix_requested_at").is_some_and(|t| dt::now() - t >= Duration::days(7));
    if status == "approved" || aged { "public_website_only".into() } else { "hidden".into() }
}

/// `can_view(project, viewer)`.
pub fn can_view(state: &AppState, project: &Doc, viewer: Option<&SiteUser>) -> bool {
    if can_edit(project, viewer) {
        return true;
    }
    if project.bool("taken_down") && !is_site_master(state, viewer) {
        return false;
    }
    matches!(effective_visibility(project).as_str(), "public" | "public_website_only" | "unlisted")
}

fn release_not_found() -> ApiError {
    ApiError::not_found("Release not found")
}

/// `release_with_project(release_id, viewer)`.
pub async fn release_with_project(
    state: &AppState,
    release_id: &str,
    viewer: Option<&SiteUser>,
) -> Result<(Doc, Doc), ApiError> {
    let release = get_release(state, release_id).await?.ok_or_else(release_not_found)?;
    let project = project_by_id(state, release.oid("project_id")).await?;
    let project = project.filter(|p| can_view(state, p, viewer)).ok_or_else(release_not_found)?;
    let is_owner = viewer.is_some_and(|v| project.oid("owner_id") == Some(v.id));
    if release.str("status") != "published" && !is_owner {
        return Err(release_not_found());
    }
    Ok((release, project))
}

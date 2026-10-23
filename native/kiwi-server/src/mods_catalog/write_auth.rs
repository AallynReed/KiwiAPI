//! `mods_hub/write_auth.py`'s `get_mod_write_user`: the creator's own site
//! session, or an API token with `mods:write` whose account holds a live
//! creator link. API callers are default-denied outside `API_ROUTES`.

use axum::{
    extract::{FromRequestParts, MatchedPath},
    http::request::Parts,
};
use mongodb::bson::{doc, oid::ObjectId};

use super::{
    model::{self, CreatorLink, Project, Release},
    service::{coll, find_docs, get_project, project_one, user_by_id, users},
};
use crate::{
    auth::{
        Rejection,
        extract::{authenticate_site, bearer, enforce_token_limits, resolve_token},
        models::{SiteUser, User},
        scopes,
        site::site_credential,
    },
    dt,
    error::ApiError,
    middleware::pyurl::first,
    ratelimit::RateLimitInfo,
    state::AppState,
    validation::parse_qsl,
};

pub const CREATOR_HEADER: &str = "X-Kiwi-Creator";

/// `_API_ROUTES`: route template -> methods an API caller may use.
pub const API_ROUTES: &[(&str, &[&str])] = &[
    ("/v1/mods/hub/me/projects", &["GET"]),
    ("/v1/mods/hub/me/projects/{handle}/{slug}", &["GET"]),
    ("/v1/mods/hub/me/projects/{handle}/{slug}/releases", &["GET"]),
    ("/v1/mods/hub/me/projects/{handle}/{slug}/commits", &["GET"]),
    ("/v1/mods/hub/me/projects/{handle}/{slug}/placement", &["GET"]),
    ("/v1/mods/hub/projects", &["POST"]),
    ("/v1/mods/hub/projects/{handle}/{slug}", &["PATCH"]),
    ("/v1/mods/hub/projects/{handle}/{slug}/fork", &["POST"]),
    ("/v1/mods/hub/projects/{handle}/{slug}/banner", &["POST", "DELETE"]),
    ("/v1/mods/hub/projects/{handle}/{slug}/previews", &["POST"]),
    ("/v1/mods/hub/projects/{handle}/{slug}/previews/{sha}", &["DELETE"]),
    ("/v1/mods/hub/projects/{handle}/{slug}/branches", &["POST"]),
    ("/v1/mods/hub/projects/{handle}/{slug}/branches/{name}", &["DELETE"]),
    ("/v1/mods/hub/projects/{handle}/{slug}/commits", &["POST"]),
    ("/v1/mods/hub/projects/{handle}/{slug}/releases", &["POST"]),
    ("/v1/mods/hub/projects/{handle}/{slug}/releases/upload", &["POST"]),
    ("/v1/mods/hub/projects/{handle}/{slug}/fix-placement", &["POST"]),
    ("/v1/mods/hub/releases/{release_id}", &["PATCH", "DELETE"]),
    ("/v1/mods/hub/releases/{release_id}/config", &["POST"]),
];

const CREATE_ROUTES: [&str; 2] = ["/v1/mods/hub/projects", "/v1/mods/hub/projects/{handle}/{slug}/fork"];

/// The creator to act as, plus the token's `X-RateLimit-*` on the API path.
#[derive(Clone, Debug)]
pub struct WriteUser {
    pub user: SiteUser,
    pub rate_limit: Option<RateLimitInfo>,
}

fn forbidden(message: &str) -> Rejection {
    Rejection::forbidden("forbidden", message)
}

fn internal(e: ApiError) -> Rejection {
    Rejection::from(e)
}

async fn live_links(state: &AppState, api_user: &User) -> Result<Vec<CreatorLink>, Rejection> {
    let docs = find_docs(
        &coll(state, model::CREATOR_LINKS),
        doc! {"$and": [{"api_user_id": api_user.id}, {"revoked": false}]},
        None,
        None,
        None,
    )
    .await
    .map_err(internal)?;
    Ok(docs.iter().map(CreatorLink::from_doc).collect())
}

async fn target_project(state: &AppState, parts: &Parts) -> Result<Option<Project>, Rejection> {
    let params = crate::admin::path_params(parts);
    let get = |k: &str| params.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone()).filter(|v| !v.is_empty());
    if let (Some(h), Some(s)) = (get("handle"), get("slug")) {
        return get_project(state, &h, &s).await.map_err(internal);
    }
    if let Some(rid) = get("release_id") {
        let Ok(oid) = ObjectId::parse_str(&rid) else { return Ok(None) };
        let rel = coll(state, model::RELEASES)
            .find_one(doc! {"_id": oid})
            .await
            .map_err(|e| internal(super::service::internal(e)))?;
        if let Some(r) = rel.as_ref().map(Release::from_doc) {
            return project_one(state, doc! {"_id": r.project_id}).await.map_err(internal);
        }
    }
    Ok(None)
}

async fn pick_creator(
    state: &AppState,
    parts: &Parts,
    links: &[CreatorLink],
    project: Option<&Project>,
) -> Result<CreatorLink, Rejection> {
    if let Some(p) = project {
        return links
            .iter()
            .find(|l| Some(l.site_user_id) == p.owner_id)
            .cloned()
            .ok_or_else(|| forbidden("This token isn't connected to that mod's creator."));
    }
    let from_query = parts
        .uri
        .query()
        .map(parse_qsl)
        .and_then(|q| q.into_iter().rev().find(|(k, _)| k == "creator").map(|(_, v)| v))
        .filter(|v| !v.is_empty());
    let raw =
        from_query.or_else(|| first(&parts.headers, CREATOR_HEADER).filter(|v| !v.is_empty())).unwrap_or_default();
    let wanted = kiwi_core::pystr::strip(&raw).to_lowercase();
    if wanted.is_empty() {
        if links.len() == 1 {
            return Ok(links[0].clone());
        }
        return Err(Rejection::from(ApiError::bad_request(format!(
            "You're connected to several creators - name one with ?creator=<handle> or the {CREATOR_HEADER} header."
        ))));
    }
    let creator =
        users(state).find_one(doc! {"username": &wanted}).await.map_err(|e| internal(super::service::internal(e)))?;
    creator
        .and_then(|c| links.iter().find(|l| l.site_user_id == c.id).cloned())
        .ok_or_else(|| forbidden(&format!("You aren't connected to the creator '{wanted}'.")))
}

async fn authorize_api(state: &AppState, parts: &Parts, cred: &str) -> Result<WriteUser, Rejection> {
    let (user, token) = resolve_token(state, cred).await?;
    if !scopes::mask_grants(token.scopes, "mods:write") {
        return Err(Rejection::forbidden("insufficient_scope", "This token is missing the required scope: mods:write"));
    }
    let info = enforce_token_limits(state, parts, &user, &token, 1, None).await?;
    let route = parts.extensions.get::<MatchedPath>().map(|m| m.as_str().to_owned()).unwrap_or_default();
    let method = parts.method.as_str();
    let allowed = API_ROUTES.iter().find(|(r, _)| *r == route).is_some_and(|(_, ms)| ms.contains(&method));
    if !allowed {
        return Err(forbidden(
            "This action is only available to the creator on the website. A connected API account can create mods, \
             cut, publish and delete releases, and edit mod metadata, images and visibility.",
        ));
    }
    let links = live_links(state, &user).await?;
    if links.is_empty() {
        return Err(forbidden(
            "This account isn't connected to any creator. Ask a creator for their creator token and add them from \
             the dev portal.",
        ));
    }
    let project = target_project(state, parts).await?;
    let link = pick_creator(state, parts, &links, project.as_ref()).await?;
    if let Some(p) = &project
        && (p.owner_id != Some(link.site_user_id) || !(link.all_projects || link.project_ids.contains(&p.id)))
    {
        return Err(forbidden("That mod isn't covered by your connection to this creator."));
    }
    if CREATE_ROUTES.contains(&route.as_str()) && !link.all_projects {
        return Err(forbidden("This creator limited your connection to specific mods, so it can't create new ones."));
    }
    let creator = user_by_id(state, link.site_user_id).await.map_err(internal)?;
    let Some(creator) = creator.filter(|c| c.is_active && !c.is_deleted) else {
        return Err(forbidden("That creator's account is no longer active."));
    };
    let _ = coll(state, model::CREATOR_LINKS)
        .update_one(
            doc! {"_id": link.id},
            doc! {"$set": {"last_used_at": model::date(&dt::now())}, "$inc": {"request_count": 1}},
        )
        .await;
    Ok(WriteUser { user: creator, rate_limit: Some(info) })
}

impl FromRequestParts<AppState> for WriteUser {
    type Rejection = Rejection;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Rejection> {
        if let Some(cred) = bearer(&parts.headers)
            && cred.matches('.').count() != 2
        {
            return authorize_api(state, parts, &cred).await;
        }
        let cred = site_credential(parts);
        let (user, _) = authenticate_site(state, cred.as_deref()).await?;
        Ok(WriteUser { user, rate_limit: None })
    }
}

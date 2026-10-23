//! `app/trove/mods_hub/write_auth.py`: who may call a Mods Hub write route.
//!
//! [`ModWriteAuth`] is `get_mod_write_auth` (and so `get_mod_write_user`): a
//! bearer that is not a JWT is an API token that needs `mods:write`, the
//! token limits, a route on the default-deny [`API_ROUTES`] list and a live
//! creator link covering the target mod; anything else is the site session.
//! Either way `user` is the creator the route acts for. Return `rate_limit`
//! as a response part where the route returns a dict (the API path sets
//! `X-RateLimit-*` there, as Python does).

pub mod docs;
pub mod links;
#[cfg(test)]
pub mod mem;
pub mod project;
pub mod site;
pub mod store;

#[cfg(test)]
pub mod testkit;
#[cfg(test)]
mod tests;

use axum::{
    extract::{FromRequestParts, MatchedPath},
    http::{StatusCode, request::Parts},
};
use mongodb::bson::{Bson, doc, oid::ObjectId};

use self::{
    links::CreatorLink,
    project::{PROJECTS, Project, RELEASES},
    store::{Res, Store},
};
use crate::{
    auth::{
        Rejection,
        extract::{authenticate_site, bearer, enforce_token_limits, resolve_token},
        models::{ApiToken, SITE_USERS, SiteUser},
        scopes,
        site::site_credential,
    },
    error::{ApiError, ErrorCode},
    ratelimit::RateLimitInfo,
    state::AppState,
    usage::fastapi_template,
};

pub const MODS_WRITE_SCOPE: &str = "mods:write";
pub const CREATOR_HEADER: &str = "X-Kiwi-Creator";

/// Route template -> methods an API caller may use; anything absent is website-only.
pub const API_ROUTES: &[(&str, &[&str])] = &[
    ("/v1/mods/hub/me/projects", &["GET"]),
    ("/v1/mods/hub/me/projects/{handle}/{slug}", &["GET"]),
    ("/v1/mods/hub/me/projects/{handle}/{slug}/releases", &["GET"]),
    ("/v1/mods/hub/me/projects/{handle}/{slug}/commits", &["GET"]),
    ("/v1/mods/hub/me/projects/{handle}/{slug}/placement", &["GET"]),
    ("/v1/mods/hub/projects", &["POST"]),
    ("/v1/mods/hub/projects/{handle}/{slug}", &["PATCH"]),
    ("/v1/mods/hub/projects/{handle}/{slug}/fork", &["POST"]),
    ("/v1/mods/hub/projects/{handle}/{slug}/banner", &["DELETE", "POST"]),
    ("/v1/mods/hub/projects/{handle}/{slug}/previews", &["POST"]),
    ("/v1/mods/hub/projects/{handle}/{slug}/previews/{sha}", &["DELETE"]),
    ("/v1/mods/hub/projects/{handle}/{slug}/branches", &["POST"]),
    ("/v1/mods/hub/projects/{handle}/{slug}/branches/{name}", &["DELETE"]),
    ("/v1/mods/hub/projects/{handle}/{slug}/commits", &["POST"]),
    ("/v1/mods/hub/projects/{handle}/{slug}/releases", &["POST"]),
    ("/v1/mods/hub/projects/{handle}/{slug}/releases/upload", &["POST"]),
    ("/v1/mods/hub/projects/{handle}/{slug}/fix-placement", &["POST"]),
    ("/v1/mods/hub/releases/{release_id}", &["DELETE", "PATCH"]),
    ("/v1/mods/hub/releases/{release_id}/config", &["POST"]),
];

/// Routes that bring a new mod into existence: they need an all-projects link.
pub const CREATE_ROUTES: &[&str] = &["/v1/mods/hub/projects", "/v1/mods/hub/projects/{handle}/{slug}/fork"];

/// The resolved write caller; `link`, `token` and `rate_limit` only on the API path.
#[derive(Debug, Clone)]
pub struct ModWriteAuth {
    pub user: SiteUser,
    pub link: Option<CreatorLink>,
    pub token: Option<ApiToken>,
    pub rate_limit: Option<RateLimitInfo>,
}

impl ModWriteAuth {
    pub fn via_api(&self) -> bool {
        self.link.is_some()
    }
}

/// What the API path reads off the request.
#[derive(Debug, Clone, Default)]
pub struct Target {
    pub route: String,
    pub method: String,
    pub params: Vec<(String, String)>,
    /// `?creator=` or the `X-Kiwi-Creator` header, raw.
    pub creator: Option<String>,
}

impl Target {
    pub fn from_parts(parts: &Parts) -> Self {
        let route = parts.extensions.get::<MatchedPath>().map(|m| fastapi_template(m.as_str())).unwrap_or_default();
        let params = crate::admin::path_params(parts);
        Target::new(route, parts.method.as_str(), params, parts.uri.query(), &parts.headers)
    }

    /// `?creator=` (last one) or else the header, as `request.query_params.get(...) or headers.get(...)`.
    pub fn new(
        route: String,
        method: &str,
        params: Vec<(String, String)>,
        query: Option<&str>,
        headers: &axum::http::HeaderMap,
    ) -> Self {
        let query = query.map(crate::validation::parse_qsl).unwrap_or_default();
        let from_query = query.into_iter().rev().find(|(k, _)| k == "creator").map(|(_, v)| v);
        let from_header = headers.get(CREATOR_HEADER).map(|v| crate::middleware::pyurl::latin1(v.as_bytes()));
        let creator = from_query.filter(|v| !v.is_empty()).or(from_header);
        Target { route, method: method.to_owned(), params, creator }
    }

    fn param(&self, name: &str) -> Option<&str> {
        self.params.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str()).filter(|v| !v.is_empty())
    }
}

fn forbidden(message: &str) -> ApiError {
    ApiError::forbidden(message)
}

/// `_target_project`: from `{handle}/{slug}`, or `{release_id}` through its release.
async fn target_project(store: &Store, t: &Target) -> Res<Option<Project>> {
    if let (Some(handle), Some(slug)) = (t.param("handle"), t.param("slug")) {
        return project::get_project(store, handle, slug).await;
    }
    let Some(oid) = t.param("release_id").and_then(docs::to_oid) else { return Ok(None) };
    let Some(release) = store.get(RELEASES, oid).await? else { return Ok(None) };
    let pid = release.get("project_id").cloned().unwrap_or(Bson::Null);
    Ok(store.find_one(PROJECTS, doc! {"_id": pid}).await?.as_ref().map(Project::from_doc))
}

async fn find_site_user(store: &Store, filter: mongodb::bson::Document) -> Res<Option<SiteUser>> {
    match store.find_one(SITE_USERS, filter).await? {
        Some(d) => mongodb::bson::from_document(d).map(Some).map_err(|e| {
            tracing::error!("site_users row: {e}");
            ApiError::internal()
        }),
        None => Ok(None),
    }
}

/// `_pick_creator`.
async fn pick_creator(store: &Store, t: &Target, links: &[CreatorLink], project: Option<&Project>) -> Res<CreatorLink> {
    if let Some(p) = project {
        return links
            .iter()
            .find(|l| p.owner_id == Some(l.site_user_id))
            .cloned()
            .ok_or_else(|| forbidden("This token isn't connected to that mod's creator."));
    }
    let wanted = kiwi_core::pystr::strip(t.creator.as_deref().unwrap_or("")).to_lowercase();
    if wanted.is_empty() {
        if links.len() == 1 {
            return Ok(links[0].clone());
        }
        return Err(ApiError::bad_request(format!(
            "You're connected to several creators - name one with ?creator=<handle> or the {CREATOR_HEADER} header."
        )));
    }
    let creator = find_site_user(store, doc! {"username": &wanted}).await?;
    creator
        .and_then(|c| links.iter().find(|l| l.site_user_id == c.id).cloned())
        .ok_or_else(|| forbidden(&format!("You aren't connected to the creator '{wanted}'.")))
}

/// Everything `_authorize_api` decides after the token passed its scope and
/// limits: the allowlist, the link, its coverage, the creator's account.
pub async fn authorize_link(store: &Store, api_user_id: ObjectId, t: &Target) -> Res<(SiteUser, CreatorLink)> {
    let allowed = API_ROUTES.iter().find(|(r, _)| *r == t.route).map(|(_, m)| *m);
    if !allowed.is_some_and(|m| m.contains(&t.method.as_str())) {
        return Err(forbidden(
            "This action is only available to the creator on the website. A connected API account can create mods, \
             cut, publish and delete releases, and edit mod metadata, images and visibility.",
        ));
    }
    let links = links::live_links(store, api_user_id).await?;
    if links.is_empty() {
        return Err(forbidden(
            "This account isn't connected to any creator. Ask a creator for their creator token and add them from \
             the dev portal.",
        ));
    }
    let project = target_project(store, t).await?;
    let link = pick_creator(store, t, &links, project.as_ref()).await?;
    if let Some(p) = &project
        && !links::covers(&link, p)
    {
        return Err(forbidden("That mod isn't covered by your connection to this creator."));
    }
    if CREATE_ROUTES.contains(&t.route.as_str()) && !link.all_projects {
        return Err(forbidden("This creator limited your connection to specific mods, so it can't create new ones."));
    }
    let creator = find_site_user(store, doc! {"_id": link.site_user_id}).await?;
    let Some(creator) = creator.filter(|c| c.is_active && !c.is_deleted) else {
        return Err(forbidden("That creator's account is no longer active."));
    };
    links::touch(store, &link).await;
    Ok((creator, link))
}

/// The `mods:write` check, before the token limits.
pub fn require_write_scope(token: &ApiToken) -> Result<(), Rejection> {
    if scopes::mask_grants_bit(token.scopes, scopes::MODS_WRITE) {
        return Ok(());
    }
    Err(Rejection::new(
        StatusCode::FORBIDDEN,
        ErrorCode::InsufficientScope.as_str(),
        format!("This token is missing the required scope: {MODS_WRITE_SCOPE}"),
    ))
}

async fn authorize_api(state: &AppState, parts: &Parts, cred: &str) -> Result<ModWriteAuth, Rejection> {
    let (user, token) = resolve_token(state, cred).await?;
    require_write_scope(&token)?;
    let info = enforce_token_limits(state, parts, &user, &token, 1, None).await?;
    let (creator, link) = authorize_link(&Store::live(state), user.id, &Target::from_parts(parts)).await?;
    Ok(ModWriteAuth { user: creator, link: Some(link), token: Some(token), rate_limit: Some(info) })
}

/// `get_mod_write_auth`.
pub async fn mod_write_auth(state: &AppState, parts: &Parts) -> Result<ModWriteAuth, Rejection> {
    if let Some(cred) = bearer(&parts.headers)
        && cred.matches('.').count() != 2
    {
        return authorize_api(state, parts, &cred).await;
    }
    let (user, _) = authenticate_site(state, site_credential(parts).as_deref()).await?;
    Ok(ModWriteAuth { user, link: None, token: None, rate_limit: None })
}

impl FromRequestParts<AppState> for ModWriteAuth {
    type Rejection = Rejection;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Rejection> {
        mod_write_auth(state, parts).await
    }
}

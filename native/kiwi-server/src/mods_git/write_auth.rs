//! `mods_hub/write_auth.py` `get_mod_write_user` for the routes served here:
//! the creator's Dashboard session, or an API token with `mods:write` whose
//! account holds a live creator connection covering the mod (default-deny
//! allowlist: `api_allowed`).

use axum::http::request::Parts;
use futures::TryStreamExt;
use mongodb::bson::{Document, doc, oid::ObjectId};

use super::service;
use crate::{
    auth::{
        Rejection,
        extract::{authenticate_site, bearer, enforce_token_limits, resolve_token},
        models::{SITE_USERS, SiteUser},
        scopes,
        site::site_credential,
    },
    dt,
    error::ApiError,
    ratelimit::RateLimitInfo,
    state::AppState,
    validation::parse_qsl,
};

pub const CREATOR_HEADER: &str = "x-kiwi-creator";
const LINKS: &str = "mod_creator_links";

pub struct WriteUser {
    pub user: SiteUser,
    /// Set on the API path: the `X-RateLimit-*` a dict reply carries.
    pub rate_limit: Option<RateLimitInfo>,
}

fn forbidden(msg: &str) -> Rejection {
    Rejection::forbidden("forbidden", msg)
}

struct Link {
    id: ObjectId,
    site_user_id: ObjectId,
    all_projects: bool,
    project_ids: Vec<ObjectId>,
}

impl Link {
    fn from_doc(d: &Document) -> Option<Link> {
        Some(Link {
            id: d.get_object_id("_id").ok()?,
            site_user_id: d.get_object_id("site_user_id").ok()?,
            all_projects: d.get_bool("all_projects").unwrap_or(true),
            project_ids: d
                .get_array("project_ids")
                .map(|a| a.iter().filter_map(|x| x.as_object_id()).collect())
                .unwrap_or_default(),
        })
    }
}

/// `get_mod_write_user`. `api_allowed` is the route's entry in `_API_ROUTES`.
pub async fn write_user(
    state: &AppState,
    parts: &Parts,
    handle_slug: Option<(&str, &str)>,
    api_allowed: bool,
) -> Result<WriteUser, Rejection> {
    if let Some(cred) = bearer(&parts.headers).filter(|c| c.matches('.').count() != 2) {
        return api_user(state, parts, &cred, handle_slug, api_allowed).await;
    }
    let cred = site_credential(parts);
    let (user, _) = authenticate_site(state, cred.as_deref()).await?;
    Ok(WriteUser { user, rate_limit: None })
}

async fn api_user(
    state: &AppState,
    parts: &Parts,
    cred: &str,
    handle_slug: Option<(&str, &str)>,
    api_allowed: bool,
) -> Result<WriteUser, Rejection> {
    let (user, token) = resolve_token(state, cred).await?;
    if !scopes::mask_grants_bit(token.scopes, scopes::MODS_WRITE) {
        return Err(Rejection::forbidden("insufficient_scope", "This token is missing the required scope: mods:write"));
    }
    let info = enforce_token_limits(state, parts, &user, &token, 1, None).await?;
    if !api_allowed {
        return Err(forbidden(
            "This action is only available to the creator on the website. A connected API account can create \
             mods, cut, publish and delete releases, and edit mod metadata, images and visibility.",
        ));
    }
    let db = state.inner.db.mongo.as_ref().ok_or_else(|| Rejection::internal("MONGO_URI not configured"))?;
    let links: Vec<Link> = db
        .collection::<Document>(LINKS)
        .find(doc! {"api_user_id": user.id, "revoked": false})
        .await
        .map_err(Rejection::internal)?
        .try_collect::<Vec<Document>>()
        .await
        .map_err(Rejection::internal)?
        .iter()
        .filter_map(Link::from_doc)
        .collect();
    if links.is_empty() {
        return Err(forbidden(
            "This account isn't connected to any creator. Ask a creator for their creator token and add them \
             from the dev portal.",
        ));
    }
    let project = match handle_slug {
        Some((h, s)) => service::get_project(state, h, s).await.map_err(Rejection::from)?,
        None => None,
    };
    let link = match &project {
        Some(p) => links
            .iter()
            .find(|l| Some(l.site_user_id) == p.owner_id())
            .ok_or_else(|| forbidden("This token isn't connected to that mod's creator."))?,
        None => {
            let query = parse_qsl(parts.uri.query().unwrap_or(""));
            let from_query =
                query.iter().rev().find(|(k, _)| k == "creator").map(|(_, v)| v.clone()).unwrap_or_default();
            let raw = if from_query.is_empty() {
                parts.headers.get(CREATOR_HEADER).map(|v| crate::request_id::latin1(v.as_bytes())).unwrap_or_default()
            } else {
                from_query
            };
            let wanted = kiwi_core::pystr::strip(&raw).to_lowercase();
            if wanted.is_empty() {
                if links.len() == 1 {
                    &links[0]
                } else {
                    return Err(Rejection::from(ApiError::bad_request(
                        "You're connected to several creators - name one with ?creator=<handle> or the \
                         X-Kiwi-Creator header.",
                    )));
                }
            } else {
                let creator = db
                    .collection::<Document>(SITE_USERS)
                    .find_one(doc! {"username": &wanted})
                    .await
                    .map_err(Rejection::internal)?
                    .and_then(|d| d.get_object_id("_id").ok());
                links
                    .iter()
                    .find(|l| Some(l.site_user_id) == creator)
                    .ok_or_else(|| forbidden(&format!("You aren't connected to the creator '{wanted}'.")))?
            }
        }
    };
    if let Some(p) = &project {
        let covers = p.owner_id() == Some(link.site_user_id) && (link.all_projects || link.project_ids.contains(&p.id));
        if !covers {
            return Err(forbidden("That mod isn't covered by your connection to this creator."));
        }
    }
    let creator = db
        .collection::<SiteUser>(SITE_USERS)
        .find_one(doc! {"_id": link.site_user_id})
        .await
        .map_err(Rejection::internal)?
        .filter(|c| c.is_active && !c.is_deleted)
        .ok_or_else(|| forbidden("That creator's account is no longer active."))?;
    let _ = db
        .collection::<Document>(LINKS)
        .update_one(
            doc! {"_id": link.id},
            doc! {"$set": {"last_used_at": dt::to_bson(dt::now())}, "$inc": {"request_count": 1}},
        )
        .await;
    Ok(WriteUser { user: creator, rate_limit: Some(info) })
}

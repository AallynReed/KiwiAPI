//! `write_auth.get_mod_write_user` for the release writes: the creator's own
//! site session, or an API token with `mods:write` whose account holds a live
//! creator connection covering the mod. Every release write is on the API
//! allowlist, so the route check always passes here.

use axum::http::request::Parts;
use mongodb::bson::{Bson, Document, doc, oid::ObjectId};

use super::access;
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
};

const LINKS: &str = "mod_creator_links";
const CREATOR_HEADER: &str = "x-kiwi-creator";

pub struct WriteUser {
    pub user: SiteUser,
    /// Set on the API path (`_enforce_token_limits` headers).
    pub rate_limit: Option<RateLimitInfo>,
}

fn forbidden(msg: &str) -> Rejection {
    Rejection::forbidden("forbidden", msg)
}

fn api(e: ApiError) -> Rejection {
    Rejection::new(e.status, e.code, e.message)
}

struct Link {
    id: Bson,
    site_user_id: Option<ObjectId>,
    all_projects: bool,
    project_ids: Vec<ObjectId>,
}

fn parse_link(d: &Document) -> Link {
    Link {
        id: d.get("_id").cloned().unwrap_or(Bson::Null),
        site_user_id: d.get_object_id("site_user_id").ok(),
        all_projects: d.get_bool("all_projects").unwrap_or(true),
        project_ids: d
            .get_array("project_ids")
            .map(|a| a.iter().filter_map(Bson::as_object_id).collect())
            .unwrap_or_default(),
    }
}

/// `_target_project(request)`.
async fn target_project(state: &AppState, parts: &Parts) -> Result<Option<super::model::Doc>, ApiError> {
    let path = crate::middleware::pyurl::scope_path(&parts.uri);
    let segs: Vec<&str> = path.split('/').collect();
    let decode = |s: &str| s.to_owned();
    if segs.get(4) == Some(&"projects") {
        let (handle, slug) = (decode(segs.get(5).unwrap_or(&"")), decode(segs.get(6).unwrap_or(&"")));
        if !handle.is_empty() && !slug.is_empty() {
            return access::get_project(state, &handle, &slug).await;
        }
        return Ok(None);
    }
    let rid = decode(segs.get(5).unwrap_or(&""));
    if rid.is_empty() {
        return Ok(None);
    }
    match access::get_release(state, &rid).await? {
        Some(r) => access::project_by_id(state, r.oid("project_id")).await,
        None => Ok(None),
    }
}

fn query_param(parts: &Parts, name: &str) -> Option<String> {
    let pairs = parts.uri.query().map(crate::validation::parse_qsl).unwrap_or_default();
    pairs.into_iter().rev().find(|(k, _)| k == name).map(|(_, v)| v)
}

async fn authorize_api(state: &AppState, parts: &Parts, cred: &str) -> Result<WriteUser, Rejection> {
    let (user, token) = resolve_token(state, cred).await?;
    if !scopes::mask_grants_bit(token.scopes, scopes::MODS_WRITE) {
        return Err(Rejection::forbidden("insufficient_scope", "This token is missing the required scope: mods:write"));
    }
    let rate_limit = enforce_token_limits(state, parts, &user, &token, 1, None).await?;
    let db = state.inner.db.mongo.as_ref().ok_or_else(|| Rejection::internal("no mongo"))?;
    let mut cursor = db
        .collection::<Document>(LINKS)
        .find(doc! {"api_user_id": user.id, "revoked": false})
        .await
        .map_err(Rejection::internal)?;
    let mut links = Vec::new();
    while cursor.advance().await.map_err(Rejection::internal)? {
        links.push(parse_link(&cursor.deserialize_current().map_err(Rejection::internal)?));
    }
    if links.is_empty() {
        return Err(forbidden(
            "This account isn't connected to any creator. Ask a creator for their creator token and add them from the dev portal.",
        ));
    }
    let project = target_project(state, parts).await.map_err(api)?;
    let link = match &project {
        Some(p) => {
            let owner = p.oid("owner_id");
            links
                .iter()
                .find(|l| l.site_user_id.is_some() && l.site_user_id == owner)
                .ok_or_else(|| forbidden("This token isn't connected to that mod's creator."))?
        }
        None => {
            let wanted = query_param(parts, "creator")
                .filter(|s| !s.is_empty())
                .or_else(|| parts.headers.get(CREATOR_HEADER).map(|v| crate::request_id::latin1(v.as_bytes())))
                .unwrap_or_default();
            let wanted = kiwi_core::pystr::strip(&wanted).to_lowercase();
            if wanted.is_empty() {
                if links.len() == 1 {
                    &links[0]
                } else {
                    return Err(Rejection::new(
                        axum::http::StatusCode::BAD_REQUEST,
                        "bad_request",
                        "You're connected to several creators - name one with ?creator=<handle> or the X-Kiwi-Creator header.",
                    ));
                }
            } else {
                let creator = access::find_one(state, SITE_USERS, doc! {"username": &wanted}).await.map_err(api)?;
                let cid = creator.as_ref().and_then(|c| c.get_object_id("_id").ok());
                links
                    .iter()
                    .find(|l| cid.is_some() && l.site_user_id == cid)
                    .ok_or_else(|| forbidden(&format!("You aren't connected to the creator '{wanted}'.")))?
            }
        }
    };
    if let Some(p) = &project {
        let covers = p.oid("owner_id") == link.site_user_id && (link.all_projects || link.project_ids.contains(&p.id));
        if !covers {
            return Err(forbidden("That mod isn't covered by your connection to this creator."));
        }
    }
    let creator: Option<SiteUser> = match link.site_user_id {
        Some(id) => {
            db.collection::<SiteUser>(SITE_USERS).find_one(doc! {"_id": id}).await.map_err(Rejection::internal)?
        }
        None => None,
    };
    let creator = creator
        .filter(|c| c.is_active && !c.is_deleted)
        .ok_or_else(|| forbidden("That creator's account is no longer active."))?;
    let touch = db
        .collection::<Document>(LINKS)
        .update_one(
            doc! {"_id": link.id.clone()},
            doc! {"$set": {"last_used_at": dt::to_bson(dt::now())}, "$inc": {"request_count": 1}},
        )
        .await;
    if let Err(e) = touch {
        tracing::debug!("creator link touch: {e}");
    }
    Ok(WriteUser { user: creator, rate_limit: Some(rate_limit) })
}

/// `get_mod_write_user`.
pub async fn write_user(state: &AppState, parts: &Parts) -> Result<WriteUser, Rejection> {
    if let Some(cred) = bearer(&parts.headers)
        && cred.matches('.').count() != 2
    {
        return authorize_api(state, parts, &cred).await;
    }
    let cred = site_credential(parts);
    let (user, _) = authenticate_site(state, cred.as_deref()).await?;
    Ok(WriteUser { user, rate_limit: None })
}

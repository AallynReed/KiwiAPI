//! `app/trove/mods_hub/creators.py`: the creator token and the creator <->
//! developer connections.

use mongodb::bson::{Bson, Document, doc, oid::ObjectId};
use serde_json::{Map, Value, json};

use crate::{
    auth::{
        models::{SITE_USERS, SiteUser},
        tokens::hash_token,
    },
    error::ApiError,
    mods_write_auth::{
        docs::{iso, to_oid},
        links::{COLLECTION, CreatorLink},
        project::PROJECTS,
        store::{Res, Store},
    },
    site_auth::store::user_doc,
};

pub const CREATOR_TOKEN_PREFIX: &str = "kiwi_creator_";
pub const MAX_LINKS_PER_CREATOR: u64 = 25;
pub const MAX_LINKS_PER_DEVELOPER: u64 = 50;

fn to_site_user(d: Document) -> Res<SiteUser> {
    mongodb::bson::from_document(d).map_err(|e| {
        tracing::error!("site_users row: {e}");
        ApiError::internal()
    })
}

async fn site_user(store: &Store, filter: Document) -> Res<Option<SiteUser>> {
    store.find_one(SITE_USERS, filter).await?.map(to_site_user).transpose()
}

fn obj(v: Value) -> Map<String, Value> {
    match v {
        Value::Object(m) => m,
        _ => Map::new(),
    }
}

/// `{**a, **b}`.
fn merge(a: Value, b: Value) -> Value {
    let mut out = obj(a);
    out.extend(obj(b));
    Value::Object(out)
}

/// `_mint()`: `(plaintext, sha256, display prefix)`.
fn mint(store: &Store) -> (String, String, String) {
    let raw = format!("{CREATOR_TOKEN_PREFIX}{}", store.token_urlsafe());
    let prefix: String = raw.chars().take(CREATOR_TOKEN_PREFIX.len() + 8).collect();
    let hashed = hash_token(&raw);
    (raw, hashed, prefix)
}

/// `token_dto(user)`.
pub fn token_dto(user: &SiteUser) -> Value {
    json!({
        "has_token": user.creator_token_hash.as_deref().is_some_and(|h| !h.is_empty()),
        "prefix": user.creator_token_prefix,
        "issued_at": iso(user.creator_token_at.map(crate::dt::from_bson)),
    })
}

async fn store_token(store: &Store, user: &mut SiteUser) -> Res<String> {
    let (raw, hashed, prefix) = mint(store);
    user.creator_token_hash = Some(hashed);
    user.creator_token_prefix = Some(prefix);
    user.creator_token_at = Some(crate::dt::to_bson(store.now()));
    user.updated_at = crate::dt::to_bson(store.now());
    store.save(SITE_USERS, user_doc(user)).await?;
    Ok(raw)
}

/// `ensure_token(user)`: the plaintext only when this call minted it.
pub async fn ensure_token(store: &Store, user: &mut SiteUser) -> Res<(Value, Option<String>)> {
    if user.creator_token_hash.as_deref().is_some_and(|h| !h.is_empty()) {
        return Ok((token_dto(user), None));
    }
    let raw = store_token(store, user).await?;
    Ok((token_dto(user), Some(raw)))
}

/// `rotate_token(user)`: a new token and every live connection cut.
pub async fn rotate_token(store: &Store, user: &mut SiteUser) -> Res<(Value, String)> {
    let raw = store_token(store, user).await?;
    let now = crate::mods_write_auth::docs::date(store.now());
    store
        .update_many(
            COLLECTION,
            doc! {"$and": [{"site_user_id": user.id}, {"revoked": false}]},
            doc! {"$set": {"revoked": true, "revoked_at": now, "revoked_by_rotation": true}},
        )
        .await?;
    Ok((token_dto(user), raw))
}

fn card(p: &Document) -> (String, Value) {
    let id = p.get_object_id("_id").map(|i| i.to_hex()).unwrap_or_default();
    let s = |k: &str| p.get_str(k).unwrap_or_default().to_owned();
    (id.clone(), json!({"id": id, "slug": s("slug"), "title": s("title"), "handle": s("owner_handle")}))
}

/// `_project_titles(ids)`.
async fn project_titles(store: &Store, ids: &[ObjectId]) -> Res<Map<String, Value>> {
    if ids.is_empty() {
        return Ok(Map::new());
    }
    let ids: Vec<Bson> = ids.iter().map(|i| Bson::ObjectId(*i)).collect();
    let docs = store.find(PROJECTS, doc! {"_id": {"$in": ids}}, None, 0, 0).await?;
    Ok(docs.iter().map(card).collect())
}

/// `_link_dto(link, projects=...)`.
async fn link_dto(store: &Store, link: &CreatorLink, projects: Option<&Map<String, Value>>) -> Res<Value> {
    let fetched;
    let projects = match projects {
        Some(p) => p,
        None => {
            fetched = project_titles(store, &link.project_ids).await?;
            &fetched
        }
    };
    let ids: Vec<String> = link.project_ids.iter().map(|i| i.to_hex()).collect();
    let cards: Vec<Value> = ids.iter().filter_map(|k| projects.get(k).cloned()).collect();
    Ok(json!({
        "id": link.id.to_hex(),
        "label": link.label,
        "all_projects": link.all_projects,
        "project_ids": ids,
        "projects": cards,
        "created_at": iso(Some(link.created_at)),
        "last_used_at": iso(link.last_used_at),
        "request_count": link.request_count,
    }))
}

/// `_creator_face(creator)`.
fn creator_face(creator: &SiteUser) -> Value {
    let display = creator.display_name.as_deref().filter(|d| !d.is_empty()).unwrap_or(&creator.username);
    json!({"creator": {"handle": creator.username, "display_name": display}})
}

async fn live_count(store: &Store, key: &str, id: ObjectId) -> Res<u64> {
    store.count(COLLECTION, doc! {"$and": [{key: id}, {"revoked": false}]}).await
}

/// `connect(api_user, raw_token, label)`.
pub async fn connect(store: &Store, api_user_id: ObjectId, raw_token: &str, label: &str) -> Res<Value> {
    let token = kiwi_core::pystr::strip(raw_token);
    if token.is_empty() {
        return Err(ApiError::bad_request("Paste the creator token."));
    }
    let creator = site_user(store, doc! {"creator_token_hash": hash_token(token)}).await?;
    let Some(creator) = creator.filter(|c| c.is_active && !c.is_deleted) else {
        return Err(ApiError::not_found(
            "That creator token isn't valid. Ask the creator for a current one from their Dashboard.",
        ));
    };
    let label: String = kiwi_core::pystr::strip(label).chars().take(60).collect();

    let pair = doc! {"$and": [{"site_user_id": creator.id}, {"api_user_id": api_user_id}]};
    if let Some(d) = store.find_one(COLLECTION, pair).await? {
        let mut existing = CreatorLink::from_doc(&d);
        if existing.revoked {
            existing.revoked = false;
            existing.revoked_at = None;
            existing.revoked_by_rotation = false;
        }
        if !label.is_empty() {
            existing.label = label;
        }
        existing.save(store).await?;
        return Ok(merge(link_dto(store, &existing, None).await?, creator_face(&creator)));
    }

    if live_count(store, "site_user_id", creator.id).await? >= MAX_LINKS_PER_CREATOR {
        return Err(ApiError::conflict(
            "This creator has connected the maximum number of API accounts. They can free one up from their \
             Dashboard.",
        ));
    }
    if live_count(store, "api_user_id", api_user_id).await? >= MAX_LINKS_PER_DEVELOPER {
        return Err(ApiError::conflict("You've reached the maximum number of connected creators."));
    }
    let mut link = CreatorLink::new(creator.id, api_user_id, label, store.now());
    link.id = store.insert(COLLECTION, link.fields()).await?;
    Ok(merge(link_dto(store, &link, None).await?, creator_face(&creator)))
}

async fn live_links_sorted(store: &Store, key: &str, id: ObjectId) -> Res<Vec<CreatorLink>> {
    let docs = store
        .find(COLLECTION, doc! {"$and": [{key: id}, {"revoked": false}]}, Some(doc! {"created_at": -1}), 0, 0)
        .await?;
    Ok(docs.iter().map(CreatorLink::from_doc).collect())
}

/// `list_for_developer(api_user)`.
pub async fn list_for_developer(store: &Store, api_user_id: ObjectId) -> Res<Vec<Value>> {
    let mut out = Vec::new();
    for link in live_links_sorted(store, "api_user_id", api_user_id).await? {
        let Some(creator) = site_user(store, doc! {"_id": link.site_user_id}).await? else { continue };
        out.push(merge(link_dto(store, &link, None).await?, creator_face(&creator)));
    }
    Ok(out)
}

/// `list_for_creator(creator)`.
pub async fn list_for_creator(store: &Store, creator: &SiteUser) -> Res<Vec<Value>> {
    let links = live_links_sorted(store, "site_user_id", creator.id).await?;
    let owned = store.find(PROJECTS, doc! {"owner_id": creator.id}, None, 0, 0).await?;
    let projects: Map<String, Value> = owned.iter().map(card).collect();
    let mut out = Vec::with_capacity(links.len());
    for link in &links {
        out.push(link_dto(store, link, Some(&projects)).await?);
    }
    Ok(out)
}

/// `owned_cards(creator)`.
pub async fn owned_cards(store: &Store, creator: &SiteUser) -> Res<Vec<Value>> {
    let docs = store.find(PROJECTS, doc! {"owner_id": creator.id}, Some(doc! {"title": 1}), 0, 0).await?;
    Ok(docs
        .iter()
        .map(|p| {
            let (id, c) = card(p);
            json!({"id": id, "slug": c["slug"], "title": c["title"]})
        })
        .collect())
}

fn no_such() -> ApiError {
    ApiError::not_found("No such connection.")
}

async fn link_by_id(store: &Store, link_id: &str) -> Res<Option<CreatorLink>> {
    let Some(oid) = to_oid(link_id) else { return Ok(None) };
    Ok(store.get(COLLECTION, oid).await?.as_ref().map(CreatorLink::from_doc))
}

/// `_own_link(creator, link_id)`.
async fn own_link(store: &Store, creator: &SiteUser, link_id: &str) -> Res<CreatorLink> {
    link_by_id(store, link_id).await?.filter(|l| l.site_user_id == creator.id && !l.revoked).ok_or_else(no_such)
}

/// `set_scope(creator, link_id, all_projects=, project_ids=)`.
pub async fn set_scope(
    store: &Store,
    creator: &SiteUser,
    link_id: &str,
    all_projects: bool,
    project_ids: &[String],
) -> Res<Value> {
    let mut link = own_link(store, creator, link_id).await?;
    if all_projects {
        link.all_projects = true;
        link.project_ids = Vec::new();
    } else {
        let wanted: Vec<Bson> = project_ids.iter().filter_map(|i| to_oid(i)).map(Bson::ObjectId).collect();
        let owned = store
            .find(PROJECTS, doc! {"$and": [{"owner_id": creator.id}, {"_id": {"$in": wanted}}]}, None, 0, 0)
            .await?;
        if owned.is_empty() {
            return Err(ApiError::bad_request("Pick at least one mod, or grant access to all of them."));
        }
        link.all_projects = false;
        link.project_ids = owned.iter().filter_map(|p| p.get_object_id("_id").ok()).collect();
    }
    link.save(store).await?;
    link_dto(store, &link, None).await
}

/// `revoke_by_creator(creator, link_id)`.
pub async fn revoke_by_creator(store: &Store, creator: &SiteUser, link_id: &str) -> Res<()> {
    let mut link = own_link(store, creator, link_id).await?;
    link.revoked = true;
    link.revoked_at = Some(store.now());
    link.save(store).await
}

/// `disconnect_by_developer(api_user, link_id)`.
pub async fn disconnect_by_developer(store: &Store, api_user_id: ObjectId, link_id: &str) -> Res<()> {
    let mut link =
        link_by_id(store, link_id).await?.filter(|l| l.api_user_id == api_user_id && !l.revoked).ok_or_else(no_such)?;
    link.revoked = true;
    link.revoked_at = Some(store.now());
    link.save(store).await
}

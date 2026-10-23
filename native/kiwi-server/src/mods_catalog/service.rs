//! The read half of `mods_hub/service.py` for this component: visibility,
//! catalog queries, details, profiles, the public `/v1/mods` API and the lazy
//! popularity refresh.

use std::{
    collections::{HashMap, HashSet},
    sync::Mutex,
};

use axum::http::StatusCode;
use chrono::{DateTime, Duration, Utc};
use futures::TryStreamExt;
use kiwi_core::pystr;
use mongodb::{
    Collection,
    bson::{Bson, Document, doc, oid::ObjectId},
    options::ReturnDocument,
};
use serde_json::{Map, Value, json};

use super::{
    dto::{self, Urls, effective_visibility, project_card},
    git,
    model::{self, Profile, Project, Release},
};
use crate::{
    auth::models::{SITE_USERS, SiteUser},
    dt,
    error::{ApiError, ErrorCode},
    state::AppState,
};

pub const STRAY_HANDLE: &str = "stray";

pub fn internal(e: impl std::fmt::Display) -> ApiError {
    tracing::error!("mods catalog: {e}");
    ApiError::internal()
}

pub fn coll(state: &AppState, name: &str) -> Collection<Document> {
    state.mongo().collection(name)
}

pub fn users(state: &AppState) -> Collection<SiteUser> {
    state.mongo().collection(SITE_USERS)
}

pub fn not_found(what: &str) -> ApiError {
    ApiError::not_found(what)
}

pub fn not_public() -> ApiError {
    ApiError::new(StatusCode::NOT_FOUND, ErrorCode::NotPublic, "This mod isn't public yet.")
}

pub async fn find_docs(
    c: &Collection<Document>,
    filter: Document,
    sort: Option<Document>,
    skip: Option<u64>,
    limit: Option<i64>,
) -> Result<Vec<Document>, ApiError> {
    let mut q = c.find(filter);
    if let Some(s) = sort {
        q = q.sort(s);
    }
    if let Some(s) = skip {
        q = q.skip(s);
    }
    if let Some(l) = limit {
        q = q.limit(l);
    }
    q.await.map_err(internal)?.try_collect().await.map_err(internal)
}

pub async fn projects(
    state: &AppState,
    filter: Document,
    sort: Option<Document>,
    skip: Option<u64>,
    limit: Option<i64>,
) -> Result<Vec<Project>, ApiError> {
    Ok(find_docs(&coll(state, model::PROJECTS), filter, sort, skip, limit)
        .await?
        .iter()
        .map(Project::from_doc)
        .collect())
}

pub async fn project_one(state: &AppState, filter: Document) -> Result<Option<Project>, ApiError> {
    Ok(coll(state, model::PROJECTS).find_one(filter).await.map_err(internal)?.as_ref().map(Project::from_doc))
}

pub async fn releases(state: &AppState, filter: Document, sort: Document) -> Result<Vec<Release>, ApiError> {
    Ok(find_docs(&coll(state, model::RELEASES), filter, Some(sort), None, None)
        .await?
        .iter()
        .map(Release::from_doc)
        .collect())
}

pub async fn user_by_id(state: &AppState, id: ObjectId) -> Result<Option<SiteUser>, ApiError> {
    users(state).find_one(doc! {"_id": id}).await.map_err(internal)
}

pub async fn user_by_username(state: &AppState, handle: &str) -> Result<Option<SiteUser>, ApiError> {
    let name = pystr::strip(handle).to_lowercase();
    users(state).find_one(doc! {"username": name}).await.map_err(internal)
}

pub async fn profile_of(state: &AppState, user_id: ObjectId) -> Result<Option<Profile>, ApiError> {
    Ok(coll(state, model::PROFILES)
        .find_one(doc! {"site_user_id": user_id})
        .await
        .map_err(internal)?
        .as_ref()
        .map(Profile::from_doc))
}

/// Beanie `save()`: `$set` of every model field, upserted, and the stored
/// document merged back (so datetimes come back at BSON precision).
pub async fn save_doc(state: &AppState, name: &str, id: ObjectId, doc: Document) -> Result<Document, ApiError> {
    coll(state, name)
        .find_one_and_update(doc! {"_id": id}, doc! {"$set": doc})
        .upsert(true)
        .return_document(ReturnDocument::After)
        .await
        .map_err(internal)?
        .ok_or_else(|| internal("save: no document returned"))
}

pub async fn save_project(state: &AppState, p: &mut Project) -> Result<(), ApiError> {
    let stored = save_doc(state, model::PROJECTS, p.id, p.to_doc()).await?;
    *p = Project::from_doc(&stored);
    Ok(())
}

// --- visibility --------------------------------------------------------------------

pub fn can_edit(p: &Project, actor: Option<&SiteUser>) -> bool {
    actor.is_some_and(|a| Some(a.id) == p.owner_id || p.collaborators.iter().any(|c| c.user_id == a.id))
}

pub fn is_primary_owner(p: &Project, actor: Option<&SiteUser>) -> bool {
    actor.is_some_and(|a| Some(a.id) == p.owner_id)
}

pub fn is_site_master(state: &AppState, actor: Option<&SiteUser>) -> bool {
    actor.is_some_and(|a| a.username == state.settings().site_master_handle)
}

const REACHABLE: [&str; 3] = ["public", "public_website_only", "unlisted"];

pub fn can_view(state: &AppState, p: &Project, viewer: Option<&SiteUser>) -> bool {
    if can_edit(p, viewer) {
        return true;
    }
    if p.taken_down && !is_site_master(state, viewer) {
        return false;
    }
    REACHABLE.contains(&effective_visibility(p, dt::now()))
}

pub fn source_visible(state: &AppState, p: &Project, viewer: Option<&SiteUser>) -> bool {
    if p.mode != "files" {
        return false;
    }
    if can_edit(p, viewer) {
        return true;
    }
    if p.taken_down && !is_site_master(state, viewer) {
        return false;
    }
    if p.source_visibility != "public" {
        return false;
    }
    REACHABLE.contains(&effective_visibility(p, dt::now()))
}

/// `get_project(handle, slug)`.
pub async fn get_project(state: &AppState, handle: &str, slug: &str) -> Result<Option<Project>, ApiError> {
    if pystr::strip(handle).to_lowercase() == STRAY_HANDLE {
        return project_one(state, doc! {"is_stray": true, "slug": slug}).await;
    }
    let Some(user) = user_by_username(state, handle).await? else { return Ok(None) };
    project_one(state, doc! {"owner_id": user.id, "slug": slug}).await
}

/// `get_for_view(handle, slug, viewer)`.
pub async fn get_for_view(
    state: &AppState,
    handle: &str,
    slug: &str,
    viewer: Option<&SiteUser>,
) -> Result<Project, ApiError> {
    let p = get_project(state, handle, slug).await?.ok_or_else(|| not_found("Mod project not found"))?;
    if !can_view(state, &p, viewer) {
        if p.visibility == "draft" && !p.taken_down {
            return Err(not_public());
        }
        return Err(not_found("Mod project not found"));
    }
    Ok(p)
}

// --- details -------------------------------------------------------------------------

pub async fn list_releases(
    state: &AppState,
    p: &Project,
    include_drafts: bool,
    include_hidden: bool,
) -> Result<Vec<Value>, ApiError> {
    let mut q = doc! {"project_id": p.id};
    if !include_drafts {
        q.insert("status", "published");
    }
    if !include_hidden && !p.hidden_release_branches.is_empty() {
        q.insert("branch", doc! {"$nin": p.hidden_release_branches.clone()});
    }
    Ok(releases(state, q, doc! {"created_at": -1}).await?.iter().map(|r| Value::Object(dto::release_dto(r))).collect())
}

pub async fn owner_avatar_url(state: &AppState, p: &Project) -> Result<Option<String>, ApiError> {
    let Some(owner) = p.owner_id else { return Ok(None) };
    let Some(user) = user_by_id(state, owner).await? else { return Ok(None) };
    let profile = profile_of(state, user.id).await?;
    Ok(dto::profile_avatar_url(
        &Urls::of(state.settings()),
        &user,
        profile.as_ref().and_then(|p| p.avatar_sha.as_deref()),
        true,
    ))
}

pub async fn has_starred(state: &AppState, viewer: Option<&SiteUser>, p: &Project) -> Result<bool, ApiError> {
    let Some(v) = viewer else { return Ok(false) };
    Ok(coll(state, model::STARS)
        .find_one(doc! {"project_id": p.id, "site_user_id": v.id})
        .await
        .map_err(internal)?
        .is_some())
}

/// `_branches_or_default` as branch DTOs.
pub async fn branch_dtos(state: &AppState, p: &Project) -> Result<Vec<Value>, ApiError> {
    let branches = git::list_branches(state, &p.id.to_hex()).await.map_err(internal)?;
    if branches.is_empty() {
        return Ok(vec![dto::branch_dto(&p.default_branch, None)]);
    }
    Ok(branches.iter().map(|(n, h)| dto::branch_dto(n, Some(h))).collect())
}

/// `project_detail(project, viewer)`.
pub async fn project_detail(state: &AppState, p: &Project, viewer: Option<&SiteUser>) -> Result<Value, ApiError> {
    let is_owner = can_edit(p, viewer);
    let primary = is_primary_owner(p, viewer);
    let src_visible = source_visible(state, p, viewer);
    let releases = list_releases(state, p, is_owner, is_owner).await?;
    let (mut branches, mut commit_count, mut clone_url) = (Vec::new(), 0u64, Value::Null);
    if src_visible {
        branches = branch_dtos(state, p).await?;
        commit_count = git::count_commits(state, &p.id.to_hex(), &p.default_branch).await.map_err(internal)?;
        let api = Urls::of(state.settings()).api;
        clone_url = format!("{api}/git/mods/{}/{}.git", p.owner_handle, p.slug).into();
    }
    let reason = if is_owner || is_site_master(state, viewer) {
        p.takedown_reason.clone().map_or(Value::Null, Value::String)
    } else {
        Value::Null
    };
    let avatar = owner_avatar_url(state, p).await?;
    let starred = has_starred(state, viewer, p).await?;
    let mut m = project_card(p, is_owner);
    let s = |v: &str| Value::String(v.to_owned());
    m.insert("description".into(), s(&p.description));
    m.insert("description_i18n".into(), model::map_json(&p.description_i18n));
    m.insert("readme_text".into(), s(&p.readme_text));
    m.insert("readme_i18n".into(), model::map_json(&p.readme_i18n));
    m.insert("warnings".into(), s(&p.warnings));
    m.insert("warnings_i18n".into(), model::map_json(&p.warnings_i18n));
    m.insert("default_branch".into(), s(&p.default_branch));
    m.insert("preview_shas".into(), json!(p.preview_shas));
    m.insert("discord_url".into(), json!(p.discord_url));
    m.insert("website_url".into(), json!(p.website_url));
    m.insert("donation_urls".into(), json!(p.donation_urls));
    m.insert("taken_down".into(), p.taken_down.into());
    m.insert("takedown_reason".into(), reason);
    m.insert("is_owner".into(), is_owner.into());
    m.insert("is_primary_owner".into(), primary.into());
    m.insert("owner_avatar_url".into(), json!(avatar));
    let collabs: Vec<Value> =
        p.collaborators.iter().map(|c| json!({"id": c.user_id.to_hex(), "username": c.username})).collect();
    m.insert("collaborators".into(), Value::Array(collabs));
    m.insert("starred".into(), starred.into());
    m.insert("mode".into(), s(&p.mode));
    m.insert("source_visibility".into(), s(&p.source_visibility));
    m.insert("source_visible".into(), src_visible.into());
    m.insert("hidden_release_branches".into(), json!(p.hidden_release_branches));
    m.insert("branch_order".into(), json!(p.branch_order));
    m.insert("issues_enabled".into(), p.issues_enabled.into());
    m.insert("open_issue_count".into(), p.open_issue_count.into());
    m.insert("commit_count".into(), commit_count.into());
    m.insert("clone_url".into(), clone_url);
    m.insert("branches".into(), Value::Array(branches));
    m.insert("releases".into(), Value::Array(releases));
    Ok(Value::Object(m))
}

/// `cards_with_avatars(projects)`.
pub async fn cards_with_avatars(state: &AppState, list: &[Project]) -> Result<Vec<Value>, ApiError> {
    let mut cards: Vec<Map<String, Value>> = list.iter().map(|p| project_card(p, false)).collect();
    let mut owner_ids: Vec<ObjectId> = Vec::new();
    for p in list {
        if let Some(o) = p.owner_id
            && !owner_ids.contains(&o)
        {
            owner_ids.push(o);
        }
    }
    if owner_ids.is_empty() {
        return Ok(cards.into_iter().map(Value::Object).collect());
    }
    let found: Vec<SiteUser> = users(state)
        .find(doc! {"_id": {"$in": owner_ids.clone()}})
        .await
        .map_err(internal)?
        .try_collect()
        .await
        .map_err(internal)?;
    let profiles =
        find_docs(&coll(state, model::PROFILES), doc! {"site_user_id": {"$in": owner_ids}}, None, None, None).await?;
    let mut by_user: HashMap<ObjectId, Profile> = HashMap::new();
    for d in &profiles {
        let pr = Profile::from_doc(d);
        by_user.insert(pr.site_user_id, pr);
    }
    let urls = Urls::of(state.settings());
    let mut avatar: HashMap<ObjectId, Option<String>> = HashMap::new();
    let mut names: HashMap<ObjectId, String> = HashMap::new();
    for u in &found {
        let sha = by_user.get(&u.id).and_then(|p| p.avatar_sha.as_deref());
        avatar.insert(u.id, dto::profile_avatar_url(&urls, u, sha, true));
        names.insert(u.id, u.username.clone());
    }
    for (card, p) in cards.iter_mut().zip(list) {
        let url = p.owner_id.and_then(|o| avatar.get(&o).cloned().flatten());
        card.insert("owner_avatar_url".into(), json!(url));
        if let Some(name) = p.owner_id.and_then(|o| names.get(&o)).filter(|n| !n.is_empty()) {
            card.insert("handle".into(), Value::String(name.clone()));
        }
    }
    Ok(cards.into_iter().map(Value::Object).collect())
}

// --- catalog ---------------------------------------------------------------------------

/// `re.escape` (Python 3.7+ special characters only).
pub fn py_re_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if "()[]{}?*+-|^$\\.&~# \t\n\r\x0b\x0c".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// `_search_clause(q)`.
pub fn search_clause(q: &str) -> Document {
    let rx = || doc! {"$regex": py_re_escape(pystr::strip(q)), "$options": "i"};
    doc! {"$or": [{"title": rx()}, {"summary": rx()}, {"tags": rx()}, {"owner_username": rx()}, {"author": rx()}]}
}

/// `_author_eq(author)`.
pub fn author_eq(author: &str) -> Document {
    doc! {"$regex": format!("^{}$", py_re_escape(pystr::strip(author))), "$options": "i"}
}

/// `_SORTS.get(sort, "-updated_at")`.
pub fn sort_doc(sort: &str) -> Document {
    match sort {
        "recent" => doc! {"last_release_at": -1},
        "downloads" => doc! {"download_count": -1},
        "stars" => doc! {"star_count": -1},
        "popular" => doc! {"popularity_score": -1},
        "new" => doc! {"created_at": -1},
        "title" => doc! {"title": 1},
        _ => doc! {"updated_at": -1},
    }
}

/// `_browse_match()` at `now`.
pub fn browse_match(now: DateTime<Utc>) -> Document {
    let cutoff = dt::to_bson(now - dto::remix_auto_approve());
    doc! {
        "taken_down": false,
        "visibility": {"$in": ["public", "public_website_only"]},
        "$and": [{"$or": [
            {"is_remix": {"$ne": true}},
            {"is_remix": true, "remix_status": "approved"},
            {"is_remix": true, "remix_status": "pending", "remix_requested_at": {"$lte": cutoff}},
        ]}],
    }
}

pub struct ListArgs {
    pub q: Option<String>,
    pub tag: Option<String>,
    pub author: Option<String>,
    pub sort: String,
    pub limit: i64,
    pub offset: i64,
}

/// `list_public(...)`: hub cards with avatars, and the total.
pub async fn list_public(state: &AppState, a: &ListArgs) -> Result<(Vec<Value>, u64), ApiError> {
    if a.sort == "popular" {
        ensure_popularity_fresh(state).await;
    }
    let mut query = browse_match(dt::now());
    if let Some(tag) = a.tag.as_deref().filter(|t| !t.is_empty()) {
        query.insert("tags", pystr::strip(tag).to_lowercase());
    }
    if let Some(author) = a.author.as_deref().filter(|t| !t.is_empty()) {
        query.insert("owner_username", author_eq(author));
    }
    if let Some(q) = a.q.as_deref().filter(|t| !t.is_empty())
        && let Ok(and) = query.get_array_mut("$and")
    {
        and.push(Bson::Document(search_clause(q)));
    }
    let c = coll(state, model::PROJECTS);
    let total = c.count_documents(query.clone()).await.map_err(internal)?;
    let docs = projects(state, query, Some(sort_doc(&a.sort)), Some(a.offset as u64), Some(a.limit)).await?;
    Ok((cards_with_avatars(state, &docs).await?, total))
}

/// `tag_facets()`.
pub async fn tag_facets(state: &AppState) -> Result<Value, ApiError> {
    let pipeline = vec![
        doc! {"$match": browse_match(dt::now())},
        doc! {"$unwind": "$tags"},
        doc! {"$group": {"_id": "$tags", "n": {"$sum": 1}}},
    ];
    let rows: Vec<Document> = coll(state, model::PROJECTS)
        .aggregate(pipeline)
        .await
        .map_err(internal)?
        .try_collect()
        .await
        .map_err(internal)?;
    let mut counts: Vec<(String, i64)> = Vec::new();
    for r in &rows {
        let tag = match r.get("_id") {
            Some(Bson::String(s)) => pystr::strip(s).to_owned(),
            _ => String::new(),
        };
        if tag.is_empty() {
            continue;
        }
        let n = model::Rd(r).i("n", 0);
        match counts.iter_mut().find(|(t, _)| *t == tag) {
            Some(e) => e.1 += n,
            None => counts.push((tag, n)),
        }
    }
    Ok(facets_from_counts(&counts))
}

/// The pure half of `tag_facets`: categories in bit order, then custom tags.
pub fn facets_from_counts(counts: &[(String, i64)]) -> Value {
    let mut lower_sum: HashMap<String, i64> = HashMap::new();
    for (tag, n) in counts {
        *lower_sum.entry(tag.to_lowercase()).or_default() += n;
    }
    let mut categories = Vec::new();
    let mut used: HashSet<String> = HashSet::new();
    for name in kiwi_core::mod_categories::NAMES {
        let low = name.to_lowercase();
        let n = lower_sum.get(&low).copied().unwrap_or(0);
        if n > 0 {
            categories.push(json!({"tag": name, "count": n}));
            used.insert(low);
        }
    }
    let mut custom: Vec<(&String, i64)> =
        counts.iter().filter(|(t, _)| !used.contains(&t.to_lowercase())).map(|(t, n)| (t, *n)).collect();
    custom.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase())));
    let custom: Vec<Value> = custom.into_iter().map(|(t, n)| json!({"tag": t, "count": n})).collect();
    json!({"categories": categories, "custom": custom})
}

/// `list_forks(original)`.
pub async fn list_forks(state: &AppState, original: &Project) -> Result<Vec<Value>, ApiError> {
    let docs = projects(
        state,
        doc! {"forked_from_id": original.id, "taken_down": false},
        Some(doc! {"updated_at": -1}),
        None,
        None,
    )
    .await?;
    let now = dt::now();
    Ok(docs
        .iter()
        .filter(|p| matches!(effective_visibility(p, now), "public" | "public_website_only"))
        .map(|p| Value::Object(project_card(p, false)))
        .collect())
}

// --- profiles ----------------------------------------------------------------------------

/// `profile_dto(user, profile, is_owner, mods)`.
pub fn profile_dto(u: &Urls, user: &SiteUser, profile: Option<&Profile>, is_owner: bool, mods: &[Project]) -> Value {
    let p = profile;
    let from_profile = p.filter(|p| !p.display_name.is_empty()).map(|p| pystr::strip(&p.display_name).to_owned());
    let name = from_profile
        .filter(|n| !n.is_empty())
        .or_else(|| {
            let d = pystr::strip(user.display_name.as_deref().unwrap_or("")).to_owned();
            (!d.is_empty()).then_some(d)
        })
        .unwrap_or_else(|| user.username.clone());
    let order: Vec<String> = p.map(|p| p.mod_order.clone()).unwrap_or_default();
    let by_slug: HashMap<&str, &Project> = mods.iter().map(|m| (m.slug.as_str(), m)).collect();
    let in_order: HashSet<&str> = order.iter().map(String::as_str).collect();
    let mut ordered: Vec<&Project> = order.iter().filter_map(|s| by_slug.get(s.as_str()).copied()).collect();
    ordered.extend(mods.iter().filter(|m| !in_order.contains(m.slug.as_str())));
    let featured = p
        .and_then(|p| p.featured_slug.as_deref())
        .filter(|s| !s.is_empty())
        .and_then(|s| by_slug.get(s))
        .map(|m| project_card(m, false));
    let banner_sha = p.and_then(|p| p.banner_sha.clone());
    let banner_url = dto::public_img_url(u, banner_sha.as_deref());
    let taken_down = p.is_some_and(|p| p.taken_down);
    let reason = p.filter(|p| p.taken_down && is_owner).and_then(|p| p.takedown_reason.clone());
    let mut m = Map::new();
    m.insert("handle".into(), user.username.clone().into());
    m.insert("display_name".into(), name.into());
    m.insert("tagline".into(), p.map_or(String::new(), |p| p.tagline.clone()).into());
    m.insert("tagline_i18n".into(), p.map_or(json!({}), |p| model::map_json(&p.tagline_i18n)));
    m.insert("readme".into(), p.map_or(String::new(), |p| p.readme.clone()).into());
    m.insert("readme_i18n".into(), p.map_or(json!({}), |p| model::map_json(&p.readme_i18n)));
    m.insert(
        "avatar_url".into(),
        json!(dto::profile_avatar_url(u, user, p.and_then(|p| p.avatar_sha.as_deref()), false)),
    );
    m.insert("avatar_sha".into(), json!(p.and_then(|p| p.avatar_sha.clone())));
    m.insert("banner_url".into(), json!(banner_url));
    m.insert("banner_sha".into(), json!(banner_sha));
    m.insert("discord_url".into(), json!(p.and_then(|p| p.discord_url.clone())));
    m.insert("website_url".into(), json!(p.and_then(|p| p.website_url.clone())));
    m.insert("donation_urls".into(), json!(p.map(|p| p.donation_urls.clone()).unwrap_or_default()));
    m.insert("is_owner".into(), is_owner.into());
    m.insert("taken_down".into(), taken_down.into());
    m.insert("takedown_reason".into(), json!(reason));
    m.insert("joined_at".into(), dto::iso(&dt::from_bson(user.created_at)));
    m.insert("page_url".into(), format!("{}/mods/{}", u.app, user.username).into());
    m.insert("mod_count".into(), ordered.len().into());
    m.insert("mod_order".into(), json!(ordered.iter().map(|m| m.slug.clone()).collect::<Vec<_>>()));
    m.insert("featured_slug".into(), featured.as_ref().map_or(Value::Null, |f| f["slug"].clone()));
    m.insert("featured".into(), featured.map_or(Value::Null, Value::Object));
    m.insert("mods".into(), Value::Array(ordered.iter().map(|m| Value::Object(project_card(m, false))).collect()));
    Value::Object(m)
}

/// `profile_view(handle, viewer)`.
pub async fn profile_view(
    state: &AppState,
    handle: &str,
    viewer: Option<&SiteUser>,
) -> Result<Option<Value>, ApiError> {
    let Some(user) = user_by_username(state, handle).await? else { return Ok(None) };
    if !user.is_active {
        return Ok(None);
    }
    let has_public =
        project_one(state, doc! {"owner_id": user.id, "visibility": "public", "taken_down": false}).await?.is_some();
    if !has_public {
        return Ok(None);
    }
    let profile = profile_of(state, user.id).await?;
    let is_owner = viewer.is_some_and(|v| v.id == user.id);
    if profile.as_ref().is_some_and(|p| p.taken_down) && !is_owner {
        return Ok(None);
    }
    let mut q = doc! {"owner_id": user.id};
    if !is_owner {
        q.insert("visibility", "public");
        q.insert("taken_down", false);
    }
    let mods = projects(state, q, Some(doc! {"updated_at": -1}), None, None).await?;
    Ok(Some(profile_dto(&Urls::of(state.settings()), &user, profile.as_ref(), is_owner, &mods)))
}

/// `public_modder_handles()`.
pub async fn public_modder_handles(state: &AppState) -> Result<Vec<Value>, ApiError> {
    let pipeline = vec![
        doc! {"$match": {"visibility": "public", "taken_down": false, "owner_id": {"$ne": Bson::Null}}},
        doc! {"$group": {"_id": "$owner_id", "at": {"$max": "$updated_at"}}},
    ];
    let rows: Vec<Document> = coll(state, model::PROJECTS)
        .aggregate(pipeline)
        .await
        .map_err(internal)?
        .try_collect()
        .await
        .map_err(internal)?;
    let mut latest: HashMap<ObjectId, Option<DateTime<Utc>>> = HashMap::new();
    let mut ids: Vec<ObjectId> = Vec::new();
    for r in &rows {
        let Some(id) = r.get_object_id("_id").ok() else { continue };
        if !latest.contains_key(&id) {
            ids.push(id);
        }
        latest.insert(id, model::Rd(r).odt("at"));
    }
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let found: Vec<SiteUser> = users(state)
        .find(doc! {"$and": [{"_id": {"$in": ids.clone()}}, {"is_active": true}]})
        .await
        .map_err(internal)?
        .try_collect()
        .await
        .map_err(internal)?;
    let hidden = find_docs(
        &coll(state, model::PROFILES),
        doc! {"$and": [{"site_user_id": {"$in": ids}}, {"taken_down": true}]},
        None,
        None,
        None,
    )
    .await?;
    let hidden: HashSet<ObjectId> = hidden.iter().filter_map(|d| model::Rd(d).oid("site_user_id")).collect();
    Ok(found
        .iter()
        .filter(|u| !hidden.contains(&u.id))
        .map(|u| json!({"handle": u.username, "updated_at": dto::opt_iso(latest.get(&u.id).unwrap_or(&None))}))
        .collect())
}

// --- the creator's own lists ---------------------------------------------------------

/// `list_owned(actor)`: resyncs drifted handles as a side effect.
pub async fn list_owned(state: &AppState, actor: &SiteUser) -> Result<Vec<Value>, ApiError> {
    let mut docs = projects(
        state,
        doc! {"$or": [{"owner_id": actor.id}, {"collaborators.user_id": actor.id}]},
        Some(doc! {"updated_at": -1}),
        None,
        None,
    )
    .await?;
    for p in docs.iter_mut() {
        if p.owner_id == Some(actor.id) && p.owner_handle != actor.username {
            p.owner_handle = actor.username.clone();
            save_project(state, p).await?;
        }
    }
    Ok(docs
        .iter()
        .map(|p| {
            let mut card = project_card(p, false);
            card.insert("is_collaborator".into(), (p.owner_id != Some(actor.id)).into());
            Value::Object(card)
        })
        .collect())
}

/// `list_starred(actor)`.
pub async fn list_starred(state: &AppState, actor: &SiteUser) -> Result<Vec<Value>, ApiError> {
    let stars = find_docs(
        &coll(state, model::STARS),
        doc! {"site_user_id": actor.id},
        Some(doc! {"created_at": -1}),
        None,
        None,
    )
    .await?;
    if stars.is_empty() {
        return Ok(Vec::new());
    }
    let ids: Vec<ObjectId> = stars.iter().filter_map(|s| model::Rd(s).oid("project_id")).collect();
    let by_id: HashMap<ObjectId, Project> = projects(state, doc! {"_id": {"$in": ids.clone()}}, None, None, None)
        .await?
        .into_iter()
        .map(|p| (p.id, p))
        .collect();
    Ok(ids
        .iter()
        .filter_map(|id| by_id.get(id))
        .filter(|p| can_view(state, p, Some(actor)))
        .map(|p| Value::Object(project_card(p, false)))
        .collect())
}

// --- public catalog API (/v1/mods) ---------------------------------------------------

fn public_query() -> Document {
    doc! {"visibility": "public", "taken_down": false, "is_remix": {"$ne": true}}
}

/// `public_list(...)`.
pub async fn public_list(state: &AppState, a: &ListArgs) -> Result<(Vec<Value>, u64), ApiError> {
    if a.sort == "popular" {
        ensure_popularity_fresh(state).await;
    }
    let mut query = public_query();
    if let Some(tag) = a.tag.as_deref().filter(|t| !t.is_empty()) {
        query.insert("tags", pystr::strip(tag).to_lowercase());
    }
    if let Some(author) = a.author.as_deref().filter(|t| !t.is_empty()) {
        query.insert("owner_username", author_eq(author));
    }
    if let Some(q) = a.q.as_deref().filter(|t| !t.is_empty()) {
        query.extend(search_clause(q));
    }
    let total = coll(state, model::PROJECTS).count_documents(query.clone()).await.map_err(internal)?;
    let docs = projects(state, query, Some(sort_doc(&a.sort)), Some(a.offset as u64), Some(a.limit)).await?;
    let u = Urls::of(state.settings());
    Ok((docs.iter().map(|p| Value::Object(dto::public_mod_dto(&u, p, None))).collect(), total))
}

/// `_published_releases(project)`.
pub async fn published_releases(state: &AppState, p: &Project) -> Result<Vec<Release>, ApiError> {
    let mut q = doc! {"project_id": p.id, "status": "published"};
    if !p.hidden_release_branches.is_empty() {
        q.insert("branch", doc! {"$nin": p.hidden_release_branches.clone()});
    }
    releases(state, q, doc! {"published_at": -1}).await
}

/// `public_detail(handle, slug)`.
pub async fn public_detail(state: &AppState, handle: &str, slug: &str) -> Result<Value, ApiError> {
    let p = get_for_view(state, handle, slug, None).await?;
    let rs = published_releases(state, &p).await?;
    Ok(Value::Object(dto::public_mod_dto(&Urls::of(state.settings()), &p, Some(&rs))))
}

/// `public_popular(limit)`.
pub async fn public_popular(state: &AppState, limit: i64) -> Result<Vec<Value>, ApiError> {
    ensure_popularity_fresh(state).await;
    let limit = limit.clamp(1, 25);
    let docs = projects(state, public_query(), Some(doc! {"popularity_score": -1}), None, Some(limit)).await?;
    let u = Urls::of(state.settings());
    Ok(docs.iter().map(|p| Value::Object(dto::public_mod_dto(&u, p, None))).collect())
}

/// `_published_releases_for(projects, latest_per_branch=)`.
async fn published_releases_for(
    state: &AppState,
    list: &[&Project],
    latest_per_branch: bool,
) -> Result<HashMap<ObjectId, Vec<Release>>, ApiError> {
    if list.is_empty() {
        return Ok(HashMap::new());
    }
    let ids: Vec<ObjectId> = list.iter().map(|p| p.id).collect();
    let docs =
        releases(state, doc! {"project_id": {"$in": ids}, "status": "published"}, doc! {"published_at": -1}).await?;
    let hidden: HashMap<ObjectId, HashSet<&str>> =
        list.iter().map(|p| (p.id, p.hidden_release_branches.iter().map(String::as_str).collect())).collect();
    let mut out: HashMap<ObjectId, Vec<Release>> = list.iter().map(|p| (p.id, Vec::new())).collect();
    let mut newest: HashMap<ObjectId, HashSet<String>> = HashMap::new();
    for r in docs {
        if hidden.get(&r.project_id).is_some_and(|h| h.contains(r.branch.as_str())) || !out.contains_key(&r.project_id)
        {
            continue;
        }
        if latest_per_branch && !newest.entry(r.project_id).or_default().insert(r.branch.clone()) {
            continue;
        }
        if let Some(v) = out.get_mut(&r.project_id) {
            v.push(r);
        }
    }
    Ok(out)
}

/// `lookup_by_hashes(hashes, releases_mode=)`.
pub async fn lookup_by_hashes(state: &AppState, hashes: &[String], mode: &str) -> Result<Value, ApiError> {
    let mut uniq: Vec<String> = Vec::new();
    for h in hashes {
        let h = pystr::strip(h).to_lowercase();
        if !h.is_empty() && !uniq.contains(&h) {
            uniq.push(h);
        }
    }
    let mut results: Map<String, Value> = Map::new();
    let mut taken_down: HashSet<String> = HashSet::new();
    if !uniq.is_empty() {
        let wanted: HashSet<&str> = uniq.iter().map(String::as_str).collect();
        let rs = releases(
            state,
            doc! {"$and": [
                {"$or": [{"tmod_sha": {"$in": uniq.clone()}}, {"prior_tmod_shas": {"$in": uniq.clone()}}]},
                {"status": "published"},
            ]},
            doc! {"published_at": -1},
        )
        .await?;
        let mut proj_ids: Vec<ObjectId> = Vec::new();
        for r in &rs {
            if !proj_ids.contains(&r.project_id) {
                proj_ids.push(r.project_id);
            }
        }
        let mut projs: HashMap<ObjectId, Project> = HashMap::new();
        if !proj_ids.is_empty() {
            projs = projects(state, doc! {"_id": {"$in": proj_ids}}, None, None, None)
                .await?
                .into_iter()
                .map(|p| (p.id, p))
                .collect();
        }
        let now = dt::now();
        let shown = |p: &Project| !p.taken_down && matches!(effective_visibility(p, now), "public" | "unlisted");
        let want_releases = matches!(mode, "latest" | "all");
        let by_project = if want_releases {
            let visible: Vec<&Project> = projs.values().filter(|p| shown(p)).collect();
            published_releases_for(state, &visible, mode == "latest").await?
        } else {
            HashMap::new()
        };
        let u = Urls::of(state.settings());
        for r in &rs {
            let all: Vec<&str> =
                std::iter::once(r.tmod_sha.as_str()).chain(r.prior_tmod_shas.iter().map(String::as_str)).collect();
            let keys: Vec<&str> =
                all.iter().copied().filter(|h| wanted.contains(h) && !results.contains_key(*h)).collect();
            if keys.is_empty() {
                continue;
            }
            let Some(proj) = projs.get(&r.project_id).filter(|p| shown(p)) else { continue };
            let rel = if want_releases { by_project.get(&proj.id).map(Vec::as_slice) } else { None };
            let hit = json!({
                "mod": Value::Object(dto::public_mod_dto(&u, proj, rel)),
                "release": Value::Object(dto::public_release_dto(&u, r)),
            });
            for key in keys {
                results.insert(key.to_owned(), hit.clone());
            }
        }
        for r in &rs {
            if projs.get(&r.project_id).is_some_and(|p| p.taken_down) {
                for h in std::iter::once(&r.tmod_sha).chain(r.prior_tmod_shas.iter()) {
                    if wanted.contains(h.as_str()) {
                        taken_down.insert(h.clone());
                    }
                }
            }
        }
        let wanted_list: Vec<String> = wanted.iter().map(|s| (*s).to_owned()).collect();
        let arts =
            find_docs(&coll(state, model::TAKEN_DOWN), doc! {"sha256": {"$in": wanted_list}}, None, None, None).await?;
        for a in &arts {
            if let Some(s) = model::Rd(a).os("sha256") {
                taken_down.insert(s);
            }
        }
        taken_down.retain(|h| !results.contains_key(h));
    }
    let unknown: Vec<&String> = uniq.iter().filter(|h| !results.contains_key(*h)).collect();
    let gone: Vec<&String> = uniq.iter().filter(|h| taken_down.contains(*h)).collect();
    Ok(json!({"results": Value::Object(results), "unknown": unknown, "taken_down": gone}))
}

// --- popularity ----------------------------------------------------------------------

static POP_AT: Mutex<Option<DateTime<Utc>>> = Mutex::new(None);

/// `ensure_popularity_fresh()`: at most once per 10 minutes in this process.
pub async fn ensure_popularity_fresh(state: &AppState) {
    let now = dt::now();
    {
        let mut at = POP_AT.lock().unwrap_or_else(|e| e.into_inner());
        if at.is_some_and(|last| now - last < Duration::minutes(10)) {
            return;
        }
        *at = Some(now);
    }
    if let Err(e) = refresh_popularity(state).await {
        tracing::warn!("mods_hub: popularity refresh failed: {e}");
    }
}

/// Python `round(x, 4)`.
pub fn round4(x: f64) -> f64 {
    format!("{x:.4}").parse().unwrap_or(x)
}

/// `4·log1p(7-day) + 2·log1p(stars) + log1p(downloads)` per project; `None`
/// where Python's `math.log1p` raises (a count of -1 or less).
pub fn raw_scores(rows: &[(i64, i64, i64)]) -> Option<Vec<f64>> {
    let lp = |n: i64| if n <= -1 { None } else { Some((n as f64).ln_1p()) };
    rows.iter().map(|&(d7, stars, dl)| Some(4.0 * lp(d7)? + 2.0 * lp(stars)? + lp(dl)?)).collect()
}

/// `refresh_popularity()`.
pub async fn refresh_popularity(state: &AppState) -> anyhow::Result<()> {
    let since = dt::to_bson(dt::now() - Duration::days(7));
    let rows: Vec<Document> = coll(state, model::DOWNLOAD_EVENTS)
        .aggregate(vec![
            doc! {"$match": {"created_at": {"$gte": since}}},
            doc! {"$group": {"_id": "$project_id", "n": {"$sum": 1}}},
        ])
        .await?
        .try_collect()
        .await?;
    let mut recent: HashMap<ObjectId, i64> = HashMap::new();
    for r in &rows {
        if let Ok(id) = r.get_object_id("_id") {
            recent.insert(id, model::Rd(r).i("n", 0));
        }
    }
    let list = projects(state, doc! {"visibility": "public", "taken_down": false}, None, None, None)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let inputs: Vec<(i64, i64, i64)> =
        list.iter().map(|p| (recent.get(&p.id).copied().unwrap_or(0), p.star_count, p.download_count)).collect();
    let raw = raw_scores(&inputs).ok_or_else(|| anyhow::anyhow!("math domain error"))?;
    let top = raw.iter().copied().fold(None, |m: Option<f64>, x| Some(m.map_or(x, |m| m.max(x)))).unwrap_or(0.0);
    let top = if top == 0.0 { 1.0 } else { top };
    let c = coll(state, model::PROJECTS);
    for (p, raw) in list.iter().zip(raw) {
        let d7 = recent.get(&p.id).copied().unwrap_or(0);
        let score = round4(raw / top);
        if p.downloads_7d != d7 || p.popularity_score != score {
            c.update_one(
                doc! {"_id": p.id},
                doc! {"$set": {"downloads_7d": crate::auth::pyutil::bson_int(d7), "popularity_score": score}},
            )
            .await?;
        }
    }
    Ok(())
}

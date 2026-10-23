//! `app/trove/modpacks/service.py` over Mongo: lookups and visibility, the
//! JSON-ready views, the owner writes, master moderation and the on-the-fly
//! artifact. In-memory values follow Beanie: `insert()` keeps microseconds,
//! `save()` reads the stored document back. Each write is a pure edit of the
//! [`Pack`] plus the store call.

use std::collections::{BTreeSet, HashMap};

use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use futures::TryStreamExt;
use mongodb::{
    Collection,
    bson::{Bson, Document, doc, oid::ObjectId},
    options::ReturnDocument,
};
use serde_json::{Map, Value, json};

use super::{
    artifact::{self, Meta, Record},
    body::{AdminListQuery, CreateBody, EntryInput, ListQuery, UpdateBody},
    hub::{self, ModRef, Release, and, internal},
    model::{self, Collaborator, DEFAULT_VARIANT, Entry, PACKS, Pack, STARS, Variant},
    text,
};
use crate::{auth::models::SiteUser, dt, error::ApiError, state::AppState};

pub type Res<T> = Result<T, ApiError>;

const MAX_UPLOAD_BYTES: usize = 80 * 1024 * 1024;
const APPEAL_CONTACT: &str = "support@aallyn.net";

fn coll(state: &AppState) -> Collection<Document> {
    state.mongo().collection(PACKS)
}

fn not_found(what: &str) -> ApiError {
    ApiError::not_found(what)
}

fn strip(s: &str) -> String {
    kiwi_core::pystr::strip(s).to_owned()
}

async fn find_pack(state: &AppState, filter: Document) -> Res<Option<Pack>> {
    match coll(state).find_one(filter).await.map_err(internal)? {
        Some(d) => Ok(Some(Pack::from_doc(&d)?)),
        None => Ok(None),
    }
}

async fn find_packs(state: &AppState, filter: Document, sort: Document, skip: u64, limit: i64) -> Res<Vec<Pack>> {
    let c = coll(state);
    let mut find = c.find(filter).sort(sort);
    if skip > 0 {
        find = find.skip(skip);
    }
    if limit > 0 {
        find = find.limit(limit);
    }
    let docs: Vec<Document> = find.await.map_err(internal)?.try_collect().await.map_err(internal)?;
    docs.iter().map(|d| Pack::from_doc(d).map_err(ApiError::from)).collect()
}

/// Beanie's `save()`: `$set` of every field, upserting, then the stored
/// document merged back.
async fn save(state: &AppState, pack: &mut Pack) -> Res<()> {
    let stored = coll(state)
        .find_one_and_update(doc! {"_id": pack.id}, doc! {"$set": pack.to_doc()})
        .upsert(true)
        .return_document(ReturnDocument::After)
        .await
        .map_err(internal)?
        .ok_or_else(ApiError::internal)?;
    *pack = Pack::from_doc(&stored)?;
    Ok(())
}

// --- lookup ----------------------------------------------------------------

pub async fn get_pack(state: &AppState, handle: &str, slug: &str) -> Res<Option<Pack>> {
    let Some(user) = hub::user_by_handle(state, handle).await? else { return Ok(None) };
    let uid = model::oid(&user, "_id")?;
    find_pack(state, and(vec![doc! {"owner_id": uid}, doc! {"slug": slug}])).await
}

pub async fn get_for_view(state: &AppState, handle: &str, slug: &str, viewer: Option<ObjectId>) -> Res<Pack> {
    match get_pack(state, handle, slug).await? {
        Some(p) if p.can_view(viewer) => Ok(p),
        _ => Err(not_found("Modpack not found")),
    }
}

/// The router's `_require_owned`: a pack the caller can edit, or 404.
pub async fn require_owned(state: &AppState, handle: &str, slug: &str, actor: &SiteUser) -> Res<Pack> {
    match get_pack(state, handle, slug).await? {
        Some(p) if p.can_edit(Some(actor.id)) => Ok(p),
        _ => Err(not_found("Modpack not found")),
    }
}

async fn pack_by_id(state: &AppState, pack_id: &str) -> Res<Pack> {
    let Ok(oid) = ObjectId::parse_str(pack_id) else { return Err(not_found("Modpack not found")) };
    find_pack(state, doc! {"_id": oid}).await?.ok_or_else(|| not_found("Modpack not found"))
}

pub fn require_primary_owner(pack: &Pack, actor: ObjectId) -> Res<()> {
    if pack.is_primary_owner(Some(actor)) {
        Ok(())
    } else {
        Err(ApiError::forbidden("Only the modpack's owner can do this."))
    }
}

fn variant_or_404(pack: &Pack, name: &str) -> Res<usize> {
    pack.variant_index(name).ok_or_else(|| not_found("Variant not found"))
}

// --- entry resolution ------------------------------------------------------

/// `_resolve_entry`'s view of one entry.
#[derive(Clone, Debug, PartialEq)]
pub struct EntryView {
    pub custom: bool,
    pub custom_sha: Option<String>,
    pub custom_filename: Option<String>,
    pub handle: String,
    pub slug: String,
    pub title: String,
    pub author: String,
    pub branch: String,
    pub version_locked: bool,
    pub locked_tag: Option<String>,
    pub version: Option<String>,
    pub is_beta: bool,
    pub available: bool,
    pub reason: Option<&'static str>,
}

impl EntryView {
    pub fn to_json(&self) -> Value {
        let mut m = Map::new();
        m.insert("custom".into(), self.custom.into());
        if self.custom {
            m.insert("custom_sha".into(), self.custom_sha.clone().into());
            m.insert("custom_filename".into(), self.custom_filename.clone().into());
        }
        for (k, v) in [
            ("handle", Value::from(self.handle.clone())),
            ("slug", self.slug.clone().into()),
            ("title", self.title.clone().into()),
            ("author", self.author.clone().into()),
            ("branch", self.branch.clone().into()),
            ("version_locked", self.version_locked.into()),
            ("locked_tag", self.locked_tag.clone().into()),
            ("version", self.version.clone().into()),
            ("is_beta", self.is_beta.into()),
            ("available", self.available.into()),
            ("reason", self.reason.into()),
        ] {
            m.insert(k.into(), v);
        }
        Value::Object(m)
    }
}

/// A custom entry's view (`present`: its blob is in the store).
pub fn custom_view(e: &Entry, sha: &str, present: bool) -> EntryView {
    EntryView {
        custom: true,
        custom_sha: Some(sha.to_owned()),
        custom_filename: e.custom_filename.clone(),
        handle: String::new(),
        slug: String::new(),
        title: if e.title.is_empty() { "Uploaded mod".into() } else { e.title.clone() },
        author: e.author.clone(),
        branch: String::new(),
        version_locked: false,
        locked_tag: None,
        version: None,
        is_beta: false,
        available: present,
        reason: (!present).then_some("missing"),
    }
}

/// The tag a locked entry resolves; `None` tracks the latest `.tmod`.
pub fn locked_tag(e: &Entry) -> Option<&str> {
    e.locked_tag.as_deref().filter(|t| e.version_locked && !t.is_empty())
}

/// A hub entry's view from its mod (`None`: gone) and the release the lookup
/// found (only looked up for an available mod).
pub fn hub_view(e: &Entry, m: Option<&ModRef>, rel: Option<Release>) -> (EntryView, Option<Release>) {
    let mut view = EntryView {
        custom: false,
        custom_sha: None,
        custom_filename: None,
        handle: e.handle.clone(),
        slug: e.slug.clone(),
        title: e.title.clone(),
        author: String::new(),
        branch: e.branch.clone(),
        version_locked: e.version_locked,
        locked_tag: e.locked_tag.clone(),
        version: None,
        is_beta: false,
        available: false,
        reason: None,
    };
    let Some(m) = m else {
        view.reason = Some("removed");
        return (view, None);
    };
    view.handle = m.owner_handle.clone();
    view.slug = m.slug.clone();
    view.title = m.title.clone();
    view.author = m.owner_username.clone();
    view.is_beta = m.is_beta;
    if m.unavailable() {
        view.reason = Some("unavailable");
        return (view, None);
    }
    let Some(rel) = rel else {
        view.reason = Some("no build");
        return (view, None);
    };
    view.version = Some(rel.tag.clone());
    if rel.release_format != "tmod" {
        view.reason = Some("not a .tmod");
        return (view, None);
    }
    view.available = true;
    (view, Some(rel))
}

/// `_resolve_entry`: the view, plus the release a hub entry points at.
pub async fn resolve_entry(state: &AppState, e: &Entry) -> Res<(EntryView, Option<Release>)> {
    if let Some(sha) = e.custom() {
        return Ok((custom_view(e, sha, hub::has_blob(state, sha).await?), None));
    }
    let m = match e.project_id {
        Some(id) => hub::mod_by_id(state, id).await?,
        None => None,
    };
    let rel = match &m {
        Some(m) if !m.unavailable() => match locked_tag(e) {
            Some(tag) => hub::release_locked(state, m.id, &e.branch, tag).await?,
            None => hub::release_latest(state, m.id, &e.branch).await?,
        },
        _ => None,
    };
    Ok(hub_view(e, m.as_ref(), rel))
}

async fn resolve_all(state: &AppState, v: &Variant) -> Res<Vec<(EntryView, Option<Release>)>> {
    let mut out = Vec::with_capacity(v.entries.len());
    for e in &v.entries {
        out.push(resolve_entry(state, e).await?);
    }
    Ok(out)
}

async fn views_of(state: &AppState, v: &Variant) -> Res<Vec<EntryView>> {
    Ok(resolve_all(state, v).await?.into_iter().map(|(view, _)| view).collect())
}

// --- views -----------------------------------------------------------------

/// `{**a, **b}`: a repeated key keeps its first position, takes the new value.
pub fn merge(mut a: Map<String, Value>, b: impl IntoIterator<Item = (String, Value)>) -> Map<String, Value> {
    for (k, v) in b {
        a.insert(k, v);
    }
    a
}

fn object(v: Value) -> Map<String, Value> {
    match v {
        Value::Object(m) => m,
        _ => Map::new(),
    }
}

/// `_variant_view` from its entries' views (sorted by title casefold).
pub fn variant_json(v: &Variant, mut views: Vec<EntryView>) -> Value {
    views.sort_by_cached_key(|v| text::casefold(&v.title));
    let available = views.iter().filter(|e| e.available).count();
    json!({
        "name": v.name,
        "label": v.display(),
        "entries": views.iter().map(EntryView::to_json).collect::<Vec<_>>(),
        "mod_count": views.len(),
        "available_count": available,
    })
}

pub fn pack_card(p: &Pack) -> Map<String, Value> {
    let total: usize = p.variants.iter().map(|v| v.entries.len()).sum();
    object(json!({
        "slug": p.slug,
        "handle": p.owner_handle,
        "title": p.title,
        "summary": p.summary,
        "tags": p.tags,
        "owner_username": p.owner_username,
        "visibility": p.visibility,
        "taken_down": p.taken_down,
        "banner_sha": p.banner_sha,
        "preview_sha": p.preview_shas.first(),
        "download_count": p.download_count,
        "star_count": p.star_count,
        "variant_count": p.variants.len(),
        "mod_count": p.card_variant().map_or(0, |v| v.entries.len()),
        "total_entries": total,
        "updated_at": dt::isoformat(&p.updated_at),
        "created_at": dt::isoformat(&p.created_at),
    }))
}

/// `pack_detail` from the variant views and the viewer's star.
pub fn detail_json(pack: &Pack, variants: Vec<Value>, viewer: Option<ObjectId>, starred: bool) -> Map<String, Value> {
    let is_owner = pack.can_edit(viewer);
    let collaborators: Vec<Value> =
        pack.collaborators.iter().map(|c| json!({"id": c.user_id.to_hex(), "username": c.username})).collect();
    let rest = json!({
        "description": pack.description,
        "warnings": pack.warnings,
        "preview_shas": pack.preview_shas,
        "discord_url": pack.discord_url,
        "website_url": pack.website_url,
        "donation_urls": pack.donation_urls,
        "default_variant": pack.default_variant,
        "variants": variants,
        "taken_down": pack.taken_down,
        "takedown_reason": if is_owner { pack.takedown_reason.clone() } else { None },
        "is_owner": is_owner,
        "is_primary_owner": pack.is_primary_owner(viewer),
        "collaborators": collaborators,
        "starred": starred,
    });
    merge(pack_card(pack), object(rest))
}

pub async fn has_starred(state: &AppState, viewer: Option<ObjectId>, pack: &Pack) -> Res<bool> {
    let Some(uid) = viewer else { return Ok(false) };
    let found = state
        .mongo()
        .collection::<Document>(STARS)
        .find_one(and(vec![doc! {"modpack_id": pack.id}, doc! {"site_user_id": uid}]))
        .await
        .map_err(internal)?;
    Ok(found.is_some())
}

pub async fn pack_detail(state: &AppState, pack: &Pack, viewer: Option<ObjectId>) -> Res<Map<String, Value>> {
    let mut variants = Vec::with_capacity(pack.variants.len());
    for v in &pack.variants {
        variants.push(variant_json(v, views_of(state, v).await?));
    }
    let starred = has_starred(state, viewer, pack).await?;
    Ok(detail_json(pack, variants, viewer, starred))
}

/// The absolute bases of the catalog's URLs (`api_url` / `app_url`, `rstrip("/")`).
#[derive(Clone, Copy)]
pub struct Bases<'a> {
    pub api: &'a str,
    pub app: &'a str,
}

impl<'a> Bases<'a> {
    pub fn of(state: &'a AppState) -> Self {
        Bases { api: text::rstrip_slash(&state.settings().api_url), app: text::rstrip_slash(&state.settings().app_url) }
    }

    fn img(&self, sha: Option<&str>) -> Option<String> {
        sha.filter(|s| !s.is_empty()).map(|s| format!("{}/v1/mods/hub/image/{s}", self.api))
    }
}

pub fn public_pack_card(b: Bases<'_>, p: &Pack) -> Map<String, Value> {
    let previews: Vec<String> = p.preview_shas.iter().filter_map(|s| b.img(Some(s))).collect();
    object(json!({
        "slug": p.slug,
        "handle": p.owner_handle,
        "title": p.title,
        "summary": p.summary,
        "tags": p.tags,
        "author": p.owner_username,
        "banner_url": b.img(p.banner_sha.as_deref()),
        "preview_urls": previews,
        "download_count": p.download_count,
        "star_count": p.star_count,
        "variant_count": p.variants.len(),
        "default_variant": p.default_variant,
        "mod_count": p.card_variant().map_or(0, |v| v.entries.len()),
        "page_url": format!("{}/modpacks/{}/{}", b.app, p.owner_handle, p.slug),
        "created_at": dt::isoformat(&p.created_at),
        "updated_at": dt::isoformat(&p.updated_at),
    }))
}

/// `_public_variant` from its entries' views (stored order).
pub fn public_variant_json(b: Bases<'_>, p: &Pack, v: &Variant, views: &[EntryView]) -> Value {
    let mods: Vec<Value> = views
        .iter()
        .map(|view| {
            json!({
                "handle": view.handle, "slug": view.slug, "title": view.title, "author": view.author,
                "variant": view.branch, "version": view.version, "version_locked": view.version_locked,
                "available": view.available, "is_beta": view.is_beta,
                "page_url": format!("{}/mods/{}/{}", b.app, view.handle, view.slug),
                "author_url": format!("{}/mods/{}", b.app, view.handle),
            })
        })
        .collect();
    let available = views.iter().filter(|v| v.available).count();
    let base = format!("{}/v1/modpacks/{}/{}/download?variant={}", b.api, p.owner_handle, p.slug, text::quote(&v.name));
    json!({
        "name": v.name,
        "label": v.display(),
        "mod_count": mods.len(),
        "available_count": available,
        "download_url": format!("{base}&format=tpack"),
        "zip_url": format!("{base}&format=zip"),
        "configs_url": format!("{base}&format=configs"),
        "mods": mods,
    })
}

/// `public_pack_dto` from the variant JSON.
pub fn public_detail_json(b: Bases<'_>, p: &Pack, variants: Vec<Value>) -> Map<String, Value> {
    let rest = json!({
        "description": p.description,
        "warnings": p.warnings,
        "discord_url": p.discord_url,
        "website_url": p.website_url,
        "donation_urls": p.donation_urls,
        "variants": variants,
    });
    merge(public_pack_card(b, p), object(rest))
}

pub async fn public_detail(state: &AppState, handle: &str, slug: &str) -> Res<Map<String, Value>> {
    let p = get_for_view(state, handle, slug, None).await?;
    let b = Bases::of(state);
    let mut variants = Vec::with_capacity(p.variants.len());
    for v in &p.variants {
        variants.push(public_variant_json(b, &p, v, &views_of(state, v).await?));
    }
    Ok(public_detail_json(b, &p, variants))
}

// --- listings --------------------------------------------------------------

fn search_clause(q: &str) -> Document {
    let rx = doc! {"$regex": text::re_escape(kiwi_core::pystr::strip(q)), "$options": "i"};
    doc! {"$or": [{"title": rx.clone()}, {"summary": rx.clone()}, {"tags": rx.clone()}, {"owner_username": rx}]}
}

fn sort_spec(sort: &str) -> Document {
    match sort {
        "downloads" => doc! {"download_count": -1},
        "stars" => doc! {"star_count": -1},
        "new" => doc! {"created_at": -1},
        "title" => doc! {"title": 1},
        _ => doc! {"updated_at": -1},
    }
}

fn nonempty(s: &Option<String>) -> Option<&str> {
    s.as_deref().filter(|s| !s.is_empty())
}

/// The `list_public` / `public_list` filter and sort.
pub fn browse_query(q: &ListQuery) -> (Document, Document) {
    let mut filter = doc! {"visibility": "public", "taken_down": false};
    if let Some(tag) = nonempty(&q.tag) {
        filter.insert("tags", text::lower(kiwi_core::pystr::strip(tag)));
    }
    if let Some(author) = nonempty(&q.author) {
        let rx = format!("^{}$", text::re_escape(kiwi_core::pystr::strip(author)));
        filter.insert("owner_username", doc! {"$regex": rx, "$options": "i"});
    }
    if let Some(term) = nonempty(&q.q) {
        filter.extend(search_clause(term));
    }
    (filter, sort_spec(&q.sort))
}

/// `paginate`: the total, then one sorted page.
async fn paginate(
    state: &AppState,
    filter: Document,
    sort: Document,
    limit: i64,
    offset: i64,
) -> Res<(Vec<Pack>, u64)> {
    let total = coll(state).count_documents(filter.clone()).await.map_err(internal)?;
    let docs = find_packs(state, filter, sort, offset as u64, limit).await?;
    Ok((docs, total))
}

pub async fn browse(state: &AppState, q: &ListQuery) -> Res<(Vec<Pack>, u64)> {
    let (filter, sort) = browse_query(q);
    paginate(state, filter, sort, q.limit, q.offset).await
}

pub async fn list_owned(state: &AppState, actor: &SiteUser) -> Res<Vec<Value>> {
    let filter = doc! {"$or": [{"owner_id": actor.id}, {"collaborators.user_id": actor.id}]};
    let mut docs = find_packs(state, filter, doc! {"updated_at": -1}, 0, 0).await?;
    let mut out = Vec::with_capacity(docs.len());
    for p in &mut docs {
        if p.owner_id == actor.id && p.owner_handle != actor.username {
            p.owner_handle = actor.username.clone();
            save(state, p).await?;
        }
        let collab = Value::Bool(p.owner_id != actor.id);
        out.push(Value::Object(merge(pack_card(p), [("is_collaborator".to_owned(), collab)])));
    }
    Ok(out)
}

/// `backfill_modpack_handles`: resync every drifted `owner_handle` to the
/// owner's current username. Idempotent, so it runs on every boot.
pub async fn backfill_handles(state: &AppState) -> Res<usize> {
    let pipeline = [
        doc! {"$lookup": {"from": crate::auth::models::SITE_USERS, "localField": "owner_id",
        "foreignField": "_id", "as": "_owner"}},
        doc! {"$unwind": "$_owner"},
        doc! {"$match": {"$expr": {"$ne": ["$owner_handle", "$_owner.username"]}}},
        doc! {"$project": {"_id": 1, "username": "$_owner.username"}},
    ];
    let rows: Vec<Document> =
        coll(state).aggregate(pipeline).await.map_err(internal)?.try_collect().await.map_err(internal)?;
    for row in &rows {
        let (Some(id), Some(name)) = (row.get("_id"), row.get("username")) else { continue };
        coll(state)
            .update_one(doc! {"_id": id.clone()}, doc! {"$set": {"owner_handle": name.clone()}})
            .await
            .map_err(internal)?;
    }
    if !rows.is_empty() {
        tracing::info!("modpacks: resynced owner_handle on {} project(s)", rows.len());
    }
    Ok(rows.len())
}

pub async fn packs_including_mod(state: &AppState, mod_id: ObjectId) -> Res<Vec<Pack>> {
    let filter = doc! {"visibility": "public", "taken_down": false, "variants.entries.project_id": mod_id};
    find_packs(state, filter, doc! {"download_count": -1}, 0, 60).await
}

/// `site_packs_for_mod` (`public` false) / `public_packs_for_mod`.
pub async fn packs_for_mod(state: &AppState, handle: &str, slug: &str, public: bool) -> Res<Vec<Value>> {
    let Some(m) = hub::get_project(state, handle, slug).await? else { return Ok(Vec::new()) };
    let packs = packs_including_mod(state, m.id).await?;
    let b = Bases::of(state);
    Ok(packs.iter().map(|p| Value::Object(if public { public_pack_card(b, p) } else { pack_card(p) })).collect())
}

// --- create / update -------------------------------------------------------

async fn unique_slug(state: &AppState, owner: ObjectId, title: &str) -> Res<String> {
    let base = text::slugify(title);
    let mut candidate = base.clone();
    let mut n = 2;
    while find_pack(state, and(vec![doc! {"owner_id": owner}, doc! {"slug": &candidate}])).await?.is_some() {
        candidate = format!("{base}-{n}");
        n += 1;
    }
    Ok(candidate)
}

/// The `ModpackProject` `create_modpack` inserts (`created` / `updated`:
/// its two `utcnow()` defaults).
pub fn new_pack(
    actor: &SiteUser,
    req: &CreateBody,
    slug: String,
    created: DateTime<Utc>,
    updated: DateTime<Utc>,
) -> Pack {
    let owner_username = actor.display_name.clone().filter(|d| !d.is_empty()).unwrap_or_else(|| actor.username.clone());
    Pack {
        id: ObjectId::new(),
        slug,
        title: strip(&req.title),
        summary: strip(&req.summary),
        description: req.description.clone(),
        warnings: String::new(),
        tags: text::clean_tags(&req.tags),
        owner_id: actor.id,
        owner_username,
        owner_handle: actor.username.clone(),
        collaborators: Vec::new(),
        visibility: req.visibility.clone(),
        banner_sha: None,
        preview_shas: Vec::new(),
        discord_url: None,
        website_url: None,
        donation_urls: Vec::new(),
        download_count: 0,
        star_count: 0,
        variants: vec![Variant { name: DEFAULT_VARIANT.into(), label: "Default".into(), entries: Vec::new() }],
        default_variant: DEFAULT_VARIANT.into(),
        taken_down: false,
        takedown_reason: None,
        created_at: created,
        updated_at: updated,
    }
}

pub async fn create_modpack(state: &AppState, actor: &SiteUser, req: &CreateBody) -> Res<Pack> {
    let slug = unique_slug(state, actor.id, &req.title).await?;
    let pack = new_pack(actor, req, slug, dt::now(), dt::now());
    coll(state).insert_one(pack.to_doc()).await.map_err(internal)?;
    Ok(pack)
}

/// `update_modpack`'s edits.
pub fn apply_update(pack: &mut Pack, actor_username: &str, req: &UpdateBody, now: DateTime<Utc>) -> Res<()> {
    if let Some(t) = &req.title {
        pack.title = strip(t);
    }
    if let Some(s) = &req.summary {
        pack.summary = strip(s);
    }
    if let Some(d) = &req.description {
        pack.description = d.clone();
    }
    if let Some(w) = &req.warnings {
        pack.warnings = w.clone();
    }
    if let Some(t) = &req.tags {
        pack.tags = text::clean_tags(t);
    }
    if let Some(v) = &req.visibility {
        pack.visibility = v.clone();
    }
    if let Some(u) = &req.discord_url {
        pack.discord_url = text::clean_url(Some(u), "Discord")?;
    }
    if let Some(u) = &req.website_url {
        pack.website_url = text::clean_url(Some(u), "website")?;
    }
    if let Some(urls) = &req.donation_urls {
        let mut cleaned = Vec::new();
        for u in urls {
            if let Some(u) = text::clean_url(Some(u), "support")? {
                cleaned.push(u);
            }
        }
        cleaned.truncate(5);
        pack.donation_urls = cleaned;
    }
    if let Some(dv) = &req.default_variant {
        if pack.variant_index(dv).is_none() {
            return Err(ApiError::bad_request("No such variant to make default."));
        }
        pack.default_variant = dv.clone();
    }
    if let Some(order) = &req.variant_order {
        let unlisted = order.iter().collect::<std::collections::HashSet<_>>().len();
        let rank = |name: &str| order.iter().rposition(|n| n == name).unwrap_or(unlisted);
        pack.variants.sort_by_key(|v| rank(&v.name));
    }
    pack.owner_handle = actor_username.to_owned();
    pack.updated_at = now;
    Ok(())
}

pub async fn update_modpack(state: &AppState, mut pack: Pack, actor: &SiteUser, req: &UpdateBody) -> Res<Pack> {
    apply_update(&mut pack, &actor.username, req, dt::now())?;
    save(state, &mut pack).await?;
    Ok(pack)
}

async fn delete_pack(state: &AppState, pack: &Pack) -> Res<()> {
    state.mongo().collection::<Document>(STARS).delete_many(doc! {"modpack_id": pack.id}).await.map_err(internal)?;
    coll(state).delete_one(doc! {"_id": pack.id}).await.map_err(internal)?;
    Ok(())
}

pub async fn delete_modpack(state: &AppState, pack: &Pack, actor: &SiteUser) -> Res<()> {
    require_primary_owner(pack, actor.id)?;
    delete_pack(state, pack).await
}

pub async fn set_banner(state: &AppState, mut pack: Pack, sha: String) -> Res<Pack> {
    if !hub::image_exists(state, &sha).await? {
        return Err(not_found("No such uploaded image"));
    }
    pack.banner_sha = Some(sha);
    pack.updated_at = dt::now();
    save(state, &mut pack).await?;
    Ok(pack)
}

// --- master moderation -----------------------------------------------------

/// The `master_list_modpacks` filter.
pub fn master_query(q: &AdminListQuery) -> Document {
    let mut filter = Document::new();
    if let Some(term) = nonempty(&q.q) {
        filter.extend(search_clause(term));
    }
    if let Some(owner) = nonempty(&q.owner) {
        filter.insert("owner_username", owner);
    }
    if let Some(v) = nonempty(&q.visibility) {
        filter.insert("visibility", v);
    }
    filter
}

pub fn master_card(p: &Pack) -> Value {
    Value::Object(merge(
        pack_card(p),
        [
            ("id".to_owned(), Value::from(p.id.to_hex())),
            ("taken_down".to_owned(), p.taken_down.into()),
            ("owner_id".to_owned(), p.owner_id.to_hex().into()),
        ],
    ))
}

pub async fn master_list(state: &AppState, q: &AdminListQuery) -> Res<(Vec<Value>, u64)> {
    let (docs, total) = paginate(state, master_query(q), doc! {"updated_at": -1}, q.limit, q.offset).await?;
    Ok((docs.iter().map(master_card).collect(), total))
}

/// `moderation.notify_takedown` for a modpack: `(subject, text, html)`.
pub fn takedown_email(label: &str, reason: &str, url: &str) -> (String, String, String) {
    let reason = kiwi_core::pystr::strip(if reason.is_empty() { "Removed by a moderator." } else { reason });
    let location = if url.is_empty() { String::new() } else { format!("Location: {url}") };
    let text_body = [
        format!("Your modpack \"{label}\" has been removed by a moderator on Better Trove Tools."),
        location,
        String::new(),
        format!("Reason: {reason}"),
        String::new(),
        format!("If you think this was a mistake, email {APPEAL_CONTACT} and we'll take another look."),
        "- Better Trove Tools".into(),
    ]
    .join("\n");
    let loc_html = if url.is_empty() { String::new() } else { format!("<p style='color:#9aa4b2'>{url}</p>") };
    let html = format!(
        "<div style=\"font-family:system-ui,sans-serif;max-width:480px;margin:0 auto;\
padding:24px;color:#e8ecf3;background:#0a0e14\">\
<h1 style='font-size:1.3rem;margin:0 0 12px'>Your content was removed</h1>\
<p>Your modpack <strong>{label}</strong> has been removed by a moderator.</p>\
{loc_html}\
<p style='margin:16px 0 4px;color:#9aa4b2'>Reason</p>\
<p style='background:#161b22;border:1px solid #232a33;border-radius:8px;\
padding:12px 14px'>{reason}</p>\
<p style='margin-top:18px'>If you think this was a mistake, email \
<a href='mailto:{APPEAL_CONTACT}' style='color:#569cff'>{APPEAL_CONTACT}</a> \
and we'll take another look.</p>\
<p style='color:#9aa4b2'>- Better Trove Tools</p></div>"
    );
    ("Your modpack was removed on Better Trove Tools".into(), text_body, html)
}

pub async fn take_down(state: &AppState, pack_id: &str, reason: &str) -> Res<Pack> {
    let mut pack = pack_by_id(state, pack_id).await?;
    pack.taken_down = true;
    let r = strip(reason);
    pack.takedown_reason = (!r.is_empty()).then_some(r);
    pack.updated_at = dt::now();
    save(state, &mut pack).await?;
    state
        .mongo()
        .collection::<Document>("content_reports")
        .update_many(
            and(vec![doc! {"target_type": "modpack"}, doc! {"target_id": pack.id}, doc! {"resolved": false}]),
            doc! {"$set": {"resolved": true}},
        )
        .await
        .map_err(internal)?;
    let owner = state
        .mongo()
        .collection::<Document>(crate::auth::models::SITE_USERS)
        .find_one(doc! {"_id": pack.owner_id})
        .await
        .map_err(internal)?;
    if let Some(to) = owner.as_ref().and_then(|o| o.get_str("notify_email").ok()).filter(|e| !e.is_empty()) {
        let reason = pack.takedown_reason.clone().unwrap_or_else(|| "Removed by a moderator.".into());
        let url = format!("/modpacks/{}/{}", pack.owner_handle, pack.slug);
        let (subject, text_body, html) = takedown_email(&pack.title, &reason, &url);
        if let Err(e) = crate::email::queue_email(state, to, &subject, &text_body, Some(&html)).await {
            tracing::warn!("takedown email failed for modpack {}: {e:#}", pack.title);
        }
    }
    Ok(pack)
}

pub async fn restore(state: &AppState, pack_id: &str) -> Res<Pack> {
    let mut pack = pack_by_id(state, pack_id).await?;
    pack.taken_down = false;
    pack.takedown_reason = None;
    pack.updated_at = dt::now();
    save(state, &mut pack).await?;
    Ok(pack)
}

pub async fn master_delete(state: &AppState, pack_id: &str) -> Res<()> {
    let pack = pack_by_id(state, pack_id).await?;
    delete_pack(state, &pack).await
}

// --- collaborators ---------------------------------------------------------

/// `add_collaborator`'s username normalization.
pub fn collaborator_name(username: &str) -> String {
    text::lower(kiwi_core::pystr::strip(username).trim_start_matches('@'))
}

/// `add_collaborator` once the user is found: `Ok(false)` when nothing changes.
pub fn apply_add_collaborator(pack: &mut Pack, uid: ObjectId, username: &str, now: DateTime<Utc>) -> Res<bool> {
    if uid == pack.owner_id {
        return Err(ApiError::bad_request("That's the owner."));
    }
    if pack.collaborators.iter().any(|c| c.user_id == uid) {
        return Ok(false);
    }
    if pack.collaborators.len() >= 20 {
        return Err(ApiError::bad_request("Too many collaborators (max 20)."));
    }
    pack.collaborators.push(Collaborator { user_id: uid, username: username.to_owned() });
    pack.updated_at = now;
    Ok(true)
}

pub async fn add_collaborator(state: &AppState, mut pack: Pack, actor: &SiteUser, username: &str) -> Res<Pack> {
    require_primary_owner(&pack, actor.id)?;
    let uname = collaborator_name(username);
    if uname.is_empty() {
        return Err(ApiError::bad_request("Enter a username to collaborate with."));
    }
    let user = state
        .mongo()
        .collection::<Document>(crate::auth::models::SITE_USERS)
        .find_one(doc! {"username": &uname})
        .await
        .map_err(internal)?
        .ok_or_else(|| not_found(&format!("No site user '@{uname}'. They must sign in once first.")))?;
    let uid = model::oid(&user, "_id")?;
    let name = model::s_or(&user, "username", "")?;
    if apply_add_collaborator(&mut pack, uid, &name, dt::now())? {
        save(state, &mut pack).await?;
    }
    Ok(pack)
}

pub fn apply_remove_collaborator(pack: &mut Pack, user_id: &str, now: DateTime<Utc>) {
    let uid = ObjectId::parse_str(user_id).ok();
    pack.collaborators.retain(|c| Some(c.user_id) != uid);
    pack.updated_at = now;
}

pub async fn remove_collaborator(state: &AppState, mut pack: Pack, actor: &SiteUser, user_id: &str) -> Res<Pack> {
    require_primary_owner(&pack, actor.id)?;
    apply_remove_collaborator(&mut pack, user_id, dt::now());
    save(state, &mut pack).await?;
    Ok(pack)
}

// --- variants --------------------------------------------------------------

pub fn apply_add_variant(pack: &mut Pack, name: &str, copy_from: Option<&str>, now: DateTime<Utc>) -> Res<()> {
    let slug = text::slugify(name);
    if pack.variant_index(&slug).is_some() {
        return Err(ApiError::conflict(format!("A variant named '{slug}' already exists.")));
    }
    if pack.variants.len() >= 30 {
        return Err(ApiError::bad_request("Too many variants (max 30)."));
    }
    let entries = copy_from
        .filter(|c| !c.is_empty())
        .and_then(|c| pack.variant_index(c))
        .map(|i| pack.variants[i].entries.clone())
        .unwrap_or_default();
    pack.variants.push(Variant { name: slug, label: strip(name), entries });
    pack.updated_at = now;
    Ok(())
}

pub fn apply_update_variant(pack: &mut Pack, name: &str, label: Option<&str>, now: DateTime<Utc>) -> Res<()> {
    let i = variant_or_404(pack, name)?;
    if let Some(l) = label {
        pack.variants[i].label = strip(l);
    }
    pack.updated_at = now;
    Ok(())
}

pub fn apply_delete_variant(pack: &mut Pack, name: &str, now: DateTime<Utc>) -> Res<()> {
    variant_or_404(pack, name)?;
    if pack.variants.len() <= 1 {
        return Err(ApiError::bad_request("A modpack needs at least one variant."));
    }
    pack.variants.retain(|v| v.name != name);
    if pack.default_variant == name {
        pack.default_variant = pack.variants[0].name.clone();
    }
    pack.updated_at = now;
    Ok(())
}

pub async fn add_variant(state: &AppState, mut pack: Pack, name: &str, copy_from: Option<&str>) -> Res<Pack> {
    apply_add_variant(&mut pack, name, copy_from, dt::now())?;
    save(state, &mut pack).await?;
    Ok(pack)
}

pub async fn update_variant(state: &AppState, mut pack: Pack, name: &str, label: Option<&str>) -> Res<Pack> {
    apply_update_variant(&mut pack, name, label, dt::now())?;
    save(state, &mut pack).await?;
    Ok(pack)
}

pub async fn delete_variant(state: &AppState, mut pack: Pack, name: &str) -> Res<Pack> {
    apply_delete_variant(&mut pack, name, dt::now())?;
    save(state, &mut pack).await?;
    Ok(pack)
}

// --- entries ---------------------------------------------------------------

pub type Fingerprint = (String, BTreeSet<String>);

/// `_fingerprint_bytes`: the lowercased header title and replaced paths
/// (the mod's own preview image excluded).
pub fn fingerprint(data: &[u8]) -> Option<Fingerprint> {
    let h = hub::read_tmod(data, true)?;
    let title = text::lower(kiwi_core::pystr::strip(h.prop("title").unwrap_or("")));
    let preview = text::lower(kiwi_core::pystr::strip(h.prop("previewPath").unwrap_or("")));
    let mut files: BTreeSet<String> =
        h.files.iter().filter(|(p, _)| !p.is_empty()).map(|(p, _)| text::lower(p)).collect();
    files.remove(&preview);
    Some((title, files))
}

/// `_pair_conflicts`.
pub fn pair_conflicts(a: &str, fa: &Fingerprint, b: &str, fb: &Fingerprint) -> Vec<String> {
    let mut out = Vec::new();
    if !fa.0.is_empty() && fa.0 == fb.0 {
        out.push(format!("“{a}” and “{b}” share the same mod title."));
    }
    let shared: Vec<&str> = fa.1.intersection(&fb.1).map(String::as_str).collect();
    if !shared.is_empty() {
        let mut shown = shared.iter().take(3).copied().collect::<Vec<_>>().join(", ");
        if shared.len() > 3 {
            shown.push_str(" …");
        }
        out.push(format!("“{a}” and “{b}” both replace: {shown}"));
    }
    out
}

/// `_detect_conflicts` over fingerprinted entries.
pub fn all_conflicts(fps: &[(String, Fingerprint)]) -> Vec<String> {
    let mut out = Vec::new();
    for i in 0..fps.len() {
        for j in i + 1..fps.len() {
            out.extend(pair_conflicts(&fps[i].0, &fps[i].1, &fps[j].0, &fps[j].1));
        }
    }
    out
}

/// The CAS key of an entry's packed `.tmod` (custom upload or release).
fn entry_sha(e: &Entry, rel: Option<&Release>) -> Option<String> {
    e.custom().map(str::to_owned).or_else(|| rel.map(|r| r.tmod_sha.clone())).filter(|s| !s.is_empty())
}

/// The label and fingerprint of every entry resolving to a packable build;
/// a blank title falls back to `fallback`, else the slug or "Uploaded mod".
async fn fingerprints(state: &AppState, entries: &[Entry], fallback: Option<&str>) -> Res<Vec<(String, Fingerprint)>> {
    let mut out = Vec::new();
    for e in entries {
        let (view, rel) = resolve_entry(state, e).await?;
        if !view.available {
            continue;
        }
        let Some(sha) = entry_sha(e, rel.as_ref()) else { continue };
        let Some(data) = hub::get_blob(state, &sha).await? else { continue };
        let Some(fp) = tokio::task::spawn_blocking(move || fingerprint(&data)).await.map_err(internal)? else {
            continue;
        };
        let label = match (view.title.is_empty(), fallback) {
            (false, _) => view.title,
            (true, Some(f)) => f.to_owned(),
            (true, None) if !e.slug.is_empty() => e.slug.clone(),
            (true, None) => "Uploaded mod".into(),
        };
        out.push((label, fp));
    }
    Ok(out)
}

/// One `set_entries` input once looked up: a custom upload, or the hub mod
/// (`None`: missing or hidden).
pub enum Input<'a> {
    Custom { item: &'a EntryInput, present: bool },
    Hub { item: &'a EntryInput, m: Option<ModRef> },
}

/// `set_entries`' validation and dedupe, before the conflict check.
pub fn resolve_inputs(inputs: Vec<Input<'_>>) -> Res<Vec<Entry>> {
    let mut resolved: Vec<Entry> = Vec::new();
    let mut seen_sha: Vec<String> = Vec::new();
    let mut seen_mod: Vec<(ObjectId, String)> = Vec::new();
    for input in inputs {
        match input {
            Input::Custom { item, present } => {
                let sha = item.custom_sha.clone().unwrap_or_default();
                if !present {
                    return Err(ApiError::bad_request("An uploaded mod is missing from storage."));
                }
                if seen_sha.contains(&sha) {
                    continue;
                }
                seen_sha.push(sha.clone());
                resolved.push(Entry {
                    custom_sha: Some(sha),
                    custom_filename: item.custom_filename.clone(),
                    title: if item.title.is_empty() { "Uploaded mod".into() } else { item.title.clone() },
                    author: item.author.clone(),
                    ..Entry::default()
                });
            }
            Input::Hub { item, m } => {
                let Some(m) = m.filter(|m| !m.unavailable()) else {
                    return Err(ApiError::bad_request(format!(
                        "Mod '{}/{}' isn't available to add.",
                        item.handle, item.slug
                    )));
                };
                let b = strip(if item.branch.is_empty() { "main" } else { &item.branch });
                let branch = if b.is_empty() { "main".to_owned() } else { b };
                let key = (m.id, branch.clone());
                if seen_mod.contains(&key) {
                    continue;
                }
                seen_mod.push(key);
                resolved.push(Entry {
                    project_id: Some(m.id),
                    handle: m.owner_handle,
                    slug: m.slug,
                    title: m.title,
                    branch,
                    version_locked: item.version_locked,
                    locked_tag: if item.version_locked {
                        item.locked_tag.clone().filter(|t| !t.is_empty())
                    } else {
                        None
                    },
                    ..Entry::default()
                });
            }
        }
    }
    Ok(resolved)
}

pub async fn set_entries(state: &AppState, mut pack: Pack, variant: &str, input: &[EntryInput]) -> Res<Pack> {
    let vi = variant_or_404(&pack, variant)?;
    let mut looked_up = Vec::with_capacity(input.len());
    for item in input {
        let looked = match item.custom_sha.as_deref().filter(|s| !s.is_empty()) {
            Some(sha) => {
                let present = hub::has_blob(state, sha).await?;
                if !present {
                    return Err(ApiError::bad_request("An uploaded mod is missing from storage."));
                }
                Input::Custom { item, present }
            }
            None => {
                let m = hub::get_project(state, &item.handle, &item.slug).await?;
                if m.as_ref().is_none_or(ModRef::unavailable) {
                    return Err(ApiError::bad_request(format!(
                        "Mod '{}/{}' isn't available to add.",
                        item.handle, item.slug
                    )));
                }
                Input::Hub { item, m }
            }
        };
        looked_up.push(looked);
    }
    let resolved = resolve_inputs(looked_up)?;
    let conflicts = all_conflicts(&fingerprints(state, &resolved, None).await?);
    if !conflicts.is_empty() {
        return Err(ApiError::conflict(format!(
            "These mods conflict and can't be in the same variant: {}",
            conflicts.join(" ")
        )));
    }
    pack.variants[vi].entries = resolved;
    pack.updated_at = dt::now();
    save(state, &mut pack).await?;
    Ok(pack)
}

/// What `upload_entry` did: the pack, and the matched mod's handle and slug
/// when the upload is a build the hub already hosts.
pub struct Uploaded {
    pub pack: Pack,
    pub matched: Option<(String, String)>,
}

/// `upload_entry`'s checks on the bytes: `(fingerprint, title, author)`.
pub fn vet_upload(data: &[u8]) -> Res<(Fingerprint, String, String)> {
    if data.is_empty() {
        return Err(ApiError::bad_request("The uploaded file is empty."));
    }
    if data.len() > MAX_UPLOAD_BYTES {
        return Err(ApiError::new(StatusCode::PAYLOAD_TOO_LARGE, "bad_request", "That .tmod is too large to upload."));
    }
    let (Some(fp), Some(h)) = (fingerprint(data), hub::read_tmod(data, true)) else {
        return Err(ApiError::bad_request("That isn't a valid .tmod file."));
    };
    let title = strip(h.prop("title").unwrap_or(""));
    if title.is_empty() {
        return Err(ApiError::bad_request("The .tmod has no title in its header."));
    }
    Ok((fp, title, strip(h.prop("author").unwrap_or(""))))
}

/// The entry an upload adds: a version-locked reference to the hub mod that
/// ships the same bytes, else the custom upload. `None` when it's already there.
pub fn upload_addition(
    entries: &[Entry],
    sha: &str,
    title: &str,
    author: &str,
    hosted: Option<(&ModRef, &Release)>,
) -> Option<Entry> {
    match hosted {
        Some((m, rel)) => {
            (!entries.iter().any(|e| e.project_id == Some(m.id) && e.branch == rel.branch)).then(|| Entry {
                project_id: Some(m.id),
                handle: m.owner_handle.clone(),
                slug: m.slug.clone(),
                title: m.title.clone(),
                branch: rel.branch.clone(),
                version_locked: true,
                locked_tag: Some(rel.tag.clone()),
                ..Entry::default()
            })
        }
        None => (!entries.iter().any(|e| e.custom_sha.as_deref() == Some(sha))).then(|| Entry {
            custom_sha: Some(sha.to_owned()),
            custom_filename: Some(format!("{}.tmod", text::safe_filename(title, "modpack"))),
            title: title.to_owned(),
            author: author.to_owned(),
            ..Entry::default()
        }),
    }
}

pub async fn upload_entry(state: &AppState, mut pack: Pack, variant: &str, data: Vec<u8>) -> Res<Uploaded> {
    let vi = variant_or_404(&pack, variant)?;
    let (vetted, data) = tokio::task::spawn_blocking(move || (vet_upload(&data), data)).await.map_err(internal)?;
    let (fp, title, author) = vetted?;
    let existing = fingerprints(state, &pack.variants[vi].entries, Some("a mod")).await?;
    let conflicts: Vec<String> =
        existing.iter().flat_map(|(label, efp)| pair_conflicts(&title, &fp, label, efp)).collect();
    if !conflicts.is_empty() {
        return Err(ApiError::conflict(format!(
            "This mod conflicts with mods already in the variant: {}",
            conflicts.join(" ")
        )));
    }
    let sha = hub::put_blob(state, data).await?;
    let mut hosted = None;
    if let Some(rel) = hub::release_by_artifact_hash(state, &sha).await?
        && let Some(m) = hub::mod_by_id(state, rel.project_id).await?
        && !m.unavailable()
    {
        hosted = Some((m, rel));
    }
    let add = upload_addition(&pack.variants[vi].entries, &sha, &title, &author, hosted.as_ref().map(|(m, r)| (m, r)));
    pack.variants[vi].entries.extend(add);
    pack.updated_at = dt::now();
    save(state, &mut pack).await?;
    let matched = hosted.map(|(m, _)| (m.owner_handle, m.slug));
    Ok(Uploaded { pack, matched })
}

// --- stars -----------------------------------------------------------------

fn is_duplicate_key(e: &mongodb::error::Error) -> bool {
    use mongodb::error::{ErrorKind, WriteFailure};
    matches!(e.kind.as_ref(), ErrorKind::Write(WriteFailure::WriteError(w)) if w.code == 11000)
}

pub async fn star_pack(state: &AppState, actor: &SiteUser, mut pack: Pack) -> Res<Value> {
    if !has_starred(state, Some(actor.id), &pack).await? {
        let stars = state.mongo().collection::<Document>(STARS);
        match stars.insert_one(model::star_doc(pack.id, actor.id, dt::now())).await {
            Ok(_) => {
                coll(state)
                    .update_one(doc! {"_id": pack.id}, doc! {"$inc": {"star_count": 1}})
                    .await
                    .map_err(internal)?;
                pack.star_count += 1;
            }
            Err(e) if is_duplicate_key(&e) => {}
            Err(e) => return Err(internal(e)),
        }
    }
    Ok(json!({"starred": true, "star_count": pack.star_count}))
}

pub async fn unstar_pack(state: &AppState, actor: &SiteUser, mut pack: Pack) -> Res<Value> {
    let stars = state.mongo().collection::<Document>(STARS);
    let star = stars
        .find_one(and(vec![doc! {"modpack_id": pack.id}, doc! {"site_user_id": actor.id}]))
        .await
        .map_err(internal)?;
    if let Some(star) = star {
        let id = star.get("_id").cloned().unwrap_or(Bson::Null);
        stars.delete_one(doc! {"_id": id}).await.map_err(internal)?;
        coll(state).update_one(doc! {"_id": pack.id}, doc! {"$inc": {"star_count": -1}}).await.map_err(internal)?;
        pack.star_count = (pack.star_count - 1).max(0);
    }
    Ok(json!({"starred": false, "star_count": pack.star_count}))
}

// --- download --------------------------------------------------------------

/// `_collect_builds` from resolved entries and the blobs they name:
/// `(downloadable, manifest)`.
pub fn collect_builds(
    entries: &[Entry],
    resolved: &[(EntryView, Option<Release>)],
    blobs: &HashMap<String, Vec<u8>>,
) -> (Vec<(String, Vec<u8>)>, Vec<Record>) {
    let mut downloadable = Vec::new();
    let mut records = Vec::new();
    for (e, (view, rel)) in entries.iter().zip(resolved) {
        let mut record = Record {
            handle: view.handle.clone(),
            slug: view.slug.clone(),
            title: view.title.clone(),
            branch: view.branch.clone(),
            version: view.version.clone(),
            locked: view.version_locked,
            available: view.available,
            filename: None,
            config: None,
        };
        let sha = if view.available { entry_sha(e, rel.as_ref()) } else { None };
        match sha.map(|s| blobs.get(&s)) {
            Some(Some(data)) => {
                let name = match (e.custom(), rel) {
                    (Some(_), _) => e
                        .custom_filename
                        .clone()
                        .filter(|f| !f.is_empty())
                        .unwrap_or_else(|| format!("{}.tmod", text::safe_filename(&view.title, "modpack"))),
                    (None, Some(r)) => hub::release_download_filename(r),
                    (None, None) => String::new(),
                };
                record.filename = Some(name.clone());
                downloadable.push((name, data.clone()));
            }
            Some(None) => record.available = false,
            None => {}
        }
        records.push(record);
    }
    (downloadable, records)
}

/// `build_artifact`: `(bytes, filename, media type)`.
pub async fn build_artifact(
    state: &AppState,
    pack: &Pack,
    variant: Option<&str>,
    fmt: &str,
) -> Res<(Vec<u8>, String, &'static str)> {
    let Some(vi) = pack.find_variant(variant) else { return Err(not_found("This modpack has no variants")) };
    let v = &pack.variants[vi];
    let resolved = resolve_all(state, v).await?;
    let mut blobs = HashMap::new();
    for (e, (view, rel)) in v.entries.iter().zip(&resolved) {
        if let Some(sha) = entry_sha(e, rel.as_ref()).filter(|_| view.available)
            && !blobs.contains_key(&sha)
            && let Some(data) = hub::get_blob(state, &sha).await?
        {
            blobs.insert(sha, data);
        }
    }
    let (downloadable, mut records) = collect_builds(&v.entries, &resolved, &blobs);
    drop(blobs);
    if downloadable.is_empty() {
        return Err(ApiError::bad_request(
            "This modpack has no downloadable mods right now (every entry is missing a published build).",
        ));
    }
    let meta = Meta {
        title: pack.title.clone(),
        handle: pack.owner_handle.clone(),
        slug: pack.slug.clone(),
        author: pack.owner_username.clone(),
        variant: v.name.clone(),
        warnings: pack.warnings.clone(),
    };
    let fmt = fmt.to_owned();
    tokio::task::spawn_blocking(move || {
        let configs = artifact::collect_configs(&downloadable, &mut records);
        artifact::assemble(&meta, &downloadable, &records, &configs, &fmt, crate::blueprints::pack::now_date_time())
    })
    .await
    .map_err(internal)?
}

/// `record_download`, after the response: `$inc`, best effort.
pub fn record_download(state: &AppState, pack_id: ObjectId) {
    let c = coll(state);
    tokio::spawn(async move {
        if let Err(e) = c.update_many(doc! {"_id": pack_id}, doc! {"$inc": {"download_count": 1}}).await {
            tracing::warn!("modpacks: failed to record download: {e}");
        }
    });
}

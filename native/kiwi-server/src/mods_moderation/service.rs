//! Master moderation over the Mods Hub and Modpacks: reports, takedowns (with
//! the statement-of-reasons email), forced deletes, the stray approval queue,
//! stray handovers, claims and remix claims (`mods_hub/service.py`,
//! `modpacks/service.py`, `trove/moderation.py`).

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use futures::TryStreamExt;
use kiwi_core::pystr;
use md5::{Digest, Md5};
use mongodb::{
    Collection,
    bson::{Bson, Document, doc, oid::ObjectId},
};
use serde_json::{Value, json};

use super::model::{
    CLAIM, CONTENT_REPORT, F, IMAGE_ASSET, MOD_PROFILE, MOD_PROJECT, MOD_RELEASE, MODPACK, Model, REMIX_CLAIM,
    TAKEN_DOWN, internal,
};
use crate::{
    auth::models::{SITE_USERS, SiteUser},
    dt,
    error::ApiError,
    media::cas::ContentStore,
    pyfloat,
    routes::store::service::re_escape,
    state::AppState,
};

pub const MOD_PROJECTS: &str = "mod_projects";
pub const MOD_RELEASES: &str = "mod_releases";
pub const MOD_PROFILES: &str = "mod_profiles";
pub const MOD_IMAGES: &str = "mod_images";
pub const MOD_STARS: &str = "mod_stars";
pub const MOD_ISSUES: &str = "mod_issues";
pub const MOD_ISSUE_EVENTS: &str = "mod_issue_events";
pub const CLAIMS: &str = "mod_claim_requests";
pub const REMIX_CLAIMS: &str = "mod_remix_claims";
pub const REPORTS: &str = "content_reports";
pub const TAKEDOWNS: &str = "mod_taken_down_artifacts";
pub const MODPACKS: &str = "modpack_projects";
pub const MODPACK_STARS: &str = "modpack_stars";

pub const STRAY_HANDLE: &str = "stray";
pub const DEFAULT_REASON: &str = "Removed by a moderator.";
const APPEAL_CONTACT: &str = "support@aallyn.net";

pub type Res<T> = Result<T, ApiError>;

pub fn coll(state: &AppState, name: &str) -> Collection<Document> {
    state.mongo().collection(name)
}

pub fn now() -> DateTime<Utc> {
    dt::now()
}

pub async fn find_models(
    c: &Collection<Document>,
    spec: &'static [F],
    filter: Document,
    sort: Option<Document>,
    skip: u64,
    limit: Option<i64>,
) -> Res<Vec<Model>> {
    let mut find = c.find(filter);
    if let Some(s) = sort {
        find = find.sort(s);
    }
    if skip > 0 {
        find = find.skip(skip);
    }
    if let Some(l) = limit {
        find = find.limit(l);
    }
    let docs: Vec<Document> = find.await.map_err(internal)?.try_collect().await.map_err(internal)?;
    docs.iter().map(|d| Model::load(spec, d)).collect()
}

pub async fn find_model(c: &Collection<Document>, spec: &'static [F], filter: Document) -> Res<Option<Model>> {
    match c.find_one(filter).await.map_err(internal)? {
        Some(d) => Model::load(spec, &d).map(Some),
        None => Ok(None),
    }
}

/// `Model.get(to_oid(id))`: a malformed id finds nothing.
pub async fn by_id(state: &AppState, name: &str, spec: &'static [F], id: &str) -> Res<Option<Model>> {
    let Ok(oid) = ObjectId::parse_str(id) else { return Ok(None) };
    find_model(&coll(state, name), spec, doc! {"_id": oid}).await
}

pub async fn site_user(state: &AppState, id: ObjectId) -> Res<Option<SiteUser>> {
    match coll(state, SITE_USERS).find_one(doc! {"_id": id}).await.map_err(internal)? {
        Some(d) => mongodb::bson::from_document(d).map(Some).map_err(internal),
        None => Ok(None),
    }
}

fn not_found(what: &str) -> ApiError {
    ApiError::not_found(what)
}

fn and2(a: Document, b: Document) -> Document {
    doc! {"$and": [a, b]}
}

// --- moderation.py ------------------------------------------------------------------

/// `list_reports` item.
pub fn report_dto(r: &Model) -> Value {
    json!({
        "id": r.id.to_hex(),
        "target_type": r.str("target_type"),
        "target_id": r.oid("target_id").map(|o| o.to_hex()),
        "target_label": r.str("target_label"),
        "target_url": r.str("target_url"),
        "reason": r.str("reason"),
        "resolved": r.bool("resolved"),
        "created_at": r.iso("created_at"),
    })
}

pub async fn list_reports(state: &AppState, resolved: bool) -> Res<Vec<Value>> {
    let rows = find_models(
        &coll(state, REPORTS),
        CONTENT_REPORT,
        doc! {"resolved": resolved},
        Some(doc! {"created_at": -1}),
        0,
        None,
    )
    .await?;
    Ok(rows.iter().map(report_dto).collect())
}

pub async fn dismiss_report(state: &AppState, report_id: &str) -> Res<()> {
    if let Some(mut r) = by_id(state, REPORTS, CONTENT_REPORT, report_id).await?
        && !r.bool("resolved")
    {
        r.set("resolved", true);
        r.save(&coll(state, REPORTS)).await?;
    }
    Ok(())
}

pub fn resolve_reports_filter(target_type: &str, target_id: ObjectId) -> Document {
    doc! {"$and": [{"target_type": target_type}, {"target_id": target_id}, {"resolved": false}]}
}

pub async fn resolve_reports_for(state: &AppState, target_type: &str, target_id: ObjectId) -> Res<()> {
    coll(state, REPORTS)
        .update_many(resolve_reports_filter(target_type, target_id), doc! {"$set": {"resolved": true}})
        .await
        .map_err(internal)?;
    Ok(())
}

/// The statement-of-reasons email: `(subject, text, html)`.
pub fn takedown_email(kind: &str, label: &str, reason: &str, url: &str) -> (String, String, String) {
    let what = match kind {
        "mod" | "modpack" | "profile" => kind,
        _ => "content",
    };
    let reason = pystr::strip(if reason.is_empty() { DEFAULT_REASON } else { reason });
    let subject = format!("Your {what} was removed on Better Trove Tools");
    let text = [
        format!("Your {what} \"{label}\" has been removed by a moderator on Better Trove Tools."),
        if url.is_empty() { String::new() } else { format!("Location: {url}") },
        String::new(),
        format!("Reason: {reason}"),
        String::new(),
        format!("If you think this was a mistake, email {APPEAL_CONTACT} and we'll take another look."),
        "- Better Trove Tools".to_owned(),
    ]
    .join("\n");
    let loc_html = if url.is_empty() { String::new() } else { format!("<p style='color:#9aa4b2'>{url}</p>") };
    let html = format!(
        "<div style=\"font-family:system-ui,sans-serif;max-width:480px;margin:0 auto;\
         padding:24px;color:#e8ecf3;background:#0a0e14\">\
         <h1 style='font-size:1.3rem;margin:0 0 12px'>Your content was removed</h1>\
         <p>Your {what} <strong>{label}</strong> has been removed by a moderator.</p>\
         {loc_html}\
         <p style='margin:16px 0 4px;color:#9aa4b2'>Reason</p>\
         <p style='background:#161b22;border:1px solid #232a33;border-radius:8px;\
         padding:12px 14px'>{reason}</p>\
         <p style='margin-top:18px'>If you think this was a mistake, email \
         <a href='mailto:{APPEAL_CONTACT}' style='color:#569cff'>{APPEAL_CONTACT}</a> \
         and we'll take another look.</p>\
         <p style='color:#9aa4b2'>- Better Trove Tools</p></div>"
    );
    (subject, text, html)
}

pub async fn notify_takedown(
    state: &AppState,
    notify_email: Option<&str>,
    kind: &str,
    label: &str,
    reason: &str,
    url: &str,
) {
    let Some(to) = notify_email.filter(|e| !e.is_empty()) else { return };
    let (subject, text, html) = takedown_email(kind, label, reason, url);
    if let Err(e) = crate::email::queue_email(state, to, &subject, &text, Some(&html)).await {
        tracing::warn!("takedown email failed for {kind} {label}: {e:#}");
    }
}

async fn owner_email(state: &AppState, owner: Option<ObjectId>) -> Res<Option<String>> {
    match owner {
        Some(id) => Ok(site_user(state, id).await?.and_then(|u| u.notify_email)),
        None => Ok(None),
    }
}

// --- cards ---------------------------------------------------------------------------

fn lineage_block(p: &Model, prefix: &str) -> Value {
    let key = |s: &str| format!("{prefix}_{s}");
    if p.opt_str(&key("slug")).is_none_or(str::is_empty) {
        return Value::Null;
    }
    json!({
        "slug": p.json_str_opt(&key("slug")),
        "handle": p.json_str_opt(&key("handle")),
        "title": p.json_str_opt(&key("title")),
        "owner": p.json_str_opt(&key("owner")),
    })
}

/// `project_card(p)`.
pub fn project_card(p: &Model) -> Value {
    let visibility = match p.str("visibility") {
        "public_website_only" => "public",
        v => v,
    };
    let author = if p.str("author").is_empty() { p.str("owner_username") } else { p.str("author") };
    json!({
        "slug": p.str("slug"),
        "handle": p.str("owner_handle"),
        "title": p.str("title"),
        "title_i18n": p.map("title_i18n"),
        "summary": p.str("summary"),
        "summary_i18n": p.map("summary_i18n"),
        "tags": p.strs("tags"),
        "owner_username": p.str("owner_username"),
        "visibility": visibility,
        "taken_down": p.bool("taken_down"),
        "is_beta": p.bool("is_beta"),
        "mode": p.str("mode"),
        "banner_sha": p.json_str_opt("banner_sha"),
        "preview_sha": p.strs("preview_shas").into_iter().next(),
        "download_count": p.int("download_count"),
        "star_count": p.int("star_count"),
        "is_stray": p.bool("is_stray"),
        "uploaded_on_behalf": p.bool("uploaded_on_behalf"),
        "author": author,
        "updated_at": p.iso("updated_at"),
        "last_release_at": p.iso("last_release_at"),
        "created_at": p.iso("created_at"),
        "forked_from": lineage_block(p, "forked_from"),
        "inspired_by": lineage_block(p, "inspired_by"),
        "fork_count": p.int("fork_count"),
    })
}

/// `{**project_card(p), "id": ..., "stray_status": ..., "taken_down": ...}`.
pub fn stray_card(p: &Model) -> Value {
    let mut card = project_card(p);
    card["id"] = json!(p.id.to_hex());
    card["stray_status"] = p.json_str_opt("stray_status");
    card["taken_down"] = json!(p.bool("taken_down"));
    card
}

/// `master_list_projects` item.
pub fn master_card(p: &Model, size: i64) -> Value {
    let mut card = project_card(p);
    card["id"] = json!(p.id.to_hex());
    card["taken_down"] = json!(p.bool("taken_down"));
    card["owner_id"] = json!(p.oid("owner_id").map(|o| o.to_hex()));
    card["size_bytes"] = json!(size);
    card["popularity_score"] = json!(pyfloat::round(p.float("popularity_score"), 3));
    card["downloads_7d"] = json!(p.int("downloads_7d"));
    card
}

/// `modpacks.pack_card(p)`.
pub fn pack_card(p: &Model) -> Value {
    let variants = p.doc.get_array("variants").map(|a| a.as_slice()).unwrap_or(&[]);
    let entries = |v: &Bson| v.as_document().and_then(|d| d.get_array("entries").ok()).map_or(0, |e| e.len());
    let default_name = p.str("default_variant");
    let default_v = variants
        .iter()
        .find(|v| v.as_document().and_then(|d| d.get_str("name").ok()) == Some(default_name))
        .or(variants.first());
    json!({
        "slug": p.str("slug"),
        "handle": p.str("owner_handle"),
        "title": p.str("title"),
        "summary": p.str("summary"),
        "tags": p.strs("tags"),
        "owner_username": p.str("owner_username"),
        "visibility": p.str("visibility"),
        "taken_down": p.bool("taken_down"),
        "banner_sha": p.json_str_opt("banner_sha"),
        "preview_sha": p.strs("preview_shas").into_iter().next(),
        "download_count": p.int("download_count"),
        "star_count": p.int("star_count"),
        "variant_count": variants.len(),
        "mod_count": default_v.map_or(0, entries),
        "total_entries": variants.iter().map(entries).sum::<usize>(),
        "updated_at": p.iso("updated_at"),
        "created_at": p.iso("created_at"),
    })
}

pub fn master_pack_card(p: &Model) -> Value {
    let mut card = pack_card(p);
    card["id"] = json!(p.id.to_hex());
    card["taken_down"] = json!(p.bool("taken_down"));
    card["owner_id"] = json!(p.oid("owner_id").map(|o| o.to_hex()));
    card
}

// --- listings ---------------------------------------------------------------------------

/// `_search_clause(q)`; modpacks leave `author` out.
pub fn search_clause(q: &str, with_author: bool) -> Document {
    let rx = doc! {"$regex": re_escape(pystr::strip(q)), "$options": "i"};
    let mut or = vec![
        doc! {"title": rx.clone()},
        doc! {"summary": rx.clone()},
        doc! {"tags": rx.clone()},
        doc! {"owner_username": rx.clone()},
    ];
    if with_author {
        or.push(doc! {"author": rx});
    }
    doc! {"$or": or}
}

pub fn master_filter(q: Option<&str>, owner: Option<&str>, visibility: Option<&str>, with_author: bool) -> Document {
    let mut query = Document::new();
    if let Some(q) = q.filter(|s| !s.is_empty()) {
        query.extend(search_clause(q, with_author));
    }
    if let Some(o) = owner.filter(|s| !s.is_empty()) {
        query.insert("owner_username", o);
    }
    if let Some(v) = visibility.filter(|s| !s.is_empty()) {
        query.insert("visibility", v);
    }
    query
}

/// `_PROJECT_SORTS` (`None` = by size, computed here).
pub fn project_sort(sort: &str) -> Option<Document> {
    Some(match sort {
        "size" => return None,
        "created" => doc! {"created_at": -1},
        "popularity" => doc! {"popularity_score": -1, "download_count": -1},
        "downloads" => doc! {"download_count": -1},
        "stars" => doc! {"star_count": -1},
        _ => doc! {"updated_at": -1},
    })
}

fn num(v: Option<&Bson>) -> i64 {
    match v {
        Some(Bson::Int32(n)) => i64::from(*n),
        Some(Bson::Int64(n)) => *n,
        Some(Bson::Double(f)) => *f as i64,
        _ => 0,
    }
}

/// `_project_sizes`' grouped release query.
pub fn sizes_pipeline(ids: &[ObjectId]) -> Vec<Document> {
    vec![
        doc! {"$match": {"project_id": {"$in": ids.to_vec()}}},
        doc! {"$group": {"_id": "$project_id", "s": {"$sum": "$tmod_size"}}},
    ]
}

/// The image shas `_project_sizes` looks up (banner, then previews).
pub fn size_shas(projects: &[Model]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for p in projects {
        if let Some(b) = p.opt_str("banner_sha").filter(|s| !s.is_empty()) {
            out.push(b.to_owned());
        }
        out.extend(p.strs("preview_shas"));
    }
    out.sort();
    out.dedup();
    out
}

/// `_project_sizes` from the grouped release rows and the image docs.
pub fn compute_sizes(projects: &[Model], rel_rows: &[Document], images: &[Model]) -> HashMap<ObjectId, i64> {
    let rel: HashMap<String, i64> =
        rel_rows.iter().map(|r| (crate::admin::users::id_key(r.get("_id")), num(r.get("s")))).collect();
    let img: HashMap<&str, i64> = images.iter().map(|a| (a.str("sha"), a.int("byte_size"))).collect();
    projects
        .iter()
        .map(|p| {
            let mut total = rel.get(&p.id.to_hex()).copied().unwrap_or(0);
            if let Some(b) = p.opt_str("banner_sha").filter(|s| !s.is_empty()) {
                total += img.get(b).copied().unwrap_or(0);
            }
            for sh in p.strs("preview_shas") {
                total += img.get(sh.as_str()).copied().unwrap_or(0);
            }
            (p.id, total)
        })
        .collect()
}

async fn project_sizes(state: &AppState, projects: &[Model]) -> Res<HashMap<ObjectId, i64>> {
    if projects.is_empty() {
        return Ok(HashMap::new());
    }
    let ids: Vec<ObjectId> = projects.iter().map(|p| p.id).collect();
    let rel_rows: Vec<Document> = coll(state, MOD_RELEASES)
        .aggregate(sizes_pipeline(&ids))
        .await
        .map_err(internal)?
        .try_collect()
        .await
        .map_err(internal)?;
    let shas = size_shas(projects);
    let images = if shas.is_empty() {
        Vec::new()
    } else {
        find_models(&coll(state, MOD_IMAGES), IMAGE_ASSET, doc! {"sha": {"$in": shas}}, None, 0, None).await?
    };
    Ok(compute_sizes(projects, &rel_rows, &images))
}

pub struct Page {
    pub limit: i64,
    pub offset: i64,
}

pub async fn master_list_projects(
    state: &AppState,
    filter: Document,
    sort: &str,
    page: &Page,
) -> Res<(Vec<Value>, u64)> {
    let c = coll(state, MOD_PROJECTS);
    let total = c.count_documents(filter.clone()).await.map_err(internal)?;
    let (docs, sizes) = match project_sort(sort) {
        None => {
            let mut all = find_models(&c, MOD_PROJECT, filter, None, 0, None).await?;
            let sizes = project_sizes(state, &all).await?;
            all.sort_by_key(|p| std::cmp::Reverse(sizes.get(&p.id).copied().unwrap_or(0)));
            let docs: Vec<Model> = all.into_iter().skip(page.offset as usize).take(page.limit as usize).collect();
            (docs, sizes)
        }
        Some(spec) => {
            let docs = find_models(&c, MOD_PROJECT, filter, Some(spec), page.offset as u64, Some(page.limit)).await?;
            let sizes = project_sizes(state, &docs).await?;
            (docs, sizes)
        }
    };
    Ok((docs.iter().map(|p| master_card(p, sizes.get(&p.id).copied().unwrap_or(0))).collect(), total))
}

pub fn stray_filter(status: Option<&str>, q: Option<&str>) -> Document {
    let mut query = doc! {"is_stray": true};
    if let Some(s) = status.filter(|s| !s.is_empty()) {
        query.insert("stray_status", s);
    }
    if let Some(q) = q.filter(|s| !s.is_empty()) {
        query.extend(search_clause(q, true));
    }
    query
}

pub async fn master_list_stray(state: &AppState, filter: Document, page: &Page) -> Res<(Vec<Value>, u64)> {
    let c = coll(state, MOD_PROJECTS);
    let total = c.count_documents(filter.clone()).await.map_err(internal)?;
    let docs =
        find_models(&c, MOD_PROJECT, filter, Some(doc! {"updated_at": -1}), page.offset as u64, Some(page.limit))
            .await?;
    Ok((docs.iter().map(stray_card).collect(), total))
}

pub async fn master_list_modpacks(state: &AppState, filter: Document, page: &Page) -> Res<(Vec<Value>, u64)> {
    let c = coll(state, MODPACKS);
    let total = c.count_documents(filter.clone()).await.map_err(internal)?;
    let docs =
        find_models(&c, MODPACK, filter, Some(doc! {"updated_at": -1}), page.offset as u64, Some(page.limit)).await?;
    Ok((docs.iter().map(master_pack_card).collect(), total))
}

// --- takedowns ----------------------------------------------------------------------------

/// The `TakenDownArtifact` upsert for one release hash.
pub fn takedown_upsert(sha: &str, project: &Model, at: DateTime<Utc>) -> (Document, Document) {
    (
        doc! {"sha256": sha},
        doc! {
            "$set": {"project_id": project.id, "title": project.str("title"), "owner_handle": project.str("owner_handle")},
            "$setOnInsert": {"created_at": dt::to_bson(at)},
        },
    )
}

/// `{tmod_sha, *prior_tmod_shas} - {None, ""}`.
pub fn release_shas(release: &Model) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for s in std::iter::once(release.str("tmod_sha").to_owned()).chain(release.strs("prior_tmod_shas")) {
        if !s.is_empty() && !out.contains(&s) {
            out.push(s);
        }
    }
    out
}

/// `_remember_takedown`.
pub async fn remember_takedown(state: &AppState, project: &Model) -> Res<()> {
    let releases =
        find_models(&coll(state, MOD_RELEASES), MOD_RELEASE, doc! {"project_id": project.id}, None, 0, None).await?;
    let c = coll(state, TAKEDOWNS);
    for release in &releases {
        for sha in release_shas(release) {
            let (filter, update) = takedown_upsert(&sha, project, now());
            c.update_one(filter, update).upsert(true).await.map_err(internal)?;
        }
    }
    fill_takedown_md5s(state).await
}

pub fn md5_hex(data: Option<&[u8]>) -> String {
    match data {
        Some(d) if !d.is_empty() => hex::encode(Md5::digest(d)),
        _ => String::new(),
    }
}

/// `_fill_takedown_md5s`: never fails on a single row.
pub async fn fill_takedown_md5s(state: &AppState) -> Res<()> {
    let c = coll(state, TAKEDOWNS);
    let rows = find_models(&c, TAKEN_DOWN, doc! {"md5": Bson::Null}, None, 0, None).await?;
    let store = ContentStore::new(&state.settings().mods_store_dir);
    for row in rows {
        let sha = row.str("sha256").to_owned();
        let st = store.clone();
        let md5 = match tokio::task::spawn_blocking(move || st.get(&sha).map(|d| md5_hex(d.as_deref()))).await {
            Ok(Ok(m)) => m,
            Ok(Err(e)) => {
                tracing::warn!("mods_hub: md5 of taken-down {} failed: {e}", row.str("sha256"));
                continue;
            }
            Err(e) => {
                tracing::warn!("mods_hub: md5 of taken-down {} failed: {e}", row.str("sha256"));
                continue;
            }
        };
        if let Err(e) = c.update_one(doc! {"_id": row.id}, doc! {"$set": {"md5": md5}}).await {
            tracing::warn!("mods_hub: md5 of taken-down {} failed: {e}", row.str("sha256"));
        }
    }
    Ok(())
}

/// `backfill_takedown_artifacts`.
pub async fn backfill_takedown_artifacts(state: &AppState) -> Res<()> {
    let recorded: Vec<Bson> = coll(state, TAKEDOWNS).distinct("project_id", doc! {}).await.map_err(internal)?;
    let projects =
        find_models(&coll(state, MOD_PROJECTS), MOD_PROJECT, doc! {"taken_down": true}, None, 0, None).await?;
    for p in &projects {
        if !recorded.contains(&Bson::ObjectId(p.id)) {
            remember_takedown(state, p).await?;
        }
    }
    fill_takedown_md5s(state).await
}

pub fn apply_takedown(p: &mut Model, reason: &str, at: DateTime<Utc>) {
    let reason = pystr::strip(reason);
    p.set("taken_down", true);
    p.set("takedown_reason", if reason.is_empty() { DEFAULT_REASON } else { reason });
    p.set_dt("updated_at", at);
}

pub fn apply_restore(p: &mut Model, at: DateTime<Utc>) {
    p.set("taken_down", false);
    p.set("takedown_reason", Bson::Null);
    p.set_dt("updated_at", at);
}

pub async fn take_down_project(state: &AppState, project_id: &str, reason: &str) -> Res<Value> {
    let mut p =
        by_id(state, MOD_PROJECTS, MOD_PROJECT, project_id).await?.ok_or_else(|| not_found("Mod project not found"))?;
    apply_takedown(&mut p, reason, now());
    p.save(&coll(state, MOD_PROJECTS)).await?;
    remember_takedown(state, &p).await?;
    resolve_reports_for(state, "mod", p.id).await?;
    let email = owner_email(state, p.oid("owner_id")).await?;
    let url = format!("/mods/{}/{}", p.str("owner_handle"), p.str("slug"));
    notify_takedown(state, email.as_deref(), "mod", p.str("title"), p.str("takedown_reason"), &url).await;
    Ok(json!({
        "slug": p.str("slug"), "handle": p.str("owner_handle"),
        "taken_down": p.bool("taken_down"), "takedown_reason": p.json_str_opt("takedown_reason"),
    }))
}

pub async fn restore_project(state: &AppState, project_id: &str) -> Res<Value> {
    let mut p =
        by_id(state, MOD_PROJECTS, MOD_PROJECT, project_id).await?.ok_or_else(|| not_found("Mod project not found"))?;
    apply_restore(&mut p, now());
    p.save(&coll(state, MOD_PROJECTS)).await?;
    coll(state, TAKEDOWNS).delete_many(doc! {"project_id": p.id}).await.map_err(internal)?;
    Ok(json!({"slug": p.str("slug"), "handle": p.str("owner_handle"), "taken_down": p.bool("taken_down")}))
}

pub async fn take_down_profile(state: &AppState, profile_id: &str, reason: &str) -> Res<Value> {
    let mut p =
        by_id(state, MOD_PROFILES, MOD_PROFILE, profile_id).await?.ok_or_else(|| not_found("Profile not found"))?;
    apply_takedown(&mut p, reason, now());
    p.save(&coll(state, MOD_PROFILES)).await?;
    resolve_reports_for(state, "profile", p.id).await?;
    let email = owner_email(state, p.oid("site_user_id")).await?;
    let label = if p.str("display_name").is_empty() { p.str("handle") } else { p.str("display_name") };
    let url = format!("/mods/{}", p.str("handle"));
    notify_takedown(state, email.as_deref(), "profile", label, p.str("takedown_reason"), &url).await;
    Ok(json!({
        "handle": p.str("handle"), "taken_down": p.bool("taken_down"),
        "takedown_reason": p.json_str_opt("takedown_reason"),
    }))
}

pub async fn restore_profile(state: &AppState, profile_id: &str) -> Res<Value> {
    let mut p =
        by_id(state, MOD_PROFILES, MOD_PROFILE, profile_id).await?.ok_or_else(|| not_found("Profile not found"))?;
    apply_restore(&mut p, now());
    p.save(&coll(state, MOD_PROFILES)).await?;
    Ok(json!({"handle": p.str("handle"), "taken_down": p.bool("taken_down")}))
}

/// A modpack takedown keeps an empty reason empty.
pub fn apply_pack_takedown(p: &mut Model, reason: &str, at: DateTime<Utc>) {
    let reason = pystr::strip(reason);
    p.set("taken_down", true);
    p.set("takedown_reason", if reason.is_empty() { Bson::Null } else { Bson::String(reason.to_owned()) });
    p.set_dt("updated_at", at);
}

pub async fn take_down_modpack(state: &AppState, pack_id: &str, reason: &str) -> Res<Value> {
    let mut p = by_id(state, MODPACKS, MODPACK, pack_id).await?.ok_or_else(|| not_found("Modpack not found"))?;
    apply_pack_takedown(&mut p, reason, now());
    p.save(&coll(state, MODPACKS)).await?;
    resolve_reports_for(state, "modpack", p.id).await?;
    let email = owner_email(state, p.oid("owner_id")).await?;
    let url = format!("/modpacks/{}/{}", p.str("owner_handle"), p.str("slug"));
    let reason = p.opt_str("takedown_reason").filter(|s| !s.is_empty()).unwrap_or(DEFAULT_REASON);
    notify_takedown(state, email.as_deref(), "modpack", p.str("title"), reason, &url).await;
    Ok(json!({
        "slug": p.str("slug"), "handle": p.str("owner_handle"),
        "taken_down": p.bool("taken_down"), "takedown_reason": p.json_str_opt("takedown_reason"),
    }))
}

pub async fn restore_modpack(state: &AppState, pack_id: &str) -> Res<Value> {
    let mut p = by_id(state, MODPACKS, MODPACK, pack_id).await?.ok_or_else(|| not_found("Modpack not found"))?;
    apply_restore(&mut p, now());
    p.save(&coll(state, MODPACKS)).await?;
    Ok(json!({"slug": p.str("slug"), "handle": p.str("owner_handle"), "taken_down": p.bool("taken_down")}))
}

pub async fn master_delete_modpack(state: &AppState, pack_id: &str) -> Res<()> {
    let p = by_id(state, MODPACKS, MODPACK, pack_id).await?.ok_or_else(|| not_found("Modpack not found"))?;
    coll(state, MODPACK_STARS).delete_many(doc! {"modpack_id": p.id}).await.map_err(internal)?;
    coll(state, MODPACKS).delete_one(doc! {"_id": p.id}).await.map_err(internal)?;
    Ok(())
}

/// `gitstore.repo_dir(project_id)`.
pub fn repo_dir(mods_store_dir: &str, project_id: &ObjectId) -> std::path::PathBuf {
    std::path::Path::new(mods_store_dir).join("git").join(format!("{}.git", project_id.to_hex()))
}

/// `_purge_project`: metadata and the git repo go, CAS blobs stay.
pub async fn purge_project(state: &AppState, p: &Model) -> Res<()> {
    if p.bool("taken_down") {
        remember_takedown(state, p).await?;
    }
    coll(state, MOD_RELEASES).delete_many(doc! {"project_id": p.id}).await.map_err(internal)?;
    coll(state, REPORTS)
        .delete_many(and2(doc! {"target_type": "mod"}, doc! {"target_id": p.id}))
        .await
        .map_err(internal)?;
    coll(state, MOD_STARS).delete_many(doc! {"project_id": p.id}).await.map_err(internal)?;
    coll(state, MOD_ISSUE_EVENTS).delete_many(doc! {"project_id": p.id}).await.map_err(internal)?;
    coll(state, MOD_ISSUES).delete_many(doc! {"project_id": p.id}).await.map_err(internal)?;
    let dir = repo_dir(&state.settings().mods_store_dir, &p.id);
    let _ = tokio::task::spawn_blocking(move || {
        if dir.exists() {
            let _ = std::fs::remove_dir_all(&dir);
        }
    })
    .await;
    coll(state, MOD_PROJECTS).delete_one(doc! {"_id": p.id}).await.map_err(internal)?;
    Ok(())
}

pub async fn master_delete_project(state: &AppState, project_id: &str) -> Res<()> {
    let p =
        by_id(state, MOD_PROJECTS, MOD_PROJECT, project_id).await?.ok_or_else(|| not_found("Mod project not found"))?;
    purge_project(state, &p).await
}

// --- stray queue and handover ---------------------------------------------------------------

async fn get_stray(state: &AppState, project_id: &str) -> Res<Model> {
    match by_id(state, MOD_PROJECTS, MOD_PROJECT, project_id).await? {
        Some(p) if p.bool("is_stray") => Ok(p),
        _ => Err(not_found("Stray mod not found")),
    }
}

/// `_mark_releases_announced`.
pub fn announce_update(project_id: ObjectId, at: DateTime<Utc>) -> (Document, Document) {
    (
        doc! {"project_id": project_id, "status": "published", "announced_at": Bson::Null},
        doc! {"$set": {"announced_at": dt::to_bson(at)}},
    )
}

pub async fn mark_releases_announced(state: &AppState, project_id: ObjectId) -> Res<()> {
    let (filter, update) = announce_update(project_id, now());
    coll(state, MOD_RELEASES).update_many(filter, update).await.map_err(internal)?;
    Ok(())
}

pub async fn approve_stray(state: &AppState, project_id: &str) -> Res<Value> {
    let mut p = get_stray(state, project_id).await?;
    mark_stray(&mut p, "approved", now());
    p.save(&coll(state, MOD_PROJECTS)).await?;
    mark_releases_announced(state, p.id).await?;
    Ok(stray_card(&p))
}

pub async fn reject_stray(state: &AppState, project_id: &str) -> Res<Value> {
    let mut p = get_stray(state, project_id).await?;
    mark_stray(&mut p, "rejected", now());
    p.save(&coll(state, MOD_PROJECTS)).await?;
    Ok(stray_card(&p))
}

/// The approval queue's verdict: approved goes public, rejected back to draft.
pub fn mark_stray(p: &mut Model, status: &'static str, at: DateTime<Utc>) {
    p.set("stray_status", status);
    p.set("visibility", if status == "approved" { "public" } else { "draft" });
    p.set_dt("updated_at", at);
}

/// `_slugify(title)`.
pub fn slugify(title: &str) -> String {
    let lower = title.to_lowercase();
    let mut out = String::with_capacity(lower.len());
    let mut dash = false;
    for c in lower.chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            out.push(c);
            dash = false;
        } else if !dash {
            out.push('-');
            dash = true;
        }
    }
    let s = out.trim_matches('-');
    let s = &s[..s.len().min(60)];
    if s.is_empty() { "mod".into() } else { s.to_owned() }
}

/// `_unique_slug(owner_id, title)`.
pub async fn unique_slug(state: &AppState, owner: Option<ObjectId>, title: &str) -> Res<String> {
    let base = slugify(title);
    let mut candidate = base.clone();
    let mut n = 2;
    let c = coll(state, MOD_PROJECTS);
    let owner = owner.map_or(Bson::Null, Bson::ObjectId);
    while c
        .find_one(and2(doc! {"owner_id": owner.clone()}, doc! {"slug": &candidate}))
        .await
        .map_err(internal)?
        .is_some()
    {
        candidate = format!("{base}-{n}");
        n += 1;
    }
    Ok(candidate)
}

/// `handover_stray(project, user)`.
pub async fn handover_stray(state: &AppState, p: &mut Model, user: &SiteUser) -> Res<()> {
    let c = coll(state, MOD_PROJECTS);
    let clash =
        c.find_one(and2(doc! {"owner_id": user.id}, doc! {"slug": p.str("slug")})).await.map_err(internal)?.is_some();
    let slug = if clash { Some(unique_slug(state, Some(user.id), p.str("title")).await?) } else { None };
    apply_handover(p, user, slug, now());
    p.save(&c).await?;
    coll(state, MOD_RELEASES)
        .update_many(doc! {"project_id": p.id}, doc! {"$set": {"owner_id": user.id}})
        .await
        .map_err(internal)?;
    mark_releases_announced(state, p.id).await
}

/// A stray mod becoming `user`'s ordinary public mod.
pub fn apply_handover(p: &mut Model, user: &SiteUser, new_slug: Option<String>, at: DateTime<Utc>) {
    p.set("owner_id", user.id);
    p.set(
        "owner_username",
        user.display_name.clone().filter(|s| !s.is_empty()).unwrap_or_else(|| user.username.clone()),
    );
    p.set("owner_handle", user.username.clone());
    p.set("is_stray", false);
    p.set("stray_status", Bson::Null);
    p.set("visibility", "public");
    if let Some(slug) = new_slug {
        p.set("slug", slug);
    }
    p.set_dt("updated_at", at);
}

pub fn pending_claims_of(project_id: ObjectId) -> Document {
    and2(doc! {"project_id": project_id}, doc! {"status": "pending"})
}

pub async fn admin_assign_stray(
    state: &AppState,
    project_ids: &[String],
    user_id: &str,
    master: ObjectId,
) -> Res<Value> {
    let user = match ObjectId::parse_str(user_id) {
        Ok(oid) => site_user(state, oid).await?,
        Err(_) => None,
    };
    let Some(user) = user else { return Err(not_found("No such user.")) };
    let mut assigned = Vec::new();
    let mut errors = Vec::new();
    for pid in project_ids {
        let p = by_id(state, MOD_PROJECTS, MOD_PROJECT, pid).await?;
        let Some(mut p) = p.filter(|p| p.bool("is_stray")) else {
            errors.push(json!({"id": pid, "error": "Not a stray mod (already claimed or missing)."}));
            continue;
        };
        handover_stray(state, &mut p, &user).await?;
        coll(state, CLAIMS)
            .update_many(
                pending_claims_of(p.id),
                doc! {"$set": {"status": "rejected", "resolved_by": master, "resolved_at": dt::to_bson(now())}},
            )
            .await
            .map_err(internal)?;
        assigned.push(json!({"id": p.id.to_hex(), "slug": p.str("slug")}));
    }
    let owner_username = user.display_name.clone().filter(|s| !s.is_empty()).unwrap_or_else(|| user.username.clone());
    Ok(json!({
        "assigned": assigned, "errors": errors,
        "owner_handle": user.username, "owner_username": owner_username,
    }))
}

// --- claims -------------------------------------------------------------------------------------

/// `_claim_dto`.
pub fn claim_dto(c: &Model) -> Value {
    json!({
        "id": c.id.to_hex(),
        "project_id": c.oid("project_id").map(|o| o.to_hex()),
        "project_slug": c.str("project_slug"),
        "project_title": c.str("project_title"),
        "claimant_username": c.str("claimant_username"),
        "message": c.str("message"),
        "status": c.str("status"),
        "created_at": c.iso("created_at"),
        "resolved_at": c.iso("resolved_at"),
    })
}

/// `_remix_claim_dto`.
pub fn remix_claim_dto(c: &Model) -> Value {
    json!({
        "id": c.id.to_hex(),
        "target": {"id": c.oid("target_id").map(|o| o.to_hex()), "slug": c.str("target_slug"), "title": c.str("target_title")},
        "original": {"id": c.oid("original_id").map(|o| o.to_hex()), "slug": c.str("original_slug"), "title": c.str("original_title")},
        "claimant_username": c.str("claimant_username"),
        "message": c.str("message"),
        "status": c.str("status"),
        "created_at": c.iso("created_at"),
        "resolved_at": c.iso("resolved_at"),
    })
}

pub fn status_filter(status: Option<&str>) -> Document {
    match status.filter(|s| !s.is_empty()) {
        Some(s) => doc! {"status": s},
        None => Document::new(),
    }
}

pub async fn list_claims(state: &AppState, name: &str, spec: &'static [F], status: Option<&str>) -> Res<Vec<Model>> {
    find_models(&coll(state, name), spec, status_filter(status), Some(doc! {"created_at": -1}), 0, Some(100)).await
}

async fn get_claim(state: &AppState, name: &str, spec: &'static [F], claim_id: &str) -> Res<Model> {
    by_id(state, name, spec, claim_id).await?.ok_or_else(|| not_found("Claim not found"))
}

pub fn resolve(c: &mut Model, status: &str, master: ObjectId, at: DateTime<Utc>) {
    c.set("status", status.to_owned());
    c.set("resolved_by", master);
    c.set_dt("resolved_at", at);
}

fn already_resolved() -> ApiError {
    ApiError::bad_request("This claim is already resolved.")
}

pub async fn approve_claim(state: &AppState, claim_id: &str, master: ObjectId) -> Res<Value> {
    let mut claim = get_claim(state, CLAIMS, CLAIM, claim_id).await?;
    if claim.str("status") != "pending" {
        return Err(already_resolved());
    }
    let project = match claim.oid("project_id") {
        Some(pid) => find_model(&coll(state, MOD_PROJECTS), MOD_PROJECT, doc! {"_id": pid}).await?,
        None => None,
    };
    let Some(mut project) = project.filter(|p| p.bool("is_stray")) else {
        return Err(ApiError::bad_request("This mod is no longer claimable."));
    };
    let user = match claim.oid("claimant_id") {
        Some(uid) => site_user(state, uid).await?,
        None => None,
    };
    let Some(user) = user else { return Err(not_found("The claiming user no longer exists.")) };
    handover_stray(state, &mut project, &user).await?;
    resolve(&mut claim, "approved", master, now());
    claim.save(&coll(state, CLAIMS)).await?;
    coll(state, CLAIMS)
        .update_many(
            pending_claims_of(project.id),
            doc! {"$set": {"status": "rejected", "resolved_at": dt::to_bson(now())}},
        )
        .await
        .map_err(internal)?;
    let mut out = claim_dto(&claim);
    out["handle"] = json!(project.str("owner_handle"));
    out["slug"] = json!(project.str("slug"));
    Ok(out)
}

pub async fn reject_claim(state: &AppState, claim_id: &str, master: ObjectId) -> Res<Value> {
    let mut claim = get_claim(state, CLAIMS, CLAIM, claim_id).await?;
    resolve(&mut claim, "rejected", master, now());
    claim.save(&coll(state, CLAIMS)).await?;
    Ok(claim_dto(&claim))
}

/// The target's remix link as `approve_remix_claim` sets it.
pub fn link_remix(target: &mut Model, original: &Model, master: ObjectId, at: DateTime<Utc>) {
    target.set("is_remix", true);
    target.set("remix_status", "approved");
    if target.dt("remix_requested_at").is_none() {
        target.set_dt("remix_requested_at", at);
    }
    target.set_dt("remix_resolved_at", at);
    target.set("remix_resolved_by", master);
    target.set("forked_from_id", original.id);
    target.set("forked_from_slug", original.str("slug").to_owned());
    target.set("forked_from_handle", original.str("owner_handle").to_owned());
    target.set("forked_from_title", original.str("title").to_owned());
    target.set("forked_from_owner", original.str("owner_username").to_owned());
}

pub async fn approve_remix_claim(state: &AppState, claim_id: &str, master: ObjectId) -> Res<Value> {
    let mut claim = get_claim(state, REMIX_CLAIMS, REMIX_CLAIM, claim_id).await?;
    if claim.str("status") != "pending" {
        return Err(already_resolved());
    }
    let c = coll(state, MOD_PROJECTS);
    let load = |id: Option<ObjectId>| {
        let c = c.clone();
        async move {
            match id {
                Some(id) => find_model(&c, MOD_PROJECT, doc! {"_id": id}).await,
                None => Ok(None),
            }
        }
    };
    let target = load(claim.oid("target_id")).await?;
    let original = load(claim.oid("original_id")).await?;
    let (Some(mut target), Some(mut original)) = (target, original) else {
        return Err(not_found("Mod no longer exists."));
    };
    let already_linked = target.oid("forked_from_id") == Some(original.id);
    link_remix(&mut target, &original, master, now());
    target.save(&c).await?;
    if !already_linked {
        original.set("fork_count", crate::auth::pyutil::bson_int(original.int("fork_count") + 1));
        original.save(&c).await?;
    }
    resolve(&mut claim, "approved", master, now());
    claim.save(&coll(state, REMIX_CLAIMS)).await?;
    Ok(remix_claim_dto(&claim))
}

pub async fn reject_remix_claim(state: &AppState, claim_id: &str, master: ObjectId) -> Res<Value> {
    let mut claim = get_claim(state, REMIX_CLAIMS, REMIX_CLAIM, claim_id).await?;
    resolve(&mut claim, "rejected", master, now());
    claim.save(&coll(state, REMIX_CLAIMS)).await?;
    Ok(remix_claim_dto(&claim))
}

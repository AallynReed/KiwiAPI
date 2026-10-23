//! The write half of `mods_hub/service.py` for this component.

use std::collections::HashSet;

use axum::http::StatusCode;
use md5::{Digest, Md5};
use mongodb::bson::{Document, doc, oid::ObjectId};
use serde_json::{Map, Value};

use super::{
    announce,
    bodies::{CreateProject, I18n, UpdateProfile, UpdateProject},
    dto::{self, remix_card, remix_claim_dto},
    git,
    model::{self, Claim, Collaborator, ImageAsset, Profile, Project, RemixClaim, StrMap},
    service::{
        can_edit, can_view, coll, get_project, internal, is_primary_owner, not_found, profile_of, profile_view,
        project_detail, project_one, save_doc, save_project, source_visible,
    },
};
use crate::{auth::models::SiteUser, dt, error::ApiError, media::cas::ContentStore, state::AppState};

pub const CONTENT_LANGS: [&str; 9] = ["fr", "de", "pt-PT", "es", "ru", "ja", "ko", "zh-CN", "th"];

fn is_duplicate(e: &mongodb::error::Error) -> bool {
    use mongodb::error::{ErrorKind, WriteFailure};
    matches!(&*e.kind, ErrorKind::Write(WriteFailure::WriteError(w)) if w.code == 11000)
}

// --- input cleaning ----------------------------------------------------------------

/// `_slugify(title)`.
pub fn slugify(title: &str) -> String {
    let mut s = String::new();
    let mut run = false;
    for c in title.to_lowercase().chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            s.push(c);
            run = false;
        } else if !run {
            s.push('-');
            run = true;
        }
    }
    let s = s.trim_matches('-');
    let cut = &s[..s.len().min(60)];
    if cut.is_empty() { "mod".into() } else { cut.to_owned() }
}

async fn unique_slug(state: &AppState, owner: ObjectId, title: &str) -> Result<String, ApiError> {
    let base = slugify(title);
    let mut candidate = base.clone();
    let mut n = 2;
    while project_one(state, doc! {"owner_id": owner, "slug": &candidate}).await?.is_some() {
        candidate = format!("{base}-{n}");
        n += 1;
    }
    Ok(candidate)
}

/// `_clean_tags(tags)`.
pub fn clean_tags(tags: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for t in tags {
        let low = kiwi_core::pystr::strip(t).to_lowercase();
        let mut collapsed = String::new();
        let mut space = false;
        for c in low.chars() {
            if kiwi_core::pystr::is_space(c) {
                if !space {
                    collapsed.push(' ');
                }
                space = true;
            } else {
                collapsed.push(c);
                space = false;
            }
        }
        let t = kiwi_core::pystr::prefix(&collapsed, 30).to_owned();
        if !t.is_empty() && !out.contains(&t) {
            out.push(t);
        }
    }
    out.truncate(12);
    out
}

/// `f"{n:,}"`.
pub fn thousands(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn label(base: &str) -> &str {
    match base {
        "readme_text" | "readme" => "README",
        other => other,
    }
}

/// `_clean_i18n_map(translations, base=, max_len=)`.
pub fn clean_i18n(translations: &I18n, base: &str, max_len: usize) -> Result<StrMap, ApiError> {
    let lab = label(base);
    let mut out: StrMap = Vec::new();
    for (lang, text) in translations {
        let lang = kiwi_core::pystr::strip(lang);
        if lang == "en" {
            return Err(ApiError::bad_request(format!("The English {lab} is `{base}`, not a translation.")));
        }
        if !CONTENT_LANGS.contains(&lang) {
            return Err(ApiError::bad_request(format!("'{lang}' isn't a language the site supports.")));
        }
        if text.chars().count() > max_len {
            return Err(ApiError::bad_request(format!(
                "The {lang} {lab} is too long (max {} characters).",
                thousands(max_len)
            )));
        }
        let text = kiwi_core::pystr::strip(text);
        if !text.is_empty() {
            match out.iter_mut().find(|(k, _)| k == lang) {
                Some(e) => e.1 = text.to_owned(),
                None => out.push((lang.to_owned(), text.to_owned())),
            }
        }
    }
    Ok(out)
}

fn url_ok(u: &str) -> bool {
    let low: String = u.chars().take(8).collect::<String>().to_ascii_lowercase();
    let rest = if low.starts_with("https://") {
        &u[8..]
    } else if low.starts_with("http://") {
        &u[7..]
    } else {
        return false;
    };
    !rest.is_empty() && !rest.chars().any(kiwi_core::pystr::is_space)
}

/// `_clean_url(u, field=)`.
pub fn clean_url(u: &str, field: &str) -> Result<Option<String>, ApiError> {
    let u = kiwi_core::pystr::strip(u);
    if u.is_empty() {
        return Ok(None);
    }
    if u.chars().count() > 300 || !url_ok(u) {
        return Err(ApiError::bad_request(format!("Invalid {field} URL - it must start with http:// or https://.")));
    }
    Ok(Some(u.to_owned()))
}

fn clean_donations(urls: &[String]) -> Result<Vec<String>, ApiError> {
    let mut cleaned: Vec<String> = Vec::new();
    for u in urls.iter().take(5) {
        if let Some(cu) = clean_url(u, "donation")?
            && !cleaned.contains(&cu)
        {
            cleaned.push(cu);
        }
    }
    Ok(cleaned)
}

/// `list(dict.fromkeys(b.strip() for b in items if b and b.strip()))`.
pub fn dedup_stripped(items: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for b in items {
        let s = kiwi_core::pystr::strip(b);
        if !s.is_empty() && !out.iter().any(|o| o == s) {
            out.push(s.to_owned());
        }
    }
    out
}

fn require_owner(p: &Project, actor: &SiteUser) -> Result<(), ApiError> {
    if can_edit(p, Some(actor)) {
        Ok(())
    } else {
        Err(ApiError::forbidden("You don't have edit access to this mod project."))
    }
}

fn require_primary_owner(p: &Project, actor: &SiteUser) -> Result<(), ApiError> {
    if is_primary_owner(p, Some(actor)) {
        Ok(())
    } else {
        Err(ApiError::forbidden("Only the mod's owner can do this."))
    }
}

fn owner_name(actor: &SiteUser) -> String {
    actor.display_name.clone().filter(|d| !d.is_empty()).unwrap_or_else(|| actor.username.clone())
}

type Credit = (String, String, String, String);

/// `_resolve_attribution(ref, viewer)`.
async fn resolve_attribution(state: &AppState, r: Option<&str>, viewer: &SiteUser) -> Result<Option<Credit>, ApiError> {
    let r = kiwi_core::pystr::strip(r.unwrap_or("")).trim_matches('/');
    if r.is_empty() {
        return Ok(None);
    }
    let Some((handle, slug)) = r.split_once('/') else {
        return Err(not_found(&format!("Credit a mod by '<handle>/<slug>' (got '{r}').")));
    };
    match get_project(state, handle, slug).await? {
        Some(o) if can_view(state, &o, Some(viewer)) => Ok(Some((o.slug, o.owner_handle, o.title, o.owner_username))),
        _ => Err(not_found(&format!("No mod '{r}' to credit."))),
    }
}

fn set_credit(p: &mut Project, c: Option<Credit>) {
    let (slug, handle, title, owner) = match c {
        Some((a, b, c, d)) => (Some(a), Some(b), Some(c), Some(d)),
        None => (None, None, None, None),
    };
    p.inspired_by_slug = slug;
    p.inspired_by_handle = handle;
    p.inspired_by_title = title;
    p.inspired_by_owner = owner;
}

async fn insert(state: &AppState, name: &str, d: Document) -> Result<(), ApiError> {
    coll(state, name).insert_one(d).await.map_err(internal)?;
    Ok(())
}

// --- projects ------------------------------------------------------------------------

/// `create_project(actor, ...)`.
pub async fn create_project(state: &AppState, actor: &SiteUser, req: CreateProject) -> Result<Project, ApiError> {
    let slug = unique_slug(state, actor.id, &req.title).await?;
    let (mut mode, mut inspired_by, mut author) = (req.mode, req.inspired_by, String::new());
    if req.on_behalf {
        author = kiwi_core::pystr::strip(req.credited_author.as_deref().unwrap_or("")).to_owned();
        if author.is_empty() {
            return Err(ApiError::bad_request("Name the creator this mod was made by."));
        }
        mode = "releases".into();
        inspired_by = None;
    }
    let credit = resolve_attribution(state, inspired_by.as_deref(), actor).await?;
    let mut p = Project::new(slug, kiwi_core::pystr::strip(&req.title).to_owned());
    p.summary = kiwi_core::pystr::strip(&req.summary).to_owned();
    p.description = req.description;
    p.tags = clean_tags(&req.tags);
    p.visibility = req.visibility;
    p.is_beta = req.is_beta;
    p.mode = mode;
    p.source_visibility = req.source_visibility;
    p.owner_id = Some(actor.id);
    p.owner_username = owner_name(actor);
    p.owner_handle = actor.username.clone();
    p.uploaded_on_behalf = req.on_behalf;
    p.author = author;
    set_credit(&mut p, credit);
    insert(state, model::PROJECTS, p.to_doc()).await?;
    git::ensure_repo(state, &p.id.to_hex()).await.map_err(internal)?;
    Ok(p)
}

/// `fork_project(actor, original)`.
pub async fn fork_project(state: &AppState, actor: &SiteUser, original: &mut Project) -> Result<Project, ApiError> {
    if !source_visible(state, original, Some(actor)) {
        return Err(ApiError::forbidden(
            "This mod's source is locked - you can credit it as inspiration instead of forking.",
        ));
    }
    let slug = unique_slug(state, actor.id, &original.title).await?;
    let mut fork = Project::new(slug, original.title.clone());
    fork.summary = original.summary.clone();
    fork.description = original.description.clone();
    fork.tags = original.tags.clone();
    fork.visibility = "draft".into();
    fork.mode = original.mode.clone();
    fork.owner_id = Some(actor.id);
    fork.owner_username = owner_name(actor);
    fork.owner_handle = actor.username.clone();
    fork.forked_from_id = Some(original.id);
    fork.forked_from_slug = Some(original.slug.clone());
    fork.forked_from_handle = Some(original.owner_handle.clone());
    fork.forked_from_title = Some(original.title.clone());
    fork.forked_from_owner = Some(original.owner_username.clone());
    fork.is_remix = true;
    fork.remix_status = "pending".into();
    insert(state, model::PROJECTS, fork.to_doc()).await?;
    let commit = git::ForkCommit {
        author: git::author_ident(&fork.owner_username),
        message: format!("Fork of {}", original.slug),
        ts: dt::now().timestamp(),
    };
    git::fork(state, &original.id.to_hex(), &fork.id.to_hex(), &original.default_branch, &fork.default_branch, &commit)
        .await
        .map_err(internal)?;
    original.fork_count += 1;
    save_project(state, original).await?;
    Ok(fork)
}

fn set_i18n(target: &mut StrMap, given: &Option<I18n>, base: &str, max_len: usize) -> Result<(), ApiError> {
    if let Some(t) = given {
        *target = clean_i18n(t, base, max_len)?;
    }
    Ok(())
}

/// `update_project(project, actor, **fields)`.
pub async fn update_project(
    state: &AppState,
    mut p: Project,
    actor: &SiteUser,
    req: UpdateProject,
) -> Result<Project, ApiError> {
    require_owner(&p, actor)?;
    let was_public = p.visibility == "public";
    let was_hidden: Vec<String> = p.hidden_release_branches.clone();
    let strip = |s: &str| kiwi_core::pystr::strip(s).to_owned();
    if let Some(v) = &req.title {
        p.title = strip(v);
    }
    if let Some(v) = &req.summary {
        p.summary = strip(v);
    }
    if let Some(v) = req.description {
        p.description = v;
    }
    if let Some(v) = req.readme_text {
        p.readme_text = v;
    }
    if let Some(v) = req.warnings {
        p.warnings = v;
    }
    set_i18n(&mut p.title_i18n, &req.title_i18n, "title", 120)?;
    set_i18n(&mut p.summary_i18n, &req.summary_i18n, "summary", 280)?;
    set_i18n(&mut p.description_i18n, &req.description_i18n, "description", 40_000)?;
    set_i18n(&mut p.readme_i18n, &req.readme_i18n, "readme_text", 60_000)?;
    set_i18n(&mut p.warnings_i18n, &req.warnings_i18n, "warnings", 4_000)?;
    if let Some(t) = &req.tags {
        p.tags = clean_tags(t);
    }
    if let Some(v) = req.visibility {
        p.visibility = v;
    }
    if let Some(v) = req.is_beta {
        p.is_beta = v;
    }
    if let Some(v) = req.issues_enabled {
        p.issues_enabled = v;
    }
    if let Some(mode) = req.mode {
        if p.uploaded_on_behalf && mode != "releases" {
            return Err(ApiError::bad_request("A mod uploaded on someone's behalf stays releases-only."));
        }
        p.mode = mode;
    }
    if let Some(v) = req.source_visibility {
        p.source_visibility = v;
    }
    if let Some(v) = &req.hidden_release_branches {
        p.hidden_release_branches = dedup_stripped(v);
    }
    if let Some(v) = &req.branch_order {
        p.branch_order = dedup_stripped(v);
    }
    if let Some(v) = &req.discord_url {
        p.discord_url = clean_url(v, "Discord invite")?;
    }
    if let Some(v) = &req.website_url {
        p.website_url = clean_url(v, "website")?;
    }
    if let Some(v) = &req.donation_urls {
        p.donation_urls = clean_donations(v)?;
    }
    if let Some(v) = &req.inspired_by {
        let credit = resolve_attribution(state, Some(v), actor).await?;
        set_credit(&mut p, credit);
    }
    if p.uploaded_on_behalf {
        p.discord_url = None;
        p.website_url = None;
        p.donation_urls = Vec::new();
        set_credit(&mut p, None);
        p.mode = "releases".into();
    }
    if p.is_remix && p.remix_requested_at.is_none() && matches!(p.visibility.as_str(), "public" | "public_website_only")
    {
        p.remix_requested_at = Some(dt::now());
    }
    p.owner_handle = actor.username.clone();
    p.updated_at = dt::now();
    save_project(state, &mut p).await?;
    if !was_public && p.visibility == "public" {
        announce::announce_newly_visible(state, &p, None).await?;
    } else if p.visibility == "public" {
        let now_hidden: HashSet<&String> = p.hidden_release_branches.iter().collect();
        let mut revealed: Vec<String> = was_hidden
            .iter()
            .filter(|b| !now_hidden.contains(b))
            .cloned()
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        revealed.sort();
        if !revealed.is_empty() {
            announce::announce_newly_visible(state, &p, Some(&revealed)).await?;
        }
    }
    Ok(p)
}

fn store(state: &AppState) -> ContentStore {
    ContentStore::new(&state.settings().mods_store_dir)
}

/// `_remember_takedown(project)`.
async fn remember_takedown(state: &AppState, p: &Project) -> Result<(), ApiError> {
    let rels = super::service::releases(state, doc! {"project_id": p.id}, doc! {}).await?;
    let arts = coll(state, model::TAKEN_DOWN);
    for r in rels {
        let mut seen: Vec<String> = Vec::new();
        for sha in std::iter::once(r.tmod_sha).chain(r.prior_tmod_shas) {
            if sha.is_empty() || seen.contains(&sha) {
                continue;
            }
            arts.update_one(
                doc! {"sha256": &sha},
                doc! {"$set": {"project_id": p.id, "title": &p.title, "owner_handle": &p.owner_handle},
                "$setOnInsert": {"created_at": model::date(&dt::now())}},
            )
            .upsert(true)
            .await
            .map_err(internal)?;
            seen.push(sha);
        }
    }
    fill_takedown_md5s(state).await;
    Ok(())
}

/// `_fill_takedown_md5s()`: never fails.
async fn fill_takedown_md5s(state: &AppState) {
    let arts = coll(state, model::TAKEN_DOWN);
    let Ok(rows) = super::service::find_docs(&arts, doc! {"md5": null}, None, None, None).await else { return };
    let cas = store(state);
    for row in rows {
        let r = model::Rd(&row);
        let sha = r.s("sha256", "");
        let cas = cas.clone();
        let data = tokio::task::spawn_blocking(move || cas.get(&sha)).await;
        let md5 = match data {
            Ok(Ok(Some(d))) if !d.is_empty() => hex::encode(Md5::digest(&d)),
            Ok(Ok(_)) => String::new(),
            _ => continue,
        };
        if let Err(e) = arts.update_one(doc! {"_id": r.id()}, doc! {"$set": {"md5": md5}}).await {
            tracing::warn!("mods_hub: md5 of taken-down artifact failed: {e}");
        }
    }
}

/// `delete_project(project, actor)` (`_purge_project`).
pub async fn delete_project(state: &AppState, p: &Project, actor: &SiteUser) -> Result<(), ApiError> {
    require_primary_owner(p, actor)?;
    if p.taken_down {
        remember_takedown(state, p).await?;
    }
    let del = |name: &'static str, filter: Document| async move {
        coll(state, name).delete_many(filter).await.map_err(internal).map(|_| ())
    };
    del(model::RELEASES, doc! {"project_id": p.id}).await?;
    del(model::REPORTS, doc! {"target_type": "mod", "target_id": p.id}).await?;
    del(model::STARS, doc! {"project_id": p.id}).await?;
    del(model::ISSUE_EVENTS, doc! {"project_id": p.id}).await?;
    del(model::ISSUES, doc! {"project_id": p.id}).await?;
    git::delete_repo(state, &p.id.to_hex()).await;
    coll(state, model::PROJECTS).delete_one(doc! {"_id": p.id}).await.map_err(internal)?;
    Ok(())
}

// --- remixes -------------------------------------------------------------------------

pub async fn list_remixes_for_owner(
    state: &AppState,
    original: &Project,
    actor: &SiteUser,
) -> Result<Vec<Value>, ApiError> {
    require_owner(original, actor)?;
    let docs = super::service::projects(
        state,
        doc! {"forked_from_id": original.id},
        Some(doc! {"updated_at": -1}),
        None,
        None,
    )
    .await?;
    let now = dt::now();
    Ok(docs.iter().filter(|p| p.is_remix).map(|p| Value::Object(remix_card(p, now))).collect())
}

pub async fn set_remix_status(
    state: &AppState,
    original: &Project,
    actor: &SiteUser,
    remix_id: &str,
    status: &str,
) -> Result<Value, ApiError> {
    require_owner(original, actor)?;
    if !matches!(status, "approved" | "denied" | "pending") {
        return Err(ApiError::bad_request("Bad status."));
    }
    let remix = match ObjectId::parse_str(remix_id) {
        Ok(oid) => project_one(state, doc! {"_id": oid}).await?,
        Err(_) => None,
    };
    let Some(mut remix) = remix.filter(|r| r.forked_from_id == Some(original.id) && r.is_remix) else {
        return Err(not_found("Remix not found"));
    };
    remix.remix_status = status.to_owned();
    remix.remix_resolved_at = Some(dt::now());
    remix.remix_resolved_by = Some(actor.id);
    save_project(state, &mut remix).await?;
    Ok(Value::Object(remix_card(&remix, dt::now())))
}

fn with_already(mut m: Map<String, Value>, already: bool) -> Value {
    m.insert("already".into(), already.into());
    Value::Object(m)
}

pub async fn create_remix_claim(
    state: &AppState,
    actor: &SiteUser,
    original: &Project,
    target: &Project,
    message: &str,
) -> Result<Value, ApiError> {
    require_owner(original, actor)?;
    if target.id == original.id {
        return Err(ApiError::bad_request("A mod can't reference itself."));
    }
    let c = coll(state, model::REMIX_CLAIMS);
    if let Some(d) = c.find_one(doc! {"target_id": target.id, "status": "pending"}).await.map_err(internal)? {
        return Ok(with_already(remix_claim_dto(&RemixClaim::from_doc(&d)), true));
    }
    let claim = RemixClaim {
        id: ObjectId::new(),
        target_id: target.id,
        target_slug: target.slug.clone(),
        target_title: target.title.clone(),
        original_id: original.id,
        original_slug: original.slug.clone(),
        original_title: original.title.clone(),
        claimant_id: actor.id,
        claimant_username: actor.username.clone(),
        message: kiwi_core::pystr::prefix(message, 2000).to_owned(),
        status: "pending".into(),
        resolved_by: None,
        resolved_at: None,
        created_at: dt::now(),
    };
    insert(state, model::REMIX_CLAIMS, claim.to_doc()).await?;
    Ok(with_already(remix_claim_dto(&claim), false))
}

// --- stars ---------------------------------------------------------------------------

pub async fn star_project(state: &AppState, actor: &SiteUser, p: &mut Project) -> Result<Value, ApiError> {
    let stars = coll(state, model::STARS);
    if stars.find_one(doc! {"project_id": p.id, "site_user_id": actor.id}).await.map_err(internal)?.is_none() {
        match stars.insert_one(model::star_doc(ObjectId::new(), p.id, actor.id, dt::now())).await {
            Err(e) if is_duplicate(&e) => {}
            Err(e) => return Err(internal(e)),
            Ok(_) => {
                coll(state, model::PROJECTS)
                    .update_one(doc! {"_id": p.id}, doc! {"$inc": {"star_count": 1}})
                    .await
                    .map_err(internal)?;
                p.star_count += 1;
            }
        }
    }
    Ok(serde_json::json!({"starred": true, "star_count": p.star_count}))
}

pub async fn unstar_project(state: &AppState, actor: &SiteUser, p: &mut Project) -> Result<Value, ApiError> {
    let stars = coll(state, model::STARS);
    if let Some(star) = stars.find_one(doc! {"project_id": p.id, "site_user_id": actor.id}).await.map_err(internal)? {
        stars.delete_one(doc! {"_id": model::Rd(&star).id()}).await.map_err(internal)?;
        coll(state, model::PROJECTS)
            .update_one(doc! {"_id": p.id}, doc! {"$inc": {"star_count": -1}})
            .await
            .map_err(internal)?;
        p.star_count = (p.star_count - 1).max(0);
    }
    Ok(serde_json::json!({"starred": false, "star_count": p.star_count}))
}

// --- collaborators -------------------------------------------------------------------

pub async fn add_collaborator(
    state: &AppState,
    mut p: Project,
    actor: &SiteUser,
    username: &str,
) -> Result<Value, ApiError> {
    require_primary_owner(&p, actor)?;
    let uname = kiwi_core::pystr::strip(username).trim_start_matches('@').to_lowercase();
    if uname.is_empty() {
        return Err(ApiError::bad_request("Enter a username to collaborate with."));
    }
    let user = super::service::users(state)
        .find_one(doc! {"username": &uname})
        .await
        .map_err(internal)?
        .ok_or_else(|| not_found(&format!("No site user '@{uname}'. They must sign in once first.")))?;
    if Some(user.id) == p.owner_id {
        return Err(ApiError::bad_request("That's the owner."));
    }
    if p.collaborators.iter().any(|c| c.user_id == user.id) {
        return project_detail(state, &p, Some(actor)).await;
    }
    if p.collaborators.len() >= 20 {
        return Err(ApiError::bad_request("Too many collaborators (max 20)."));
    }
    p.collaborators.push(Collaborator { user_id: user.id, username: user.username.clone() });
    p.updated_at = dt::now();
    save_project(state, &mut p).await?;
    project_detail(state, &p, Some(actor)).await
}

pub async fn remove_collaborator(
    state: &AppState,
    mut p: Project,
    actor: &SiteUser,
    user_id: &str,
) -> Result<Value, ApiError> {
    require_primary_owner(&p, actor)?;
    let uid = ObjectId::parse_str(user_id).ok();
    p.collaborators.retain(|c| Some(c.user_id) != uid);
    p.updated_at = dt::now();
    save_project(state, &mut p).await?;
    project_detail(state, &p, Some(actor)).await
}

// --- claims --------------------------------------------------------------------------

pub async fn create_claim(state: &AppState, p: &Project, actor: &SiteUser, message: &str) -> Result<Value, ApiError> {
    if !p.is_stray {
        return Err(ApiError::bad_request("This mod isn't available to claim."));
    }
    let c = coll(state, model::CLAIMS);
    if let Some(d) =
        c.find_one(doc! {"project_id": p.id, "claimant_id": actor.id, "status": "pending"}).await.map_err(internal)?
    {
        return Ok(with_already(dto::claim_dto(&Claim::from_doc(&d)), true));
    }
    let claim = Claim {
        id: ObjectId::new(),
        project_id: p.id,
        project_slug: p.slug.clone(),
        project_title: p.title.clone(),
        claimant_id: actor.id,
        claimant_username: actor.username.clone(),
        message: kiwi_core::pystr::prefix(message, 2000).to_owned(),
        status: "pending".into(),
        resolved_by: None,
        resolved_at: None,
        created_at: dt::now(),
    };
    insert(state, model::CLAIMS, claim.to_doc()).await?;
    Ok(with_already(dto::claim_dto(&claim), false))
}

// --- images --------------------------------------------------------------------------

fn be32(d: &[u8], i: usize) -> Option<i64> {
    Some(i64::from(u32::from_be_bytes(d.get(i..i + 4)?.try_into().ok()?)))
}

fn le16(d: &[u8], i: usize) -> Option<i64> {
    Some(i64::from(u16::from_le_bytes(d.get(i..i + 2)?.try_into().ok()?)))
}

fn webp_dims(d: &[u8]) -> (Option<i64>, Option<i64>) {
    let dims = || -> Option<(i64, i64)> {
        match d.get(12..16)? {
            b"VP8 " => Some((le16(d, 26)? & 0x3FFF, le16(d, 28)? & 0x3FFF)),
            b"VP8L" => {
                let b = d.get(21..25)?;
                let bits = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
                Some((i64::from(bits & 0x3FFF) + 1, i64::from((bits >> 14) & 0x3FFF) + 1))
            }
            b"VP8X" => {
                let w = u32::from_le_bytes([*d.get(24)?, *d.get(25)?, *d.get(26)?, 0]);
                let h = u32::from_le_bytes([*d.get(27)?, *d.get(28)?, *d.get(29)?, 0]);
                Some((i64::from(w) + 1, i64::from(h) + 1))
            }
            _ => None,
        }
    };
    dims().map_or((None, None), |(w, h)| (Some(w), Some(h)))
}

fn jpeg_dims(d: &[u8]) -> (Option<i64>, Option<i64>) {
    let (mut i, n) = (2usize, d.len());
    while i + 9 < n {
        if d[i] != 0xFF {
            i += 1;
            continue;
        }
        let marker = d[i + 1];
        if (0xC0..=0xCF).contains(&marker) && !matches!(marker, 0xC4 | 0xC8 | 0xCC) {
            let h = u16::from_be_bytes([d[i + 5], d[i + 6]]);
            let w = u16::from_be_bytes([d[i + 7], d[i + 8]]);
            return (Some(i64::from(w)), Some(i64::from(h)));
        }
        if matches!(marker, 0xD0..=0xD9) {
            i += 2;
            continue;
        }
        let seg = u16::from_be_bytes([d[i + 2], d[i + 3]]);
        i += 2 + usize::from(seg);
    }
    (None, None)
}

/// `store.sniff_image(data)`.
pub fn sniff_image(d: &[u8]) -> Option<(&'static str, Option<i64>, Option<i64>)> {
    if d.len() < 24 {
        return None;
    }
    if d.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some(("image/png", be32(d, 16), be32(d, 20)));
    }
    if d.starts_with(b"GIF87a") || d.starts_with(b"GIF89a") {
        return Some(("image/gif", le16(d, 6), le16(d, 8)));
    }
    if d.starts_with(b"RIFF") && &d[8..12] == b"WEBP" {
        let (w, h) = webp_dims(d);
        return Some(("image/webp", w, h));
    }
    if d.starts_with(b"\xff\xd8") {
        let (w, h) = jpeg_dims(d);
        return Some(("image/jpeg", w, h));
    }
    None
}

/// `store_image(actor, data, ct)`: the asset's sha.
pub async fn store_image(state: &AppState, actor: &SiteUser, data: Vec<u8>) -> Result<String, ApiError> {
    let max = state.settings().mods_image_max_bytes;
    if data.len() as i64 > max {
        return Err(ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            crate::error::ErrorCode::BadRequest,
            format!("Image exceeds the {max}-byte limit."),
        ));
    }
    let (content_type, width, height) =
        sniff_image(&data).ok_or_else(|| ApiError::bad_request("Unsupported image - use PNG, JPEG, WebP or GIF."))?;
    let size = data.len() as i64;
    let cas = store(state);
    let (sha, _) = tokio::task::spawn_blocking(move || cas.put(&data)).await.map_err(internal)?.map_err(internal)?;
    let images = coll(state, model::IMAGES);
    if images.find_one(doc! {"sha": &sha}).await.map_err(internal)?.is_some() {
        return Ok(sha);
    }
    let asset = ImageAsset {
        id: ObjectId::new(),
        sha: sha.clone(),
        content_type: content_type.into(),
        byte_size: size,
        owner_id: actor.id,
        width,
        height,
        created_at: dt::now(),
    };
    insert(state, model::IMAGES, asset.to_doc()).await?;
    Ok(sha)
}

async fn image_exists(state: &AppState, sha: &str) -> Result<bool, ApiError> {
    Ok(coll(state, model::IMAGES).find_one(doc! {"sha": sha}).await.map_err(internal)?.is_some())
}

pub async fn set_banner(state: &AppState, p: &mut Project, actor: &SiteUser, sha: &str) -> Result<(), ApiError> {
    require_owner(p, actor)?;
    if !image_exists(state, sha).await? {
        return Err(not_found("No such uploaded image"));
    }
    p.banner_sha = Some(sha.to_owned());
    p.updated_at = dt::now();
    save_project(state, p).await
}

pub async fn clear_banner(state: &AppState, p: &mut Project, actor: &SiteUser) -> Result<(), ApiError> {
    require_owner(p, actor)?;
    if p.banner_sha.is_some() {
        p.banner_sha = None;
        p.updated_at = dt::now();
        save_project(state, p).await?;
    }
    Ok(())
}

pub async fn add_preview(state: &AppState, p: &mut Project, actor: &SiteUser, sha: &str) -> Result<(), ApiError> {
    require_owner(p, actor)?;
    if !image_exists(state, sha).await? {
        return Err(not_found("No such uploaded image"));
    }
    if !p.preview_shas.iter().any(|s| s == sha) {
        if p.preview_shas.len() >= 12 {
            return Err(ApiError::bad_request("Too many preview images (max 12)."));
        }
        p.preview_shas.push(sha.to_owned());
        p.updated_at = dt::now();
        save_project(state, p).await?;
    }
    Ok(())
}

pub async fn remove_preview(state: &AppState, p: &mut Project, actor: &SiteUser, sha: &str) -> Result<(), ApiError> {
    require_owner(p, actor)?;
    if let Some(i) = p.preview_shas.iter().position(|s| s == sha) {
        p.preview_shas.remove(i);
        p.updated_at = dt::now();
        save_project(state, p).await?;
    }
    Ok(())
}

// --- profiles ------------------------------------------------------------------------

async fn get_or_make_profile(state: &AppState, actor: &SiteUser) -> Result<Profile, ApiError> {
    if let Some(p) = profile_of(state, actor.id).await? {
        return Ok(p);
    }
    let p = Profile::new(actor.id, actor.username.clone());
    insert(state, model::PROFILES, p.to_doc()).await?;
    Ok(p)
}

async fn save_profile(state: &AppState, p: &mut Profile) -> Result<(), ApiError> {
    let stored = save_doc(state, model::PROFILES, p.id, p.to_doc()).await?;
    *p = Profile::from_doc(&stored);
    Ok(())
}

/// `profile_view(actor.username, actor)` as a `-> dict` route returns it: a
/// missing profile fails FastAPI's response validation (a 500).
async fn view_after_write(state: &AppState, actor: &SiteUser) -> Result<Value, ApiError> {
    profile_view(state, &actor.username, Some(actor)).await?.ok_or_else(ApiError::internal)
}

pub async fn update_profile(state: &AppState, actor: &SiteUser, req: UpdateProfile) -> Result<Value, ApiError> {
    let mut p = get_or_make_profile(state, actor).await?;
    if let Some(order) = &req.mod_order {
        p.mod_order = dedup_stripped(order);
    }
    if let Some(fs) = &req.featured_slug {
        let fs = kiwi_core::pystr::strip(fs);
        let owns = if fs.is_empty() {
            false
        } else {
            project_one(state, doc! {"owner_id": actor.id, "slug": fs}).await?.is_some()
        };
        p.featured_slug = owns.then(|| fs.to_owned());
    }
    if let Some(v) = &req.display_name {
        p.display_name = kiwi_core::pystr::prefix(kiwi_core::pystr::strip(v), 80).to_owned();
    }
    if let Some(v) = &req.tagline {
        p.tagline = kiwi_core::pystr::prefix(kiwi_core::pystr::strip(v), 160).to_owned();
    }
    if let Some(v) = req.readme {
        p.readme = v;
    }
    set_i18n(&mut p.tagline_i18n, &req.tagline_i18n, "tagline", 160)?;
    set_i18n(&mut p.readme_i18n, &req.readme_i18n, "readme", 40_000)?;
    if let Some(v) = &req.discord_url {
        p.discord_url = clean_url(v, "Discord")?;
    }
    if let Some(v) = &req.website_url {
        p.website_url = clean_url(v, "website")?;
    }
    if let Some(v) = &req.donation_urls {
        p.donation_urls = clean_donations(v)?;
    }
    p.handle = actor.username.clone();
    p.updated_at = dt::now();
    save_profile(state, &mut p).await?;
    view_after_write(state, actor).await
}

pub async fn set_profile_image(state: &AppState, actor: &SiteUser, sha: &str, banner: bool) -> Result<Value, ApiError> {
    if !image_exists(state, sha).await? {
        return Err(not_found("No such uploaded image"));
    }
    let mut p = get_or_make_profile(state, actor).await?;
    if banner {
        p.banner_sha = Some(sha.to_owned());
    } else {
        p.avatar_sha = Some(sha.to_owned());
    }
    p.handle = actor.username.clone();
    p.updated_at = dt::now();
    save_profile(state, &mut p).await?;
    view_after_write(state, actor).await
}

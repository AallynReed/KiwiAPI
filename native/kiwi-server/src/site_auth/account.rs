//! `app/site_auth/account.py`: GDPR export, and self-delete as an
//! anonymize-in-place of the account and its personal records.

use axum::{
    extract::State,
    http::{HeaderValue, header},
    response::{IntoResponse, Response},
};
use futures::TryStreamExt;
use kiwi_core::pystr;
use mongodb::bson::{Bson, Document, doc, oid::ObjectId};
use serde_json::{Value, json};

use super::{
    oauth::clear_discord_token,
    routes::clear_claim,
    store::{USERNAME_REQUESTS, coll, internal, save_user, token_hex, username_taken},
};
use crate::{
    auth::{
        CurrentSiteUser,
        models::{SITE_SESSIONS, SiteUser},
    },
    dt,
    error::ApiError,
    pyjson,
    state::AppState,
    validation::{DepsThenBody, Field, FromBody},
};

pub const MOD_PROJECTS: &str = "mod_projects";
pub const MODPACK_PROJECTS: &str = "modpack_projects";
pub const MOD_PROFILES: &str = "mod_profiles";
pub const MOD_STARS: &str = "mod_stars";
pub const MODPACK_STARS: &str = "modpack_stars";
pub const GIVEAWAYS: &str = "giveaways";
pub const GIVEAWAY_ENTRIES: &str = "giveaway_entries";
pub const DM_SUBSCRIPTIONS: &str = "dm_subscriptions";
pub const SITE_WEBHOOKS: &str = "site_webhooks";
pub const IMAGE_DESIGNS: &str = "image_designs";
pub const MOD_GIT_TOKENS: &str = "mod_git_tokens";
pub const MOD_CLAIM_REQUESTS: &str = "mod_claim_requests";
pub const EMAIL_OUTBOX: &str = "email_outbox";

async fn find_all(state: &AppState, name: &str, filter: Document) -> Result<Vec<Document>, ApiError> {
    coll(state, name)?.find(filter).await.map_err(internal)?.try_collect().await.map_err(internal)
}

fn s(d: &Document, key: &str, default: &str) -> Value {
    match d.get(key) {
        Some(Bson::String(v)) => Value::String(v.clone()),
        Some(Bson::Null) => Value::Null,
        _ => Value::String(default.to_owned()),
    }
}

fn opt_s(d: &Document, key: &str) -> Value {
    match d.get(key) {
        Some(Bson::String(v)) => Value::String(v.clone()),
        _ => Value::Null,
    }
}

fn b(d: &Document, key: &str) -> Value {
    Value::Bool(d.get_bool(key).unwrap_or(false))
}

fn id(d: &Document, key: &str) -> Value {
    d.get_object_id(key).map_or(Value::Null, |o| Value::String(o.to_hex()))
}

/// `jsonable_encoder` of a datetime field (`default_factory=utcnow` when absent).
fn when(d: &Document, key: &str, now: &str) -> Value {
    match d.get(key) {
        Some(Bson::DateTime(t)) => Value::String(dt::isoformat(&dt::from_bson(*t))),
        None => Value::String(now.to_owned()),
        _ => Value::Null,
    }
}

fn iso_opt(t: Option<mongodb::bson::DateTime>) -> Value {
    t.map_or(Value::Null, |t| Value::String(dt::isoformat(&dt::from_bson(t))))
}

/// The export document (`data` before `jsonable_encoder`).
async fn export_data(state: &AppState, user: &SiteUser) -> Result<Value, ApiError> {
    let uid = user.id;
    let now = dt::isoformat(&dt::now());
    let mine = doc! { "site_user_id": uid };
    let owned = doc! { "owner_id": uid };
    let owned_mods = find_all(state, MOD_PROJECTS, owned.clone()).await?;
    let collab_mods = find_all(state, MOD_PROJECTS, doc! { "collaborators.user_id": uid }).await?;
    let owned_packs = find_all(state, MODPACK_PROJECTS, owned.clone()).await?;
    let profile = coll(state, MOD_PROFILES)?.find_one(mine.clone()).await.map_err(internal)?;
    let stars = find_all(state, MOD_STARS, mine.clone()).await?;
    let pack_stars = find_all(state, MODPACK_STARS, mine.clone()).await?;
    let entries = find_all(state, GIVEAWAY_ENTRIES, mine.clone()).await?;
    let dm_subs = find_all(state, DM_SUBSCRIPTIONS, owned.clone()).await?;
    let webhooks = find_all(state, SITE_WEBHOOKS, owned.clone()).await?;
    let images = find_all(state, IMAGE_DESIGNS, owned).await?;
    let git_tokens = find_all(state, MOD_GIT_TOKENS, mine.clone()).await?;
    let sessions = find_all(state, SITE_SESSIONS, mine.clone()).await?;
    let name_reqs = find_all(state, USERNAME_REQUESTS, mine).await?;

    let project = |m: &Document| {
        json!({"slug": s(m, "slug", ""), "title": s(m, "title", ""), "visibility": s(m, "visibility", "draft"),
               "created_at": when(m, "created_at", &now)})
    };
    let labelled = |d: &Document| json!({"id": id(d, "_id"), "label": s(d, "label", "")});
    Ok(json!({
        "exported_at": now,
        "account": {
            "id": uid.to_hex(), "username": user.username, "discord_handle": user.discord_handle,
            "display_name": user.display_name, "notify_email": user.notify_email,
            "discord_id": user.discord_id, "is_verified": user.is_verified,
            "claimed_trove_name": user.claimed_trove_display, "claim_verified": user.claim_verified,
            "created_at": dt::isoformat(&dt::from_bson(user.created_at)), "last_login_at": iso_opt(user.last_login_at),
        },
        "mods_owned": owned_mods.iter().map(project).collect::<Vec<_>>(),
        "mods_collaborating": collab_mods.iter()
            .map(|m| json!({"slug": s(m, "slug", ""), "title": s(m, "title", ""), "owner_handle": s(m, "owner_handle", "")}))
            .collect::<Vec<_>>(),
        "modpacks_owned": owned_packs.iter().map(project).collect::<Vec<_>>(),
        "profile": profile.map_or(Value::Null, |p| json!({
            "handle": s(&p, "handle", ""), "display_name": s(&p, "display_name", ""),
            "tagline": s(&p, "tagline", ""), "readme": s(&p, "readme", ""),
        })),
        "starred_mods": stars.iter()
            .map(|d| json!({"project_id": id(d, "project_id"), "at": when(d, "created_at", &now)})).collect::<Vec<_>>(),
        "starred_modpacks": pack_stars.iter()
            .map(|d| json!({"modpack_id": id(d, "modpack_id"), "at": when(d, "created_at", &now)})).collect::<Vec<_>>(),
        "giveaway_entries": entries.iter()
            .map(|d| json!({"giveaway_id": id(d, "giveaway_id"), "at": when(d, "entered_at", &now)})).collect::<Vec<_>>(),
        "dm_subscriptions": dm_subs.iter().map(labelled).collect::<Vec<_>>(),
        "webhooks": webhooks.iter().map(labelled).collect::<Vec<_>>(),
        "image_designs": images.iter()
            .map(|d| json!({"id": id(d, "_id"), "created_at": when(d, "created_at", &now)})).collect::<Vec<_>>(),
        "git_tokens": git_tokens.iter()
            .map(|d| json!({"name": s(d, "name", ""), "prefix": s(d, "prefix", ""), "revoked": b(d, "revoked"),
                            "created_at": when(d, "created_at", &now)}))
            .collect::<Vec<_>>(),
        "sessions": sessions.iter()
            .map(|d| json!({"device": opt_s(d, "device"), "created_at": when(d, "created_at", &now),
                            "last_used_at": when(d, "last_used_at", &now), "expires_at": when(d, "expires_at", &now)}))
            .collect::<Vec<_>>(),
        "username_change_requests": name_reqs.iter()
            .map(|d| json!({"requested_username": s(d, "requested_username", ""), "status": s(d, "status", "pending"),
                            "created_at": when(d, "created_at", &now)}))
            .collect::<Vec<_>>(),
    }))
}

/// `GET /v1/site-auth/me/export`.
pub async fn export(
    State(state): State<AppState>,
    CurrentSiteUser(user): CurrentSiteUser,
) -> Result<Response, ApiError> {
    let data = export_data(&state, &user).await?;
    let mut res = pyjson::PyJson(data).into_response();
    res.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_static("attachment; filename=\"better-trove-tools-export.json\""),
    );
    Ok(res)
}

pub struct DeleteBody {
    pub confirm_username: String,
}

impl FromBody for DeleteBody {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| Some(DeleteBody { confirm_username: m.field("confirm_username").str().required() })).required()
    }
}

/// `_unique_anon_username`.
async fn anon_username(state: &AppState) -> Result<String, ApiError> {
    for _ in 0..12 {
        let candidate = format!("deleted_{}", token_hex(8));
        if username_taken(state, &candidate).await?.is_none() {
            return Ok(candidate);
        }
    }
    Ok(format!("deleted_{}", token_hex(8)))
}

async fn delete_many(state: &AppState, name: &str, filter: Document) -> Result<(), ApiError> {
    coll(state, name)?.delete_many(filter).await.map_err(internal)?;
    Ok(())
}

async fn update_many(state: &AppState, name: &str, filter: Document, update: Document) -> Result<(), ApiError> {
    coll(state, name)?.update_many(filter, update).await.map_err(internal)?;
    Ok(())
}

/// Delete the user's rows of `name` after decrementing `counter` on each one's parent.
async fn drop_counted(
    state: &AppState,
    name: &str,
    uid: ObjectId,
    (parent_key, parents, counter): (&str, &str, &str),
) -> Result<(), ApiError> {
    for row in find_all(state, name, doc! { "site_user_id": uid }).await? {
        let parent = row.get(parent_key).cloned().unwrap_or(Bson::Null);
        update_many(state, parents, doc! { "_id": parent }, doc! { "$inc": { counter: -1 } }).await?;
    }
    delete_many(state, name, doc! { "site_user_id": uid }).await
}

/// A model field's default as Beanie's `save()` fills it in when absent.
#[derive(Clone, Copy)]
enum Fill {
    Str(&'static str),
    Int,
    Float,
    Bool(bool),
    Null,
    Dict,
    List,
    Now,
}

use Fill::{Bool, Dict, Float, Int, List, Now, Null, Str};

const MOD_PROJECT_DEFAULTS: &[(&str, Fill)] = &[
    ("summary", Str("")),
    ("description", Str("")),
    ("readme_text", Str("")),
    ("warnings", Str("")),
    ("title_i18n", Dict),
    ("summary_i18n", Dict),
    ("description_i18n", Dict),
    ("readme_i18n", Dict),
    ("warnings_i18n", Dict),
    ("tags", List),
    ("owner_id", Null),
    ("owner_username", Str("")),
    ("owner_handle", Str("")),
    ("collaborators", List),
    ("is_stray", Bool(false)),
    ("stray_status", Null),
    ("author", Str("")),
    ("source", Null),
    ("source_id", Null),
    ("source_url", Null),
    ("source_author_id", Null),
    ("source_file_id", Null),
    ("source_likes", Int),
    ("uploaded_on_behalf", Bool(false)),
    ("issues_enabled", Bool(true)),
    ("issue_seq", Int),
    ("open_issue_count", Int),
    ("visibility", Str("draft")),
    ("is_beta", Bool(false)),
    ("mode", Str("files")),
    ("source_visibility", Str("public")),
    ("default_branch", Str("main")),
    ("banner_sha", Null),
    ("preview_shas", List),
    ("discord_url", Null),
    ("website_url", Null),
    ("donation_urls", List),
    ("download_count", Int),
    ("star_count", Int),
    ("downloads_7d", Int),
    ("popularity_score", Float),
    ("hidden_release_branches", List),
    ("branch_order", List),
    ("forked_from_id", Null),
    ("forked_from_slug", Null),
    ("forked_from_handle", Null),
    ("forked_from_title", Null),
    ("forked_from_owner", Null),
    ("inspired_by_slug", Null),
    ("inspired_by_handle", Null),
    ("inspired_by_title", Null),
    ("inspired_by_owner", Null),
    ("fork_count", Int),
    ("is_remix", Bool(false)),
    ("remix_status", Str("pending")),
    ("remix_requested_at", Null),
    ("remix_resolved_at", Null),
    ("remix_resolved_by", Null),
    ("taken_down", Bool(false)),
    ("takedown_reason", Null),
    ("created_at", Now),
    ("updated_at", Now),
    ("last_release_at", Null),
];

const MODPACK_DEFAULTS: &[(&str, Fill)] = &[
    ("summary", Str("")),
    ("description", Str("")),
    ("warnings", Str("")),
    ("tags", List),
    ("owner_handle", Str("")),
    ("collaborators", List),
    ("visibility", Str("draft")),
    ("banner_sha", Null),
    ("preview_shas", List),
    ("discord_url", Null),
    ("website_url", Null),
    ("donation_urls", List),
    ("download_count", Int),
    ("star_count", Int),
    ("variants", List),
    ("default_variant", Str("default")),
    ("taken_down", Bool(false)),
    ("takedown_reason", Null),
    ("created_at", Now),
    ("updated_at", Now),
];

impl Fill {
    fn bson(self) -> Bson {
        match self {
            Str(s) => Bson::String(s.to_owned()),
            Int => Bson::Int32(0),
            Float => Bson::Double(0.0),
            Bool(b) => Bson::Boolean(b),
            Null => Bson::Null,
            Dict => Bson::Document(Document::new()),
            List => Bson::Array(Vec::new()),
            Now => super::store::date(dt::now()),
        }
    }
}

/// Take the user out of every other project's `collaborators`, filling the
/// top-level fields Beanie's `save()` would add to an older document.
async fn leave_collaborations(state: &AppState, name: &str, uid: ObjectId) -> Result<(), ApiError> {
    let defaults = if name == MOD_PROJECTS { MOD_PROJECT_DEFAULTS } else { MODPACK_DEFAULTS };
    for proj in find_all(state, name, doc! { "collaborators.user_id": uid }).await? {
        let collaborators = proj.get_array("collaborators").map(|a| a.as_slice()).unwrap_or(&[]);
        let is_me = |c: &Bson| c.as_document().and_then(|d| d.get_object_id("user_id").ok()) == Some(uid);
        if !collaborators.iter().any(is_me) {
            continue;
        }
        let kept: Vec<Bson> = collaborators
            .iter()
            .filter(|c| !is_me(c))
            .map(|c| {
                let d = c.as_document().cloned().unwrap_or_default();
                Bson::Document(doc! {
                    "user_id": d.get("user_id").cloned().unwrap_or(Bson::Null),
                    "username": d.get("username").cloned().unwrap_or(Bson::Null),
                })
            })
            .collect();
        let mut set = doc! { "collaborators": kept };
        for (field, fill) in defaults {
            if !proj.contains_key(*field) {
                set.insert(*field, fill.bson());
            }
        }
        let pid = proj.get("_id").cloned().unwrap_or(Bson::Null);
        coll(state, name)?.update_one(doc! { "_id": pid }, doc! { "$set": set }).await.map_err(internal)?;
    }
    Ok(())
}

/// `anonymize_account`: irreversibly strip the account's personal data.
pub async fn anonymize(state: &AppState, mut user: SiteUser) -> Result<(), ApiError> {
    let uid = user.id;
    if let Some(addr) = user.notify_email.as_deref().filter(|a| !a.is_empty()) {
        delete_many(state, EMAIL_OUTBOX, doc! { "to": addr }).await?;
    }
    let mine = doc! { "site_user_id": uid };
    let owned = doc! { "owner_id": uid };
    delete_many(state, SITE_SESSIONS, mine.clone()).await?;
    delete_many(state, MOD_GIT_TOKENS, mine.clone()).await?;
    delete_many(state, MOD_PROFILES, mine.clone()).await?;
    delete_many(state, MOD_CLAIM_REQUESTS, doc! { "claimant_id": uid }).await?;
    delete_many(state, USERNAME_REQUESTS, mine).await?;
    delete_many(state, DM_SUBSCRIPTIONS, owned.clone()).await?;
    delete_many(state, SITE_WEBHOOKS, owned.clone()).await?;
    delete_many(state, IMAGE_DESIGNS, owned.clone()).await?;

    drop_counted(state, MOD_STARS, uid, ("project_id", MOD_PROJECTS, "star_count")).await?;
    drop_counted(state, MODPACK_STARS, uid, ("modpack_id", MODPACK_PROJECTS, "star_count")).await?;
    drop_counted(state, GIVEAWAY_ENTRIES, uid, ("giveaway_id", GIVEAWAYS, "entry_count")).await?;

    let anon = anon_username(state).await?;
    update_many(state, GIVEAWAYS, doc! { "winner_user_id": uid }, doc! { "$set": { "winner_username": &anon } })
        .await?;
    let handles = doc! { "$set": { "owner_username": &anon, "owner_handle": &anon } };
    update_many(state, MOD_PROJECTS, owned.clone(), handles.clone()).await?;
    update_many(state, MODPACK_PROJECTS, owned, handles).await?;
    leave_collaborations(state, MOD_PROJECTS, uid).await?;
    leave_collaborations(state, MODPACK_PROJECTS, uid).await?;

    user.username = anon;
    user.discord_handle = String::new();
    user.notify_email = None;
    user.display_name = None;
    user.discord_id = None;
    user.discord_avatar = None;
    user.is_active = false;
    user.is_verified = false;
    user.is_deleted = true;
    clear_claim(&mut user);
    user.token_version += 1;
    user.updated_at = dt::to_bson(dt::now());
    save_user(state, &user).await?;
    clear_discord_token(state, uid).await
}

/// `POST /v1/site-auth/me/delete`.
pub async fn delete(
    State(state): State<AppState>,
    DepsThenBody(CurrentSiteUser(user), body): DepsThenBody<CurrentSiteUser, DeleteBody>,
) -> Result<Response, ApiError> {
    if pystr::strip(&body.confirm_username).to_lowercase() != user.username.to_lowercase() {
        return Err(ApiError::bad_request("The confirmation username doesn't match your account."));
    }
    anonymize(&state, user).await?;
    Ok(pyjson::no_content())
}

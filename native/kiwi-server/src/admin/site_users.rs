//! `/admin/site-users*`, `/admin/site-claims*` and `/admin/username-requests*`:
//! the Discord-signup website accounts, their Trove-name claims and the
//! admin-approved username changes (`app/site_auth/usernames.py`).

use std::collections::HashMap;

use axum::{extract::State, http::StatusCode};
use futures::TryStreamExt;
use kiwi_core::pystr;
use mongodb::bson::{Bson, Document, doc, oid::ObjectId};
use serde_json::{Value, json};

use super::{
    Args, RequestSeg, Res, Seg, UserSeg, coll,
    discord::FetchError,
    docs::{
        self, MOD_PROJECTS, MODPACK_PROJECTS, USERNAME_REQUESTS, UsernameRequest, find_all, find_one, iso_opt, pyd_opt,
    },
    internal,
};
use crate::{
    auth::{
        Superuser,
        models::{SITE_SESSIONS, SITE_USERS, SiteUser},
    },
    error::{ApiError, ErrorCode},
    pyjson::PydanticJson,
    routes::store::service::re_escape,
    state::AppState,
    validation::{DepsThenBody, Field, FromBody, FromParams, Params},
};

// --- bodies and queries ------------------------------------------------------------

/// `TakedownRequest` (the rejection reason).
pub struct ReasonBody {
    pub reason: String,
}

impl FromBody for ReasonBody {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| Some(ReasonBody { reason: m.field("reason").str().max_len(2000).default(String::new()) }))
            .required()
    }
}

/// `SiteUsernameRequestBody`.
pub struct UsernameBody {
    pub username: String,
}

impl FromBody for UsernameBody {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| Some(UsernameBody { username: m.field("username").str().min_len(3).max_len(24).required() }))
            .required()
    }
}

/// `SiteClaimedNameRequest`.
pub struct ClaimedNameBody {
    pub name: String,
}

impl FromBody for ClaimedNameBody {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| Some(ClaimedNameBody { name: m.field("name").str().max_len(64).default(String::new()) }))
            .required()
    }
}

/// `pending_only: bool = Query(default=True)`.
pub struct PendingOnly(pub bool);

impl FromParams for PendingOnly {
    fn from_params(p: &mut Params<'_>) -> Self {
        PendingOnly(p.query("pending_only").bool().default(true))
    }
}

/// `status: str | None = Query(default="pending")`.
pub struct StatusQuery(pub Option<String>);

impl FromParams for StatusQuery {
    fn from_params(p: &mut Params<'_>) -> Self {
        StatusQuery(p.query("status").str().nullable_default("pending".into()))
    }
}

/// `list_site_users`' query.
pub struct SiteUsersQuery {
    pub q: Option<String>,
    pub limit: i64,
    pub offset: i64,
}

impl FromParams for SiteUsersQuery {
    fn from_params(p: &mut Params<'_>) -> Self {
        SiteUsersQuery {
            q: p.query("q").str().optional(),
            limit: p.query("limit").int().ge(1).le(200).default(50),
            offset: p.query("offset").int().ge(0).default(0),
        }
    }
}

// --- views ---------------------------------------------------------------------------

/// `_site_claim_view` (`SiteClaimAdminView`).
pub fn claim_view(u: &SiteUser) -> Value {
    json!({
        "user_id": u.id.to_hex(),
        "username": u.username,
        "display_name": u.display_name,
        "discord_id": u.discord_id,
        "claimed_trove_name": u.claimed_trove_name,
        "claimed_trove_display": u.claimed_trove_display,
        "claimed_at": pyd_opt(&u.claimed_at),
        "claim_verified": u.claim_verified,
        "claim_verified_at": pyd_opt(&u.claim_verified_at),
    })
}

/// `_site_user_dto`.
pub fn site_user_dto(u: &SiteUser, mod_count: i64, modpack_count: i64) -> Value {
    json!({
        "id": u.id.to_hex(),
        "username": u.username,
        "discord_handle": u.discord_handle,
        "display_name": u.display_name,
        "is_active": u.is_active,
        "is_verified": u.is_verified,
        "discord_id": u.discord_id.filter(|&d| d != 0).map(|d| d.to_string()),
        "claimed_trove_name": u.claimed_trove_display.clone().filter(|s| !s.is_empty()).or_else(|| u.claimed_trove_name.clone()),
        "claim_verified": u.claim_verified,
        "mod_count": mod_count,
        "modpack_count": modpack_count,
        "created_at": iso_opt(&Some(u.created_at)),
        "last_login_at": iso_opt(&u.last_login_at),
    })
}

/// `username_request_dto`.
pub fn request_dto(r: &UsernameRequest) -> Value {
    json!({
        "id": r.id.to_hex(),
        "site_user_id": r.site_user_id.to_hex(),
        "current_username": r.current_username,
        "requested_username": r.requested_username,
        "status": r.status,
        "reason": r.reason,
        "created_at": iso_opt(&Some(r.created_at)),
        "resolved_at": iso_opt(&r.resolved_at),
    })
}

// --- shared --------------------------------------------------------------------------

/// `_load_site_user_or_404`: a malformed id and a missing user are both 404s.
async fn load_site_user(state: &AppState, id: &str, message: &str) -> Result<SiteUser, ApiError> {
    let Ok(oid) = ObjectId::parse_str(id) else { return Err(ApiError::not_found(message)) };
    find_one(&coll(state, SITE_USERS), doc! {"_id": oid}).await?.ok_or_else(|| ApiError::not_found(message))
}

async fn save_user(state: &AppState, u: &SiteUser) -> Result<(), ApiError> {
    docs::save(&coll(state, SITE_USERS), u.id, docs::site_user_doc(u)?).await
}

/// The caller's edits saved together with `revoke_all_sessions(user)`.
async fn save_revoking_sessions(state: &AppState, u: &mut SiteUser) -> Result<(), ApiError> {
    u.token_version += 1;
    u.updated_at = docs::now();
    save_user(state, u).await?;
    coll(state, SITE_SESSIONS)
        .update_many(doc! {"$and": [{"site_user_id": u.id}, {"revoked": false}]}, doc! {"$set": {"revoked": true}})
        .await
        .map_err(internal)?;
    Ok(())
}

pub const USERNAME_FORMAT: &str = "Username must be 3-24 characters using lowercase letters, numbers, \
    underscores or periods - a period can't start it or repeat (\"..\").";

pub const RESERVED_USERNAMES: [&str; 20] = [
    "stray",
    "why",
    "admin",
    "administrator",
    "api",
    "me",
    "mods",
    "modpacks",
    "trovesaurus",
    "kiwi",
    "support",
    "login",
    "logout",
    "dashboard",
    "system",
    "moderator",
    "staff",
    "root",
    "null",
    "undefined",
];

/// `normalize_username`.
pub fn normalize_username(raw: &str) -> Result<String, ApiError> {
    let name = pystr::strip(raw).to_lowercase();
    let n = name.chars().count();
    let charset = name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '.');
    if !(3..=24).contains(&n) || !charset || name.starts_with('.') || name.contains("..") {
        return Err(ApiError::bad_request(USERNAME_FORMAT));
    }
    if RESERVED_USERNAMES.contains(&name.as_str()) {
        return Err(ApiError::new(StatusCode::CONFLICT, ErrorCode::BadRequest, "That username is reserved."));
    }
    Ok(name)
}

/// `_assert_username_free`.
async fn assert_username_free(state: &AppState, name: &str, exclude: ObjectId, message: &str) -> Result<(), ApiError> {
    let taken: Option<SiteUser> = find_one(&coll(state, SITE_USERS), doc! {"username": name}).await?;
    if taken.is_some_and(|t| t.id != exclude) {
        return Err(ApiError::conflict(message));
    }
    Ok(())
}

/// `_rehome_handles`: the user's mods and modpacks follow the new name.
async fn rehome_handles(state: &AppState, owner: ObjectId, name: &str) -> Result<(), ApiError> {
    for c in [MOD_PROJECTS, MODPACK_PROJECTS] {
        coll(state, c)
            .update_many(doc! {"owner_id": owner}, doc! {"$set": {"owner_handle": name}})
            .await
            .map_err(internal)?;
    }
    Ok(())
}

async fn rename(state: &AppState, u: &mut SiteUser, name: &str) -> Result<(), ApiError> {
    u.username = name.to_owned();
    u.updated_at = docs::now();
    save_user(state, u).await?;
    rehome_handles(state, u.id, name).await
}

async fn save_request(state: &AppState, r: &UsernameRequest) -> Result<(), ApiError> {
    docs::save(&coll(state, USERNAME_REQUESTS), r.id, r.to_doc()).await
}

// --- Trove-name claims -----------------------------------------------------------------

pub fn claims_filter(pending_only: bool) -> Document {
    let mut filter = doc! {"claimed_trove_name": {"$ne": Bson::Null}};
    if pending_only {
        filter.insert("claim_verified", false);
    }
    filter
}

pub async fn list_site_claims(
    State(state): State<AppState>,
    _: Superuser,
    Args(PendingOnly(pending)): Args<PendingOnly>,
) -> Res {
    let filter = claims_filter(pending);
    let users: Vec<SiteUser> = find_all(&coll(&state, SITE_USERS), filter, Some(doc! {"claimed_at": 1})).await?;
    let items: Vec<Value> = users.iter().map(claim_view).collect();
    let count = items.len();
    Ok(PydanticJson(json!({"items": items, "count": count})))
}

/// `_get_claimant`.
async fn claimant(state: &AppState, id: &str) -> Result<SiteUser, ApiError> {
    let Ok(oid) = ObjectId::parse_str(id) else { return Err(ApiError::not_found("No such user.")) };
    match find_one::<SiteUser>(&coll(state, SITE_USERS), doc! {"_id": oid}).await? {
        Some(u) if u.claimed_trove_name.as_deref().is_some_and(|n| !n.is_empty()) => Ok(u),
        _ => Err(ApiError::not_found("No Trove-name claim for that user.")),
    }
}

pub async fn approve_site_claim(State(state): State<AppState>, _: Superuser, Seg(id): UserSeg) -> Res {
    let mut u = claimant(&state, &id).await?;
    u.claim_verified = true;
    u.claim_verified_at = Some(docs::now());
    u.updated_at = docs::now();
    save_user(&state, &u).await?;
    Ok(PydanticJson(claim_view(&u)))
}

pub async fn reject_site_claim(State(state): State<AppState>, _: Superuser, Seg(id): UserSeg) -> Res {
    let mut u = claimant(&state, &id).await?;
    docs::clear_claim(&mut u);
    u.updated_at = docs::now();
    save_user(&state, &u).await?;
    Ok(PydanticJson(claim_view(&u)))
}

// --- username requests -------------------------------------------------------------------

pub async fn list_username_requests(
    State(state): State<AppState>,
    _: Superuser,
    Args(StatusQuery(status)): Args<StatusQuery>,
) -> Res {
    let mut filter = Document::new();
    if let Some(s) = status.filter(|s| !s.is_empty()) {
        filter.insert("status", s);
    }
    let docs: Vec<Document> = coll(&state, USERNAME_REQUESTS)
        .find(filter)
        .sort(doc! {"created_at": -1})
        .limit(100)
        .await
        .map_err(internal)?
        .try_collect()
        .await
        .map_err(internal)?;
    let items = docs
        .into_iter()
        .map(|d| UsernameRequest::from_doc(d).map(|r| request_dto(&r)))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(PydanticJson(json!({"items": items})))
}

/// `_get_request`: anything that goes wrong loading it is a 404.
async fn get_request(state: &AppState, id: &str) -> Result<UsernameRequest, ApiError> {
    let missing = || ApiError::not_found("Username request not found");
    let oid = ObjectId::parse_str(id).map_err(|_| missing())?;
    match coll(state, USERNAME_REQUESTS).find_one(doc! {"_id": oid}).await {
        Ok(Some(d)) => UsernameRequest::from_doc(d).map_err(|_| missing()),
        _ => Err(missing()),
    }
}

fn already_resolved() -> ApiError {
    ApiError::bad_request("This request is already resolved.")
}

pub async fn approve_username_request(
    State(state): State<AppState>,
    Superuser(admin): Superuser,
    Seg(id): RequestSeg,
) -> Res {
    let mut req = get_request(&state, &id).await?;
    if req.status != "pending" {
        return Err(already_resolved());
    }
    let mut user: SiteUser = find_one(&coll(&state, SITE_USERS), doc! {"_id": req.site_user_id})
        .await?
        .ok_or_else(|| ApiError::not_found("The requesting user no longer exists."))?;
    let name = req.requested_username.clone();
    assert_username_free(&state, &name, user.id, "That username was taken in the meantime.").await?;
    rename(&state, &mut user, &name).await?;
    req.status = "approved".into();
    req.resolved_by = Some(admin.id);
    req.resolved_at = Some(docs::now());
    save_request(&state, &req).await?;
    let mut out = request_dto(&req);
    out["new_username"] = json!(name);
    Ok(PydanticJson(out))
}

pub async fn reject_username_request(
    State(state): State<AppState>,
    Seg(id): RequestSeg,
    DepsThenBody(Superuser(admin), body): DepsThenBody<Superuser, ReasonBody>,
) -> Res {
    let mut req = get_request(&state, &id).await?;
    if req.status != "pending" {
        return Err(already_resolved());
    }
    req.status = "rejected".into();
    req.reason = pystr::prefix(pystr::strip(&body.reason), 2000).to_owned();
    req.resolved_by = Some(admin.id);
    req.resolved_at = Some(docs::now());
    save_request(&state, &req).await?;
    Ok(PydanticJson(request_dto(&req)))
}

// --- site users ----------------------------------------------------------------------

/// `list_site_users`' filter.
pub fn site_users_filter(q: Option<&str>) -> Document {
    match q.map(pystr::strip).filter(|s| !s.is_empty()) {
        Some(s) => {
            let rx = doc! {"$regex": re_escape(s), "$options": "i"};
            doc! {"$or": [{"username": rx.clone()}, {"discord_handle": rx.clone()}, {"display_name": rx}]}
        }
        None => Document::new(),
    }
}

async fn owner_counts(state: &AppState, name: &str, ids: &[ObjectId]) -> Result<HashMap<String, i64>, ApiError> {
    let pipeline = vec![
        doc! {"$match": {"owner_id": {"$in": ids.to_vec()}}},
        doc! {"$group": {"_id": "$owner_id", "n": {"$sum": 1}}},
    ];
    let rows: Vec<Document> =
        coll(state, name).aggregate(pipeline).await.map_err(internal)?.try_collect().await.map_err(internal)?;
    let mut out = HashMap::new();
    for r in &rows {
        out.insert(super::users::id_key(r.get("_id")), super::users::int_field(r, "n")?);
    }
    Ok(out)
}

pub async fn list_site_users(State(state): State<AppState>, _: Superuser, Args(q): Args<SiteUsersQuery>) -> Res {
    let filter = site_users_filter(q.q.as_deref());
    let c = coll(&state, SITE_USERS);
    let total = c.count_documents(filter.clone()).await.map_err(internal)?;
    let docs: Vec<Document> = c
        .find(filter)
        .sort(doc! {"created_at": -1})
        .skip(q.offset as u64)
        .limit(q.limit)
        .await
        .map_err(internal)?
        .try_collect()
        .await
        .map_err(internal)?;
    let users = docs.into_iter().map(docs::load).collect::<Result<Vec<SiteUser>, _>>()?;
    let ids: Vec<ObjectId> = users.iter().map(|u| u.id).collect();
    let (mods, packs) = if ids.is_empty() {
        (HashMap::new(), HashMap::new())
    } else {
        (owner_counts(&state, MOD_PROJECTS, &ids).await?, owner_counts(&state, MODPACK_PROJECTS, &ids).await?)
    };
    let items: Vec<Value> = users
        .iter()
        .map(|u| {
            let k = u.id.to_hex();
            site_user_dto(u, mods.get(&k).copied().unwrap_or(0), packs.get(&k).copied().unwrap_or(0))
        })
        .collect();
    let count = items.len();
    Ok(PydanticJson(json!({"items": items, "count": count, "total": total})))
}

const NO_SITE_USER: &str = "No such site user.";

pub async fn get_site_user(State(state): State<AppState>, _: Superuser, Seg(id): UserSeg) -> Res {
    let u = load_site_user(&state, &id, NO_SITE_USER).await?;
    Ok(PydanticJson(site_user_dto(&u, 0, 0)))
}

pub async fn deactivate_site_user(State(state): State<AppState>, _: Superuser, Seg(id): UserSeg) -> Res {
    let mut u = load_site_user(&state, &id, NO_SITE_USER).await?;
    u.is_active = false;
    save_revoking_sessions(&state, &mut u).await?;
    Ok(PydanticJson(json!({"id": u.id.to_hex(), "is_active": u.is_active})))
}

pub async fn activate_site_user(State(state): State<AppState>, _: Superuser, Seg(id): UserSeg) -> Res {
    let mut u = load_site_user(&state, &id, NO_SITE_USER).await?;
    u.is_active = true;
    u.updated_at = docs::now();
    save_user(&state, &u).await?;
    Ok(PydanticJson(json!({"id": u.id.to_hex(), "is_active": u.is_active})))
}

pub async fn logout_site_user(State(state): State<AppState>, _: Superuser, Seg(id): UserSeg) -> Res {
    let mut u = load_site_user(&state, &id, NO_SITE_USER).await?;
    save_revoking_sessions(&state, &mut u).await?;
    Ok(PydanticJson(json!({"id": u.id.to_hex(), "ok": true})))
}

/// `usernames.admin_set_username`.
pub async fn set_site_username(
    State(state): State<AppState>,
    Seg(id): UserSeg,
    DepsThenBody(Superuser(admin), body): DepsThenBody<Superuser, UsernameBody>,
) -> Res {
    let mut user = load_site_user(&state, &id, "No such user.").await?;
    let name = normalize_username(&body.username)?;
    if name == user.username {
        return Ok(PydanticJson(json!({"username": name, "changed": false})));
    }
    assert_username_free(&state, &name, user.id, "That username is already taken.").await?;
    rename(&state, &mut user, &name).await?;
    let pending = coll(&state, USERNAME_REQUESTS)
        .find_one(doc! {"$and": [{"site_user_id": user.id}, {"status": "pending"}]})
        .await
        .map_err(internal)?;
    if let Some(d) = pending {
        let mut req = UsernameRequest::from_doc(d)?;
        req.status = "approved".into();
        req.resolved_by = Some(admin.id);
        req.resolved_at = Some(docs::now());
        save_request(&state, &req).await?;
    }
    Ok(PydanticJson(json!({"username": name, "changed": true})))
}

pub async fn set_site_claimed_name(
    State(state): State<AppState>,
    Seg(id): UserSeg,
    DepsThenBody(_, body): DepsThenBody<Superuser, ClaimedNameBody>,
) -> Res {
    let mut u = load_site_user(&state, &id, NO_SITE_USER).await?;
    let name = pystr::strip(&body.name).to_owned();
    if name.is_empty() {
        docs::clear_claim(&mut u);
    } else {
        let low = name.to_lowercase();
        let clash: Option<SiteUser> = find_one(&coll(&state, SITE_USERS), doc! {"claimed_trove_name": &low}).await?;
        if clash.is_some_and(|c| c.id != u.id) {
            return Err(ApiError::conflict(format!("'{name}' is already claimed by another user.")));
        }
        let now = docs::now();
        u.claimed_trove_name = Some(low);
        u.claimed_trove_display = Some(name);
        u.claimed_at = Some(now);
        u.claim_verified = true;
        u.claim_verified_at = Some(now);
        u.claim_baseline = Document::new();
    }
    u.updated_at = docs::now();
    save_user(&state, &u).await?;
    Ok(PydanticJson(site_user_dto(&u, 0, 0)))
}

pub async fn refresh_site_discord(State(state): State<AppState>, _: Superuser, Seg(id): UserSeg) -> Res {
    let mut u = load_site_user(&state, &id, NO_SITE_USER).await?;
    let Some(discord_id) = u.discord_id.filter(|&d| d != 0) else {
        return Err(ApiError::bad_request("This account has no linked Discord id."));
    };
    let data = match super::discord::fetch_user(state.settings(), discord_id).await {
        Ok(d) => d,
        Err(FetchError::Rest(m)) => {
            return Err(ApiError::new(StatusCode::BAD_GATEWAY, ErrorCode::ServiceUnavailable, m));
        }
        Err(FetchError::Other) => return Err(ApiError::internal()),
    };
    let data = match data {
        Some(Value::Object(m)) if !m.is_empty() => m,
        Some(Value::Object(_)) | None | Some(Value::Null) => {
            return Err(ApiError::not_found("Discord has no record of that user any more."));
        }
        Some(other) if !py_truthy(&other) => {
            return Err(ApiError::not_found("Discord has no record of that user any more."));
        }
        Some(other) => return Err(internal(format!("discord user payload: {other}"))),
    };
    let handle = match data.get("username") {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(s)) => pystr::strip(s).to_owned(),
        Some(v) if !py_truthy(v) => String::new(),
        Some(other) => return Err(internal(format!("discord username: {other}"))),
    };
    if !handle.is_empty() {
        u.discord_handle = handle;
    }
    if let Some(avatar) = data.get("avatar") {
        u.discord_avatar = match avatar {
            Value::Null => None,
            Value::String(s) => Some(s.clone()),
            other => return Err(internal(format!("discord avatar: {other}"))),
        };
    }
    u.updated_at = docs::now();
    save_user(&state, &u).await?;
    Ok(PydanticJson(site_user_dto(&u, 0, 0)))
}

/// Python truthiness of a JSON value.
fn py_truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(m) => !m.is_empty(),
    }
}

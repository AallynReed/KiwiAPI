//! The master's Mods Hub and Modpacks moderation routes of `app/admin/router.py`
//! (`/admin/mods*`, `/admin/modpacks*`): content reports, takedowns, forced
//! deletes, the stray import and approval queue, stray assignment, claims and
//! remix claims. Every route sits behind `get_current_superuser`.

pub mod model;
pub mod service;
pub mod strayimport;

#[cfg(test)]
mod store_tests;
#[cfg(test)]
mod tests;

use axum::{
    Router,
    extract::{FromRequestParts, State},
    http::request::Parts,
    response::Response,
    routing::{delete, get, post},
};
use kiwi_core::pystr;
use serde_json::{Value, json};

use self::{
    model::{CLAIM, REMIX_CLAIM},
    service::{CLAIMS, Page, REMIX_CLAIMS},
};
use crate::{
    admin::{Args, site_users::StatusQuery},
    auth::Superuser,
    error::{self, ApiError},
    pyjson::{self, PydanticJson},
    state::AppState,
    validation::{DepsThenBody, Field, FromBody, FromParams, Params},
};

type Res<T = PydanticJson> = Result<T, ApiError>;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/admin/mods/reports", get(list_reports))
        .route("/admin/mods/reports/{report_id}/dismiss", post(dismiss_report))
        .route("/admin/mods/projects", get(list_projects))
        .route("/admin/mods/projects/{project_id}", delete(delete_project))
        .route("/admin/mods/projects/{project_id}/takedown", post(take_down_project))
        .route("/admin/mods/projects/{project_id}/restore", post(restore_project))
        .route("/admin/mods/profiles/{profile_id}/takedown", post(take_down_profile))
        .route("/admin/mods/profiles/{profile_id}/restore", post(restore_profile))
        .route(
            "/admin/mods/stray/import",
            get(stray_import_state).post(stray_import_start).fallback(|| async { error::method_not_allowed("GET") }),
        )
        .route("/admin/mods/stray", get(list_stray))
        .route("/admin/mods/stray/assign", post(assign_stray))
        .route("/admin/mods/stray/{project_id}/approve", post(approve_stray))
        .route("/admin/mods/stray/{project_id}/reject", post(reject_stray))
        .route("/admin/mods/claims", get(list_claims))
        .route("/admin/mods/claims/{claim_id}/approve", post(approve_claim))
        .route("/admin/mods/claims/{claim_id}/reject", post(reject_claim))
        .route("/admin/mods/remix-claims", get(list_remix_claims))
        .route("/admin/mods/remix-claims/{claim_id}/approve", post(approve_remix_claim))
        .route("/admin/mods/remix-claims/{claim_id}/reject", post(reject_remix_claim))
        .route("/admin/modpacks", get(list_modpacks))
        .route("/admin/modpacks/{pack_id}", delete(delete_modpack))
        .route("/admin/modpacks/{pack_id}/takedown", post(take_down_modpack))
        .route("/admin/modpacks/{pack_id}/restore", post(restore_modpack))
}

/// The route's one `{id}` path parameter, Starlette-decoded.
pub struct IdSeg(pub String);

impl<S: Send + Sync> FromRequestParts<S> for IdSeg {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        Ok(IdSeg(crate::admin::path_params(parts).into_iter().next().map(|(_, v)| v).unwrap_or_default()))
    }
}

// --- params and bodies ------------------------------------------------------------------

/// `TakedownRequest`.
pub struct TakedownBody {
    pub reason: String,
}

impl FromBody for TakedownBody {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| Some(TakedownBody { reason: m.field("reason").str().max_len(2000).default(String::new()) }))
            .required()
    }
}

/// `StrayAssignRequest`.
pub struct AssignBody {
    pub user_id: String,
    pub project_ids: Vec<String>,
}

impl FromBody for AssignBody {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| {
            Some(AssignBody {
                user_id: m.field("user_id").str().min_len(1).required(),
                project_ids: m.field("project_ids").list(|f| f.str().required()).min_len(1).required(),
            })
        })
        .required()
    }
}

pub struct ResolvedQuery(pub bool);

impl FromParams for ResolvedQuery {
    fn from_params(p: &mut Params<'_>) -> Self {
        ResolvedQuery(p.query("resolved").bool().default(false))
    }
}

fn page(p: &mut Params<'_>) -> Page {
    Page { limit: p.query("limit").int().ge(1).le(200).default(50), offset: p.query("offset").int().ge(0).default(0) }
}

/// `list_all_mod_projects` / `list_all_modpacks` query.
pub struct MasterQuery {
    pub q: Option<String>,
    pub owner: Option<String>,
    pub visibility: Option<String>,
    pub sort: String,
    pub page: Page,
}

impl FromParams for MasterQuery {
    fn from_params(p: &mut Params<'_>) -> Self {
        MasterQuery {
            q: p.query("q").str().optional(),
            owner: p.query("owner").str().optional(),
            visibility: p.query("visibility").str().optional(),
            sort: p.query("sort").str().default("updated".into()),
            page: page(p),
        }
    }
}

pub struct PackQuery(pub MasterQuery);

impl FromParams for PackQuery {
    fn from_params(p: &mut Params<'_>) -> Self {
        PackQuery(MasterQuery {
            q: p.query("q").str().optional(),
            owner: p.query("owner").str().optional(),
            visibility: p.query("visibility").str().optional(),
            sort: String::new(),
            page: page(p),
        })
    }
}

pub struct StrayQuery {
    pub status: Option<String>,
    pub q: Option<String>,
    pub page: Page,
}

impl FromParams for StrayQuery {
    fn from_params(p: &mut Params<'_>) -> Self {
        StrayQuery {
            status: p.query("status").str().nullable_default("pending".into()),
            q: p.query("q").str().optional(),
            page: page(p),
        }
    }
}

pub struct ImportQuery {
    pub resync: bool,
    pub force: bool,
    pub author_id: Option<String>,
    pub owner_id: Option<String>,
}

impl FromParams for ImportQuery {
    fn from_params(p: &mut Params<'_>) -> Self {
        ImportQuery {
            resync: p.query("resync").bool().default(false),
            force: p.query("force").bool().default(false),
            author_id: p.query("author_id").str().optional(),
            owner_id: p.query("owner_id").str().optional(),
        }
    }
}

fn listing(items: Vec<Value>, total: u64) -> PydanticJson {
    let count = items.len();
    PydanticJson(json!({"items": items, "count": count, "total": total}))
}

// --- reports and takedowns -------------------------------------------------------------------

async fn list_reports(
    State(state): State<AppState>,
    _: Superuser,
    Args(ResolvedQuery(resolved)): Args<ResolvedQuery>,
) -> Res {
    Ok(PydanticJson(json!({"items": service::list_reports(&state, resolved).await?})))
}

async fn dismiss_report(State(state): State<AppState>, _: Superuser, IdSeg(id): IdSeg) -> Res {
    service::dismiss_report(&state, &id).await?;
    Ok(PydanticJson(json!({"ok": true})))
}

async fn list_projects(State(state): State<AppState>, _: Superuser, Args(q): Args<MasterQuery>) -> Res {
    let filter = service::master_filter(q.q.as_deref(), q.owner.as_deref(), q.visibility.as_deref(), true);
    let (items, total) = service::master_list_projects(&state, filter, &q.sort, &q.page).await?;
    Ok(listing(items, total))
}

async fn delete_project(State(state): State<AppState>, _: Superuser, IdSeg(id): IdSeg) -> Result<Response, ApiError> {
    service::master_delete_project(&state, &id).await?;
    Ok(pyjson::no_content())
}

async fn take_down_project(
    State(state): State<AppState>,
    IdSeg(id): IdSeg,
    DepsThenBody(_, body): DepsThenBody<Superuser, TakedownBody>,
) -> Res {
    Ok(PydanticJson(service::take_down_project(&state, &id, &body.reason).await?))
}

async fn restore_project(State(state): State<AppState>, _: Superuser, IdSeg(id): IdSeg) -> Res {
    Ok(PydanticJson(service::restore_project(&state, &id).await?))
}

async fn take_down_profile(
    State(state): State<AppState>,
    IdSeg(id): IdSeg,
    DepsThenBody(_, body): DepsThenBody<Superuser, TakedownBody>,
) -> Res {
    Ok(PydanticJson(service::take_down_profile(&state, &id, &body.reason).await?))
}

async fn restore_profile(State(state): State<AppState>, _: Superuser, IdSeg(id): IdSeg) -> Res {
    Ok(PydanticJson(service::restore_profile(&state, &id).await?))
}

// --- stray -----------------------------------------------------------------------------------

async fn stray_import_state(State(state): State<AppState>, _: Superuser) -> Res {
    let st = strayimport::load_state(&state).await?;
    Ok(PydanticJson(strayimport::state_dto(&st)))
}

async fn stray_import_start(State(state): State<AppState>, _: Superuser, Args(q): Args<ImportQuery>) -> Res {
    let author_id = q.author_id.as_deref().map(pystr::strip).filter(|s| !s.is_empty()).map(str::to_owned);
    let mut owner = None;
    if let Some(oid) = q.owner_id.as_deref().map(pystr::strip).filter(|s| !s.is_empty()) {
        if author_id.is_none() {
            return Err(ApiError::bad_request("An owner needs an author id."));
        }
        let found = match mongodb::bson::oid::ObjectId::parse_str(oid) {
            Ok(id) => service::site_user(&state, id).await?,
            Err(_) => None,
        };
        owner = Some(found.ok_or_else(|| ApiError::not_found("No such user."))?);
    }
    Ok(PydanticJson(strayimport::start(&state, q.resync, q.force, author_id, owner).await?))
}

async fn list_stray(State(state): State<AppState>, _: Superuser, Args(q): Args<StrayQuery>) -> Res {
    let filter = service::stray_filter(q.status.as_deref(), q.q.as_deref());
    let (items, total) = service::master_list_stray(&state, filter, &q.page).await?;
    Ok(listing(items, total))
}

async fn approve_stray(State(state): State<AppState>, _: Superuser, IdSeg(id): IdSeg) -> Res {
    Ok(PydanticJson(service::approve_stray(&state, &id).await?))
}

async fn reject_stray(State(state): State<AppState>, _: Superuser, IdSeg(id): IdSeg) -> Res {
    Ok(PydanticJson(service::reject_stray(&state, &id).await?))
}

async fn assign_stray(
    State(state): State<AppState>,
    DepsThenBody(Superuser(admin), body): DepsThenBody<Superuser, AssignBody>,
) -> Res {
    Ok(PydanticJson(service::admin_assign_stray(&state, &body.project_ids, &body.user_id, admin.id).await?))
}

// --- claims ------------------------------------------------------------------------------------

async fn list_claims(State(state): State<AppState>, _: Superuser, Args(StatusQuery(status)): Args<StatusQuery>) -> Res {
    let rows = service::list_claims(&state, CLAIMS, CLAIM, status.as_deref()).await?;
    Ok(PydanticJson(json!({"items": rows.iter().map(service::claim_dto).collect::<Vec<_>>()})))
}

async fn approve_claim(State(state): State<AppState>, Superuser(admin): Superuser, IdSeg(id): IdSeg) -> Res {
    Ok(PydanticJson(service::approve_claim(&state, &id, admin.id).await?))
}

async fn reject_claim(State(state): State<AppState>, Superuser(admin): Superuser, IdSeg(id): IdSeg) -> Res {
    Ok(PydanticJson(service::reject_claim(&state, &id, admin.id).await?))
}

async fn list_remix_claims(
    State(state): State<AppState>,
    _: Superuser,
    Args(StatusQuery(status)): Args<StatusQuery>,
) -> Res {
    let rows = service::list_claims(&state, REMIX_CLAIMS, REMIX_CLAIM, status.as_deref()).await?;
    Ok(PydanticJson(json!({"items": rows.iter().map(service::remix_claim_dto).collect::<Vec<_>>()})))
}

async fn approve_remix_claim(State(state): State<AppState>, Superuser(admin): Superuser, IdSeg(id): IdSeg) -> Res {
    Ok(PydanticJson(service::approve_remix_claim(&state, &id, admin.id).await?))
}

async fn reject_remix_claim(State(state): State<AppState>, Superuser(admin): Superuser, IdSeg(id): IdSeg) -> Res {
    Ok(PydanticJson(service::reject_remix_claim(&state, &id, admin.id).await?))
}

// --- modpacks ------------------------------------------------------------------------------------

async fn list_modpacks(State(state): State<AppState>, _: Superuser, Args(PackQuery(q)): Args<PackQuery>) -> Res {
    let filter = service::master_filter(q.q.as_deref(), q.owner.as_deref(), q.visibility.as_deref(), false);
    let (items, total) = service::master_list_modpacks(&state, filter, &q.page).await?;
    Ok(listing(items, total))
}

async fn delete_modpack(State(state): State<AppState>, _: Superuser, IdSeg(id): IdSeg) -> Result<Response, ApiError> {
    service::master_delete_modpack(&state, &id).await?;
    Ok(pyjson::no_content())
}

async fn take_down_modpack(
    State(state): State<AppState>,
    IdSeg(id): IdSeg,
    DepsThenBody(_, body): DepsThenBody<Superuser, TakedownBody>,
) -> Res {
    Ok(PydanticJson(service::take_down_modpack(&state, &id, &body.reason).await?))
}

async fn restore_modpack(State(state): State<AppState>, _: Superuser, IdSeg(id): IdSeg) -> Res {
    Ok(PydanticJson(service::restore_modpack(&state, &id).await?))
}

//! Giveaways (`app/giveaways`): the master-only vault and giveaway admin
//! (`/admin/vault/*`, `/admin/giveaways*`, always reachable), the signed-in
//! site-user routes and the tokenless `giveaways:read` lists (both behind
//! `feature_giveaways_enabled`, checked before auth like `_GIVEAWAYS_GATE`).
//!
//! The auto-open/draw worker ([`spawn_worker`]) ends
//! every tick with the events bus' `publish_type("giveaways")`.

pub mod body;
pub mod model;
pub mod service;
pub mod store;
pub mod views;

#[cfg(test)]
mod store_tests;
#[cfg(test)]
mod tests;

use std::{future::Future, sync::Arc, time::Duration};

use axum::{
    Router,
    extract::State,
    http::{HeaderValue, StatusCode, Uri, header},
    response::{IntoResponse, Response},
    routing::{get, patch, post},
};
use serde_json::Value;

use self::{
    body::{
        AdminBody, GiveawayCreate, GiveawayUpdate, VaultCodeUpdate, VaultCodesAdd, VaultItemCreate, VaultItemUpdate,
    },
    service::Res,
    store::Mongo,
};
use crate::{
    auth::{CurrentSiteUser, Public, Superuser, scopes},
    error,
    features::{self, RequireFeature},
    jobs,
    middleware::pyurl::scope_path,
    pyjson::{PydanticJson, no_content},
    ratelimit::RateLimitInfo,
    state::AppState,
    validation::{FromParams, Params, Query},
};

type Gate = RequireFeature<features::Giveaways>;
type GwPublic = Public<{ scopes::GIVEAWAYS_READ }>;

const PUBLIC_CACHE: &str = "public, max-age=30";

/// Starlette's 405 names only the first route on the path.
async fn allow_get() -> Response {
    error::method_not_allowed("GET")
}

async fn allow_patch() -> Response {
    error::method_not_allowed("PATCH")
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/admin/vault/items", get(vault_items).post(vault_item_create).fallback(allow_get))
        .route("/admin/vault/items/{item_id}", patch(vault_item_update).delete(vault_item_delete).fallback(allow_patch))
        .route("/admin/vault/items/{item_id}/codes", get(vault_item_codes).post(vault_codes_add).fallback(allow_get))
        .route("/admin/vault/codes/{code_id}", patch(vault_code_update).delete(vault_code_delete).fallback(allow_patch))
        .route("/admin/giveaways", get(giveaways_list).post(giveaways_create).fallback(allow_get))
        .route("/admin/giveaways/{giveaway_id}", patch(giveaways_update))
        .route("/admin/giveaways/{giveaway_id}/draw", post(giveaways_draw))
        .route("/admin/giveaways/{giveaway_id}/cancel", post(giveaways_cancel))
        .route("/v1/giveaways/mine", get(my_entries))
        .route("/v1/giveaways/me", get(my_giveaways))
        .route("/v1/giveaways/{giveaway_id}/enter", post(enter))
        .route("/v1/giveaways/ongoing", get(ongoing))
        .route("/v1/giveaways/upcoming", get(upcoming))
        .route("/v1/giveaways/ended", get(ended))
}

pub fn mongo(state: &AppState) -> Mongo<'_> {
    Mongo { db: state.mongo(), email_enabled: state.settings().email_enabled() }
}

/// Path segment `i` as Starlette matched it (percent-decoded).
fn seg(uri: &Uri, i: usize) -> String {
    scope_path(uri).split('/').nth(i).unwrap_or_default().to_owned()
}

fn model(r: Res<Value>) -> Response {
    match r {
        Ok(v) => PydanticJson(v).into_response(),
        Err(e) => e.into_response(),
    }
}

fn created(r: Res<Value>) -> Response {
    match r {
        Ok(v) => (StatusCode::CREATED, PydanticJson(v)).into_response(),
        Err(e) => e.into_response(),
    }
}

fn deleted(r: Res<()>) -> Response {
    match r {
        Ok(()) => no_content(),
        Err(e) => e.into_response(),
    }
}

fn public(rate_limit: RateLimitInfo, r: Res<Vec<Value>>) -> Response {
    match r {
        Ok(v) => (
            rate_limit,
            [(header::CACHE_CONTROL, HeaderValue::from_static(PUBLIC_CACHE))],
            PydanticJson(Value::Array(v)),
        )
            .into_response(),
        Err(e) => e.into_response(),
    }
}

// --- vault ------------------------------------------------------------------

async fn vault_items(State(st): State<AppState>, _: Superuser) -> Response {
    model(service::list_items(&mongo(&st)).await)
}

async fn vault_item_create(State(st): State<AppState>, AdminBody(req): AdminBody<VaultItemCreate>) -> Response {
    created(service::create_item(&mongo(&st), req).await)
}

async fn vault_item_update(
    State(st): State<AppState>,
    uri: Uri,
    AdminBody(req): AdminBody<VaultItemUpdate>,
) -> Response {
    model(service::update_item(&mongo(&st), &seg(&uri, 4), req).await)
}

async fn vault_item_delete(State(st): State<AppState>, uri: Uri, _: Superuser) -> Response {
    deleted(service::delete_item(&mongo(&st), &seg(&uri, 4)).await)
}

async fn vault_item_codes(State(st): State<AppState>, uri: Uri, _: Superuser) -> Response {
    model(service::list_codes(&mongo(&st), &seg(&uri, 4)).await)
}

async fn vault_codes_add(State(st): State<AppState>, uri: Uri, AdminBody(req): AdminBody<VaultCodesAdd>) -> Response {
    model(service::add_codes(&mongo(&st), &seg(&uri, 4), req).await)
}

async fn vault_code_update(
    State(st): State<AppState>,
    uri: Uri,
    AdminBody(req): AdminBody<VaultCodeUpdate>,
) -> Response {
    model(service::update_code(&mongo(&st), &seg(&uri, 4), req).await)
}

async fn vault_code_delete(State(st): State<AppState>, uri: Uri, _: Superuser) -> Response {
    deleted(service::delete_code(&mongo(&st), &seg(&uri, 4)).await)
}

// --- giveaways (admin) --------------------------------------------------------

async fn giveaways_list(State(st): State<AppState>, _: Superuser) -> Response {
    model(service::list_admin(&mongo(&st)).await)
}

async fn giveaways_create(State(st): State<AppState>, AdminBody(req): AdminBody<GiveawayCreate>) -> Response {
    created(service::create(&mongo(&st), req).await)
}

async fn giveaways_update(State(st): State<AppState>, uri: Uri, AdminBody(req): AdminBody<GiveawayUpdate>) -> Response {
    model(service::update(&mongo(&st), &seg(&uri, 3), req).await)
}

async fn giveaways_draw(State(st): State<AppState>, uri: Uri, _: Superuser) -> Response {
    model(service::draw_now(&mongo(&st), &seg(&uri, 3)).await)
}

async fn giveaways_cancel(State(st): State<AppState>, uri: Uri, _: Superuser) -> Response {
    model(service::cancel(&mongo(&st), &seg(&uri, 3)).await)
}

// --- signed-in site users -------------------------------------------------------

async fn my_entries(_: Gate, CurrentSiteUser(user): CurrentSiteUser, State(st): State<AppState>) -> Response {
    model(service::my_entry_ids(&mongo(&st), &user).await)
}

async fn my_giveaways(_: Gate, CurrentSiteUser(user): CurrentSiteUser, State(st): State<AppState>) -> Response {
    model(service::my_participations(&mongo(&st), &user).await)
}

async fn enter(_: Gate, CurrentSiteUser(user): CurrentSiteUser, State(st): State<AppState>, uri: Uri) -> Response {
    model(service::enter(&mongo(&st), &user, &seg(&uri, 3)).await)
}

// --- tokenless giveaways:read ---------------------------------------------------

pub struct EndedQuery {
    pub days: i64,
}

impl FromParams for EndedQuery {
    fn from_params(p: &mut Params<'_>) -> Self {
        EndedQuery { days: p.query("days").int().ge(1).le(30).default(7) }
    }
}

async fn ongoing(_: Gate, Public(ctx): GwPublic, State(st): State<AppState>) -> Response {
    public(ctx.rate_limit, service::list_ongoing(&mongo(&st)).await)
}

async fn upcoming(_: Gate, Public(ctx): GwPublic, State(st): State<AppState>) -> Response {
    public(ctx.rate_limit, service::list_upcoming(&mongo(&st)).await)
}

async fn ended(_: Gate, Public(ctx): GwPublic, State(st): State<AppState>, Query(q): Query<EndedQuery>) -> Response {
    public(ctx.rate_limit, service::list_ended(&mongo(&st), q.days).await)
}

// --- worker -----------------------------------------------------------------------

/// The 60 s auto-open/draw loop. `publish` is the
/// events bus' `publish_type("giveaways")`, run after every good tick.
pub fn spawn_worker<P, PF>(state: &AppState, publish: P)
where
    P: Fn(AppState) -> PF + Send + Sync + 'static,
    PF: Future<Output = anyhow::Result<()>> + Send + 'static,
{
    let publish = Arc::new(publish);
    jobs::spawn_periodic(
        state,
        "giveaways",
        move |st| {
            let publish = publish.clone();
            async move {
                service::run_due(&mongo(&st)).await?;
                publish(st).await
            }
        },
        |_| async { Duration::from_secs(60) },
    );
}

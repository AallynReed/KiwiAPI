//! Outbound Discord webhooks (`app/webhooks`): the User Dashboard CRUD under
//! `/v1/webhooks` (feature gate first, then the site session) and the
//! delivery-queue consumer. Only Discord webhook URLs are ever stored, so
//! delivery never reaches a host the user picked.

pub mod delivery;
pub mod embeds;
pub mod model;
pub mod service;
pub mod url;

#[cfg(test)]
mod tests;

use axum::{
    Router,
    extract::State,
    http::{StatusCode, request::Parts},
    response::{IntoResponse, Response},
    routing::{get, patch, post},
};
use serde_json::{Value, json};

use crate::{
    auth::CurrentSiteUser,
    error::method_not_allowed,
    features::{RequireFeature, Webhooks},
    outbound::store::MongoColl,
    pyjson::PydanticJson,
    state::AppState,
    validation::DepsThenBody,
};

pub use delivery::{enqueue, start as start_jobs};

const PREFIX: &str = "/v1/webhooks/";

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/v1/webhooks/events",
            get(list_event_types)
                .patch(update_hook)
                .delete(delete_hook)
                .fallback(|| async { method_not_allowed("GET") }),
        )
        .route("/v1/webhooks", get(list_hooks).post(create_hook).fallback(|| async { method_not_allowed("GET") }))
        .route(
            "/v1/webhooks/{webhook_id}",
            patch(update_hook).delete(delete_hook).fallback(|| async { method_not_allowed("PATCH") }),
        )
        .route("/v1/webhooks/{webhook_id}/test", post(test_hook).fallback(|| async { method_not_allowed("POST") }))
}

/// `{webhook_id}` as Starlette decodes it.
pub struct HookId(pub String);

impl<S: Send + Sync> axum::extract::FromRequestParts<S> for HookId {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        let path = crate::middleware::pyurl::scope_path(&parts.uri);
        let rest = path.strip_prefix(PREFIX).unwrap_or_default();
        Ok(HookId(rest.strip_suffix("/test").unwrap_or(rest).to_owned()))
    }
}

fn coll(state: &AppState) -> MongoColl {
    MongoColl::new(state.mongo(), model::COLLECTION)
}

fn reply(r: crate::error::ApiResult<Value>) -> Response {
    match r {
        Ok(v) => PydanticJson(v).into_response(),
        Err(e) => e.into_response(),
    }
}

async fn list_event_types(State(state): State<AppState>, _: RequireFeature<Webhooks>) -> PydanticJson {
    let asset = state.settings().asset_url();
    PydanticJson(json!({"events": model::EVENT_TYPES, "meta": embeds::event_meta(&asset)}))
}

async fn list_hooks(
    State(state): State<AppState>,
    _: RequireFeature<Webhooks>,
    CurrentSiteUser(user): CurrentSiteUser,
) -> Response {
    reply(service::list(&coll(&state), user.id).await)
}

async fn create_hook(
    State(state): State<AppState>,
    DepsThenBody((_, CurrentSiteUser(user)), req): DepsThenBody<
        (RequireFeature<Webhooks>, CurrentSiteUser),
        service::WebhookCreate,
    >,
) -> Response {
    match service::create(&coll(&state), user.id, &req, crate::dt::now()).await {
        Ok(v) => (StatusCode::CREATED, PydanticJson(v)).into_response(),
        Err(e) => e.into_response(),
    }
}

async fn update_hook(
    State(state): State<AppState>,
    HookId(id): HookId,
    DepsThenBody((_, CurrentSiteUser(user)), req): DepsThenBody<
        (RequireFeature<Webhooks>, CurrentSiteUser),
        service::WebhookUpdate,
    >,
) -> Response {
    reply(service::update(&coll(&state), user.id, &id, &req, crate::dt::now()).await)
}

async fn delete_hook(
    State(state): State<AppState>,
    HookId(id): HookId,
    _: RequireFeature<Webhooks>,
    CurrentSiteUser(user): CurrentSiteUser,
) -> Response {
    match service::delete(&coll(&state), user.id, &id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => e.into_response(),
    }
}

async fn test_hook(
    State(state): State<AppState>,
    HookId(id): HookId,
    _: RequireFeature<Webhooks>,
    CurrentSiteUser(user): CurrentSiteUser,
    uri: axum::http::Uri,
) -> Response {
    let event = crate::middleware::pyurl::query_param(&uri, "event");
    let asset = state.settings().asset_url();
    let poster = delivery::LivePoster(state.clone());
    reply(service::send_test(&coll(&state), &poster, user.id, &id, event.as_deref(), &asset).await)
}

//! Discord DM alert subscriptions (`app/dm_subs`): the User Dashboard CRUD
//! under `/v1/dm-subscriptions` (feature gate first, then the site session)
//! and the delivery-queue consumer.

pub mod delivery;
pub mod embeds;
pub mod model;
pub mod service;

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
    features::{DmSubs, RequireFeature},
    outbound::store::MongoColl,
    pyjson::PydanticJson,
    state::AppState,
    validation::DepsThenBody,
};

pub use delivery::{check_market, enqueue, start as start_jobs};

const PREFIX: &str = "/v1/dm-subscriptions/";

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/v1/dm-subscriptions/events",
            get(list_event_types).patch(update_sub).delete(delete_sub).fallback(|| async { method_not_allowed("GET") }),
        )
        .route("/v1/dm-subscriptions", get(list_subs).post(create_sub).fallback(|| async { method_not_allowed("GET") }))
        .route(
            "/v1/dm-subscriptions/{sub_id}",
            patch(update_sub).delete(delete_sub).fallback(|| async { method_not_allowed("PATCH") }),
        )
        .route("/v1/dm-subscriptions/{sub_id}/test", post(test_sub).fallback(|| async { method_not_allowed("POST") }))
}

/// `{sub_id}` as Starlette decodes it.
pub struct SubId(pub String);

impl<S: Send + Sync> axum::extract::FromRequestParts<S> for SubId {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        let path = crate::middleware::pyurl::scope_path(&parts.uri);
        let rest = path.strip_prefix(PREFIX).unwrap_or_default();
        Ok(SubId(rest.strip_suffix("/test").unwrap_or(rest).to_owned()))
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

async fn list_event_types(_: RequireFeature<DmSubs>) -> PydanticJson {
    PydanticJson(json!({"events": model::EVENT_TYPES, "challenge_types": model::CHALLENGE_TYPES}))
}

async fn list_subs(
    State(state): State<AppState>,
    _: RequireFeature<DmSubs>,
    CurrentSiteUser(user): CurrentSiteUser,
) -> Response {
    reply(service::list(&coll(&state), user.id, user.discord_id).await)
}

async fn create_sub(
    State(state): State<AppState>,
    DepsThenBody((_, CurrentSiteUser(user)), req): DepsThenBody<
        (RequireFeature<DmSubs>, CurrentSiteUser),
        service::DmSubCreate,
    >,
) -> Response {
    match service::create(&coll(&state), user.id, user.discord_id, &req, crate::dt::now()).await {
        Ok(v) => (StatusCode::CREATED, PydanticJson(v)).into_response(),
        Err(e) => e.into_response(),
    }
}

async fn update_sub(
    State(state): State<AppState>,
    SubId(id): SubId,
    DepsThenBody((_, CurrentSiteUser(user)), req): DepsThenBody<
        (RequireFeature<DmSubs>, CurrentSiteUser),
        service::DmSubUpdate,
    >,
) -> Response {
    reply(service::update(&coll(&state), user.id, user.discord_id, &id, &req, crate::dt::now()).await)
}

async fn delete_sub(
    State(state): State<AppState>,
    SubId(id): SubId,
    _: RequireFeature<DmSubs>,
    CurrentSiteUser(user): CurrentSiteUser,
) -> Response {
    match service::delete(&coll(&state), user.id, &id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => e.into_response(),
    }
}

async fn test_sub(
    State(state): State<AppState>,
    SubId(id): SubId,
    _: RequireFeature<DmSubs>,
    CurrentSiteUser(user): CurrentSiteUser,
) -> Response {
    reply(service::send_test(&coll(&state), &delivery::LiveDm::new(&state), user.id, &id).await)
}

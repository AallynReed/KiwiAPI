//! Creator tokens and creator <-> developer connections: the dev-portal half
//! at `/v1/mods/hub/creator-links` (portal session) and the Dashboard half at
//! `/site/mods/creator-token` and `/site/mods/creator-connections/{link_id}`.

pub mod service;

#[cfg(test)]
mod tests;

use axum::{
    Router,
    extract::State,
    http::{HeaderValue, StatusCode, header, request::Parts},
    response::{IntoResponse, Response},
    routing::{delete, get, patch, post},
};
use mongodb::bson::oid::ObjectId;
use serde_json::{Map, Value, json};

use crate::{
    auth::{CurrentSiteUser, CurrentUser, models::SiteUser},
    error::{ApiError, method_not_allowed},
    features::{ModsHub, RequireFeature},
    mods_write_auth::{site::SiteGate, store::Store},
    pyjson::{PyJson, PydanticJson},
    state::AppState,
    validation::{DepsThenBody, Field, FromBody},
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/v1/mods/hub/creator-links",
            get(list_links).post(add_link).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/v1/mods/hub/creator-links/{link_id}",
            delete(remove_link).fallback(|| async { method_not_allowed("DELETE") }),
        )
        .route(
            "/site/mods/creator-token",
            get(site_token).post(site_issue_token).fallback(|| async { method_not_allowed("GET") }),
        )
        .route(
            "/site/mods/creator-token/rotate",
            post(site_rotate_token).fallback(|| async { method_not_allowed("POST") }),
        )
        .route(
            "/site/mods/creator-connections/{link_id}",
            patch(site_set_scope).delete(site_revoke).fallback(|| async { method_not_allowed("PATCH") }),
        )
}

/// `{link_id}` as Starlette decodes it.
pub struct LinkId(pub String);

impl<S: Send + Sync> axum::extract::FromRequestParts<S> for LinkId {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        Ok(LinkId(crate::admin::path_param(parts, "link_id")))
    }
}

/// `CreatorLinkRequest`.
pub struct LinkBody {
    pub token: String,
    pub label: String,
}

impl FromBody for LinkBody {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| {
            Some(LinkBody {
                token: m.field("token").str().min_len(8).max_len(200).required(),
                label: m.field("label").str().max_len(60).default(String::new()),
            })
        })
        .required()
    }
}

/// `CreatorScopeRequest`.
pub struct ScopeBody {
    pub all_projects: bool,
    pub project_ids: Vec<String>,
}

impl FromBody for ScopeBody {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| {
            Some(ScopeBody {
                all_projects: m.field("all_projects").bool().default(true),
                project_ids: m.field("project_ids").list(|f| f.str().required()).max_len(500).default(Vec::new()),
            })
        })
        .required()
    }
}

fn no_store(v: Value) -> Response {
    let mut resp = PyJson(v).into_response();
    resp.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    resp
}

fn reply(r: Result<Response, ApiError>) -> Response {
    r.unwrap_or_else(IntoResponse::into_response)
}

fn obj(v: Value) -> Map<String, Value> {
    match v {
        Value::Object(m) => m,
        _ => Map::new(),
    }
}

/// `GET /v1/mods/hub/creator-links`.
pub async fn list_op(store: &Store, api_user_id: ObjectId) -> Response {
    reply(
        service::list_for_developer(store, api_user_id)
            .await
            .map(|items| PydanticJson(json!({"items": items})).into_response()),
    )
}

/// `POST /v1/mods/hub/creator-links`.
pub async fn connect_op(store: &Store, api_user_id: ObjectId, req: &LinkBody) -> Response {
    reply(
        service::connect(store, api_user_id, &req.token, &req.label)
            .await
            .map(|v| (StatusCode::CREATED, PydanticJson(v)).into_response()),
    )
}

/// `DELETE /v1/mods/hub/creator-links/{link_id}`.
pub async fn disconnect_op(store: &Store, api_user_id: ObjectId, link_id: &str) -> Response {
    reply(
        service::disconnect_by_developer(store, api_user_id, link_id)
            .await
            .map(|()| StatusCode::NO_CONTENT.into_response()),
    )
}

/// `GET /site/mods/creator-token`: token state, connections and the mod picker.
pub async fn panel_op(store: &Store, user: &SiteUser) -> Response {
    let out = async {
        let mut out = obj(service::token_dto(user));
        out.insert("connections".into(), Value::Array(service::list_for_creator(store, user).await?));
        out.insert("mods".into(), Value::Array(service::owned_cards(store, user).await?));
        Ok(no_store(Value::Object(out)))
    };
    reply(out.await)
}

fn with_token(dto: Value, raw: String) -> Response {
    let mut m = obj(dto);
    m.insert("token".into(), Value::String(raw));
    no_store(Value::Object(m))
}

/// `POST /site/mods/creator-token`: the plaintext exactly once.
pub async fn issue_op(store: &Store, user: &mut SiteUser) -> Response {
    let out = async {
        match service::ensure_token(store, user).await? {
            (dto, Some(raw)) => Ok(with_token(dto, raw)),
            (_, None) => Err(ApiError::http(
                StatusCode::CONFLICT,
                Some(
                    "You already have a creator token. Rotate it to get a new one (that disconnects any API accounts \
                     using the old one).",
                ),
            )),
        }
    };
    reply(out.await)
}

/// `POST /site/mods/creator-token/rotate`.
pub async fn rotate_op(store: &Store, user: &mut SiteUser) -> Response {
    reply(service::rotate_token(store, user).await.map(|(dto, raw)| with_token(dto, raw)))
}

/// `PATCH /site/mods/creator-connections/{link_id}`.
pub async fn scope_op(store: &Store, user: &SiteUser, link_id: &str, req: &ScopeBody) -> Response {
    reply(service::set_scope(store, user, link_id, req.all_projects, &req.project_ids).await.map(no_store))
}

/// `DELETE /site/mods/creator-connections/{link_id}`.
pub async fn revoke_op(store: &Store, user: &SiteUser, link_id: &str) -> Response {
    reply(service::revoke_by_creator(store, user, link_id).await.map(|()| StatusCode::NO_CONTENT.into_response()))
}

async fn list_links(
    State(state): State<AppState>,
    _: RequireFeature<ModsHub>,
    CurrentUser(user): CurrentUser,
) -> Response {
    list_op(&Store::live(&state), user.id).await
}

async fn add_link(
    State(state): State<AppState>,
    DepsThenBody((_, CurrentUser(user)), req): DepsThenBody<(RequireFeature<ModsHub>, CurrentUser), LinkBody>,
) -> Response {
    connect_op(&Store::live(&state), user.id, &req).await
}

async fn remove_link(
    State(state): State<AppState>,
    LinkId(id): LinkId,
    _: RequireFeature<ModsHub>,
    CurrentUser(user): CurrentUser,
) -> Response {
    disconnect_op(&Store::live(&state), user.id, &id).await
}

async fn site_token(State(state): State<AppState>, _: SiteGate, CurrentSiteUser(user): CurrentSiteUser) -> Response {
    panel_op(&Store::live(&state), &user).await
}

async fn site_issue_token(
    State(state): State<AppState>,
    _: SiteGate,
    CurrentSiteUser(mut user): CurrentSiteUser,
) -> Response {
    issue_op(&Store::live(&state), &mut user).await
}

async fn site_rotate_token(
    State(state): State<AppState>,
    _: SiteGate,
    CurrentSiteUser(mut user): CurrentSiteUser,
) -> Response {
    rotate_op(&Store::live(&state), &mut user).await
}

async fn site_set_scope(
    State(state): State<AppState>,
    LinkId(id): LinkId,
    DepsThenBody((_, CurrentSiteUser(user)), req): DepsThenBody<(SiteGate, CurrentSiteUser), ScopeBody>,
) -> Response {
    scope_op(&Store::live(&state), &user, &id, &req).await
}

async fn site_revoke(
    State(state): State<AppState>,
    LinkId(id): LinkId,
    _: SiteGate,
    CurrentSiteUser(user): CurrentSiteUser,
) -> Response {
    revoke_op(&Store::live(&state), &user, &id).await
}

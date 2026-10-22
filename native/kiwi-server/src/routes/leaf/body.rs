//! A JSON body behind dependencies, in FastAPI's order: the body is read and
//! decoded first (a syntax error is a 422 before any dependency runs), then
//! the dependency extractor, then the body is validated.

use axum::{
    extract::{FromRequest, FromRequestParts, Request},
    response::{IntoResponse, Response},
};

use crate::{
    error::ApiError,
    state::AppState,
    validation::{BodyInput, Field, FromBody, Validator, read_body},
};

pub struct DepsThenBody<D, B>(pub D, pub B);

impl<D, B> FromRequest<AppState> for DepsThenBody<D, B>
where
    D: FromRequestParts<AppState> + Send,
    D::Rejection: IntoResponse,
    B: FromBody + Send,
{
    type Rejection = Response;

    async fn from_request(req: Request, state: &AppState) -> Result<Self, Response> {
        let (mut parts, body) = req.into_parts();
        let body = read_body(&parts.headers, body).await.map_err(IntoResponse::into_response)?;
        let deps = D::from_request_parts(&mut parts, state).await.map_err(IntoResponse::into_response)?;
        let value = validate::<B>(&body).map_err(IntoResponse::into_response)?;
        Ok(DepsThenBody(deps, value))
    }
}

/// Validates an already-read body (`loc = ["body", ...]`).
pub fn validate<B: FromBody>(body: &BodyInput) -> Result<B, ApiError> {
    let mut v = Validator::default();
    let value = B::from_body(Field::new(&mut v, vec!["body".into()], body.input()));
    v.finish(value)?.ok_or_else(ApiError::internal)
}

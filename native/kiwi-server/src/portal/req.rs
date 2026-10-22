//! FastAPI's evaluation order for a route: the body is read and decoded first
//! (a JSON syntax error is a 422 before anything else), then the dependency
//! runs (its 401 wins), then parameter and body errors are reported together.

use axum::{
    extract::{FromRequestParts, RawPathParams, Request},
    http::request::Parts,
    response::{IntoResponse, Response},
};

use crate::{
    error::ApiError,
    state::AppState,
    validation::{BodyInput, Field, FromBody, FromParams, Params, read_body},
};

/// A route without path/query parameters.
pub struct NoParams;

impl FromParams for NoParams {
    fn from_params(_: &mut Params<'_>) -> Self {
        NoParams
    }
}

async fn params_of(parts: &mut Parts, state: &AppState) -> (Vec<(String, String)>, Option<String>) {
    let path = match RawPathParams::from_request_parts(parts, state).await {
        Ok(raw) => raw.iter().map(|(k, v)| (k.to_owned(), v.to_owned())).collect(),
        Err(_) => Vec::new(),
    };
    (path, parts.uri.query().map(str::to_owned))
}

fn finish<P, B: FromBody>(mut p: Params<'_>, params: P, body: Option<&BodyInput>) -> Result<(P, Option<B>), ApiError> {
    let mut v = std::mem::take(&mut p.v);
    let value = body.map(|b| B::from_body(Field::new(&mut v, vec!["body".into()], b.input())));
    match v.finish(value)? {
        Some(Some(b)) => Ok((params, Some(b))),
        Some(None) => Err(ApiError::internal()),
        None => Ok((params, None)),
    }
}

/// Dependency `D`, parameters `P` and a JSON body `B`.
pub async fn with_body<D, P, B>(state: &AppState, req: Request) -> Result<(D, P, B, Parts), Response>
where
    D: FromRequestParts<AppState>,
    D::Rejection: IntoResponse,
    P: FromParams,
    B: FromBody,
{
    with_body_at(state, req, None).await
}

/// [`with_body`] for a literal route standing in for a `{param}` one:
/// `path` gives the values Starlette would have captured.
pub async fn with_body_at<D, P, B>(
    state: &AppState,
    req: Request,
    path: Option<Vec<(String, String)>>,
) -> Result<(D, P, B, Parts), Response>
where
    D: FromRequestParts<AppState>,
    D::Rejection: IntoResponse,
    P: FromParams,
    B: FromBody,
{
    let (mut parts, body) = req.into_parts();
    let body = read_body(&parts.headers, body).await.map_err(IntoResponse::into_response)?;
    let dep = D::from_request_parts(&mut parts, state).await.map_err(IntoResponse::into_response)?;
    let (captured, query) = params_of(&mut parts, state).await;
    let path = path.unwrap_or(captured);
    let mut p = Params::new(path, query.as_deref(), &parts.headers);
    let params = P::from_params(&mut p);
    let (params, body) = finish::<P, B>(p, params, Some(&body)).map_err(IntoResponse::into_response)?;
    let body = body.ok_or_else(|| ApiError::internal().into_response())?;
    Ok((dep, params, body, parts))
}

/// Dependency `D` and parameters `P`; the body is never read.
pub async fn without_body<D, P>(state: &AppState, req: Request) -> Result<(D, P, Parts), Response>
where
    D: FromRequestParts<AppState>,
    D::Rejection: IntoResponse,
    P: FromParams,
{
    without_body_at(state, req, None).await
}

pub async fn without_body_at<D, P>(
    state: &AppState,
    req: Request,
    path: Option<Vec<(String, String)>>,
) -> Result<(D, P, Parts), Response>
where
    D: FromRequestParts<AppState>,
    D::Rejection: IntoResponse,
    P: FromParams,
{
    let (mut parts, _) = req.into_parts();
    let dep = D::from_request_parts(&mut parts, state).await.map_err(IntoResponse::into_response)?;
    let (captured, query) = params_of(&mut parts, state).await;
    let path = path.unwrap_or(captured);
    let mut p = Params::new(path, query.as_deref(), &parts.headers);
    let params = P::from_params(&mut p);
    let (params, _) = finish::<P, NoBody>(p, params, None).map_err(IntoResponse::into_response)?;
    Ok((dep, params, parts))
}

struct NoBody;

impl FromBody for NoBody {
    fn from_body(_: Field<'_>) -> Option<Self> {
        Some(NoBody)
    }
}

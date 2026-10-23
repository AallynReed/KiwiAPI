//! `HeadMethodMiddleware`: HEAD runs the GET route (axum drops the body at the
//! top), except under `/v1/events`, where it would open an endless SSE stream:
//! a route there answers with the GET route's 405, anything else is routed as
//! the HEAD it is.

use axum::{
    extract::{MatchedPath, Request},
    http::{HeaderValue, Method, header},
    middleware::Next,
    response::Response,
};

use super::errors::envelope;
use super::pyurl::scope_path;

pub async fn head_as_get(mut req: Request, next: Next) -> Response {
    if req.method() == Method::HEAD {
        if scope_path(req.uri()).starts_with("/v1/events") {
            if req.extensions().get::<MatchedPath>().is_none() {
                return next.run(req).await;
            }
            let mut res = envelope(405, "method_not_allowed", "Method Not Allowed");
            res.headers_mut().insert(header::ALLOW, HeaderValue::from_static("GET"));
            return res;
        }
        *req.method_mut() = Method::GET;
    }
    next.run(req).await
}

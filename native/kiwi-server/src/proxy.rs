//! Streaming reverse proxy to the Python app for every route not ported yet.
//! Bodies stream both ways (uploads, SSE, git smart-HTTP), the Host header is
//! kept (Python's api-host redirect reads it) and X-Forwarded-For/-Proto carry
//! the client this server resolved.

use std::{
    net::SocketAddr,
    time::{Duration, Instant},
};

use axum::{
    body::Body,
    extract::{ConnectInfo, Request, State},
    http::{HeaderName, HeaderValue, Method, StatusCode, Uri, header},
    response::{IntoResponse, Response},
};

use crate::middleware::{forwarded, pyurl::header_value};
use crate::state::AppState;

const RESTART_GRACE: Duration = Duration::from_secs(20);

const HOP_BY_HOP: [&str; 8] = [
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];

fn strip_hop_by_hop(headers: &mut axum::http::HeaderMap) {
    let listed: Vec<HeaderName> = headers
        .get_all(header::CONNECTION)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .filter_map(|n| HeaderName::from_bytes(n.trim().as_bytes()).ok())
        .collect();
    for name in listed {
        headers.remove(name);
    }
    for name in HOP_BY_HOP {
        headers.remove(name);
    }
}

pub async fn proxy(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    req: Request,
) -> Response {
    let (mut parts, body) = req.into_parts();
    let path_and_query = parts.uri.path_and_query().map(|p| p.as_str()).unwrap_or("/");
    let Ok(uri) = format!("{}{}", state.settings().py_upstream, path_and_query).parse::<Uri>() else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    parts.uri = uri;
    parts.version = axum::http::Version::HTTP_11;
    strip_hop_by_hop(&mut parts.headers);

    // Resolve the client the way uvicorn would (FORWARDED_ALLOW_IPS), then hand
    // Python only that answer; api-py trusts us with --forwarded-allow-ips '*'.
    let client = forwarded::resolve(forwarded::trusted(), Some(peer), &parts.headers);
    parts.headers.remove("x-forwarded-for");
    parts.headers.insert("x-forwarded-for", header_value(&client.host));
    parts.headers.insert("x-forwarded-proto", HeaderValue::from_static(client.scheme));

    // While api-py restarts nothing is listening: hold safe requests until it is back.
    let result = if matches!(parts.method, Method::GET | Method::HEAD | Method::OPTIONS) {
        let deadline = Instant::now() + RESTART_GRACE;
        loop {
            let res = state.inner.upstream.request(Request::from_parts(parts.clone(), Body::empty())).await;
            match res {
                Err(err) if err.is_connect() && Instant::now() < deadline => {
                    tokio::time::sleep(Duration::from_millis(250)).await
                }
                res => break res,
            }
        }
    } else {
        state.inner.upstream.request(Request::from_parts(parts, body)).await
    };
    match result {
        Ok(resp) => {
            let (mut parts, body) = resp.into_parts();
            strip_hop_by_hop(&mut parts.headers);
            Response::from_parts(parts, Body::new(body))
        }
        Err(err) => {
            tracing::warn!("upstream request failed: {err}");
            (
                StatusCode::BAD_GATEWAY,
                [(header::CONTENT_TYPE, "application/json")],
                r#"{"error":{"code":"bad_gateway","message":"Upstream unavailable.","details":null,"request_id":null}}"#,
            )
                .into_response()
        }
    }
}

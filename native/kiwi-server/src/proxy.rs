//! Streaming reverse proxy to the Python app for every route not ported yet.
//! Bodies stream both ways (uploads, SSE, git smart-HTTP), the Host header is
//! kept (Python's api-host redirect reads it) and the peer is appended to
//! X-Forwarded-For like any proxy hop.

use std::net::SocketAddr;

use axum::{
    body::Body,
    extract::{ConnectInfo, Request, State},
    http::{header, HeaderName, HeaderValue, StatusCode, Uri},
    response::{IntoResponse, Response},
};

use crate::state::AppState;

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

    let peer_ip = peer.ip().to_string();
    let forwarded = match parts.headers.get("x-forwarded-for").and_then(|v| v.to_str().ok()) {
        Some(prev) if !prev.trim().is_empty() => format!("{prev}, {peer_ip}"),
        _ => peer_ip,
    };
    if let Ok(v) = HeaderValue::from_str(&forwarded) {
        parts.headers.insert("x-forwarded-for", v);
    }

    match state.inner.upstream.request(Request::from_parts(parts, body)).await {
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

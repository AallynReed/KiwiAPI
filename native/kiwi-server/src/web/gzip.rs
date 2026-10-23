//! `StaticCompressionMiddleware`: Starlette 1.9's `GZipMiddleware(minimum_size=1024)`
//! (level 9) on paths ending in a text-asset suffix, whatever the route.
//!
//! In the Python app every response reaches it through `BaseHTTPMiddleware`,
//! which re-sends each non-empty body message with `more_body=True` and then
//! an empty closing one. So GZip always takes its streaming path: a body with
//! any bytes gets `Vary: Accept-Encoding` (with or without gzip) and, when the
//! client accepts gzip, is compressed frame by frame (`Z_SYNC_FLUSH`, then a
//! final `Z_FINISH`) without Content-Length. An empty body, and the unhandled
//! 500 the request-id layer writes itself, pass untouched.

use std::pin::Pin;
use std::task::{Context, Poll};

use axum::{
    body::Body,
    extract::Request,
    http::{HeaderValue, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use bytes::Bytes;
use http_body::{Frame, SizeHint};
use http_body_util::BodyExt;

use crate::middleware::headers::add_vary_header;
use crate::middleware::pyurl::{first, latin1, scope_path};

const SUFFIXES: [&str; 7] = [".css", ".js", ".json", ".svg", ".txt", ".map", ".md"];
const LEVEL: i32 = 9;
const EXCLUDED: [&str; 14] = [
    "application/grpc",
    "application/gzip",
    "application/x-gzip",
    "application/zip",
    "audio/*",
    "font/woff",
    "font/woff2",
    "image/avif",
    "image/gif",
    "image/jpeg",
    "image/png",
    "image/webp",
    "text/event-stream",
    "video/*",
];

fn excluded(res: &Response) -> bool {
    let h = res.headers();
    if h.contains_key(header::CONTENT_ENCODING) || res.status() == StatusCode::PARTIAL_CONTENT {
        return true;
    }
    let ct = h.get(header::CONTENT_TYPE).map(|v| latin1(v.as_bytes())).unwrap_or_default();
    let media = ct.split(';').next().unwrap_or("").trim().to_lowercase();
    let family = format!("{}/*", media.split('/').next().unwrap_or(""));
    let grpc = media.starts_with("application/grpc+");
    EXCLUDED.iter().any(|e| *e == media || *e == family || (grpc && *e == "application/grpc"))
}

/// The gzip stream with Linux's header (OS byte 3).
struct Gz {
    z: kiwi_zlib::Deflate,
    started: bool,
}

impl Gz {
    fn new() -> Option<Self> {
        Some(Gz { z: kiwi_zlib::compressobj(LEVEL, 31, 8, kiwi_zlib::Z_DEFAULT_STRATEGY)?, started: false })
    }

    fn chunk(&mut self, data: &[u8], last: bool) -> Option<Vec<u8>> {
        let mut out = self.z.compress(data)?;
        out.extend(self.z.flush(if last { kiwi_zlib::Z_FINISH } else { kiwi_zlib::Z_SYNC_FLUSH })?);
        if !self.started && out.len() >= 10 {
            out[9] = 3;
            self.started = true;
        }
        Some(out)
    }
}

async fn next_bytes(body: &mut Body) -> Result<Option<Bytes>, axum::Error> {
    while let Some(frame) = body.frame().await {
        if let Ok(d) = frame?.into_data()
            && !d.is_empty()
        {
            return Ok(Some(d));
        }
    }
    Ok(None)
}

pub async fn static_compression(req: Request, next: Next) -> Response {
    let path = scope_path(req.uri());
    if !SUFFIXES.iter().any(|s| path.ends_with(s)) {
        return next.run(req).await;
    }
    let gzip = first(req.headers(), "accept-encoding").is_some_and(|v| v.contains("gzip"));
    let res = next.run(req).await;
    if excluded(&res) || res.extensions().get::<crate::request_id::SelfWritten>().is_some() {
        return res;
    }
    let (mut parts, mut body) = res.into_parts();
    if let Some(n) = http_body::Body::size_hint(&body).exact()
        && !parts.headers.contains_key(header::CONTENT_LENGTH)
    {
        parts.headers.insert(header::CONTENT_LENGTH, HeaderValue::from(n));
    }
    let first_frame = match next_bytes(&mut body).await {
        Ok(Some(b)) => b,
        Ok(None) => return Response::from_parts(parts, Body::empty()),
        Err(_) => return crate::error::ApiError::internal().into_response(),
    };
    add_vary_header(&mut parts.headers, "Accept-Encoding");
    let gz = if gzip {
        let Some(g) = Gz::new() else { return crate::error::ApiError::internal().into_response() };
        parts.headers.insert(header::CONTENT_ENCODING, HeaderValue::from_static("gzip"));
        parts.headers.remove(header::CONTENT_LENGTH);
        Some(g)
    } else {
        None
    };
    Response::from_parts(parts, Body::new(Streamed { inner: body, pending: Some(first_frame), done: false, gz }))
}

pin_project_lite::pin_project! {
    struct Streamed {
        #[pin]
        inner: Body,
        pending: Option<Bytes>,
        done: bool,
        gz: Option<Gz>,
    }
}

impl http_body::Body for Streamed {
    type Data = Bytes;
    type Error = axum::Error;

    fn poll_frame(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Bytes>, axum::Error>>> {
        let mut this = self.project();
        loop {
            if *this.done {
                return Poll::Ready(None);
            }
            let data = match this.pending.take() {
                Some(d) => Some(d),
                None => match std::task::ready!(this.inner.as_mut().poll_frame(cx)) {
                    Some(Ok(frame)) => match frame.into_data() {
                        Ok(d) if !d.is_empty() => Some(d),
                        _ => continue,
                    },
                    Some(Err(e)) => return Poll::Ready(Some(Err(e))),
                    None => None,
                },
            };
            let Some(gz) = this.gz.as_mut() else {
                match data {
                    Some(d) => return Poll::Ready(Some(Ok(Frame::data(d)))),
                    None => {
                        *this.done = true;
                        return Poll::Ready(None);
                    }
                }
            };
            let last = data.is_none();
            *this.done = last;
            return match gz.chunk(data.as_deref().unwrap_or(&[]), last) {
                Some(out) => Poll::Ready(Some(Ok(Frame::data(Bytes::from(out))))),
                None => Poll::Ready(Some(Err(axum::Error::new("deflate failed")))),
            };
        }
    }

    fn is_end_stream(&self) -> bool {
        self.done
    }

    fn size_hint(&self) -> SizeHint {
        SizeHint::default()
    }
}

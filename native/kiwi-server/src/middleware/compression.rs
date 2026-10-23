//! `ResponseCompressionMiddleware`. In the Python app every response reaches
//! it through `BaseHTTPMiddleware`, i.e. as a stream, so the stream path is
//! the one that runs: a compressible type with no declared length or one of
//! 1024+ bytes is gzipped incrementally (zlib's gzip wrapper, level 5, mtime 0,
//! OS byte 3 as on Linux), Content-Length dropped, `Vary: Accept-Encoding`
//! merged. Everything else passes untouched, without that Vary.
//!
//! The deflate stream is classic zlib's (kiwi-zlib): zlib-rs picks different
//! matches at level 5, so its bytes differ from CPython's.

use std::pin::Pin;
use std::task::{Context, Poll};

use axum::{
    body::Body,
    extract::Request,
    http::{HeaderMap, HeaderValue, header},
    middleware::Next,
    response::Response,
};
use bytes::Bytes;
use http_body::{Body as _, Frame, SizeHint};

use super::headers::add_vary;

const MIN_BYTES: u64 = 1024;
const LEVEL: i32 = 5;
const GZIP_HEADER: [u8; 10] = [0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 3];

const COMPRESSIBLE: [&str; 13] = [
    "application/javascript",
    "application/json",
    "application/manifest+json",
    "application/x-ndjson",
    "application/xml",
    "image/svg+xml",
    "text/css",
    "text/csv",
    "text/html",
    "text/javascript",
    "text/markdown",
    "text/plain",
    "text/xml",
];

fn accepts_gzip(headers: &HeaderMap) -> bool {
    headers
        .get(header::ACCEPT_ENCODING)
        .is_some_and(|v| v.as_bytes().to_ascii_lowercase().windows(4).any(|w| w == b"gzip"))
}

fn is_compressible(res: &Response) -> bool {
    let status = res.status().as_u16();
    if status < 200 || matches!(status, 204 | 206 | 304) {
        return false;
    }
    let h = res.headers();
    if h.contains_key(header::CONTENT_ENCODING) {
        return false;
    }
    if h.get(header::CONTENT_RANGE).is_some_and(|v| !v.is_empty()) {
        return false;
    }
    let media = h.get(header::CONTENT_TYPE).map(|v| super::pyurl::latin1(v.as_bytes())).unwrap_or_default();
    let media = media.split(';').next().unwrap_or("").trim().to_lowercase();
    if !COMPRESSIBLE.contains(&media.as_str()) {
        return false;
    }
    let declared = match h.get(header::CONTENT_LENGTH) {
        Some(v) => {
            let s = v.as_bytes();
            if !s.is_empty() && s.iter().all(u8::is_ascii_digit) {
                std::str::from_utf8(s).ok().and_then(|s| s.parse::<u64>().ok())
            } else {
                None
            }
        }
        None => res.body().size_hint().exact(),
    };
    !declared.is_some_and(|n| n < MIN_BYTES)
}

pub async fn compression(req: Request, next: Next) -> Response {
    if !accepts_gzip(req.headers()) {
        return next.run(req).await;
    }
    let res = next.run(req).await;
    if !is_compressible(&res) {
        return res;
    }
    let Some(z) = kiwi_zlib::compressobj(LEVEL, -15, 8, kiwi_zlib::Z_DEFAULT_STRATEGY) else { return res };
    let (mut parts, body) = res.into_parts();
    parts.headers.insert(header::CONTENT_ENCODING, HeaderValue::from_static("gzip"));
    add_vary(&mut parts.headers, &["Accept-Encoding"]);
    parts.headers.remove(header::CONTENT_LENGTH);
    Response::from_parts(parts, Body::new(GzipBody { inner: body, z, crc: 0, len: 0, started: false, done: false }))
}

pin_project_lite::pin_project! {
    struct GzipBody {
        #[pin]
        inner: Body,
        z: kiwi_zlib::Deflate,
        crc: u32,
        len: u64,
        started: bool,
        done: bool,
    }
}

impl http_body::Body for GzipBody {
    type Data = Bytes;
    type Error = axum::Error;

    fn poll_frame(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Bytes>, axum::Error>>> {
        let mut this = self.project();
        loop {
            if *this.done {
                return Poll::Ready(None);
            }
            let polled = std::task::ready!(this.inner.as_mut().poll_frame(cx));
            let mut out = Vec::new();
            if !*this.started {
                out.extend_from_slice(&GZIP_HEADER);
            }
            match polled {
                Some(Ok(frame)) => {
                    let Ok(data) = frame.into_data() else { continue };
                    let mut crc = crc32fast::Hasher::new_with_initial(*this.crc);
                    crc.update(&data);
                    *this.crc = crc.finalize();
                    *this.len += data.len() as u64;
                    let Some(z) = this.z.compress(&data) else {
                        return Poll::Ready(Some(Err(axum::Error::new("deflate failed"))));
                    };
                    out.extend(z);
                    if !out.is_empty() {
                        *this.started = true;
                        return Poll::Ready(Some(Ok(Frame::data(out.into()))));
                    }
                }
                Some(Err(e)) => return Poll::Ready(Some(Err(e))),
                None => {
                    *this.started = true;
                    *this.done = true;
                    let Some(z) = this.z.flush(kiwi_zlib::Z_FINISH) else {
                        return Poll::Ready(Some(Err(axum::Error::new("deflate failed"))));
                    };
                    out.extend(z);
                    out.extend_from_slice(&this.crc.to_le_bytes());
                    out.extend_from_slice(&(*this.len as u32).to_le_bytes());
                    return Poll::Ready(Some(Ok(Frame::data(out.into()))));
                }
            }
        }
    }

    fn is_end_stream(&self) -> bool {
        self.done
    }

    fn size_hint(&self) -> SizeHint {
        SizeHint::default()
    }
}

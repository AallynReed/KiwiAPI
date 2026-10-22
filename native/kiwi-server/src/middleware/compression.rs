//! `ResponseCompressionMiddleware`. In the Python app every response reaches
//! it through `BaseHTTPMiddleware`, i.e. as a stream, so the stream path is
//! the one that runs: a compressible type with no declared length or one of
//! 1024+ bytes is gzipped incrementally (zlib's gzip wrapper, level 5, mtime 0,
//! OS byte 3 as on Linux), Content-Length dropped, `Vary: Accept-Encoding`
//! merged. Everything else passes untouched, without that Vary.
//!
//! The deflate stream comes from zlib itself (libz-sys, built from source):
//! zlib-rs picks different matches at level 5, so its bytes differ from
//! CPython's.

use std::os::raw::{c_int, c_void};
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
use libz_sys as z;

use super::headers::add_vary;

const MIN_BYTES: u64 = 1024;
const LEVEL: c_int = 5;
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
    let Some(z) = Deflate::new() else { return res };
    let (mut parts, body) = res.into_parts();
    parts.headers.insert(header::CONTENT_ENCODING, HeaderValue::from_static("gzip"));
    add_vary(&mut parts.headers, &["Accept-Encoding"]);
    parts.headers.remove(header::CONTENT_LENGTH);
    Response::from_parts(parts, Body::new(GzipBody { inner: body, z, crc: 0, len: 0, started: false, done: false }))
}

unsafe extern "C" fn zalloc(_: *mut c_void, items: z::uInt, size: z::uInt) -> *mut c_void {
    unsafe { libc_calloc(items as usize, size as usize) }
}

unsafe extern "C" fn zfree(_: *mut c_void, ptr: *mut c_void) {
    unsafe { libc_free(ptr) }
}

unsafe extern "C" {
    #[link_name = "calloc"]
    fn libc_calloc(n: usize, size: usize) -> *mut c_void;
    #[link_name = "free"]
    fn libc_free(ptr: *mut c_void);
}

/// A raw deflate stream (`compressobj(5, DEFLATED, -15)`).
struct Deflate(Box<z::z_stream>);

// The stream is owned by one body and zlib keeps no thread-local state.
unsafe impl Send for Deflate {}

impl Deflate {
    fn new() -> Option<Self> {
        let mut strm = Box::new(z::z_stream {
            next_in: std::ptr::null_mut(),
            avail_in: 0,
            total_in: 0,
            next_out: std::ptr::null_mut(),
            avail_out: 0,
            total_out: 0,
            msg: std::ptr::null_mut(),
            state: std::ptr::null_mut(),
            zalloc,
            zfree,
            opaque: std::ptr::null_mut(),
            data_type: 0,
            adler: 0,
            reserved: 0,
        });
        let rc = unsafe {
            z::deflateInit2_(
                &mut *strm,
                LEVEL,
                z::Z_DEFLATED,
                -15,
                8,
                z::Z_DEFAULT_STRATEGY,
                z::zlibVersion(),
                std::mem::size_of::<z::z_stream>() as c_int,
            )
        };
        (rc == z::Z_OK).then_some(Deflate(strm))
    }

    /// Python `compress(data)` (finish = false) or `flush(Z_FINISH)`.
    fn run(&mut self, input: &[u8], finish: bool, out: &mut Vec<u8>) -> bool {
        let flush = if finish { z::Z_FINISH } else { z::Z_NO_FLUSH };
        let mut rest = input;
        loop {
            let (chunk, tail) = rest.split_at(rest.len().min(1 << 30));
            rest = tail;
            self.0.next_in = chunk.as_ptr() as *mut _;
            self.0.avail_in = chunk.len() as z::uInt;
            loop {
                out.reserve(chunk.len() / 2 + 4096);
                let spare = (out.capacity() - out.len()).min(u32::MAX as usize);
                self.0.next_out = unsafe { out.as_mut_ptr().add(out.len()) };
                self.0.avail_out = spare as z::uInt;
                let rc = unsafe { z::deflate(&mut *self.0, flush) };
                let written = spare - self.0.avail_out as usize;
                unsafe { out.set_len(out.len() + written) };
                match rc {
                    z::Z_STREAM_END => break,
                    z::Z_OK | z::Z_BUF_ERROR => {}
                    _ => return false,
                }
                if !finish && self.0.avail_in == 0 && self.0.avail_out != 0 {
                    break;
                }
            }
            if rest.is_empty() {
                break;
            }
        }
        self.0.next_in = std::ptr::null_mut();
        self.0.next_out = std::ptr::null_mut();
        true
    }
}

impl Drop for Deflate {
    fn drop(&mut self) {
        unsafe { z::deflateEnd(&mut *self.0) };
    }
}

// zlib's checksum type is c_ulong: u32 on Windows, u64 on Linux.
#[allow(clippy::useless_conversion, clippy::unnecessary_cast)]
fn crc32(crc: u32, data: &[u8]) -> u32 {
    let mut crc = std::os::raw::c_ulong::from(crc);
    for chunk in data.chunks(1 << 30) {
        crc = unsafe { z::crc32(crc, chunk.as_ptr(), chunk.len() as z::uInt) };
    }
    crc as u32
}

pin_project_lite::pin_project! {
    struct GzipBody {
        #[pin]
        inner: Body,
        z: Deflate,
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
                    *this.crc = crc32(*this.crc, &data);
                    *this.len += data.len() as u64;
                    if !this.z.run(&data, false, &mut out) {
                        return Poll::Ready(Some(Err(axum::Error::new("deflate failed"))));
                    }
                    if !out.is_empty() {
                        *this.started = true;
                        return Poll::Ready(Some(Ok(Frame::data(out.into()))));
                    }
                }
                Some(Err(e)) => return Poll::Ready(Some(Err(e))),
                None => {
                    *this.started = true;
                    *this.done = true;
                    if !this.z.run(&[], true, &mut out) {
                        return Poll::Ready(Some(Err(axum::Error::new("deflate failed"))));
                    }
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

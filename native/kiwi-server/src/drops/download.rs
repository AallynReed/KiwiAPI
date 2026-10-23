//! Starlette 1.9 `FileResponse` for the master download: `Accept-Ranges`,
//! the attachment name, Content-Length / Last-Modified / ETag from the file's
//! stat, and `Range` (honoured only on a 200, gated by `If-Range`).

use std::{
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use axum::{
    body::Body,
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use bytes::Bytes;
use chrono::DateTime;
use futures::StreamExt;
use kiwi_core::drops::{RangeError, byterange_header, byteranges_length, content_disposition, parse_range_header};
use md5::{Digest, Md5};
use tokio::io::{AsyncReadExt, AsyncSeekExt};

use crate::{error::ApiError, pyfloat, request_id};

const CHUNK: usize = 64 * 1024;

/// `os.stat(path).st_mtime`: `sec + nsec * 1e-9` as a double.
pub fn st_mtime(modified: SystemTime) -> f64 {
    match modified.duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_secs() as f64 + f64::from(d.subsec_nanos()) * 1e-9,
        Err(e) => {
            let d = e.duration();
            -(d.as_secs() as f64) - f64::from(d.subsec_nanos()) * 1e-9
        }
    }
}

/// `FileResponse.set_stat_headers`' ETag and Last-Modified.
pub fn stat_headers(mtime: f64, size: u64) -> (String, String) {
    let base = format!("{}-{size}", pyfloat::repr(mtime));
    let etag = format!("\"{}\"", hex::encode(Md5::digest(base.as_bytes())));
    let secs = mtime.floor() as i64;
    let last_modified = DateTime::from_timestamp(secs, 0)
        .map(|d| d.format("%a, %d %b %Y %H:%M:%S GMT").to_string())
        .unwrap_or_default();
    (etag, last_modified)
}

/// The `media_type` header value: text types get `; charset=utf-8`.
pub fn content_type(media_type: &str) -> String {
    if media_type.starts_with("text/") && !media_type.to_lowercase().contains("charset=") {
        format!("{media_type}; charset=utf-8")
    } else {
        media_type.to_owned()
    }
}

fn hv(s: &str) -> Result<HeaderValue, ApiError> {
    let bytes: Vec<u8> = s.chars().map(|c| c as u32 as u8).collect();
    HeaderValue::from_bytes(&bytes).map_err(|_| ApiError::internal())
}

fn plain(status: StatusCode, body: &'static str, extra: Option<(HeaderName, String)>) -> Response {
    let mut res = Response::new(Body::from(body));
    *res.status_mut() = status;
    let h = res.headers_mut();
    if let Some((name, value)) = extra
        && let Ok(v) = HeaderValue::from_str(&value)
    {
        h.insert(name, v);
    }
    h.insert(header::CONTENT_LENGTH, HeaderValue::from(body.len()));
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain; charset=utf-8"));
    res
}

fn read_range(path: std::path::PathBuf, start: u64, end: u64) -> impl futures::Stream<Item = std::io::Result<Bytes>> {
    futures::stream::unfold((None::<tokio::fs::File>, path, start), move |(file, path, pos)| async move {
        if pos >= end {
            return None;
        }
        let mut file = match file {
            Some(f) => f,
            None => match tokio::fs::File::open(&path).await {
                Ok(mut f) => match f.seek(std::io::SeekFrom::Start(pos)).await {
                    Ok(_) => f,
                    Err(e) => return Some((Err(e), (None, path, end))),
                },
                Err(e) => return Some((Err(e), (None, path, end))),
            },
        };
        let want = CHUNK.min((end - pos) as usize);
        let mut buf = vec![0u8; want];
        match file.read(&mut buf).await {
            Ok(0) => None,
            Ok(n) => {
                buf.truncate(n);
                Some((Ok(Bytes::from(buf)), (Some(file), path, pos + n as u64)))
            }
            Err(e) => Some((Err(e), (None, path, end))),
        }
    })
}

/// `FileResponse(path, media_type=..., filename=..., headers={"Cache-Control": "no-store"})`.
pub async fn file_response(
    path: &Path,
    media_type: &str,
    filename: &str,
    req_headers: &HeaderMap,
) -> Result<Response, ApiError> {
    file_response_with(path, media_type, Some(filename), "no-store", req_headers).await
}

/// [`file_response`] with another `Cache-Control`.
pub async fn file_response_cached(
    path: &Path,
    media_type: &str,
    filename: &str,
    cache_control: &str,
    req_headers: &HeaderMap,
) -> Result<Response, ApiError> {
    file_response_with(path, media_type, Some(filename), cache_control, req_headers).await
}

/// `FileResponse(path, media_type=..., filename=..., headers={"Cache-Control": cache_control})`.
pub async fn file_response_with(
    path: &Path,
    media_type: &str,
    filename: Option<&str>,
    cache_control: &str,
    req_headers: &HeaderMap,
) -> Result<Response, ApiError> {
    let meta = tokio::fs::metadata(path).await.map_err(|_| ApiError::internal())?;
    if !meta.is_file() {
        return Err(ApiError::internal());
    }
    let size = meta.len();
    let mtime = st_mtime(meta.modified().map_err(|_| ApiError::internal())?);
    let (etag, last_modified) = stat_headers(mtime, size);
    let ctype = content_type(media_type);
    let mut headers: Vec<(HeaderName, HeaderValue)> = vec![
        (header::CACHE_CONTROL, hv(cache_control)?),
        (header::CONTENT_TYPE, hv(&ctype)?),
        (header::ACCEPT_RANGES, HeaderValue::from_static("bytes")),
    ];
    if let Some(filename) = filename {
        headers.push((header::CONTENT_DISPOSITION, hv(&content_disposition(filename))?));
    }
    headers.extend([
        (header::CONTENT_LENGTH, HeaderValue::from(size)),
        (header::LAST_MODIFIED, hv(&last_modified)?),
        (header::ETAG, hv(&etag)?),
    ]);
    let get = |name: &str| req_headers.get(name).map(|v| request_id::latin1(v.as_bytes()));
    let use_range = |if_range: &str| !if_range.starts_with("W/") && (if_range == last_modified || if_range == etag);
    let ranges = match get("range") {
        Some(r) if get("if-range").is_none_or(|ir| use_range(&ir)) => match parse_range_header(&r, size) {
            Ok(ranges) => ranges,
            Err(RangeError::Malformed(msg)) => return Ok(plain(StatusCode::BAD_REQUEST, msg, None)),
            Err(RangeError::NotSatisfiable) => {
                let cr = (header::CONTENT_RANGE, format!("bytes */{size}"));
                return Ok(plain(StatusCode::RANGE_NOT_SATISFIABLE, "", Some(cr)));
            }
        },
        _ => Vec::new(),
    };
    let set = |headers: &mut Vec<(HeaderName, HeaderValue)>, name: HeaderName, value: HeaderValue| match headers
        .iter_mut()
        .find(|(n, _)| *n == name)
    {
        Some(slot) => slot.1 = value,
        None => headers.push((name, value)),
    };
    let owned = path.to_path_buf();
    let (status, body) = match ranges.as_slice() {
        [] => (StatusCode::OK, Body::from_stream(read_range(owned, 0, size))),
        [(start, end)] => {
            set(&mut headers, header::CONTENT_RANGE, hv(&format!("bytes {start}-{}/{size}", end - 1))?);
            set(&mut headers, header::CONTENT_LENGTH, HeaderValue::from(end - start));
            (StatusCode::PARTIAL_CONTENT, Body::from_stream(read_range(owned, *start, *end)))
        }
        many => {
            let mut b = [0u8; 13];
            getrandom::fill(&mut b).map_err(|_| ApiError::internal())?;
            let boundary = hex::encode(b);
            let length = byteranges_length(many, &boundary, &ctype, size);
            set(&mut headers, header::CONTENT_TYPE, hv(&format!("multipart/byteranges; boundary={boundary}"))?);
            set(&mut headers, header::CONTENT_LENGTH, HeaderValue::from(length));
            let parts: Vec<_> = many
                .iter()
                .map(|&(start, end)| {
                    let head = Bytes::from(byterange_header(&boundary, &ctype, start, end, size).into_bytes());
                    futures::stream::once(async move { Ok(head) })
                        .chain(read_range(owned.clone(), start, end))
                        .chain(futures::stream::once(async { Ok(Bytes::from_static(b"\r\n")) }))
                })
                .collect();
            let tail = Bytes::from(format!("--{boundary}--").into_bytes());
            let stream = futures::stream::iter(parts).flatten().chain(futures::stream::once(async move { Ok(tail) }));
            (StatusCode::PARTIAL_CONTENT, Body::from_stream(stream))
        }
    };
    let mut res = Response::new(body);
    *res.status_mut() = status;
    for (name, value) in headers {
        res.headers_mut().append(name, value);
    }
    Ok(res.into_response())
}

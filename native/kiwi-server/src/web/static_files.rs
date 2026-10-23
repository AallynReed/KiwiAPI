//! Starlette 1.9 `StaticFiles(directory=site/static)` mounted at `/static`:
//! the path normalised like `os.path.normpath`, symlinks resolved and kept
//! inside the root, `FileResponse` headers from the stat (type from Python's
//! `mimetypes`), `If-None-Match` / `If-Modified-Since` 304s and ranges. The
//! body goes out in 64 KiB frames, one per ASGI message, which the gzip layer
//! relies on.

use std::path::{Path, PathBuf};

use axum::{
    body::Body,
    extract::Request,
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    response::{IntoResponse, Response},
};
use bytes::Bytes;
use tokio::io::AsyncReadExt;

use crate::drops::download::{content_type, st_mtime, stat_headers};
use crate::error::{ApiError, http_exception};
use crate::middleware::pyurl::{latin1, scope_path};

const CHUNK: usize = 64 * 1024;

/// `os.path.normpath(os.path.join(*route_path.split("/")))` (posix).
fn normpath(route_path: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for seg in route_path.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                if out.last().is_some_and(|s| *s != "..") {
                    out.pop();
                } else {
                    out.push("..");
                }
            }
            s => out.push(s),
        }
    }
    if out.is_empty() { ".".to_owned() } else { out.join("/") }
}

/// `email.utils.parsedate` reduced to what the comparison needs.
fn parsedate(s: &str) -> Option<chrono::NaiveDateTime> {
    let s = s.trim();
    let s = s.split_once(',').map_or(s, |(_, rest)| rest).trim();
    let parts: Vec<&str> = s.split_whitespace().collect();
    if parts.len() < 4 {
        return None;
    }
    let day: u32 = parts[0].parse().ok()?;
    let months = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];
    let month = months.iter().position(|m| parts[1].to_lowercase().starts_with(m))? as u32 + 1;
    let mut year: i32 = parts[2].parse().ok()?;
    if year < 100 {
        year += if year > 68 { 1900 } else { 2000 };
    }
    let hms: Vec<u32> = parts[3].split(':').map(|x| x.parse().ok()).collect::<Option<_>>()?;
    let (h, m, sec) = (*hms.first()?, *hms.get(1)?, hms.get(2).copied().unwrap_or(0));
    chrono::NaiveDate::from_ymd_opt(year, month, day)?.and_hms_opt(h, m, sec)
}

fn not_modified(req: &HeaderMap, etag: &str, last_modified: &str) -> bool {
    if let Some(inm) = req.get(header::IF_NONE_MATCH).map(|v| latin1(v.as_bytes())) {
        if inm.trim() == "*" {
            return true;
        }
        return inm.split(',').map(|t| t.trim().trim_start_matches("W/")).any(|t| t == etag);
    }
    let Some(ims) = req.get(header::IF_MODIFIED_SINCE).map(|v| latin1(v.as_bytes())) else { return false };
    matches!((parsedate(&ims), parsedate(last_modified)), (Some(a), Some(b)) if a >= b)
}

/// `FileResponse`'s simple body: 64 KiB reads, an empty last frame when the
/// size is a multiple of the chunk.
pub fn file_frames(path: PathBuf) -> Body {
    let stream = futures::stream::unfold((None::<tokio::fs::File>, path, false), |(file, path, done)| async move {
        if done {
            return None;
        }
        let mut file = match file {
            Some(f) => f,
            None => match tokio::fs::File::open(&path).await {
                Ok(f) => f,
                Err(e) => return Some((Err(e), (None, path, true))),
            },
        };
        let mut buf = vec![0u8; CHUNK];
        let mut n = 0;
        while n < CHUNK {
            match file.read(&mut buf[n..]).await {
                Ok(0) => break,
                Ok(k) => n += k,
                Err(e) => return Some((Err(e), (None, path, true))),
            }
        }
        buf.truncate(n);
        let more = n == CHUNK;
        Some((Ok::<_, std::io::Error>(Bytes::from(buf)), (Some(file), path, !more)))
    });
    Body::from_stream(stream)
}

/// A stat failure Starlette turns into an error reply.
enum StatError {
    Unauthorized,
    Internal,
}

impl StatError {
    fn response(self) -> Response {
        match self {
            StatError::Unauthorized => ApiError::http(StatusCode::UNAUTHORIZED, None).into_response(),
            StatError::Internal => ApiError::internal().into_response(),
        }
    }
}

type Found = Option<(PathBuf, std::fs::Metadata)>;

fn resolve(root: &Path, rel: &str) -> Result<Found, StatError> {
    if rel.contains('\0') {
        return Ok(None);
    }
    let real_root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let joined = real_root.join(rel);
    let full = match std::fs::canonicalize(&joined) {
        Ok(p) => p,
        Err(e) => return stat_error(e),
    };
    if !full.starts_with(&real_root) {
        return Ok(None);
    }
    match std::fs::metadata(&full) {
        Ok(m) => Ok(Some((full, m))),
        Err(e) => stat_error(e),
    }
}

/// How `lookup_path` + `get_response` treat a failed stat: missing is a 404,
/// a permission error a 401, a too-long name a 404, anything else a 500.
fn stat_error(e: std::io::Error) -> Result<Found, StatError> {
    use std::io::ErrorKind::*;
    match e.kind() {
        NotFound | NotADirectory | InvalidFilename | InvalidInput => Ok(None),
        PermissionDenied => Err(StatError::Unauthorized),
        _ if e.raw_os_error() == Some(36) => Ok(None),
        _ if cfg!(windows) => Ok(None),
        _ => Err(StatError::Internal),
    }
}

pub async fn serve(root: PathBuf, req: Request) -> Response {
    let (method, path, headers) = (req.method().clone(), scope_path(req.uri()), req.headers().clone());
    if method != Method::GET && method != Method::HEAD {
        let mut res = ApiError::http(StatusCode::METHOD_NOT_ALLOWED, None).into_response();
        res.headers_mut().insert(header::ALLOW, HeaderValue::from_static("GET, HEAD"));
        return res;
    }
    let route_path = path.strip_prefix("/static").unwrap_or(&path);
    let rel = normpath(route_path);
    let found = match tokio::task::spawn_blocking({
        let root = root.clone();
        move || resolve(&root, &rel)
    })
    .await
    {
        Ok(Ok(found)) => found,
        Ok(Err(e)) => return e.response(),
        Err(_) => return ApiError::internal().into_response(),
    };
    let Some((full, meta)) = found.filter(|(_, m)| m.is_file()) else {
        return http_exception(
            StatusCode::NOT_FOUND,
            None,
            &method,
            &crate::middleware::pyurl::url_path(req.uri()),
            &headers,
        );
    };
    let mtime = meta.modified().map(st_mtime).unwrap_or(0.0);
    let (etag, last_modified) = stat_headers(mtime, meta.len());
    let media = kiwi_core::web::mimetypes::guess_type(&full.to_string_lossy())
        .unwrap_or_else(|| "application/octet-stream".to_owned());
    if not_modified(&headers, &etag, &last_modified) {
        let mut res = Response::new(Body::empty());
        *res.status_mut() = StatusCode::NOT_MODIFIED;
        if let Ok(v) = HeaderValue::from_str(&etag) {
            res.headers_mut().insert(header::ETAG, v);
        }
        return res;
    }
    if headers.contains_key(header::RANGE) {
        match crate::drops::download::file_response_with(&full, &media, None, "", &headers).await {
            Ok(mut res) if res.status() != StatusCode::OK => {
                res.headers_mut().remove(header::CACHE_CONTROL);
                return res;
            }
            Ok(_) => {}
            Err(e) => return e.into_response(),
        }
    }
    let mut res = Response::new(file_frames(full));
    let h = res.headers_mut();
    h.insert(header::CONTENT_TYPE, crate::middleware::pyurl::header_value(&content_type(&media)));
    h.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    h.insert(header::CONTENT_LENGTH, HeaderValue::from(meta.len()));
    h.insert(header::LAST_MODIFIED, crate::middleware::pyurl::header_value(&last_modified));
    h.insert(header::ETAG, crate::middleware::pyurl::header_value(&etag));
    res
}

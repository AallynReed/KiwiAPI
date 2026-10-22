//! Reading an uploader's form the way Starlette does (the whole multipart body
//! is parsed before the handler runs) without holding it in memory: the body
//! is spooled to a temp file, mapped, and file parts are copied straight from
//! the map to the drop's folder.

use std::{
    io::Write,
    ops::Range,
    path::{Path, PathBuf},
    sync::Arc,
};

use axum::body::Body;
use futures::StreamExt;
use kiwi_core::form::{self, FormError, FormValue, UploadFile};
use memmap2::Mmap;
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use crate::{
    routes::misc::feedback::str_field,
    validation::{ErrorItem, Fail, LocItem},
};

const CHUNK: usize = 1024 * 1024;

/// A request body on disk, mapped read-only; the file goes when this does.
pub struct Spool {
    path: PathBuf,
    map: Option<Mmap>,
}

impl Spool {
    pub async fn read(body: Body) -> std::io::Result<Spool> {
        let mut name = [0u8; 8];
        getrandom::fill(&mut name).map_err(std::io::Error::other)?;
        let path = std::env::temp_dir().join(format!("kiwi-drop-{}.body", hex::encode(name)));
        let mut spool = Spool { path, map: None };
        let mut file = tokio::fs::OpenOptions::new().write(true).create_new(true).open(&spool.path).await?;
        let mut stream = body.into_data_stream();
        let mut len = 0usize;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(std::io::Error::other)?;
            len += chunk.len();
            file.write_all(&chunk).await?;
        }
        file.flush().await?;
        drop(file);
        if len > 0 {
            let f = std::fs::File::open(&spool.path)?;
            // SAFETY: the spool file is private to this request and never written again.
            spool.map = Some(unsafe { Mmap::map(&f)? });
        }
        Ok(spool)
    }

    /// No body at all (Starlette never reads one that isn't a form).
    pub fn empty() -> Spool {
        Spool { path: PathBuf::new(), map: None }
    }

    pub fn bytes(&self) -> &[u8] {
        self.map.as_deref().unwrap_or(&[])
    }
}

impl Drop for Spool {
    fn drop(&mut self) {
        self.map.take();
        if !self.path.as_os_str().is_empty() {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

pub type Parsed = (Vec<(String, FormValue)>, Vec<Option<Range<usize>>>);

/// `request.form()` over a spooled body: the items as [`form::parse_form`]
/// gives them, except that only parts named in `need` carry their file data;
/// the others are located by their span in the spool.
pub async fn parse(
    spool: Arc<Spool>,
    content_type: Option<Vec<u8>>,
    need: &'static [&'static str],
) -> Result<Parsed, FormError> {
    tokio::task::spawn_blocking(move || {
        let body = spool.bytes();
        let items = form::parse_form_spans(content_type.as_deref(), body)?;
        let mut out = Vec::with_capacity(items.len());
        let mut spans = Vec::with_capacity(items.len());
        for (name, value, span) in items {
            let value = match value {
                FormValue::File(mut f) if need.contains(&name.as_str()) => {
                    f.data = span.clone().map(|r| body[r].to_vec()).unwrap_or_default();
                    FormValue::File(f)
                }
                v => v,
            };
            out.push((name, value));
            spans.push(span);
        }
        Ok((out, spans))
    })
    .await
    .unwrap_or_else(|_| Err(FormError::Http("There was an error parsing the body".into())))
}

fn missing(name: &str) -> ErrorItem {
    ErrorItem {
        kind: "missing".into(),
        loc: vec![LocItem::from("body"), name.into()],
        msg: "Field required".into(),
        input: Value::Null,
        ctx: None,
    }
}

/// A required `str = Form(...)` field.
pub fn required_str(items: &[(String, FormValue)], name: &str, errors: &mut Vec<ErrorItem>) -> String {
    let present = items
        .iter()
        .rev()
        .find(|(k, _)| k == name)
        .is_some_and(|(_, v)| !matches!(v, FormValue::Text(t) if t.is_empty()));
    if !present {
        errors.push(missing(name));
        return String::new();
    }
    str_field(items, name, None, None, errors).unwrap_or_default()
}

/// `str | None = Form(default=None)`.
pub fn optional_str(items: &[(String, FormValue)], name: &str, errors: &mut Vec<ErrorItem>) -> Option<String> {
    str_field(items, name, None, None, errors)
}

/// `UploadFile = File(...)`: the last part of that name and its span.
pub fn required_file(
    items: &[(String, FormValue)],
    spans: &[Option<Range<usize>>],
    name: &str,
    errors: &mut Vec<ErrorItem>,
) -> Option<(UploadFile, Range<usize>)> {
    let Some(i) = items.iter().rposition(|(k, _)| k == name) else {
        errors.push(missing(name));
        return None;
    };
    match &items[i].1 {
        FormValue::File(f) => Some((f.clone(), spans[i].clone().unwrap_or(0..0))),
        FormValue::Text(t) if t.is_empty() => {
            errors.push(missing(name));
            None
        }
        FormValue::Text(t) => {
            let fail = Fail::value_error("Expected UploadFile, received: <class 'str'>");
            errors.push(ErrorItem {
                kind: fail.kind,
                loc: vec![LocItem::from("body"), name.into()],
                msg: fail.msg,
                input: Value::String(t.clone()),
                ctx: fail.ctx,
            });
            None
        }
    }
}

/// Why `_write_stream` stopped.
#[derive(Debug)]
pub enum WriteError {
    OverLimit,
    Io(std::io::Error),
}

/// `_write_stream`: copy `data` to `dest` in 1 MB chunks, hashing as it goes,
/// stopping the moment it passes `limit`. Blocking: run off the runtime.
pub fn write_stream(data: &[u8], dest: &Path, limit: i64) -> Result<(u64, String), WriteError> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(WriteError::Io)?;
    }
    let mut out = std::fs::File::create(dest).map_err(WriteError::Io)?;
    let mut digest = Sha256::new();
    let mut size: u64 = 0;
    for chunk in data.chunks(CHUNK) {
        size += chunk.len() as u64;
        if size as i128 > i128::from(limit) {
            return Err(WriteError::OverLimit);
        }
        digest.update(chunk);
        out.write_all(chunk).map_err(WriteError::Io)?;
    }
    out.flush().map_err(WriteError::Io)?;
    out.sync_all().map_err(WriteError::Io)?;
    Ok((size, hex::encode(digest.finalize())))
}

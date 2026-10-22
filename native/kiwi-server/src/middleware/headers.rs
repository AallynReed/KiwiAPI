//! Starlette `MutableHeaders` operations on an `http::HeaderMap`.

use axum::http::{HeaderMap, HeaderName, HeaderValue};

use super::pyurl::{header_value, latin1};

pub fn setdefault(h: &mut HeaderMap, name: &'static str, value: &str) {
    if !h.contains_key(name) {
        h.insert(HeaderName::from_static(name), header_value(value));
    }
}

fn first_str(h: &HeaderMap, name: &str) -> Option<String> {
    h.get(name).map(|v| latin1(v.as_bytes()))
}

/// `_add_vary` of `app/core/middleware.py`: merge into the first Vary,
/// skipping fields already listed (case-insensitive).
pub fn add_vary(h: &mut HeaderMap, fields: &[&str]) {
    let existing = first_str(h, "vary");
    let have: Vec<String> = existing
        .as_deref()
        .unwrap_or("")
        .split(',')
        .map(|v| v.trim().to_lowercase())
        .filter(|v| !v.is_empty())
        .collect();
    let merged: Vec<&str> = fields.iter().copied().filter(|f| !have.contains(&f.to_lowercase())).collect();
    if merged.is_empty() {
        return;
    }
    let value = match existing.filter(|e| !e.is_empty()) {
        Some(e) => format!("{e}, {}", merged.join(", ")),
        None => merged.join(", "),
    };
    h.insert("vary", header_value(&value));
}

/// Starlette 1.9 `MutableHeaders.add_vary_header` (and CORS's Vary write):
/// every Vary value joined into one, `field` appended, no dedupe.
pub fn add_vary_header(h: &mut HeaderMap, field: &str) {
    let mut values: Vec<String> = h.get_all("vary").iter().map(|v| latin1(v.as_bytes())).collect();
    values.push(field.to_owned());
    h.insert("vary", header_value(&values.join(", ")));
}

pub fn set(h: &mut HeaderMap, name: &'static str, value: &str) {
    h.insert(HeaderName::from_static(name), header_value(value));
}

pub fn set_static(h: &mut HeaderMap, name: &'static str, value: &'static str) {
    h.insert(HeaderName::from_static(name), HeaderValue::from_static(value));
}

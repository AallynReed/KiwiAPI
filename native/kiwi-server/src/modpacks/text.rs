//! The string helpers of `app/trove/modpacks/service.py`, with Python's
//! `str` semantics.

use kiwi_core::pystr;

use crate::error::ApiError;

#[rustfmt::skip]
#[path = "casefold_table.rs"]
mod table;

/// `str.casefold()`.
pub fn casefold(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        let cp = c as u32;
        if let Ok(i) = table::MULTI.binary_search_by_key(&cp, |&(k, _)| k) {
            out.push_str(table::MULTI[i].1);
            continue;
        }
        let i = table::RUNS.partition_point(|&(first, _, _, _)| first <= cp);
        let mapped = i
            .checked_sub(1)
            .map(|i| table::RUNS[i])
            .filter(|&(first, last, stride, _)| cp <= last && (cp - first).is_multiple_of(stride))
            .and_then(|(_, _, _, delta)| char::from_u32(cp.wrapping_add_signed(delta)));
        out.push(mapped.unwrap_or(c));
    }
    out
}

/// `str.lower()`.
pub fn lower(s: &str) -> String {
    s.to_lowercase()
}

/// `_slugify`: `re.sub(r"[^a-z0-9]+", "-", text.lower()).strip("-")[:60] or "pack"`.
pub fn slugify(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut dash = false;
    for c in lower(text).chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            out.push(c);
            dash = false;
        } else if !dash {
            out.push('-');
            dash = true;
        }
    }
    let s: String = out.trim_matches('-').chars().take(60).collect();
    if s.is_empty() { "pack".into() } else { s }
}

/// `re.sub(r"\s+", " ", s)`.
fn collapse_space(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut space = false;
    for c in s.chars() {
        if pystr::is_space(c) {
            if !space {
                out.push(' ');
            }
            space = true;
        } else {
            out.push(c);
            space = false;
        }
    }
    out
}

/// `_clean_tags`.
pub fn clean_tags(tags: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for t in tags {
        let t: String = collapse_space(&lower(pystr::strip(t))).chars().take(30).collect();
        if !t.is_empty() && !out.contains(&t) {
            out.push(t);
        }
    }
    out.truncate(12);
    out
}

/// `^https?://\S+$` under `re.IGNORECASE` (where `ſ` matches `s`).
fn is_url(u: &str) -> bool {
    let mut it = u.chars();
    for want in ['h', 't', 't', 'p'] {
        if it.next().map(|c| c.to_ascii_lowercase()) != Some(want) {
            return false;
        }
    }
    let rest = it.as_str();
    let rest = match rest.chars().next() {
        Some(c @ ('s' | 'S' | 'ſ')) => &rest[c.len_utf8()..],
        _ => rest,
    };
    let Some(tail) = rest.strip_prefix("://") else { return false };
    !tail.is_empty() && !tail.chars().any(pystr::is_space)
}

/// `_clean_url`: `None` for blank, else the stripped URL or the 400.
pub fn clean_url(u: Option<&str>, field: &str) -> Result<Option<String>, ApiError> {
    let u = pystr::strip(u.unwrap_or(""));
    if u.is_empty() {
        return Ok(None);
    }
    if u.chars().count() > 300 || !is_url(u) {
        return Err(ApiError::bad_request(format!("Invalid {field} URL - it must start with http:// or https://.")));
    }
    Ok(Some(u.to_owned()))
}

/// `_safe_filename` (modpacks falls back to `"modpack"`, the hub to `"mod"`).
pub fn safe_filename(name: &str, fallback: &str) -> String {
    let cleaned: String = name
        .chars()
        .filter(|&c| !matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' | '\0'..='\x1f'))
        .collect();
    let s: String = pystr::strip(&cleaned).trim_matches('.').chars().take(120).collect();
    if s.is_empty() { fallback.into() } else { s }
}

/// `re.escape`.
pub fn re_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if "()[]{}?*+-|^$\\.&~# \t\n\r\x0b\x0c".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// `urllib.parse.quote(s)`.
pub fn quote(s: &str) -> String {
    crate::middleware::pyurl::quote(s, "/")
}

/// `str.rstrip("/")`.
pub fn rstrip_slash(s: &str) -> &str {
    s.trim_end_matches('/')
}

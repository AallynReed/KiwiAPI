//! `app/site_auth/cookies.py`: the HttpOnly session cookies, their exact
//! Starlette `Set-Cookie` spelling, and the origin pin that decides whether a
//! request may authenticate with the cookie at all.

use std::collections::HashMap;

use axum::http::{header, request::Parts, HeaderMap, HeaderValue};
use chrono::{DateTime, Utc};

use super::{pyutil::latin1, sessions::TokenPair, AuthSettings};

pub const ACCESS_COOKIE: &str = "kiwi_site_access";
pub const REFRESH_COOKIE: &str = "kiwi_site_refresh";
pub const SESSION_HINT_COOKIE: &str = "kiwi_site_session";
const REFRESH_PATH: &str = "/v1/site-auth";

/// The bits of `urllib.parse.urlsplit` these rules use.
struct Split<'a> {
    scheme: String,
    netloc: &'a str,
    hostname: Option<String>,
}

fn urlsplit(url: &str) -> Split<'_> {
    let (scheme, rest) = match url.split_once(':') {
        Some((s, r))
            if !s.is_empty()
                && s.starts_with(|c: char| c.is_ascii_alphabetic())
                && s.chars().all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c)) =>
        {
            (s.to_ascii_lowercase(), r)
        }
        _ => (String::new(), url),
    };
    let netloc = rest.strip_prefix("//").map(|r| &r[..r.find(['/', '?', '#']).unwrap_or(r.len())]).unwrap_or("");
    let host_port = netloc.rsplit_once('@').map_or(netloc, |(_, h)| h);
    let host = if let Some(v6) = host_port.strip_prefix('[') {
        v6.split(']').next().unwrap_or("")
    } else {
        host_port.split(':').next().unwrap_or("")
    };
    let hostname = (!host.is_empty()).then(|| host.to_lowercase());
    Split { scheme, netloc, hostname }
}

/// `.aallyn.net` for a 3+ label app host, else host-only (`None`).
pub fn cookie_domain(s: &AuthSettings) -> Option<String> {
    let host = urlsplit(&s.app_url).hostname.unwrap_or_default();
    let parts: Vec<&str> = host.split('.').collect();
    if parts.len() < 3 {
        return None;
    }
    Some(format!(".{}", parts[parts.len() - 2..].join(".")))
}

fn secure(s: &AuthSettings) -> bool {
    urlsplit(&s.app_url).scheme == "https"
}

/// Origins whose requests may use the session cookie as their credential.
pub fn allowed_cookie_origins(s: &AuthSettings) -> Vec<String> {
    let mut out = Vec::new();
    for url in [&s.app_url, &s.api_url] {
        let p = urlsplit(url);
        if !p.scheme.is_empty() && p.hostname.is_some() {
            out.push(format!("{}://{}", p.scheme, p.netloc));
        }
    }
    if s.debug {
        out.push("http://localhost:8913".into());
        out.push("http://127.0.0.1:8913".into());
    }
    out
}

/// `cookie_auth_allowed(request)`.
pub fn cookie_auth_allowed(headers: &HeaderMap, s: &AuthSettings) -> bool {
    match headers.get(header::ORIGIN).map(latin1).filter(|o| !o.is_empty()) {
        Some(origin) => allowed_cookie_origins(s).contains(&origin),
        None => headers.get("sec-fetch-site").map(latin1).as_deref() == Some("same-origin"),
    }
}

/// Starlette's `cookie_parser` over the first Cookie header.
pub fn parse_cookies(headers: &HeaderMap) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let Some(raw) = headers.get(header::COOKIE).map(latin1) else {
        return out;
    };
    for chunk in raw.split(';') {
        let (k, v) = chunk.split_once('=').unwrap_or(("", chunk));
        let (k, v) = (k.trim_matches(super::pyutil::is_space), v.trim_matches(super::pyutil::is_space));
        if !k.is_empty() || !v.is_empty() {
            out.insert(k.to_owned(), unquote(v));
        }
    }
    out
}

/// `http.cookies._unquote`.
fn unquote(v: &str) -> String {
    let Some(inner) = v.strip_prefix('"').and_then(|s| s.strip_suffix('"')).filter(|_| v.len() >= 2) else {
        return v.to_owned();
    };
    let chars: Vec<char> = inner.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let octal = chars
            .get(i + 1..i + 4)
            .filter(|d| ('0'..='3').contains(&d[0]) && d[1..].iter().all(|c| ('0'..='7').contains(c)));
        match (chars[i], octal, chars.get(i + 1)) {
            ('\\', Some(d), _) => {
                let n = d.iter().fold(0, |acc, c| acc * 8 + c.to_digit(8).unwrap_or(0));
                out.push(char::from_u32(n).unwrap_or('\u{fffd}'));
                i += 4;
            }
            ('\\', None, Some(&c)) if c != '\n' => {
                out.push(c);
                i += 2;
            }
            (c, _, _) => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

const LEGAL: &str = "!#$%&'*+-.^_`|~:";

/// `http.cookies._quote`.
fn quote(v: &str) -> String {
    if !v.is_empty() && v.chars().all(|c| c.is_ascii_alphanumeric() || LEGAL.contains(c)) {
        return v.to_owned();
    }
    let mut out = String::from('"');
    for c in v.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 || (0x7f..=0xff).contains(&(c as u32)) => {
                out.push_str(&format!("\\{:03o}", c as u32))
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Attributes shared by the three cookies of one deployment.
struct Attrs<'a> {
    domain: Option<&'a str>,
    secure: bool,
}

/// One Starlette `set_cookie`/`delete_cookie` header value (Morsel attributes
/// in sorted order; `expires` only on delete, formatted as `now`).
fn cookie_line(
    a: &Attrs<'_>,
    (name, value): (&str, &str),
    max_age: i64,
    expires: Option<DateTime<Utc>>,
    path: &str,
    httponly: bool,
) -> String {
    let mut out = format!("{name}={}", quote(value));
    if let Some(d) = a.domain {
        out.push_str(&format!("; Domain={d}"));
    }
    if let Some(t) = expires {
        out.push_str(&format!("; expires={}", t.format("%a, %d %b %Y %H:%M:%S GMT")));
    }
    if httponly {
        out.push_str("; HttpOnly");
    }
    out.push_str(&format!("; Max-Age={max_age}; Path={path}; SameSite=lax"));
    if a.secure {
        out.push_str("; Secure");
    }
    out
}

/// `set_session_cookies`: access (15 min), refresh (30 d, refresh path only)
/// and the JS-readable session hint.
pub fn session_cookie_lines(s: &AuthSettings, tokens: &TokenPair) -> [String; 3] {
    let domain = cookie_domain(s);
    let a = Attrs { domain: domain.as_deref(), secure: secure(s) };
    let max_age = s.refresh_token_expire_days * 86400;
    [
        cookie_line(&a, (ACCESS_COOKIE, &tokens.access_token), s.access_token_expire_minutes * 60, None, "/", true),
        cookie_line(&a, (REFRESH_COOKIE, &tokens.refresh_token), max_age, None, REFRESH_PATH, true),
        cookie_line(&a, (SESSION_HINT_COOKIE, "1"), max_age, None, "/", false),
    ]
}

/// `clear_session_cookies` at `now` (Python stamps `expires` with the current time).
pub fn clear_cookie_lines(s: &AuthSettings, now: DateTime<Utc>) -> [String; 3] {
    let domain = cookie_domain(s);
    let a = Attrs { domain: domain.as_deref(), secure: secure(s) };
    [
        cookie_line(&a, (ACCESS_COOKIE, ""), 0, Some(now), "/", true),
        cookie_line(&a, (REFRESH_COOKIE, ""), 0, Some(now), REFRESH_PATH, true),
        cookie_line(&a, (SESSION_HINT_COOKIE, ""), 0, Some(now), "/", false),
    ]
}

fn append(headers: &mut HeaderMap, lines: [String; 3]) {
    for line in lines {
        if let Ok(v) = HeaderValue::from_str(&line) {
            headers.append(header::SET_COOKIE, v);
        }
    }
}

pub fn set_session_cookies(headers: &mut HeaderMap, tokens: &TokenPair) {
    append(headers, session_cookie_lines(super::auth_settings(), tokens));
}

pub fn clear_session_cookies(headers: &mut HeaderMap) {
    append(headers, clear_cookie_lines(super::auth_settings(), Utc::now()));
}

/// `_credential`: the Bearer token, else the access cookie when the origin
/// pin allows it.
pub fn site_credential(parts: &Parts) -> Option<String> {
    if let Some(cred) = super::extract::bearer(&parts.headers) {
        return Some(cred);
    }
    let token = parse_cookies(&parts.headers).remove(ACCESS_COOKIE).filter(|t| !t.is_empty())?;
    cookie_auth_allowed(&parts.headers, super::auth_settings()).then_some(token)
}

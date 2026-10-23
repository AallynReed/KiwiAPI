//! The Bilibili thumbnail proxy (`fetch_bilibili_image`): httpx's URL parsing
//! and normalization (`httpx/_urlparse.py`, which decides 400 vs 500 and
//! spells the URL in the 502 message), the fetch, and httpx's error texts.

use std::error::Error as _;

use crate::validation::py_repr_str;

/// What `fetch_bilibili_image` raised.
#[derive(Debug, PartialEq, Eq)]
pub enum ImageError {
    /// ValueError: a 400 `bad_request` with this message.
    BadRequest(String),
    /// httpx.InvalidURL (not caught by the route): an unhandled 500.
    InvalidUrl(String),
    /// httpx.HTTPError: a 502 with this `str(exc)`.
    Upstream(String),
}

/// An httpx `ParseResult`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpxUrl {
    pub scheme: String,
    pub userinfo: String,
    pub host: String,
    pub port: Option<i128>,
    pub path: String,
    pub query: Option<String>,
    pub fragment: Option<String>,
}

const MAX_URL_LENGTH: usize = 65536;
const UNRESERVED: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~";
const SUB_DELIMS: &str = "!$&'()*+,;=";

/// Printable ASCII minus `excluded`.
fn printable_except(c: char, excluded: &str) -> bool {
    (' '..='~').contains(&c) && !excluded.contains(c)
}

fn safe_path(c: char) -> bool {
    printable_except(c, " \"#<>?`{}")
}

fn safe_query(c: char) -> bool {
    printable_except(c, " \"#<>")
}

fn safe_fragment(c: char) -> bool {
    printable_except(c, " \"<>`")
}

fn safe_userinfo(c: char) -> bool {
    printable_except(c, " \"#<>?`{}/;=@[\\]^|")
}

fn safe_host(c: char) -> bool {
    SUB_DELIMS.contains(c) || "\"`{}%|\\".contains(c)
}

/// httpx `quote(string, safe)` (existing `%XX` kept, everything else not
/// unreserved or safe percent-encoded as UTF-8).
fn quote(s: &str, safe: fn(char) -> bool) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if UNRESERVED.contains(c) || safe(c) {
            out.push(c);
        } else {
            for b in c.encode_utf8(&mut [0; 4]).bytes() {
                out.push_str(&format!("%{b:02X}"));
            }
        }
    }
    out
}

fn invalid(msg: String) -> ImageError {
    ImageError::InvalidUrl(msg)
}

fn check_printable(s: &str, what: Option<&str>) -> Result<(), ImageError> {
    if let Some((idx, c)) = s.chars().enumerate().find(|(_, c)| c.is_ascii() && (*c < ' ' || *c == '\x7f')) {
        let repr = py_repr_str(&c.to_string());
        return Err(invalid(match what {
            None => format!("Invalid non-printable ASCII character in URL, {repr} at position {idx}."),
            Some(key) => {
                format!("Invalid non-printable ASCII character in URL {key} component, {repr} at position {idx}.")
            }
        }));
    }
    Ok(())
}

/// `ipaddress.IPv4Address(host)` validity.
fn valid_ipv4(host: &str) -> bool {
    let parts: Vec<&str> = host.split('.').collect();
    parts.len() == 4
        && parts.iter().all(|p| {
            !p.is_empty()
                && p.len() <= 3
                && p.bytes().all(|b| b.is_ascii_digit())
                && (*p == "0" || !p.starts_with('0'))
                && p.parse::<u16>().is_ok_and(|v| v <= 255)
        })
}

/// `ipaddress.IPv6Address(addr)` validity, scope id included.
fn valid_ipv6(addr: &str) -> bool {
    let (ip, scope) = match addr.split_once('%') {
        Some((ip, scope)) => (ip, Some(scope)),
        None => (addr, None),
    };
    if scope.is_some_and(|s| s.is_empty() || s.contains('%')) {
        return false;
    }
    ip.parse::<std::net::Ipv6Addr>().is_ok()
}

fn encode_host(host: &str) -> Result<String, ImageError> {
    if host.is_empty() {
        return Ok(String::new());
    }
    let b = host.as_bytes();
    let ipv4_style =
        host.split('.').count() == 4 && host.split('.').all(|p| !p.is_empty() && p.bytes().all(|c| c.is_ascii_digit()));
    if ipv4_style {
        if !valid_ipv4(host) {
            return Err(invalid(format!("Invalid IPv4 address: {}", py_repr_str(host))));
        }
        return Ok(host.to_owned());
    }
    if b.len() >= 2 && b[0] == b'[' && b[b.len() - 1] == b']' {
        if !valid_ipv6(&host[1..host.len() - 1]) {
            return Err(invalid(format!("Invalid IPv6 address: {}", py_repr_str(host))));
        }
        return Ok(host[1..host.len() - 1].to_owned());
    }
    if host.is_ascii() {
        return Ok(quote(&host.to_ascii_lowercase(), safe_host));
    }
    idna::domain_to_ascii_strict(&host.to_lowercase())
        .map_err(|_| invalid(format!("Invalid IDNA hostname: {}", py_repr_str(host))))
}

fn normalize_port(port: Option<&str>, scheme: &str) -> Result<Option<i128>, ImageError> {
    let Some(port) = port.filter(|p| !p.is_empty()) else { return Ok(None) };
    let n =
        kiwi_core::feeds::py::int(port, 10).ok_or_else(|| invalid(format!("Invalid port: {}", py_repr_str(port))))?;
    let default = match scheme {
        "ftp" => Some(21),
        "http" | "ws" => Some(80),
        "https" | "wss" => Some(443),
        _ => None,
    };
    Ok(if Some(n) == default { None } else { Some(n) })
}

/// `normalize_path`: drop `.` and `..` segments.
fn normalize_path(path: &str) -> String {
    if !path.contains('.') {
        return path.to_owned();
    }
    let parts: Vec<&str> = path.split('/').collect();
    if !parts.iter().any(|p| *p == "." || *p == "..") {
        return path.to_owned();
    }
    let mut out: Vec<&str> = Vec::new();
    for p in parts {
        match p {
            "." => {}
            ".." => {
                if !out.is_empty() && out != [""] {
                    out.pop();
                }
            }
            p => out.push(p),
        }
    }
    out.join("/")
}

/// `(userinfo, host, port)` of an authority (`AUTHORITY_REGEX`).
fn split_authority(authority: &str) -> (&str, &str, &str) {
    let (userinfo, rest) = match authority.rfind('@') {
        Some(at) => (&authority[..at], &authority[at + 1..]),
        None => ("", authority),
    };
    let host_end = if rest.starts_with('[') && rest.contains(']') {
        rest.rfind(']').map_or(0, |p| p + 1)
    } else {
        rest.find([':', '@']).unwrap_or(rest.len())
    };
    let (host, after) = rest.split_at(host_end);
    (userinfo, host, after.strip_prefix(':').unwrap_or(after))
}

/// httpx `urlparse(url)`.
pub fn urlparse(url: &str) -> Result<HttpxUrl, ImageError> {
    if url.chars().count() > MAX_URL_LENGTH {
        return Err(invalid("URL too long".into()));
    }
    check_printable(url, None)?;
    let mut rest = url;
    let mut scheme = "";
    if let Some(r) = rest.strip_prefix(':') {
        rest = r;
    } else if rest.starts_with(|c: char| c.is_ascii_alphabetic()) {
        let end =
            rest.find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'))).unwrap_or(rest.len());
        if rest[end..].starts_with(':') {
            scheme = &rest[..end];
            rest = &rest[end + 1..];
        }
    }
    let mut authority = "";
    if let Some(r) = rest.strip_prefix("//") {
        let end = r.find(['/', '?', '#']).unwrap_or(r.len());
        authority = &r[..end];
        rest = &r[end..];
    }
    let path_end = rest.find(['?', '#']).unwrap_or(rest.len());
    let path = &rest[..path_end];
    rest = &rest[path_end..];
    let mut query = None;
    if let Some(r) = rest.strip_prefix('?') {
        let end = r.find('#').unwrap_or(r.len());
        query = Some(&r[..end]);
        rest = &r[end..];
    }
    let fragment = rest.strip_prefix('#');
    build(scheme, authority, None, path, query, fragment)
}

fn build(
    scheme: &str,
    authority: &str,
    host_override: Option<&str>,
    path: &str,
    query: Option<&str>,
    fragment: Option<&str>,
) -> Result<HttpxUrl, ImageError> {
    let (userinfo, host, port) = split_authority(authority);
    let host = host_override.unwrap_or(host);
    let parsed_host = encode_host(host)?;
    let parsed_port = normalize_port(Some(port), scheme)?;
    let parsed_userinfo = quote(userinfo, safe_userinfo);
    let has_scheme = !scheme.is_empty();
    let has_authority = !parsed_userinfo.is_empty() || !parsed_host.is_empty() || parsed_port.is_some();
    if has_authority && !path.is_empty() && !path.starts_with('/') {
        return Err(invalid("For absolute URLs, path must be empty or begin with '/'".into()));
    }
    if !has_scheme && !has_authority {
        if path.starts_with("//") {
            return Err(invalid("Relative URLs cannot have a path starting with '//'".into()));
        }
        if path.starts_with(':') {
            return Err(invalid("Relative URLs cannot have a path starting with ':'".into()));
        }
    }
    let path = if has_scheme || has_authority { normalize_path(path) } else { path.to_owned() };
    Ok(HttpxUrl {
        scheme: scheme.to_ascii_lowercase(),
        userinfo: parsed_userinfo,
        host: parsed_host,
        port: parsed_port,
        path: quote(&path, safe_path),
        query: query.map(|q| quote(q, safe_query)),
        fragment: fragment.map(|f| quote(f, safe_fragment)),
    })
}

impl HttpxUrl {
    fn authority(&self) -> String {
        let mut out = String::new();
        if !self.userinfo.is_empty() {
            out.push_str(&self.userinfo);
            out.push('@');
        }
        if self.host.contains(':') {
            out.push_str(&format!("[{}]", self.host));
        } else {
            out.push_str(&self.host);
        }
        if let Some(p) = self.port {
            out.push_str(&format!(":{p}"));
        }
        out
    }

    /// `URL.raw_path`: path (or `/`) plus the query.
    pub fn raw_path(&self) -> String {
        let mut p = if self.path.is_empty() { "/".to_owned() } else { self.path.clone() };
        if let Some(q) = &self.query {
            p.push('?');
            p.push_str(q);
        }
        p
    }

    /// `URL.host`: an `xn--` host decoded back to Unicode.
    pub fn display_host(&self) -> Result<String, ImageError> {
        if !self.host.starts_with("xn--") {
            return Ok(self.host.clone());
        }
        let (text, res) = idna::domain_to_unicode(&self.host);
        res.map(|_| text).map_err(|_| ImageError::BadRequest(format!("Invalid A-label: {}", self.host)))
    }
}

impl std::fmt::Display for HttpxUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if !self.scheme.is_empty() {
            write!(f, "{}:", self.scheme)?;
        }
        let authority = self.authority();
        if !authority.is_empty() {
            write!(f, "//{authority}")?;
        }
        f.write_str(&self.path)?;
        if let Some(q) = &self.query {
            write!(f, "?{q}")?;
        }
        if let Some(fr) = &self.fragment {
            write!(f, "#{fr}")?;
        }
        Ok(())
    }
}

/// `_hdslb_target(url)`.
pub fn hdslb_target(url: &str) -> Result<HttpxUrl, ImageError> {
    let parsed = urlparse(url)?;
    let host = parsed.display_host()?.to_lowercase();
    if parsed.scheme != "https" || !(host == "hdslb.com" || host.ends_with(".hdslb.com")) {
        return Err(ImageError::BadRequest("only https hdslb.com images may be proxied".into()));
    }
    let authority = parsed.authority();
    let host_arg = if host.contains(':') && !(host.starts_with('[') && host.ends_with(']')) {
        format!("[{host}]")
    } else {
        host.clone()
    };
    let components = [
        ("authority", Some(authority.as_str())),
        ("path", Some(parsed.path.as_str())),
        ("query", parsed.query.as_deref()),
        ("fragment", parsed.fragment.as_deref()),
        ("host", Some(host_arg.as_str())),
    ];
    for (key, value) in components {
        if let Some(v) = value {
            if v.chars().count() > MAX_URL_LENGTH {
                return Err(invalid(format!("URL component '{key}' too long")));
            }
            check_printable(v, Some(key))?;
        }
    }
    build("https", &authority, Some(&host_arg), &parsed.path, parsed.query.as_deref(), parsed.fragment.as_deref())
}

/// `Response.raise_for_status()`'s message (`None` for a 2xx).
pub fn status_message(status: u16, reason: &str, location: Option<&str>, url: &str) -> Option<String> {
    if (200..300).contains(&status) {
        return None;
    }
    let kind = match status / 100 {
        1 => "Informational response",
        3 => "Redirect response",
        4 => "Client error",
        5 => "Server error",
        _ => "Invalid status code",
    };
    let mut msg = format!("{kind} '{status} {reason}' for url '{url}'\n");
    if let Some(loc) = location.filter(|_| matches!(status, 301 | 302 | 303 | 307 | 308)) {
        msg.push_str(&format!("Redirect location: '{loc}'\n"));
    }
    msg.push_str(&format!(
        "For more information check: https://developer.mozilla.org/en-US/docs/Web/HTTP/Status/{status}"
    ));
    Some(msg)
}

/// `str(exc)` of the httpx transport error a failed request becomes.
pub fn transport_message(err: &reqwest::Error) -> String {
    if err.is_timeout() {
        return String::new();
    }
    let mut source = err.source();
    while let Some(e) = source {
        if let Some(io) = e.downcast_ref::<std::io::Error>() {
            let text = io.to_string();
            if let Some(detail) = text.strip_prefix("failed to lookup address information: ") {
                let code = match detail {
                    "Name or service not known" => -2,
                    "Temporary failure in name resolution" => -3,
                    "Non-recoverable failure in name resolution" => -4,
                    "No address associated with hostname" => -5,
                    _ => -2,
                };
                return format!("[Errno {code}] {detail}");
            }
            if io.kind() == std::io::ErrorKind::TimedOut {
                return String::new();
            }
            if matches!(
                io.kind(),
                std::io::ErrorKind::ConnectionRefused
                    | std::io::ErrorKind::ConnectionReset
                    | std::io::ErrorKind::HostUnreachable
                    | std::io::ErrorKind::NetworkUnreachable
                    | std::io::ErrorKind::AddrNotAvailable
            ) && err.is_connect()
            {
                return "All connection attempts failed".into();
            }
        }
        if let Some(tls) = e.downcast_ref::<rustls::Error>() {
            return tls.to_string();
        }
        source = e.source();
    }
    if err.is_connect() {
        return "All connection attempts failed".into();
    }
    let mut text = err.to_string();
    let mut source = err.source();
    while let Some(e) = source {
        text = e.to_string();
        source = e.source();
    }
    text
}

/// httpx's decoding of the response headers (ascii, utf-8, else latin-1),
/// then `headers.get("content-type", "image/jpeg")`.
fn content_type(headers: &reqwest::header::HeaderMap) -> String {
    let all = || headers.values().map(|v| v.as_bytes());
    let decode: fn(&[u8]) -> String = if all().all(|b| b.is_ascii()) {
        |b| b.iter().map(|&c| c as char).collect()
    } else if all().all(|b| std::str::from_utf8(b).is_ok()) {
        |b| String::from_utf8_lossy(b).into_owned()
    } else {
        |b| b.iter().map(|&c| c as char).collect()
    };
    let values: Vec<String> =
        headers.get_all(reqwest::header::CONTENT_TYPE).iter().map(|v| decode(v.as_bytes())).collect();
    if values.is_empty() { "image/jpeg".into() } else { values.join(", ") }
}

/// `fetch_bilibili_image(url)`: the bytes and content type, never following
/// redirects.
pub async fn fetch(url: &str) -> Result<(bytes::Bytes, String), ImageError> {
    let target = hdslb_target(url)?;
    let request_url = format!("https://{}{}", target.authority(), target.raw_path());
    let Ok(parsed) = reqwest::Url::parse(&request_url) else {
        return Err(ImageError::Upstream("[Errno -2] Name or service not known".into()));
    };
    let resp = super::http::clients()
        .image
        .get(parsed)
        .send()
        .await
        .map_err(|e| ImageError::Upstream(transport_message(&e)))?;
    let status = resp.status();
    let reason = resp
        .extensions()
        .get::<hyper::ext::ReasonPhrase>()
        .map(|r| String::from_utf8_lossy(r.as_bytes()).chars().filter(char::is_ascii).collect::<String>())
        .unwrap_or_else(|| status.canonical_reason().unwrap_or("").to_owned());
    let location = resp
        .headers()
        .get_all(reqwest::header::LOCATION)
        .iter()
        .map(|v| String::from_utf8_lossy(v.as_bytes()).into_owned())
        .collect::<Vec<_>>();
    let location = (!location.is_empty()).then(|| location.join(", "));
    let ctype = content_type(resp.headers());
    let body = resp.bytes().await.map_err(|e| ImageError::Upstream(transport_message(&e)))?;
    if let Some(msg) = status_message(status.as_u16(), &reason, location.as_deref(), &target.to_string()) {
        return Err(ImageError::Upstream(msg));
    }
    let ctype = if ctype.starts_with("image/") { ctype } else { "image/jpeg".into() };
    Ok((body, ctype))
}

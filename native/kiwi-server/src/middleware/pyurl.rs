//! How Starlette/uvicorn see a request: latin-1 header strings, the
//! percent-decoded ASGI path, `request.url` (a urlsplit of the rebuilt URL),
//! query params, cookies and Python's `int()` / `quote()`.

use axum::http::{HeaderMap, HeaderValue, Uri};

pub fn latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| b as char).collect()
}

/// Encode a str that came from latin-1 (chars above U+00FF become `?`).
pub fn latin1_bytes(s: &str) -> Vec<u8> {
    s.chars().map(|c| if (c as u32) < 256 { c as u8 } else { b'?' }).collect()
}

pub fn header_value(s: &str) -> HeaderValue {
    HeaderValue::from_bytes(&latin1_bytes(s)).unwrap_or_else(|_| HeaderValue::from_static(""))
}

/// Starlette `headers.get(name)`: the first value, latin-1 decoded.
pub fn first(headers: &HeaderMap, name: &str) -> Option<String> {
    headers.get(name).map(|v| latin1(v.as_bytes()))
}

/// uvicorn's `scope["path"]`: `unquote` of the raw path (UTF-8, errors replaced).
pub fn scope_path(uri: &Uri) -> String {
    let raw = uri.path();
    if !raw.contains('%') {
        return raw.to_owned();
    }
    percent_encoding::percent_decode_str(raw).decode_utf8_lossy().into_owned()
}

/// `request.url.path` and `request.url.query`: urlsplit of `path?query` after
/// Python drops tab/CR/LF, so a decoded `?` or `#` in the path ends it there.
pub fn url_path_query(uri: &Uri) -> (String, String) {
    let mut s = scope_path(uri);
    if let Some(q) = uri.query().filter(|q| !q.is_empty()) {
        s.push('?');
        s.push_str(q);
    }
    s.retain(|c| !matches!(c, '\t' | '\r' | '\n'));
    if let Some(i) = s.find('#') {
        s.truncate(i);
    }
    match s.split_once('?') {
        Some((p, q)) => (p.to_owned(), q.to_owned()),
        None => (s, String::new()),
    }
}

pub fn url_path(uri: &Uri) -> String {
    url_path_query(uri).0
}

/// `request.url.hostname`: from the Host header (else the URI authority),
/// userinfo and port removed, lowercased.
pub fn hostname(headers: &HeaderMap, uri: &Uri) -> Option<String> {
    let netloc = match headers.get("host") {
        Some(h) => latin1(h.as_bytes()),
        None => uri.authority()?.as_str().to_owned(),
    };
    let netloc = netloc.split(['/', '?', '#']).next().unwrap_or("");
    let hostinfo = netloc.rsplit_once('@').map_or(netloc, |(_, h)| h);
    let host = match hostinfo.split_once('[') {
        Some((_, bracketed)) => bracketed.split(']').next().unwrap_or(""),
        None => hostinfo.split(':').next().unwrap_or(""),
    };
    if host.is_empty() {
        return None;
    }
    Some(match host.split_once('%') {
        Some((h, zone)) => format!("{}%{zone}", h.to_lowercase()),
        None => host.to_lowercase(),
    })
}

/// `request.query_params.get(name)`: the LAST value, blank values kept.
pub fn query_param(uri: &Uri, name: &str) -> Option<String> {
    let q = uri.query()?;
    form_urlencoded::parse(q.as_bytes()).filter(|(k, _)| k == name).last().map(|(_, v)| v.into_owned())
}

/// Whether Starlette's `request.cookies` (first Cookie header) has `name`.
pub fn has_cookie(headers: &HeaderMap, name: &str) -> bool {
    let Some(raw) = first(headers, "cookie") else { return false };
    raw.split(';').any(|chunk| match chunk.split_once('=') {
        Some((k, _)) => k.trim() == name,
        None => false,
    })
}

/// Python `int(s)` for the decimal forms a header can carry.
pub fn py_int(s: &str) -> Option<i128> {
    let t = s.trim();
    let (neg, digits) = match t.as_bytes().first()? {
        b'-' => (true, &t[1..]),
        b'+' => (false, &t[1..]),
        _ => (false, t),
    };
    if digits.is_empty() || digits.starts_with('_') || digits.ends_with('_') || digits.contains("__") {
        return None;
    }
    let mut n: i128 = 0;
    for c in digits.chars() {
        if c == '_' {
            continue;
        }
        n = n.checked_mul(10)?.checked_add(c.to_digit(10)? as i128)?;
    }
    Some(if neg { -n } else { n })
}

/// `urllib.parse.quote(s, safe)`.
pub fn quote(s: &str, safe: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        let c = b as char;
        if b.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-' | '~') || (b < 128 && safe.contains(c)) {
            out.push(c);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// A JSON string as Python `json.dumps` writes it with `ensure_ascii=True`.
pub fn ascii_json_str(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 || (c as u32) > 0x7e => {
                let mut buf = [0u16; 2];
                for unit in c.encode_utf16(&mut buf) {
                    out.push_str(&format!("\\u{unit:04x}"));
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls() {
        let u: Uri = "/mods/a%20b?x=1".parse().unwrap();
        assert_eq!(url_path_query(&u), ("/mods/a b".into(), "x=1".into()));
        let u: Uri = "/mods/a%3Fb?x=1".parse().unwrap();
        assert_eq!(url_path_query(&u), ("/mods/a".into(), "b?x=1".into()));
        let u: Uri = "/x%23y?q".parse().unwrap();
        assert_eq!(url_path_query(&u), ("/x".into(), String::new()));
        assert_eq!(quote("https://t.net/mods/a b?x=%20&y=ü", ":/%#?=@[]!$&'()*+,;"), "https://t.net/mods/a%20b?x=%20&y=%C3%BC");
        assert_eq!(py_int(" 1_000 "), Some(1000));
        assert_eq!(py_int("1__0"), None);
        assert_eq!(py_int("-5"), Some(-5));
        assert_eq!(py_int("abc"), None);
        assert_eq!(ascii_json_str("a\"é\u{1}"), "\"a\\\"\\u00e9\\u0001\"");
    }

    #[test]
    fn hosts() {
        let mut h = HeaderMap::new();
        let u: Uri = "/".parse().unwrap();
        h.insert("host", HeaderValue::from_static("API.Aallyn.net:443"));
        assert_eq!(hostname(&h, &u).as_deref(), Some("api.aallyn.net"));
        h.insert("host", HeaderValue::from_static("u@[::1]:8000"));
        assert_eq!(hostname(&h, &u).as_deref(), Some("::1"));
        h.insert("cookie", HeaderValue::from_static("a=1; kiwi_site_access=; b"));
        assert!(has_cookie(&h, "kiwi_site_access"));
        h.insert("cookie", HeaderValue::from_static("kiwi_site_access"));
        assert!(!has_cookie(&h, "kiwi_site_access"));
    }
}

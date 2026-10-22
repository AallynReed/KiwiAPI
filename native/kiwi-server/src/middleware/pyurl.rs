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

/// Starlette 1.9 `request.url.path` from the decoded ASGI path: `?`, `#` and
/// `\` escaped back, tab/CR/LF dropped by urlsplit.
pub fn request_url_path(decoded: &str) -> String {
    let mut s = decoded.replace('?', "%3F").replace('#', "%23").replace('\\', "%5C");
    s.retain(|c| !matches!(c, '\t' | '\r' | '\n'));
    s
}

/// `request.url.path` and `request.url.query`.
pub fn url_path_query(uri: &Uri) -> (String, String) {
    let mut query = uri.query().unwrap_or("").to_owned();
    query.retain(|c| !matches!(c, '\t' | '\r' | '\n'));
    if let Some(i) = query.find('#') {
        query.truncate(i);
    }
    (request_url_path(&scope_path(uri)), query)
}

pub fn url_path(uri: &Uri) -> String {
    url_path_query(uri).0
}

fn host_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || "._~%!$&'()*+,;=-".contains(c)
}

/// Starlette 1.9 `parse_host_header` plus `is_valid_port`: the host part (IP
/// literal brackets kept) of a Host value it accepts.
fn parse_host(v: &str) -> Option<&str> {
    let (host, rest) = if let Some(inner) = v.strip_prefix('[') {
        let close = inner.find(']')?;
        let lit = &inner[..close];
        let ok = match lit.strip_prefix('v') {
            Some(future) => future.split_once('.').is_some_and(|(ver, addr)| {
                !ver.is_empty()
                    && ver.chars().all(|c| c.is_ascii_hexdigit())
                    && !addr.is_empty()
                    && addr.chars().all(|c| (host_char(c) && c != '%') || c == ':')
            }),
            None => {
                lit.chars().all(|c| c.is_ascii_hexdigit() || c == ':' || c == '.')
                    && lit.parse::<std::net::Ipv6Addr>().is_ok()
            }
        };
        if !ok {
            return None;
        }
        v.split_at(close + 2)
    } else {
        let end = v.find(|c: char| !host_char(c)).unwrap_or(v.len());
        if end == 0 {
            return None;
        }
        v.split_at(end)
    };
    let port_ok = match rest.strip_prefix(':') {
        None => rest.is_empty(),
        Some(p) => {
            let digits = p.trim_start_matches('0');
            !p.is_empty()
                && p.bytes().all(|b| b.is_ascii_digit())
                && digits.len() <= 5
                && digits.parse::<u32>().unwrap_or(0) <= 65535
        }
    };
    port_ok.then_some(host)
}

/// `request.url.hostname`: the Host header (else the URI authority) as
/// Starlette 1.9 accepts it, lowercased up to any `%`. `None` where Starlette
/// falls back to the server address, which is never one of our hosts.
pub fn hostname(headers: &HeaderMap, uri: &Uri) -> Option<String> {
    let value = match headers.get("host") {
        Some(h) => latin1(h.as_bytes()),
        None => uri.authority()?.as_str().to_owned(),
    };
    let host = parse_host(&value)?;
    let host = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')).unwrap_or(host);
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
        assert_eq!(url_path_query(&u), ("/mods/a%3Fb".into(), "x=1".into()));
        let u: Uri = "/x%23y%5C%09z?q".parse().unwrap();
        assert_eq!(url_path_query(&u), ("/x%23y%5Cz".into(), "q".into()));
        assert_eq!(
            quote("https://t.net/mods/a b?x=%20&y=ü", ":/%#?=@[]!$&'()*+,;"),
            "https://t.net/mods/a%20b?x=%20&y=%C3%BC"
        );
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
        h.insert("host", HeaderValue::from_static("[::1]:8000"));
        assert_eq!(hostname(&h, &u).as_deref(), Some("::1"));
        h.insert("host", HeaderValue::from_static("API.Aallyn.NET%ZoNe"));
        assert_eq!(hostname(&h, &u).as_deref(), Some("api.aallyn.net%ZoNe"));
        h.insert("host", HeaderValue::from_static("[v1.X]"));
        assert_eq!(hostname(&h, &u).as_deref(), Some("v1.x"));
        h.insert("host", HeaderValue::from_static("api.aallyn.net:000443"));
        assert_eq!(hostname(&h, &u).as_deref(), Some("api.aallyn.net"));
        for bad in ["u@[::1]:8000", "api.aallyn.net:99999", "api.aallyn.net:", "a/b", "[V1.x]", "[1.2.3.4]", ""] {
            h.insert("host", HeaderValue::from_static(bad));
            assert_eq!(hostname(&h, &u), None, "{bad}");
        }
        h.insert("cookie", HeaderValue::from_static("a=1; kiwi_site_access=; b"));
        assert!(has_cookie(&h, "kiwi_site_access"));
        h.insert("cookie", HeaderValue::from_static("kiwi_site_access"));
        assert!(!has_cookie(&h, "kiwi_site_access"));
    }
}

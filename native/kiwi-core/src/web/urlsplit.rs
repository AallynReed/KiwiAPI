//! Python 3.13 `urllib.parse.urlsplit` (scheme, netloc, path, query) with the
//! `ValueError`s it raises, `.hostname`, `unquote` and `parse_qs`.

use std::net::Ipv6Addr;

use unicode_normalization::UnicodeNormalization;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Split {
    pub scheme: String,
    pub netloc: String,
    pub path: String,
    pub query: String,
}

/// The `ValueError` urlsplit raises.
#[derive(Debug, PartialEq, Eq)]
pub struct Invalid;

fn is_ipv4(s: &str) -> bool {
    let parts: Vec<&str> = s.split('.').collect();
    parts.len() == 4
        && parts.iter().all(|p| {
            !p.is_empty()
                && p.len() <= 3
                && p.bytes().all(|b| b.is_ascii_digit())
                && (p.len() == 1 || !p.starts_with('0'))
                && p.parse::<u16>().is_ok_and(|n| n <= 255)
        })
}

fn is_ipv6(s: &str) -> bool {
    let (addr, scope) = match s.split_once('%') {
        Some((a, z)) => (a, Some(z)),
        None => (s, None),
    };
    if scope.is_some_and(|z| z.is_empty() || z.contains('%')) {
        return false;
    }
    addr.parse::<Ipv6Addr>().is_ok()
}

fn check_bracketed_host(host: &str) -> Result<(), Invalid> {
    if let Some(rest) = host.strip_prefix(['v', 'V']) {
        let (hex, tail) = rest.split_once('.').unwrap_or((rest, ""));
        let ok = !hex.is_empty() && hex.bytes().all(|b| b.is_ascii_hexdigit()) && !tail.is_empty();
        return if ok { Ok(()) } else { Err(Invalid) };
    }
    if is_ipv4(host) || !is_ipv6(host) { Err(Invalid) } else { Ok(()) }
}

fn check_bracketed_netloc(netloc: &str) -> Result<(), Invalid> {
    let host_port = netloc.rsplit_once('@').map_or(netloc, |(_, h)| h);
    let host = match host_port.split_once('[') {
        Some((before, bracketed)) => {
            if !before.is_empty() {
                return Err(Invalid);
            }
            let (host, port) = bracketed.split_once(']').unwrap_or((bracketed, ""));
            if !port.is_empty() && !port.starts_with(':') {
                return Err(Invalid);
            }
            host
        }
        None => host_port.split_once(':').map_or(host_port, |(h, _)| h),
    };
    check_bracketed_host(host)
}

fn check_netloc(netloc: &str) -> Result<(), Invalid> {
    if netloc.is_ascii() {
        return Ok(());
    }
    let n: String = netloc.chars().filter(|c| !"@:#?".contains(*c)).collect();
    let normal: String = n.nfkc().collect();
    if n != normal && normal.contains(['/', '?', '#', '@', ':']) {
        return Err(Invalid);
    }
    Ok(())
}

/// `urlsplit(url)`.
pub fn urlsplit(url: &str) -> Result<Split, Invalid> {
    let url: String =
        url.trim_start_matches(|c: char| c <= ' ').chars().filter(|c| !matches!(c, '\t' | '\r' | '\n')).collect();
    let mut rest = url.as_str();
    let mut out = Split::default();
    if let Some(i) = rest.find(':')
        && i > 0
        && rest.starts_with(|c: char| c.is_ascii_alphabetic())
        && rest[..i].chars().all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))
    {
        out.scheme = rest[..i].to_ascii_lowercase();
        rest = &rest[i + 1..];
    }
    if let Some(after) = rest.strip_prefix("//") {
        let end = after.find(['/', '?', '#']).unwrap_or(after.len());
        let netloc = &after[..end];
        rest = &after[end..];
        if netloc.contains('[') != netloc.contains(']') {
            return Err(Invalid);
        }
        if netloc.contains('[') {
            check_bracketed_netloc(netloc)?;
        }
        out.netloc = netloc.to_owned();
    }
    let rest = rest.split_once('#').map_or(rest, |(u, _)| u);
    match rest.split_once('?') {
        Some((p, q)) => {
            out.path = p.to_owned();
            out.query = q.to_owned();
        }
        None => out.path = rest.to_owned(),
    }
    check_netloc(&out.netloc)?;
    Ok(out)
}

impl Split {
    /// `.hostname`: lowercased up to any `%` zone, `None` when empty.
    pub fn hostname(&self) -> Option<String> {
        let host_info = self.netloc.rsplit_once('@').map_or(self.netloc.as_str(), |(_, h)| h);
        let host = match host_info.split_once('[') {
            Some((_, bracketed)) => bracketed.split_once(']').map_or(bracketed, |(h, _)| h),
            None => host_info.split_once(':').map_or(host_info, |(h, _)| h),
        };
        if host.is_empty() {
            return None;
        }
        Some(match host.split_once('%') {
            Some((h, zone)) => format!("{}%{zone}", h.to_lowercase()),
            None => host.to_lowercase(),
        })
    }
}

/// `urllib.parse.unquote(s)`.
pub fn unquote(s: &str) -> String {
    crate::store::py::unquote(s)
}

/// `parse_qs(query)[name][0]`: blank values and `=`-less fields are skipped.
pub fn parse_qs_first(query: &str, name: &str) -> Option<String> {
    query.split('&').find_map(|field| {
        let (k, v) = field.split_once('=')?;
        if v.is_empty() {
            return None;
        }
        (crate::form::unquote_plus(k) == name).then(|| crate::form::unquote_plus(v))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn python_answers() {
        let s = urlsplit(" https://WWW.Trove.Aallyn.net:443/mods/a%20b?utm_source=x#frag").unwrap();
        assert_eq!(s.scheme, "https");
        assert_eq!(s.path, "/mods/a%20b");
        assert_eq!(s.query, "utm_source=x");
        assert_eq!(s.hostname().as_deref(), Some("www.trove.aallyn.net"));
        assert_eq!(urlsplit("http://[::1]:80/").unwrap().hostname().as_deref(), Some("::1"));
        assert_eq!(urlsplit("http://[1.2.3.4]/"), Err(Invalid));
        assert_eq!(urlsplit("http://[::1/"), Err(Invalid));
        assert_eq!(urlsplit("/x?a=1").unwrap().hostname(), None);
        assert_eq!(urlsplit("http://a\u{2100}b/"), Err(Invalid));
        assert_eq!(
            parse_qs_first("a=&utm_source=Dis+cord%21&utm_source=x", "utm_source").as_deref(),
            Some("Dis cord!")
        );
        assert_eq!(parse_qs_first("utm_source", "utm_source"), None);
    }
}

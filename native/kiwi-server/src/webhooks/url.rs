//! `urllib.parse.urlparse` (CPython 3.13) as far as `normalise_url` and
//! `_mask_url` use it. `Err` is the `ValueError` urlsplit raises (a 500 in
//! the route).

use std::net::Ipv6Addr;

use crate::error::ApiError;

pub struct Parsed {
    pub scheme: String,
    pub netloc: String,
    pub path: String,
}

#[derive(Debug)]
pub struct Invalid;

/// Code points whose NFKC form holds one of `/?#@:` (Unicode 15.1).
const NFKC_DELIMS: [u32; 19] = [
    0x2047, 0x2048, 0x2049, 0x2100, 0x2101, 0x2105, 0x2106, 0x2a74, 0xfe13, 0xfe16, 0xfe55, 0xfe56, 0xfe5f, 0xfe6b,
    0xff03, 0xff0f, 0xff1a, 0xff1f, 0xff20,
];

fn is_scheme_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || "+-.".contains(c)
}

/// `ipaddress.IPv4Address`: four decimal octets, no leading zeros.
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

/// `_check_bracketed_host`.
fn check_bracketed_host(host: &str) -> Result<(), Invalid> {
    if host.starts_with(['v', 'V']) {
        let rest = &host[1..];
        let (hex, tail) = rest.split_once('.').unwrap_or((rest, ""));
        let ok = !hex.is_empty() && hex.bytes().all(|b| b.is_ascii_hexdigit()) && !tail.is_empty();
        return if ok && !tail.contains('\n') { Ok(()) } else { Err(Invalid) };
    }
    if is_ipv4(host) || !is_ipv6(host) { Err(Invalid) } else { Ok(()) }
}

/// `_check_bracketed_netloc`.
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

/// `urlparse(url)`: scheme, netloc and the path without its `;params`.
pub fn urlparse(url: &str) -> Result<Parsed, Invalid> {
    let url: String =
        url.trim_start_matches(|c: char| c <= ' ').chars().filter(|c| !matches!(c, '\t' | '\r' | '\n')).collect();
    let mut rest = url.as_str();
    let mut scheme = String::new();
    if let Some(i) = rest.find(':')
        && i > 0
        && rest.starts_with(|c: char| c.is_ascii_alphabetic())
        && rest[..i].chars().all(is_scheme_char)
    {
        scheme = rest[..i].to_ascii_lowercase();
        rest = &rest[i + 1..];
    }
    let mut netloc = "";
    if let Some(after) = rest.strip_prefix("//") {
        let end = after.find(['/', '?', '#']).unwrap_or(after.len());
        netloc = &after[..end];
        rest = &after[end..];
        if netloc.contains('[') != netloc.contains(']') {
            return Err(Invalid);
        }
        if netloc.contains('[') {
            check_bracketed_netloc(netloc)?;
        }
    }
    let mut path = rest.split('#').next().unwrap_or("");
    path = path.split('?').next().unwrap_or("");
    if !netloc.is_ascii() && netloc.chars().filter(|c| !"@:#?".contains(*c)).any(|c| NFKC_DELIMS.contains(&(c as u32)))
    {
        return Err(Invalid);
    }
    if scheme == "https" && path.contains(';') {
        let cut = match path.rfind('/') {
            Some(slash) => path[slash..].find(';').map(|i| slash + i),
            None => path.find(';'),
        };
        if let Some(i) = cut {
            path = &path[..i];
        }
    }
    Ok(Parsed { scheme, netloc: netloc.to_owned(), path: path.to_owned() })
}

impl Parsed {
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

const DISCORD_HOSTS: [&str; 4] = ["discord.com", "discordapp.com", "canary.discord.com", "ptb.discord.com"];

/// `normalise_url`: an https Discord webhook URL, rebuilt as
/// `https://{host}{path}`, else the route's 400 (`Err(None)` is a 500).
pub fn normalise_url(raw: &str) -> Result<String, Option<ApiError>> {
    let p = urlparse(kiwi_core::pystr::strip(raw)).map_err(|_| None)?;
    let host = p.hostname().unwrap_or_default().to_lowercase();
    if p.scheme != "https" || !DISCORD_HOSTS.contains(&host.as_str()) {
        return Err(Some(ApiError::bad_request(
            "URL must be an https Discord webhook (discord.com/api/webhooks/\u{2026}).",
        )));
    }
    let parts: Vec<&str> = p.path.split('/').filter(|s| !s.is_empty()).collect();
    let Some(i) = parts.iter().position(|s| *s == "webhooks") else {
        return Err(Some(ApiError::bad_request("That doesn't look like a Discord webhook URL.")));
    };
    if parts.len() < i + 3 {
        return Err(Some(ApiError::bad_request("Discord webhook URL is missing its id/token.")));
    }
    Ok(format!("https://{host}{}", p.path))
}

/// `_mask_url`: the token segment shortened to `abcd…yz` for display.
pub fn mask_url(url: &str) -> Result<String, Invalid> {
    let p = urlparse(url)?;
    let mut parts: Vec<String> = p.path.split('/').map(str::to_owned).collect();
    if parts.len() >= 2
        && let Some(last) = parts.last_mut()
    {
        let chars: Vec<char> = last.chars().collect();
        if chars.len() > 6 {
            let head: String = chars[..4].iter().collect();
            let tail: String = chars[chars.len() - 2..].iter().collect();
            *last = format!("{head}\u{2026}{tail}");
        }
    }
    let host = p.hostname().unwrap_or_else(|| "None".to_owned());
    Ok(format!("https://{host}{}", parts.join("/")))
}

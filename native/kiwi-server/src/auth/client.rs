//! `request.client.host` as uvicorn's `--proxy-headers --forwarded-allow-ips`
//! produces it, and the coarse `device_label` from a User-Agent.

use std::{
    net::{IpAddr, SocketAddr},
    sync::OnceLock,
};

use axum::{extract::ConnectInfo, http::request::Parts};

use super::pyutil::{latin1, strip};

/// Set by a front middleware to pin the resolved client IP for a request;
/// when absent it is derived from the peer and X-Forwarded-For.
#[derive(Debug, Clone)]
pub struct ClientIp(pub Option<String>);

/// uvicorn's `_TrustedHosts`.
pub struct TrustedHosts {
    always: bool,
    hosts: Vec<IpAddr>,
    networks: Vec<(IpAddr, u8)>,
    literals: Vec<String>,
}

impl TrustedHosts {
    pub fn parse(spec: &str) -> Self {
        let always = spec == "*";
        let mut t = TrustedHosts { always, hosts: Vec::new(), networks: Vec::new(), literals: Vec::new() };
        if always {
            return t;
        }
        for item in spec.split(',').map(strip) {
            if let Some((addr, len)) = item.split_once('/') {
                match (addr.parse::<IpAddr>(), len.parse::<u8>()) {
                    (Ok(ip), Ok(len)) if len <= max_prefix(ip) && is_network(ip, len) => t.networks.push((ip, len)),
                    _ => t.literals.push(item.to_owned()),
                }
            } else {
                match item.parse::<IpAddr>() {
                    Ok(ip) => t.hosts.push(ip),
                    Err(_) => t.literals.push(item.to_owned()),
                }
            }
        }
        t
    }

    pub fn contains(&self, host: Option<&str>) -> bool {
        if self.always {
            return true;
        }
        let Some(host) = host.filter(|h| !h.is_empty()) else {
            return false;
        };
        match host.parse::<IpAddr>() {
            Ok(ip) => self.hosts.contains(&ip) || self.networks.iter().any(|&(net, len)| in_network(ip, net, len)),
            Err(_) => self.literals.iter().any(|l| l == host),
        }
    }

    /// `get_trusted_client_address`: the right-most untrusted hop.
    pub fn client_from_forwarded(&self, xff: &str) -> String {
        let hops: Vec<&str> = xff.split(',').map(strip).collect();
        if !self.always {
            for hop in hops.iter().rev() {
                let host = parse_host_port(hop);
                if !self.contains(Some(host)) {
                    return host.to_owned();
                }
            }
        }
        parse_host_port(hops[0]).to_owned()
    }
}

fn max_prefix(ip: IpAddr) -> u8 {
    if ip.is_ipv4() { 32 } else { 128 }
}

fn bits(ip: IpAddr) -> u128 {
    match ip {
        IpAddr::V4(v) => u128::from(u32::from(v)),
        IpAddr::V6(v) => u128::from(v),
    }
}

fn mask(ip: IpAddr, len: u8) -> u128 {
    let width = u32::from(max_prefix(ip));
    let full = if width == 32 { u128::from(u32::MAX) } else { u128::MAX };
    if len == 0 { 0 } else { full & !((1u128 << (width - u32::from(len))) - 1) }
}

/// `ip_network(strict=True)` rejects host bits set.
fn is_network(ip: IpAddr, len: u8) -> bool {
    bits(ip) & !mask(ip, len) & mask(ip, max_prefix(ip)) == 0
}

fn in_network(ip: IpAddr, net: IpAddr, len: u8) -> bool {
    ip.is_ipv4() == net.is_ipv4() && bits(ip) & mask(net, len) == bits(net)
}

/// `_parse_host_port`, host half only.
fn parse_host_port(value: &str) -> &str {
    if let Some(inner) = value.strip_prefix('[') {
        let Some(end) = inner.find(']') else {
            return value;
        };
        let rest = &inner[end + 1..];
        if rest.is_empty() || rest.starts_with(':') {
            return &inner[..end];
        }
        return value;
    }
    if value.matches(':').count() == 1 {
        let (host, port) = value.rsplit_once(':').expect("one colon");
        if python_int_like(port) {
            return host;
        }
        return value;
    }
    value
}

fn python_int_like(s: &str) -> bool {
    super::pyutil::int_of(&serde_json::Value::String(s.to_owned())).is_some()
}

/// The client IP for this request, or `None` when unknown (callers pick
/// their own fallback, e.g. `"unknown"` for rate-limit keys).
pub fn client_ip(parts: &Parts) -> Option<String> {
    if let Some(ClientIp(ip)) = parts.extensions.get::<ClientIp>() {
        return ip.clone();
    }
    if let Some(c) = parts.extensions.get::<crate::middleware::forwarded::ClientAddr>() {
        return (!c.host.is_empty()).then(|| c.host.clone());
    }
    static TRUSTED: OnceLock<TrustedHosts> = OnceLock::new();
    let trusted = TRUSTED.get_or_init(|| TrustedHosts::parse(&super::auth_settings().forwarded_allow_ips));
    let peer = parts.extensions.get::<ConnectInfo<SocketAddr>>().map(|c| c.0.ip().to_string());
    resolve_client(trusted, peer, forwarded_for(parts).as_deref())
}

/// All X-Forwarded-For values joined with ", " (latin-1), as uvicorn reads them.
pub fn forwarded_for(parts: &Parts) -> Option<String> {
    let values: Vec<String> = parts.headers.get_all("x-forwarded-for").iter().map(latin1).collect();
    (!values.is_empty()).then(|| values.join(", "))
}

pub fn resolve_client(trusted: &TrustedHosts, peer: Option<String>, xff: Option<&str>) -> Option<String> {
    if trusted.contains(peer.as_deref())
        && let Some(xff) = xff
    {
        let host = trusted.client_from_forwarded(xff);
        if !host.is_empty() {
            return Some(host);
        }
    }
    peer
}

/// `device_label`: "Browser on OS", never the raw UA.
pub fn device_label(user_agent: Option<&str>) -> Option<String> {
    let ua = user_agent.filter(|u| !u.is_empty())?;
    let has = |s: &str| ua.contains(s);
    let browser = if has("Edg") {
        "Edge"
    } else if has("OPR") || has("Opera") {
        "Opera"
    } else if has("Firefox") || has("FxiOS") {
        "Firefox"
    } else if has("Chrome") || has("CriOS") {
        "Chrome"
    } else if has("Safari") {
        "Safari"
    } else {
        "Browser"
    };
    let os = if has("Windows") {
        "Windows"
    } else if has("Android") {
        "Android"
    } else if has("iPhone") || has("iPad") || has("iOS") {
        "iOS"
    } else if has("Mac OS") || has("Macintosh") {
        "macOS"
    } else if has("Linux") {
        "Linux"
    } else {
        ""
    };
    Some(if os.is_empty() { browser.to_owned() } else { format!("{browser} on {os}") })
}

/// The request's User-Agent as Starlette reads it.
pub fn user_agent(parts: &Parts) -> Option<String> {
    parts.headers.get("user-agent").map(latin1)
}

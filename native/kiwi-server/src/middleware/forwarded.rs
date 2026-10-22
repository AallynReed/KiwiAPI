//! uvicorn `--proxy-headers` with `--forwarded-allow-ips` (env
//! `FORWARDED_ALLOW_IPS`, default `127.0.0.1`): which client and scheme a
//! request is from. Rust routes read [`ClientAddr`] from the request
//! extensions; the proxy hands the same answer to Python.

use std::net::{IpAddr, SocketAddr};
use std::sync::OnceLock;

use axum::http::HeaderMap;

use super::pyurl::latin1;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientAddr {
    /// `request.client.host` as Python sees it (not necessarily an IP).
    pub host: String,
    pub scheme: &'static str,
}

#[derive(Debug, Default)]
pub struct TrustedHosts {
    always: bool,
    hosts: Vec<IpAddr>,
    nets: Vec<(IpAddr, u8)>,
    literals: Vec<String>,
}

fn parse_net(s: &str) -> Option<(IpAddr, u8)> {
    let (addr, len) = s.split_once('/')?;
    let ip: IpAddr = addr.parse().ok()?;
    let len: u8 = len.parse().ok()?;
    let max = if ip.is_ipv4() { 32 } else { 128 };
    if len > max {
        return None;
    }
    // ipaddress.ip_network is strict: host bits must be zero.
    let ok = match ip {
        IpAddr::V4(a) => len == 32 || u32::from(a) & (u32::MAX >> len) == 0,
        IpAddr::V6(a) => len == 128 || u128::from(a) & (u128::MAX >> len) == 0,
    };
    ok.then_some((ip, len))
}

fn in_net(ip: IpAddr, (net, len): (IpAddr, u8)) -> bool {
    match (ip, net) {
        (IpAddr::V4(a), IpAddr::V4(n)) => {
            len == 0 || (u32::from(a) ^ u32::from(n)) >> (32 - len as u32) == 0
        }
        (IpAddr::V6(a), IpAddr::V6(n)) => {
            len == 0 || (u128::from(a) ^ u128::from(n)) >> (128 - len as u32) == 0
        }
        _ => false,
    }
}

/// Python `ipaddress.ip_address` accepts no brackets, zones only on v6.
fn parse_ip(s: &str) -> Option<IpAddr> {
    s.parse().ok()
}

impl TrustedHosts {
    pub fn parse(raw: &str) -> Self {
        let raw = raw.trim();
        if raw == "*" {
            return TrustedHosts { always: true, ..Default::default() };
        }
        let mut t = TrustedHosts::default();
        for host in raw.split(',').map(str::trim) {
            if host.contains('/') {
                match parse_net(host) {
                    Some(n) => t.nets.push(n),
                    None => t.literals.push(host.to_owned()),
                }
            } else {
                match parse_ip(host) {
                    Some(ip) => t.hosts.push(ip),
                    None => t.literals.push(host.to_owned()),
                }
            }
        }
        t
    }

    pub fn contains(&self, host: Option<&str>) -> bool {
        if self.always {
            return true;
        }
        let Some(host) = host.filter(|h| !h.is_empty()) else { return false };
        match parse_ip(host) {
            Some(ip) => self.hosts.contains(&ip) || self.nets.iter().any(|n| in_net(ip, *n)),
            None => self.literals.iter().any(|l| l == host),
        }
    }

    /// `get_trusted_client_address`, host part only.
    fn client_from_xff(&self, xff: &str) -> String {
        let hosts: Vec<&str> = xff.split(',').map(str::trim).collect();
        if self.always {
            return parse_host_port(hosts[0]);
        }
        for hp in hosts.iter().rev() {
            let host = parse_host_port(hp);
            if !self.contains(Some(&host)) {
                return host;
            }
        }
        parse_host_port(hosts[0])
    }
}

/// uvicorn `_parse_host_port`, host part.
fn parse_host_port(value: &str) -> String {
    if let Some(rest) = value.strip_prefix('[') {
        let Some(end) = rest.find(']') else { return value.to_owned() };
        let remainder = &rest[end + 1..];
        if remainder.is_empty() || remainder.starts_with(':') {
            return rest[..end].to_owned();
        }
        return value.to_owned();
    }
    if value.matches(':').count() == 1 {
        let (host, port) = value.rsplit_once(':').unwrap_or((value, ""));
        if super::pyurl::py_int(port).is_some() {
            return host.to_owned();
        }
    }
    value.to_owned()
}

pub fn trusted() -> &'static TrustedHosts {
    static T: OnceLock<TrustedHosts> = OnceLock::new();
    T.get_or_init(|| {
        TrustedHosts::parse(&std::env::var("FORWARDED_ALLOW_IPS").unwrap_or_else(|_| "127.0.0.1".into()))
    })
}

/// The client and scheme uvicorn's ProxyHeadersMiddleware would put in scope.
pub fn resolve(trusted: &TrustedHosts, peer: Option<SocketAddr>, headers: &HeaderMap) -> ClientAddr {
    let peer_host = peer.map(|p| p.ip().to_string());
    let mut out = ClientAddr { host: peer_host.clone().unwrap_or_default(), scheme: "http" };
    if !trusted.contains(peer_host.as_deref()) {
        return out;
    }
    let mut proto = None;
    let mut xff: Vec<String> = Vec::new();
    for (name, value) in headers {
        if name == "x-forwarded-proto" {
            proto = Some(latin1(value.as_bytes()));
        } else if name == "x-forwarded-for" {
            xff.push(latin1(value.as_bytes()));
        }
    }
    if let Some(p) = proto {
        out.scheme = match p.trim() {
            "http" => "http",
            "https" => "https",
            "ws" => "ws",
            "wss" => "wss",
            _ => out.scheme,
        };
    }
    if !xff.is_empty() {
        let host = trusted.client_from_xff(&xff.join(", "));
        if !host.is_empty() {
            out.host = host;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn h(xff: &[&str], proto: Option<&str>) -> HeaderMap {
        let mut m = HeaderMap::new();
        for v in xff {
            m.append("x-forwarded-for", HeaderValue::from_str(v).unwrap());
        }
        if let Some(p) = proto {
            m.insert("x-forwarded-proto", HeaderValue::from_str(p).unwrap());
        }
        m
    }

    #[test]
    fn uvicorn_semantics() {
        let t = TrustedHosts::parse("10.8.0.1");
        let edge: SocketAddr = "10.8.0.1:5555".parse().unwrap();
        let other: SocketAddr = "172.18.0.5:5555".parse().unwrap();
        let r = resolve(&t, Some(edge), &h(&["6.6.6.6, 1.2.3.4"], Some("https")));
        assert_eq!(r, ClientAddr { host: "1.2.3.4".into(), scheme: "https" });
        let r = resolve(&t, Some(other), &h(&["1.2.3.4"], Some("https")));
        assert_eq!(r, ClientAddr { host: "172.18.0.5".into(), scheme: "http" });
        let r = resolve(&t, Some(edge), &h(&["10.8.0.1"], None));
        assert_eq!(r.host, "10.8.0.1");
        let r = resolve(&t, Some(edge), &h(&[""], None));
        assert_eq!(r.host, "10.8.0.1");
        let r = resolve(&t, Some(edge), &h(&["[2001:db8::1]:443", "5.6.7.8:99"], None));
        assert_eq!(r.host, "5.6.7.8");
        let star = TrustedHosts::parse("*");
        let r = resolve(&star, Some(other), &h(&["9.9.9.9, 1.2.3.4"], None));
        assert_eq!(r.host, "9.9.9.9");
        let nets = TrustedHosts::parse("127.0.0.1, 172.16.0.0/12, unix");
        assert!(nets.contains(Some("172.20.1.2")));
        assert!(!nets.contains(Some("192.168.1.1")));
        assert!(nets.contains(Some("unix")));
        assert!(!nets.contains(None));
        assert_eq!(parse_host_port("[::1]"), "::1");
        assert_eq!(parse_host_port("a:b"), "a:b");
        assert_eq!(parse_host_port("2001:db8::1"), "2001:db8::1");
    }
}

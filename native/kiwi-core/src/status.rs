//! Pure parts of `app/trove/status.py`: the history window and the prober's verdicts.

use crate::{gems::py_round, pystr::strip};

/// `_ENVIRONMENTS`, in snapshot order.
pub const ENVIRONMENTS: [&str; 3] = ["eu", "us", "pts"];
/// `_LIVE_REGIONS`: the regions `overall` rolls up.
pub const LIVE_REGIONS: [&str; 2] = ["eu", "us"];

/// One stored `TroveStatusEvent`.
#[derive(Clone, Debug, PartialEq)]
pub struct Segment {
    pub status: String,
    pub online: bool,
    pub started_at: i64,
    pub ended_at: Option<i64>,
}

/// A segment clamped to the window; `ended_at` stays `None` for the open one.
#[derive(Clone, Debug, PartialEq)]
pub struct WindowSegment {
    pub status: String,
    pub online: bool,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub duration_seconds: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct History {
    pub env: &'static str,
    pub days: i64,
    pub window_start: i64,
    pub window_end: i64,
    pub uptime: Option<f64>,
    pub covered_seconds: i64,
    pub segments: Vec<WindowSegment>,
}

impl History {
    /// The non-online segments (`outages`).
    pub fn outages(&self) -> impl Iterator<Item = &WindowSegment> {
        self.segments.iter().filter(|s| s.status != "online")
    }
}

/// `get_history`'s env mapping: anything unknown (the route's `live`) is `eu`.
pub fn history_env(env: &str) -> &'static str {
    ENVIRONMENTS.iter().copied().find(|e| *e == env).unwrap_or("eu")
}

/// `get_history(env, days)` over `rows` (one env, sorted by `started_at`) at `now`.
pub fn history(env: &str, days: i64, now: i64, rows: &[Segment]) -> History {
    let env = history_env(env);
    let days = days.clamp(1, 90);
    let window_start = now - days * 86400;
    let mut segments = Vec::new();
    let (mut up, mut covered) = (0i64, 0i64);
    for r in rows {
        let seg_end = r.ended_at.unwrap_or(now);
        if seg_end <= window_start || r.started_at >= now {
            continue;
        }
        let start = r.started_at.max(window_start);
        let end = seg_end.min(now);
        let dur = (end - start).max(0);
        covered += dur;
        if r.status == "online" {
            up += dur;
        }
        segments.push(WindowSegment {
            status: r.status.clone(),
            online: r.online,
            started_at: start,
            ended_at: r.ended_at.map(|_| end),
            duration_seconds: dur,
        });
    }
    let uptime = (covered > 0).then(|| py_round(up as f64 / covered as f64, 5));
    History { env, days, window_start, window_end: now, uptime, covered_seconds: covered, segments }
}

/// `_auth_is_reachable`: `(online, error)` for one login-route reply.
pub fn auth_is_reachable<'a>(
    status: u16,
    header_names: impl IntoIterator<Item = &'a str>,
    body: &str,
) -> (bool, Option<String>) {
    let trion = header_names.into_iter().any(|k| k.to_ascii_lowercase().starts_with("x-trionworlds"));
    if trion || body.contains("Signature:") {
        return (true, None);
    }
    match status {
        404 => (false, Some("login_route_unavailable".into())),
        s if s >= 500 => (false, Some(format!("HTTP {s}"))),
        _ => (true, None),
    }
}

/// `_verdict`.
pub fn verdict(auth_online: bool, game_online: bool) -> &'static str {
    if auth_online && game_online { "online" } else { "down" }
}

/// `_overall` over `(env, status)` pairs.
pub fn overall(environments: &[(&str, &str)]) -> &'static str {
    let live: Vec<&str> =
        LIVE_REGIONS.iter().filter_map(|r| environments.iter().find(|(e, _)| e == r).map(|(_, s)| *s)).collect();
    if live.is_empty() {
        "unknown"
    } else if live.iter().all(|s| *s == "online") {
        "online"
    } else {
        "down"
    }
}

/// The anchors `_local_network_is_up` probes: `host:port` items whose port is
/// all ASCII digits (Python's `isdigit`; other Unicode digits are not handled).
pub fn parse_anchors(hosts: &str) -> Vec<(String, u64)> {
    let mut out = Vec::new();
    for raw in hosts.split(',') {
        let raw = strip(raw);
        let Some((host, port)) = raw.rsplit_once(':') else { continue };
        if host.is_empty() || port.is_empty() || !port.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        out.push((host.to_owned(), port.parse().unwrap_or(u64::MAX)));
    }
    out
}

/// `bytes.fromhex(s.strip())`; `None` where Python raises ValueError.
pub fn fromhex(s: &str) -> Option<Vec<u8>> {
    let s = strip(s);
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i].is_ascii_whitespace() || b[i] == 0x0b {
            i += 1;
            continue;
        }
        let hi = (*b.get(i)? as char).to_digit(16)?;
        let lo = (*b.get(i + 1)? as char).to_digit(16)?;
        out.push((hi * 16 + lo) as u8);
        i += 2;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(status: &str, start: i64, end: Option<i64>) -> Segment {
        Segment { status: status.into(), online: status == "online", started_at: start, ended_at: end }
    }

    #[test]
    fn history_clamps_and_rolls_up() {
        let now = 1_000_000;
        let rows = [
            seg("online", now - 5 * 86400, Some(now - 86400)),
            seg("down", now - 86400, Some(now - 3600)),
            seg("online", now - 3600, None),
            seg("online", now + 10, None),
        ];
        let h = history("live", 2, now, &rows);
        assert_eq!(h.env, "eu");
        assert_eq!(h.window_start, now - 2 * 86400);
        assert_eq!(h.covered_seconds, 2 * 86400);
        assert_eq!(h.segments.len(), 3);
        assert_eq!(h.segments[0].started_at, now - 2 * 86400);
        assert_eq!(h.segments[2].ended_at, None);
        assert_eq!(h.uptime, Some(py_round((86400.0 + 3600.0) / 172800.0, 5)));
        assert_eq!(h.outages().count(), 1);
        assert_eq!(history("pts", 30, now, &[]).uptime, None);
    }

    #[test]
    fn verdicts() {
        assert_eq!(auth_is_reachable(404, ["X-TrionWorlds-Error"], ""), (true, None));
        assert_eq!(auth_is_reachable(404, [], ""), (false, Some("login_route_unavailable".into())));
        assert_eq!(auth_is_reachable(502, [], ""), (false, Some("HTTP 502".into())));
        assert_eq!(auth_is_reachable(302, [], ""), (true, None));
        assert_eq!(overall(&[("pts", "down")]), "unknown");
        assert_eq!(overall(&[("eu", "online"), ("us", "online"), ("pts", "down")]), "online");
        assert_eq!(overall(&[("eu", "online"), ("us", "down")]), "down");
    }

    #[test]
    fn anchors_and_hex() {
        assert_eq!(
            parse_anchors(" a:80 ,b:x,c:0,x:1:2,:1,h:+1"),
            [("a".into(), 80), ("c".into(), 0), ("x:1".into(), 2)]
        );
        assert_eq!(fromhex(" 20 0a "), Some(vec![0x20, 0x0a]));
        assert_eq!(fromhex("2 0"), None);
        assert_eq!(fromhex("abc"), None);
    }
}

//! The settings the middleware reads, from the same environment variables and
//! with the same defaults as `app/core/config.py` (pydantic-settings matches
//! names case-insensitively; list fields are JSON).

use std::path::PathBuf;

use regex::Regex;

const MIB: u64 = 1024 * 1024;

#[derive(Clone, Debug)]
pub struct MwConfig {
    pub app_url: String,
    pub api_side_hosts: Vec<String>,
    pub cors_origins: Vec<String>,
    pub cors_origin_regex: String,
    pub cors_regex: Regex,
    pub max_request_body_bytes: u64,
    pub mods_max_request_body_bytes: u64,
    pub leaderboards_max_request_body_bytes: u64,
    pub market_max_request_body_bytes: u64,
    pub ocr_max_request_body_bytes: u64,
    pub mods_git_max_body_bytes: u64,
    pub sound_studio_max_request_body_bytes: u64,
    pub unlock_debug_max_request_body_bytes: u64,
    pub mod_workshop_max_request_body_bytes: u64,
    pub drops_max_request_body_bytes: u64,
    pub idempotency_lock_seconds: u64,
    pub idempotency_result_seconds: u64,
    pub static_root: PathBuf,
}

const DEFAULT_CORS_ORIGINS: [&str; 6] = [
    "https://docs.aallyn.net",
    "https://dev.aallyn.net",
    "http://localhost:25468",
    "http://127.0.0.1:25468",
    "http://localhost:15546",
    "http://127.0.0.1:15546",
];
const DEFAULT_CORS_REGEX: &str = r"https://([a-z0-9-]+\.)?aallyn\.net|http://(localhost|127\.0\.0\.1)(:\d+)?";

pub(crate) fn env_ci(name: &str) -> Option<String> {
    if let Ok(v) = std::env::var(name.to_ascii_uppercase()) {
        return Some(v);
    }
    std::env::vars().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v)
}

fn parse_u64(get: &dyn Fn(&str) -> Option<String>, name: &str, default: u64) -> u64 {
    match get(name) {
        Some(v) => v.trim().replace('_', "").parse().unwrap_or_else(|_| {
            tracing::warn!("{name}={v:?} is not an integer; using {default}");
            default
        }),
        None => default,
    }
}

fn host_of(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    rest.split('/').next().unwrap_or("").to_lowercase()
}

/// `API_SIDE_HOSTS` of `app/core/middleware.py`: the api host, plus its apex
/// and `www.` apex when it has a subdomain, minus the app host.
pub fn api_side_hosts(api_url: &str, app_url: &str) -> Vec<String> {
    let host = host_of(api_url);
    let app_host = host_of(app_url);
    let mut hosts = vec![host.clone()];
    let labels: Vec<&str> = host.split('.').collect();
    if labels.len() > 2 {
        let apex = labels[1..].join(".");
        hosts.push(format!("www.{apex}"));
        hosts.push(apex);
    }
    hosts.retain(|h| *h != app_host);
    hosts.dedup();
    hosts
}

/// Python `re.fullmatch` for a pattern compiled once.
pub fn fullmatch_regex(src: &str) -> Regex {
    Regex::new(&format!("^(?:{src})$")).unwrap_or_else(|e| {
        tracing::warn!("CORS_ORIGIN_REGEX does not compile ({e}); matching nothing");
        Regex::new("[^\\s\\S]").expect("static regex")
    })
}

impl MwConfig {
    pub fn from_env() -> Self {
        Self::from_lookup(&env_ci)
    }

    /// Settings from any name -> value lookup (lower-case names).
    pub fn from_lookup(get: &dyn Fn(&str) -> Option<String>) -> Self {
        let api_url = get("api_url").unwrap_or_else(|| "https://api.aallyn.net".into());
        let app_url = get("app_url").unwrap_or_else(|| "https://trove.aallyn.net".into());
        let cors_origins = match get("cors_origins") {
            Some(raw) => serde_json::from_str::<Vec<String>>(&raw).unwrap_or_else(|e| {
                tracing::warn!("CORS_ORIGINS is not a JSON list of strings ({e}); using the defaults");
                DEFAULT_CORS_ORIGINS.iter().map(|s| s.to_string()).collect()
            }),
            None => DEFAULT_CORS_ORIGINS.iter().map(|s| s.to_string()).collect(),
        };
        let cors_origin_regex = get("cors_origin_regex").unwrap_or_else(|| DEFAULT_CORS_REGEX.into());
        let site_root = get("site_root").unwrap_or_else(|| "site".into());
        MwConfig {
            api_side_hosts: api_side_hosts(&api_url, &app_url),
            app_url: app_url.trim_end_matches('/').to_owned(),
            cors_regex: fullmatch_regex(&cors_origin_regex),
            cors_origin_regex,
            cors_origins,
            max_request_body_bytes: parse_u64(get, "max_request_body_bytes", 8 * MIB),
            mods_max_request_body_bytes: parse_u64(get, "mods_max_request_body_bytes", 20 * MIB),
            leaderboards_max_request_body_bytes: parse_u64(get, "leaderboards_max_request_body_bytes", 20 * MIB),
            market_max_request_body_bytes: parse_u64(get, "market_max_request_body_bytes", 20 * MIB),
            ocr_max_request_body_bytes: parse_u64(get, "ocr_max_request_body_bytes", 12 * MIB),
            mods_git_max_body_bytes: parse_u64(get, "mods_git_max_body_bytes", 100 * MIB),
            sound_studio_max_request_body_bytes: parse_u64(get, "sound_studio_max_request_body_bytes", 32 * MIB),
            unlock_debug_max_request_body_bytes: parse_u64(get, "unlock_debug_max_request_body_bytes", 160 * MIB),
            mod_workshop_max_request_body_bytes: parse_u64(get, "mod_workshop_max_request_body_bytes", 32 * MIB),
            drops_max_request_body_bytes: parse_u64(get, "drops_max_request_body_bytes", 512 * MIB),
            idempotency_lock_seconds: parse_u64(get, "idempotency_lock_seconds", 60),
            idempotency_result_seconds: parse_u64(get, "idempotency_result_seconds", 86400),
            static_root: PathBuf::from(site_root).join("static"),
        }
    }

    /// The per-surface body cap of the `security` middleware.
    pub fn body_cap(&self, path: &str) -> u64 {
        if path.starts_with("/git/") {
            self.mods_git_max_body_bytes
        } else if path.starts_with("/v1/mods/") {
            self.mods_max_request_body_bytes
        } else if path == "/v1/leaderboards/insert" {
            self.leaderboards_max_request_body_bytes
        } else if path == "/v1/market/insert" {
            self.market_max_request_body_bytes
        } else if path == "/v1/ocr/character" {
            self.ocr_max_request_body_bytes
        } else if path == "/site/sound-studio/build" {
            self.sound_studio_max_request_body_bytes
        } else if path.starts_with("/site/blueprint-editor/") {
            64 * MIB
        } else if path == "/site/unlock-debug" {
            self.unlock_debug_max_request_body_bytes
        } else if path.starts_with("/site/mod-workshop/") {
            self.mod_workshop_max_request_body_bytes
        } else if path.starts_with("/site/drops/") {
            self.drops_max_request_body_bytes
        } else if path == "/v1/misc/feedback" {
            24 * MIB
        } else {
            self.max_request_body_bytes
        }
    }

    /// `PublicAssetCorsMiddleware._is_own_origin`.
    pub fn is_own_origin(&self, origin: &str) -> bool {
        if origin.is_empty() {
            return false;
        }
        self.cors_origins.iter().any(|o| o == origin)
            || (!self.cors_origin_regex.is_empty() && self.cors_regex.is_match(origin))
    }

    /// Starlette `CORSMiddleware.is_allowed_origin`.
    pub fn is_allowed_origin(&self, origin: &str) -> bool {
        self.cors_regex.is_match(origin) || self.cors_origins.iter().any(|o| o == origin)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn side_hosts() {
        let mut h = api_side_hosts("https://api.aallyn.net", "https://trove.aallyn.net");
        h.sort();
        assert_eq!(h, ["aallyn.net", "api.aallyn.net", "www.aallyn.net"]);
        assert_eq!(api_side_hosts("http://localhost:8000", "http://localhost:8000"), Vec::<String>::new());
        let mut h = api_side_hosts("https://api.example.net", "https://example.net");
        h.sort();
        assert_eq!(h, ["api.example.net", "www.example.net"]);
    }
}

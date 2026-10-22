//! Path classes and CSP strings of `app/core/middleware.py` and `app/core/csp.py`,
//! byte for byte. A new site page goes in `PAGE_PATHS` (and in Python's copy
//! while it still runs).

pub const PAGE_PATHS: [&str; 56] = [
    "/",
    "/app",
    "/browse",
    "/documentation",
    "/commands",
    "/leaderboards",
    "/updates",
    "/support",
    "/login",
    "/dashboard",
    "/market",
    "/store",
    "/codexes",
    "/codexes/crafting",
    "/status",
    "/giveaways",
    "/activity",
    "/class-activity",
    "/clubs",
    "/terms",
    "/privacy",
    "/accessibility",
    "/changelog",
    "/mods",
    "/modpacks",
    "/server-time",
    "/swf-docs",
    "/calendar",
    "/streams",
    "/releases",
    "/classes",
    "/star-chart",
    "/gem-simulator",
    "/gem-evaluator",
    "/gem-builds",
    "/calculators",
    "/gems-guide",
    "/abilities",
    "/gem-abilities",
    "/guides",
    "/gem-tools",
    "/allies",
    "/fishing-guide",
    "/gear-guide",
    "/loot-collector-guide",
    "/dressing-room",
    "/sound-studio",
    "/mod-workshop",
    "/tomes",
    "/blueprint-editor",
    "/unlock-debug",
    "/unlock_debug",
    "/search",
    "/zakros-ui-requests",
    "/zakros-ui-stats",
    "/community-sites",
];

pub const PAGE_PREFIXES: [&str; 5] = ["/mods/", "/modpacks/", "/player/", "/drop/", "/swf-docs/"];

pub const EMBED_PATH: &str = "/embed/viewer";

const PUBLIC_ASSET_PREFIXES: [&str; 2] = ["/site/rigs/", "/site/dressing/"];
const PUBLIC_ASSET_PATHS: [&str; 2] = ["/site/render/brdf-map.png", "/site/codexes/render"];

pub const API_CSP: &str = "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; \
connect-src 'self'; img-src 'self' data:; base-uri 'none'; frame-ancestors 'none'";

pub const SITE_CSP: &str = "default-src 'self'; \
script-src 'self' https://challenges.cloudflare.com https://hcaptcha.com https://*.hcaptcha.com; \
style-src 'self' 'unsafe-inline' https://hcaptcha.com https://*.hcaptcha.com; \
font-src 'self' data:; \
img-src 'self' data: blob: https:; \
media-src 'self' blob: https://api.aallyn.net; \
connect-src 'self' https://api.aallyn.net https://challenges.cloudflare.com https://hcaptcha.com https://*.hcaptcha.com; \
frame-src https://challenges.cloudflare.com https://hcaptcha.com https://*.hcaptcha.com; \
base-uri 'none'; frame-ancestors 'none'";

pub fn is_page_path(path: &str) -> bool {
    PAGE_PATHS.contains(&path)
}

/// `_is_site_page`: a page with a canonical home on `app_url`.
pub fn is_site_page(path: &str) -> bool {
    is_page_path(path) || PAGE_PREFIXES.iter().any(|p| path.starts_with(p))
}

/// `_is_site_path`: everything that gets the site CSP.
pub fn is_site_path(path: &str) -> bool {
    is_site_page(path) || path.starts_with("/static/") || path.starts_with("/site/")
}

pub fn is_public_asset(path: &str) -> bool {
    PUBLIC_ASSET_PREFIXES.iter().any(|p| path.starts_with(p)) || PUBLIC_ASSET_PATHS.contains(&path)
}

/// `csp.embed_csp`: the site CSP with `frame-ancestors` swapped for the allowlist.
pub fn embed_csp(origins: &[String]) -> String {
    let base: Vec<&str> =
        SITE_CSP.split(';').map(str::trim).filter(|d| !d.is_empty() && !d.starts_with("frame-ancestors")).collect();
    let ancestors = if origins.is_empty() { "'none'".to_owned() } else { origins.join(" ") };
    format!("{}; frame-ancestors {ancestors}", base.join("; "))
}

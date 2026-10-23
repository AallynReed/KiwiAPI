//! `app/site/feature_map.py`: the site flag map, the path gate, the sitemap
//! page list and the host-aware robots body, plus their resolution
//! (`_resolve_feature_flags`) and `GET /site/feature-flags`.

use axum::{
    extract::{FromRequestParts, State},
    http::{HeaderValue, StatusCode, header, request::Parts},
    response::{IntoResponse, Response},
};
use serde_json::{Map, Value};

use crate::{
    config::Settings,
    error::{self, ApiError},
    features,
    middleware::{config::api_side_hosts, pyurl::scope_path},
    pyjson::PyJson,
    state::AppState,
};

/// `SITE_FEATURE_FLAGS`: template key -> runtime-config flag, in order.
pub const SITE_FEATURE_FLAGS: [(&str, &str); 42] = [
    ("mods_hub_enabled", features::MODS_HUB_FLAG),
    ("market_enabled", features::MARKET_FLAG),
    ("store_enabled", features::STORE_FLAG),
    ("leaderboards_enabled", features::LEADERBOARDS_FLAG),
    ("player_activity_enabled", features::PLAYER_ACTIVITY_FLAG),
    ("class_activity_enabled", features::CLASS_ACTIVITY_FLAG),
    ("clubs_enabled", features::CLUBS_FLAG),
    ("updates_enabled", features::UPDATES_FLAG),
    ("codexes_enabled", features::CODEXES_FLAG),
    ("server_status_enabled", features::SERVER_STATUS_FLAG),
    ("giveaways_enabled", features::GIVEAWAYS_FLAG),
    ("commands_enabled", features::COMMANDS_FLAG),
    ("server_time_enabled", features::SERVER_TIME_FLAG),
    ("webhooks_enabled", features::WEBHOOKS_FLAG),
    ("dm_subscriptions_enabled", features::DM_SUBS_FLAG),
    ("image_studio_enabled", features::IMAGE_STUDIO_FLAG),
    ("calendar_enabled", features::CALENDAR_FLAG),
    ("streams_enabled", features::STREAMS_FLAG),
    ("btt_releases_enabled", features::BTT_RELEASES_FLAG),
    ("classes_enabled", features::CLASSES_FLAG),
    ("star_chart_enabled", features::STAR_CHART_FLAG),
    ("gem_simulator_enabled", features::GEM_SIMULATOR_FLAG),
    ("gem_evaluator_enabled", features::GEM_EVALUATOR_FLAG),
    ("gem_builds_enabled", features::GEM_BUILDS_FLAG),
    ("calculators_enabled", features::CALCULATORS_FLAG),
    ("gems_guide_enabled", features::GEMS_GUIDE_FLAG),
    ("abilities_enabled", features::ABILITIES_FLAG),
    ("guides_enabled", features::GUIDES_FLAG),
    ("allies_enabled", features::ALLIES_FLAG),
    ("gem_tools_enabled", features::GEM_TOOLS_FLAG),
    ("fishing_guide_enabled", features::FISHING_GUIDE_FLAG),
    ("gear_guide_enabled", features::GEAR_GUIDE_FLAG),
    ("loot_collector_guide_enabled", features::LOOT_COLLECTOR_GUIDE_FLAG),
    ("dressing_room_enabled", features::DRESSING_ROOM_FLAG),
    ("dressing_room_page_enabled", features::DRESSING_ROOM_PAGE_FLAG),
    ("sound_studio_enabled", features::SOUND_STUDIO_FLAG),
    ("mod_workshop_enabled", features::MOD_WORKSHOP_FLAG),
    ("blueprint_editor_enabled", features::BLUEPRINT_EDITOR_FLAG),
    ("tomes_enabled", features::TOMES_FLAG),
    ("unlock_debug_enabled", features::UNLOCK_DEBUG_FLAG),
    ("file_drops_enabled", features::FILE_DROPS_FLAG),
    ("community_sites_enabled", features::COMMUNITY_SITES_FLAG),
];

/// A resolved flag map: insertion-ordered, `get` is `dict.get(key, default)`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Flags(pub Vec<(String, bool)>);

impl Flags {
    pub fn get_or(&self, key: &str, default: bool) -> bool {
        self.0.iter().find(|(k, _)| k == key).map_or(default, |(_, v)| *v)
    }

    /// `f[key]` for a key the map always carries.
    pub fn on(&self, key: &str) -> bool {
        self.get_or(key, true)
    }

    pub fn set(&mut self, key: &str, value: bool) {
        match self.0.iter_mut().find(|(k, _)| k == key) {
            Some(e) => e.1 = value,
            None => self.0.push((key.to_owned(), value)),
        }
    }

    pub fn to_json(&self) -> Value {
        Value::Object(self.0.iter().map(|(k, v)| (k.clone(), Value::Bool(*v))).collect::<Map<String, Value>>())
    }

    /// Every flag on, as the map resolves on a fresh install.
    pub fn all_on() -> Flags {
        let mut f = Flags(SITE_FEATURE_FLAGS.iter().map(|(k, _)| ((*k).to_owned(), true)).collect());
        apply_derived(&mut f);
        f
    }
}

/// `apply_derived`: the Dressing Room page is off whenever its system is.
pub fn apply_derived(f: &mut Flags) {
    let page = f.get_or("dressing_room_page_enabled", true) && f.get_or("dressing_room_enabled", true);
    f.set("dressing_room_page_enabled", page);
}

fn lookup_failed(e: anyhow::Error) -> ApiError {
    tracing::error!("site feature flag lookup failed: {e:#}");
    ApiError::internal()
}

/// Every site flag through `features.is_enabled`, then `apply_derived`.
pub async fn resolve(state: &AppState) -> Result<Flags, ApiError> {
    let mut out = Vec::with_capacity(SITE_FEATURE_FLAGS.len());
    for (attr, flag) in SITE_FEATURE_FLAGS {
        out.push((attr.to_owned(), features::is_enabled(state, flag).await.map_err(lookup_failed)?));
    }
    let mut f = Flags(out);
    apply_derived(&mut f);
    Ok(f)
}

pub async fn is_on(state: &AppState, flag: &str) -> Result<bool, ApiError> {
    features::is_enabled(state, flag).await.map_err(lookup_failed)
}

/// `feature_blocks(p, f)`: the path belongs to a feature that is off.
pub fn feature_blocks(p: &str, f: &Flags) -> bool {
    let on = |k: &str| f.on(k);
    let starts = |prefix: &str| p.starts_with(prefix);
    if !on("mods_hub_enabled")
        && (p == "/mods"
            || starts("/mods/")
            || starts("/site/mods/")
            || p == "/modpacks"
            || starts("/modpacks/")
            || starts("/site/modpacks/"))
    {
        return true;
    }
    if !on("market_enabled") && (p == "/market" || starts("/site/market/")) {
        return true;
    }
    if !on("store_enabled") && (p == "/store" || starts("/site/store/")) {
        return true;
    }
    if !on("leaderboards_enabled")
        && (p == "/leaderboards"
            || starts("/player/")
            || (starts("/site/leaderboards/")
                && !starts("/site/leaderboards/activity")
                && !starts("/site/leaderboards/class-activity")))
    {
        return true;
    }
    if !on("player_activity_enabled")
        && (p == "/activity" || starts("/activity/") || starts("/site/leaderboards/activity"))
    {
        return true;
    }
    if !on("class_activity_enabled") && (p == "/class-activity" || starts("/site/leaderboards/class-activity")) {
        return true;
    }
    if !on("clubs_enabled") && (p == "/clubs" || p == "/site/clubs") {
        return true;
    }
    if !on("updates_enabled") && (p == "/updates" || starts("/site/updates/")) {
        return true;
    }
    if !on("codexes_enabled") && (p == "/codexes" || p == "/codexes/crafting" || starts("/site/codexes/")) {
        return true;
    }
    if !on("server_status_enabled") && (p == "/status" || starts("/status/") || starts("/site/trove-status")) {
        return true;
    }
    if !on("giveaways_enabled") && (p == "/giveaways" || p == "/site/giveaways") {
        return true;
    }
    if !on("commands_enabled") && p == "/commands" {
        return true;
    }
    if !on("server_time_enabled") && (p == "/server-time" || p == "/site/server-time") {
        return true;
    }
    if !on("calendar_enabled") && (p == "/calendar" || starts("/site/calendar")) {
        return true;
    }
    if !on("streams_enabled") && p == "/streams" {
        return true;
    }
    if !on("btt_releases_enabled") && (p == "/releases" || starts("/site/btt")) {
        return true;
    }
    if !on("classes_enabled") && (p == "/classes" || starts("/site/stats/classes")) {
        return true;
    }
    if !on("star_chart_enabled") && p == "/star-chart" {
        return true;
    }
    if !on("sound_studio_enabled") && (p == "/sound-studio" || starts("/site/sound-studio")) {
        return true;
    }
    if !on("mod_workshop_enabled") && (p == "/mod-workshop" || starts("/site/mod-workshop")) {
        return true;
    }
    if !on("blueprint_editor_enabled") && (p == "/blueprint-editor" || starts("/site/blueprint-editor")) {
        return true;
    }
    if !on("gem_simulator_enabled") && p == "/gem-simulator" {
        return true;
    }
    if !on("gem_evaluator_enabled")
        && (p == "/gem-evaluator"
            || matches!(
                p,
                "/site/gems/evaluate" | "/site/gems/evaluate-simple" | "/site/gems/stat-range" | "/site/gems/lookups"
            ))
    {
        return true;
    }
    if !on("gem_builds_enabled") && (p == "/gem-builds" || starts("/site/gems/builds")) {
        return true;
    }
    if !on("calculators_enabled") && p == "/calculators" {
        return true;
    }
    if !on("dressing_room_page_enabled") && p == "/dressing-room" {
        return true;
    }
    if !on("dressing_room_enabled") && starts("/site/dressing/") {
        return true;
    }
    if !on("abilities_enabled") && matches!(p, "/abilities" | "/gem-abilities") {
        return false;
    }
    if !on("guides_enabled") && p == "/guides" {
        return false;
    }
    if !on("allies_enabled") && p == "/allies" {
        return false;
    }
    if !on("gem_tools_enabled") && p == "/gem-tools" {
        return false;
    }
    if !on("gems_guide_enabled") && p == "/gems-guide" {
        return true;
    }
    if !on("fishing_guide_enabled") && p == "/fishing-guide" {
        return true;
    }
    if !on("gear_guide_enabled") && p == "/gear-guide" {
        return true;
    }
    if !on("loot_collector_guide_enabled") && p == "/loot-collector-guide" {
        return true;
    }
    if !on("community_sites_enabled") && p == "/community-sites" {
        return true;
    }
    if !on("tomes_enabled") && (p == "/tomes" || p == "/site/tomes") {
        return true;
    }
    if !on("unlock_debug_enabled") && matches!(p, "/unlock-debug" | "/unlock_debug" | "/site/unlock-debug") {
        return true;
    }
    if !on("file_drops_enabled") && (starts("/drop/") || starts("/site/drops/")) {
        return true;
    }
    if !on("calculators_enabled") && !on("gem_builds_enabled") && p == "/site/gems/parse-star-chart" {
        return true;
    }
    false
}

/// `SITEMAP_PAGES`: (path, flag that must be on; `None` = always listed).
pub const SITEMAP_PAGES: [(&str, Option<&str>); 50] = [
    ("/", None),
    ("/app", None),
    ("/browse", None),
    ("/documentation", None),
    ("/swf-docs", None),
    ("/support", None),
    ("/terms", None),
    ("/privacy", None),
    ("/accessibility", None),
    ("/changelog", None),
    ("/commands", Some("commands_enabled")),
    ("/classes", Some("classes_enabled")),
    ("/star-chart", Some("star_chart_enabled")),
    ("/gem-simulator", Some("gem_simulator_enabled")),
    ("/gems-guide", Some("gems_guide_enabled")),
    ("/abilities", Some("abilities_enabled")),
    ("/guides", Some("guides_enabled")),
    ("/allies", Some("allies_enabled")),
    ("/gem-tools", Some("gem_tools_enabled")),
    ("/gem-evaluator", Some("gem_evaluator_enabled")),
    ("/gem-builds", Some("gem_builds_enabled")),
    ("/calculators", Some("calculators_enabled")),
    ("/zakros-ui-stats", None),
    ("/fishing-guide", Some("fishing_guide_enabled")),
    ("/gear-guide", Some("gear_guide_enabled")),
    ("/loot-collector-guide", Some("loot_collector_guide_enabled")),
    ("/dressing-room", Some("dressing_room_page_enabled")),
    ("/sound-studio", Some("sound_studio_enabled")),
    ("/mod-workshop", Some("mod_workshop_enabled")),
    ("/blueprint-editor", Some("blueprint_editor_enabled")),
    ("/leaderboards", Some("leaderboards_enabled")),
    ("/activity", Some("player_activity_enabled")),
    ("/class-activity", Some("class_activity_enabled")),
    ("/updates", Some("updates_enabled")),
    ("/market", Some("market_enabled")),
    ("/tomes", Some("tomes_enabled")),
    ("/community-sites", Some("community_sites_enabled")),
    ("/store", Some("store_enabled")),
    ("/codexes", Some("codexes_enabled")),
    ("/codexes/crafting", Some("codexes_enabled")),
    ("/status", Some("server_status_enabled")),
    ("/giveaways", Some("giveaways_enabled")),
    ("/clubs", Some("clubs_enabled")),
    ("/server-time", Some("server_time_enabled")),
    ("/calendar", Some("calendar_enabled")),
    ("/streams", Some("streams_enabled")),
    ("/releases", Some("btt_releases_enabled")),
    ("/mods", Some("mods_hub_enabled")),
    ("/mods/why", Some("mods_hub_enabled")),
    ("/modpacks", Some("mods_hub_enabled")),
];

fn host_of(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    rest.split('/').next().unwrap_or("").to_lowercase()
}

/// `robots_body(host)`.
pub fn robots_body(s: &Settings, host: &str) -> String {
    let host = host.to_lowercase();
    if api_side_hosts(&s.api_url, &s.app_url).contains(&host) {
        return "User-agent: *\nAllow: /\nDisallow: /v1/\nDisallow: /site/\nDisallow: /git/\n".into();
    }
    if host == host_of(&s.api_url) || host == host_of(&s.dev_url) {
        return "User-agent: *\nDisallow: /\n".into();
    }
    format!(
        "User-agent: *\nAllow: /\nDisallow: /dashboard\nDisallow: /login\nDisallow: /site/\n\nSitemap: {}/sitemap.xml\n",
        s.app_url.trim_end_matches('/')
    )
}

/// The site router's `_resolve_feature_flags` dependency: the resolved map,
/// or the framework 404 for a path whose feature is off.
pub struct SiteGate(pub Flags);

impl FromRequestParts<AppState> for SiteGate {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Response> {
        let flags = resolve(state).await.map_err(IntoResponse::into_response)?;
        let path = scope_path(&parts.uri);
        if feature_blocks(&path, &flags) {
            return Err(error::http_exception(StatusCode::NOT_FOUND, None, &parts.method, &path, &parts.headers));
        }
        Ok(SiteGate(flags))
    }
}

/// The calculation switches `/site/feature-flags` and search add to the map.
pub async fn with_calc_flags(state: &AppState, mut f: Flags, renames_key: &str) -> Result<Flags, ApiError> {
    f.set("cheater_detection_enabled", is_on(state, features::CHEATER_DETECTION_FLAG).await?);
    f.set("alt_clusters_enabled", is_on(state, features::ALT_CLUSTERS_FLAG).await?);
    f.set(renames_key, is_on(state, features::RENAMES_FLAG).await?);
    Ok(f)
}

pub async fn feature_flags(_: SiteGate, State(state): State<AppState>) -> Result<Response, ApiError> {
    let mut f = with_calc_flags(&state, resolve(&state).await?, "renames_enabled").await?;
    f.set("duplicates_enabled", is_on(&state, features::DUPLICATES_FLAG).await?);
    let mut res = PyJson(f.to_json()).into_response();
    res.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=5"));
    Ok(res)
}

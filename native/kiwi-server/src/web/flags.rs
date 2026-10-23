//! `app/web/feature_flags.py` + `app/site/feature_map.py`: the resolved flag map
//! fetched from the api (memoised 5s, all-on when the api is unreachable), the
//! path gate (`feature_blocks`) and the sitemap's static page list.

use std::time::{Duration, Instant};

use serde_json::{Map, Value};
use tokio::sync::Mutex;

use super::internal::InternalApi;

/// `SITE_FEATURE_FLAGS` keys, in order.
pub const SITE_FLAGS: [&str; 42] = [
    "mods_hub_enabled",
    "market_enabled",
    "store_enabled",
    "leaderboards_enabled",
    "player_activity_enabled",
    "class_activity_enabled",
    "clubs_enabled",
    "updates_enabled",
    "codexes_enabled",
    "server_status_enabled",
    "giveaways_enabled",
    "commands_enabled",
    "server_time_enabled",
    "webhooks_enabled",
    "dm_subscriptions_enabled",
    "image_studio_enabled",
    "calendar_enabled",
    "streams_enabled",
    "btt_releases_enabled",
    "classes_enabled",
    "star_chart_enabled",
    "gem_simulator_enabled",
    "gem_evaluator_enabled",
    "gem_builds_enabled",
    "calculators_enabled",
    "gems_guide_enabled",
    "abilities_enabled",
    "guides_enabled",
    "allies_enabled",
    "gem_tools_enabled",
    "fishing_guide_enabled",
    "gear_guide_enabled",
    "loot_collector_guide_enabled",
    "dressing_room_enabled",
    "dressing_room_page_enabled",
    "sound_studio_enabled",
    "mod_workshop_enabled",
    "blueprint_editor_enabled",
    "tomes_enabled",
    "unlock_debug_enabled",
    "file_drops_enabled",
    "community_sites_enabled",
];

const EXTRA_FLAGS: [&str; 4] =
    ["cheater_detection_enabled", "alt_clusters_enabled", "renames_enabled", "duplicates_enabled"];

const TTL: Duration = Duration::from_secs(5);

/// Python truthiness of a JSON value.
pub fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// The resolved map (`{**_DEFAULT, **data}`).
#[derive(Clone, Debug)]
pub struct Flags(pub Map<String, Value>);

impl Flags {
    pub fn all_on() -> Self {
        Flags(SITE_FLAGS.iter().chain(EXTRA_FLAGS.iter()).map(|k| ((*k).to_owned(), Value::Bool(true))).collect())
    }

    /// `getattr(request.state, attr, True)`.
    pub fn value(&self, attr: &str) -> Value {
        self.0.get(attr).cloned().unwrap_or(Value::Bool(true))
    }

    pub fn on(&self, attr: &str) -> bool {
        truthy(&self.value(attr))
    }

    fn f(&self, attr: &str) -> bool {
        self.0.get(attr).is_some_and(truthy)
    }
}

pub struct FlagCache {
    api: InternalApi,
    memo: Mutex<Option<(Instant, Flags)>>,
}

impl FlagCache {
    pub fn new(api: InternalApi) -> Self {
        FlagCache { api, memo: Mutex::new(None) }
    }

    /// `_fetch()`: the memo is shared, a failure caches the all-on map too.
    pub async fn get(&self) -> Flags {
        if let Some((at, flags)) = self.memo.lock().await.as_ref()
            && at.elapsed() <= TTL
        {
            return flags.clone();
        }
        let mut flags = Flags::all_on();
        if let Some(Value::Object(data)) = self.api.get("/site/feature-flags", &[], 8.0, &[]).await {
            for (k, v) in data {
                flags.0.insert(k, v);
            }
        }
        *self.memo.lock().await = Some((Instant::now(), flags.clone()));
        flags
    }
}

/// `feature_blocks(p, f)`, rule for rule (the four early `return False`s
/// included: those pages are never blocked).
pub fn feature_blocks(p: &str, flags: &Flags) -> bool {
    let f = |k: &str| flags.f(k);
    let sw = |s: &str| p.starts_with(s);
    if !f("mods_hub_enabled")
        && (p == "/mods"
            || sw("/mods/")
            || sw("/site/mods/")
            || p == "/modpacks"
            || sw("/modpacks/")
            || sw("/site/modpacks/"))
    {
        return true;
    }
    if !f("market_enabled") && (p == "/market" || sw("/site/market/")) {
        return true;
    }
    if !f("store_enabled") && (p == "/store" || sw("/site/store/")) {
        return true;
    }
    if !f("leaderboards_enabled")
        && (p == "/leaderboards"
            || sw("/player/")
            || (sw("/site/leaderboards/")
                && !sw("/site/leaderboards/activity")
                && !sw("/site/leaderboards/class-activity")))
    {
        return true;
    }
    if !f("player_activity_enabled") && (p == "/activity" || sw("/activity/") || sw("/site/leaderboards/activity")) {
        return true;
    }
    if !f("class_activity_enabled") && (p == "/class-activity" || sw("/site/leaderboards/class-activity")) {
        return true;
    }
    if !f("clubs_enabled") && (p == "/clubs" || p == "/site/clubs") {
        return true;
    }
    if !f("updates_enabled") && (p == "/updates" || sw("/site/updates/")) {
        return true;
    }
    if !f("codexes_enabled") && (p == "/codexes" || p == "/codexes/crafting" || sw("/site/codexes/")) {
        return true;
    }
    if !f("server_status_enabled") && (p == "/status" || sw("/status/") || sw("/site/trove-status")) {
        return true;
    }
    if !f("giveaways_enabled") && (p == "/giveaways" || p == "/site/giveaways") {
        return true;
    }
    if !f("commands_enabled") && p == "/commands" {
        return true;
    }
    if !f("server_time_enabled") && (p == "/server-time" || p == "/site/server-time") {
        return true;
    }
    if !f("calendar_enabled") && (p == "/calendar" || sw("/site/calendar")) {
        return true;
    }
    if !f("streams_enabled") && p == "/streams" {
        return true;
    }
    if !f("btt_releases_enabled") && (p == "/releases" || sw("/site/btt")) {
        return true;
    }
    if !f("classes_enabled") && (p == "/classes" || sw("/site/stats/classes")) {
        return true;
    }
    if !f("star_chart_enabled") && p == "/star-chart" {
        return true;
    }
    if !f("sound_studio_enabled") && (p == "/sound-studio" || sw("/site/sound-studio")) {
        return true;
    }
    if !f("mod_workshop_enabled") && (p == "/mod-workshop" || sw("/site/mod-workshop")) {
        return true;
    }
    if !f("blueprint_editor_enabled") && (p == "/blueprint-editor" || sw("/site/blueprint-editor")) {
        return true;
    }
    if !f("gem_simulator_enabled") && p == "/gem-simulator" {
        return true;
    }
    if !f("gem_evaluator_enabled")
        && (p == "/gem-evaluator"
            || ["/site/gems/evaluate", "/site/gems/evaluate-simple", "/site/gems/stat-range", "/site/gems/lookups"]
                .contains(&p))
    {
        return true;
    }
    if !f("gem_builds_enabled") && (p == "/gem-builds" || sw("/site/gems/builds")) {
        return true;
    }
    if !f("calculators_enabled") && p == "/calculators" {
        return true;
    }
    if !f("dressing_room_page_enabled") && p == "/dressing-room" {
        return true;
    }
    if !f("dressing_room_enabled") && sw("/site/dressing/") {
        return true;
    }
    if !f("abilities_enabled") && (p == "/abilities" || p == "/gem-abilities") {
        return false;
    }
    if !f("guides_enabled") && p == "/guides" {
        return false;
    }
    if !f("allies_enabled") && p == "/allies" {
        return false;
    }
    if !f("gem_tools_enabled") && p == "/gem-tools" {
        return false;
    }
    if !f("gems_guide_enabled") && p == "/gems-guide" {
        return true;
    }
    if !f("fishing_guide_enabled") && p == "/fishing-guide" {
        return true;
    }
    if !f("gear_guide_enabled") && p == "/gear-guide" {
        return true;
    }
    if !f("loot_collector_guide_enabled") && p == "/loot-collector-guide" {
        return true;
    }
    if !f("community_sites_enabled") && p == "/community-sites" {
        return true;
    }
    if !f("tomes_enabled") && (p == "/tomes" || p == "/site/tomes") {
        return true;
    }
    if !f("unlock_debug_enabled") && ["/unlock-debug", "/unlock_debug", "/site/unlock-debug"].contains(&p) {
        return true;
    }
    if !f("file_drops_enabled") && (sw("/drop/") || sw("/site/drops/")) {
        return true;
    }
    !f("calculators_enabled") && !f("gem_builds_enabled") && p == "/site/gems/parse-star-chart"
}

/// `SITEMAP_PAGES`: (path, flag that must be on).
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

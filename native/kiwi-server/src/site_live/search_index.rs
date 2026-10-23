//! `app/site/search_index.py`: every page and tab of the site, how a query
//! matches them, and the navbar's Pages menu built from the same registry.

use serde_json::{Value, json};
use unicode_normalization::{UnicodeNormalization, char::canonical_combining_class};

use crate::modpacks::text::casefold;

pub const SUBJECT_PAGES: &str = "pages";
pub const SUBJECT_COLLECTIONS: &str = "collections";
pub const SUBJECT_ITEMS: &str = "items";
pub const SUBJECT_RECIPES: &str = "recipes";
pub const SUBJECT_STYLES: &str = "styles";
pub const SUBJECT_PLAYERS: &str = "players";
pub const SUBJECT_MODS: &str = "mods";
pub const SUBJECT_MODPACKS: &str = "modpacks";

pub const SUBJECT_ORDER: [&str; 8] = [
    SUBJECT_PAGES,
    SUBJECT_COLLECTIONS,
    SUBJECT_ITEMS,
    SUBJECT_RECIPES,
    SUBJECT_STYLES,
    SUBJECT_PLAYERS,
    SUBJECT_MODS,
    SUBJECT_MODPACKS,
];

pub fn subject_label(subject: &str) -> &'static str {
    match subject {
        SUBJECT_PAGES => "Pages",
        SUBJECT_COLLECTIONS => "Collections",
        SUBJECT_ITEMS => "Items",
        SUBJECT_RECIPES => "Recipes",
        SUBJECT_STYLES => "Styles",
        SUBJECT_PLAYERS => "Players",
        SUBJECT_MODS => "Mods",
        _ => "Modpacks",
    }
}

/// `CODEX_TYPE_SUBJECT`, in order.
pub const CODEX_TYPE_SUBJECT: [(&str, &str); 18] = [
    ("ally", SUBJECT_COLLECTIONS),
    ("mount", SUBJECT_COLLECTIONS),
    ("dragon", SUBJECT_COLLECTIONS),
    ("badge", SUBJECT_COLLECTIONS),
    ("wings", SUBJECT_COLLECTIONS),
    ("aura", SUBJECT_COLLECTIONS),
    ("boat", SUBJECT_COLLECTIONS),
    ("sail", SUBJECT_COLLECTIONS),
    ("flask", SUBJECT_COLLECTIONS),
    ("tome", SUBJECT_COLLECTIONS),
    ("magrider", SUBJECT_COLLECTIONS),
    ("fishingpole", SUBJECT_COLLECTIONS),
    ("skin", SUBJECT_COLLECTIONS),
    ("item", SUBJECT_ITEMS),
    ("fish", SUBJECT_ITEMS),
    ("memento", SUBJECT_ITEMS),
    ("recipe", SUBJECT_RECIPES),
    ("style", SUBJECT_STYLES),
];

/// The codex types a subject spans, in `CODEX_TYPE_SUBJECT` order.
pub fn codex_types(subject: &str) -> Vec<&'static str> {
    CODEX_TYPE_SUBJECT.iter().filter(|(_, s)| *s == subject).map(|(t, _)| *t).collect()
}

#[derive(Debug, PartialEq, Eq)]
pub struct Destination {
    pub name: &'static str,
    pub path: &'static str,
    pub flag: Option<&'static str>,
    pub icon: &'static str,
    pub group: &'static str,
    pub parent: &'static str,
    pub keywords: &'static [&'static str],
    pub badge: &'static str,
    pub in_nav: bool,
}

impl Destination {
    pub fn is_tab(&self) -> bool {
        !self.parent.is_empty()
    }

    /// `to_row`.
    pub fn row(&self) -> Value {
        json!({
            "name": self.name,
            "path": self.path,
            "icon": self.icon,
            "group": self.group,
            "parent": self.parent,
            "kind": if self.is_tab() { "tab" } else { "page" },
        })
    }
}

const fn page(
    name: &'static str,
    path: &'static str,
    flag: Option<&'static str>,
    icon: &'static str,
    group: &'static str,
    keywords: &'static [&'static str],
) -> Destination {
    Destination { name, path, flag, icon, group, parent: "", keywords, badge: "", in_nav: true }
}

const fn hidden(d: Destination) -> Destination {
    Destination { in_nav: false, ..d }
}

const fn beta(d: Destination) -> Destination {
    Destination { badge: "Beta", ..d }
}

const fn tab(
    name: &'static str,
    path: &'static str,
    flag: &'static str,
    parent: &'static str,
    keywords: &'static [&'static str],
) -> Destination {
    Destination {
        name,
        path,
        flag: Some(flag),
        icon: "fa-solid fa-table-columns",
        group: parent,
        parent,
        keywords,
        badge: "",
        in_nav: false,
    }
}

#[rustfmt::skip]
pub static PAGES: [Destination; 44] = [
    page("Leaderboards", "/leaderboards", Some("leaderboards_enabled"), "fa-solid fa-ranking-star", "Live", &["ranking", "top", "standings"]),
    page("Clubs", "/clubs", Some("clubs_enabled"), "fa-solid fa-shield-halved", "Live", &["guild"]),
    page("Player Activity", "/activity", Some("player_activity_enabled"), "fa-solid fa-chart-line", "Live", &["online", "population"]),
    page("Class Activity", "/class-activity", Some("class_activity_enabled"), "fa-solid fa-users", "Live", &["classes played"]),
    page("Server Status", "/status", Some("server_status_enabled"), "fa-solid fa-signal", "Live", &["uptime", "down", "maintenance"]),
    page("Server Time", "/server-time", Some("server_time_enabled"), "fa-solid fa-clock", "Live", &["clock", "timezone", "utc"]),
    page("Calendar", "/calendar", Some("calendar_enabled"), "fa-solid fa-calendar-days", "Live", &["events", "schedule"]),

    page("Market", "/market", Some("market_enabled"), "fa-solid fa-shop", "Economy", &["marketplace", "prices", "trading", "flux"]),
    page("Tome Values", "/tomes", Some("tomes_enabled"), "fa-solid fa-book", "Economy", &["tomes"]),
    page("Store History", "/store", Some("store_enabled"), "fa-solid fa-clock-rotate-left", "Economy", &["shop", "credits"]),
    page("Calculators", "/calculators", Some("calculators_enabled"), "fa-solid fa-calculator", "Economy", &["math"]),

    page("Star Chart", "/star-chart", Some("star_chart_enabled"), "fa-solid fa-star", "Plan", &["constellation", "build planner"]),
    page("Gem Tools", "/gem-tools", Some("gem_tools_enabled"), "fa-solid fa-gem", "Plan", &["simulator", "evaluator", "builds", "optimizer"]),
    beta(page("Dressing Room", "/dressing-room", Some("dressing_room_page_enabled"), "fa-solid fa-shirt", "Plan", &["costume", "outfit", "preview"])),
    hidden(page("Gem Simulator", "/gem-simulator", Some("gem_simulator_enabled"), "fa-solid fa-gem", "Plan", &["gems", "rolling"])),
    hidden(page("Gem Evaluator", "/gem-evaluator", Some("gem_evaluator_enabled"), "fa-solid fa-magnifying-glass-chart", "Plan", &["gems", "rate my gem"])),
    hidden(page("Gem Builds", "/gem-builds", Some("gem_builds_enabled"), "fa-solid fa-wand-magic-sparkles", "Plan", &["gems", "optimizer"])),
    page("Abilities", "/abilities", Some("abilities_enabled"), "fa-solid fa-bolt", "Plan", &["gems", "rings", "empowered", "what does this do"]),
    page("Allies", "/allies", Some("allies_enabled"), "fa-solid fa-paw", "Plan", &["pets", "companion", "light", "stats"]),

    page("Mods Hub", "/mods", Some("mods_hub_enabled"), "fa-solid fa-cubes", "Create", &["modding", "addons"]),
    page("Modpacks", "/modpacks", Some("mods_hub_enabled"), "fa-solid fa-box-open", "Create", &["bundles"]),
    page("Mod Workshop", "/mod-workshop", Some("mod_workshop_enabled"), "fa-solid fa-screwdriver-wrench", "Create", &["tmod", "compiler", "build a mod"]),
    page("Blueprint Editor", "/blueprint-editor", Some("blueprint_editor_enabled"), "fa-solid fa-cube", "Create", &["voxel", "blueprint", "recolour", "recolor", "materials", "glass", "model", "mount", "creature", "parts"]),
    page("Sound Studio", "/sound-studio", Some("sound_studio_enabled"), "fa-solid fa-sliders", "Create", &["audio", "music", "bnk"]),
    page("Unlock Debug", "/unlock-debug", Some("unlock_debug_enabled"), "fa-solid fa-terminal", "Create", &["debug console", "trove.exe", "patch", "byte patch", "console"]),

    page("Classes", "/classes", Some("classes_enabled"), "fa-solid fa-hat-wizard", "Learn", &["class list"]),
    page("Trove Commands", "/commands", Some("commands_enabled"), "fa-solid fa-keyboard", "Learn", &["slash commands", "chat"]),
    beta(page("Codexes", "/codexes", Some("codexes_enabled"), "fa-solid fa-book-atlas", "Learn", &["database", "catalog", "game data"])),
    page("Guides", "/guides", Some("guides_enabled"), "fa-solid fa-compass", "Learn", &["how to", "explainer", "tutorial"]),
    hidden(page("Gems Guide", "/gems-guide", Some("gems_guide_enabled"), "fa-solid fa-circle-question", "Learn", &["gems", "guide", "explainer", "how gems work"])),
    hidden(page("Fishing Guide", "/fishing-guide", Some("fishing_guide_enabled"), "fa-solid fa-fish", "Learn", &["fish", "pools", "lures", "bait", "turtles", "guide"])),
    hidden(page("Gear Guide", "/gear-guide", Some("gear_guide_enabled"), "fa-solid fa-shield-halved", "Learn", &["gear", "equipment", "rarity", "power rank", "forge", "pearl", "star", "banner", "torch"])),
    hidden(page("Loot Collector Rules", "/loot-collector-guide", Some("loot_collector_guide_enabled"), "fa-solid fa-filter", "Learn", &["loot", "collector", "rules", "mod", "zakros", "deconstruct", "guide"])),
    hidden(page("Recipe Cost Calculator", "/codexes/crafting", Some("codexes_enabled"), "fa-solid fa-flask", "Learn", &["crafting", "cost"])),
    page("Updates", "/updates", Some("updates_enabled"), "fa-solid fa-code-branch", "Learn", &["patch notes", "changes", "datamining"]),
    page("Streams", "/streams", Some("streams_enabled"), "fa-solid fa-video", "Learn", &["twitch", "live"]),
    page("Community Sites", "/community-sites", Some("community_sites_enabled"), "fa-solid fa-handshake", "Learn", &["trovesaurus", "soruden", "mystic cave", "other sites", "fan sites", "links", "french", "spanish"]),
    page("Giveaways", "/giveaways", Some("giveaways_enabled"), "fa-solid fa-gift", "Learn", &["free"]),
    page("App Releases", "/releases", Some("btt_releases_enabled"), "fa-solid fa-cloud-arrow-down", "Learn", &["app", "download", "versions"]),

    hidden(page("Get the App", "/app", None, "fa-solid fa-download", "About", &["desktop", "install"])),
    hidden(page("Dashboard", "/dashboard", None, "fa-solid fa-gauge", "About", &["account", "settings", "profile"])),
    hidden(page("Changelog", "/changelog", None, "fa-solid fa-list-check", "About", &["site updates"])),
    hidden(page("Accessibility", "/accessibility", None, "fa-solid fa-universal-access", "About", &["a11y"])),
    hidden(page("Support", "/support", None, "fa-solid fa-heart", "About", &["donate", "tip"])),
];

#[rustfmt::skip]
pub static TABS: [Destination; 17] = [
    tab("Gem Abilities", "/abilities?tab=gems", "abilities_enabled", "Abilities", &["empowered", "gems"]),
    tab("Ring Abilities", "/abilities?tab=rings", "abilities_enabled", "Abilities", &["rings", "mods"]),
    tab("Class Abilities", "/abilities?tab=classes", "abilities_enabled", "Abilities", &["classes", "skills"]),
    tab("Star Chart Abilities", "/abilities?tab=star-chart", "abilities_enabled", "Abilities", &["constellation", "nodes"]),
    tab("Ally Abilities", "/abilities?tab=allies", "abilities_enabled", "Abilities", &["allies", "pets"]),

    tab("Market Analytics", "/market?tab=analytics", "market_enabled", "Market", &["movers", "liquidity", "volume", "deals"]),
    tab("Market Listings", "/market?tab=listings", "market_enabled", "Market", &["prices", "sell"]),

    tab("Possible Renames", "/leaderboards?tab=renames", "leaderboard_renames_enabled", "Leaderboards", &["name change"]),
    tab("Cheaters", "/leaderboards?tab=cheaters", "cheater_detection_enabled", "Leaderboards", &["anti-cheat", "suspicious"]),
    tab("Alt Clusters", "/leaderboards?tab=clusters", "alt_clusters_enabled", "Leaderboards", &["alts", "multi-account"]),

    tab("Updates: Files", "/updates?tab=files", "updates_enabled", "Updates", &["archive", "browse", "tree"]),
    tab("Updates: Audio", "/updates?tab=audio", "updates_enabled", "Updates", &["bnk", "sound", "music"]),
    tab("Updates: Interface", "/updates?tab=interface", "updates_enabled", "Updates", &["swf", "ui", "flash"]),

    tab("Store Gallery", "/store?tab=gallery", "store_enabled", "Store History", &["art", "textures"]),
    tab("Store Availability", "/store?tab=availability", "store_enabled", "Store History", &["timeline", "history"]),

    tab("My Mods", "/dashboard#mods", "mods_hub_enabled", "Dashboard", &["my projects", "uploads"]),
    tab("Stray Mods", "/mods/stray", "mods_hub_enabled", "Mods Hub", &["unclaimed", "imported"]),
];

pub fn all_destinations() -> impl Iterator<Item = &'static Destination> {
    PAGES.iter().chain(TABS.iter())
}

/// `normalize`: NFKD, combining marks dropped, casefolded, stripped.
pub fn normalize(text: &str) -> String {
    let bare: String = text.nfkd().filter(|&c| canonical_combining_class(c) == 0).collect();
    kiwi_core::pystr::strip(&casefold(&bare)).to_owned()
}

/// `_WORD_RE.findall(normalize(text))`.
fn tokens(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for c in normalize(text).chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            cur.push(c);
        } else if !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// `score(destination, query)`: 0 when it does not match.
pub fn score(d: &Destination, query: &str) -> i32 {
    let q = normalize(query);
    if q.is_empty() {
        return 0;
    }
    let name = normalize(d.name);
    if name == q {
        return 100;
    }
    if name.starts_with(&q) {
        return 80;
    }
    if tokens(d.name).iter().any(|t| t.starts_with(&q)) {
        return 60;
    }
    if name.contains(&q) {
        return 40;
    }
    for keyword in d.keywords {
        let kw = normalize(keyword);
        if kw.starts_with(&q) || kw.contains(&q) {
            return 25;
        }
    }
    let words = tokens(query);
    if words.len() > 1 {
        let mut hay = vec![name];
        hay.extend(d.keywords.iter().map(|k| normalize(k)));
        let hay = hay.join(" ");
        if words.iter().all(|w| hay.contains(w.as_str())) {
            return 20;
        }
    }
    0
}

/// `search_destinations(query, enabled, limit=limit)`: an unknown flag is off.
pub fn search_destinations(
    query: &str,
    enabled: &dyn Fn(&str) -> Option<bool>,
    limit: usize,
) -> Vec<&'static Destination> {
    let mut scored: Vec<(i32, i32, &'static Destination)> = Vec::new();
    for d in all_destinations() {
        if let Some(flag) = d.flag
            && !enabled(flag).unwrap_or(false)
        {
            continue;
        }
        let value = score(d, query);
        if value != 0 {
            scored.push((value, if d.is_tab() { -1 } else { 0 }, d));
        }
    }
    scored.sort_by(|a, b| (-a.0, -a.1, a.2.name).cmp(&(-b.0, -b.1, b.2.name)));
    scored.into_iter().take(limit).map(|r| r.2).collect()
}

pub const NAV_GROUPS: [&str; 5] = ["Live", "Economy", "Plan", "Create", "Learn"];

/// `nav_menu(enabled)`: enabled nav pages per column, empty columns dropped.
pub fn nav_menu(enabled: &dyn Fn(&str) -> Option<bool>) -> Vec<(&'static str, Vec<&'static Destination>)> {
    NAV_GROUPS
        .iter()
        .filter_map(|&group| {
            let items: Vec<&'static Destination> = PAGES
                .iter()
                .filter(|d| d.group == group && d.in_nav && d.flag.is_none_or(|f| enabled(f).unwrap_or(false)))
                .collect();
            (!items.is_empty()).then_some((group, items))
        })
        .collect()
}

pub fn nav_paths(enabled: &dyn Fn(&str) -> Option<bool>) -> Vec<&'static str> {
    nav_menu(enabled).into_iter().flat_map(|(_, items)| items.into_iter().map(|d| d.path)).collect()
}

/// `nav_is_active(path, enabled)`: prefix matching, `/` excluded.
pub fn nav_is_active(path: &str, enabled: &dyn Fn(&str) -> Option<bool>) -> bool {
    let p = if path.is_empty() { "/" } else { path };
    let trimmed = p.trim_end_matches('/');
    let current = if trimmed.is_empty() { "/" } else { trimmed };
    nav_paths(enabled).into_iter().any(|known| {
        let base = known.split('?').next().unwrap_or("").trim_end_matches('/');
        let base = if base.is_empty() { "/" } else { base };
        base != "/" && (current == base || current.starts_with(&format!("{base}/")))
    })
}

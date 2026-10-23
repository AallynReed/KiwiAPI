//! The navbar's Pages menu (`app/site/search_index.py` PAGES, `nav_menu`,
//! `nav_is_active`) and the hub pages (`app/site/hubs.py`).

use minijinja::Value;

use super::flags::Flags;

struct Page {
    name: &'static str,
    path: &'static str,
    flag: Option<&'static str>,
    icon: &'static str,
    group: &'static str,
    badge: &'static str,
    in_nav: bool,
}

const fn p(
    name: &'static str,
    path: &'static str,
    flag: Option<&'static str>,
    icon: &'static str,
    group: &'static str,
) -> Page {
    Page { name, path, flag, icon, group, badge: "", in_nav: true }
}

const fn hidden(mut page: Page) -> Page {
    page.in_nav = false;
    page
}

const fn beta(mut page: Page) -> Page {
    page.badge = "Beta";
    page
}

const PAGES: [Page; 44] = [
    p("Leaderboards", "/leaderboards", Some("leaderboards_enabled"), "fa-solid fa-ranking-star", "Live"),
    p("Clubs", "/clubs", Some("clubs_enabled"), "fa-solid fa-shield-halved", "Live"),
    p("Player Activity", "/activity", Some("player_activity_enabled"), "fa-solid fa-chart-line", "Live"),
    p("Class Activity", "/class-activity", Some("class_activity_enabled"), "fa-solid fa-users", "Live"),
    p("Server Status", "/status", Some("server_status_enabled"), "fa-solid fa-signal", "Live"),
    p("Server Time", "/server-time", Some("server_time_enabled"), "fa-solid fa-clock", "Live"),
    p("Calendar", "/calendar", Some("calendar_enabled"), "fa-solid fa-calendar-days", "Live"),
    p("Market", "/market", Some("market_enabled"), "fa-solid fa-shop", "Economy"),
    p("Tome Values", "/tomes", Some("tomes_enabled"), "fa-solid fa-book", "Economy"),
    p("Store History", "/store", Some("store_enabled"), "fa-solid fa-clock-rotate-left", "Economy"),
    p("Calculators", "/calculators", Some("calculators_enabled"), "fa-solid fa-calculator", "Economy"),
    p("Star Chart", "/star-chart", Some("star_chart_enabled"), "fa-solid fa-star", "Plan"),
    p("Gem Tools", "/gem-tools", Some("gem_tools_enabled"), "fa-solid fa-gem", "Plan"),
    beta(p("Dressing Room", "/dressing-room", Some("dressing_room_page_enabled"), "fa-solid fa-shirt", "Plan")),
    hidden(p("Gem Simulator", "/gem-simulator", Some("gem_simulator_enabled"), "fa-solid fa-gem", "Plan")),
    hidden(p(
        "Gem Evaluator",
        "/gem-evaluator",
        Some("gem_evaluator_enabled"),
        "fa-solid fa-magnifying-glass-chart",
        "Plan",
    )),
    hidden(p("Gem Builds", "/gem-builds", Some("gem_builds_enabled"), "fa-solid fa-wand-magic-sparkles", "Plan")),
    p("Abilities", "/abilities", Some("abilities_enabled"), "fa-solid fa-bolt", "Plan"),
    p("Allies", "/allies", Some("allies_enabled"), "fa-solid fa-paw", "Plan"),
    p("Mods Hub", "/mods", Some("mods_hub_enabled"), "fa-solid fa-cubes", "Create"),
    p("Modpacks", "/modpacks", Some("mods_hub_enabled"), "fa-solid fa-box-open", "Create"),
    p("Mod Workshop", "/mod-workshop", Some("mod_workshop_enabled"), "fa-solid fa-screwdriver-wrench", "Create"),
    p("Blueprint Editor", "/blueprint-editor", Some("blueprint_editor_enabled"), "fa-solid fa-cube", "Create"),
    p("Sound Studio", "/sound-studio", Some("sound_studio_enabled"), "fa-solid fa-sliders", "Create"),
    p("Unlock Debug", "/unlock-debug", Some("unlock_debug_enabled"), "fa-solid fa-terminal", "Create"),
    p("Classes", "/classes", Some("classes_enabled"), "fa-solid fa-hat-wizard", "Learn"),
    p("Trove Commands", "/commands", Some("commands_enabled"), "fa-solid fa-keyboard", "Learn"),
    beta(p("Codexes", "/codexes", Some("codexes_enabled"), "fa-solid fa-book-atlas", "Learn")),
    p("Guides", "/guides", Some("guides_enabled"), "fa-solid fa-compass", "Learn"),
    hidden(p("Gems Guide", "/gems-guide", Some("gems_guide_enabled"), "fa-solid fa-circle-question", "Learn")),
    hidden(p("Fishing Guide", "/fishing-guide", Some("fishing_guide_enabled"), "fa-solid fa-fish", "Learn")),
    hidden(p("Gear Guide", "/gear-guide", Some("gear_guide_enabled"), "fa-solid fa-shield-halved", "Learn")),
    hidden(p(
        "Loot Collector Rules",
        "/loot-collector-guide",
        Some("loot_collector_guide_enabled"),
        "fa-solid fa-filter",
        "Learn",
    )),
    hidden(p("Recipe Cost Calculator", "/codexes/crafting", Some("codexes_enabled"), "fa-solid fa-flask", "Learn")),
    p("Updates", "/updates", Some("updates_enabled"), "fa-solid fa-code-branch", "Learn"),
    p("Streams", "/streams", Some("streams_enabled"), "fa-solid fa-video", "Learn"),
    p("Community Sites", "/community-sites", Some("community_sites_enabled"), "fa-solid fa-handshake", "Learn"),
    p("Giveaways", "/giveaways", Some("giveaways_enabled"), "fa-solid fa-gift", "Learn"),
    p("App Releases", "/releases", Some("btt_releases_enabled"), "fa-solid fa-cloud-arrow-down", "Learn"),
    hidden(p("Get the App", "/app", None, "fa-solid fa-download", "About")),
    hidden(p("Dashboard", "/dashboard", None, "fa-solid fa-gauge", "About")),
    hidden(p("Changelog", "/changelog", None, "fa-solid fa-list-check", "About")),
    hidden(p("Accessibility", "/accessibility", None, "fa-solid fa-universal-access", "About")),
    hidden(p("Support", "/support", None, "fa-solid fa-heart", "About")),
];

const NAV_GROUPS: [&str; 5] = ["Live", "Economy", "Plan", "Create", "Learn"];

/// `enabled.get(d.flag, False)` over the context processor's flag dict.
fn enabled(flags: &Flags, flag: Option<&str>) -> bool {
    flag.is_none_or(|f| flags.on(f))
}

fn sections(flags: &Flags) -> Vec<(&'static str, Vec<&'static Page>)> {
    NAV_GROUPS
        .iter()
        .filter_map(|g| {
            let items: Vec<&Page> =
                PAGES.iter().filter(|d| d.group == *g && d.in_nav && enabled(flags, d.flag)).collect();
            (!items.is_empty()).then_some((*g, items))
        })
        .collect()
}

/// `nav_menu(flags)` as template data.
pub fn nav_menu(flags: &Flags) -> Value {
    let out: Vec<Value> = sections(flags)
        .into_iter()
        .map(|(group, items)| {
            let items: Vec<Value> = items
                .iter()
                .map(|d| {
                    let flag = d.flag.map_or(Value::from(()), Value::from);
                    Value::from_iter([
                        ("name", Value::from(d.name)),
                        ("path", Value::from(d.path)),
                        ("flag", flag),
                        ("icon", Value::from(d.icon)),
                        ("group", Value::from(d.group)),
                        ("badge", Value::from(d.badge)),
                        ("in_nav", Value::from(d.in_nav)),
                    ])
                })
                .collect();
            Value::from_iter([("group", Value::from(group)), ("items", Value::from(items))])
        })
        .collect();
    Value::from(out)
}

/// `nav_is_active(path, flags)`.
pub fn nav_is_active(path: &str, flags: &Flags) -> bool {
    let p = if path.is_empty() { "/" } else { path };
    let trimmed = p.trim_end_matches('/');
    let current = if trimmed.is_empty() { "/" } else { trimmed };
    sections(flags).iter().flat_map(|(_, items)| items.iter()).any(|d| {
        let base = d.path.split('?').next().unwrap_or("").trim_end_matches('/');
        !base.is_empty() && (current == base || current.starts_with(&format!("{base}/")))
    })
}

pub struct Hub {
    pub path: &'static str,
    title: &'static str,
    sub: &'static str,
    empty: &'static str,
    entries: &'static [Entry],
}

struct Entry {
    name: &'static str,
    path: &'static str,
    flag: &'static str,
    icon: &'static str,
    blurb: &'static str,
    kind: &'static str,
}

pub const GUIDES: Hub = Hub {
    path: "/guides",
    title: "Trove <span class=\"highlight-blue\">Guides</span>",
    sub: "Everything we have written up, in one place. More as we go.",
    empty: "No guides are available right now.",
    entries: &[
        Entry {
            name: "Gems Guide",
            path: "/gems-guide",
            flag: "gems_guide_enabled",
            icon: "fa-solid fa-gem",
            blurb: "Tiers, elements, lesser versus empowered, stat rolls, levelling and focusing - the whole gem system, one step at a time.",
            kind: "Interactive",
        },
        Entry {
            name: "Gear Guide",
            path: "/gear-guide",
            flag: "gear_guide_enabled",
            icon: "fa-solid fa-shield-halved",
            blurb: "Every gear rarity and its Power Rank, what the forge, stars and Pearls of Wisdom do, and every banner and torch with its stats.",
            kind: "Reference",
        },
        Entry {
            name: "Fishing Guide",
            path: "/fishing-guide",
            flag: "fishing_guide_enabled",
            icon: "fa-solid fa-fish",
            blurb: "Every lure, pool and quest, plus all 155 fish with the conditions each one needs and what it is worth.",
            kind: "Reference",
        },
        Entry {
            name: "Loot Collector Rules",
            path: "/loot-collector-guide",
            flag: "loot_collector_guide_enabled",
            icon: "fa-solid fa-filter",
            blurb: "Write rules that decide what the Loot Collector mod takes, keeps, unlocks or hides - the whole grammar, worked through with examples.",
            kind: "Mod guide",
        },
    ],
};

pub const GEM_TOOLS: Hub = Hub {
    path: "/gem-tools",
    title: "Trove <span class=\"highlight-blue\">Gem Tools</span>",
    sub: "Roll one, rate the one you have, or work out the build around it.",
    empty: "No gem tools are available right now.",
    entries: &[
        Entry {
            name: "Gem Simulator",
            path: "/gem-simulator",
            flag: "gem_simulator_enabled",
            icon: "fa-solid fa-gem",
            blurb: "Roll, level and augment gems the way the game does, and equip a loadout to see what it comes to.",
            kind: "Simulator",
        },
        Entry {
            name: "Gem Evaluator",
            path: "/gem-evaluator",
            flag: "gem_evaluator_enabled",
            icon: "fa-solid fa-magnifying-glass-chart",
            blurb: "Score a gem you already own against what it could have rolled, so you know whether to keep pouring into it.",
            kind: "Rating",
        },
        Entry {
            name: "Gem Builds",
            path: "/gem-builds",
            flag: "gem_builds_enabled",
            icon: "fa-solid fa-wand-magic-sparkles",
            blurb: "The optimizer: class, subclass, ally, food and star chart in, the best gem split out.",
            kind: "Optimizer",
        },
    ],
};

/// `_hub(...)`'s view: `hub_view` plus path and meta.
pub fn hub_view(hub: &Hub, flags: &Flags, meta_title: &str, meta_description: &str) -> Value {
    let entries: Vec<Value> = hub
        .entries
        .iter()
        .filter(|e| flags.on(e.flag))
        .map(|e| {
            Value::from_iter([
                ("name", e.name),
                ("path", e.path),
                ("flag", e.flag),
                ("icon", e.icon),
                ("blurb", e.blurb),
                ("kind", e.kind),
            ])
        })
        .collect();
    let count = entries.len();
    Value::from_iter([
        ("title", Value::from(hub.title)),
        ("sub", Value::from(hub.sub)),
        ("empty", Value::from(hub.empty)),
        ("entries", Value::from(entries)),
        ("count", Value::from(count)),
        ("path", Value::from(hub.path)),
        ("meta_title", Value::from(meta_title)),
        ("meta_description", Value::from(meta_description)),
    ])
}

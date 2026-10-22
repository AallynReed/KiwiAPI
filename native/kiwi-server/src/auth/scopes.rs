//! Scope bitmask registry from `app/core/scopes.py`. Bits are permanent; mask
//! 0 means every scope, present and future.

use serde::Serialize;

#[derive(Debug, Clone, Copy, Serialize)]
pub struct Scope {
    pub key: &'static str,
    pub bit: i64,
    pub resource: &'static str,
    pub description: &'static str,
}

pub const ROTATIONS_READ: i64 = 1 << 0;
pub const FEEDS_READ: i64 = 1 << 1;
pub const STATS_READ: i64 = 1 << 2;
pub const GEMS_READ: i64 = 1 << 3;
pub const MISC_READ: i64 = 1 << 4;
pub const MODS_READ: i64 = 1 << 5;
pub const UPDATES_READ: i64 = 1 << 6;
pub const CODEXES_READ: i64 = 1 << 7;
pub const BTT_READ: i64 = 1 << 8;
pub const LEADERBOARDS_READ: i64 = 1 << 9;
pub const MARKET_READ: i64 = 1 << 10;
pub const ACTIVITY_READ: i64 = 1 << 11;
pub const GIVEAWAYS_READ: i64 = 1 << 12;
pub const EVENTS_READ: i64 = 1 << 13;
pub const OCR_READ: i64 = 1 << 14;
pub const STORE_READ: i64 = 1 << 15;
pub const MODS_WRITE: i64 = 1 << 16;

pub const ALL_SCOPES: i64 = 0;

const fn s(key: &'static str, bit: i64, resource: &'static str, description: &'static str) -> Scope {
    Scope { key, bit, resource, description }
}

pub const REGISTRY: &[Scope] = &[
    s(
        "rotations:read",
        ROTATIONS_READ,
        "rotations",
        "Read rotation/timer data - server time, bonuses, merchants, biomes",
    ),
    s("feeds:read", FEEDS_READ, "feeds", "Read relayed feeds - Trove news, Twitch/YouTube/Bilibili"),
    s(
        "stats:read",
        STATS_READ,
        "stats",
        "Read raw game-stat data - stat tables (power rank / magic find / light) and classes",
    ),
    s(
        "gems:read",
        GEMS_READ,
        "gems",
        "Use the gem tools - simulator (generate/augment), evaluator, and build optimizer",
    ),
    s("misc:read", MISC_READ, "misc", "Misc tools - third-party modding software list and the time converter"),
    s("mods:read", MODS_READ, "mods", "Mod tools - decompile a .tmod file and build a .tmod from files"),
    s(
        "updates:read",
        UPDATES_READ,
        "updates",
        "Browse archived game files - versions, directory structure, and single-file download",
    ),
    s(
        "codexes:read",
        CODEXES_READ,
        "codexes",
        "Read parsed game codexes - collectibles, styles, recipes and items, the relationships between them, badge requirements and progression trees",
    ),
    s("btt:read", BTT_READ, "btt", "Read BetterTroveTools releases - latest version per platform and channel"),
    s(
        "leaderboards:read",
        LEADERBOARDS_READ,
        "leaderboards",
        "Read Trove in-game leaderboards - boards, entries, timestamps",
    ),
    s(
        "market:read",
        MARKET_READ,
        "market",
        "Read Trove marketplace listings - listings, items, interest list, price history",
    ),
    s(
        "activity:read",
        ACTIVITY_READ,
        "activity",
        "Read Trove player-activity estimates - live active-player count and multi-period trend series",
    ),
    s("giveaways:read", GIVEAWAYS_READ, "giveaways", "Read public giveaways - ongoing draws, prizes, entry counts"),
    s(
        "events:read",
        EVENTS_READ,
        "events",
        "Subscribe to the live event stream (SSE) - real-time challenge + chaos-chest push updates",
    ),
    s(
        "ocr:read",
        OCR_READ,
        "ocr",
        "Read character stats from a screenshot - self-hosted OCR of the in-game stat sheet",
    ),
    s(
        "store:read",
        STORE_READ,
        "store",
        "Read the in-game Kiwi Store catalog - categories, products, prices, deals, loot odds",
    ),
    s(
        "mods:write",
        MODS_WRITE,
        "mods",
        "Manage Mods Hub projects for creators who connected their account to yours - create mods, cut and publish releases, edit metadata, images and visibility",
    ),
];

const KNOWN_MASK: i64 = {
    let mut m = 0;
    let mut i = 0;
    while i < REGISTRY.len() {
        m |= REGISTRY[i].bit;
        i += 1;
    }
    m
};

/// `catalog()`: `{key, bit, resource, description}` in registry order.
pub fn catalog() -> &'static [Scope] {
    REGISTRY
}

pub fn bit_for(key: &str) -> Option<i64> {
    REGISTRY.iter().find(|s| s.key == key).map(|s| s.bit)
}

pub fn key_for(bit: i64) -> Option<&'static str> {
    REGISTRY.iter().find(|s| s.bit == bit).map(|s| s.key)
}

pub fn is_valid_mask(mask: i64) -> bool {
    mask == ALL_SCOPES || (mask >= 0 && mask & !KNOWN_MASK == 0)
}

/// `mask_grants(mask, scope)` by key.
pub fn mask_grants(mask: i64, scope: &str) -> bool {
    mask == ALL_SCOPES || bit_for(scope).is_some_and(|bit| mask & bit != 0)
}

/// `mask_grants` by registered bit (an unregistered bit grants only via mask 0).
pub fn mask_grants_bit(mask: i64, bit: i64) -> bool {
    mask == ALL_SCOPES || (key_for(bit).is_some() && mask & bit != 0)
}

/// Scope keys set in the mask (empty for 0 / all).
pub fn decode(mask: i64) -> Vec<&'static str> {
    if mask == ALL_SCOPES {
        return Vec::new();
    }
    REGISTRY.iter().filter(|s| mask & s.bit != 0).map(|s| s.key).collect()
}

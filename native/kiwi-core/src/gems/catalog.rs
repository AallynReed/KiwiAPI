//! The gem enums of `app/trove/gems/constants.py` (ids are the wire values)
//! and the static tables of `bases.py`.

use super::GemError;

pub const TIERS: [&str; 4] = ["Radiant", "Stellar", "Crystal", "Mystic"];
pub const TYPES: [&str; 2] = ["Lesser", "Empowered"];
pub const ELEMENTS: [&str; 4] = ["Water", "Fire", "Air", "Cosmic"];
pub const RESTRICTIONS: [&str; 2] = ["Fierce", "Arcane"];
pub const AUGMENTS: [&str; 3] = ["Rough Focus", "Precise Focus", "Superior Focus"];
pub const STATS: [&str; 10] = [
    "Physical Damage",
    "Magic Damage",
    "Critical Damage",
    "Critical Hit",
    "Max Health",
    "Max Health %",
    "Light",
    "Movement Speed",
    "Jump",
    "Health Regen",
];
pub const ABILITIES: [&str; 12] = [
    "Stinging Curse",
    "Volatile Velocity",
    "Spirit Surge",
    "Mired Mojo",
    "Stunburst",
    "Pyrodisc",
    "Explosive Epilogue",
    "Cubic Curtain",
    "Berserk Battler",
    "Empyrean Barrier",
    "Flower Power",
    "Vampirian Vanquisher",
];

pub const RADIANT: u8 = 1;
pub const MYSTIC: u8 = 4;
pub const LESSER: u8 = 1;
pub const EMPOWERED: u8 = 2;
pub const WATER: u8 = 1;
pub const COSMIC: u8 = 4;
pub const FIERCE: u8 = 1;

pub const PHYSICAL_DAMAGE: u8 = 1;
pub const MAGIC_DAMAGE: u8 = 2;
pub const CRITICAL_DAMAGE: u8 = 3;
pub const CRITICAL_HIT: u8 = 4;
pub const MAX_HEALTH: u8 = 5;
pub const MAX_HEALTH_BONUS: u8 = 6;
pub const LIGHT: u8 = 7;
pub const HEALTH_REGEN: u8 = 10;
pub const RETIRED: [u8; 1] = [HEALTH_REGEN];

/// `KARMA_MAX`: the karma bar's size (a constant in the game client).
pub const KARMA_MAX: i64 = 125;

/// The display name of a 1-based enum value.
pub fn name(names: &'static [&'static str], id: u8) -> &'static str {
    names[id as usize - 1]
}

/// `Enum(value)`: the member, or Python's `ValueError`.
pub fn member(names: &[&str], enum_name: &str, value: i64) -> Result<u8, GemError> {
    if (1..=names.len() as i64).contains(&value) {
        Ok(value as u8)
    } else {
        Err(GemError::Value(format!("{value} is not a valid {enum_name}")))
    }
}

pub fn tier(v: i64) -> Result<u8, GemError> {
    member(&TIERS, "GemTier", v)
}
pub fn gem_type(v: i64) -> Result<u8, GemError> {
    member(&TYPES, "GemType", v)
}
pub fn element(v: i64) -> Result<u8, GemError> {
    member(&ELEMENTS, "GemElement", v)
}
pub fn restriction(v: i64) -> Result<u8, GemError> {
    member(&RESTRICTIONS, "GemRestriction", v)
}
pub fn stat_type(v: i64) -> Result<u8, GemError> {
    member(&STATS, "GemStatType", v)
}
pub fn augment(v: i64) -> Result<u8, GemError> {
    member(&AUGMENTS, "AugmentType", v)
}

/// `_STAT`, in its declaration order: stat -> (game stat key, percent).
pub const STAT_KEYS: [(u8, &str, bool); 8] = [
    (PHYSICAL_DAMAGE, "PhysicalDamage", false),
    (MAGIC_DAMAGE, "SpellDamage", false),
    (CRITICAL_DAMAGE, "CriticalHitDamage", false),
    (CRITICAL_HIT, "CriticalHitChance", false),
    (MAX_HEALTH, "MaxHealth", false),
    (MAX_HEALTH_BONUS, "MaxHealth", true),
    (LIGHT, "Light", false),
    (HEALTH_REGEN, "HealthRegen_controller", false),
];

pub fn stat_key(stat: u8) -> Option<(&'static str, bool)> {
    STAT_KEYS.iter().find(|k| k.0 == stat).map(|k| (k.1, k.2))
}

const PR_BASE: [i64; 4] = [3, 5, 7, 9];
const LESSER_PR: [[i64; 2]; 4] = [[85, 113], [150, 200], [175, 250], [200, 260]];
const EMPOWERED_PR: [[i64; 2]; 4] = [[113, 150], [200, 266], [220, 280], [240, 300]];
const FILE_TIER: [i64; 4] = [9, 10, 11, 12];
const COLORS: [&str; 4] = ["blue", "red", "yellow", "opal"];
const FOCUS_ITEMS: [&str; 3] = ["item/gem/booster/augment1", "item/gem/booster/augment2", "item/gem/booster/augment3"];

/// `_TIER_PR_BASE`: Power Rank per stat step.
pub fn pr_base(tier: u8) -> i64 {
    PR_BASE[tier as usize - 1]
}

/// The Power Rank one container's roll spans, by gem type.
pub fn pr_threshold(tier: u8, gem_type: u8) -> [i64; 2] {
    if gem_type == LESSER { LESSER_PR[tier as usize - 1] } else { EMPOWERED_PR[tier as usize - 1] }
}

pub fn file_tier(tier: u8) -> i64 {
    FILE_TIER[tier as usize - 1]
}

pub fn color(element: u8) -> &'static str {
    COLORS[element as usize - 1]
}

pub fn focus_item(augment: u8) -> &'static str {
    FOCUS_ITEMS[augment as usize - 1]
}

const ALL_SIX: [u8; 6] = [PHYSICAL_DAMAGE, MAGIC_DAMAGE, CRITICAL_DAMAGE, CRITICAL_HIT, MAX_HEALTH, MAX_HEALTH_BONUS];
const COSMIC_SEVEN: [u8; 7] =
    [PHYSICAL_DAMAGE, MAGIC_DAMAGE, CRITICAL_DAMAGE, CRITICAL_HIT, MAX_HEALTH, MAX_HEALTH_BONUS, LIGHT];
pub const PHYSICAL_POOL: [u8; 5] = [PHYSICAL_DAMAGE, CRITICAL_DAMAGE, CRITICAL_HIT, MAX_HEALTH, MAX_HEALTH_BONUS];
pub const MAGIC_POOL: [u8; 5] = [MAGIC_DAMAGE, CRITICAL_DAMAGE, CRITICAL_HIT, MAX_HEALTH, MAX_HEALTH_BONUS];

/// `GEM_STAT_RESTRICTIONS[element]`.
pub fn element_stats(element: u8) -> &'static [u8] {
    if element == COSMIC { &COSMIC_SEVEN } else { &ALL_SIX }
}

/// `GEM_ABILITIES[element]`.
pub fn element_abilities(element: u8) -> &'static [u8] {
    if element == COSMIC { &[9, 10, 11, 12] } else { &[1, 2, 3, 4, 5, 6, 7, 8] }
}

//! `app/trove/codexes/localize.py`: `$…` keys to display text.

use std::collections::HashMap;

use super::{py, wire::clean_localized_text};

pub type Loc = HashMap<String, String>;

pub const STAT_NAMES: &[(&str, &str)] = &[
    ("$Stat_PhysicalDamage", "Physical Damage"),
    ("$Stat_SpellDamage", "Magic Damage"),
    ("$Stat_MaxHealth", "Maximum Health"),
    ("$Stat_MaxEnergy", "Maximum Energy"),
    ("$Stat_HealthRegen_controller", "Health Regen"),
    ("$Stat_EnergyRegen_controller", "Energy Regen"),
    ("$Stat_Stability", "Stability"),
    ("$Stat_CriticalHitChance", "Critical Hit"),
    ("$Stat_MovementSpeed", "Movement Speed"),
    ("$Stat_Jump", "Jump"),
    ("$Stat_Superstition", "Superstition"),
    ("$Stat_IncomingDamageMod", "Incoming Damage"),
    ("$Stat_OutgoingDamageMod", "Damage"),
    ("$Stat_MagicFind", "Magic Find"),
    ("$Stat_Mining", "Lasermancy"),
    ("$Stat_AttackSpeed", "Attack Speed"),
    ("$Stat_MaxFlasks", "Flask Capacity"),
    ("$Stat_CraftingSpeed", "Crafting Speed"),
    ("$Stat_CooldownSpeed", "Cooldown Speed"),
    ("$Stat_Acceleration", "Acceleration"),
    ("$Stat_TurningRate", "Turning Rate"),
    ("$Stat_ExperienceBoost", "Experience Boost"),
    ("$Stat_CriticalHitDamage", "Critical Damage"),
    ("$Stat_BattleFactor", "Battle Factor"),
    ("$Stat_ActionTimeMod", "Action Time Mod"),
    ("$Stat_PowerRank", "Power Rank"),
    ("$Stat_Glide", "Glide"),
    ("$Stat_ShootProjectileSpeedMultiplier", "Projectile Speed"),
    ("$Stat_RedGemStatBoost", "Red Gem Stat Boost"),
    ("$Stat_BlueGemStatBoost", "Blue Gem Stat Boost"),
    ("$Stat_YellowGemStatBoost", "Yellow Gem Stat Boost"),
    ("$Stat_DoubleHitChance", "Double Hit"),
    ("$Stat_JackpotExperience", "Jackpot Experience"),
    ("$Stat_GemEfficiency", "Gem Efficiency"),
    ("$Stat_AdventurineGainBoost", "Adventurine"),
    ("$Stat_ClubExperienceBoost", "Club Experience Boost"),
    ("$Stat_MaxExploration", "Maximum Exploration"),
    ("$Stat_JumpSpeedMultiplier", "Jump Speed"),
    ("$Stat_GreenGemStatBoost", "Green Gem Stat Boost"),
    ("$Stat_Light", "Light"),
    ("$Stat_Dark", "Dark"),
    ("$Stat_OpalGemStatBoost", "Opal Gem Stat Boost"),
    ("$Stat_MaxNCharge", "Maximum N-Charge"),
    ("$Stat_NChargeRegen", "N-Charge Regen"),
    ("$Stat_MaxArmor", "Maximum Armor"),
];

const SLOT_NAMES: &[(&str, &str)] =
    &[("$EquipmentSlot_Mount", "Mount"), ("$EquipmentSlot_Wings", "Wings"), ("$EquipmentSlot_Boat", "Boat")];

fn table(t: &[(&'static str, &'static str)], key: &str) -> Option<&'static str> {
    t.iter().find(|(k, _)| *k == key).map(|(_, v)| *v)
}

fn from_loc(loc: &Loc, key: Option<&str>) -> String {
    match key.filter(|k| !k.is_empty()).and_then(|k| loc.get(k)) {
        Some(text) if !text.is_empty() => clean_localized_text(text),
        _ => String::new(),
    }
}

fn humanize(key: &str) -> String {
    let body = py::lstrip_chars(key, "$");
    let body = match body.split_once('_') {
        Some((_, rest)) => rest,
        None => body,
    };
    let body: Vec<char> = body.replace("_controller", "").chars().collect();
    let mut out = String::new();
    for (i, &ch) in body.iter().enumerate() {
        if ch.is_uppercase() && i > 0 && !body[i - 1].is_uppercase() {
            out.push(' ');
        }
        out.push(ch);
    }
    py::title(py::strip(&out.replace('_', " ")))
}

fn bonus_base(key: &str) -> Option<String> {
    let (base, sep, tail) = match key.find("_controller") {
        Some(i) => (&key[..i], "_controller", &key[i + "_controller".len()..]),
        None => (key, "", ""),
    };
    let base = base.strip_suffix("Bonus")?;
    let full = format!("{base}{sep}{tail}");
    table(STAT_NAMES, &full).map(|_| full)
}

/// `resolve_stat_name`.
pub fn resolve_stat_name(loc: &Loc, key: &str) -> String {
    if let Some(base) = bonus_base(key) {
        return format!("{} Bonus", resolve_stat_name(loc, &base));
    }
    let text = from_loc(loc, Some(key));
    if !text.is_empty() {
        return text;
    }
    match table(STAT_NAMES, key) {
        Some(v) => v.to_owned(),
        None => humanize(key),
    }
}

/// `resolve_slot_name`.
pub fn resolve_slot_name(loc: &Loc, key: Option<&str>) -> String {
    let Some(key) = key.filter(|k| !k.is_empty()) else { return String::new() };
    let text = from_loc(loc, Some(key));
    if !text.is_empty() {
        return text;
    }
    match table(SLOT_NAMES, key) {
        Some(v) => v.to_owned(),
        None => humanize(key),
    }
}

/// `resolve_text`.
pub fn resolve_text(loc: &Loc, key: Option<&str>) -> String {
    from_loc(loc, key)
}

//! The helpers the decoders share: `decode/fields.py`, `decode/common.py` and
//! the bits of `app/trove/codexes` (stat keys, stat names, locale tables,
//! uleb reads) they draw on.

use std::sync::{Arc, OnceLock};

use indexmap::IndexMap;
use regex::Regex;

use super::{
    json::J,
    value::{Prefab, W, parse},
};
use crate::{obj, pyfloat};

pub type Locale = IndexMap<String, String>;

// fields.py
pub const IDENTITY: i128 = 49;
pub const ID_NAME: i128 = 1;
pub const ID_CATEGORY: i128 = 2;
pub const ID_DESCRIPTION: i128 = 5;
pub const ID_ICON: i128 = 6;
pub const ACTION_SECTION: usize = 1;
pub const ACTIONS: [(i128, Option<i128>, Option<i128>); 20] = [
    (54, Some(5), Some(23)),
    (57, Some(11), Some(16)),
    (60, Some(5), Some(24)),
    (62, Some(4), None),
    (69, Some(8), None),
    (93, Some(3), Some(19)),
    (100, Some(8), Some(16)),
    (135, None, Some(3)),
    (153, Some(11), None),
    (162, None, Some(1)),
    (178, Some(11), None),
    (194, Some(23), Some(17)),
    (204, Some(6), None),
    (232, None, Some(2)),
    (291, Some(6), Some(4)),
    (380, Some(11), Some(16)),
    (382, Some(3), Some(19)),
    (437, Some(8), None),
    (1008, Some(3), Some(19)),
    (1020, Some(11), Some(16)),
];
pub const EFFECTS: [i128; 100] = [
    45, 46, 64, 131, 132, 136, 141, 142, 150, 151, 155, 156, 157, 158, 165, 171, 189, 213, 218, 228, 231, 233, 234,
    235, 240, 242, 243, 250, 252, 260, 261, 262, 272, 274, 275, 278, 280, 281, 282, 283, 284, 287, 288, 303, 304, 309,
    315, 316, 319, 321, 324, 330, 331, 332, 336, 337, 339, 344, 349, 350, 354, 365, 373, 379, 383, 384, 389, 392, 396,
    397, 401, 406, 433, 447, 450, 452, 453, 458, 461, 466, 473, 475, 483, 485, 488, 492, 499, 1004, 1005, 1013, 1016,
    1017, 1021, 1031, 1032, 1034, 1037, 1038, 1050, 1051,
];
pub const EFFECT_DURATION: i128 = 4;
pub const EFFECT_DESCRIPTION: i128 = 2;
pub const EFFECT_NAME: i128 = 23;
pub const PROC_SPAWNER: i128 = 150;
pub const PROC_SECTION: usize = 2;
pub const PROC_COOLDOWN: i128 = 4;
pub const DAMAGE: [(i128, &str); 18] = [
    (32, "damage"),
    (12, "multiplier"),
    (5, "max_health_percent"),
    (28, "crit_chance_bonus"),
    (25, "crit_damage_multiplier"),
    (19, "energy_drain"),
    (31, "block_damage"),
    (2, "max_block_hardness"),
    (4, "max_blocks"),
    (9, "knockback_multiplier"),
    (10, "knockback"),
    (14, "min_knockback"),
    (23, "knockup"),
    (33, "stability_ignore"),
    (16, "target_effect"),
    (20, "attacker_effect"),
    (11, "damage_type"),
    (26, "damage_stat"),
];
pub const DAMAGE_SIGNATURE: [i128; 5] = [4, 9, 12, 14, 32];
pub const DAMAGE_INTS: [i128; 4] = [2, 4, 31, 32];
pub const DAMAGE_FLOATS: [i128; 6] = [5, 9, 10, 12, 14, 33];
pub const HEALING: [(i128, &str); 6] = [
    (7, "health"),
    (1, "max_health_percent"),
    (2, "energy"),
    (3, "max_energy_percent"),
    (5, "target_effect"),
    (6, "attacker_effect"),
];
pub const MOD_STAT: i128 = 0;
pub const MOD_OP: i128 = 1;
pub const MOD_VALUE: i128 = 2;
pub const MOD_LABEL: i128 = 3;
pub const MOD_FLAGS: i128 = 5;
pub const MOD_OPS: [&str; 7] = ["MultiplySum", "Add", "Set", "Nullify", "Multiply", "Minimum", "Maximum"];
pub const MOD_FLAG_NAMES: [(i128, &str); 8] = [
    (1, "PVE"),
    (2, "PVP"),
    (4, "Reproportion"),
    (8, "OwnedClass"),
    (16, "Discovery"),
    (32, "Delve"),
    (64, "PostRange"),
    (128, "ExcludePVP"),
];

pub fn is_effect(cid: i128) -> bool {
    EFFECTS.contains(&cid)
}

pub fn action_spec(cid: i128) -> Option<(Option<i128>, Option<i128>)> {
    ACTIONS.iter().find(|(c, _, _)| *c == cid).map(|(_, e, c)| (*e, *c))
}

// codexes/bonuses.py
pub const STAT_KEYS: [&str; 45] = [
    "$Stat_PhysicalDamage",
    "$Stat_SpellDamage",
    "$Stat_MaxHealth",
    "$Stat_MaxEnergy",
    "$Stat_HealthRegen_controller",
    "$Stat_EnergyRegen_controller",
    "$Stat_Stability",
    "$Stat_CriticalHitChance",
    "$Stat_MovementSpeed",
    "$Stat_Jump",
    "$Stat_Superstition",
    "$Stat_IncomingDamageMod",
    "$Stat_OutgoingDamageMod",
    "$Stat_MagicFind",
    "$Stat_Mining",
    "$Stat_AttackSpeed",
    "$Stat_MaxFlasks",
    "$Stat_CraftingSpeed",
    "$Stat_CooldownSpeed",
    "$Stat_Acceleration",
    "$Stat_TurningRate",
    "$Stat_ExperienceBoost",
    "$Stat_CriticalHitDamage",
    "$Stat_BattleFactor",
    "$Stat_ActionTimeMod",
    "$Stat_PowerRank",
    "$Stat_Glide",
    "$Stat_ShootProjectileSpeedMultiplier",
    "$Stat_RedGemStatBoost",
    "$Stat_BlueGemStatBoost",
    "$Stat_YellowGemStatBoost",
    "$Stat_DoubleHitChance",
    "$Stat_JackpotExperience",
    "$Stat_GemEfficiency",
    "$Stat_AdventurineGainBoost",
    "$Stat_ClubExperienceBoost",
    "$Stat_MaxExploration",
    "$Stat_JumpSpeedMultiplier",
    "$Stat_GreenGemStatBoost",
    "$Stat_Light",
    "$Stat_Dark",
    "$Stat_OpalGemStatBoost",
    "$Stat_MaxNCharge",
    "$Stat_NChargeRegen",
    "$Stat_MaxArmor",
];

/// `STAT_KEYS.get(id)`.
pub fn stat_keys(id: i128) -> Option<&'static str> {
    usize::try_from(id).ok().and_then(|i| STAT_KEYS.get(i)).copied()
}

/// `OPERATIONS.get(byte)`.
pub fn operation(byte: i128) -> Option<&'static str> {
    match byte {
        0 => Some("MultiplySum"),
        2 => Some("Add"),
        4 => Some("Set"),
        6 => Some("Nullify"),
        8 => Some("Multiply"),
        10 => Some("Minimum"),
        12 => Some("Maximum"),
        _ => None,
    }
}

/// `fields.stat_key`: KStatType ordinal -> `$Stat_*` key.
pub fn stat_key(ordinal: i128) -> String {
    if let Some(k) = stat_keys(ordinal) {
        return k.to_owned();
    }
    match ordinal {
        45 => "$Stat_HealDoneMultiplier".into(),
        46 => "$Stat_HealReceiveMultiplier".into(),
        _ => format!("$Stat_{ordinal}"),
    }
}

/// `bonuses.bonus_key`.
pub fn bonus_key(stat_key: &str) -> String {
    match stat_key.split_once("_controller") {
        Some((base, tail)) => format!("{base}Bonus_controller{tail}"),
        None => format!("{stat_key}Bonus"),
    }
}

/// `bonuses._normalize`: (display value, is_percent).
pub fn normalize(stat_key: &str, operation: i128, amount: f64) -> (f64, bool) {
    match operation {
        0 => (amount * 100.0, true),
        8 => ((amount - 1.0) * 100.0, true),
        2 => match stat_key {
            "$Stat_CriticalHitChance" => (amount / 10.0, false),
            "$Stat_CriticalHitDamage" | "$Stat_AttackSpeed" => (amount, true),
            _ => (amount, false),
        },
        _ => (amount, false),
    }
}

/// `bonuses._is_hidden`.
pub fn is_hidden(r: &str) -> bool {
    ["flask_auto_use", "_reset_death_save", "equip_flask_cooldown", "mount_block_harvester"]
        .iter()
        .any(|t| r.contains(t))
}

// codexes/localize.py
const STAT_NAMES: [(&str, &str); 45] = [
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

fn stat_name(key: &str) -> Option<&'static str> {
    STAT_NAMES.iter().find(|(k, _)| *k == key).map(|(_, v)| *v)
}

/// Python `str.title()`.
pub fn title(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_cased = false;
    for c in s.chars() {
        let cased = c.is_lowercase() || c.is_uppercase();
        if cased {
            if prev_cased {
                out.extend(c.to_lowercase());
            } else {
                out.extend(c.to_uppercase());
            }
        } else {
            out.push(c);
        }
        prev_cased = cased;
    }
    out
}

fn humanize(key: &str) -> String {
    let body = key.trim_start_matches('$');
    let body = match body.split_once('_') {
        Some((_, rest)) => rest,
        None => body,
    };
    let body = body.replace("_controller", "");
    let chars: Vec<char> = body.chars().collect();
    let mut out = String::new();
    for (i, ch) in chars.iter().enumerate() {
        if ch.is_uppercase() && i > 0 && !chars[i - 1].is_uppercase() {
            out.push(' ');
        }
        out.push(*ch);
    }
    title(out.replace('_', " ").trim())
}

fn bonus_base(key: &str) -> Option<String> {
    let (base, sep, tail) = match key.split_once("_controller") {
        Some((b, t)) => (b, "_controller", t),
        None => (key, "", ""),
    };
    let base = base.strip_suffix("Bonus")?;
    let full = format!("{base}{sep}{tail}");
    stat_name(&full).map(|_| full)
}

/// `localize.resolve_stat_name({}, key)`.
pub fn resolve_stat_name(key: &str) -> String {
    resolve_stat_name_in(&Locale::new(), key)
}

/// `localize.resolve_stat_name(loc_map, key)`.
pub fn resolve_stat_name_in(loc: &Locale, key: &str) -> String {
    if let Some(base) = bonus_base(key) {
        return format!("{} Bonus", resolve_stat_name_in(loc, &base));
    }
    let found = loc.get(key).filter(|t| !t.is_empty()).map(|t| clean_localized_text(t)).filter(|t| !t.is_empty());
    found.or_else(|| stat_name(key).map(str::to_owned)).unwrap_or_else(|| humanize(key))
}

// codexes/binfab.py
/// `read_uleb`: `None` where Python raises (runs off the end, or overlong).
pub fn read_uleb(data: &[u8], mut pos: usize) -> Option<(i128, usize)> {
    let (mut result, mut shift) = (0i128, 0u32);
    loop {
        let byte = *data.get(pos)?;
        pos += 1;
        result |= i128::from(byte & 0x7F) << shift;
        if byte & 0x80 == 0 {
            return Some((result, pos));
        }
        shift += 7;
        if shift > 63 {
            return None;
        }
    }
}

pub fn unzig(v: i128) -> i128 {
    (v >> 1) ^ -(v & 1)
}

fn read_varint(data: &[u8], offset: usize) -> Option<(i128, usize)> {
    let (mut value, mut shift, mut cursor) = (0i128, 0u32, offset);
    while cursor < data.len() && shift <= 63 {
        let byte = data[cursor];
        value |= i128::from(byte & 0x7F) << shift;
        cursor += 1;
        if byte & 0x80 == 0 {
            return Some((value, cursor));
        }
        shift += 7;
    }
    None
}

/// `clean_localized_text`.
pub fn clean_localized_text(text: &str) -> String {
    let mut t = text.replace("\\$", "$");
    if t.starts_with("$$") {
        t = format!("${}", t.trim_start_matches('$'));
    }
    let t = t.replace("\\n", "\n");
    let mut t = t.trim_matches('`').trim().to_owned();
    if !t.starts_with('$') && t.starts_with("\\\"") {
        t.remove(0);
    }
    t
}

/// Python's `bytes.decode("utf-8", errors="ignore")`.
pub fn decode_ignore(b: &[u8]) -> String {
    let mut out = String::with_capacity(b.len());
    for chunk in b.utf8_chunks() {
        out.push_str(chunk.valid());
    }
    out
}

/// `extract_localization_map`.
pub fn extract_localization_map(content: &[u8]) -> Locale {
    let mut mapping = Locale::new();
    let n = content.len();
    let mut cursor = 0;
    while let Some(off) = content.get(cursor..).and_then(|s| s.iter().position(|&b| b == b'$')) {
        let start = cursor + off;
        let mut end = start + 1;
        while end < n {
            let b = content[end];
            if !(b.is_ascii_alphanumeric() || b == b'_' || b == b'$') {
                break;
            }
            end += 1;
        }
        if end >= n || content[end] != 0x18 {
            cursor = start + 1;
            continue;
        }
        let Some((len, value_start)) = read_varint(content, end + 1) else {
            cursor = start + 1;
            continue;
        };
        if len < 0 || value_start as i128 + len > n as i128 {
            cursor = start + 1;
            continue;
        }
        let value_end = value_start + len as usize;
        let key = clean_localized_text(&decode_ignore(&content[start..end]));
        let value = clean_localized_text(&decode_ignore(&content[value_start..value_end]));
        if key.starts_with('$') && !value.is_empty() && !value.starts_with('$') {
            mapping.insert(key, value);
        }
        cursor = value_end;
    }
    mapping
}

// decode/common.py
/// `common.locale`: `$key -> text` from a `languages/<lang>/*.binfab` table.
pub fn locale(data: Option<&[u8]>) -> Locale {
    let Some(data) = data.filter(|d| !d.is_empty()) else { return Locale::new() };
    let Ok(pf) = parse(data) else { return locale_by_adjacency(data) };
    let mut out = Locale::new();
    if let Some(rows) = pf.root_leaf().get(0) {
        for row in rows.iter() {
            let leaf = row.leaf();
            if let (Some(k), Some(v)) = (leaf.str(0), leaf.str(1)) {
                out.entry(k.to_owned()).or_insert_with(|| v.to_owned());
            }
        }
    }
    if out.is_empty() { locale_by_adjacency(data) } else { out }
}

fn locale_by_adjacency(data: &[u8]) -> Locale {
    let raw = kiwi_core::binfab::harvest_strings(data, 2, 512);
    let mut out = Locale::new();
    for (i, h) in raw.iter().enumerate() {
        if h.field != 0 || !h.text.starts_with('$') || i + 1 >= raw.len() {
            continue;
        }
        let next = &raw[i + 1];
        let gap = next.offset as i128 - (h.offset as i128 + h.text.len() as i128);
        if next.field == 1 && (0..=4).contains(&gap) {
            out.insert(h.text.clone(), next.text.clone());
        }
    }
    out
}

/// `locale(tree.read(path))`.
pub fn locale_at(tree: &super::tree::Tree, path: &str) -> Locale {
    locale(tree.read(path).as_deref())
}

/// `ability.is_modifier`.
pub fn is_modifier(v: &W) -> bool {
    let Some(d) = v.as_dict() else { return false };
    let int_or_absent = |i: i128| d.get(i).is_none_or(|x| x.as_int().is_some());
    d.str(MOD_LABEL).is_some()
        && d.int(MOD_FLAGS).is_some()
        && int_or_absent(MOD_STAT)
        && int_or_absent(MOD_OP)
        && d.get(MOD_VALUE).and_then(W::as_num).is_some()
}

/// `common.stat_rows`.
pub fn stat_rows(values: &[W]) -> Vec<J> {
    let mut rows: Vec<J> = Vec::new();
    for v in values {
        if !is_modifier(v) {
            continue;
        }
        let d = v.as_dict().expect("modifier");
        let (sid, op) = (d.int0(MOD_STAT), d.int0(MOD_OP));
        let Some(key) = stat_keys(sid) else { continue };
        if !(0..7).contains(&op) {
            continue;
        }
        let raw = d.get(MOD_VALUE).and_then(W::as_num).unwrap_or(0.0);
        let mut value = normalize(key, op * 2, raw).0.abs();
        if sid == 0x0F && op == 0 {
            value = if raw.abs() < 1.0 { raw * 100.0 } else { raw };
        } else if sid == 0x0C && op == 1 {
            value = raw * 100.0;
        }
        let stat = if op == 0 { bonus_key(key) } else { key.to_owned() };
        let row = obj! {
            "name" => resolve_stat_name(&stat), "stat" => stat.as_str(), "op" => MOD_OPS[op as usize],
            "value" => pyfloat::round(value, 4), "amount" => pyfloat::round(raw, 4),
        };
        if !rows.contains(&row) {
            rows.push(row);
        }
    }
    rows
}

/// Every node under `v` (`walk_values`), collected.
pub fn walk_values(v: &W) -> Vec<W> {
    let mut out = Vec::new();
    v.walk(&mut |x| out.push(x.clone()));
    out
}

pub fn working_groups() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"^(InProgress|ReadyForGame|READY_FOR_GAME|NEEDS_AUDIO|Hidden)").expect("regex"))
}

/// `common.collection_members`: member ref -> (group id, group name).
pub fn collection_members(pf: Option<&Prefab>, names: &Locale) -> IndexMap<String, (String, String)> {
    let mut out: IndexMap<String, (String, String)> = IndexMap::new();
    let Some(pf) = pf else { return out };
    let Some(groups) = pf.root_leaf().get(0) else { return out };
    for group in groups.iter() {
        let g = group.leaf();
        let gid = g.str(0).unwrap_or("").to_owned();
        let label = if working_groups().is_match(&gid) {
            String::new()
        } else {
            let key = g.get(1).filter(|x| x.truthy());
            let named = key.and_then(W::as_str).and_then(|k| names.get(k)).cloned().unwrap_or_default();
            if named.is_empty() { gid.clone() } else { named }
        };
        for row in g.iter_of(3) {
            if let Some(r) = row.leaf().str(0) {
                let key = r.strip_suffix(".binfab").unwrap_or(r).to_owned();
                out.entry(key).or_insert_with(|| (gid.clone(), label.clone()));
            }
        }
    }
    out
}

/// `prefabs/item/gem/large/blue_x_t3.binfab` -> `blue_x_t3`.
pub fn stem(path: &str) -> String {
    let last = path.rsplit('/').next().unwrap_or(path);
    match last.rsplit_once('.') {
        Some((s, _)) => s.to_owned(),
        None => last.to_owned(),
    }
}

/// `str.removesuffix`.
pub fn rs<'a>(s: &'a str, suffix: &str) -> &'a str {
    s.strip_suffix(suffix).unwrap_or(s)
}

/// `str.removeprefix`.
pub fn rp<'a>(s: &'a str, prefix: &str) -> &'a str {
    s.strip_prefix(prefix).unwrap_or(s)
}

/// `names.get(key or "", "")` for a wire value that may not be a str.
pub fn lookup(names: &Locale, key: Option<&W>) -> String {
    match key {
        Some(W::Str(s)) => names.get(&**s).cloned().unwrap_or_default(),
        _ => String::new(),
    }
}

/// Python `round(x, 4)` of a float.
pub fn r4(x: f64) -> f64 {
    pyfloat::round(x, 4)
}

/// Thousands separators on a run of digits.
pub fn group_digits(digits: &str) -> String {
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// `f"{n:,}"` of an int.
pub fn commas(n: i128) -> String {
    let out = group_digits(&n.unsigned_abs().to_string());
    if n < 0 { format!("-{out}") } else { out }
}

/// Python `str.casefold()` for the ids the game uses.
pub fn casefold(s: &str) -> String {
    s.chars()
        .flat_map(char::to_lowercase)
        .map(|c| if c == 'ς' { 'σ' } else { c })
        .collect::<String>()
        .replace('ß', "ss")
}

pub fn arc(s: &str) -> Arc<str> {
    Arc::from(s)
}

//! `app/trove/codexes/bonuses.py`: numeric stat records and ability refs.

use std::sync::LazyLock;

use regex::bytes::Regex;
use serde_json::{Map, Value, json};

use super::{float_value, int_value, py, wire::unzig};

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

fn stat_key(id: i128) -> Option<&'static str> {
    usize::try_from(id).ok().and_then(|i| STAT_KEYS.get(i).copied())
}

fn operation_name(op: u8) -> Option<&'static str> {
    Some(match op {
        0 => "MultiplySum",
        2 => "Add",
        4 => "Set",
        6 => "Nullify",
        8 => "Multiply",
        10 => "Minimum",
        12 => "Maximum",
        _ => return None,
    })
}

/// `bonus_key`: the `…Bonus` twin, `_controller` kept on the end.
pub fn bonus_key(key: &str) -> String {
    match key.find("_controller") {
        Some(i) => format!("{}Bonus{}", &key[..i], &key[i..]),
        None => format!("{key}Bonus"),
    }
}

fn label_ok(label: &str) -> bool {
    !label.is_empty() && label.bytes().all(|b| b.is_ascii_alphanumeric() || b"_/.$-".contains(&b))
}

const LABEL_STATS: [(&str, &str); 13] = [
    ("physicaldamage", "$Stat_PhysicalDamage"),
    ("spelldamage", "$Stat_SpellDamage"),
    ("maxhealth", "$Stat_MaxHealth"),
    ("maxenergy", "$Stat_MaxEnergy"),
    ("healthregen", "$Stat_HealthRegen_controller"),
    ("energyregen", "$Stat_EnergyRegen_controller"),
    ("criticalhitchance", "$Stat_CriticalHitChance"),
    ("criticalhitdamage", "$Stat_CriticalHitDamage"),
    ("critdamage", "$Stat_CriticalHitDamage"),
    ("attackspeed", "$Stat_AttackSpeed"),
    ("magicfind", "$Stat_MagicFind"),
    ("jump", "$Stat_Jump"),
    ("mining", "$Stat_Mining"),
];

fn stat_from_label(label: &str) -> Option<&'static str> {
    let lowered = py::lower(label);
    if lowered.ends_with("ground_movespeed") || lowered.ends_with("wing_movespeed") {
        return Some("$Stat_MovementSpeed");
    }
    if lowered.ends_with("glide_movespeed") {
        return Some("$Stat_Glide");
    }
    LABEL_STATS
        .iter()
        .find(|(suffix, _)| lowered == *suffix || lowered.ends_with(&format!("_{suffix}")))
        .map(|(_, k)| *k)
}

const HIDDEN_ABILITY_SUBSTRINGS: [&str; 4] =
    ["flask_auto_use", "_reset_death_save", "equip_flask_cooldown", "mount_block_harvester"];

/// `decode_le_float`: a finite little-endian f32 as f64.
pub fn decode_le_float(raw: &[u8]) -> Option<f64> {
    let b: [u8; 4] = raw.try_into().ok()?;
    let v = f64::from(f32::from_le_bytes(b));
    v.is_finite().then_some(v)
}

fn normalize(stat_key: &str, operation: u8, amount: f64) -> (f64, bool) {
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

fn slot_for_label(label: &str) -> Option<&'static str> {
    let lowered = py::lower(label);
    if lowered.contains("ground_movespeed") {
        return Some("$EquipmentSlot_Mount");
    }
    if lowered.contains("wing_movespeed") || lowered.contains("glide_movespeed") {
        return Some("$EquipmentSlot_Wings");
    }
    None
}

fn slot_by_context(b: u8) -> Option<&'static str> {
    Some(match b {
        0x00 => "$EquipmentSlot_Mount",
        0x02 => "$EquipmentSlot_Cart",
        0x04 => "$EquipmentSlot_Wings",
        0x06 => "$EquipmentSlot_Boat",
        _ => return None,
    })
}

/// A fixed-length pattern (`None` = any byte) anchored with Python's `$`: at the
/// end, or before a final newline.
fn ends_with_py(hay: &[u8], pat: &[Option<u8>]) -> Option<usize> {
    let hay = match hay.last() {
        Some(b'\n') => &hay[..hay.len() - 1],
        _ => hay,
    };
    if hay.len() < pat.len() {
        return None;
    }
    let start = hay.len() - pat.len();
    pat.iter().zip(&hay[start..]).all(|(p, b)| p.is_none_or(|p| p == *b)).then_some(start)
}

const MOUNT_OPEN: [Option<u8>; 11] = [
    Some(0xbe),
    Some(0x01),
    Some(0xae),
    Some(0x01),
    Some(0x00),
    Some(0x04),
    Some(0x00),
    Some(0x10),
    Some(0x10),
    Some(0x04),
    Some(0x24),
];
const UNLOCK_OPEN: [Option<u8>; 10] =
    [Some(0x3e), Some(0xae), None, Some(0x00), Some(0x04), Some(0x00), None, Some(0x10), None, Some(0x24)];

fn context_open(b: &[u8]) -> Option<u8> {
    b.windows(7).find_map(|w| {
        (w[0] == 0x1e
            && (w[1] == 0x14 || w[1] == 0x24)
            && w[2] == 0x00
            && matches!(w[3], 0x00 | 0x02 | 0x04 | 0x06)
            && w[4..7] == [0xbe, 0x01, 0xae])
        .then_some(w[3])
    })
}

fn context_cont(b: &[u8]) -> Option<u8> {
    let hay = match b.last() {
        Some(b'\n') => &b[..b.len() - 1],
        _ => b,
    };
    let w = hay.get(hay.len().checked_sub(7)?..)?;
    (w[0] == 0x1e
        && (w[1] == 0x14 || w[1] == 0x24)
        && w[2] == 0x00
        && w[3] < 0x80
        && w[4] == 0x10
        && (w[5] == 0x02 || w[5] == 0x04)
        && w[6] == 0x24)
        .then_some(w[3])
}

/// `_slot_context`: (slot, opened a block, continuation stat id).
fn slot_context(prefix: &[u8], current: Option<&'static str>) -> (Option<&'static str>, bool, Option<i128>) {
    let before = if prefix.len() >= 4 { &prefix[..prefix.len() - 4] } else { prefix };
    if let Some(b) = context_open(before) {
        return (slot_by_context(b), true, None);
    }
    if ends_with_py(before, &MOUNT_OPEN).is_some() {
        return (Some("$EquipmentSlot_Mount"), true, None);
    }
    if ends_with_py(before, &UNLOCK_OPEN).is_some() {
        return (None, true, None);
    }
    if let Some(b) = context_cont(before) {
        return (current, false, Some(unzig(u128::from(b))));
    }
    (None, false, None)
}

fn stat_window(data: &[u8]) -> (usize, usize) {
    let Some(desc) = py::find(data, b"_description", 0) else { return (0, data.len()) };
    let start = desc.saturating_sub(96);
    match py::rfind(data, b".blueprint") {
        Some(end) if end > start => (start, end),
        _ => (start, data.len().min(desc + 4096)),
    }
}

/// One decoded stat record (`data.stats[i]`).
#[derive(Clone, Debug, PartialEq)]
pub struct StatRecord {
    pub stat: String,
    pub stat_id: Option<i128>,
    pub operation: u8,
    pub amount: f64,
    pub value: f64,
    pub is_percent: bool,
    pub label: String,
    pub slot: Option<&'static str>,
    pub component: Option<u8>,
    pub offset: usize,
}

impl StatRecord {
    pub fn to_value(&self) -> Map<String, Value> {
        let op = match operation_name(self.operation) {
            Some(name) => Value::from(name),
            None => Value::from(self.operation),
        };
        let mut m = Map::new();
        m.insert("stat".into(), Value::from(self.stat.clone()));
        m.insert("stat_id".into(), self.stat_id.map_or(Value::Null, int_value));
        m.insert("operation".into(), op);
        m.insert("amount".into(), float_value(self.amount));
        m.insert("value".into(), float_value(self.value));
        m.insert("is_percent".into(), Value::Bool(self.is_percent));
        m.insert("label".into(), Value::from(self.label.clone()));
        m.insert("slot".into(), self.slot.map_or(Value::Null, Value::from));
        m.insert("component".into(), self.component.map_or(Value::Null, Value::from));
        m.insert("level".into(), Value::from(0));
        m.insert("offset".into(), Value::from(self.offset));
        m
    }
}

fn detect(data: &[u8], start: usize, end: usize) -> Vec<StatRecord> {
    let mut records = Vec::new();
    let n = data.len();
    let end = end.min(n);
    let mut component: Option<u8> = None;
    let mut current_slot: Option<&'static str> = None;
    let mut i = start.max(8);
    while i < end {
        if data[i] == 0xAE && i + 3 < n && data[i + 2] == 0x00 && data[i + 3] == 0x04 {
            let group = data[i + 1];
            if group > 0 && group < 32 {
                component = Some(group);
            }
        }
        if data[i] != 0x38 || i + 1 >= n {
            i += 1;
            continue;
        }
        let length = data[i + 1] as usize;
        let label_end = i + 2 + length;
        if label_end > n {
            i += 1;
            continue;
        }
        let label = py::ascii_ignore(&data[i + 2..label_end]);
        if length > 0 && !label_ok(&label) {
            i += 1;
            continue;
        }
        if label_end >= n || data[label_end] != 0x46 {
            i += 1;
            continue;
        }
        let value = match decode_le_float(&data[i - 4..i]) {
            Some(v) if v.abs() <= 1_000_000.0 => v,
            _ => {
                i += 1;
                continue;
            }
        };
        let prefix = &data[i.saturating_sub(32)..i];
        let (slot, opened, continuation_id) = slot_context(prefix, current_slot);
        if opened {
            current_slot = slot;
        }
        let mut stat_id = Some(continuation_id.unwrap_or_else(|| unzig(u128::from(data[i - 8]))));
        let mut key = stat_id.and_then(stat_key);
        if key.is_none() {
            match stat_from_label(&label) {
                Some(k) => {
                    key = Some(k);
                    stat_id = None;
                }
                None => {
                    i += 1;
                    continue;
                }
            }
        }
        let key = key.expect("resolved stat");
        let operation = data[i - 6];
        let (display, is_percent) = normalize(key, operation, value);
        let stat = if operation == 0 { bonus_key(key) } else { key.to_owned() };
        records.push(StatRecord {
            stat,
            stat_id,
            operation,
            amount: value,
            value: display,
            is_percent,
            slot: slot_for_label(&label).or(if opened { slot } else { current_slot }),
            label,
            component,
            offset: i,
        });
        i = label_end + 1;
    }
    records
}

/// `extract_stat_bonuses`.
pub fn extract_stat_bonuses(data: &[u8]) -> Vec<StatRecord> {
    let (start, end) = stat_window(data);
    let records = detect(data, start, end);
    if records.is_empty() && (start, end) != (0, data.len()) {
        return detect(data, 0, data.len());
    }
    records
}

static ABILITY_RE: LazyLock<Regex> = LazyLock::new(|| py::bytes_re(r"abilities/[A-Za-z0-9_/.\\]+"));

fn ability_loc_key(r: &str) -> String {
    let flat = py::strip_chars(&r.replace('\\', "/"), "/").replace(['/', '.'], "_");
    let spawner = if r.contains("use_flask") { "inventoryeventeffectspawner" } else { "combateventeffectspawner" };
    format!("$prefabs_{flat}_{spawner}_description")
}

/// `extract_abilities`: `{ref, hidden[, key, amount]}` rows in source order.
pub fn extract_abilities(data: &[u8]) -> Vec<Map<String, Value>> {
    let mut refs: Vec<String> = Vec::new();
    for m in ABILITY_RE.find_iter(data) {
        let r = py::rstrip_chars(&py::ascii_ignore(m.as_bytes()).replace('\\', "/"), "./").to_owned();
        if !refs.contains(&r) {
            refs.push(r);
        }
    }
    let dummy: Vec<&str> = refs
        .iter()
        .filter(|r| r.contains("use_flask_dummyhealth_"))
        .map(|r| py::last_part(r, "dummyhealth_"))
        .collect();
    refs.iter()
        .map(|r| {
            let mut hidden = HIDDEN_ABILITY_SUBSTRINGS.iter().any(|t| r.contains(t));
            if r.contains("use_flask_health_") && dummy.contains(&py::last_part(r, "use_flask_health_")) {
                hidden = true;
            }
            let mut row = Map::new();
            row.insert("ref".into(), json!(r));
            row.insert("hidden".into(), json!(hidden));
            if !hidden {
                row.insert("key".into(), json!(ability_loc_key(r)));
                row.insert("amount".into(), json!(0));
            }
            row
        })
        .collect()
}

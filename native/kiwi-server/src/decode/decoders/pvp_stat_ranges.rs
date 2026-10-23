//! `pvp_stat_ranges.py` (pvp.json): the PvE -> PvP stat curves.

use std::collections::HashMap;

use crate::{
    decode::{
        common::{r4, read_uleb, stat_keys, unzig},
        json::{J, Map},
        registry::{Built, Ctx, DecodeError},
    },
    obj,
};

const MODES: [(&str, &str); 3] = [
    ("pvp", "pvp_statranges.binfab"),
    ("battleroyale", "pvp_battleroyale_statranges.binfab"),
    ("bloodstone", "pvp_bloodstone_statranges.binfab"),
];
const REQUIRED: [i128; 6] = [2, 4, 5, 6, 7, 8];
const EMPTY_LIST: [u8; 10] = [0x3e, 0x2e, 0x00, 0x08, 0xbe, 0x01, 0x2e, 0x00, 0x08, 0x1e];
const ROLE_FILES: [&str; 2] = ["pvp_roles.binfab", "pvp_classes.binfab"];

fn scale(stat: &str) -> Option<J> {
    match stat {
        "PhysicalDamage"
        | "SpellDamage"
        | "MaxHealth"
        | "MaxEnergy"
        | "HealthRegen_controller"
        | "EnergyRegen_controller"
        | "MovementSpeed"
        | "AttackSpeed"
        | "Jump"
        | "CriticalHitDamage" => Some(J::Int(1)),
        "CriticalHitChance" => Some(J::Float(0.1)),
        _ => None,
    }
}

#[derive(Clone)]
enum F {
    Vec2([f64; 2]),
    Float(f64),
    Int(i128),
}

struct Row {
    stat: i128,
    fields: HashMap<i128, F>,
}

fn index_error() -> DecodeError {
    DecodeError { kind: "IndexError", msg: "index out of range".into() }
}

fn byte(data: &[u8], i: usize) -> Result<u8, DecodeError> {
    data.get(i).copied().ok_or_else(index_error)
}

fn uleb(data: &[u8], pos: usize) -> Result<(i128, usize), DecodeError> {
    read_uleb(data, pos).ok_or_else(index_error)
}

fn f32_at(data: &[u8], pos: usize) -> Result<f64, DecodeError> {
    let b = data
        .get(pos..pos + 4)
        .ok_or_else(|| DecodeError { kind: "error", msg: "unpack_from requires a buffer".into() })?;
    Ok(f64::from(f32::from_le_bytes([b[0], b[1], b[2], b[3]])))
}

fn parse(data: &[u8]) -> Result<Vec<Row>, DecodeError> {
    if data.get(..2) != Some(&[0x3e, 0xae][..]) {
        return Err(DecodeError::value("not a statranges list"));
    }
    let (count, mut pos) = uleb(data, 2)?;
    pos += 1;
    let mut rows = Vec::new();
    for _ in 0..count.max(0) {
        let (key, p) = uleb(data, pos)?;
        pos = p;
        if key & 0xF != 4 || byte(data, pos)? != 0 {
            return Err(DecodeError::value(format!("bad record header at {pos}")));
        }
        let (stat, p) = uleb(data, pos + 1)?;
        pos = p;
        let mut row = Row { stat: unzig(stat), fields: HashMap::new() };
        while byte(data, pos)? != 0x1E {
            let (key, p) = uleb(data, pos)?;
            pos = p;
            let (field, wire) = (key >> 4, key & 0xF);
            match wire {
                0xC => {
                    if byte(data, pos)? != 0x24 {
                        return Err(DecodeError::value(format!("field {field}: not a vec2")));
                    }
                    row.fields.insert(field, F::Vec2([f32_at(data, pos + 1)?, f32_at(data, pos + 5)?]));
                    pos += 9;
                }
                4 => {
                    row.fields.insert(field, F::Float(f32_at(data, pos)?));
                    pos += 4;
                }
                0 => {
                    let (v, p) = uleb(data, pos)?;
                    pos = p;
                    row.fields.insert(field, F::Int(unzig(v)));
                }
                _ => return Err(DecodeError::value(format!("field {field}: unknown wire type {wire}"))),
            }
        }
        rows.push(row);
        pos += 1;
    }
    Ok(rows)
}

fn rounded(f: &F) -> Result<J, DecodeError> {
    match f {
        F::Float(x) => Ok(J::Float(r4(*x))),
        F::Int(i) => Ok(J::Int(*i)),
        F::Vec2(_) => Err(DecodeError { kind: "TypeError", msg: "type list doesn't define __round__ method".into() }),
    }
}

fn truthy(f: &F) -> bool {
    match f {
        F::Float(x) => *x != 0.0,
        F::Int(i) => *i != 0,
        F::Vec2(_) => true,
    }
}

fn modifier(stat: Option<&str>, label: &str, value: f64) -> J {
    obj! {"stat" => stat, "label" => label, "op" => "multiply", "value" => value}
}

fn class(tech: &str, name: &str) -> J {
    J::List(vec![J::from(tech), J::from(name)])
}

fn roles() -> J {
    let role = |key: &str, name: &str, classes: Vec<J>, mods: Vec<J>| {
        obj! {"key" => key, "name" => name, "classes" => classes, "modifiers" => mods}
    };
    J::List(vec![
        role(
            "Tank",
            "Tank",
            vec![
                class("candybarbarian", "Candy Barbarian"),
                class("spirittank", "Revenant"),
                class("knight", "Knight"),
            ],
            vec![
                modifier(Some("MaxHealth"), "Maximum Health", 1.15),
                modifier(Some("OutgoingDamageMod"), "Outgoing damage", 0.9),
            ],
        ),
        role(
            "Assassin",
            "Assassin",
            vec![class("shadowhunter", "Shadow Hunter"), class("neonninja", "Neon Ninja")],
            vec![
                modifier(Some("MaxHealth"), "Maximum Health", 0.85),
                modifier(Some("OutgoingDamageMod"), "Outgoing damage", 1.12),
            ],
        ),
        role(
            "Skirmisher",
            "Skirmisher",
            vec![
                class("lunarlancer", "Lunar Lancer"),
                class("adventurer", "Boomeranger"),
                class("crimefighter", "Vanguardian"),
            ],
            vec![
                modifier(Some("MovementSpeed"), "Movement Speed", 1.08),
                modifier(Some("MaxHealth"), "Maximum Health", 0.95),
            ],
        ),
        role(
            "Mage",
            "Mage",
            vec![
                class("icemage", "Ice Sage"),
                class("dracolyte", "Dracolyte"),
                class("tombraiser", "Tomb Raiser"),
                class("faetrickster", "Fae Trickster"),
            ],
            vec![modifier(Some("SpellDamage"), "Magic Damage", 1.15), modifier(None, "Cooldown", 1.2)],
        ),
        role(
            "Support",
            "Support",
            vec![class("chloromancer", "Chloromancer"), class("bard", "Bard")],
            vec![modifier(Some("HealDoneMultiplier"), "Healing done", 0.9), modifier(None, "Cooldown", 1.15)],
        ),
        role(
            "RangedDPS",
            "Ranged DPS",
            vec![
                class("gunslinger", "Gunslinger"),
                class("piratelord", "Pirate Captain"),
                class("dinotamer", "Dino Tamer"),
                class("solarion", "Solarion"),
            ],
            vec![
                modifier(Some("CriticalHitDamage"), "Critical Damage", 1.15),
                modifier(Some("MaxHealth"), "Maximum Health", 0.92),
            ],
        ),
    ])
}

pub fn build(ctx: &Ctx) -> Built {
    let mut modes = Map::new();
    for (key, name) in MODES {
        let data = ctx
            .tree
            .read(&format!("prefabs/pvp/data/stats/{name}"))
            .ok_or_else(|| DecodeError::value(format!("missing prefabs/pvp/data/stats/{name}")))?;
        let mut stats = Map::new();
        for row in parse(&data)? {
            let stat = stat_keys(row.stat).map_or("", |k| k.strip_prefix("$Stat_").unwrap_or(k));
            let Some(sc) = scale(stat) else { continue };
            let missing: Vec<String> =
                REQUIRED.iter().filter(|f| !row.fields.contains_key(f)).map(|f| f.to_string()).collect();
            if !missing.is_empty() {
                return Err(DecodeError::value(format!("{name} {stat}: missing fields [{}]", missing.join(", "))));
            }
            let out = match &row.fields[&2] {
                F::Vec2(v) => J::List(v.iter().map(|x| J::Float(r4(*x))).collect()),
                _ => return Err(DecodeError { kind: "TypeError", msg: "object is not iterable".into() }),
            };
            stats.insert(
                stat.to_owned(),
                obj! {
                    "scale" => sc, "out" => out, "knee" => rounded(&row.fields[&4])?, "sqrt" => rounded(&row.fields[&5])?,
                    "curve" => truthy(&row.fields[&6]), "floor" => truthy(&row.fields[&7]), "cap" => rounded(&row.fields[&8])?,
                },
            );
        }
        modes.insert(key.into(), J::Dict(stats));
    }
    for name in ROLE_FILES {
        if let Some(data) = ctx.tree.read(&format!("prefabs/pvp/data/stats/{name}"))
            && data != EMPTY_LIST
        {
            return Err(DecodeError::value(format!("{name} now ships data: decode it instead of the hand-kept ROLES")));
        }
    }
    Ok(obj! {"modes" => modes, "roles" => roles(), "role_modes" => vec![J::from("pvp")]})
}

pub fn count(data: &J) -> Option<usize> {
    data.get("modes")?.as_dict().map(|m| m.values().map(J::len).sum())
}

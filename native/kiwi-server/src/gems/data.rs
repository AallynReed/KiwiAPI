//! The gem tables, read from the game files into kiwi-core's structs.

use std::{collections::HashMap, sync::Arc};

use kiwi_core::gems::{
    GemError,
    builds::{BonusStat, ClassInfo, HEALTH_LINES, HealthSources},
    pynum::{PyNum, py_sum},
    upgrades::{BoosterRow, Cost, GemStats, LevelRow, PoolRow, Record, StatRow, Upgrades},
};
use serde_json::{Map, Value};

use super::{
    star_chart::StarChart,
    store::{Cached, Store},
};

pub const UPGRADES: &str = "gem_upgrades.json";
pub const ABILITIES: &str = "gem_abilities.json";

const SUM_FILES: [&str; 10] = [
    "health",
    "health_per",
    "damage",
    "critical_damage",
    "light",
    "bonus_damage",
    "physical_damage/dragons_damage",
    "magic_damage/dragons_damage",
    "dragons_damage",
    "dragons_critical_damage",
];

const BUILD_FILES: [&str; 17] = [
    UPGRADES,
    "classes.json",
    "star_chart.json",
    "builds/food.json",
    "builds/ally.json",
    "builds/face_damage.json",
    "builds/health_optimizer.json",
    "builds/health.json",
    "builds/health_per.json",
    "builds/damage.json",
    "builds/critical_damage.json",
    "builds/light.json",
    "builds/bonus_damage.json",
    "builds/physical_damage/dragons_damage.json",
    "builds/magic_damage/dragons_damage.json",
    "builds/dragons_damage.json",
    "builds/dragons_critical_damage.json",
];

fn num(v: Option<&Value>) -> f64 {
    match v {
        Some(Value::Number(n)) => n.as_f64().unwrap_or(0.0),
        Some(Value::Bool(b)) => f64::from(u8::from(*b)),
        _ => 0.0,
    }
}

fn int(v: Option<&Value>) -> i64 {
    match v {
        Some(Value::Number(n)) => n.as_i64().unwrap_or_else(|| n.as_f64().unwrap_or(0.0) as i64),
        Some(Value::Bool(b)) => i64::from(*b),
        _ => 0,
    }
}

fn text(v: Option<&Value>) -> String {
    v.and_then(Value::as_str).unwrap_or_default().to_owned()
}

/// Python truthiness of an optional value.
pub fn truthy(v: Option<&Value>) -> bool {
    v.is_some_and(crate::features::py_truthy)
}

fn list(v: Option<&Value>) -> &[Value] {
    v.and_then(Value::as_array).map_or(&[], Vec::as_slice)
}

fn obj(v: Option<&Value>) -> Option<&Map<String, Value>> {
    v.and_then(Value::as_object)
}

/// A JSON number as Python sees it (int or float).
pub fn py_num(v: &Value) -> PyNum {
    match v {
        Value::Number(n) => match n.as_i64() {
            Some(i) => PyNum::Int(i),
            None => PyNum::Float(n.as_f64().unwrap_or(0.0)),
        },
        Value::Bool(b) => PyNum::Int(i64::from(*b)),
        _ => PyNum::Int(0),
    }
}

/// `sum(d.values())` of a JSON object.
fn sum_values(v: Option<&Value>) -> PyNum {
    obj(v).map_or(PyNum::Int(0), |o| py_sum(o.values().map(py_num)))
}

fn upgrades(v: Option<&Value>) -> Upgrades {
    let mut up = Upgrades::default();
    let Some(d) = obj(v) else { return up };
    for g in list(d.get("gems")) {
        let stats = list(g.get("stats"))
            .iter()
            .map(|s| StatRow {
                stat: text(s.get("stat")),
                percent: truthy(s.get("percent")),
                min: num(s.get("min")),
                max: num(s.get("max")),
                step: num(s.get("step")),
            })
            .collect();
        let pool = list(g.get("pool"))
            .iter()
            .map(|p| PoolRow {
                stat: text(p.get("stat")),
                percent: truthy(p.get("percent")),
                weight: num(p.get("weight")),
            })
            .collect();
        let levels = list(g.get("levels"))
            .iter()
            .map(|lv| LevelRow {
                level: int(lv.get("level")),
                chance: num(lv.get("chance")),
                double_chance: num(lv.get("double_chance")),
                stat_steps: int(lv.get("stat_steps")),
                boost: truthy(lv.get("boost")),
                cost: list(lv.get("cost"))
                    .iter()
                    .map(|c| Cost { item: text(c.get("item")), count: int(c.get("count")) })
                    .collect(),
                dust: int(lv.get("dust")),
                amber: int(lv.get("amber")),
            })
            .collect();
        let i = up.records.len();
        up.records.push(Record { max_level: int(g.get("max_level")), stats, pool, levels });
        for item in list(g.get("items")) {
            up.index.insert((text(item.get("size")), text(item.get("color")), int(item.get("tier"))), i);
        }
    }
    if let Some(m) = obj(d.get("materials")) {
        for (color, mats) in m {
            up.materials.insert(color.clone(), (text(mats.get("dust")), text(mats.get("amber"))));
        }
    }
    up.boosters = list(d.get("boosters"))
        .iter()
        .map(|b| BoosterRow {
            item: text(b.get("item")),
            chance_multiplier: num(b.get("chance_multiplier")),
            double_multiplier: num(b.get("double_multiplier")),
        })
        .collect();
    up.focuses = list(d.get("focuses")).iter().map(|f| (text(f.get("item")), num(f.get("amount")))).collect();
    if let Some(m) = obj(d.get("names")) {
        for (item, name) in m {
            up.names.insert(item.clone(), name.as_str().unwrap_or_default().to_owned());
        }
    }
    up
}

pub struct GemsData {
    pub up: Upgrades,
}

pub fn gems(store: &Store) -> Arc<GemsData> {
    static CACHE: std::sync::LazyLock<Cached<GemsData>> = std::sync::LazyLock::new(Cached::default);
    CACHE.get(store, &[UPGRADES], |s| GemsData { up: upgrades(s.load(UPGRADES).as_ref()) })
}

/// A food or ally: key, label, stat lines.
pub struct Bonus {
    pub key: String,
    pub label: String,
    pub stats: Vec<BonusStat>,
}

pub struct BuildsData {
    pub class_names: Vec<String>,
    pub classes: Vec<ClassInfo>,
    pub foods: Vec<Bonus>,
    pub allies: Vec<Bonus>,
    pub face_damage: f64,
    pub sums: HashMap<&'static str, f64>,
    pub health: HealthSources,
    pub chart: StarChart,
    pub gem_stats: Result<GemStats, GemError>,
}

fn bonuses(v: Option<&Value>) -> Vec<Bonus> {
    obj(v).map_or_else(Vec::new, |m| {
        m.iter()
            .map(|(k, b)| Bonus {
                key: k.clone(),
                label: b.get("qualified_name").map_or_else(|| k.clone(), |l| l.as_str().unwrap_or_default().to_owned()),
                stats: list(b.get("stats"))
                    .iter()
                    .map(|s| BonusStat {
                        name: text(s.get("name")),
                        value: num(s.get("value")),
                        percentage: truthy(s.get("percentage")),
                    })
                    .collect(),
            })
            .collect()
    })
}

fn classes(v: Option<&Value>) -> (Vec<String>, Vec<ClassInfo>) {
    let rows = list(v);
    let names = rows.iter().map(|c| text(c.get("name"))).collect();
    let infos = rows
        .iter()
        .map(|c| ClassInfo {
            name: text(c.get("name")),
            magic: c.get("damage_type").and_then(Value::as_str) == Some("Magic"),
            stats: list(c.get("stats")).iter().map(|s| (text(s.get("name")), num(s.get("value")))).collect(),
            bonuses: list(c.get("bonuses"))
                .iter()
                .map(|b| (text(b.get("name")), b.get("value").filter(|v| !v.is_null()).map(|v| num(Some(v)))))
                .collect(),
        })
        .collect();
    (names, infos)
}

fn health_sources(v: Option<&Value>) -> HealthSources {
    let d = obj(v);
    let get = |k: &str| d.and_then(|d| d.get(k));
    let sums = HEALTH_LINES.map(|line| sum_values(get(line)).f64());
    let gear_lines = obj(get("gear_lines")).map_or_else(Vec::new, |m| {
        m.iter()
            .filter_map(|(flag, line)| {
                let stat = line.get("stat").and_then(Value::as_str)?;
                let i = HEALTH_LINES.iter().position(|l| *l == stat)?;
                Some((flag.clone(), i, num(line.get("value"))))
            })
            .collect()
    });
    HealthSources { sums, gear_lines }
}

pub fn builds(store: &Store) -> Arc<BuildsData> {
    static CACHE: std::sync::LazyLock<Cached<BuildsData>> = std::sync::LazyLock::new(Cached::default);
    CACHE.get(store, &BUILD_FILES, |s| {
        let (class_names, classes) = classes(s.load("classes.json").as_ref());
        let face = s.load("builds/face_damage.json");
        let mut sums = HashMap::new();
        for name in SUM_FILES {
            let data = s.load(&format!("builds/{name}.json"));
            let total = match obj(data.as_ref()) {
                Some(m) if !m.is_empty() => sum_values(data.as_ref()).f64(),
                _ => 0.0,
            };
            sums.insert(name, total);
        }
        BuildsData {
            class_names,
            classes,
            foods: bonuses(s.load("builds/food.json").as_ref()),
            allies: bonuses(s.load("builds/ally.json").as_ref()),
            face_damage: obj(face.as_ref()).map_or(0.0, |m| num(m.get("Face"))),
            sums,
            health: health_sources(s.load("builds/health_optimizer.json").as_ref()),
            chart: StarChart::new(s.load("star_chart.json").as_ref()),
            gem_stats: upgrades(s.load(UPGRADES).as_ref()).build_gem_stats(),
        }
    })
}

/// `gamedata.load("gem_abilities.json", [])`, cached on the file.
pub fn abilities(store: &Store) -> Arc<Value> {
    static CACHE: std::sync::LazyLock<Cached<Value>> = std::sync::LazyLock::new(Cached::default);
    CACHE.get(store, &[ABILITIES], |s| s.load(ABILITIES).unwrap_or_else(|| Value::Array(Vec::new())))
}

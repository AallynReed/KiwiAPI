//! The baseline JSON `server_time._load` reads (`app/trove/gamedata/{daily_buffs,
//! weekly_buffs,biomes}.json`, cached for the process; a missing or broken file
//! is `{}`), and the response models that project it.

use std::{collections::HashMap, path::Path, sync::OnceLock};

use kiwi_core::rotations::BIOME_ICON_FALLBACK;
use serde_json::{Map, Value, json};

/// Relative to the working directory, which is the repo root in the image (`/app`).
pub const BASELINE_DIR: &str = "app/trove/gamedata";

pub struct Gamedata {
    pub daily: Map<String, Value>,
    pub weekly: Map<String, Value>,
    pub biomes: Map<String, Value>,
    icons: HashMap<String, String>,
}

/// A value pydantic would reject; the route answers 500 like Python.
#[derive(Debug)]
pub struct Invalid;

pub type Out<T = Value> = Result<T, Invalid>;

fn load(dir: &Path, file: &str) -> Map<String, Value> {
    std::fs::read(dir.join(file))
        .ok()
        .and_then(|b| crate::pyjson::loads(&b).ok())
        .and_then(|v| match v {
            Value::Object(m) => Some(m),
            _ => None,
        })
        .unwrap_or_default()
}

impl Gamedata {
    pub fn load(dir: &Path) -> Gamedata {
        let biomes = load(dir, "biomes.json");
        let mut icons = HashMap::new();
        for sub in biomes.values() {
            let parent = sub.get("biome").and_then(Value::as_str).filter(|p| !p.is_empty());
            if let Some(parent) = parent
                && !icons.contains_key(parent)
            {
                let icon = sub.get("icon").and_then(Value::as_str).unwrap_or("unknown");
                icons.insert(parent.to_owned(), icon.to_owned());
            }
        }
        Gamedata { daily: load(dir, "daily_buffs.json"), weekly: load(dir, "weekly_buffs.json"), biomes, icons }
    }

    /// The process-wide baseline.
    pub fn get() -> &'static Gamedata {
        static DATA: OnceLock<Gamedata> = OnceLock::new();
        DATA.get_or_init(|| Gamedata::load(Path::new(BASELINE_DIR)))
    }

    /// `_sub_biome(key)`: a d15 entry is a sub-biome key.
    pub fn sub_biome(&self, key: &str) -> Out {
        match self.biomes.get(key) {
            Some(Value::Object(sb)) if !sb.is_empty() => {
                let field = |k: &str, default: &str| match sb.get(k) {
                    None => Ok(Value::String(default.to_owned())),
                    Some(v @ Value::String(_)) => Ok(v.clone()),
                    Some(_) => Err(Invalid),
                };
                Ok(
                    json!({"name": field("name", key)?, "final_name": field("final_name", key)?, "icon": field("icon", "unknown")?}),
                )
            }
            Some(v) if crate::features::py_truthy(v) => Err(Invalid),
            _ => Ok(json!({"name": key, "final_name": key, "icon": "unknown"})),
        }
    }

    /// `_parent_biome(name)["icon"]`: the icon map, else the fallback table.
    pub fn parent_icon(&self, name: &str) -> &str {
        match self.icons.get(name).filter(|i| !i.is_empty()) {
            Some(icon) => icon,
            None => BIOME_ICON_FALLBACK.iter().find(|(n, _)| *n == name).map_or("unknown", |(_, i)| i),
        }
    }

    pub fn parent_biome(&self, name: &str) -> Value {
        json!({"name": name, "final_name": name, "icon": self.parent_icon(name)})
    }
}

fn get_str(m: &Map<String, Value>, key: &str) -> Out {
    match m.get(key) {
        Some(v @ Value::String(_)) => Ok(v.clone()),
        _ => Err(Invalid),
    }
}

fn get_opt_str(m: &Map<String, Value>, key: &str) -> Out {
    match m.get(key) {
        None | Some(Value::Null) => Ok(Value::Null),
        Some(v @ Value::String(_)) => Ok(v.clone()),
        _ => Err(Invalid),
    }
}

fn get_str_list(m: &Map<String, Value>, key: &str) -> Out {
    match m.get(key) {
        Some(Value::Array(items)) if items.iter().all(Value::is_string) => Ok(Value::Array(items.clone())),
        _ => Err(Invalid),
    }
}

/// The `DailyBuff` model.
pub fn daily_buff(v: &Value) -> Out {
    let m = v.as_object().ok_or(Invalid)?;
    Ok(json!({
        "name": get_str(m, "name")?,
        "color": get_str(m, "color")?,
        "weekday": get_str(m, "weekday")?,
        "emoji": get_str(m, "emoji")?,
        "normal_buffs": get_str_list(m, "normal_buffs")?,
        "premium_buffs": get_str_list(m, "premium_buffs")?,
        "icon": get_opt_str(m, "icon")?,
        "banner": get_opt_str(m, "banner")?,
    }))
}

/// The `WeeklyBuff` model.
pub fn weekly_buff(v: &Value) -> Out {
    let m = v.as_object().ok_or(Invalid)?;
    Ok(json!({
        "name": get_str(m, "name")?,
        "color": get_str(m, "color")?,
        "weekday": get_str(m, "weekday")?,
        "emoji": get_str(m, "emoji")?,
        "buffs": get_str_list(m, "buffs")?,
        "banner": get_opt_str(m, "banner")?,
    }))
}

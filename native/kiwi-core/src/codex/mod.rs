//! The codex indexer's prefab decoders (`app/trove/codexes/*.py`): heuristic
//! byte scanners over `.binfab` files, ported rule for rule. Entry payloads are
//! `serde_json` values built in Python's dict insertion order.

pub mod abilities;
pub mod badges;
pub mod blocks;
pub mod bonuses;
pub mod catalogue;
pub mod extract;
pub mod geode;
pub mod links;
pub mod localize;
pub mod loot;
pub mod mastery;
pub mod models;
pub mod powerrank;
pub mod providers;
pub mod py;
pub mod recipe;
pub mod styles;
pub mod types;
pub mod unlocks;
pub mod upgrades;
pub mod wire;

#[cfg(test)]
mod tests;

/// A Python exception the decoders let escape (its `str()`), which aborts the
/// whole index run exactly where Python's would.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PyErr(pub String);

impl std::fmt::Display for PyErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for PyErr {}

pub type PyResult<T> = Result<T, PyErr>;

/// A Python int as a JSON number (ints past `u64` only come from garbage varints).
pub fn int_value(v: i128) -> serde_json::Value {
    if let Ok(i) = i64::try_from(v) {
        serde_json::Value::from(i)
    } else if let Ok(u) = u64::try_from(v) {
        serde_json::Value::from(u)
    } else {
        serde_json::Value::from(v as f64)
    }
}

/// A finite float as a JSON number (NaN/inf have no JSON form).
pub fn float_value(v: f64) -> serde_json::Value {
    serde_json::Number::from_f64(v).map_or(serde_json::Value::Null, serde_json::Value::Number)
}

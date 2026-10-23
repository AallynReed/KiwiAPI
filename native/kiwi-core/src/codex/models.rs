//! `app/trove/codexes/models.py`: the child-table projections of `data`.

use serde_json::{Map, Value};

/// A `codex_stat` row (branch added by the writer).
#[derive(Clone, Debug, PartialEq)]
pub struct StatRow {
    pub path: String,
    pub ord: i64,
    pub stat_key: String,
    pub stat_id: Option<i64>,
    pub stat_name: String,
    pub operation: String,
    pub amount: Option<f64>,
    pub value: Option<f64>,
    pub is_percent: bool,
    pub slot_key: Option<String>,
    pub label: String,
    pub level: i64,
}

/// A `codex_ability` row.
#[derive(Clone, Debug, PartialEq)]
pub struct AbilityRow {
    pub path: String,
    pub ord: i64,
    pub r#ref: String,
    pub hidden: bool,
    pub loc_key: Option<String>,
    pub name: String,
    pub description: String,
}

fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|f| f != 0.0),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
    }
}

/// `str(v)` of a JSON scalar as Python prints it.
fn py_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::Null => "None".into(),
        other => other.to_string(),
    }
}

/// `_f`: a finite float or `None`.
pub fn finite(v: Option<&Value>) -> Option<f64> {
    let f = match v? {
        Value::Number(n) => n.as_f64()?,
        Value::Bool(b) => f64::from(u8::from(*b)),
        _ => return None,
    };
    f.is_finite().then_some(f)
}

fn text(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => s.clone(),
        Some(other) if truthy(Some(other)) => py_str(other),
        _ => String::new(),
    }
}

fn opt_str(v: Option<&Value>) -> Option<String> {
    v.and_then(Value::as_str).map(str::to_owned)
}

fn list<'a>(data: &'a Map<String, Value>, key: &str) -> &'a [Value] {
    data.get(key).and_then(Value::as_array).map_or(&[], Vec::as_slice)
}

/// `stat_rows`.
pub fn stat_rows(path: &str, data: &Map<String, Value>) -> Vec<StatRow> {
    let mut out = Vec::new();
    for (ord, stat) in list(data, "stats").iter().enumerate() {
        let Some(key) = stat.get("stat").filter(|k| truthy(Some(k))) else { continue };
        let operation = match stat.get("operation") {
            Some(Value::String(s)) => s.clone(),
            other => {
                if truthy(other) {
                    py_str(other.expect("truthy"))
                } else {
                    String::new()
                }
            }
        };
        out.push(StatRow {
            path: path.to_owned(),
            ord: ord as i64,
            stat_key: py_str(key),
            stat_id: stat.get("stat_id").and_then(Value::as_i64),
            stat_name: text(stat.get("stat_name")),
            operation,
            amount: finite(stat.get("amount")),
            value: finite(stat.get("value")),
            is_percent: truthy(stat.get("is_percent")),
            slot_key: opt_str(stat.get("slot")),
            label: text(stat.get("label")),
            level: stat.get("level").and_then(Value::as_i64).unwrap_or(0),
        });
    }
    out
}

/// `ability_rows`.
pub fn ability_rows(path: &str, data: &Map<String, Value>) -> Vec<AbilityRow> {
    let mut out = Vec::new();
    for (ord, a) in list(data, "abilities").iter().enumerate() {
        let Some(r) = a.get("ref").filter(|r| truthy(Some(r))) else { continue };
        out.push(AbilityRow {
            path: path.to_owned(),
            ord: ord as i64,
            r#ref: py_str(r),
            hidden: truthy(a.get("hidden")),
            loc_key: opt_str(a.get("key")),
            name: text(a.get("name")),
            description: text(a.get("description")),
        });
    }
    out
}

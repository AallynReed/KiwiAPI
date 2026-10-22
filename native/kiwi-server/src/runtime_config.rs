//! Live tunables, as in `app/admin/runtime_config.py`: sparse overrides in the
//! Mongo `runtime_config` collection (`{key, value: <JSON string>, updated_at,
//! updated_by_user_id}`) over the registry defaults, cached 5s per key. Without
//! Mongo every key reads its default.

mod registry;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use mongodb::bson::{Bson, Document, doc};
use serde_json::Value;

use crate::{config::Settings, state::AppState};

pub use registry::REGISTRY;

pub const COLLECTION: &str = "runtime_config";
const CACHE_TTL: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ty {
    Str,
    Int,
    Bool,
    Float,
}

impl Ty {
    pub fn as_str(self) -> &'static str {
        match self {
            Ty::Str => "str",
            Ty::Int => "int",
            Ty::Bool => "bool",
            Ty::Float => "float",
        }
    }
}

/// A bound as Python wrote it: `0` and `0.0` print differently in messages.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Num {
    Int(i64),
    Float(f64),
}

impl Num {
    fn as_f64(self) -> f64 {
        match self {
            Num::Int(i) => i as f64,
            Num::Float(f) => f,
        }
    }

    pub fn to_json(self) -> Value {
        match self {
            Num::Int(i) => Value::from(i),
            Num::Float(f) => Value::from(f),
        }
    }

    fn py_str(self) -> String {
        match self {
            Num::Int(i) => i.to_string(),
            Num::Float(f) => py_float_repr(f),
        }
    }
}

pub enum DefaultValue {
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(&'static str),
    /// Python's `default=settings.<field>`.
    Setting(fn(&Settings) -> Value),
}

pub struct Spec {
    pub key: &'static str,
    pub ty: Ty,
    pub category: &'static str,
    pub description: &'static str,
    pub default: DefaultValue,
    pub secret: bool,
    pub min_value: Option<Num>,
    pub max_value: Option<Num>,
    pub choices: Option<&'static [&'static str]>,
    pub pattern: Option<&'static str>,
}

impl Spec {
    pub const BASE: Spec = Spec {
        key: "",
        ty: Ty::Str,
        category: "",
        description: "",
        default: DefaultValue::Str(""),
        secret: false,
        min_value: None,
        max_value: None,
        choices: None,
        pattern: None,
    };

    pub fn default_value(&self, settings: &Settings) -> Value {
        match self.default {
            DefaultValue::Bool(b) => Value::Bool(b),
            DefaultValue::Int(i) => Value::from(i),
            DefaultValue::Float(f) => Value::from(f),
            DefaultValue::Str(s) => Value::from(s),
            DefaultValue::Setting(f) => f(settings),
        }
    }

    /// Python's `_validate`: coerce + range-check a write and return the value to
    /// persist. Error messages match `InvalidSettingError` text exactly.
    pub fn validate(&self, value: &Value) -> Result<Value, InvalidSetting> {
        let key = self.key;
        let err = |msg: String| Err(InvalidSetting(msg));
        match self.ty {
            Ty::Str => {
                let Value::String(s) = value else {
                    return err(format!("{key} expects a string"));
                };
                if let Some(choices) = self.choices
                    && !choices.contains(&s.as_str())
                {
                    let list: Vec<String> = choices.iter().map(|c| py_str_repr(c)).collect();
                    return err(format!("{key} must be one of [{}]", list.join(", ")));
                }
                if let Some(pattern) = self.pattern
                    && !py_fullmatch(pattern, s)
                {
                    return err(format!("{key} has an invalid format"));
                }
                Ok(value.clone())
            }
            Ty::Bool => match value {
                Value::Bool(_) => Ok(value.clone()),
                _ => err(format!("{key} expects a boolean")),
            },
            Ty::Int => {
                let Some(n) = value.as_i64().map(i128::from).or_else(|| value.as_u64().map(i128::from)) else {
                    return err(format!("{key} expects an integer"));
                };
                let below = |b: Num| match b {
                    Num::Int(b) => n < i128::from(b),
                    Num::Float(b) => (n as f64) < b,
                };
                let above = |b: Num| match b {
                    Num::Int(b) => n > i128::from(b),
                    Num::Float(b) => (n as f64) > b,
                };
                if let Some(min) = self.min_value
                    && below(min)
                {
                    return err(format!("{key} must be >= {}", min.py_str()));
                }
                if let Some(max) = self.max_value
                    && above(max)
                {
                    return err(format!("{key} must be <= {}", max.py_str()));
                }
                Ok(value.clone())
            }
            Ty::Float => {
                let Some(v) = value.as_f64() else {
                    return err(format!("{key} expects a number"));
                };
                if let Some(min) = self.min_value
                    && v < min.as_f64()
                {
                    return err(format!("{key} must be >= {}", min.py_str()));
                }
                if let Some(max) = self.max_value
                    && v > max.as_f64()
                {
                    return err(format!("{key} must be <= {}", max.py_str()));
                }
                Ok(Value::from(v))
            }
        }
    }
}

/// `InvalidSettingError` (the admin router answers 400 with this message).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct InvalidSetting(pub String);

/// `UnknownSettingError` (the admin router answers 404).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown runtime setting {0:?}")]
pub struct UnknownSetting(pub String);

static INDEX: LazyLock<HashMap<&'static str, &'static Spec>> =
    LazyLock::new(|| REGISTRY.iter().map(|s| (s.key, s)).collect());

static CACHE: LazyLock<Mutex<HashMap<&'static str, (Value, Instant)>>> = LazyLock::new(Default::default);

pub fn spec(key: &str) -> Option<&'static Spec> {
    INDEX.get(key).copied()
}

/// The effective value of a runtime setting: the stored override, else the
/// registry default (also when the stored JSON doesn't parse).
pub async fn get_setting(state: &AppState, key: &str) -> anyhow::Result<Value> {
    let spec = spec(key).ok_or_else(|| UnknownSetting(key.to_owned()))?;
    let now = Instant::now();
    let hit =
        cache().get(spec.key).filter(|(_, at)| now.duration_since(*at) < CACHE_TTL).map(|(value, _)| value.clone());
    if let Some(value) = hit {
        return Ok(value);
    }
    let value = match load_override(state, spec.key).await? {
        Some(raw) => decode_stored(&raw).unwrap_or_else(|| spec.default_value(state.settings())),
        None => spec.default_value(state.settings()),
    };
    cache().insert(spec.key, (value.clone(), now));
    Ok(value)
}

/// `(<prefix>_max, <prefix>_window_seconds)` for one rate-limit family.
pub async fn get_rate_limit(state: &AppState, prefix: &str) -> anyhow::Result<(i64, i64)> {
    let max = get_setting(state, &format!("{prefix}_max")).await?;
    let window = get_setting(state, &format!("{prefix}_window_seconds")).await?;
    Ok((as_int(&max)?, as_int(&window)?))
}

/// Drop one key from the cache (what Python does after a write or reset).
pub fn invalidate(key: &str) {
    cache().remove(key);
}

/// Python `json.loads` of a stored value; `None` where it raises.
pub fn decode_stored(raw: &str) -> Option<Value> {
    serde_json::from_str(raw).ok()
}

/// Python `json.dumps(value)` with default arguments: what a write stores.
pub fn encode_value(value: &Value) -> String {
    let mut out = String::new();
    encode_into(value, &mut out);
    out
}

fn cache() -> std::sync::MutexGuard<'static, HashMap<&'static str, (Value, Instant)>> {
    CACHE.lock().unwrap_or_else(|e| e.into_inner())
}

async fn load_override(state: &AppState, key: &str) -> anyhow::Result<Option<String>> {
    let Some(db) = state.inner.db.mongo.as_ref() else {
        return Ok(None);
    };
    let found = db
        .collection::<Document>(COLLECTION)
        .find_one(doc! {"key": key})
        .projection(doc! {"_id": 0, "value": 1})
        .await?;
    match found {
        None => Ok(None),
        Some(d) => match d.get("value") {
            Some(Bson::String(s)) => Ok(Some(s.clone())),
            _ => anyhow::bail!("runtime_config {key:?}: stored value is not a string"),
        },
    }
}

fn as_int(v: &Value) -> anyhow::Result<i64> {
    v.as_i64()
        .or_else(|| v.as_f64().map(|f| f as i64))
        .ok_or_else(|| anyhow::anyhow!("rate limit value {v} is not a number"))
}

fn encode_into(value: &Value, out: &mut String) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) if n.is_f64() => out.push_str(&py_float_repr(n.as_f64().unwrap_or(f64::NAN))),
        Value::Number(n) => out.push_str(&n.to_string()),
        Value::String(s) => encode_ascii_str(s, out),
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                encode_into(item, out);
            }
            out.push(']');
        }
        Value::Object(map) => {
            out.push('{');
            for (i, (k, v)) in map.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                encode_ascii_str(k, out);
                out.push_str(": ");
                encode_into(v, out);
            }
            out.push('}');
        }
    }
}

/// `json.dumps` string with `ensure_ascii=True`.
fn encode_ascii_str(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            ' '..='~' => out.push(c),
            _ => {
                let mut buf = [0u16; 2];
                for unit in c.encode_utf16(&mut buf) {
                    out.push_str(&format!("\\u{unit:04x}"));
                }
            }
        }
    }
    out.push('"');
}

/// Python `repr(float)` (shortest round-trip digits, exponent outside 1e-4..1e16),
/// with `json.dumps`' spellings of the non-finite values.
pub fn py_float_repr(f: f64) -> String {
    if f.is_nan() {
        return "NaN".into();
    }
    if f.is_infinite() {
        return if f > 0.0 { "Infinity".into() } else { "-Infinity".into() };
    }
    let (digits, exp) = shortest_digits(f.abs());
    let sign = if f.is_sign_negative() { "-" } else { "" };
    if !(-4..16).contains(&exp) {
        let (first, rest) = digits.split_at(1);
        let frac = if rest.is_empty() { String::new() } else { format!(".{rest}") };
        let esign = if exp < 0 { '-' } else { '+' };
        return format!("{sign}{first}{frac}e{esign}{:02}", exp.abs());
    }
    let point = exp + 1;
    let body = if point <= 0 {
        format!("0.{}{digits}", "0".repeat(point.unsigned_abs() as usize))
    } else if (point as usize) < digits.len() {
        let (int, frac) = digits.split_at(point as usize);
        format!("{int}.{frac}")
    } else {
        format!("{digits}{}.0", "0".repeat(point as usize - digits.len()))
    };
    format!("{sign}{body}")
}

fn sci_parts(s: &str) -> (String, i32) {
    let (mantissa, exp) = s.split_once('e').unwrap_or((s, "0"));
    (mantissa.chars().filter(char::is_ascii_digit).collect(), exp.parse().unwrap_or(0))
}

/// Shortest round-trip digits and decimal exponent of a finite `f >= 0`. Rust
/// rounds an exact tie between two shortest candidates up; Python's dtoa takes
/// the even one.
fn shortest_digits(f: f64) -> (String, i32) {
    let (digits, exp) = sci_parts(&format!("{f:e}"));
    if digits.bytes().last().is_some_and(|d| d % 2 == 1) {
        let (exact, exact_exp) = sci_parts(&format!("{f:.800e}"));
        let exact = exact.trim_end_matches('0');
        let n = digits.len();
        if exact_exp == exp && exact.len() == n + 1 && exact.ends_with('5') {
            let floor = &exact[..n];
            let (head, tail) = floor.split_at(1);
            if floor != digits && format!("{head}.{tail}0e{exp}").parse::<f64>() == Ok(f) {
                return (floor.to_owned(), exp);
            }
        }
    }
    (digits, exp)
}

/// Python `repr(str)`. Non-ASCII is treated as printable apart from the C1
/// controls and the usual invisible separators.
pub fn py_str_repr(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') { '"' } else { '\'' };
    let mut out = String::with_capacity(s.len() + 2);
    out.push(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            ' '..='~' => out.push(c),
            c if py_unprintable(c) => {
                let n = c as u32;
                if n <= 0xff {
                    out.push_str(&format!("\\x{n:02x}"));
                } else if n <= 0xffff {
                    out.push_str(&format!("\\u{n:04x}"));
                } else {
                    out.push_str(&format!("\\U{n:08x}"));
                }
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

fn py_unprintable(c: char) -> bool {
    matches!(c as u32,
        0..=0x1f | 0x7f..=0xa0 | 0xad | 0x1680 | 0x2000..=0x200f | 0x2028..=0x202f
        | 0x205f..=0x2064 | 0x3000 | 0xd800..=0xdfff | 0xfeff | 0xfff9..=0xfffb)
}

/// `re.fullmatch(pattern, s)` for the registry's patterns. Python's `\s` also
/// matches U+001C..U+001F, which Rust's does not.
fn py_fullmatch(pattern: &str, s: &str) -> bool {
    match regex::Regex::new(&py_pattern(pattern)) {
        Ok(re) => re.is_match(s),
        Err(e) => {
            tracing::error!("runtime_config pattern {pattern:?} does not compile: {e}");
            false
        }
    }
}

fn py_pattern(p: &str) -> String {
    let mut out = String::with_capacity(p.len() + 16);
    let mut chars = p.chars().peekable();
    let mut in_class = false;
    let mut class_open = false;
    while let Some(c) = chars.next() {
        let opening = class_open;
        class_open = false;
        match c {
            '\\' => match (chars.next(), in_class) {
                (Some('s'), false) => out.push_str(r"[\s\x1C-\x1F]"),
                (Some('S'), false) => out.push_str(r"[^\s\x1C-\x1F]"),
                (Some('s'), true) => out.push_str(r"\s\x1C-\x1F"),
                (Some('Z'), false) => out.push_str(r"\z"),
                (Some(n), _) => {
                    out.push('\\');
                    out.push(n);
                }
                (None, _) => out.push('\\'),
            },
            '[' if !in_class => {
                in_class = true;
                class_open = true;
                out.push('[');
                if chars.peek() == Some(&'^') {
                    chars.next();
                    out.push('^');
                }
            }
            ']' if in_class && !opening => {
                in_class = false;
                out.push(']');
            }
            '[' | ']' | '&' | '~' if in_class => {
                out.push('\\');
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    format!(r"\A(?:{out})\z")
}

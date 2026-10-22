//! Python semantics the embed contexts lean on, over decoded JSON values.

use anyhow::{anyhow, bail};
use serde_json::Value;

pub use crate::feeds::pyval::{get, or, py_int, py_repr, py_str, truthy, type_name};

/// `d.get(key) or fallback`.
pub fn get_or<'a>(d: &'a Value, key: &str, fallback: &'a Value) -> anyhow::Result<&'a Value> {
    Ok(or(get(d, key)?, fallback))
}

/// `float(v)`; `None` where Python raises TypeError/ValueError.
pub fn py_float(v: &Value) -> Option<f64> {
    match v {
        Value::Bool(b) => Some(f64::from(u8::from(*b))),
        Value::Number(n) => n.as_f64(),
        Value::String(s) => kiwi_core::pynum::float(s),
        _ => None,
    }
}

/// `f"{i:,}"`.
pub fn commas(i: i128) -> String {
    let digits = i.unsigned_abs().to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3 + 1);
    if i < 0 {
        out.push('-');
    }
    for (n, c) in digits.chars().enumerate() {
        if n > 0 && (digits.len() - n).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// `format(x, ".{prec}f")`.
pub fn fixed(x: f64, prec: usize) -> String {
    if x.is_nan() {
        "nan".into()
    } else if x.is_infinite() {
        if x > 0.0 { "inf".into() } else { "-inf".into() }
    } else {
        format!("{x:.prec$}")
    }
}

/// `str.strip()` on a value that must be a str.
pub fn strip_str(v: &Value) -> anyhow::Result<String> {
    match v {
        Value::String(s) => Ok(kiwi_core::pystr::strip(s).to_owned()),
        other => bail!("AttributeError: '{}' object has no attribute 'strip'", type_name(other)),
    }
}

/// Iterating a value (`for t in v`): list items, str chars, dict keys.
pub fn iter(v: &Value) -> anyhow::Result<Vec<Value>> {
    Ok(match v {
        Value::Array(a) => a.clone(),
        Value::String(s) => s.chars().map(|c| Value::String(c.to_string())).collect(),
        Value::Object(o) => o.keys().map(|k| Value::String(k.clone())).collect(),
        other => bail!("TypeError: '{}' object is not iterable", type_name(other)),
    })
}

/// `x in container` for the containers `iter` walks.
pub fn contains(container: &Value, x: &Value) -> anyhow::Result<bool> {
    Ok(match container {
        Value::Array(a) => a.iter().any(|v| py_eq(v, x)),
        Value::Object(o) => match x {
            Value::String(k) => o.contains_key(k),
            Value::Array(_) | Value::Object(_) => bail!("TypeError: unhashable type"),
            _ => false,
        },
        Value::String(s) => match x {
            Value::String(sub) => s.contains(sub.as_str()),
            other => bail!("TypeError: 'in <string>' requires string as left operand, not {}", type_name(other)),
        },
        other => bail!("TypeError: argument of type '{}' is not iterable", type_name(other)),
    })
}

/// Python `==` between decoded JSON values (`1 == 1.0 == True`).
pub fn py_eq(a: &Value, b: &Value) -> bool {
    let num = |v: &Value| match v {
        Value::Bool(b) => Some(f64::from(u8::from(*b))),
        Value::Number(n) => n.as_f64(),
        _ => None,
    };
    match (a, b) {
        (Value::Array(x), Value::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(p, q)| py_eq(p, q)),
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| py_eq(v, w)))
        }
        _ => match (num(a), num(b)) {
            (Some(p), Some(q)) => p == q,
            _ => a == b,
        },
    }
}

/// `", ".join(items)`: every item must be a str.
pub fn join(items: &[Value], sep: &str) -> anyhow::Result<String> {
    let strs: Vec<&str> = items
        .iter()
        .map(|v| v.as_str().ok_or_else(|| anyhow!("TypeError: sequence item: expected str instance")))
        .collect::<anyhow::Result<_>>()?;
    Ok(strs.join(sep))
}

/// Titlecase letters (category Lt), which count as cased.
const TITLECASE: [char; 31] = [
    '\u{1c5}', '\u{1c8}', '\u{1cb}', '\u{1f2}', '\u{1f88}', '\u{1f89}', '\u{1f8a}', '\u{1f8b}', '\u{1f8c}', '\u{1f8d}',
    '\u{1f8e}', '\u{1f8f}', '\u{1f98}', '\u{1f99}', '\u{1f9a}', '\u{1f9b}', '\u{1f9c}', '\u{1f9d}', '\u{1f9e}',
    '\u{1f9f}', '\u{1fa8}', '\u{1fa9}', '\u{1faa}', '\u{1fab}', '\u{1fac}', '\u{1fad}', '\u{1fae}', '\u{1faf}',
    '\u{1fbc}', '\u{1fcc}', '\u{1ffc}',
];

fn cased(c: char) -> bool {
    c.is_lowercase() || c.is_uppercase() || TITLECASE.contains(&c)
}

/// `str.title()` (titlecase taken as uppercase outside the Lt digraphs).
pub fn title(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_cased = false;
    for c in s.chars() {
        if prev_cased {
            out.extend(c.to_lowercase());
        } else {
            match c {
                '\u{1c4}'..='\u{1c6}' => out.push('\u{1c5}'),
                '\u{1c7}'..='\u{1c9}' => out.push('\u{1c8}'),
                '\u{1ca}'..='\u{1cc}' => out.push('\u{1cb}'),
                '\u{1f1}'..='\u{1f3}' => out.push('\u{1f2}'),
                'ß' => out.push_str("Ss"),
                _ => out.extend(c.to_uppercase()),
            }
        }
        prev_cased = cased(c);
    }
    out
}

/// `str.capitalize()`.
pub fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => {
            let mut out = title(&first.to_string());
            out.extend(chars.flat_map(char::to_lowercase));
            out
        }
    }
}

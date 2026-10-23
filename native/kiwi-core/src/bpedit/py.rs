//! The bits of Python semantics the editor's messages and JSON inputs lean on.

use serde_json::Value;

/// `f"{n:,}"`.
pub fn commas(n: i64) -> String {
    let digits = n.unsigned_abs().to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3 + 1);
    if n < 0 {
        out.push('-');
    }
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// `round(x)` on a float: half to even.
pub fn round_half_even(x: f64) -> i64 {
    let r = x.round();
    let r = if (x - x.trunc()).abs() == 0.5 && r % 2.0 != 0.0 { r - x.signum() } else { r };
    r as i64
}

/// Python floor division.
pub fn fdiv(a: i64, b: i64) -> i64 {
    let q = a / b;
    if a % b != 0 && ((a < 0) != (b < 0)) { q - 1 } else { q }
}

/// Why `int(value)` failed: `TypeError`/`ValueError` (callers catch them) or
/// `OverflowError` (nobody does).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntErr {
    Type,
    Value,
    Overflow,
}

/// `int(value)` for a `json.loads` value.
pub fn int(v: &Value) -> Result<i64, IntErr> {
    match v {
        Value::Bool(b) => Ok(i64::from(*b)),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(i)
            } else if n.as_u64().is_some() {
                Ok(i64::MAX)
            } else {
                let f = n.as_f64().unwrap_or(f64::NAN);
                if f.is_nan() {
                    Err(IntErr::Value)
                } else if f.is_infinite() {
                    Err(IntErr::Overflow)
                } else {
                    Ok(f.trunc() as i64)
                }
            }
        }
        Value::String(s) => crate::pynum::int(s).ok_or(IntErr::Value),
        _ => Err(IntErr::Type),
    }
}

/// `bool(value)`.
pub fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// `str(value)` of a `json.loads` value, as `pack_path(str(want))` sees it.
pub fn str_of(v: &Value) -> String {
    match v {
        Value::Null => "None".into(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::String(s) => s.clone(),
        Value::Number(n) => match n.as_f64() {
            Some(f) if !n.is_i64() && !n.is_u64() => format!("{f:?}"),
            _ => n.to_string(),
        },
        other => py_repr(other),
    }
}

/// `repr()` of a JSON value as a Python object (lists/dicts with `, `/`: `).
pub fn py_repr(v: &Value) -> String {
    match v {
        Value::Array(a) => format!("[{}]", a.iter().map(py_repr).collect::<Vec<_>>().join(", ")),
        Value::Object(o) => format!(
            "{{{}}}",
            o.iter().map(|(k, v)| format!("{}: {}", crate::pystr::repr(k), py_repr(v))).collect::<Vec<_>>().join(", ")
        ),
        Value::String(s) => crate::pystr::repr(s),
        other => str_of(other),
    }
}

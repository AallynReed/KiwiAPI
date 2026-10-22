//! Small Python-semantics helpers the auth code needs to match byte for byte.

use axum::http::HeaderValue;
use mongodb::bson::Bson;
use serde_json::Value;

/// `str.isspace()` for one char (Rust's `is_whitespace` lacks U+001C..U+001F).
pub fn is_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

/// `str.strip()` with no arguments.
pub fn strip(s: &str) -> &str {
    s.trim_matches(is_space)
}

/// A header value as Starlette decodes it (latin-1).
pub fn latin1(value: &HeaderValue) -> String {
    value.as_bytes().iter().map(|&b| b as char).collect()
}

/// `int(x)` for a JSON value, as PyJWT applies it to time claims. `None`
/// where Python raises ValueError/TypeError/OverflowError.
pub fn int_of(v: &Value) -> Option<i128> {
    match v {
        Value::Bool(b) => Some(i128::from(*b)),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Some(i128::from(i))
            } else if let Some(u) = n.as_u64() {
                Some(i128::from(u))
            } else {
                let f = n.as_f64()?;
                f.is_finite().then(|| f.trunc() as i128)
            }
        }
        Value::String(s) => int_of_str(s),
        _ => None,
    }
}

fn int_of_str(s: &str) -> Option<i128> {
    let s = strip(s);
    let (neg, digits) = match s.as_bytes().first()? {
        b'-' => (true, &s[1..]),
        b'+' => (false, &s[1..]),
        _ => (false, s),
    };
    if digits.is_empty() || digits.starts_with('_') || digits.ends_with('_') || digits.contains("__") {
        return None;
    }
    let mut acc: i128 = 0;
    for c in digits.chars() {
        if c == '_' {
            continue;
        }
        let d = c.to_digit(10)?;
        acc = acc.saturating_mul(10).saturating_add(i128::from(d));
    }
    Some(if neg { -acc } else { acc })
}

/// Python `==` between a JSON claim and an int (`3 == 3.0 == True`-style).
pub fn eq_int(v: Option<&Value>, n: i64) -> bool {
    match v {
        Some(Value::Bool(b)) => i64::from(*b) == n,
        Some(Value::Number(num)) => {
            if let Some(i) = num.as_i64() {
                i == n
            } else if num.is_u64() {
                false
            } else {
                num.as_f64() == Some(n as f64)
            }
        }
        _ => false,
    }
}

/// `json.dumps(v, separators=(",", ":"))` with the default `ensure_ascii=True`,
/// as PyJWT serializes the payload.
pub fn dumps_ascii(v: &Value) -> String {
    let mut out = String::new();
    write_ascii(v, &mut out);
    out
}

fn write_ascii(v: &Value, out: &mut String) {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Number(n) => out.push_str(&n.to_string()),
        Value::String(s) => write_str(s, out),
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_ascii(item, out);
            }
            out.push(']');
        }
        Value::Object(map) => {
            out.push('{');
            for (i, (k, item)) in map.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_str(k, out);
                out.push(':');
                write_ascii(item, out);
            }
            out.push('}');
        }
    }
}

fn write_str(s: &str, out: &mut String) {
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
            c if !(' '..='~').contains(&c) => {
                let mut buf = [0u16; 2];
                for unit in c.encode_utf16(&mut buf) {
                    out.push_str(&format!("\\u{unit:04x}"));
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// How pymongo encodes a Python int: int32 when it fits, else int64.
pub fn bson_int(n: i64) -> Bson {
    match i32::try_from(n) {
        Ok(v) => Bson::Int32(v),
        Err(_) => Bson::Int64(n),
    }
}

/// `round(x, 2)` (correctly rounded, ties to even on the exact binary value).
pub fn round2(x: f64) -> f64 {
    format!("{x:.2}").parse().unwrap_or(x)
}

//! The Python formatting the Discord text relies on: `str.format(**kw)` with
//! named fields, `f"{n:,}"`, `round()`, `int()` and `<t:unix:style>` tags.

use serde_json::Value;

use crate::{
    features::py_truthy,
    feeds::pyval::{py_int, py_str},
};

/// A field Python's `str.format` would raise on.
#[derive(Debug)]
pub struct FormatError;

/// `template.format(**args)` for named fields (`{name}`, `{name!s}`,
/// `{name!r}`, `{name:}`); `Err` wherever Python raises KeyError, IndexError
/// or ValueError (and for the field forms the Discord text never uses).
pub fn format(template: &str, args: &[(&str, &str)]) -> Result<String, FormatError> {
    let mut out = String::with_capacity(template.len() + 16);
    let mut it = template.chars().peekable();
    while let Some(c) = it.next() {
        match c {
            '{' if it.peek() == Some(&'{') => {
                it.next();
                out.push('{');
            }
            '{' => {
                let mut field = String::new();
                let mut depth = 1;
                loop {
                    match it.next() {
                        None => return Err(FormatError),
                        Some('{') => {
                            depth += 1;
                            field.push('{');
                        }
                        Some('}') => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                            field.push('}');
                        }
                        Some(ch) => field.push(ch),
                    }
                }
                let (head, spec) = match field.find(':') {
                    Some(i) => (&field[..i], &field[i + 1..]),
                    None => (field.as_str(), ""),
                };
                let (name, conv) = match head.find('!') {
                    Some(i) => (&head[..i], Some(&head[i + 1..])),
                    None => (head, None),
                };
                if name.is_empty() || name.chars().all(|c| c.is_ascii_digit()) || name.contains(['.', '[']) {
                    return Err(FormatError);
                }
                if !spec.is_empty() {
                    return Err(FormatError);
                }
                let val = args.iter().find(|(k, _)| *k == name).map(|(_, v)| *v).ok_or(FormatError)?;
                match conv {
                    None | Some("s") => out.push_str(val),
                    Some("r") => out.push_str(&kiwi_core::pystr::repr(val)),
                    Some(_) => return Err(FormatError),
                }
            }
            '}' if it.peek() == Some(&'}') => {
                it.next();
                out.push('}');
            }
            '}' => return Err(FormatError),
            _ => out.push(c),
        }
    }
    Ok(out)
}

/// `f"{n:,}"` for an int.
pub fn thousands(n: i128) -> String {
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

/// `isinstance(v, int)` (bool included).
pub fn as_py_int(v: &Value) -> Option<i128> {
    match v {
        Value::Bool(b) => Some(i128::from(*b)),
        Value::Number(n) if n.is_i64() || n.is_u64() => py_int(v),
        _ => None,
    }
}

/// `isinstance(v, (int, float))` and `round(v)`.
pub fn round_num(v: &Value) -> Option<i128> {
    if let Some(i) = as_py_int(v) {
        return Some(i);
    }
    match v {
        Value::Number(n) => n.as_f64().filter(|f| f.is_finite()).map(|f| f.round_ties_even() as i128),
        _ => None,
    }
}

/// `_num`: `f"{v:,}"` for an int, else an em dash.
pub fn num(v: &Value) -> String {
    as_py_int(v).map_or_else(|| "—".into(), thousands)
}

/// `_count`: `f"{round(v):,}"` for a number, else an em dash.
pub fn count(v: &Value) -> String {
    round_num(v).map_or_else(|| "—".into(), thousands)
}

/// `float(v)`; `None` where Python raises TypeError or ValueError.
pub fn py_float(v: &Value) -> Option<f64> {
    match v {
        Value::Bool(b) => Some(f64::from(u8::from(*b))),
        Value::Number(n) => n.as_f64(),
        Value::String(s) => kiwi_core::pynum::float(s),
        _ => None,
    }
}

/// `_flux`: `f"{round(float(v)):,}"`, an em dash where `float()` fails.
pub fn flux(v: &Value) -> String {
    match py_float(v) {
        Some(f) if f.is_finite() => thousands(f.round_ties_even() as i128),
        _ => "—".into(),
    }
}

/// `discord_ts(unix, style)`: `<t:{int(unix)}:{style}>`.
pub fn ts(unix: &Value, style: &str) -> anyhow::Result<String> {
    let i = py_int(unix).ok_or_else(|| anyhow::anyhow!("int() of {unix}"))?;
    Ok(format!("<t:{i}:{style}>"))
}

pub fn ts_i(unix: i64, style: &str) -> String {
    format!("<t:{unix}:{style}>")
}

/// `x or default` on a dict value.
pub fn or_else<'a>(v: &'a Value, default: &'a Value) -> &'a Value {
    if py_truthy(v) { v } else { default }
}

/// `str(v)`.
pub fn s(v: &Value) -> String {
    py_str(v)
}

//! Decoder output in Python's data model, and `json.dumps(data, indent=INDENT,
//! ensure_ascii=False)` of it, so a rebuilt file is byte-identical to Python's.

use std::fmt::Write;

use indexmap::IndexMap;

use super::value::{K, W};
use crate::{pyfloat, pyjson};

pub type Map = IndexMap<String, J>;

#[derive(Clone, Debug)]
pub enum J {
    Null,
    Bool(bool),
    Int(i128),
    Float(f64),
    Str(String),
    List(Vec<J>),
    Dict(Map),
}

/// A dict literal: `obj! {"a" => 1, "b" => "x"}`.
#[macro_export]
macro_rules! obj {
    ($($k:expr => $v:expr),* $(,)?) => {{
        #[allow(unused_mut)]
        let mut m = $crate::decode::json::Map::new();
        $(m.insert(String::from($k), $crate::decode::json::J::from($v));)*
        $crate::decode::json::J::Dict(m)
    }};
}

impl J {
    fn num(&self) -> Option<f64> {
        match self {
            J::Bool(b) => Some(f64::from(u8::from(*b))),
            J::Int(i) => Some(*i as f64),
            J::Float(f) => Some(*f),
            _ => None,
        }
    }

    fn int_like(&self) -> Option<i128> {
        match self {
            J::Bool(b) => Some(i128::from(*b)),
            J::Int(i) => Some(*i),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            J::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_int(&self) -> Option<i128> {
        match self {
            J::Int(i) => Some(*i),
            _ => None,
        }
    }

    pub fn as_list(&self) -> Option<&Vec<J>> {
        match self {
            J::List(v) => Some(v),
            _ => None,
        }
    }

    pub fn as_list_mut(&mut self) -> Option<&mut Vec<J>> {
        match self {
            J::List(v) => Some(v),
            _ => None,
        }
    }

    pub fn as_dict(&self) -> Option<&Map> {
        match self {
            J::Dict(m) => Some(m),
            _ => None,
        }
    }

    pub fn as_dict_mut(&mut self) -> Option<&mut Map> {
        match self {
            J::Dict(m) => Some(m),
            _ => None,
        }
    }

    pub fn get(&self, key: &str) -> Option<&J> {
        self.as_dict().and_then(|m| m.get(key))
    }

    /// `d.get(key, "")` of a str field.
    pub fn str_or<'a>(&'a self, key: &str, default: &'a str) -> &'a str {
        self.get(key).and_then(J::as_str).unwrap_or(default)
    }

    pub fn truthy(&self) -> bool {
        match self {
            J::Null => false,
            J::Bool(b) => *b,
            J::Int(i) => *i != 0,
            J::Float(f) => *f != 0.0,
            J::Str(s) => !s.is_empty(),
            J::List(v) => !v.is_empty(),
            J::Dict(m) => !m.is_empty(),
        }
    }

    pub fn len(&self) -> usize {
        match self {
            J::List(v) => v.len(),
            J::Dict(m) => m.len(),
            _ => 0,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Python `==`: numbers compare across bool/int/float, dicts ignore order.
impl PartialEq for J {
    fn eq(&self, other: &J) -> bool {
        match (self, other) {
            (J::Null, J::Null) => true,
            (J::Str(a), J::Str(b)) => a == b,
            (J::List(a), J::List(b)) => a == b,
            (J::Dict(a), J::Dict(b)) => a.len() == b.len() && a.iter().all(|(k, v)| b.get(k) == Some(v)),
            (a, b) => match (a.int_like(), b.int_like()) {
                (Some(x), Some(y)) => x == y,
                _ => match (a.num(), b.num()) {
                    (Some(x), Some(y)) => x == y,
                    _ => false,
                },
            },
        }
    }
}

macro_rules! from_int {
    ($($t:ty),*) => {$(
        impl From<$t> for J {
            fn from(v: $t) -> J {
                J::Int(v as i128)
            }
        }
    )*};
}
from_int!(i128, i64, i32, u32, u8, usize);

impl From<f64> for J {
    fn from(v: f64) -> J {
        J::Float(v)
    }
}

impl From<bool> for J {
    fn from(v: bool) -> J {
        J::Bool(v)
    }
}

impl From<&str> for J {
    fn from(v: &str) -> J {
        J::Str(v.to_owned())
    }
}

impl From<&String> for J {
    fn from(v: &String) -> J {
        J::Str(v.clone())
    }
}

impl From<String> for J {
    fn from(v: String) -> J {
        J::Str(v)
    }
}

impl From<std::sync::Arc<str>> for J {
    fn from(v: std::sync::Arc<str>) -> J {
        J::Str(v.to_string())
    }
}

impl From<Vec<J>> for J {
    fn from(v: Vec<J>) -> J {
        J::List(v)
    }
}

impl From<Vec<String>> for J {
    fn from(v: Vec<String>) -> J {
        J::List(v.into_iter().map(J::Str).collect())
    }
}

impl From<Map> for J {
    fn from(v: Map) -> J {
        J::Dict(v)
    }
}

impl<T: Into<J>> From<Option<T>> for J {
    fn from(v: Option<T>) -> J {
        v.map_or(J::Null, Into::into)
    }
}

impl From<&W> for J {
    /// A parsed wire value as `json.dumps` sees it (tuples and lists alike, int
    /// dict keys as their decimal text).
    fn from(v: &W) -> J {
        match v {
            W::None => J::Null,
            W::Int(i) => J::Int(*i),
            W::Float(f) => J::Float(*f),
            W::Str(s) => J::Str(s.to_string()),
            W::Bytes(_) => J::Null,
            W::Tuple(x) | W::Obj(x) | W::Rep(x) | W::List(x) => J::List(x.iter().map(J::from).collect()),
            W::Dict(d) => J::Dict(
                d.items()
                    .iter()
                    .map(|(k, v)| {
                        let key = match k {
                            K::Int(i) => i.to_string(),
                            K::Str(s) => s.to_string(),
                        };
                        (key, J::from(v))
                    })
                    .collect(),
            ),
        }
    }
}

impl From<W> for J {
    fn from(v: W) -> J {
        J::from(&v)
    }
}

/// `float.__repr__` the way the json encoder writes it.
pub fn float_text(f: f64) -> String {
    if f.is_nan() {
        "NaN".into()
    } else if f.is_infinite() {
        if f > 0.0 { "Infinity" } else { "-Infinity" }.into()
    } else {
        pyfloat::repr(f)
    }
}

/// `json.dumps(value, indent=indent, ensure_ascii=False)`.
pub fn dumps(value: &J, indent: Option<usize>) -> String {
    let mut out = String::with_capacity(1 << 16);
    write(&mut out, value, indent, 0);
    out
}

fn newline(out: &mut String, indent: usize, level: usize) {
    out.push('\n');
    for _ in 0..indent * level {
        out.push(' ');
    }
}

fn write(out: &mut String, value: &J, indent: Option<usize>, level: usize) {
    match value {
        J::Null => out.push_str("null"),
        J::Bool(true) => out.push_str("true"),
        J::Bool(false) => out.push_str("false"),
        J::Int(i) => {
            let _ = write!(out, "{i}");
        }
        J::Float(f) => out.push_str(&float_text(*f)),
        J::Str(s) => pyjson::write_str(out, s, false),
        J::List(items) => {
            if items.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(if indent.is_some() { "," } else { ", " });
                }
                if let Some(n) = indent {
                    newline(out, n, level + 1);
                }
                write(out, item, indent, level + 1);
            }
            if let Some(n) = indent {
                newline(out, n, level);
            }
            out.push(']');
        }
        J::Dict(map) => {
            if map.is_empty() {
                out.push_str("{}");
                return;
            }
            out.push('{');
            for (i, (k, v)) in map.iter().enumerate() {
                if i > 0 {
                    out.push_str(if indent.is_some() { "," } else { ", " });
                }
                if let Some(n) = indent {
                    newline(out, n, level + 1);
                }
                pyjson::write_str(out, k, false);
                out.push_str(": ");
                write(out, v, indent, level + 1);
            }
            if let Some(n) = indent {
                newline(out, n, level);
            }
            out.push('}');
        }
    }
}

/// `json.loads` of a file into this model (a baseline the decoders build on).
pub fn from_serde(v: &serde_json::Value) -> J {
    match v {
        serde_json::Value::Null => J::Null,
        serde_json::Value::Bool(b) => J::Bool(*b),
        serde_json::Value::Number(n) => match (n.as_i64(), n.as_u64()) {
            (Some(i), _) => J::Int(i128::from(i)),
            (None, Some(u)) => J::Int(i128::from(u)),
            _ => J::Float(n.as_f64().unwrap_or(f64::NAN)),
        },
        serde_json::Value::String(s) => J::Str(s.clone()),
        serde_json::Value::Array(a) => J::List(a.iter().map(from_serde).collect()),
        serde_json::Value::Object(m) => J::Dict(m.iter().map(|(k, v)| (k.clone(), from_serde(v))).collect()),
    }
}

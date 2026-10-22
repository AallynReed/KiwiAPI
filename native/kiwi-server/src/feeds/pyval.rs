//! Python semantics over upstream JSON (`dict.get`, truthiness, `str()`,
//! `int()`) and pymongo's encoding of the values the relays store.

use anyhow::{anyhow, bail};
use mongodb::bson::{Bson, Document};
use serde_json::{Map, Value};

use crate::{pyfloat, validation::py_repr_str};

/// `bool(v)`.
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

/// `d.get(key)` on something that must be a dict (AttributeError otherwise).
pub fn get<'a>(d: &'a Value, key: &str) -> anyhow::Result<Option<&'a Value>> {
    match d {
        Value::Object(m) => Ok(m.get(key)),
        other => Err(anyhow!("AttributeError: {} has no attribute 'get'", type_name(other))),
    }
}

/// `a or b`.
pub fn or<'a>(a: Option<&'a Value>, b: &'a Value) -> &'a Value {
    a.filter(|v| truthy(v)).unwrap_or(b)
}

pub static NONE: Value = Value::Null;
pub static EMPTY_STR: Value = Value::String(String::new());

pub fn empty_dict() -> &'static Value {
    static EMPTY: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
    EMPTY.get_or_init(|| Value::Object(Map::new()))
}

pub fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Number(n) if n.is_f64() => "float",
        Value::Number(_) => "int",
        Value::String(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}

/// `str(v)`.
pub fn py_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => py_repr(other),
    }
}

/// `repr(v)`.
pub fn py_repr(v: &Value) -> String {
    match v {
        Value::Null => "None".into(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::Number(n) => match (n.as_i64(), n.as_u64()) {
            (Some(i), _) => i.to_string(),
            (_, Some(u)) => u.to_string(),
            _ => pyfloat::repr(n.as_f64().unwrap_or(f64::NAN)),
        },
        Value::String(s) => py_repr_str(s),
        Value::Array(a) => format!("[{}]", a.iter().map(py_repr).collect::<Vec<_>>().join(", ")),
        Value::Object(o) => {
            let items: Vec<String> = o.iter().map(|(k, v)| format!("{}: {}", py_repr_str(k), py_repr(v))).collect();
            format!("{{{}}}", items.join(", "))
        }
    }
}

/// `int(v)`; `None` where Python raises.
pub fn py_int(v: &Value) -> Option<i128> {
    match v {
        Value::Bool(b) => Some(i128::from(*b)),
        Value::Number(n) => match (n.as_i64(), n.as_u64(), n.as_f64()) {
            (Some(i), _, _) => Some(i128::from(i)),
            (_, Some(u), _) => Some(i128::from(u)),
            (_, _, Some(f)) if f.is_finite() => Some(f.trunc() as i128),
            _ => None,
        },
        Value::String(s) => kiwi_core::feeds::py::int(s, 10),
        _ => None,
    }
}

/// A Python int as pymongo encodes it (int32 when it fits).
pub fn int_bson(i: i128) -> anyhow::Result<Bson> {
    if let Ok(v) = i32::try_from(i) {
        Ok(Bson::Int32(v))
    } else if let Ok(v) = i64::try_from(i) {
        Ok(Bson::Int64(v))
    } else {
        bail!("OverflowError: MongoDB can only handle up to 8-byte ints")
    }
}

/// A JSON value as pymongo encodes the Python object it decodes to.
pub fn to_bson(v: &Value) -> anyhow::Result<Bson> {
    Ok(match v {
        Value::Null => Bson::Null,
        Value::Bool(b) => Bson::Boolean(*b),
        Value::Number(n) => match (n.as_i64(), n.as_u64(), n.as_f64()) {
            (Some(i), _, _) => int_bson(i.into())?,
            (_, Some(u), _) => int_bson(u.into())?,
            (_, _, f) => Bson::Double(f.unwrap_or(f64::NAN)),
        },
        Value::String(s) => Bson::String(s.clone()),
        Value::Array(a) => Bson::Array(a.iter().map(to_bson).collect::<anyhow::Result<_>>()?),
        Value::Object(o) => {
            let mut d = Document::new();
            for (k, v) in o {
                d.insert(k.clone(), to_bson(v)?);
            }
            Bson::Document(d)
        }
    })
}

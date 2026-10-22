//! `CheatersResponse(**payload)` as FastAPI dumps it: model field order and
//! defaults, undeclared keys dropped, lax int/float coercion (scores and
//! ranges become floats). A payload the model rejects is Python's 500.

use serde_json::{Map, Value};

use kiwi_core::cheaters::fv;

type R<T> = Result<T, String>;

enum Ty {
    Int,
    Float,
    Str,
    OptInt,
    OptFloat,
    OptStr,
    Dict,
    StrList,
    Literal(&'static [&'static str]),
    Model(&'static [Field]),
    Models(&'static [Field]),
}

enum Def {
    Required,
    Null,
    ZeroInt,
    ZeroFloat,
    NameStem,
    EmptyList,
}

struct Field(&'static str, Ty, Def);

use Def::*;
use Ty::*;

const EVIDENCE: &[Field] = &[
    Field("type", Literal(&["score_outlier", "rank_gap", "velocity_outlier", "sustained_velocity"]), Required),
    Field("summary", Str, Required),
    Field("measurements", Dict, Required),
    Field("confidence", Float, Required),
];

const BOARD_ENTRY: &[Field] = &[
    Field("uuid", Int, Required),
    Field("name", Str, Required),
    Field("category", Str, Required),
    Field("contest_type", OptStr, Required),
    Field("rank", Int, Required),
    Field("score", Float, Required),
    Field("evidence", Models(EVIDENCE), Required),
    Field("confidence", Float, ZeroFloat),
];

const PLAYER: &[Field] = &[
    Field("player_name", Str, Required),
    Field("leaderboards", Models(BOARD_ENTRY), Required),
    Field("confidence", Float, Required),
];

const CLUSTER_BOARD: &[Field] = &[
    Field("uuid", Int, Required),
    Field("name", Str, Required),
    Field("category", Str, Required),
    Field("contest_type", OptStr, Null),
    Field("members", Int, Required),
    Field("member_names", StrList, EmptyList),
    Field("score_min", OptFloat, Null),
    Field("score_max", OptFloat, Null),
    Field("spread", OptFloat, Null),
    Field("rank_min", OptInt, Null),
    Field("rank_max", OptInt, Null),
    Field("matching_hours", OptInt, Null),
    Field("avg_hourly_gain", OptFloat, Null),
];

const CLUSTER: &[Field] = &[
    Field("stem", Str, Required),
    Field("label", Str, Required),
    Field("method", Str, NameStem),
    Field("corroborated_by", StrList, EmptyList),
    Field("member_count", Int, Required),
    Field("members", StrList, Required),
    Field("members_truncated", Int, ZeroInt),
    Field("board_count", Int, Required),
    Field("boards", Models(CLUSTER_BOARD), Required),
    Field("confidence", Float, Required),
    Field("summary", Str, Required),
    Field("measurements", Dict, Required),
];

const CONFIG: &[Field] = &[
    Field("z_threshold", Float, Required),
    Field("velocity_multiplier", Float, Required),
    Field("min_board_size", Int, Required),
];

const ANALYZED: &[Field] = &[
    Field("uuid", Int, Required),
    Field("name", Str, Required),
    Field("category", Str, Required),
    Field("reset_kind", Str, Required),
    Field("contest_type", OptStr, Required),
    Field("entries", Int, Required),
];

const SKIPPED: &[Field] = &[
    Field("uuid", Int, Required),
    Field("name", Str, Required),
    Field("category", Str, Required),
    Field("reset_kind", Str, Required),
    Field("contest_type", OptStr, Required),
    Field("reason", Literal(&["admin_excluded", "below_min_size"]), Required),
    Field("entries", OptInt, Null),
];

const RESPONSE: &[Field] = &[
    Field("players", Models(PLAYER), Required),
    Field("clusters", Models(CLUSTER), EmptyList),
    Field("clusters_boards_scanned", Int, ZeroInt),
    Field("computed_at", Int, Required),
    Field("anchor", OptInt, Required),
    Field("method", Str, Required),
    Field("config", Model(CONFIG), Required),
    Field("total_flagged", Int, Required),
    Field("boards_analyzed", Int, Required),
    Field("boards_excluded", Int, ZeroInt),
    Field("analyzed_boards", Models(ANALYZED), EmptyList),
    Field("excluded_boards", Models(SKIPPED), EmptyList),
];

fn int(v: &Value) -> R<Value> {
    match v {
        Value::Number(n) if n.is_i64() || n.is_u64() => Ok(v.clone()),
        Value::Number(n) => match n.as_f64() {
            Some(f) if f.is_finite() && f == f.trunc() && f.abs() < 9.2e18 => Ok(Value::from(f as i64)),
            _ => Err(format!("not an integer: {v}")),
        },
        Value::Bool(b) => Ok(Value::from(i64::from(*b))),
        _ => Err(format!("not an integer: {v}")),
    }
}

fn float(v: &Value) -> R<Value> {
    match v {
        Value::Number(n) => Ok(fv(n.as_f64().unwrap_or(f64::NAN))),
        Value::Bool(b) => Ok(fv(f64::from(u8::from(*b)))),
        _ => Err(format!("not a number: {v}")),
    }
}

fn coerce(ty: &Ty, v: &Value) -> R<Value> {
    match (ty, v) {
        (OptInt | OptFloat | OptStr, Value::Null) => Ok(Value::Null),
        (Int | OptInt, _) => int(v),
        (Float | OptFloat, _) => float(v),
        (Str | OptStr, Value::String(_)) => Ok(v.clone()),
        (Dict, Value::Object(_)) => Ok(v.clone()),
        (StrList, Value::Array(items)) if items.iter().all(Value::is_string) => Ok(v.clone()),
        (Literal(allowed), Value::String(s)) if allowed.contains(&s.as_str()) => Ok(v.clone()),
        (Model(fields), Value::Object(m)) => model(fields, m),
        (Models(fields), Value::Array(items)) => items
            .iter()
            .map(|i| i.as_object().ok_or_else(|| format!("not an object: {i}")).and_then(|m| model(fields, m)))
            .collect::<R<Vec<_>>>()
            .map(Value::Array),
        _ => Err(format!("unexpected value {v}")),
    }
}

fn model(fields: &[Field], m: &Map<String, Value>) -> R<Value> {
    let mut out = Map::new();
    for Field(name, ty, default) in fields {
        let v = match (m.get(*name), default) {
            (Some(v), _) => coerce(ty, v).map_err(|e| format!("{name}: {e}"))?,
            (None, Required) => return Err(format!("{name}: missing")),
            (None, Null) => Value::Null,
            (None, ZeroInt) => Value::from(0),
            (None, ZeroFloat) => fv(0.0),
            (None, NameStem) => Value::from("name_stem"),
            (None, EmptyList) => Value::Array(Vec::new()),
        };
        out.insert((*name).to_string(), v);
    }
    Ok(Value::Object(out))
}

/// The `/v1/leaderboards/cheaters` body for a detection payload.
pub fn v1_body(payload: &Value) -> R<Value> {
    let m = payload.as_object().ok_or("payload is not an object")?;
    model(RESPONSE, m)
}

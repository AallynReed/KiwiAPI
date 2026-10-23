//! `app/trove/decode/view.py`: one `.binfab` laid out for reading (the Mod
//! Workshop's binfab preview). Every field by index, typed, labelled only where
//! `fields.py` has evidence; a language table also as key/text rows.
//!
//! An upload is read under `UPLOAD_BUDGET` steps; past it, under the budget
//! Python's own fallback reader starts from, which stands in for its 30 s
//! child-process limit.

use serde_json::{Map, Value, json};

use super::{
    ability::{is_damage, is_healing},
    common::{
        ACTION_SECTION, DAMAGE, EFFECT_DESCRIPTION, EFFECT_DURATION, EFFECT_NAME, HEALING, IDENTITY, MOD_OPS,
        PROC_COOLDOWN, PROC_SECTION, PROC_SPAWNER, action_spec, is_effect, is_modifier, stat_key,
    },
    json::float_text,
    value::{Dict, K, Prefab, W, parse_budget},
};

pub const MAX_BYTES: usize = 1024 * 1024;
pub const MAX_NODES: usize = 60_000;
pub const UPLOAD_BUDGET: u64 = 3_000_000;
/// `wire.NATIVE_BUDGET`: what the child process's reader gets before Python's own.
pub const FALLBACK_BUDGET: u64 = 200_000_000;

pub const EMPTY: &str = "That file is empty.";
pub const TOO_LARGE: &str = "This file is too large to lay out.";
pub const UNREADABLE: &str = "This file couldn't be read as a Trove prefab.";
pub const TOO_SLOW: &str = "This file took too long to lay out.";

struct Budget {
    left: usize,
    truncated: bool,
}

impl Budget {
    fn spend(&mut self) -> bool {
        if self.left == 0 {
            self.truncated = true;
            return false;
        }
        self.left -= 1;
        true
    }
}

type Labels = Vec<(i128, &'static str)>;

fn component_name(cid: i128) -> Option<&'static str> {
    if cid == IDENTITY {
        return Some("Identity");
    }
    if cid == PROC_SPAWNER {
        return Some("Combat-event effect spawner");
    }
    if action_spec(cid).is_some() {
        return Some("Ability action");
    }
    is_effect(cid).then_some("Effect")
}

fn put(labels: &mut Labels, idx: i128, label: &'static str) {
    match labels.iter_mut().find(|(i, _)| *i == idx) {
        Some(slot) => slot.1 = label,
        None => labels.push((idx, label)),
    }
}

fn section_of(out: &mut Vec<(usize, Labels)>, si: usize) -> &mut Labels {
    if let Some(i) = out.iter().position(|(s, _)| *s == si) {
        return &mut out[i].1;
    }
    out.push((si, Vec::new()));
    &mut out.last_mut().expect("pushed").1
}

fn component_labels(cid: i128, n_sections: usize) -> Vec<(usize, Labels)> {
    let mut out: Vec<(usize, Labels)> = Vec::new();
    if cid == IDENTITY {
        let leaf = n_sections.wrapping_sub(1);
        *section_of(&mut out, leaf) = vec![(1, "name"), (2, "category"), (5, "description"), (6, "icon")];
    }
    if let Some((energy, cooldown)) = action_spec(cid) {
        let labels = section_of(&mut out, ACTION_SECTION);
        if let Some(e) = energy {
            put(labels, e, "energy cost");
        }
        if let Some(c) = cooldown {
            put(labels, c, "cooldown");
        }
    }
    if is_effect(cid) {
        let labels = section_of(&mut out, 0);
        put(labels, EFFECT_DURATION, "duration (s)");
        put(labels, EFFECT_DESCRIPTION, "description");
        put(labels, EFFECT_NAME, "name");
    }
    if cid == PROC_SPAWNER {
        put(section_of(&mut out, PROC_SECTION), PROC_COOLDOWN, "proc cooldown");
    }
    out
}

/// Python `str()` of a scalar.
fn py_str(v: Option<&W>) -> String {
    match v {
        None | Some(W::None) => "None".into(),
        Some(W::Int(i)) => i.to_string(),
        Some(W::Float(f)) => crate::pyfloat::repr(*f),
        Some(W::Str(s)) => s.to_string(),
        Some(other) => format!("{other:?}"),
    }
}

fn section_labels(section: &W) -> (Labels, Option<String>) {
    if is_damage(section) {
        return (DAMAGE.to_vec(), Some("Damage parameters".into()));
    }
    if is_healing(section) {
        return (HEALING.to_vec(), Some("Healing parameters".into()));
    }
    if is_modifier(section) {
        let d = section.as_dict().expect("dict");
        let op = d.int(1).unwrap_or(0);
        let key = stat_key(d.int(0).unwrap_or(0));
        let stat = key.strip_prefix("$Stat_").unwrap_or(&key);
        let verb = if (0..7).contains(&op) { MOD_OPS[op as usize].to_owned() } else { format!("op {op}") };
        let labels = vec![(0, "stat"), (1, "operation"), (2, "value"), (3, "label"), (5, "flags")];
        return (labels, Some(format!("Stat modifier: {stat} {verb} {}", py_str(d.get(2)))));
    }
    (Vec::new(), None)
}

fn int_value(i: i128) -> Value {
    if i.unsigned_abs() < (1u128 << 53) { json!(i as i64) } else { Value::String(i.to_string()) }
}

fn scalar_value(v: &W) -> Value {
    match v {
        W::Int(i) => int_value(*i),
        W::Float(f) if f.is_finite() => json!(f),
        W::Float(f) => Value::String(match float_text(*f).as_str() {
            "NaN" => "nan".into(),
            "Infinity" => "inf".into(),
            _ => "-inf".into(),
        }),
        W::Str(s) => Value::String(s.to_string()),
        _ => Value::Null,
    }
}

fn scalar(v: &W) -> Value {
    match v {
        W::Int(_) => json!({"t": "i", "v": scalar_value(v)}),
        W::Float(_) => json!({"t": "f", "v": scalar_value(v)}),
        W::Str(s) => json!({"t": "s", "v": s.to_string()}),
        W::Bytes(b) => json!({"t": "b", "n": b.len(), "v": hex::encode(&b[..b.len().min(48)])}),
        _ => json!({"t": "s", "v": format!("{v:?}")}),
    }
}

fn node(v: &W, budget: &mut Budget, labels: &[(usize, Labels)]) -> Value {
    if !budget.spend() {
        return Value::Null;
    }
    match v {
        W::None => json!({"t": "z"}),
        W::Obj(sections) => {
            let mut out = Vec::new();
            for (si, s) in sections.iter().enumerate() {
                let (mut named, note) = section_labels(s);
                if let Some((_, extra)) = labels.iter().find(|(i, _)| *i == si) {
                    for (i, l) in extra {
                        put(&mut named, *i, l);
                    }
                }
                out.push(section(s.as_dict().unwrap_or(&super::value::EMPTY), budget, &named, note));
            }
            json!({"t": "o", "v": out})
        }
        W::Rep(xs) => json!({"t": "r", "v": xs.iter().map(|x| node(x, budget, &[])).collect::<Vec<_>>()}),
        W::Tuple(xs) => json!({"t": "p", "v": xs.iter().map(scalar_value).collect::<Vec<_>>()}),
        W::List(xs) => json!({"t": "a", "v": xs.iter().map(|x| node(x, budget, &[])).collect::<Vec<_>>()}),
        W::Dict(d) => {
            let rows: Vec<Value> = d
                .items()
                .iter()
                .map(|(k, x)| {
                    let key = match k {
                        K::Str(s) => Value::String(s.to_string()),
                        K::Int(i) => int_value(*i),
                    };
                    json!([key, node(x, budget, &[])])
                })
                .collect();
            json!({"t": "m", "v": rows})
        }
        _ => scalar(v),
    }
}

fn section(s: &Dict, budget: &mut Budget, labels: &Labels, note: Option<String>) -> Value {
    let mut items: Vec<(i128, &W)> = s
        .items()
        .iter()
        .filter_map(|(k, v)| match k {
            K::Int(i) => Some((*i, v)),
            K::Str(_) => None,
        })
        .collect();
    items.sort_by_key(|(i, _)| *i);
    let fields: Vec<Value> = items
        .into_iter()
        .map(|(idx, value)| {
            let mut f = Map::new();
            f.insert("i".into(), int_value(idx));
            if let Some((_, l)) = labels.iter().find(|(i, _)| *i == idx) {
                f.insert("l".into(), Value::String((*l).to_owned()));
            }
            f.insert("v".into(), node(value, budget, &[]));
            Value::Object(f)
        })
        .collect();
    let mut out = Map::new();
    out.insert("f".into(), Value::Array(fields));
    if let Some(n) = note {
        out.insert("note".into(), Value::String(n));
    }
    Value::Object(out)
}

fn locale_rows(root: Option<&W>) -> Option<Vec<Value>> {
    let rows = root?.leaf().get(0)?;
    if !rows.is_list() || rows.is_empty() {
        return None;
    }
    let mut out = Vec::new();
    for row in rows.elems() {
        let leaf = row.leaf();
        out.push(json!([leaf.str(0)?, leaf.str(1)?]));
    }
    Some(out)
}

fn describe_prefab(pf: &Prefab, size: usize) -> Value {
    let mut budget = Budget { left: MAX_NODES, truncated: false };
    let mut out = Map::new();
    out.insert("kind".into(), json!(if pf.entity { "entity" } else { "object" }));
    out.insert("size".into(), json!(size));
    if pf.entity {
        out.insert("type_id".into(), pf.type_id.map_or(Value::Null, int_value));
        let comps: Vec<Value> = pf
            .components
            .iter()
            .map(|(cid, obj)| {
                let mut c = Map::new();
                c.insert("id".into(), int_value(*cid));
                if let Some(n) = component_name(*cid) {
                    c.insert("name".into(), json!(n));
                }
                let labels = component_labels(*cid, obj.sections().len());
                c.insert("v".into(), node(obj, &mut budget, &labels));
                Value::Object(c)
            })
            .collect();
        out.insert("components".into(), Value::Array(comps));
    } else if let Some(rows) = locale_rows(pf.root.as_ref()) {
        out.insert("kind".into(), json!("locale"));
        budget.truncated = rows.len() > MAX_NODES;
        out.insert("rows".into(), Value::Array(rows.into_iter().take(MAX_NODES).collect()));
    } else {
        let root = pf.root.as_ref().map_or(Value::Null, |r| node(r, &mut budget, &[]));
        out.insert("root".into(), root);
    }
    out.insert("truncated".into(), json!(budget.truncated));
    Value::Object(out)
}

/// `describe_isolated`: the upload laid out, or the user-facing reason it can't be.
pub fn describe_upload(data: &[u8]) -> Result<Value, &'static str> {
    if data.len() > MAX_BYTES {
        return Err(TOO_LARGE);
    }
    if data.is_empty() {
        return Err(EMPTY);
    }
    let parsed = match parse_budget(data, UPLOAD_BUDGET) {
        Some(p) => p,
        None => parse_budget(data, FALLBACK_BUDGET).ok_or(TOO_SLOW)?,
    };
    let pf = parsed.map_err(|_| UNREADABLE)?;
    Ok(describe_prefab(&pf, data.len()))
}

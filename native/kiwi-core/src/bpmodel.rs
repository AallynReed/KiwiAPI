//! Blueprint Editor model projects (`app/trove/blueprint/model.py`): every
//! part of a creature opened at once, placed from the binding map, and the
//! edits written back.

use std::collections::HashMap;

use serde_json::{Map, Value, json};

use crate::bpedit::py::{commas, int, str_of, truthy};
use crate::bpedit::{Fault, Res, codec, editor, transform};
use crate::rig::{Rig, scale_for};

pub const MAX_PARTS: usize = 48;
pub const PROJECT_VOXEL_CAP: i64 = 300_000;
const BLUEPRINT_DIR: &str = "blueprints/";

pub type Files = Vec<(String, Vec<u8>)>;

fn editor_err<T>(msg: impl Into<String>) -> Res<T> {
    Err(Fault::Editor(msg.into()))
}

/// `parts_of(files)`: the `.blueprint` files, in path order.
pub fn parts_of(files: &[(String, Vec<u8>)]) -> Vec<(String, Vec<u8>)> {
    let mut out: Vec<(String, Vec<u8>)> =
        files.iter().filter(|(p, _)| p.to_lowercase().ends_with(".blueprint")).cloned().collect();
    out.sort_by_cached_key(|(p, _)| p.to_lowercase());
    out
}

/// `basename_of(path)`.
pub fn basename_of(path: &str) -> String {
    let p = path.replace('\\', "/");
    let name: Vec<char> = p.rsplit('/').next().unwrap_or_default().chars().collect();
    name[..name.len().saturating_sub(10)].iter().collect::<String>().to_lowercase()
}

/// `rig_payload(rig_name, head_scale)`.
pub fn rig_payload(rig_name: &str, rig: &Rig, head_scale: f64) -> Value {
    let mut scales = Map::new();
    if let Some(Value::Object(rest)) = rig.raw.get("rest") {
        for ap in rest.keys() {
            scales.insert(ap.clone(), json!(scale_for(ap, Some(rig), head_scale, 1.0)));
        }
    }
    json!({
        "name": rig_name,
        "voxel_scale": rig.voxel_scale_value(),
        "rest": rig.rest_value(),
        "scales": scales,
        "animations": rig.animation_names(),
    })
}

/// `open_project(files, rig_name=, attach=, name=, head_scale=, part_scales=)`;
/// `rig` is the baked rig of `rig_name`, if both exist.
pub fn open_project(
    files: &[(String, Vec<u8>)],
    rig: Option<(&str, &Rig)>,
    attach: &[(String, String)],
    name: &str,
    head_scale: f64,
    part_scales: &HashMap<String, f64>,
) -> Res<Value> {
    let blueprints = parts_of(files);
    if blueprints.is_empty() {
        return editor_err(
            "There are no .blueprint files in there. A model project is a mod's voxel parts - open a .tmod, a .zip or the blueprints themselves.",
        );
    }
    if blueprints.len() > MAX_PARTS {
        return editor_err(format!(
            "That's {} models in one file, past the {MAX_PARTS} a project holds. Open the parts you want to work on instead.",
            commas(blueprints.len() as i64)
        ));
    }
    let mut parts = Vec::new();
    let mut skipped = Vec::new();
    let mut total = 0i64;
    for (path, raw) in &blueprints {
        let fname = path.rsplit('/').next().unwrap_or(path);
        let payload = match editor::inspect(raw, fname) {
            Ok(p) => p,
            Err(Fault::Editor(m)) => {
                skipped.push(json!({"path": path, "reason": m}));
                continue;
            }
            Err(other) => return Err(other),
        };
        total += payload.get("count").and_then(Value::as_i64).unwrap_or(0);
        if total > PROJECT_VOXEL_CAP {
            return editor_err(format!(
                "This model is past the editor's {} voxel limit for a whole project. Open its parts one at a time instead.",
                commas(PROJECT_VOXEL_CAP)
            ));
        }
        let base = basename_of(path);
        let mut ap = attach.iter().find(|(b, _)| *b == base).map(|(_, a)| a.clone());
        if let Some((_, r)) = rig
            && !ap.as_deref().is_some_and(|a| r.has_ap(a))
        {
            ap = None;
        }
        let scale = match (part_scales.get(&base), ap.as_deref().filter(|a| !a.is_empty())) {
            (_, None) => 1.0,
            (Some(s), Some(_)) => *s,
            (None, Some(a)) => scale_for(a, rig.map(|(_, r)| r), head_scale, 1.0),
        };
        parts.push(json!({
            "path": path,
            "name": payload.get("name").cloned().unwrap_or(Value::Null),
            "ap": ap,
            "scale": scale,
            "blueprint": crate::tmod::base64(raw),
            "model": payload,
        }));
    }
    if parts.is_empty() {
        return editor_err(if skipped.is_empty() {
            "There's nothing to edit in there."
        } else {
            "None of the blueprints in there could be opened."
        });
    }
    Ok(json!({
        "name": name,
        "rig": rig.map(|(n, r)| rig_payload(n, r, head_scale)),
        "parts": parts,
        "skipped": skipped,
        "extras": files.len() - blueprints.len(),
    }))
}

/// Python iteration of a `json.loads` value, `None` where it raises `TypeError`.
fn py_iter(v: &Value) -> Option<Vec<Value>> {
    match v {
        Value::Array(a) => Some(a.clone()),
        Value::Object(o) => Some(o.keys().map(|k| Value::String(k.clone())).collect()),
        Value::String(s) => Some(s.chars().map(|c| Value::String(c.to_string())).collect()),
        _ => None,
    }
}

fn is_zero(v: &Value) -> bool {
    match v {
        Value::Bool(b) => !*b,
        Value::Number(n) => n.as_f64() == Some(0.0),
        _ => false,
    }
}

/// `_delta(value)`.
fn delta(value: &Value) -> Res<[i64; 3]> {
    let bad = || Fault::Editor("A part's move wasn't understood.".into());
    let items = py_iter(value).ok_or_else(bad)?;
    if items.len() != 3 {
        return Err(bad());
    }
    let mut out = [0i64; 3];
    for (slot, v) in out.iter_mut().zip(&items) {
        *slot = int(v).map_err(|_| bad())?;
    }
    Ok(out)
}

/// `_moved(raw, delta)`.
fn moved(raw: &[u8], d: &Value) -> Res<Vec<u8>> {
    let catch = [Fault::is_transform as fn(&Fault) -> bool, Fault::is_blueprint];
    let doc = codec::decode(raw).map_err(|f| f.wrap(&catch))?;
    let d = delta(d)?;
    let doc = transform::move_on_rig(doc, d).map_err(|f| f.wrap(&catch))?;
    codec::encode_doc(&doc).map_err(|f| f.wrap(&catch))
}

pub const SUMMARY_KEYS: [&str; 8] =
    ["parts", "added_parts", "moved", "recoloured", "rematerialised", "added", "erased", "ignored"];

/// `apply_project(files, edits, extra, moves)`.
pub fn apply_project(
    files: &[(String, Vec<u8>)],
    edits: &Map<String, Value>,
    extra: Vec<(String, Vec<u8>)>,
    moves: &Map<String, Value>,
) -> Res<(Files, Map<String, Value>)> {
    let mut incoming: Vec<(String, Vec<u8>)> = Vec::new();
    for (p, d) in extra {
        match incoming.iter_mut().find(|(k, _)| *k == p) {
            Some(slot) => slot.1 = d,
            None => incoming.push((p, d)),
        }
    }
    let mut merged: Vec<(String, Vec<u8>)> = Vec::with_capacity(files.len() + incoming.len());
    for (p, d) in files {
        match incoming.iter().position(|(k, _)| k == p) {
            Some(i) => merged.push((p.clone(), incoming.remove(i).1)),
            None => merged.push((p.clone(), d.clone())),
        }
    }
    let added = incoming.len();
    merged.extend(incoming);

    let mut kept_moves: Vec<(&String, &Value)> = Vec::new();
    for (k, v) in moves {
        let items = if truthy(v) {
            py_iter(v).ok_or_else(|| Fault::Unhandled("TypeError: not iterable".into()))?
        } else {
            Vec::new()
        };
        if !(items.len() == 3 && items.iter().all(is_zero)) {
            kept_moves.push((k, v));
        }
    }
    let known: std::collections::HashSet<&str> = merged.iter().map(|(p, _)| p.as_str()).collect();
    if let Some(p) = edits.keys().chain(kept_moves.iter().map(|(k, _)| *k)).find(|p| !known.contains(p.as_str())) {
        return editor_err(format!("'{p}' isn't a file in that mod - reopen the model and try again."));
    }
    if parts_of(&merged).len() > MAX_PARTS {
        return editor_err(format!("That's more than the {MAX_PARTS} parts a project holds."));
    }
    let mut summary: Map<String, Value> = SUMMARY_KEYS.iter().map(|k| ((*k).to_owned(), json!(0))).collect();
    summary.insert("added_parts".into(), json!(added));
    summary.insert("moved".into(), json!(kept_moves.len()));
    let bump = |s: &mut Map<String, Value>, k: &str, n: i64| {
        let cur = s.get(k).and_then(Value::as_i64).unwrap_or(0);
        s.insert(k.to_owned(), json!(cur + n));
    };
    let mut out = Vec::with_capacity(merged.len());
    for (path, raw) in merged {
        let entries = edits.get(&path).filter(|v| truthy(v));
        let d = kept_moves.iter().find(|(k, _)| **k == path).map(|(_, v)| *v).filter(|v| truthy(v));
        if entries.is_none() && d.is_none() {
            out.push((path, raw));
            continue;
        }
        let mut data = raw;
        if let Some(e) = entries {
            let (bytes, part) = editor::apply_edits(&data, e)?;
            for k in ["recoloured", "rematerialised", "added", "erased", "ignored"] {
                bump(&mut summary, k, part.get(k).and_then(Value::as_i64).unwrap_or(0));
            }
            data = bytes;
        }
        if let Some(d) = d {
            data = moved(&data, d)?;
        }
        out.push((path, data));
        bump(&mut summary, "parts", 1);
    }
    Ok((out, summary))
}

/// `pack_path(path)`.
pub fn pack_path(path: &str) -> String {
    let p = path.replace('\\', "/");
    let clean: Vec<&str> = p.split('/').filter(|s| !s.is_empty() && *s != "." && *s != "..").collect();
    let mut name = if clean.is_empty() { "part.blueprint".to_owned() } else { clean.join("/") };
    if !name.to_lowercase().ends_with(".blueprint") {
        name.push_str(".blueprint");
    }
    if name.contains('/') { name } else { format!("{BLUEPRINT_DIR}{name}") }
}

/// `str(want) or part.filename or ""` for an added part's path.
pub fn wanted_path(want: &Value, filename: &str) -> String {
    let s = str_of(want);
    if s.is_empty() { filename.to_owned() } else { s }
}

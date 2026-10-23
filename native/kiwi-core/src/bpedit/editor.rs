//! `editor.py`: inspect, apply edits, composite a layer stack, transform,
//! `.qb` export/import and the Trove Creations check.

use std::collections::{BTreeSet, HashMap};

use serde_json::{Map, Value, json};

use super::{
    Fault, Res,
    codec::{self, Doc},
    lint, materials,
    merge::{self, Layer},
    py::{IntErr, commas, int, truthy},
    qb, transform,
};
use crate::blueprint::{Voxel, material_for};

pub const EDITOR_VOXEL_CAP: usize = 150_000;
pub const MAX_BLUEPRINT_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_QB_BYTES: usize = 48 * 1024 * 1024;
pub const MAX_ADDED_VOXELS: usize = 20_000;
pub const COORD_LIMIT: i64 = 4096;
pub const MAX_LAYERS: usize = 8;

fn ed(msg: impl Into<String>) -> Fault {
    Fault::Editor(msg.into())
}

/// `_decode_or_raise`.
pub fn decode_or_raise(raw: &[u8]) -> Res<Doc> {
    if raw.len() > MAX_BLUEPRINT_BYTES {
        return Err(ed("That file is too large to be a Trove blueprint."));
    }
    if codec::is_empty(raw) {
        return Err(ed("This blueprint is an empty placeholder - there's nothing to edit."));
    }
    codec::decode(raw).map_err(|e| e.wrap(&[Fault::is_blueprint]))
}

/// `inspect(raw, name=)`.
pub fn inspect(raw: &[u8], name: &str) -> Res<Value> {
    let doc = decode_or_raise(raw)?;
    let n = doc.voxels.len();
    if n > EDITOR_VOXEL_CAP {
        return Err(ed(format!(
            "This model has {} voxels, past the editor's {} limit. It can still be viewed in 3D.",
            commas(n as i64),
            commas(EDITOR_VOXEL_CAP as i64)
        )));
    }
    let mut cols: [Vec<Value>; 11] = Default::default();
    let (mut procedural, mut placeholders, mut locked) = (0usize, 0usize, 0usize);
    let mut seen: Vec<((u16, u8), usize)> = Vec::new();
    let mut seen_at: HashMap<(u16, u8), usize> = HashMap::new();
    for v in &doc.voxels {
        let (r, g, b, kind, level, spec) = material_for(v.r, v.g, v.b, v.w, v.ty);
        let editable = materials::is_editable(v.ty);
        let proc = materials::is_procedural(v.ty, v.r, v.g, v.b);
        let row: [Value; 11] = [
            v.x.into(),
            v.y.into(),
            v.z.into(),
            ((u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b)).into(),
            kind.into(),
            level.into(),
            spec.into(),
            v.ty.into(),
            v.w.into(),
            u8::from(editable).into(),
            u8::from(!proc).into(),
        ];
        for (col, val) in cols.iter_mut().zip(row) {
            col.push(val);
        }
        locked += usize::from(!editable);
        placeholders += usize::from(v.ty == materials::PLACEHOLDER_TYPE);
        procedural += usize::from(proc);
        match seen_at.get(&(v.ty, v.w)) {
            Some(&i) => seen[i].1 += 1,
            None => {
                seen_at.insert((v.ty, v.w), seen.len());
                seen.push(((v.ty, v.w), 1));
            }
        }
    }
    seen.sort_by_key(|e| std::cmp::Reverse(e.1));
    let [x, y, z, rgb, kind, level, spec, ty, w, edit, paint] = cols;
    let attach = codec::attachment(&doc);
    let materials: Vec<Value> = seen
        .iter()
        .map(|&((t, w), count)| {
            json!({"type": t, "w": w, "count": count, "label": materials::describe(i64::from(t), i64::from(w)),
                   "editable": materials::is_editable(t)})
        })
        .collect();
    Ok(json!({
        "name": name,
        "count": n,
        "size": doc.size,
        "version": doc.version,
        "attachment": attach,
        "origin": if doc.version == 5 { doc.pos } else { doc.offset },
        "creation_types": lint::creation_types(),
        "transforms": transform::OPERATIONS.iter().map(|(op, l)| json!({"op": op, "label": l})).collect::<Vec<_>>(),
        "align_modes": merge::ALIGN_MODES.iter().map(|(m, l)| json!({"mode": m, "label": l})).collect::<Vec<_>>(),
        "x": x, "y": y, "z": z, "rgb": rgb,
        "kind": kind, "level": level, "spec": spec,
        "type": ty, "w": w, "edit": edit, "paint": paint,
        "materials": materials,
        "stats": {
            "voxels": n,
            "locked": locked,
            "procedural": procedural,
            "placeholders": placeholders,
            "entities": codec::entity_count(&doc.entity),
        },
        "palette": materials::palette(),
    }))
}

fn material_of(entry: &Map<String, Value>) -> Res<(u16, u8)> {
    let need = || ed("A material edit needs both a type and a w.");
    let t = entry.get("type").ok_or_else(need)?;
    let w = entry.get("w");
    let t = match int(t) {
        Ok(t) => t,
        Err(IntErr::Overflow) => return Err(Fault::Unhandled("cannot convert float infinity to integer".into())),
        Err(_) => return Err(need()),
    };
    let w = match w.map(int) {
        Some(Ok(w)) => w,
        Some(Err(IntErr::Overflow)) => return Err(Fault::Unhandled("cannot convert float infinity".into())),
        _ => return Err(need()),
    };
    materials::validate(t, w).map_err(Fault::Editor)
}

fn colour_of(entry: &Map<String, Value>) -> Res<i64> {
    let bad = || ed("An edit has an invalid colour.");
    let Some(v) = entry.get("rgb") else { return Err(Fault::Unhandled("KeyError: 'rgb'".into())) };
    match int(v) {
        Ok(p) if (0..=0xFF_FFFF).contains(&p) => Ok(p),
        Err(IntErr::Overflow) => Err(Fault::Unhandled("cannot convert float infinity to integer".into())),
        _ => Err(bad()),
    }
}

#[derive(Default, Clone, Copy)]
struct Change {
    material: Option<(u16, u8)>,
    rgb: Option<i64>,
}

struct Add {
    cell: [i64; 3],
    rgb: i64,
    material: (u16, u8),
}

/// `_coerce_edits`: `(changes, deletes, adds)`.
type Coerced = (HashMap<usize, Change>, BTreeSet<usize>, Vec<Add>);

fn coerce(edits: &Value, total: usize) -> Res<Coerced> {
    let Value::Array(edits) = edits else { return Err(ed("The edit list wasn't understood.")) };
    if edits.len() > total + MAX_ADDED_VOXELS {
        return Err(ed("That's more edits than one save should carry."));
    }
    let mut changes: HashMap<usize, Change> = HashMap::new();
    let mut deletes = BTreeSet::new();
    let mut adds = Vec::new();
    for entry in edits {
        let Value::Object(entry) = entry else { return Err(ed("The edit list wasn't understood.")) };
        if let Some(coords) = entry.get("add") {
            let coords = match coords {
                Value::Array(a) if a.len() == 3 => a,
                _ => return Err(ed("A new voxel needs an x, y and z.")),
            };
            let mut cell = [0i64; 3];
            for (k, c) in coords.iter().enumerate() {
                cell[k] = match int(c) {
                    Ok(v) => v,
                    Err(IntErr::Overflow) => return Err(Fault::Unhandled("cannot convert float infinity".into())),
                    Err(_) => return Err(ed("A new voxel has invalid coordinates.")),
                };
            }
            if !cell.iter().all(|c| (-COORD_LIMIT..=COORD_LIMIT).contains(c)) {
                return Err(ed("A new voxel is outside the buildable area."));
            }
            let material = material_of(entry)?;
            adds.push(Add { cell, rgb: colour_of(entry)?, material });
            if adds.len() > MAX_ADDED_VOXELS {
                return Err(ed(format!(
                    "That's more than {} new voxels in one save.",
                    commas(MAX_ADDED_VOXELS as i64)
                )));
            }
            continue;
        }
        let missing = || ed("An edit is missing its voxel index.");
        let idx = match entry.get("i").map(int) {
            Some(Ok(i)) => i,
            Some(Err(IntErr::Overflow)) => return Err(Fault::Unhandled("cannot convert float infinity".into())),
            _ => return Err(missing()),
        };
        if !(0..total as i64).contains(&idx) {
            return Err(ed("An edit points outside the blueprint."));
        }
        let idx = idx as usize;
        if entry.get("del").is_some_and(truthy) {
            deletes.insert(idx);
            continue;
        }
        let mut change = Change::default();
        if entry.contains_key("type") || entry.contains_key("w") {
            change.material = Some(material_of(entry)?);
        }
        if entry.contains_key("rgb") {
            change.rgb = Some(colour_of(entry)?);
        }
        if change.material.is_some() || change.rgb.is_some() {
            let slot = changes.entry(idx).or_default();
            slot.material = change.material.or(slot.material);
            slot.rgb = change.rgb.or(slot.rgb);
        }
    }
    Ok((changes, deletes, adds))
}

/// `_reframe`: refit the box around reshaped voxels; `TransformError` escapes.
/// `(voxels, size, pos, offset, entity_blob)`.
type Reframed = (Vec<Voxel>, [i64; 3], [i64; 3], [i64; 3], Vec<u8>);

fn reframe(mut voxels: Vec<Voxel>, doc: &Doc) -> Res<Reframed> {
    let ext = |k: usize, f: fn(i64, i64) -> i64, init: i64| voxels.iter().map(|v| [v.x, v.y, v.z][k]).fold(init, f);
    let mn = [0, 1, 2].map(|k| ext(k, i64::min, i64::MAX).min(0));
    let mx = [0, 1, 2].map(|k| ext(k, i64::max, i64::MIN));
    let size = [0, 1, 2].map(|k| doc.size[k].max(mx[k] + 1) - mn[k]);
    let shift = [-mn[0], -mn[1], -mn[2]];
    let mut entity = doc.entity.clone();
    let mut attach = codec::attachment(doc);
    if shift != [0, 0, 0] {
        for v in &mut voxels {
            v.x += shift[0];
            v.y += shift[1];
            v.z += shift[2];
        }
        attach = attach.map(|a| [a[0] + shift[0], a[1] + shift[1], a[2] + shift[2]]);
        entity = transform::translate_entities(&entity, shift)?;
    }
    let (pos, offset) = transform::reorigin(doc, attach, size);
    Ok((voxels, size, pos, offset, entity))
}

/// `apply_edits(raw, edits)`: the new bytes and the summary.
pub fn apply_edits(raw: &[u8], edits: &Value) -> Res<(Vec<u8>, Map<String, Value>)> {
    let doc = decode_or_raise(raw)?;
    let (changes, deletes, adds) = coerce(edits, doc.voxels.len())?;
    let mut out: Vec<Voxel> = Vec::with_capacity(doc.voxels.len() + adds.len());
    let (mut recoloured, mut rematerialised, mut ignored) = (0i64, 0i64, 0i64);
    for (i, v) in doc.voxels.iter().enumerate() {
        if deletes.contains(&i) {
            continue;
        }
        let Some(change) = changes.get(&i) else {
            out.push(*v);
            continue;
        };
        let mut nv = *v;
        if let Some((t, w)) = change.material {
            if materials::is_editable(v.ty) {
                (nv.ty, nv.w) = (t, w);
                rematerialised += 1;
            } else {
                ignored += 1;
            }
        }
        if let Some(p) = change.rgb {
            if materials::is_procedural(v.ty, v.r, v.g, v.b) {
                ignored += 1;
            } else {
                (nv.r, nv.g, nv.b) = ((p >> 16) as u8, (p >> 8) as u8, p as u8);
                recoloured += 1;
            }
        }
        out.push(nv);
    }
    if !adds.is_empty() {
        let mut occupied: HashMap<[i64; 3], usize> =
            out.iter().enumerate().map(|(i, v)| ([v.x, v.y, v.z], i)).collect();
        for a in &adds {
            let nv = Voxel {
                x: a.cell[0],
                y: a.cell[1],
                z: a.cell[2],
                r: (a.rgb >> 16) as u8,
                g: (a.rgb >> 8) as u8,
                b: a.rgb as u8,
                w: a.material.1,
                ty: a.material.0,
            };
            match occupied.get(&a.cell) {
                Some(&i) => out[i] = nv,
                None => {
                    occupied.insert(a.cell, out.len());
                    out.push(nv);
                }
            }
        }
    }
    if out.is_empty() {
        return Err(ed("That would erase the whole model - a blueprint needs at least one voxel."));
    }
    if out.len() > EDITOR_VOXEL_CAP {
        return Err(ed(format!(
            "That would take the model past the editor's {} voxel limit.",
            commas(EDITOR_VOXEL_CAP as i64)
        )));
    }
    let (out, size, pos, offset, entity) = if !adds.is_empty() || !deletes.is_empty() {
        reframe(out, &doc)?
    } else {
        (out, doc.size, doc.pos, doc.offset, doc.entity.clone())
    };
    let data = codec::encode(&out, doc.version, Some(pos), &entity, offset, Some(size))
        .map_err(|e| e.wrap(&[Fault::is_blueprint, Fault::is_transform]))?;
    let mut summary = Map::new();
    summary.insert("voxels".into(), out.len().into());
    summary.insert("recoloured".into(), recoloured.into());
    summary.insert("rematerialised".into(), rematerialised.into());
    summary.insert("added".into(), adds.len().into());
    summary.insert("erased".into(), deletes.len().into());
    summary.insert("size".into(), json!(size));
    summary.insert("ignored".into(), ignored.into());
    summary.insert("version".into(), doc.version.into());
    summary.insert("bytes".into(), data.len().into());
    Ok((data, summary))
}

/// `transform(raw, edits, ops)`.
pub fn transform(raw: &[u8], edits: &Value, ops: &Value) -> Res<(Vec<u8>, Value)> {
    let (edited, _) = composite(raw, edits, &[], &Value::Array(Vec::new()), 0)?;
    let doc = codec::decode(&edited)?;
    let moved = transform::apply(&doc, ops)
        .and_then(|m| codec::encode_doc(&m).map(|d| (m, d)))
        .map_err(|e| e.wrap(&[Fault::is_transform, Fault::is_blueprint]))?;
    let (moved, data) = moved;
    let summary = json!({
        "applied": ops,
        "size_before": doc.size,
        "size_after": moved.size,
        "attachment": codec::attachment(&moved),
        "entities": codec::entity_count(&moved.entity),
    });
    Ok((data, summary))
}

/// `_layer_specs`: `(mode, offset, edits)` per layer.
fn layer_specs(specs: &Value, count: usize) -> Res<Vec<(String, [i64; 3], Value)>> {
    let specs = match specs {
        Value::Array(a) if a.len() == count => a,
        _ => return Err(ed("The layer list didn't match the files that arrived.")),
    };
    if count > MAX_LAYERS {
        return Err(ed(format!("That's more than {MAX_LAYERS} layers in one stack.")));
    }
    let mut out = Vec::new();
    for spec in specs {
        let Value::Object(spec) = spec else { return Err(ed("A layer's placement wasn't understood.")) };
        let mode = match spec.get("mode") {
            Some(m) if truthy(m) => super::py::str_of(m),
            _ => "attachment".to_owned(),
        };
        let bad = || ed("A layer has an invalid offset.");
        let offset: Vec<i64> = match spec.get("offset") {
            Some(o) if truthy(o) => py_iter(o)
                .ok_or_else(bad)?
                .iter()
                .map(|v| match int(v) {
                    Ok(i) => Ok(i),
                    Err(IntErr::Overflow) => Err(Fault::Unhandled("cannot convert float infinity".into())),
                    Err(_) => Err(bad()),
                })
                .collect::<Res<_>>()?,
            _ => vec![0, 0, 0],
        };
        if offset.len() != 3 || offset.iter().any(|v| v.abs() > COORD_LIMIT) {
            return Err(bad());
        }
        let edits = match spec.get("edits") {
            Some(e) if truthy(e) => e.clone(),
            _ => Value::Array(Vec::new()),
        };
        out.push((mode, [offset[0], offset[1], offset[2]], edits));
    }
    Ok(out)
}

/// What `for v in value` walks for a JSON value: list items, dict keys, string
/// characters; `None` where Python raises `TypeError`.
fn py_iter(v: &Value) -> Option<Vec<Value>> {
    match v {
        Value::Array(a) => Some(a.clone()),
        Value::Object(o) => Some(o.keys().map(|k| Value::String(k.clone())).collect()),
        Value::String(s) => Some(s.chars().map(|c| Value::String(c.to_string())).collect()),
        _ => None,
    }
}

/// `composite(raw, edits, layers, specs, anchor_at)`.
pub fn composite(
    raw: &[u8],
    edits: &Value,
    layers: &[&[u8]],
    specs: &Value,
    anchor_at: i64,
) -> Res<(Vec<u8>, Map<String, Value>)> {
    let (edited, mut summary) = apply_edits(raw, edits)?;
    if layers.is_empty() {
        summary.insert("layers".into(), 0.into());
        return Ok((edited, summary));
    }
    let placements = layer_specs(specs, layers.len())?;
    if !(0..=layers.len() as i64).contains(&anchor_at) {
        return Err(ed("The anchor layer isn't in the stack."));
    }
    let catch = [Fault::is_merge, Fault::is_blueprint];
    let flattened = (|| -> Res<(Vec<u8>, Value)> {
        let mut stack = Vec::with_capacity(layers.len() + 1);
        for (data, (mode, offset, layer_edits)) in layers.iter().zip(placements) {
            let painted = if truthy(&layer_edits) { apply_edits(data, &layer_edits)?.0 } else { data.to_vec() };
            stack.push(Layer { doc: decode_or_raise(&painted)?, mode, offset });
        }
        stack.insert(anchor_at as usize, Layer { doc: codec::decode(&edited)?, mode: "corner".into(), offset: [0; 3] });
        let (merged, msum) = merge::flatten(&stack, anchor_at as usize)?;
        Ok((codec::encode_doc(&merged)?, msum))
    })();
    let (data, msum) = flattened.map_err(|e| e.wrap(&catch))?;
    if let Value::Object(m) = msum {
        for (k, v) in m {
            summary.insert(k, v);
        }
    }
    Ok((data, summary))
}

/// Archive entries: `(path, bytes)`.
pub type Entries = Vec<(String, Vec<u8>)>;

/// `export_qb`: the zip entries and the summary.
pub fn export_qb(
    raw: &[u8],
    edits: &Value,
    layers: &[&[u8]],
    specs: &Value,
    anchor_at: i64,
    stem: &str,
) -> Res<(Entries, Value)> {
    let (edited, _) = composite(raw, edits, layers, specs, anchor_at)?;
    let doc = codec::decode(&edited).map_err(|e| e.wrap(&[Fault::is_qb, Fault::is_blueprint]))?;
    let built = qb::from_blueprint(&doc, stem);
    let mut names: Vec<&str> = built.files.iter().map(|(n, _)| n.as_str()).collect();
    names.sort_unstable();
    let summary = json!({"notes": built.notes, "attachment": built.attachment, "size": built.size, "files": names});
    Ok((built.files, summary))
}

/// `import_qb(files)`.
pub fn import_qb(files: &[(String, Vec<u8>)]) -> Res<(Vec<u8>, Value)> {
    let total: usize = files.iter().map(|(_, d)| d.len()).sum();
    if total > MAX_QB_BYTES {
        return Err(ed("Those .qb files are too large to compile."));
    }
    qb::to_blueprint(files).map_err(|e| e.wrap(&[Fault::is_qb, Fault::is_blueprint]))
}

/// `check(raw, edits, kind, layers, specs, anchor_at)`.
pub fn check(raw: &[u8], edits: &Value, kind: &str, layers: &[&[u8]], specs: &Value, anchor_at: i64) -> Res<Value> {
    let (edited, _) = composite(raw, edits, layers, specs, anchor_at)?;
    Ok(lint::check(&codec::decode(&edited)?, kind))
}

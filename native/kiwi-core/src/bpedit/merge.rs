//! `merge.py`: flatten a layer stack into one blueprint.

use std::collections::HashMap;

use serde_json::{Value, json};

use super::{
    Fault, Res,
    codec::{self, Doc},
    py::{commas, round_half_even},
    transform,
};
use crate::blueprint::Voxel;

pub const ALIGN_MODES: [(&str, &str); 3] = [
    ("attachment", "Line up the attachment points"),
    ("centre", "Line up the centres"),
    ("corner", "Line up the corners"),
];

const MAX_MERGED_VOXELS: usize = 150_000;

fn mg(msg: impl Into<String>) -> Fault {
    Fault::Merge(msg.into())
}

/// `align_offset(base, over, mode)` for a known mode.
fn align_offset(base: &Doc, over: &Doc, mode: &str) -> Res<[i64; 3]> {
    match mode {
        "corner" => Ok([0, 0, 0]),
        "centre" => {
            Ok([0, 1, 2].map(|i| round_half_even((base.size[i] - 1) as f64 / 2.0 - (over.size[i] - 1) as f64 / 2.0)))
        }
        _ => match (codec::attachment(base), codec::attachment(over)) {
            (Some(a), Some(b)) => Ok([a[0] - b[0], a[1] - b[1], a[2] - b[2]]),
            _ => Err(mg(
                "Lining up the attachment points needs both models to have one, and at least one of these doesn't \
                 (older v3/v4 blueprints don't store one). Line up the centres or the corners instead.",
            )),
        },
    }
}

/// One layer: the model, its alignment mode and its manual nudge.
pub struct Layer {
    pub doc: Doc,
    pub mode: String,
    pub offset: [i64; 3],
}

/// `flatten(docs, anchor)`: the merged doc and its summary.
pub fn flatten(docs: &[Layer], anchor: usize) -> Res<(Doc, Value)> {
    if docs.is_empty() {
        return Err(mg("There's nothing to flatten."));
    }
    if anchor >= docs.len() {
        return Err(mg("The anchor layer isn't in the stack."));
    }
    let base = &docs[anchor].doc;
    if base.voxels.is_empty() {
        return Err(mg("The anchor blueprint needs at least one voxel."));
    }
    let total: usize = docs.iter().map(|d| d.doc.voxels.len()).sum();
    if total > MAX_MERGED_VOXELS {
        return Err(mg(format!(
            "Together those are more than {} voxels, past what the editor can hold.",
            commas(MAX_MERGED_VOXELS as i64)
        )));
    }

    let mut placements: Vec<[i64; 3]> = Vec::new();
    let mut at: HashMap<[i64; 3], usize> = HashMap::new();
    let mut cells: Vec<Voxel> = Vec::new();
    let mut hidden = 0i64;
    for (i, layer) in docs.iter().enumerate() {
        if layer.doc.voxels.is_empty() {
            return Err(mg("A layer has no voxels."));
        }
        let t = if i == anchor {
            [0, 0, 0]
        } else {
            if !ALIGN_MODES.iter().any(|(m, _)| *m == layer.mode) {
                return Err(mg(format!("Unknown alignment: {}.", layer.mode)));
            }
            let a = align_offset(base, &layer.doc, &layer.mode)?;
            [a[0] + layer.offset[0], a[1] + layer.offset[1], a[2] + layer.offset[2]]
        };
        placements.push(t);
        for v in &layer.doc.voxels {
            let key = [v.x + t[0], v.y + t[1], v.z + t[2]];
            let moved = Voxel { x: key[0], y: key[1], z: key[2], ..*v };
            match at.get(&key) {
                Some(&j) => {
                    hidden += 1;
                    cells[j] = moved;
                }
                None => {
                    at.insert(key, cells.len());
                    cells.push(moved);
                }
            }
        }
    }

    let mn = [0, 1, 2].map(|k| cells.iter().map(|v| [v.x, v.y, v.z][k]).min().unwrap_or(0).min(0));
    let shift = [-mn[0], -mn[1], -mn[2]];
    let mut voxels: Vec<Voxel> =
        cells.into_iter().map(|v| Voxel { x: v.x + shift[0], y: v.y + shift[1], z: v.z + shift[2], ..v }).collect();
    voxels.sort_by_key(|v| (v.y, v.z, v.x));
    let size = [0, 1, 2].map(|k| {
        let top = voxels.iter().map(|v| [v.x, v.y, v.z][k]).max().unwrap_or(0) + 1;
        top.max(base.size[k] + shift[k])
    });

    let (entity, note) = merge_entities(anchor, docs, &placements, shift);
    let attach = codec::attachment(base).map(|a| [a[0] + shift[0], a[1] + shift[1], a[2] + shift[2]]);
    let (pos, offset) = transform::reorigin(base, attach, size);
    let merged = Doc { version: base.version, size, pos, voxels, entity, offset };
    let summary = json!({
        "voxels": merged.voxels.len(),
        "from_base": base.voxels.len(),
        "from_layers": total - base.voxels.len(),
        "layers": docs.len() - 1,
        "anchor": anchor,
        "hidden": hidden,
        "placed_at": placements,
        "size": size,
        "attachment": attach,
        "entities": note,
    });
    Ok((merged, summary))
}

fn merge_entities(anchor: usize, docs: &[Layer], placed: &[[i64; 3]], shift: [i64; 3]) -> (Vec<u8>, String) {
    let anchor_blob = docs[anchor].doc.entity.clone();
    let reads: Vec<_> = docs.iter().map(|d| codec::read_entity_records(&d.doc.entity)).collect();
    if reads.iter().any(Option::is_none) {
        return (
            anchor_blob,
            "the placed objects couldn't be read on one of the models, so only the anchor's were kept".into(),
        );
    }
    let reads: Vec<_> = reads.into_iter().flatten().collect();
    let tail = reads[anchor].1.clone();
    let mut out: Vec<codec::Record> = Vec::new();
    let mut carried = 0usize;
    for (i, ((recs, _), t)) in reads.into_iter().zip(placed).enumerate() {
        if i != anchor {
            carried += recs.len();
        }
        out.extend(
            recs.into_iter().map(|(x, y, z, sub)| (x + t[0] + shift[0], y + t[1] + shift[1], z + t[2] + shift[2], sub)),
        );
    }
    if out.is_empty() {
        return (anchor_blob, String::new());
    }
    let note = if carried > 0 {
        format!("{carried} placed object{} came across too", if carried != 1 { "s" } else { "" })
    } else {
        String::new()
    };
    (codec::write_entity_records(&out, &tail), note)
}

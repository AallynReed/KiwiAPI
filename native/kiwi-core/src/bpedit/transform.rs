//! `transform.py`: quarter turns and mirrors that carry the box, the
//! attachment point and placed decos with the voxels.

use serde_json::Value;

use super::{
    Fault, Res,
    codec::{self, Doc},
    py::fdiv,
};

pub const OPERATIONS: [(&str, &str); 6] = [
    ("rotate_x", "Rotate on X"),
    ("rotate_y", "Rotate on Y"),
    ("rotate_z", "Rotate on Z"),
    ("mirror_x", "Flip left-right"),
    ("mirror_y", "Flip upside-down"),
    ("mirror_z", "Flip front-back"),
];

const MAX_MOVE: i64 = 512;

fn tf(msg: impl Into<String>) -> Fault {
    Fault::Transform(msg.into())
}

type Point = [i64; 3];
/// A point map, given the size it was made for.
type Map = fn(Point, Point) -> Point;

/// `_mapper(op, size)`: the point map and the new size.
fn mapper(op: &str, [sx, sy, sz]: Point) -> Option<(impl Fn(Point) -> Point, Point)> {
    let (f, size): (Map, Point) = match op {
        "rotate_x" => (|[x, y, z], s| [x, z, (s[1] - 1) - y], [sx, sz, sy]),
        "rotate_y" => (|[x, y, z], s| [z, y, (s[0] - 1) - x], [sz, sy, sx]),
        "rotate_z" => (|[x, y, z], s| [y, (s[0] - 1) - x, z], [sy, sx, sz]),
        "mirror_x" => (|[x, y, z], s| [(s[0] - 1) - x, y, z], [sx, sy, sz]),
        "mirror_y" => (|[x, y, z], s| [x, (s[1] - 1) - y, z], [sx, sy, sz]),
        "mirror_z" => (|[x, y, z], s| [x, y, (s[2] - 1) - z], [sx, sy, sz]),
        _ => return None,
    };
    let old = [sx, sy, sz];
    Some((move |p| f(p, old), size))
}

fn rewrite_entities(blob: &[u8], point: impl Fn([i64; 3]) -> [i64; 3]) -> Res<Vec<u8>> {
    if blob.len() < 4 {
        return Ok(blob.to_vec());
    }
    let Some((records, tail)) = codec::read_entity_records(blob) else {
        return Err(tf(
            "This model has placed objects whose data can't be read, so it can't be moved without leaving them behind.",
        ));
    };
    if records.is_empty() {
        return Ok(blob.to_vec());
    }
    let moved: Vec<codec::Record> = records
        .into_iter()
        .map(|(x, y, z, sub)| {
            let [a, b, c] = point([x, y, z]);
            (a, b, c, sub)
        })
        .collect();
    Ok(codec::write_entity_records(&moved, &tail))
}

/// `translate_entities`.
pub fn translate_entities(blob: &[u8], shift: [i64; 3]) -> Res<Vec<u8>> {
    if shift == [0, 0, 0] {
        return Ok(blob.to_vec());
    }
    rewrite_entities(blob, |[x, y, z]| [x + shift[0], y + shift[1], z + shift[2]])
}

/// `move_on_rig`: the origin moves, the voxels don't.
pub fn move_on_rig(doc: Doc, delta: [i64; 3]) -> Res<Doc> {
    if delta.iter().map(|v| v.abs()).max().unwrap_or(0) > MAX_MOVE {
        return Err(tf("That's further than a part can be moved on its rig."));
    }
    if delta == [0, 0, 0] {
        return Ok(doc);
    }
    let mut doc = doc;
    let target = if doc.version == 5 { &mut doc.pos } else { &mut doc.offset };
    for k in 0..3 {
        target[k] += delta[k];
    }
    Ok(doc)
}

/// The origin a reshaped box gets: kept on the grip if there is one.
pub fn reorigin(doc: &Doc, attach: Option<[i64; 3]>, size: [i64; 3]) -> ([i64; 3], [i64; 3]) {
    let pos = match attach {
        Some(a) => [-a[0], -a[1], -a[2]],
        None if doc.version == 5 => [-fdiv(size[0], 2), doc.pos[1], -fdiv(size[2], 2)],
        None => codec::centred(size),
    };
    let offset = if matches!(doc.version, 3 | 4) { codec::centred(size) } else { doc.offset };
    (pos, offset)
}

/// `apply(decoded, ops)`; `ops` is the parsed JSON.
pub fn apply(doc: &Doc, ops: &Value) -> Res<Doc> {
    let ops = match ops {
        Value::Array(a) if !a.is_empty() => a,
        _ => return Err(tf("No transform was given.")),
    };
    if ops.len() > 16 {
        return Err(tf("That's more transforms than one step should carry."));
    }
    let mut voxels = doc.voxels.clone();
    let mut size = doc.size;
    let mut attach = codec::attachment(doc);
    let mut entity = doc.entity.clone();
    for op in ops {
        let name = op.as_str().unwrap_or("");
        let Some((point, next)) = mapper(name, size).filter(|_| op.is_string()) else {
            return Err(tf(format!("Unknown transform: {}.", super::py::str_of(op))));
        };
        size = next;
        for v in &mut voxels {
            [v.x, v.y, v.z] = point([v.x, v.y, v.z]);
        }
        attach = attach.map(&point);
        entity = rewrite_entities(&entity, &point)?;
    }
    let (pos, offset) = reorigin(doc, attach, size);
    Ok(Doc { version: doc.version, size, pos, voxels, entity, offset })
}

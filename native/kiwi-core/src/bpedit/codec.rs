//! `codec.py` around [`crate::blueprint`]: whole files in and out (the v5 zlib
//! container included), the exact `BlueprintError` each failure raises, and the
//! entity section.

use super::{Fault, Res, py::fdiv};
use crate::blueprint::{self, InVoxel, Voxel};

pub const MAGIC: &[u8; 5] = b"kiwib";
pub const EMPTY_ENTITIES: &[u8] = &[0, 0, 0, 0];

/// `DecodedBlueprint`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Doc {
    pub version: u32,
    pub size: [i64; 3],
    pub pos: [i64; 3],
    pub voxels: Vec<Voxel>,
    pub entity: Vec<u8>,
    pub offset: [i64; 3],
}

fn bp(msg: impl Into<String>) -> Fault {
    Fault::Blueprint(msg.into())
}

/// `read_uleb128`.
pub fn read_uleb(data: &[u8], mut pos: usize) -> Res<(u32, usize)> {
    let mut result: u128 = 0;
    let mut shift = 0u32;
    loop {
        let Some(&byte) = data.get(pos) else { return Err(bp("Truncated LEB128 value.")) };
        pos += 1;
        result |= u128::from(byte & 0x7F) << shift;
        if byte & 0x80 == 0 {
            return Ok((result as u32, pos));
        }
        shift += 7;
        if shift >= 64 {
            return Err(bp("LEB128 value too large."));
        }
    }
}

pub fn read_svarint(data: &[u8], pos: usize) -> Res<(i64, usize)> {
    let (raw, pos) = read_uleb(data, pos)?;
    let raw = i64::from(raw);
    Ok(((raw >> 1) ^ -(raw & 1), pos))
}

pub fn write_uleb(out: &mut Vec<u8>, value: i64) {
    let mut v = (value as u64 & 0xFFFF_FFFF) as u32;
    loop {
        let byte = (v & 0x7F) as u8;
        v >>= 7;
        if v != 0 {
            out.push(byte | 0x80);
        } else {
            out.push(byte);
            return;
        }
    }
}

pub fn write_svarint(out: &mut Vec<u8>, value: i64) {
    write_uleb(out, (value << 1) ^ (value >> 31));
}

/// The `zlib.error` text of a failed `decompressobj` read.
fn zlib_message(input: &[u8]) -> String {
    let mut d = flate2::Decompress::new(true);
    let mut out = Vec::with_capacity(1 << 16);
    loop {
        out.clear();
        let before = d.total_in() as usize;
        match d.decompress_vec(&input[before..], &mut out, flate2::FlushDecompress::None) {
            Err(e) => return format!("Error -3 while decompressing data: {}", e.message().unwrap_or("invalid data")),
            Ok(flate2::Status::StreamEnd) => return "Error -3 while decompressing data".into(),
            Ok(_) if d.total_in() as usize == before && out.is_empty() => {
                return "Error -3 while decompressing data".into();
            }
            Ok(_) => {}
        }
    }
}

/// `inflate_v5`: the body after the 9-byte header, `decompressobj` style.
pub fn inflate_v5(data: &[u8]) -> Res<Vec<u8>> {
    let input = data.get(9..).unwrap_or(&[]);
    match crate::zlib::inflate(input, usize::MAX) {
        Some(z) => Ok(z.data),
        None => Err(bp(format!("Failed to inflate v5 blueprint: {}", zlib_message(input)))),
    }
}

/// `is_empty_blueprint`.
pub fn is_empty(data: &[u8]) -> bool {
    match blueprint::version(data) {
        Some(3 | 4) => read_uleb(data, 9).map(|(n, _)| n == 0).unwrap_or(true),
        Some(5) => match inflate_v5(data) {
            Ok(body) if body.len() >= 28 => i32::from_le_bytes([body[24], body[25], body[26], body[27]]) <= 0,
            _ => true,
        },
        _ => true,
    }
}

/// The message Python's `_decode_v34` raises for a file the native parser
/// refused (or `Unhandled` if it would not have raised at all).
fn v34_fault(data: &[u8]) -> Fault {
    let (count, mut pos) = match read_uleb(data, 9) {
        Ok(v) => v,
        Err(e) => return e,
    };
    for _ in 0..count {
        for _ in 0..3 {
            match read_svarint(data, pos) {
                Ok((_, p)) => pos = p,
                Err(e) => return e,
            }
        }
        if pos + 6 > data.len() {
            return bp("v3/v4 voxel record is truncated.");
        }
        pos += 6;
    }
    if count == 0 {
        return bp("Blueprint contains no voxels (empty placeholder).");
    }
    Fault::Unhandled("native v3/v4 decode refused a valid file".into())
}

fn i32_at(b: &[u8], off: usize) -> i64 {
    i64::from(i32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]]))
}

/// `decode_full`.
pub fn decode(data: &[u8]) -> Res<Doc> {
    let Some(version) = blueprint::version(data) else {
        return Err(bp("Not a Trove blueprint (missing 'kiwib' magic)."));
    };
    match version {
        3 | 4 => {
            let d = blueprint::decode_v34(data, version).ok_or_else(|| v34_fault(data))?;
            Ok(Doc {
                version,
                size: d.size,
                pos: d.pos,
                entity: data[d.entity_start..].to_vec(),
                voxels: d.voxels,
                offset: d.offset,
            })
        }
        5 => {
            let body = inflate_v5(data)?;
            if body.len() < 32 {
                return Err(bp("v5 blueprint payload is too small."));
            }
            let (sx, sy, sz, count) = (i32_at(&body, 12), i32_at(&body, 16), i32_at(&body, 20), i32_at(&body, 24));
            if count <= 0 || sx <= 0 || sy <= 0 || sz <= 0 {
                return Err(bp("v5 blueprint is empty or has invalid bounds."));
            }
            // Short tables are a `struct.error`, which nothing catches.
            let d = blueprint::decode_v5(&body)
                .ok_or_else(|| Fault::Unhandled("unpack_from requires a buffer of more bytes".into()))?;
            Ok(Doc {
                version: 5,
                size: d.size,
                pos: d.pos,
                entity: body[d.entity_start..].to_vec(),
                voxels: d.voxels,
                offset: d.offset,
            })
        }
        v => Err(bp(format!("Unsupported kiwib blueprint version: {v}."))),
    }
}

/// `attachment_point`: v5 only, the negated origin.
pub fn attachment(doc: &Doc) -> Option<[i64; 3]> {
    (doc.version == 5).then(|| [-doc.pos[0], -doc.pos[1], -doc.pos[2]])
}

fn in_voxel(v: &Voxel) -> InVoxel {
    InVoxel {
        x: v.x,
        y: v.y,
        z: v.z,
        r: i64::from(v.r),
        g: i64::from(v.g),
        b: i64::from(v.b),
        w: i64::from(v.w),
        ty: i64::from(v.ty),
    }
}

/// `codec.encode(voxels, version=, pos=, entity_blob=, offset=, size=)` as it
/// runs with the native module: v5 is deflated at level 9 by the same zlib.
pub fn encode(
    voxels: &[Voxel],
    version: u32,
    pos: Option<[i64; 3]>,
    entity: &[u8],
    offset: [i64; 3],
    size: Option<[i64; 3]>,
) -> Res<Vec<u8>> {
    let blob = if version == 5 && entity.is_empty() { EMPTY_ENTITIES } else { entity };
    let ins: Vec<InVoxel> = voxels.iter().map(in_voxel).collect();
    if let Some(out) = blueprint::encode(&ins, version, size, pos, offset, blob) {
        if version != 5 {
            return Ok(out);
        }
        let mut file = MAGIC.to_vec();
        file.extend_from_slice(&5u32.to_le_bytes());
        file.extend(
            crate::zlib::deflate(&out, 9, usize::MAX, false).ok_or_else(|| Fault::Unhandled("deflate".into()))?,
        );
        return Ok(file);
    }
    if voxels.is_empty() {
        return Err(bp("Cannot encode a blueprint with no voxels."));
    }
    if !(3..=5).contains(&version) {
        return Err(bp(format!("Unsupported encode version: {version}.")));
    }
    let [sx, sy, sz] =
        size.unwrap_or_else(|| [0, 1, 2].map(|k| voxels.iter().map(|v| [v.x, v.y, v.z][k]).max().unwrap_or(0) + 1));
    let outside = voxels.iter().any(|v| v.x >= sx || v.y >= sy || v.z >= sz || v.x.min(v.y).min(v.z) < 0);
    if outside {
        return Err(bp("A voxel lies outside the blueprint's bounding box."));
    }
    // What is left is a `struct.pack` range error.
    Err(Fault::Unhandled("argument out of range".into()))
}

/// `encode(...)` with every field taken from a doc.
pub fn encode_doc(doc: &Doc) -> Res<Vec<u8>> {
    encode(&doc.voxels, doc.version, Some(doc.pos), &doc.entity, doc.offset, Some(doc.size))
}

pub type Record = (i64, i64, i64, Vec<u8>);

/// `read_entity_records`: `None` when the section can't be read exactly.
pub fn read_entity_records(blob: &[u8]) -> Option<(Vec<Record>, Vec<u8>)> {
    if blob.len() < 4 {
        return Some((Vec::new(), Vec::new()));
    }
    let count = u32::from_le_bytes([blob[0], blob[1], blob[2], blob[3]]);
    let mut records = Vec::new();
    let mut i = 4usize;
    for _ in 0..count {
        let (x, p) = read_svarint(blob, i).ok()?;
        let (y, p) = read_svarint(blob, p).ok()?;
        let (z, p) = read_svarint(blob, p).ok()?;
        let (len, p) = read_uleb(blob, p).ok()?;
        let end = p.checked_add(len as usize)?;
        if end > blob.len() {
            return None;
        }
        records.push((x, y, z, blob[p..end].to_vec()));
        i = end;
    }
    Some((records, blob[i..].to_vec()))
}

/// `write_entity_records`.
pub fn write_entity_records(records: &[Record], tail: &[u8]) -> Vec<u8> {
    let mut out = (records.len() as u32).to_le_bytes().to_vec();
    for (x, y, z, sub) in records {
        write_svarint(&mut out, *x);
        write_svarint(&mut out, *y);
        write_svarint(&mut out, *z);
        write_uleb(&mut out, sub.len() as i64);
        out.extend_from_slice(sub);
    }
    out.extend_from_slice(tail);
    out
}

/// `parse_entity_section(blob)["count"]`.
pub fn entity_count(blob: &[u8]) -> u32 {
    if blob.len() < 4 { 0 } else { u32::from_le_bytes([blob[0], blob[1], blob[2], blob[3]]) }
}

/// The centred origin Trove gives a box: `(-(sx // 2), -(sy // 2), -(sz // 2))`.
pub fn centred(size: [i64; 3]) -> [i64; 3] {
    [-fdiv(size[0], 2), -fdiv(size[1], 2), -fdiv(size[2], 2)]
}

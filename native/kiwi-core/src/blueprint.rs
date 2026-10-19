//! Trove `.blueprint` (kiwib) voxel tables, mirroring `app/trove/blueprint/codec.py`.
//!
//! zlib stays with the caller: v5 functions take the inflated body and `encode`
//! returns the uncompressed v5 payload. Every `None` means "the Python reference
//! raises here", so a binding can fall back to it and keep the exact exception.

use std::collections::HashMap;
use std::collections::hash_map::Entry;

const MAGIC: &[u8; 5] = b"kiwib";
const DEFAULT_TYPE: i64 = 0x15;
const AUTHORED: [u16; 6] = [21, 18, 54, 55, 56, 24];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Voxel {
    pub x: i64,
    pub y: i64,
    pub z: i64,
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub w: u8,
    pub ty: u16,
}

pub struct Decoded {
    pub version: u32,
    pub size: [i64; 3],
    pub pos: [i64; 3],
    pub voxels: Vec<Voxel>,
    /// Where the entity section starts in the slice that was parsed.
    pub entity_start: usize,
    pub offset: [i64; 3],
}

/// Python floor division (callers guarantee `b != 0`).
#[inline]
fn fdiv(a: i64, b: i64) -> i64 {
    let q = a / b;
    if a % b != 0 && ((a < 0) != (b < 0)) { q - 1 } else { q }
}

/// Python modulo (callers guarantee `b != 0`).
#[inline]
fn fmod(a: i64, b: i64) -> i64 {
    let r = a % b;
    if r != 0 && ((r < 0) != (b < 0)) { r + b } else { r }
}

/// `read_uleb128`: masked to 32 bits, at most 10 bytes.
#[inline]
fn uleb(data: &[u8], mut pos: usize) -> Option<(u32, usize)> {
    let mut result: u128 = 0;
    let mut shift = 0u32;
    loop {
        let byte = *data.get(pos)?;
        pos += 1;
        result |= u128::from(byte & 0x7F) << shift;
        if byte & 0x80 == 0 {
            return Some((result as u32, pos));
        }
        shift += 7;
        if shift >= 64 {
            return None;
        }
    }
}

#[inline]
fn svarint(data: &[u8], pos: usize) -> Option<(i64, usize)> {
    let (raw, pos) = uleb(data, pos)?;
    let raw = i64::from(raw);
    Some(((raw >> 1) ^ -(raw & 1), pos))
}

#[inline]
fn i32_at(b: &[u8], off: usize) -> i64 {
    i64::from(i32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]]))
}

#[inline]
fn u16_at(b: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([b[off], b[off + 1]])
}

#[inline]
fn u32_at(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

pub fn version(data: &[u8]) -> Option<u32> {
    if data.len() < 9 || &data[..5] != MAGIC {
        return None;
    }
    Some(u32_at(data, 5))
}

/// v3/v4 voxel records from offset 9: absolute coords, file order. `strict` is the
/// codec (a truncated record fails); otherwise stop there, as assembly does.
fn v34_records(data: &[u8], strict: bool) -> Option<(Vec<Voxel>, usize)> {
    let (count, mut pos) = uleb(data, 9)?;
    let mut out = Vec::with_capacity((count as usize).min(data.len() / 9));
    for _ in 0..count {
        let (x, p) = svarint(data, pos)?;
        let (y, p) = svarint(data, p)?;
        let (z, p) = svarint(data, p)?;
        pos = p;
        if pos + 6 > data.len() {
            if strict {
                return None;
            }
            break;
        }
        out.push(Voxel {
            x,
            y,
            z,
            ty: u16_at(data, pos),
            b: data[pos + 2],
            g: data[pos + 3],
            r: data[pos + 4],
            w: data[pos + 5],
        });
        pos += 6;
    }
    Some((out, pos))
}

/// The v5 tables as `struct.unpack_from` reads them; None where it would raise.
/// Returns (the 8 header ints, voxels in grid-local coords, end of the tables).
fn v5_tables(body: &[u8]) -> Option<([i64; 8], Vec<Voxel>, usize)> {
    if body.len() < 32 {
        return None;
    }
    let mut h = [0i64; 8];
    for (k, v) in h.iter_mut().enumerate() {
        *v = i32_at(body, k * 4);
    }
    let (sx, sz, count, start) = (h[3], h[5], h[6], h[7]);
    if count < 0 {
        return None;
    }
    let n = count as usize;
    let deltas_end = 32 + 4 * n.saturating_sub(1);
    let types_end = deltas_end + 2 * n;
    let end = types_end + 4 * n;
    if end > body.len() {
        return None;
    }
    if n == 0 {
        return Some((h, Vec::new(), end));
    }
    let plane = sx * sz;
    if plane == 0 || sx == 0 {
        return None;
    }
    let mut out = Vec::with_capacity(n);
    let mut l = start;
    for i in 0..n {
        if i > 0 {
            l = l.checked_add(i32_at(body, 32 + 4 * (i - 1)))?;
        }
        let rem = fmod(l, plane);
        let c = u32_at(body, types_end + 4 * i);
        out.push(Voxel {
            x: fmod(rem, sx),
            y: fdiv(l, plane),
            z: fdiv(rem, sx),
            r: (c >> 16) as u8,
            g: (c >> 8) as u8,
            b: c as u8,
            w: (c >> 24) as u8,
            ty: u16_at(body, deltas_end + 2 * i),
        });
    }
    Some((h, out, end))
}

/// `codec._decode_v34`.
pub fn decode_v34(data: &[u8], version: u32) -> Option<Decoded> {
    let (raw, pos) = v34_records(data, true)?;
    if raw.is_empty() {
        return None;
    }
    let (mut mn, mut mx) = ([i64::MAX; 3], [i64::MIN; 3]);
    for v in &raw {
        for (k, c) in [v.x, v.y, v.z].into_iter().enumerate() {
            mn[k] = mn[k].min(c);
            mx[k] = mx[k].max(c);
        }
    }
    let size = [mx[0] - mn[0] + 1, mx[1] - mn[1] + 1, mx[2] - mn[2] + 1];
    let voxels = raw
        .into_iter()
        .map(|v| Voxel { x: v.x - mn[0], y: v.y - mn[1], z: v.z - mn[2], ..v })
        .collect();
    Some(Decoded {
        version,
        size,
        pos: [-fdiv(size[0], 2), -fdiv(size[1], 2), -fdiv(size[2], 2)],
        voxels,
        entity_start: pos,
        offset: mn,
    })
}

/// `codec._decode_v5`, over the inflated body.
pub fn decode_v5(body: &[u8]) -> Option<Decoded> {
    if body.len() < 32 {
        return None;
    }
    let (sx, sy, sz, count) = (i32_at(body, 12), i32_at(body, 16), i32_at(body, 20), i32_at(body, 24));
    if count <= 0 || sx <= 0 || sy <= 0 || sz <= 0 {
        return None;
    }
    let (h, voxels, end) = v5_tables(body)?;
    Some(Decoded {
        version: 5,
        size: [h[3], h[4], h[5]],
        pos: [h[0], h[1], h[2]],
        voxels,
        entity_start: end,
        offset: [0, 0, 0],
    })
}

/// Decode by version with codec semantics. `payload` is the whole file for v3/v4
/// and the inflated body for v5.
pub fn decode_any(version: u32, payload: &[u8]) -> Option<Decoded> {
    match version {
        3 | 4 => decode_v34(payload, version),
        5 => decode_v5(payload),
        _ => None,
    }
}

/// `render.voxel.material_for`, kind as its wire code (S/G/E/GE = 0..3).
#[inline]
pub fn material_for(mut r: u8, mut g: u8, mut b: u8, w: u8, t: u16) -> (u8, u8, u8, u8, u16, u8) {
    if !AUTHORED.contains(&t) && r.max(g).max(b) <= 24 {
        (r, g, b) = match t {
            1 => (150, 130, 100),
            2 => (40, 170, 0),
            3 => (120, 120, 120),
            4 => (110, 78, 165),
            79 => (185, 194, 197),
            100 => (220, 222, 225),
            174 => (232, 33, 70),
            _ => (110, 110, 110),
        };
    }
    let kind = match t {
        55 => 2,
        56 => 3,
        18 | 54 => 1,
        _ => 0,
    };
    let wc = w.min(7);
    let level = if kind == 1 || kind == 3 { 16 + 32 * u16::from(wc) } else { 255 };
    (r, g, b, kind, level, if kind == 0 { wc } else { 0 })
}

/// A voxel with its render material.
#[derive(Clone, Copy)]
pub struct Rendered {
    pub x: i64,
    pub y: i64,
    pub z: i64,
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub kind: u8,
    pub level: u16,
    pub spec: u8,
}

#[inline]
fn rendered(v: &Voxel, origin: [i64; 3]) -> Rendered {
    let (r, g, b, kind, level, spec) = material_for(v.r, v.g, v.b, v.w, v.ty);
    Rendered { x: origin[0] + v.x, y: origin[1] + v.y, z: origin[2] + v.z, r, g, b, kind, level, spec }
}

/// `to_render_voxels`: the last voxel at a cell wins, the first one's position is kept.
pub fn render_cells(voxels: &[Voxel]) -> Vec<Rendered> {
    let mut at: HashMap<(i64, i64, i64), usize> = HashMap::with_capacity(voxels.len());
    let mut out: Vec<Rendered> = Vec::with_capacity(voxels.len());
    for v in voxels {
        let cell = rendered(v, [0, 0, 0]);
        match at.entry((v.x, v.y, v.z)) {
            Entry::Occupied(e) => out[*e.get()] = cell,
            Entry::Vacant(e) => {
                e.insert(out.len());
                out.push(cell);
            }
        }
    }
    out
}

/// `mods_hub.assembly._decode_grid`: absolute grid coords, every voxel, materials.
pub fn grid(version: u32, payload: &[u8]) -> Option<Vec<Rendered>> {
    let (voxels, origin) = match version {
        3 | 4 => (v34_records(payload, false)?.0, [0, 0, 0]),
        5 => {
            let (h, v, _) = v5_tables(payload)?;
            (v, [h[0], h[1], h[2]])
        }
        _ => return Some(Vec::new()),
    };
    Some(voxels.iter().map(|v| rendered(v, origin)).collect())
}

/// An encoder input voxel before `int(...) & mask`.
#[derive(Clone, Copy)]
pub struct InVoxel {
    pub x: i64,
    pub y: i64,
    pub z: i64,
    pub r: i64,
    pub g: i64,
    pub b: i64,
    pub w: i64,
    pub ty: i64,
}

impl InVoxel {
    #[allow(clippy::too_many_arguments)]
    pub fn new(x: i64, y: i64, z: i64, r: i64, g: i64, b: i64, w: Option<i64>, ty: Option<i64>) -> Self {
        InVoxel { x, y, z, r, g, b, w: w.unwrap_or(0), ty: ty.unwrap_or(DEFAULT_TYPE) }
    }
}

fn write_uleb(out: &mut Vec<u8>, value: i128) {
    let mut v = (value & 0xFFFF_FFFF) as u32;
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

#[inline]
fn write_svarint(out: &mut Vec<u8>, v: i128) {
    write_uleb(out, (v << 1) ^ (v >> 31));
}

#[inline]
fn i32_le(out: &mut Vec<u8>, v: i128) -> Option<()> {
    out.extend_from_slice(&i32::try_from(v).ok()?.to_le_bytes());
    Some(())
}

/// `codec.encode`: the whole file for v3/v4, the uncompressed payload (header ints
/// onward, entity blob appended) for v5. For v5 pass the already-defaulted blob.
pub fn encode(
    voxels: &[InVoxel],
    version: u32,
    size: Option<[i64; 3]>,
    pos: Option<[i64; 3]>,
    offset: [i64; 3],
    entity_blob: &[u8],
) -> Option<Vec<u8>> {
    if voxels.is_empty() || !(3..=5).contains(&version) {
        return None;
    }
    let (mut mx, mut mn) = ([i64::MIN; 3], i64::MAX);
    for v in voxels {
        mx[0] = mx[0].max(v.x);
        mx[1] = mx[1].max(v.y);
        mx[2] = mx[2].max(v.z);
        mn = mn.min(v.x.min(v.y).min(v.z));
    }
    let [sx, sy, sz] = size.unwrap_or([mx[0] + 1, mx[1] + 1, mx[2] + 1]);
    if mx[0] >= sx || mx[1] >= sy || mx[2] >= sz || mn < 0 {
        return None;
    }

    if version != 5 {
        let mut out = Vec::with_capacity(14 + voxels.len() * 12 + entity_blob.len());
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&version.to_le_bytes());
        write_uleb(&mut out, voxels.len() as i128);
        for v in voxels {
            write_svarint(&mut out, i128::from(offset[0]) + i128::from(v.x));
            write_svarint(&mut out, i128::from(offset[1]) + i128::from(v.y));
            write_svarint(&mut out, i128::from(offset[2]) + i128::from(v.z));
            out.extend_from_slice(&(v.ty as u16).to_le_bytes());
            out.extend_from_slice(&[v.b as u8, v.g as u8, v.r as u8, v.w as u8]);
        }
        out.extend_from_slice(entity_blob);
        return Some(out);
    }

    let pos = pos.unwrap_or([-fdiv(sx, 2), 0, -fdiv(sz, 2)]);
    let plane = i128::from(sx) * i128::from(sz);
    let mut cells: HashMap<i128, (u16, u32)> = HashMap::with_capacity(voxels.len());
    for v in voxels {
        let l = i128::from(v.x) + i128::from(v.z) * i128::from(sx) + i128::from(v.y) * plane;
        let color = u32::from(v.b as u8)
            | (u32::from(v.g as u8) << 8)
            | (u32::from(v.r as u8) << 16)
            | (u32::from(v.w as u8) << 24);
        cells.insert(l, (v.ty as u16, color));
    }
    let mut order: Vec<i128> = cells.keys().copied().collect();
    order.sort_unstable();

    let mut out = Vec::with_capacity(32 + order.len() * 10 + entity_blob.len());
    for v in [pos[0], pos[1], pos[2], sx, sy, sz] {
        i32_le(&mut out, i128::from(v))?;
    }
    i32_le(&mut out, order.len() as i128)?;
    i32_le(&mut out, order[0])?;
    for w in order.windows(2) {
        i32_le(&mut out, w[1] - w[0])?;
    }
    for l in &order {
        out.extend_from_slice(&cells[l].0.to_le_bytes());
    }
    for l in &order {
        out.extend_from_slice(&cells[l].1.to_le_bytes());
    }
    out.extend_from_slice(entity_blob);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floor_semantics() {
        assert_eq!((fdiv(-7, 2), fmod(-7, 2)), (-4, 1));
        assert_eq!((fdiv(7, -2), fmod(7, -2)), (-4, -1));
    }

    #[test]
    fn v34_round_trip() {
        let vox = [
            InVoxel::new(0, 0, 0, 1, 2, 3, None, None),
            InVoxel::new(2, 1, 0, 9, 8, 7, Some(3), Some(18)),
        ];
        let bytes = encode(&vox, 3, None, None, [-1, 0, 0], b"").unwrap();
        let d = decode_v34(&bytes, 3).unwrap();
        assert_eq!(d.size, [3, 2, 1]);
        assert_eq!(d.offset, [-1, 0, 0]);
        assert_eq!((d.voxels[1].x, d.voxels[1].ty, d.voxels[1].w), (2, 18, 3));
    }
}

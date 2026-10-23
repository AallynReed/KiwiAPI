//! `qb.py`: Qubicle Binary with Trove's `_a`/`_s`/`_t` material maps.

use std::collections::HashMap;

use serde_json::{Value, json};

use super::{
    Fault, Res,
    codec::{self, Doc},
    materials::{self, Class},
    py::{commas, fdiv},
};
use crate::blueprint::{Voxel, material_for};

const QB_VERSION: u32 = 257;
const CODEFLAG: u32 = 2;
const NEXTSLICEFLAG: u32 = 6;
pub const ATTACHMENT_RGB: Rgb = (255, 0, 255);
const MAX_MAPPED_SPECULAR: i64 = 4;
const VALID_ALPHA: [u8; 9] = [16, 48, 80, 112, 144, 176, 208, 240, 255];

pub type Rgb = (u8, u8, u8);
type Cell = (i64, i64, i64);

pub fn type_index(t: u16) -> Option<i64> {
    match t {
        21 => Some(0),
        18 => Some(1),
        54 => Some(2),
        55 => Some(3),
        56 => Some(4),
        _ => None,
    }
}

fn index_type(i: i64) -> u16 {
    [21, 18, 54, 55, 56][i as usize]
}

pub fn type_rgb(i: i64) -> Option<Rgb> {
    match i {
        0 => Some((255, 255, 255)),
        1 => Some((128, 128, 128)),
        2 => Some((64, 64, 64)),
        3 => Some((255, 0, 0)),
        4 => Some((255, 255, 0)),
        _ => None,
    }
}

pub fn specular_rgb(w: i64) -> Option<Rgb> {
    match w {
        0 => Some((128, 0, 0)),
        1 => Some((0, 128, 0)),
        2 => Some((0, 0, 128)),
        3 => Some((128, 128, 0)),
        4 => Some((128, 0, 128)),
        _ => None,
    }
}

fn qb(msg: &str) -> Fault {
    Fault::Qb(msg.into())
}

/// A dict keyed by cell: first-insertion order, last value wins.
#[derive(Default, Clone)]
pub struct Cells {
    at: HashMap<Cell, usize>,
    pub items: Vec<(Cell, Rgb)>,
}

impl Cells {
    pub fn set(&mut self, k: Cell, v: Rgb) {
        match self.at.get(&k) {
            Some(&i) => self.items[i].1 = v,
            None => {
                self.at.insert(k, self.items.len());
                self.items.push((k, v));
            }
        }
    }

    pub fn get(&self, k: &Cell) -> Option<Rgb> {
        self.at.get(k).map(|&i| self.items[i].1)
    }

    fn len(&self) -> usize {
        self.items.len()
    }
}

pub struct Matrix {
    pub size: (i64, i64, i64),
    pub pos: (i64, i64, i64),
    pub cells: Cells,
}

fn u32_at(d: &[u8], off: usize) -> Res<u32> {
    d.get(off..off + 4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .ok_or_else(|| Fault::Unhandled("unpack_from requires a buffer".into()))
}

/// `read_qb`.
pub fn read(data: &[u8]) -> Res<Vec<Matrix>> {
    if data.len() < 24 {
        return Err(qb("That file is too short to be a .qb."));
    }
    let h: Vec<u32> = (0..6).map(|k| u32_at(data, k * 4)).collect::<Res<_>>()?;
    let (colour_format, z_orientation, compression, count) = (h[1], h[2], h[3], h[5]);
    if count == 0 || count > 512 {
        return Err(qb("That .qb declares an implausible number of matrices."));
    }
    let mut out = Vec::new();
    let mut off = 24usize;
    for _ in 0..count {
        if off >= data.len() {
            return Err(qb("That .qb is truncated."));
        }
        let name_len = data[off] as usize;
        off += 1 + name_len;
        let (sx, sy, sz) =
            (i64::from(u32_at(data, off)?), i64::from(u32_at(data, off + 4)?), i64::from(u32_at(data, off + 8)?));
        off += 12;
        let pos = (
            i64::from(u32_at(data, off)? as i32),
            i64::from(u32_at(data, off + 4)? as i32),
            i64::from(u32_at(data, off + 8)? as i32),
        );
        off += 12;
        if (sx as i128) * (sy as i128) * (sz as i128) > 64_000_000 {
            return Err(qb("That .qb declares an implausibly large matrix."));
        }
        let mut cells = Cells::default();
        if compression == 0 {
            let need = (4 * sx * sy * sz) as usize;
            if off + need > data.len() {
                return Err(qb("That .qb is truncated."));
            }
            for z in 0..sz {
                for y in 0..sy {
                    let base = off + 4 * (z * sy * sx + y * sx) as usize;
                    for x in 0..sx {
                        let i = base + 4 * x as usize;
                        if data[i + 3] > 0 {
                            cells.set((x, y, z), (data[i], data[i + 1], data[i + 2]));
                        }
                    }
                }
            }
            off += need;
        } else {
            off = read_rle(data, off, (sx, sz), &mut cells)?;
        }
        if colour_format == 1 {
            for item in &mut cells.items {
                let (r, g, b) = item.1;
                item.1 = (b, g, r);
            }
        }
        if z_orientation == 0 {
            let mut flipped = Cells::default();
            for &((x, y, z), v) in &cells.items {
                flipped.set((x, y, sz - 1 - z), v);
            }
            cells = flipped;
        }
        out.push(Matrix { size: (sx, sy, sz), pos, cells });
    }
    Ok(out)
}

fn read_rle(data: &[u8], mut off: usize, (sx, sz): (i64, i64), cells: &mut Cells) -> Res<usize> {
    let truncated = || qb("That .qb is truncated.");
    for z in 0..sz {
        let mut index: i64 = 0;
        loop {
            if off + 4 > data.len() {
                return Err(truncated());
            }
            let chunk = u32_at(data, off)?;
            off += 4;
            if chunk == NEXTSLICEFLAG {
                break;
            }
            let place = |index: i64| {
                if sx == 0 {
                    return Err(Fault::Unhandled("integer modulo by zero".into()));
                }
                Ok((index.rem_euclid(sx), fdiv(index, sx), z))
            };
            if chunk == CODEFLAG {
                if off + 8 > data.len() {
                    return Err(truncated());
                }
                let run = i64::from(u32_at(data, off)?);
                let (r, g, b, a) = (data[off + 4], data[off + 5], data[off + 6], data[off + 7]);
                off += 8;
                if a > 0 {
                    for _ in 0..run {
                        cells.set(place(index)?, (r, g, b));
                        index += 1;
                    }
                } else {
                    index += run;
                }
            } else {
                let [r, g, b, a] = chunk.to_le_bytes();
                if a > 0 {
                    cells.set(place(index)?, (r, g, b));
                }
                index += 1;
            }
        }
    }
    Ok(off)
}

/// `_merge`: every matrix in one grid, its size and the first matrix's position.
type Merged = (Cells, (i64, i64, i64), (i64, i64, i64));

fn merge(m: Vec<Matrix>) -> Res<Merged> {
    if m.is_empty() {
        return Err(qb("That .qb has no matrices."));
    }
    if m.len() == 1 {
        let one = m.into_iter().next().unwrap_or_else(|| unreachable!());
        return Ok((one.cells, one.size, one.pos));
    }
    let mn = (
        m.iter().map(|x| x.pos.0).min().unwrap_or(0),
        m.iter().map(|x| x.pos.1).min().unwrap_or(0),
        m.iter().map(|x| x.pos.2).min().unwrap_or(0),
    );
    let mut cells = Cells::default();
    for x in &m {
        let o = (x.pos.0 - mn.0, x.pos.1 - mn.1, x.pos.2 - mn.2);
        for &((a, b, c), v) in &x.cells.items {
            cells.set((a + o.0, b + o.1, c + o.2), v);
        }
    }
    let max = |k: fn(&Cell) -> i64| cells.items.iter().map(|(c, _)| k(c)).max();
    let size = match (max(|c| c.0), max(|c| c.1), max(|c| c.2)) {
        (Some(a), Some(b), Some(c)) => (a + 1, b + 1, c + 1),
        _ => return Err(Fault::Unhandled("max() arg is an empty sequence".into())),
    };
    Ok((cells, size, m[0].pos))
}

/// `write_qb`: one RLE matrix.
pub fn write(cells: &HashMap<Cell, Rgb>, size: (i64, i64, i64), name: &str, pos: (i64, i64, i64)) -> Vec<u8> {
    let (sx, sy, sz) = size;
    let mut out = Vec::new();
    for v in [QB_VERSION, 0, 1, 1, 0, 1] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    let stem: Vec<u8> = name.chars().map(|c| u8::try_from(u32::from(c)).unwrap_or(b'?')).take(255).collect();
    out.push(stem.len() as u8);
    out.extend_from_slice(&stem);
    for v in [sx, sy, sz] {
        out.extend_from_slice(&(v as u32).to_le_bytes());
    }
    for v in [pos.0, pos.1, pos.2] {
        out.extend_from_slice(&(v as i32).to_le_bytes());
    }
    let flush = |out: &mut Vec<u8>, val: Option<Rgb>, len: u32| {
        if len == 0 {
            return;
        }
        if len > 1 {
            out.extend_from_slice(&CODEFLAG.to_le_bytes());
            out.extend_from_slice(&len.to_le_bytes());
        }
        match val {
            Some((r, g, b)) => out.extend_from_slice(&[r, g, b, 255]),
            None => out.extend_from_slice(&[0, 0, 0, 0]),
        }
    };
    for z in 0..sz {
        let (mut run_val, mut run_len): (Option<Rgb>, u32) = (None, 0);
        for y in 0..sy {
            for x in 0..sx {
                let v = cells.get(&(x, y, z)).copied();
                if v == run_val {
                    run_len += 1;
                } else {
                    flush(&mut out, run_val, run_len);
                    (run_val, run_len) = (v, 1);
                }
            }
        }
        flush(&mut out, run_val, run_len);
        out.extend_from_slice(&NEXTSLICEFLAG.to_le_bytes());
    }
    out
}

pub struct Exported {
    pub files: Vec<(String, Vec<u8>)>,
    pub notes: Vec<String>,
    pub attachment: Option<[i64; 3]>,
    pub size: [i64; 3],
}

/// `from_blueprint`: the four authoring files, in base/_a/_s/_t order.
pub fn from_blueprint(doc: &Doc, stem: &str) -> Exported {
    let attach = codec::attachment(doc);
    let (mut mn, [mut sx, mut sy, mut sz]) = ([0i64; 3], doc.size);
    if let Some(a) = attach {
        mn = [a[0].min(0), a[1].min(0), a[2].min(0)];
        sx = sx.max(a[0] + 1) - mn[0];
        sy = sy.max(a[1] + 1) - mn[1];
        sz = sz.max(a[2] + 1) - mn[2];
    }
    let qx = |x: i64| sx - 1 - (x - mn[0]);
    let mut maps: [HashMap<Cell, Rgb>; 4] = Default::default();
    let mut dropped_spec = 0i64;
    let mut unmapped: Vec<(u16, i64)> = Vec::new();
    for v in &doc.voxels {
        let key = (qx(v.x), v.y - mn[1], v.z - mn[2]);
        let (t, w) = (v.ty, i64::from(v.w));
        let mut rgb = (v.r, v.g, v.b);
        if materials::is_procedural(t, v.r, v.g, v.b) {
            let (r, g, b, ..) = material_for(v.r, v.g, v.b, v.w, t);
            rgb = (r, g, b);
        }
        maps[0].insert(key, rgb);
        let index = type_index(t).unwrap_or_else(|| {
            match unmapped.iter_mut().find(|(k, _)| *k == t) {
                Some(e) => e.1 += 1,
                None => unmapped.push((t, 1)),
            }
            0
        });
        maps[3].insert(key, type_rgb(index).unwrap_or((255, 255, 255)));
        let rough = specular_rgb(0).unwrap_or_default();
        if materials::class(t) == Class::Glass {
            let level = materials::alpha_for_w(w) as u8;
            maps[1].insert(key, (level, level, level));
            maps[2].insert(key, rough);
        } else {
            maps[1].insert(key, (255, 255, 255));
            if w > MAX_MAPPED_SPECULAR {
                dropped_spec += 1;
                maps[2].insert(key, rough);
            } else {
                maps[2].insert(key, specular_rgb(w).unwrap_or(rough));
            }
        }
    }
    let mut akey = (0, 0, 0);
    let pos = match attach {
        Some(a) => {
            akey = (qx(a[0]), a[1] - mn[1], a[2] - mn[2]);
            for m in &mut maps {
                m.insert(akey, ATTACHMENT_RGB);
            }
            (-akey.0, -akey.1, -akey.2)
        }
        None => (0, 0, 0),
    };
    let size = (sx, sy, sz);
    let names = [stem.to_owned(), format!("{stem}_a"), format!("{stem}_s"), format!("{stem}_t")];
    let files = names.iter().zip(&maps).map(|(n, m)| (format!("{n}.qb"), write(m, size, n, pos))).collect();

    let mut notes = Vec::new();
    if attach.is_some() {
        notes.push(format!(
            "The attachment point is written as a magenta voxel at ({}, {}, {}) in all four files.",
            akey.0, akey.1, akey.2
        ));
    } else if doc.version != 5 {
        notes.push(format!(
            "This is a v{} blueprint, which stores no attachment point, so none was written. Add a magenta \
             (255, 0, 255) voxel where the model should be held before compiling it back.",
            doc.version
        ));
    }
    if dropped_spec > 0 {
        notes.push(format!(
            "{} solid voxel{} use a specular finish beyond the five the material maps can express, and were \
             written as rough. Re-importing this .qb will not bring them back.",
            commas(dropped_spec),
            if dropped_spec != 1 { "s" } else { "" }
        ));
    }
    if !unmapped.is_empty() {
        let total: i64 = unmapped.iter().map(|(_, n)| n).sum();
        let mut types: Vec<u16> = unmapped.iter().map(|(t, _)| *t).collect();
        types.sort_unstable();
        let names: Vec<String> = types.iter().take(3).map(|&t| materials::describe(i64::from(t), 0)).collect();
        notes.push(format!(
            "{} voxel{} use materials the game keeps to itself ({}) and were written as plain solid. The \
             .blueprint you exported from still has them.",
            commas(total),
            if total != 1 { "s" } else { "" },
            names.join(", ")
        ));
    }
    Exported { files, notes, attachment: attach, size: [sx, sy, sz] }
}

/// `_classify`: base / a / s / t by filename suffix.
fn classify(files: &[(String, Vec<u8>)]) -> Res<HashMap<&'static str, &[u8]>> {
    let mut out: HashMap<&'static str, &[u8]> = HashMap::new();
    for (name, data) in files {
        let stem = name.rsplit('/').next().unwrap_or(name);
        let stem = if stem.to_lowercase().ends_with(".qb") { strip_last_chars(stem, 3) } else { stem };
        let low = stem.to_lowercase();
        let key = if low.ends_with("_a") {
            "a"
        } else if low.ends_with("_s") {
            "s"
        } else if low.ends_with("_t") {
            "t"
        } else {
            out.entry("base").or_insert(data);
            continue;
        };
        out.insert(key, data);
    }
    if !out.contains_key("base") {
        return Err(qb("No base .qb was found - the model file must not end in _a, _s or _t."));
    }
    Ok(out)
}

/// `s[:-n]` by characters.
fn strip_last_chars(s: &str, n: usize) -> &str {
    let count = s.chars().count();
    let keep = count.saturating_sub(n);
    s.char_indices().nth(keep).map(|(i, _)| &s[..i]).unwrap_or(s)
}

/// `to_blueprint(files)`: the compiled v5 bytes and the summary.
pub fn to_blueprint(files: &[(String, Vec<u8>)]) -> Res<(Vec<u8>, Value)> {
    let parts = classify(files)?;
    let base = read(parts["base"])?;
    let single = base.len() == 1;
    let (cells, size, base_pos) = merge(base)?;
    if cells.len() == 0 {
        return Err(qb("That .qb has no visible voxels."));
    }
    let mut maps: Vec<(&str, Cells)> = Vec::new();
    for key in ["a", "s", "t"] {
        if let Some(data) = parts.get(key) {
            let (m, msize, _) = merge(read(data)?)?;
            if msize != size {
                return Err(Fault::Qb(format!(
                    "The {} map is {}x{}x{} but the model is {}x{}x{}. Material maps have to match the model exactly.",
                    key.to_uppercase(),
                    msize.0,
                    msize.1,
                    msize.2,
                    size.0,
                    size.1,
                    size.2
                )));
            }
            maps.push((key, m));
        }
    }
    let map = |k: &str| maps.iter().find(|(n, _)| *n == k).map(|(_, m)| m);

    let mut attach = cells.items.iter().find(|(_, rgb)| *rgb == ATTACHMENT_RGB).map(|(c, _)| *c);
    let from_marker = attach.is_some();
    if attach.is_none() && single {
        let (dx, dy, dz) = base_pos;
        if dx <= 0
            && dy <= 0
            && dz <= 0
            && (dx != 0 || dy != 0 || dz != 0)
            && -dx < size.0
            && -dy < size.1
            && -dz < size.2
        {
            attach = Some((-dx, -dy, -dz));
        }
    }
    let filled_marker = attach.and_then(|a| {
        [
            (a.0 + 1, a.1, a.2),
            (a.0 - 1, a.1, a.2),
            (a.0, a.1 + 1, a.2),
            (a.0, a.1 - 1, a.2),
            (a.0, a.1, a.2 + 1),
            (a.0, a.1, a.2 - 1),
        ]
        .into_iter()
        .find(|c| cells.get(c).is_some_and(|v| v != ATTACHMENT_RGB))
    });

    let mut sorted = cells.items.clone();
    sorted.sort_by_key(|((x, y, z), _)| (*z, *y, *x));
    let (mut unknown_alpha, mut unknown_type, mut unknown_spec) = (0i64, 0i64, 0i64);
    let mut voxels: Vec<Voxel> = Vec::new();
    for ((x, y, z), mut rgb) in sorted {
        let mut src = (x, y, z);
        if rgb == ATTACHMENT_RGB {
            match filled_marker {
                Some(donor) if Some(src) == attach => {
                    rgb = cells.get(&donor).unwrap_or(rgb);
                    src = donor;
                }
                _ => continue,
            }
        }
        let index = match map("t").and_then(|m| m.get(&src)) {
            None => 0,
            Some(c) if c == ATTACHMENT_RGB => 0,
            Some(c) => match (0..5).find(|&i| type_rgb(i) == Some(c)) {
                Some(i) => i,
                None => {
                    unknown_type += 1;
                    0
                }
            },
        };
        let ty = index_type(index);
        let w = if materials::class(ty) == Class::Glass {
            match map("a").and_then(|m| m.get(&src)) {
                None => 0,
                Some(c) if c == ATTACHMENT_RGB => 0,
                Some((a, b, c)) if a == b && b == c && VALID_ALPHA.contains(&a) => {
                    if a == 255 {
                        0
                    } else {
                        fdiv(i64::from(a) - 16, 32).clamp(0, 7)
                    }
                }
                Some(_) => {
                    unknown_alpha += 1;
                    3
                }
            }
        } else {
            match map("s").and_then(|m| m.get(&src)) {
                None => 0,
                Some(c) if c == ATTACHMENT_RGB || c == (255, 255, 255) => 0,
                Some(c) => match (0..5).find(|&i| specular_rgb(i) == Some(c)) {
                    Some(i) => i,
                    None => {
                        unknown_spec += 1;
                        0
                    }
                },
            }
        };
        voxels.push(Voxel { x, y, z, r: rgb.0, g: rgb.1, b: rgb.2, w: w as u8, ty });
    }
    if voxels.is_empty() {
        return Err(qb("That .qb is only an attachment point - there's no model to compile."));
    }
    let (mut lo, mut hi) = ([i64::MAX; 3], [i64::MIN; 3]);
    for v in &voxels {
        for (k, c) in [v.x, v.y, v.z].into_iter().enumerate() {
            lo[k] = lo[k].min(c);
            hi[k] = hi[k].max(c);
        }
    }
    let tight = [hi[0] - lo[0] + 1, hi[1] - lo[1] + 1, hi[2] - lo[2] + 1];
    for v in &mut voxels {
        v.x = tight[0] - 1 - (v.x - lo[0]);
        v.y -= lo[1];
        v.z -= lo[2];
    }
    let pos = match attach {
        Some(a) => [-(tight[0] - 1 - (a.0 - lo[0])), -(a.1 - lo[1]), -(a.2 - lo[2])],
        None => [-fdiv(tight[0], 2), 0, -fdiv(tight[2], 2)],
    };
    let data = codec::encode(&voxels, 5, Some(pos), codec::EMPTY_ENTITIES, [0, 0, 0], Some(tight))?;
    let mut keys: Vec<&str> = maps.iter().map(|(k, _)| *k).collect();
    keys.sort_unstable();
    let summary = json!({
        "voxels": voxels.len(),
        "size": tight,
        "attachment": attach.map(|a| vec![a.0 - lo[0], a.1 - lo[1], a.2 - lo[2]]),
        "attachment_source": if from_marker { Some("marker") } else if attach.is_some() { Some("offset") } else { None },
        "maps": keys,
        "unknown_type": unknown_type,
        "unknown_alpha": unknown_alpha,
        "unknown_specular": unknown_spec,
    });
    Ok((data, summary))
}

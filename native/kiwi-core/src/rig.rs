//! Creature rigs: the baked rig data (`app/trove/mods_hub/assembly.py`) and
//! the prefab binding map (`rig_index.py`), over rows and bytes in memory.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Map, Value, json};

use crate::blueprint;
use crate::bpedit::codec;

/// One `rigs/<name>.rig.json`.
pub struct Rig {
    pub raw: Map<String, Value>,
    pub voxel_scale: f64,
    pub rest: HashMap<String, [f64; 16]>,
    head_aps: HashSet<String>,
}

impl Rig {
    pub fn parse(raw: Map<String, Value>) -> Rig {
        let voxel_scale = raw.get("voxel_scale").and_then(Value::as_f64).unwrap_or(0.0);
        let mut rest = HashMap::new();
        if let Some(Value::Object(r)) = raw.get("rest") {
            for (k, v) in r {
                let mut m = [0.0; 16];
                if let Some(a) = v.as_array() {
                    for (i, x) in a.iter().take(16).enumerate() {
                        m[i] = x.as_f64().unwrap_or(0.0);
                    }
                }
                rest.insert(k.clone(), m);
            }
        }
        let head_aps = head_aps_of(&raw);
        Rig { raw, voxel_scale, rest, head_aps }
    }

    pub fn rest_value(&self) -> Value {
        self.raw.get("rest").cloned().unwrap_or(Value::Null)
    }

    pub fn voxel_scale_value(&self) -> Value {
        self.raw.get("voxel_scale").cloned().unwrap_or(Value::Null)
    }

    pub fn animations_value(&self) -> Value {
        self.raw.get("animations").cloned().unwrap_or(Value::Null)
    }

    /// `animations_for(name)`.
    pub fn animation_names(&self) -> Vec<String> {
        match self.raw.get("animations") {
            Some(Value::Object(a)) => a.keys().cloned().collect(),
            _ => Vec::new(),
        }
    }

    pub fn has_ap(&self, ap: &str) -> bool {
        self.rest.contains_key(ap)
    }

    pub fn is_head_ap(&self, ap: &str) -> bool {
        self.head_aps.contains(ap)
    }

    pub fn head_aps(&self) -> &HashSet<String> {
        &self.head_aps
    }

    /// `rig_pose(name)`.
    pub fn pose(&self) -> Value {
        json!({"voxel_scale": self.voxel_scale_value(), "rest": self.rest_value()})
    }
}

/// `head_aps(rig_name)`: attach points under `head_JNT`.
fn head_aps_of(raw: &Map<String, Value>) -> HashSet<String> {
    let bones = raw.get("bones").and_then(Value::as_object);
    let list = |k: &str| bones.and_then(|b| b.get(k)).and_then(Value::as_array).cloned().unwrap_or_default();
    let names: Vec<String> = list("names").iter().map(|v| v.as_str().unwrap_or_default().to_owned()).collect();
    let parents: Vec<Option<i64>> = list("parents").iter().map(Value::as_i64).collect();
    let heads: HashSet<usize> =
        names.iter().enumerate().filter(|(_, n)| n.to_lowercase() == "head_jnt").map(|(i, _)| i).collect();
    let mut out = HashSet::new();
    for (i, name) in names.iter().enumerate() {
        if !name.to_lowercase().starts_with("ap_") {
            continue;
        }
        let mut j = parents.get(i).copied().flatten();
        let mut guard = 0;
        while let Some(p) = j {
            if p < 0 || p as usize >= names.len() || guard > names.len() {
                break;
            }
            if heads.contains(&(p as usize)) {
                out.insert(name.chars().skip(3).collect::<String>().to_lowercase());
                break;
            }
            j = parents.get(p as usize).copied().flatten();
            guard += 1;
        }
    }
    out
}

/// `scale_for(ap_key, rig_name, head_scale, part_scale)`.
pub fn scale_for(ap: &str, rig: Option<&Rig>, head_scale: f64, part_scale: f64) -> f64 {
    part_scale * if rig.is_some_and(|r| r.is_head_ap(ap)) { head_scale } else { 1.0 }
}

/// `_RIG_NAME_RE.match(name)`.
pub fn valid_name(name: &str) -> bool {
    let n = name.strip_suffix('\n').unwrap_or(name);
    !n.is_empty() && n.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// One positioned part of the viewer payload.
#[derive(Clone, Debug)]
pub struct Part {
    pub name: String,
    pub x: Vec<f64>,
    pub y: Vec<f64>,
    pub z: Vec<f64>,
    pub rgb: Vec<u32>,
    pub kind: Vec<u8>,
    pub level: Vec<u16>,
    pub spec: Option<Vec<u8>>,
    pub scale: Option<f64>,
    /// Coordinates became floats (`_unbury_enclosed_emissive`).
    pub shifted: bool,
}

impl Part {
    fn scale(&self) -> f64 {
        self.scale.unwrap_or(1.0)
    }

    pub fn len(&self) -> usize {
        self.x.len()
    }

    pub fn is_empty(&self) -> bool {
        self.x.is_empty()
    }

    pub fn to_value(&self) -> Value {
        let coords = |v: &[f64]| -> Value {
            if self.shifted {
                Value::Array(v.iter().map(|&f| json!(f)).collect())
            } else {
                Value::Array(v.iter().map(|&f| json!(f as i64)).collect())
            }
        };
        let mut m = Map::new();
        m.insert("name".into(), self.name.clone().into());
        m.insert("x".into(), coords(&self.x));
        m.insert("y".into(), coords(&self.y));
        m.insert("z".into(), coords(&self.z));
        m.insert("rgb".into(), json!(self.rgb));
        m.insert("kind".into(), json!(self.kind));
        m.insert("level".into(), json!(self.level));
        if let Some(s) = &self.spec {
            m.insert("spec".into(), json!(s));
        }
        if let Some(s) = self.scale {
            m.insert("scale".into(), json!(s));
        }
        Value::Object(m)
    }
}

/// `_part_at(key, raw, scale)`; `Err` is an exception escaping the decoder.
pub fn part_at(key: &str, raw: &[u8], scale: f64) -> Result<Option<Part>, String> {
    if !raw.starts_with(b"kiwib") || codec::is_empty(raw) {
        return Ok(None);
    }
    let Some(version) = blueprint::version(raw) else { return Ok(None) };
    let payload = match version {
        3 | 4 => raw.to_vec(),
        5 => crate::zlib::inflate(&raw[9..], usize::MAX).ok_or("zlib.error")?.data,
        _ => return Ok(None),
    };
    let cells = blueprint::grid(version, &payload).ok_or("blueprint grid decode failed")?;
    if cells.is_empty() {
        return Ok(None);
    }
    let spec: Vec<u8> = cells.iter().map(|c| c.spec).collect();
    Ok(Some(Part {
        name: key.to_owned(),
        x: cells.iter().map(|c| c.x as f64).collect(),
        y: cells.iter().map(|c| c.y as f64).collect(),
        z: cells.iter().map(|c| c.z as f64).collect(),
        rgb: cells.iter().map(|c| (u32::from(c.r) << 16) | (u32::from(c.g) << 8) | u32::from(c.b)).collect(),
        kind: cells.iter().map(|c| c.kind).collect(),
        level: cells.iter().map(|c| c.level).collect(),
        spec: spec.iter().any(|&s| s != 0).then_some(spec),
        scale: (scale != 1.0).then_some(scale),
        shifted: false,
    }))
}

fn scale_key(s: f64) -> u64 {
    if s == 0.0 { 0 } else { s.to_bits() }
}

/// `_merge_same_ap(parts)`.
pub fn merge_same_ap(parts: Vec<Part>) -> Vec<Part> {
    let mut out: Vec<Part> = Vec::new();
    for part in parts {
        let key = (part.name.clone(), scale_key(part.scale()));
        let Some(into) = out.iter_mut().find(|p| (p.name.clone(), scale_key(p.scale())) == key) else {
            out.push(part);
            continue;
        };
        let (n_into, n_part) = (into.len(), part.len());
        into.x.extend(&part.x);
        into.y.extend(&part.y);
        into.z.extend(&part.z);
        into.rgb.extend(&part.rgb);
        into.kind.extend(&part.kind);
        into.level.extend(&part.level);
        if into.spec.is_some() || part.spec.is_some() {
            let mut s = into.spec.take().unwrap_or_else(|| vec![0; n_into]);
            s.extend(part.spec.unwrap_or_else(|| vec![0; n_part]));
            into.spec = Some(s);
        }
    }
    out
}

/// `np.array(rest).reshape(4, 4).T @ np.diag([ps] * 3 + [1.0])`, row-major.
fn placed(rest: &[f64; 16], ps: f64) -> [[f64; 4]; 4] {
    let mut m = [[0.0; 4]; 4];
    for (r, row) in m.iter_mut().enumerate() {
        for (c, cell) in row.iter_mut().enumerate() {
            *cell = rest[c * 4 + r] * if c < 3 { ps } else { 1.0 };
        }
    }
    m
}

/// `(m @ [x, y, z, 1])[:3]` rounded as numpy's BLAS rounds it: many columns
/// go through the gemm kernel (a fused chain), a single one through gemv.
fn world(m: &[[f64; 4]; 4], x: f64, y: f64, z: f64, single: bool) -> [f64; 3] {
    let row = |r: usize| {
        if single {
            (m[r][0] * x + m[r][2] * z) + (m[r][1] * y + m[r][3])
        } else {
            m[r][2].mul_add(z, m[r][1].mul_add(y, m[r][0] * x)) + m[r][3]
        }
    };
    [row(0), row(1), row(2)]
}

fn worlds_of(m: &[[f64; 4]; 4], p: &Part) -> Vec<[f64; 3]> {
    let single = p.len() == 1;
    (0..p.len()).map(|i| world(m, p.x[i], p.y[i], p.z[i], single)).collect()
}

fn round_even(x: f64) -> i64 {
    crate::bpedit::py::round_half_even(x)
}

/// numpy's `pairwise_sum` over one column.
fn pairwise(a: &[f64]) -> f64 {
    let n = a.len();
    if n < 8 {
        return a.iter().fold(0.0, |r, v| r + v);
    }
    if n <= 128 {
        let mut r: [f64; 8] = std::array::from_fn(|j| a[j]);
        let mut i = 8;
        while i < n - n % 8 {
            for (j, acc) in r.iter_mut().enumerate() {
                *acc += a[i + j];
            }
            i += 8;
        }
        let mut res = ((r[0] + r[1]) + (r[2] + r[3])) + ((r[4] + r[5]) + (r[6] + r[7]));
        for v in &a[i..] {
            res += v;
        }
        return res;
    }
    let mut n2 = n / 2;
    n2 -= n2 % 8;
    pairwise(&a[..n2]) + pairwise(&a[n2..])
}

/// `.mean(0)` of `(m @ local)[:3].T` (a transposed view) or of a `vstack`
/// of them (which keeps their Fortran order): each column reduced pairwise.
fn mean_cols(points: &[[f64; 3]]) -> [f64; 3] {
    let n = points.len() as f64;
    std::array::from_fn(|k| (0.0 + pairwise(&points.iter().map(|p| p[k]).collect::<Vec<_>>())) / n)
}

/// `np.linalg.inv(a)` as OpenBLAS's `dgesv` computes it: a left-looking
/// `getf2` LU, then the `trsm` solves against the identity. `None` is the
/// `LinAlgError` of a singular matrix.
fn inv3(a: [[f64; 3]; 3]) -> Option<[[f64; 3]; 3]> {
    let mut col: [[f64; 3]; 3] = std::array::from_fn(|j| std::array::from_fn(|i| a[i][j]));
    let mut ipiv = [0usize; 3];
    for j in 0..3 {
        for (i, &p) in ipiv.iter().enumerate().take(j) {
            col[j].swap(i, p);
        }
        for i in 1..j {
            let d = (0..i).fold(0.0, |d, k| d + col[k][i] * col[j][k]);
            col[j][i] -= d;
        }
        for i in j..3 {
            let acc = (0..j).fold(0.0, |acc, k| acc + col[k][i] * col[j][k]);
            col[j][i] -= acc;
        }
        let mut jp = j;
        for i in j + 1..3 {
            if col[j][i].abs() > col[j][jp].abs() {
                jp = i;
            }
        }
        ipiv[j] = jp;
        if jp != j {
            for c in col.iter_mut().take(j + 1) {
                c.swap(j, jp);
            }
        }
        if col[j][j] == 0.0 {
            return None;
        }
        let r = 1.0 / col[j][j];
        for v in col[j].iter_mut().skip(j + 1) {
            *v *= r;
        }
    }
    let lu = |i: usize, k: usize| col[k][i];
    let inv: [f64; 3] = std::array::from_fn(|i| 1.0 / lu(i, i));
    let mut x = [[0.0; 3]; 3];
    for (c, out) in x.iter_mut().enumerate() {
        let mut b: [f64; 3] = std::array::from_fn(|i| if i == c { 1.0 } else { 0.0 });
        for (i, &p) in ipiv.iter().enumerate() {
            b.swap(i, p);
        }
        b[1] -= b[0] * lu(1, 0);
        b[2] -= lu(2, 1).mul_add(b[1], lu(2, 0) * b[0]);
        let x2 = b[2] * inv[2];
        b[0] -= lu(0, 2) * x2;
        b[1] -= lu(1, 2) * x2;
        let x1 = b[1] * inv[1];
        b[0] -= x1 * lu(0, 1);
        *out = [b[0] * inv[0], x1, x2];
    }
    Some(std::array::from_fn(|r| std::array::from_fn(|c| x[c][r])))
}

/// `_unbury_enclosed_emissive(parts, rest, voxel_scale)`; `Err` is numpy's
/// `LinAlgError` on a part drawn at scale 0.
pub fn unbury(parts: &mut [Part], rig: &Rig) -> Result<(), String> {
    let vs = rig.voxel_scale;
    let cell = if vs == 0.0 { 1.0 } else { vs };
    let tol = cell * 0.6;
    let mut grid: HashMap<(i64, i64, i64), Vec<[f64; 3]>> = HashMap::new();
    let mut worlds: Vec<Vec<[f64; 3]>> = Vec::with_capacity(parts.len());
    let mut mats = Vec::with_capacity(parts.len());
    let ident = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];
    for p in parts.iter() {
        let m = placed(rig.rest.get(&p.name).unwrap_or(&ident), vs * p.scale());
        let w = worlds_of(&m, p);
        for pt in &w {
            let key = (round_even(pt[0] / cell), round_even(pt[1] / cell), round_even(pt[2] / cell));
            grid.entry(key).or_default().push(*pt);
        }
        mats.push(m);
        worlds.push(w);
    }
    let all: Vec<[f64; 3]> = worlds.iter().flatten().copied().collect();
    if all.is_empty() {
        return Ok(());
    }
    let centre = mean_cols(&all);
    let occupied = |q: [f64; 3]| -> bool {
        let b = (round_even(q[0] / cell), round_even(q[1] / cell), round_even(q[2] / cell));
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    if let Some(pts) = grid.get(&(b.0 + dx, b.1 + dy, b.2 + dz))
                        && pts.iter().any(|pt| {
                            (pt[0] - q[0]).abs() < tol && (pt[1] - q[1]).abs() < tol && (pt[2] - q[2]).abs() < tol
                        })
                    {
                        return true;
                    }
                }
            }
        }
        false
    };
    let axes =
        [[cell, 0.0, 0.0], [-cell, 0.0, 0.0], [0.0, cell, 0.0], [0.0, -cell, 0.0], [0.0, 0.0, cell], [0.0, 0.0, -cell]];
    for (idx, p) in parts.iter_mut().enumerate() {
        let kinds = &p.kind;
        if kinds.is_empty() {
            continue;
        }
        let glow = kinds.iter().filter(|&&k| k == 2 || k == 3).count();
        if (glow as f64) / (kinds.len() as f64) < 0.5 {
            continue;
        }
        let w = &worlds[idx];
        let enclosed = w.iter().all(|pt| axes.iter().all(|ax| occupied([pt[0] + ax[0], pt[1] + ax[1], pt[2] + ax[2]])));
        if !enclosed {
            continue;
        }
        let pm = mean_cols(w);
        let out = [pm[0] - centre[0], pm[1] - centre[1], pm[2] - centre[2]];
        let norm = (0.0 + out[0] * out[0] + out[1] * out[1] + out[2] * out[2]).sqrt();
        if norm < 1e-9 {
            continue;
        }
        let m = &mats[idx];
        let inv = inv3([[m[0][0], m[0][1], m[0][2]], [m[1][0], m[1][1], m[1][2]], [m[2][0], m[2][1], m[2][2]]])
            .ok_or_else(|| "LinAlgError: Singular matrix".to_owned())?;
        let k = 1.5 * vs;
        let v = [out[0] / norm * k, out[1] / norm * k, out[2] / norm * k];
        let d: [f64; 3] = std::array::from_fn(|r| inv[r][2].mul_add(v[2], inv[r][0].mul_add(v[0], inv[r][1] * v[1])));
        p.x.iter_mut().for_each(|x| *x += d[0]);
        p.y.iter_mut().for_each(|y| *y += d[1]);
        p.z.iter_mut().for_each(|z| *z += d[2]);
        p.shifted = true;
    }
    Ok(())
}

/// The model payload: `{voxel_scale, rig, parts, rest, animations}`.
pub fn payload(rig: &Rig, rig_name: &str, parts: &[Part]) -> Value {
    json!({
        "voxel_scale": rig.voxel_scale_value(),
        "rig": rig_name,
        "parts": parts.iter().map(Part::to_value).collect::<Vec<_>>(),
        "rest": rig.rest_value(),
        "animations": rig.animations_value(),
    })
}

/// `assemble(tmod_files, rig_name, ap_overrides, part_scales)` as positioned parts.
pub fn assemble_parts(
    rig: &Rig,
    files: &[(String, Vec<u8>)],
    overrides: &HashMap<String, String>,
    part_scales: &HashMap<String, f64>,
) -> Result<Vec<Part>, String> {
    let mut parts = Vec::new();
    for (path, raw) in files {
        let p = path.to_lowercase();
        let Some(stem) = p.rsplit('/').next().and_then(|n| n.strip_suffix(".blueprint")) else { continue };
        let Some(key) = overrides.get(stem).filter(|k| !k.is_empty() && rig.has_ap(k)) else { continue };
        let scale = part_scales.get(stem).copied().unwrap_or_else(|| scale_for(key, Some(rig), 1.0, 1.0));
        if let Some(part) = part_at(key, raw, scale)? {
            parts.push(part);
        }
    }
    if parts.is_empty() {
        return Ok(parts);
    }
    let mut parts = merge_same_ap(parts);
    unbury(&mut parts, rig)?;
    Ok(parts)
}

/// `assemble(...)`: the payload, `None` when nothing places.
pub fn assemble(
    rig: &Rig,
    rig_name: &str,
    files: &[(String, Vec<u8>)],
    overrides: &HashMap<String, String>,
    part_scales: &HashMap<String, f64>,
) -> Result<Option<Value>, String> {
    let parts = assemble_parts(rig, files, overrides, part_scales)?;
    Ok((!parts.is_empty()).then(|| payload(rig, rig_name, &parts)))
}

pub const RECOLOR_MIN_SAT: f64 = 0.5;

/// `recolor(part, rgb, min_sat, max_off)`.
pub fn recolor(part: &mut Part, rgb: [i64; 3], min_sat: f64, max_off: Option<i64>) {
    let [r0, g0, b0] = rgb.map(|v| v.clamp(0, 255) as f64);
    for packed in part.rgb.iter_mut() {
        let (r, g, b) = ((*packed >> 16) & 255, (*packed >> 8) & 255, *packed & 255);
        let hi = r.max(g).max(b);
        if hi == 0 || 1.0 - f64::from(r.min(g).min(b)) / f64::from(hi) < min_sat {
            continue;
        }
        if let Some(off) = max_off
            && (r != hi || i64::from(g.max(b)) > off)
        {
            continue;
        }
        let level = f64::from(hi) / 255.0;
        *packed = (((r0 * level) as u32) << 16) | (((g0 * level) as u32) << 8) | ((b0 * level) as u32);
    }
}

/// One `assemble_placements` entry: `(AP, raw, scale[, tint])`.
pub struct Placement {
    pub ap: String,
    pub raw: Vec<u8>,
    pub scale: f64,
    pub tint: Option<([i64; 3], Option<i64>)>,
}

/// `assemble_placements(placements, rig_name)` as positioned parts.
pub fn placement_parts(rig: &Rig, placements: &[Placement]) -> Result<Vec<Part>, String> {
    let mut parts = Vec::new();
    for pl in placements {
        if pl.raw.is_empty() || !rig.has_ap(&pl.ap) {
            continue;
        }
        if let Some(mut part) = part_at(&pl.ap, &pl.raw, pl.scale)? {
            if let Some((rgb, max_off)) = pl.tint {
                recolor(&mut part, rgb, RECOLOR_MIN_SAT, max_off);
            }
            parts.push(part);
        }
    }
    if parts.is_empty() {
        return Ok(parts);
    }
    let mut parts = merge_same_ap(parts);
    unbury(&mut parts, rig)?;
    Ok(parts)
}

/// A baked voxel: `(r, g, b, kind letter, level, spec)`.
pub type Baked = (u8, u8, u8, &'static str, u16, u8);

/// Baked cells in dict order.
pub type Cells = Vec<((i64, i64, i64), Baked)>;

/// `(skeleton, prefab, parts)`.
pub type Creature = (String, String, Vec<(String, String)>);

const KIND_NAME: [&str; 4] = ["S", "G", "E", "GE"];

/// `_bake(parts, rest, voxel_scale)`: rig-space cells in dict order, later wins.
pub fn bake(parts: &[Part], rig: &Rig) -> Vec<((i64, i64, i64), Baked)> {
    let scale = if rig.voxel_scale == 0.0 { 1.0 } else { rig.voxel_scale };
    let mut at: HashMap<(i64, i64, i64), usize> = HashMap::new();
    let mut out: Vec<((i64, i64, i64), Baked)> = Vec::new();
    for part in parts {
        let Some(rest) = rig.rest.get(&part.name) else { continue };
        if part.is_empty() {
            continue;
        }
        let m = placed(rest, scale * part.scale());
        for (i, w) in worlds_of(&m, part).into_iter().enumerate() {
            let key = (round_even(w[0] / scale), round_even(w[1] / scale), round_even(w[2] / scale));
            let rgb = part.rgb[i];
            let kind = KIND_NAME.get(usize::from(part.kind[i])).copied().unwrap_or("S");
            let spec = part.spec.as_ref().map_or(0, |s| s[i]);
            let cell =
                (((rgb >> 16) & 255) as u8, ((rgb >> 8) & 255) as u8, (rgb & 255) as u8, kind, part.level[i], spec);
            match at.get(&key) {
                Some(&j) => out[j].1 = cell,
                None => {
                    at.insert(key, out.len());
                    out.push((key, cell));
                }
            }
        }
    }
    out
}

/// `assemble_voxels(parts, rig_name)`: `[(AP, raw, scale?)]` baked.
pub fn assemble_voxels(rig: &Rig, parts: &[(String, Vec<u8>, Option<f64>)]) -> Result<Cells, String> {
    let mut built = Vec::new();
    for (ap, raw, scale) in parts {
        if raw.is_empty() || !rig.has_ap(ap) {
            continue;
        }
        let s = scale.unwrap_or_else(|| scale_for(ap, Some(rig), 1.0, 1.0));
        if let Some(p) = part_at(ap, raw, s)? {
            built.push(p);
        }
    }
    Ok(if built.is_empty() { Vec::new() } else { bake(&built, rig) })
}

/// `placement_voxels(placements, rig_name)`.
pub fn placement_voxels(rig: &Rig, placements: &[Placement]) -> Result<Cells, String> {
    let parts = placement_parts(rig, placements)?;
    Ok(if parts.is_empty() { Vec::new() } else { bake(&parts, rig) })
}

// --- the binding map (rig_index) ----------------------------------------------

/// A `rig_binding` row: `(prefab, blueprint, skeleton, ap_key, mesh_scale, head_scale)`.
pub type Row = (String, String, String, String, f64, f64);

#[derive(Default)]
pub struct RigMap {
    pub by_blueprint: HashMap<String, (String, String)>,
    pub creatures: HashMap<String, (String, Vec<(String, String)>)>,
    pub owner: HashMap<String, String>,
    pub by_path: HashMap<String, String>,
    pub by_stem: HashMap<String, Vec<String>>,
    pub head_scale: HashMap<String, f64>,
    pub mesh_scale: HashMap<(String, String), f64>,
}

/// `prefab_stem(prefab)`.
pub fn prefab_stem(prefab: &str) -> String {
    let p = prefab.replace('\\', "/");
    let name = p.rsplit('/').next().unwrap_or_default();
    if name.to_lowercase().ends_with(".binfab") {
        let n: Vec<char> = name.chars().collect();
        n[..n.len().saturating_sub(7)].iter().collect()
    } else {
        name.to_owned()
    }
}

static STYLE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[[^\]]*\]").expect("style regex"));
static TIER_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"_lvl\d+").expect("tier regex"));

/// The scales `rig_index` answers with: the head scale and each part's multiplier.
#[derive(Clone, Debug, PartialEq)]
pub struct Scales {
    pub head: f64,
    pub parts: HashMap<String, f64>,
}

impl Default for Scales {
    fn default() -> Self {
        Scales { head: 1.0, parts: HashMap::new() }
    }
}

impl RigMap {
    /// `_build(rows)`.
    pub fn build(rows: &[Row]) -> RigMap {
        let mut m = RigMap::default();
        for (prefab, blueprint, skeleton, ap, mesh, head) in rows {
            m.by_blueprint.entry(blueprint.clone()).or_insert_with(|| (skeleton.clone(), ap.clone()));
            m.owner.entry(blueprint.clone()).or_insert_with(|| prefab.clone());
            if !m.creatures.contains_key(prefab) {
                m.creatures.insert(prefab.clone(), (skeleton.clone(), Vec::new()));
                m.by_path.insert(prefab.replace('\\', "/").to_lowercase(), prefab.clone());
                m.by_stem.entry(prefab_stem(prefab).to_lowercase()).or_default().push(prefab.clone());
                m.head_scale.insert(prefab.clone(), *head);
            }
            if let Some((_, parts)) = m.creatures.get_mut(prefab) {
                match parts.iter_mut().find(|(b, _)| b == blueprint) {
                    Some(slot) => slot.1 = ap.clone(),
                    None => parts.push((blueprint.clone(), ap.clone())),
                }
            }
            if *mesh != 1.0 {
                m.mesh_scale.insert((prefab.clone(), blueprint.clone()), *mesh);
            }
        }
        m
    }

    pub fn is_empty(&self) -> bool {
        self.by_blueprint.is_empty()
    }

    /// `_match_variant(basename)`.
    fn match_variant(&self, basename: &str) -> Option<String> {
        let mut seen: HashSet<String> = HashSet::from([basename.to_owned()]);
        let styled = STYLE_RE.replace_all(basename, "").into_owned();
        let cands = [
            styled.clone(),
            TIER_RE.replace_all(basename, "").into_owned(),
            TIER_RE.replace_all(&styled, "").into_owned(),
        ];
        for cand in cands {
            let cand = crate::pystr::strip(&cand).trim_matches('_').to_owned();
            if cand.is_empty() || seen.contains(&cand) {
                continue;
            }
            seen.insert(cand.clone());
            if self.by_blueprint.contains_key(&cand) {
                return Some(cand);
            }
        }
        None
    }

    /// `_known(basename)`.
    pub fn known(&self, basename: &str) -> Option<String> {
        if self.by_blueprint.contains_key(basename) { Some(basename.to_owned()) } else { self.match_variant(basename) }
    }

    /// `resolve(part_basenames)`: the winning skeleton and its parts' APs.
    pub fn resolve(&self, basenames: &[String]) -> (Option<String>, Vec<(String, String)>) {
        if self.is_empty() {
            return (None, Vec::new());
        }
        let mut hits: Vec<(String, (String, String))> = Vec::new();
        for b in basenames {
            if let Some(key) = self.known(b) {
                let hit = self.by_blueprint[&key].clone();
                match hits.iter_mut().find(|(k, _)| k == b) {
                    Some(slot) => slot.1 = hit,
                    None => hits.push((b.clone(), hit)),
                }
            }
        }
        if hits.is_empty() {
            return (None, Vec::new());
        }
        let mut counts: Vec<(String, usize)> = Vec::new();
        for (_, (skel, _)) in &hits {
            match counts.iter_mut().find(|(s, _)| s == skel) {
                Some(c) => c.1 += 1,
                None => counts.push((skel.clone(), 1)),
            }
        }
        let best = counts.iter().map(|(_, n)| *n).max().unwrap_or(0);
        let skeleton = counts.iter().find(|(_, n)| *n == best).map(|(s, _)| s.clone()).unwrap_or_default();
        let attach = hits.into_iter().filter(|(_, (s, _))| *s == skeleton).map(|(b, (_, ap))| (b, ap)).collect();
        (Some(skeleton), attach)
    }

    /// `creature_by_prefab(prefab)`.
    pub fn creature_by_prefab(&self, prefab: &str) -> (Option<String>, Vec<(String, String)>) {
        match self.creatures.get(&prefab.replace('\\', "/")) {
            Some((skel, parts)) => (Some(skel.clone()), parts.clone()),
            None => (None, Vec::new()),
        }
    }

    /// `_scales(rig_map, skeleton, attach, owners)`.
    fn scales(&self, rig: Option<&Rig>, attach: &[(String, String)], owners: &[(String, (String, String))]) -> Scales {
        let mut heads: Vec<(u64, f64, usize)> = Vec::new();
        for (_, (prefab, _)) in owners {
            let h = self.head_scale.get(prefab).copied().unwrap_or(1.0);
            match heads.iter_mut().find(|(k, _, _)| *k == scale_key(h)) {
                Some(slot) => slot.2 += 1,
                None => heads.push((scale_key(h), h, 1)),
            }
        }
        let mut parts = HashMap::new();
        for (b, (prefab, key)) in owners {
            if let Some((_, ap)) = attach.iter().find(|(k, _)| k == b) {
                let head = self.head_scale.get(prefab).copied().unwrap_or(1.0);
                let mesh = self.mesh_scale.get(&(prefab.clone(), key.clone())).copied().unwrap_or(1.0);
                parts.insert(b.clone(), scale_for(ap, rig, head, mesh));
            }
        }
        let best = heads.iter().map(|h| h.2).max().unwrap_or(0);
        let head = heads.iter().find(|h| h.2 == best).map_or(1.0, |h| h.1);
        Scales { head, parts }
    }

    /// `scales_for(attach, skeleton)`.
    pub fn scales_for(&self, attach: &[(String, String)], rig: Option<&Rig>) -> Scales {
        let mut owners = Vec::new();
        for (b, _) in attach {
            if let Some(key) = self.known(b)
                && let Some(prefab) = self.owner.get(&key)
            {
                owners.push((b.clone(), (prefab.clone(), key)));
            }
        }
        self.scales(rig, attach, &owners)
    }

    /// `creature_scales(prefab)`; `rig_for` maps the skeleton to its baked rig.
    pub fn creature_scales<'a>(&self, prefab: &str, rig_for: impl Fn(&str) -> Option<&'a Rig>) -> Scales {
        let prefab = prefab.replace('\\', "/");
        let Some((skel, parts)) = self.creatures.get(&prefab) else { return Scales::default() };
        let owners: Vec<(String, (String, String))> =
            parts.iter().map(|(b, _)| (b.clone(), (prefab.clone(), b.clone()))).collect();
        self.scales(rig_for(skel), parts, &owners)
    }

    /// `prefab_path(name)`: the canonical prefab, or the candidates a bare stem names.
    pub fn prefab_path(&self, name: &str) -> (Option<String>, Vec<String>) {
        if self.creatures.is_empty() {
            return (None, Vec::new());
        }
        let mut want = crate::pystr::strip(&name.replace('\\', "/")).trim_start_matches('/').to_lowercase();
        if want.is_empty() {
            return (None, Vec::new());
        }
        if !want.ends_with(".binfab") {
            want.push_str(".binfab");
        }
        if let Some(found) = self.by_path.get(&want) {
            return (Some(found.clone()), Vec::new());
        }
        if want.contains('/') {
            return (None, Vec::new());
        }
        let stem = &want[..want.len() - 7];
        let candidates = self.by_stem.get(stem).cloned().unwrap_or_default();
        if candidates.len() == 1 {
            return (Some(candidates[0].clone()), Vec::new());
        }
        let mut c = candidates;
        c.sort();
        (None, c)
    }

    /// `creature_for(basename)`: `(skeleton, prefab, parts)`.
    pub fn creature_for(&self, basename: &str) -> Option<Creature> {
        let prefab = self.owner.get(basename)?;
        let (skel, parts) = self.creatures.get(prefab)?;
        Some((skel.clone(), prefab.clone(), parts.clone()))
    }
}

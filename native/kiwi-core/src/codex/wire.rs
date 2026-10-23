//! `app/trove/codexes/binfab.py`: the uleb readers, the identity component,
//! the de-phantomed field run, the rig map, collection tables and locale tables.

use std::collections::HashMap;

use super::{PyErr, PyResult, py};
use crate::binfab::harvest_strings;

/// `read_uleb` (raises at the end of the data or past 10 bytes).
pub fn read_uleb(data: &[u8], mut pos: usize) -> PyResult<(u128, usize)> {
    let mut result: u128 = 0;
    let mut shift = 0u32;
    loop {
        let Some(&byte) = data.get(pos) else {
            return Err(PyErr("index out of range".into()));
        };
        pos += 1;
        result |= u128::from(byte & 0x7F) << shift;
        if byte & 0x80 == 0 {
            return Ok((result, pos));
        }
        shift += 7;
        if shift > 63 {
            return Err(PyErr("uleb128 too long".into()));
        }
    }
}

/// `read_varint`: `None` (and the offset back) on a truncated or overlong run.
pub fn read_varint(data: &[u8], offset: usize) -> (Option<u128>, usize) {
    let mut value: u128 = 0;
    let mut shift = 0u32;
    let mut cursor = offset;
    while cursor < data.len() && shift <= 63 {
        let byte = data[cursor];
        value |= u128::from(byte & 0x7F) << shift;
        cursor += 1;
        if byte & 0x80 == 0 {
            return (Some(value), cursor);
        }
        shift += 7;
    }
    (None, offset)
}

pub fn unzig(value: u128) -> i128 {
    ((value >> 1) as i128) ^ -((value & 1) as i128)
}

/// `content_start`: past a `<fmt> 00 <uleb len>` header that spans the file.
pub fn content_start(data: &[u8]) -> usize {
    if data.len() >= 4
        && data[1] == 0
        && let Ok((length, pos)) = read_uleb(data, 2)
        && pos as u128 + length == data.len() as u128
    {
        return pos;
    }
    0
}

#[derive(Clone, Debug, PartialEq)]
enum Val {
    Int(i128),
    Str(Option<String>),
}

/// One marker-delimited component: `(kind, field) -> value`, first wins.
#[derive(Default)]
struct Block(HashMap<(u8, u128), Val>);

impl Block {
    fn set(&mut self, key: (u8, u128), v: Val) {
        self.0.entry(key).or_insert(v);
    }

    fn s(&self, field: u128) -> Option<&str> {
        match self.0.get(&(b's', field)) {
            Some(Val::Str(Some(s))) => Some(s),
            _ => None,
        }
    }

    fn v(&self, field: u128) -> Option<i128> {
        match self.0.get(&(b'v', field)) {
            Some(Val::Int(i)) => Some(*i),
            _ => None,
        }
    }
}

fn marker_blocks(data: &[u8]) -> PyResult<Vec<Block>> {
    let mut pos = content_start(data);
    let n = data.len();
    let mut blocks: Vec<Block> = Vec::new();
    let mut open = false;
    while pos < n {
        let Ok((key, p)) = read_uleb(data, pos) else { break };
        pos = p;
        let (field, wt) = (key >> 4, key & 0xF);
        match wt {
            0 | 2 => {
                let (value, p) = match read_uleb(data, pos) {
                    Ok(v) => v,
                    Err(e) if e.0 == "uleb128 too long" => return Err(e),
                    Err(_) => break,
                };
                pos = p;
                if open {
                    let v = if wt == 2 { unzig(value) } else { value as i128 };
                    blocks.last_mut().expect("open block").set((b'v', field), Val::Int(v));
                }
            }
            4 => pos += 4,
            6 => pos += 8,
            8 => {
                let (length, p) = match read_uleb(data, pos) {
                    Ok(v) => v,
                    Err(e) if e.0 == "uleb128 too long" => return Err(e),
                    Err(_) => break,
                };
                pos = p;
                if pos as u128 + length > n as u128 {
                    break;
                }
                let raw = &data[pos..pos + length as usize];
                pos += length as usize;
                if open {
                    let text =
                        (!raw.is_empty() && raw.iter().all(|&b| (32..127).contains(&b))).then(|| py::latin1(raw));
                    blocks.last_mut().expect("open block").set((b's', field), Val::Str(text));
                }
            }
            _ => {
                blocks.push(Block::default());
                open = true;
            }
        }
    }
    Ok(blocks)
}

/// `decode_identity`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Identity {
    pub name_key: Option<String>,
    pub category: Option<String>,
    pub desc_key: Option<String>,
    pub tradable: Option<bool>,
}

pub fn decode_identity(data: &[u8]) -> PyResult<Option<Identity>> {
    let blocks = marker_blocks(data)?;
    let Some(first) = blocks.first() else { return Ok(None) };
    let mut ident = first;
    if ident.s(1).is_none_or(str::is_empty) {
        ident = blocks.iter().find(|b| b.s(1).is_some_and(|s| s.starts_with('$'))).unwrap_or(ident);
    }
    if ident.0.is_empty() {
        return Ok(None);
    }
    Ok(Some(Identity {
        name_key: ident.s(1).map(str::to_owned),
        category: ident.s(2).map(str::to_owned),
        desc_key: ident.s(5).map(str::to_owned),
        tradable: ident.v(14).map(|t| t == 2),
    }))
}

/// `decode_identity(data)["flags"].get(field)`.
pub fn identity_flag(data: &[u8], field: u128) -> PyResult<Option<i128>> {
    let blocks = marker_blocks(data)?;
    let Some(first) = blocks.first() else { return Ok(None) };
    let mut ident = first;
    if ident.s(1).is_none_or(str::is_empty) {
        ident = blocks.iter().find(|b| b.s(1).is_some_and(|s| s.starts_with('$'))).unwrap_or(ident);
    }
    Ok(ident.v(field))
}

/// A harvested string with its framing: offset, field, text, payload start, length.
#[derive(Clone, Debug)]
pub struct Field {
    pub off: usize,
    pub field: u128,
    pub text: String,
    pub start: usize,
    pub len: usize,
}

/// `harvest_strings(data)` (2..=512 bytes) with payload positions.
pub fn harvest(data: &[u8]) -> Vec<Field> {
    harvest_strings(data, 2, 512)
        .into_iter()
        .map(|h| {
            let (_, j) = read_uleb(data, h.offset).expect("harvested key");
            let (len, k) = read_uleb(data, j).expect("harvested length");
            Field { off: h.offset, field: h.field, text: h.text, start: k, len: len as usize }
        })
        .collect()
}

/// `_real_fields`: drop the phantom strings that start inside a real one.
pub fn real_fields(data: &[u8]) -> Vec<Field> {
    let mut out = Vec::new();
    let mut last_end: Option<usize> = None;
    for f in harvest(data) {
        if last_end.is_none_or(|e| f.start >= e) {
            last_end = Some(f.start + f.len);
            out.push(f);
        }
    }
    out
}

/// `extract_rig_refs`.
#[derive(Clone, Debug, PartialEq)]
pub struct Rig {
    pub skeleton: String,
    /// blueprint basename -> AP key, in insertion order.
    pub parts: Vec<(String, String)>,
    pub refs: Vec<(String, String)>,
    pub scales: HashMap<String, f64>,
    pub head_scale: f64,
}

fn put(v: &mut Vec<(String, String)>, k: String, val: String) {
    match v.iter_mut().find(|(key, _)| *key == k) {
        Some(slot) => slot.1 = val,
        None => v.push((k, val)),
    }
}

pub fn extract_rig_refs(data: &[u8]) -> Option<Rig> {
    let rows = real_fields(data);
    let mut skeleton: Option<String> = None;
    let mut skel_off: Option<usize> = None;
    let (mut gsf_off, mut next_skel_off): (Option<usize>, Option<usize>) = (None, None);
    for r in &rows {
        let base = py::lower(py::last_part(&r.text, "/"));
        if let Some(stem) = base.strip_suffix(".skeleton.gr2") {
            if skeleton.is_none() {
                skeleton = Some(stem.to_owned());
                skel_off = Some(r.off);
            } else if next_skel_off.is_none() {
                next_skel_off = Some(r.off);
            }
        } else if gsf_off.is_none() && base.ends_with(".gsf") {
            gsf_off = Some(r.off);
        }
    }
    let (skeleton, skel_off) = (skeleton?, skel_off?);
    let end = [gsf_off, next_skel_off].into_iter().flatten().filter(|&o| o > skel_off).min().unwrap_or(data.len());

    let mut parts = Vec::new();
    let mut refs = Vec::new();
    let mut scales: Vec<(String, f64)> = Vec::new();
    let mut last_ap_end = skel_off;
    for w in rows.windows(2) {
        let (a, b) = (&w[0], &w[1]);
        if !(skel_off < a.off && a.off < end) {
            continue;
        }
        if a.field == 0 && b.field == 1 && b.text.starts_with("AP_") {
            let r = py::lower(&a.text.replace('\\', "/"));
            let base = py::last_part(&r, "/").to_owned();
            put(&mut parts, base.clone(), py::lower(&b.text[3..]));
            put(&mut refs, base.clone(), r);
            last_ap_end = b.start + b.len;
            let scale = mesh_scale(data, last_ap_end);
            match scales.iter_mut().find(|(k, _)| *k == base) {
                Some(slot) => slot.1 = scale,
                None => scales.push((base, scale)),
            }
        }
    }
    if parts.is_empty() {
        return None;
    }
    let stop = next_skel_off.filter(|&o| o != 0).unwrap_or(data.len());
    let head_scale = head_scale(data, last_ap_end, stop);
    Some(Rig { skeleton, parts, refs, scales: scales.into_iter().filter(|(_, v)| *v != 1.0).collect(), head_scale })
}

fn f32_scale(raw: &[u8]) -> f64 {
    let v = f64::from(f32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]));
    if v > 0.0 && v <= 16.0 { v } else { 1.0 }
}

fn mesh_scale(data: &[u8], mut pos: usize) -> f64 {
    let mut prev: u128 = 1;
    for _ in 0..4 {
        let (Some(key), nxt) = read_varint(data, pos) else { break };
        let (fld, wire) = (key >> 4, key & 0xF);
        if fld <= prev {
            break;
        }
        prev = fld;
        match wire {
            4 => {
                if nxt + 4 > data.len() {
                    break;
                }
                if fld == 3 {
                    return f32_scale(&data[nxt..nxt + 4]);
                }
                pos = nxt + 4;
            }
            8 => {
                let (Some(length), nxt) = read_varint(data, nxt) else { break };
                pos = nxt.saturating_add(usize::try_from(length).unwrap_or(usize::MAX));
            }
            0 => {
                let (Some(_), nxt) = read_varint(data, nxt) else { break };
                pos = nxt;
            }
            _ => break,
        }
    }
    1.0
}

const HEAD_SCALE_MARK: [u8; 4] = [0x1e, 0x1e, 0x08, 0x24];
const HEAD_SCALE_WINDOW: usize = 96;

fn head_scale(data: &[u8], start: usize, stop: usize) -> f64 {
    let end = stop.min(start + HEAD_SCALE_WINDOW).min(data.len());
    if start + 8 > end {
        return 1.0;
    }
    (start..=end - 8).find(|&i| data[i..i + 4] == HEAD_SCALE_MARK).map_or(1.0, |i| f32_scale(&data[i + 4..i + 8]))
}

/// `parse_collection_table`: `(id, members)` groups.
pub fn parse_collection_table(data: &[u8]) -> Vec<(String, Vec<String>)> {
    let mut groups: Vec<(String, Vec<String>)> = Vec::new();
    let mut prev_bare = String::new();
    for h in harvest_strings(data, 2, 512) {
        let s = h.text;
        if s.starts_with("$CollectionName") {
            let id = if prev_bare.is_empty() {
                s.strip_prefix("$CollectionName_").unwrap_or(&s).to_owned()
            } else {
                prev_bare.clone()
            };
            groups.push((id, Vec::new()));
        } else if s.starts_with("collections/") {
            if let Some(g) = groups.last_mut() {
                g.1.push(s);
            }
        } else if !s.contains('/') && !s.contains('$') {
            prev_bare = s;
        }
    }
    groups
}

/// `collection_category_map`: member path (lowercased) -> group id, first wins,
/// in insertion order.
pub fn collection_category_map(data: &[u8]) -> Vec<(String, String)> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for (id, members) in parse_collection_table(data) {
        for m in members {
            let key = py::lower(&m.replace('\\', "/"));
            if seen.insert(key.clone()) {
                out.push((key, id.clone()));
            }
        }
    }
    out
}

/// `clean_localized_text`.
pub fn clean_localized_text(text: &str) -> String {
    let mut t = text.replace("\\$", "$");
    if t.starts_with("$$") {
        t = format!("${}", py::lstrip_chars(&t, "$"));
    }
    let t = t.replace("\\n", "\n");
    let t = py::strip(py::strip_chars(&t, "`"));
    if !t.starts_with('$') && t.starts_with("\\\"") { t[1..].to_owned() } else { t.to_owned() }
}

/// `extract_localization_map`, folded into `out` (later keys win).
pub fn extract_localization_map(content: &[u8], out: &mut HashMap<String, String>) {
    let n = content.len();
    let mut cursor = 0;
    while let Some(start) = py::find(content, b"$", cursor) {
        let mut end = start + 1;
        while end < n {
            let b = content[end];
            if !(b.is_ascii_alphanumeric() || b == b'_' || b == b'$') {
                break;
            }
            end += 1;
        }
        if end >= n || content[end] != 0x18 {
            cursor = start + 1;
            continue;
        }
        let (Some(len), value_start) = read_varint(content, end + 1) else {
            cursor = start + 1;
            continue;
        };
        if value_start as u128 + len > n as u128 {
            cursor = start + 1;
            continue;
        }
        let value_end = value_start + len as usize;
        let key = clean_localized_text(&py::ascii_ignore(&content[start..end]));
        let value = clean_localized_text(&py::utf8_ignore(&content[value_start..value_end]));
        if key.starts_with('$') && !value.is_empty() && !value.starts_with('$') {
            out.insert(key, value);
        }
        cursor = value_end;
    }
}

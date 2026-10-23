//! The byte containers Python writes around a payload: `gzip.compress`, a
//! `zipfile.ZipFile(..., ZIP_DEFLATED)` built with `writestr`, and `KVX1`.

use serde_json::{Map, Value};

use kiwi_zlib::deflate_raw;

use crate::pyjson;

/// `gzip.compress(data, level, mtime=mtime)`.
pub fn gzip(data: &[u8], level: i32, mtime: u32) -> Option<Vec<u8>> {
    kiwi_zlib::gzip_compress(data, level, mtime)
}

/// `gzip.decompress`.
pub fn gunzip(data: &[u8]) -> Option<Vec<u8>> {
    kiwi_core::zlib::gunzip(data).ok()
}

/// `(year, month, day, hour, minute, second)`, as `time.localtime()[:6]`.
pub type DateTime6 = [u16; 6];

pub fn now_date_time() -> DateTime6 {
    use chrono::{Datelike, Timelike};
    let t = chrono::Utc::now();
    [t.year() as u16, t.month() as u16, t.day() as u16, t.hour() as u16, t.minute() as u16, t.second() as u16]
}

/// `mods_workshop.to_zip(files)`: `writestr` per entry at `when`.
pub fn zip(files: &[(String, Vec<u8>)], when: DateTime6) -> Option<Vec<u8>> {
    let [y, mo, d, h, mi, s] = when;
    let date = ((y.saturating_sub(1980)) << 9) | (mo << 5) | d;
    let time = (h << 11) | (mi << 5) | (s / 2);
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (path, data) in files {
        let name = path.split('\0').next().unwrap_or("");
        let flags: u16 = if name.is_ascii() { 0 } else { 0x800 };
        let packed = deflate_raw(data, -1)?;
        let crc = crc32fast::hash(data);
        let offset = out.len() as u32;
        let head = |sig: &[u8]| {
            let mut h = sig.to_vec();
            h.extend_from_slice(&[20, 0]);
            h.extend_from_slice(&flags.to_le_bytes());
            h.extend_from_slice(&8u16.to_le_bytes());
            h.extend_from_slice(&time.to_le_bytes());
            h.extend_from_slice(&date.to_le_bytes());
            h.extend_from_slice(&crc.to_le_bytes());
            h.extend_from_slice(&(packed.len() as u32).to_le_bytes());
            h.extend_from_slice(&(data.len() as u32).to_le_bytes());
            h.extend_from_slice(&(name.len() as u16).to_le_bytes());
            h.extend_from_slice(&0u16.to_le_bytes());
            h
        };
        out.extend(head(b"PK\x03\x04"));
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(&packed);
        central.extend_from_slice(b"PK\x01\x02");
        central.extend_from_slice(&[20, 3]);
        central.extend(&head(b"")[..]);
        central.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
        central.extend_from_slice(&(0o600u32 << 16).to_le_bytes());
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name.as_bytes());
    }
    let start = out.len() as u32;
    let count = files.len() as u16;
    out.extend_from_slice(&central);
    out.extend_from_slice(b"PK\x05\x06\0\0\0\0");
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&(central.len() as u32).to_le_bytes());
    out.extend_from_slice(&start.to_le_bytes());
    out.extend_from_slice(&[0, 0]);
    Some(out)
}

pub const KVX_MAGIC: &[u8; 4] = b"KVX1";
const ARRAYS: [(&str, &str); 7] =
    [("x", "i16"), ("y", "i16"), ("z", "i16"), ("rgb", "u32"), ("kind", "u8"), ("level", "u8"), ("spec", "u8")];

/// `binpack._pack`: `None` when a value doesn't fit the type.
fn pack_array(values: &[Value], dtype: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(values.len() * 4);
    for v in values {
        let n = match v {
            Value::Bool(b) => i64::from(*b),
            Value::Number(n) => n.as_i64()?,
            _ => return None,
        };
        match dtype {
            "i16" => out.extend_from_slice(&i16::try_from(n).ok()?.to_le_bytes()),
            "u32" => out.extend_from_slice(&u32::try_from(n).ok()?.to_le_bytes()),
            _ => out.push(u8::try_from(n).ok()?),
        }
    }
    Some(out)
}

/// `binpack.encode(payload)`.
pub fn kvx_encode(payload: &Map<String, Value>) -> Vec<u8> {
    let mut header = payload.clone();
    let mut arrays: Vec<(Vec<String>, Vec<Value>, &str)> = Vec::new();
    let is_array = |k: &str| ARRAYS.iter().any(|(a, _)| *a == k);
    if let Some(Value::Array(parts)) = payload.get("parts") {
        let mut stripped = Vec::with_capacity(parts.len());
        for (i, part) in parts.iter().enumerate() {
            let Value::Object(p) = part else {
                stripped.push(part.clone());
                continue;
            };
            stripped.push(Value::Object(
                p.iter().filter(|(k, _)| !is_array(k)).map(|(k, v)| (k.clone(), v.clone())).collect(),
            ));
            for (name, dtype) in ARRAYS {
                if let Some(Value::Array(a)) = p.get(name) {
                    arrays.push((vec!["parts".into(), i.to_string(), name.into()], a.clone(), dtype));
                }
            }
        }
        header.insert("parts".into(), Value::Array(stripped));
    } else {
        header.retain(|k, _| !is_array(k));
        for (name, dtype) in ARRAYS {
            if let Some(Value::Array(a)) = payload.get(name) {
                arrays.push((vec![name.into()], a.clone(), dtype));
            }
        }
    }
    let mut body = Vec::new();
    let mut index = Map::new();
    for (path, values, dtype) in arrays {
        let Some(raw) = pack_array(&values, dtype) else {
            set_path(&mut header, &path, Value::Array(values));
            continue;
        };
        index.insert(path.join("."), serde_json::json!([body.len(), values.len(), dtype]));
        body.extend_from_slice(&raw);
        body.resize(body.len().next_multiple_of(4), 0);
    }
    if !index.is_empty() {
        header.insert("_bin".into(), Value::Object(index));
    }
    let head = pyjson::dumps_compact_ascii(&Value::Object(header));
    let mut out = KVX_MAGIC.to_vec();
    out.extend_from_slice(&(head.len() as u32).to_le_bytes());
    out.extend_from_slice(head.as_bytes());
    out.resize(out.len().next_multiple_of(4), 0);
    out.extend_from_slice(&body);
    out
}

/// `binpack._set_path`: numeric segments index lists.
fn set_path(obj: &mut Map<String, Value>, path: &[String], value: Value) {
    let Some((last, parents)) = path.split_last() else { return };
    let Some((first, rest)) = parents.split_first() else {
        obj.insert(last.clone(), value);
        return;
    };
    let Some(mut cur) = obj.get_mut(first) else { return };
    for seg in rest {
        let next = match cur {
            Value::Array(a) => seg.parse::<usize>().ok().and_then(|i| a.get_mut(i)),
            Value::Object(o) => o.get_mut(seg),
            _ => None,
        };
        let Some(next) = next else { return };
        cur = next;
    }
    match cur {
        Value::Object(o) => {
            o.insert(last.clone(), value);
        }
        Value::Array(a) => {
            if let Some(slot) = last.parse::<usize>().ok().and_then(|i| a.get_mut(i)) {
                *slot = value;
            }
        }
        _ => {}
    }
}

/// `binpack.decode(blob)`: the plain payload again.
pub fn kvx_decode(blob: &[u8]) -> Option<Value> {
    if blob.get(..4)? != KVX_MAGIC {
        return None;
    }
    let len = u32::from_le_bytes(blob.get(4..8)?.try_into().ok()?) as usize;
    let mut header: Value = serde_json::from_slice(blob.get(8..8 + len)?).ok()?;
    let body = (8 + len).next_multiple_of(4);
    let index = header.as_object_mut()?.shift_remove("_bin");
    for (path, spec) in index.as_ref().and_then(Value::as_object).into_iter().flatten() {
        let (off, count, dtype) = (spec[0].as_u64()? as usize, spec[1].as_u64()? as usize, spec[2].as_str()?);
        let start = body + off;
        let values: Vec<Value> = match dtype {
            "i16" => {
                blob.get(start..start + 2 * count)?.chunks(2).map(|c| i16::from_le_bytes([c[0], c[1]]).into()).collect()
            }
            "u32" => blob
                .get(start..start + 4 * count)?
                .chunks(4)
                .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]).into())
                .collect(),
            _ => blob.get(start..start + count)?.iter().map(|&b| b.into()).collect(),
        };
        let segs: Vec<String> = path.split('.').map(str::to_owned).collect();
        set_path(header.as_object_mut()?, &segs, Value::Array(values));
    }
    Some(header)
}

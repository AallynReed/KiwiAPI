//! `.tmod` reading for `app/trove/tmod.py`: header table, inflate or de-frame
//! the file stream, checksum it and base64 each file.

use crate::troveio::calculate_hash;

const CHUNK: usize = 32768;

pub struct Entry {
    pub path: String,
    pub index: u32,
    pub offset: u32,
    pub size: u32,
    pub checksum: u32,
}

pub struct Header {
    pub properties: Vec<(String, String)>,
    pub entries: Vec<Entry>,
    pub repaired: bool,
}

/// `read_leb128`: masked to 32 bits, too-long varints rejected.
fn leb128(d: &[u8], pos: &mut usize) -> Option<u32> {
    let mut out: u128 = 0;
    let mut shift = 0;
    loop {
        let b = *d.get(*pos)?;
        *pos += 1;
        out |= ((b & 0x7F) as u128) << shift;
        if b & 0x80 == 0 {
            return Some(out as u32);
        }
        shift += 7;
        if shift >= 64 {
            return None;
        }
    }
}

fn char_span(d: &[u8], pos: usize, chars: u32) -> Option<usize> {
    let mut end = pos;
    for _ in 0..chars {
        end += match *d.get(end)? {
            0x00..=0x7F => 1,
            0xC2..=0xDF => 2,
            0xE0..=0xEF => 3,
            0xF0..=0xF4 => 4,
            _ => return None,
        };
    }
    (end <= d.len()).then_some(end - pos)
}

type Parsed = (Vec<(String, String)>, Vec<Entry>);

fn parse(d: &[u8], header_size: usize, prop_count: u16, chars: bool) -> Option<Parsed> {
    let string = |pos: &mut usize, declared: u32| -> Option<String> {
        let span = if chars { char_span(d, *pos, declared)? } else { declared as usize };
        let s = std::str::from_utf8(d.get(*pos..*pos + span)?).ok()?.to_owned();
        *pos += span;
        Some(s)
    };
    let mut pos = 12;
    let mut properties = Vec::with_capacity(prop_count as usize);
    for _ in 0..prop_count {
        let n = leb128(d, &mut pos)?;
        let name = string(&mut pos, n)?;
        let n = leb128(d, &mut pos)?;
        properties.push((name, string(&mut pos, n)?));
    }
    let mut entries = Vec::new();
    while pos < header_size {
        let n = *d.get(pos)? as u32;
        pos += 1;
        let path = string(&mut pos, n)?;
        let index = leb128(d, &mut pos)?;
        let offset = leb128(d, &mut pos)?;
        let size = leb128(d, &mut pos)?;
        let checksum = leb128(d, &mut pos)?;
        entries.push(Entry { path, index, offset, size, checksum });
    }
    (pos == header_size).then_some((properties, entries))
}

/// `_read_header`'s success path: byte lengths first, then character lengths.
/// None when neither reading ends on `header_size` (Python reports why).
pub fn header(d: &[u8], header_size: usize, prop_count: u16) -> Option<Header> {
    [false, true].into_iter().find_map(|chars| {
        parse(d, header_size, prop_count, chars)
            .map(|(properties, entries)| Header { properties, entries, repaired: chars })
    })
}

fn deframe(data: &[u8]) -> Vec<u8> {
    let buf = data.get(7..data.len().saturating_sub(5)).unwrap_or(&[]);
    let mut out = Vec::with_capacity(buf.len());
    let mut pos = 0;
    for _ in 0..buf.len() / (CHUNK + 5) {
        out.extend_from_slice(&buf[pos..pos + CHUNK]);
        pos += CHUNK + 5;
    }
    out.extend_from_slice(&buf[pos.min(buf.len())..]);
    out
}

fn verify(stream: &[u8], entries: &[Entry]) -> bool {
    let mut checked = 0;
    for e in entries.iter().filter(|e| e.size != 0) {
        let end = e.offset as usize + e.size as usize;
        if end > stream.len() || calculate_hash(&stream[e.offset as usize..end]) != e.checksum {
            return false;
        }
        checked += 1;
        if checked >= 4 {
            return true;
        }
    }
    true
}

/// `_decompress_file_stream`: `primary` is zlib's output (None if it raised),
/// `compressed` the raw stream for the stored-block de-framer.
fn stream<'a>(primary: Option<&'a [u8]>, compressed: &[u8], entries: &[Entry]) -> std::borrow::Cow<'a, [u8]> {
    if let Some(p) = primary.filter(|p| verify(p, entries)) {
        return p.into();
    }
    let manual = deframe(compressed);
    match primary {
        Some(p) if !verify(&manual, entries) => p.into(),
        _ => manual.into(),
    }
}

fn put_leb128(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v & 0x7F) as u8 | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

/// `build_tmod` after its checks: header, file table and the level-0 stream of
/// 4-byte-padded contents. `files` paths are already normalized, ≤ 255 bytes.
pub fn pack(version: u16, properties: &[(&str, &str)], files: &[(&str, &[u8])]) -> Option<Vec<u8>> {
    let total: usize = files.iter().map(|(_, c)| c.len().next_multiple_of(4)).sum();
    let mut stream = Vec::with_capacity(total);
    let mut table = Vec::new();
    for (path, content) in files {
        let size = content.len();
        table.push(u8::try_from(path.len()).ok()?);
        table.extend_from_slice(path.as_bytes());
        put_leb128(&mut table, 0);
        put_leb128(&mut table, if size != 0 { stream.len() as u64 } else { 0 });
        put_leb128(&mut table, size as u64);
        put_leb128(&mut table, calculate_hash(content) as u64);
        stream.extend_from_slice(content);
        stream.resize(stream.len().next_multiple_of(4), 0);
    }
    let compressed = crate::zlib::deflate(&stream, 0, CHUNK, true)?;
    let mut out = vec![0u8; 8];
    out.extend_from_slice(&version.to_le_bytes());
    out.extend_from_slice(&u16::try_from(properties.len()).ok()?.to_le_bytes());
    for (name, value) in properties {
        put_leb128(&mut out, name.len() as u64);
        out.extend_from_slice(name.as_bytes());
        put_leb128(&mut out, value.len() as u64);
        out.extend_from_slice(value.as_bytes());
    }
    out.extend_from_slice(&table);
    let header = out.len() as u64;
    out[..8].copy_from_slice(&header.to_le_bytes());
    out.extend_from_slice(&compressed);
    Some(out)
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn base64(data: &[u8]) -> String {
    let mut out = Vec::with_capacity(data.len().div_ceil(3) * 4);
    let mut chunks = data.chunks_exact(3);
    for c in &mut chunks {
        let n = (c[0] as u32) << 16 | (c[1] as u32) << 8 | c[2] as u32;
        out.extend_from_slice(&[B64[(n >> 18) as usize], B64[(n >> 12 & 63) as usize], B64[(n >> 6 & 63) as usize], B64[(n & 63) as usize]]);
    }
    match *chunks.remainder() {
        [a] => out.extend_from_slice(&[B64[(a >> 2) as usize], B64[((a & 3) << 4) as usize], b'=', b'=']),
        [a, b] => {
            let n = (a as u32) << 8 | b as u32;
            out.extend_from_slice(&[B64[(n >> 10) as usize], B64[(n >> 4 & 63) as usize], B64[((n & 15) << 2) as usize], b'=']);
        }
        _ => {}
    }
    String::from_utf8(out).unwrap_or_default()
}

/// Each entry's content as base64, sliced the way Python slices (clamped).
pub fn contents(compressed: &[u8], entries: &[Entry]) -> Vec<String> {
    let primary = crate::zlib::inflate(compressed, usize::MAX).map(|z| z.data);
    let s = stream(primary.as_deref(), compressed, entries);
    entries
        .iter()
        .map(|e| {
            let start = (e.offset as usize).min(s.len());
            let end = (e.offset as usize + e.size as usize).min(s.len()).max(start);
            base64(&s[start..end])
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_rfc4648() {
        for (i, o) in [("", ""), ("f", "Zg=="), ("fo", "Zm8="), ("foo", "Zm9v"), ("foobar", "Zm9vYmFy")] {
            assert_eq!(base64(i.as_bytes()), o);
        }
    }

    #[test]
    fn leb128_masks_and_rejects() {
        let mut p = 0;
        assert_eq!(leb128(&[0xE5, 0x8E, 0x26], &mut p), Some(624485));
        let mut p = 0;
        assert_eq!(leb128(&[0x80; 10], &mut p), None);
    }
}

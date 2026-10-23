//! `app/trove/swf/extract.py`: the movie container, the bitmap tags decoded
//! the way Pillow decodes them, symbol names walked back through shapes and
//! sprites, and the inventory of everything else.

use std::{
    borrow::Cow,
    collections::{HashMap, HashSet, VecDeque},
};

use kiwi_core::swf as core;

use crate::media::pil;

pub const MAX_INFLATED: usize = 256 * 1024 * 1024;
pub const MAX_IMAGES: usize = 4000;
pub const MAX_PIXELS: u64 = 8192 * 8192;
pub const THUMB_BOX: usize = 128;

const BITMAP_TAGS: [u16; 6] = [6, 20, 21, 35, 36, 90];
const PLACE_TAGS: [u16; 3] = [4, 26, 70];

pub const INVENTORY: [(&str, &[u16]); 10] = [
    ("shapes", &[2, 22, 32, 83]),
    ("sprites", &[39]),
    ("fonts", &[10, 48, 75, 91]),
    ("texts", &[11, 33, 37]),
    ("sounds", &[14, 18, 45]),
    ("morphs", &[46, 84]),
    ("buttons", &[7, 34]),
    ("videos", &[60]),
    ("binary", &[87]),
    ("scripts", &[12, 59, 82]),
];

fn shape_version(code: u16) -> Option<u32> {
    match code {
        2 => Some(1),
        22 => Some(2),
        32 => Some(3),
        83 => Some(4),
        _ => None,
    }
}

/// `SwfError` (the container can't be read) or anything else Python would
/// raise out of `extract_images` (an unhandled 500).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fail {
    Swf(String),
    Crash(String),
}

fn swf_err(m: impl Into<String>) -> Fail {
    Fail::Swf(m.into())
}

#[derive(Debug, Clone, PartialEq)]
pub struct Header {
    pub version: u8,
    pub compression: &'static str,
    pub width: i64,
    pub height: i64,
    pub frame_rate: f64,
    pub frame_count: u16,
}

pub struct Image {
    pub char_id: u16,
    pub name: Option<String>,
    pub source: &'static str,
    pub codec: String,
    pub width: usize,
    pub height: usize,
    pub data: Vec<u8>,
    pub mime: &'static str,
    pub thumb: Option<Vec<u8>>,
}

/// Python's `repr(bytes)`.
pub fn bytes_repr(b: &[u8]) -> String {
    let quote = if b.contains(&b'\'') && !b.contains(&b'"') { '"' } else { '\'' };
    let mut s = String::from("b");
    s.push(quote);
    for &c in b {
        match c {
            b'\\' => s.push_str("\\\\"),
            b'\t' => s.push_str("\\t"),
            b'\n' => s.push_str("\\n"),
            b'\r' => s.push_str("\\r"),
            c if c as char == quote => {
                s.push('\\');
                s.push(quote);
            }
            0x20..=0x7e => s.push(c as char),
            _ => s.push_str(&format!("\\x{c:02x}")),
        }
    }
    s.push(quote);
    s
}

/// CPython's `zlib.error` text for a failed `decompress`.
fn zlib_message(e: &flate2::DecompressError) -> String {
    format!("Error -3 while decompressing data: {}", e.message().unwrap_or("invalid input data"))
}

fn cws_fallback(body: &[u8]) -> Result<Vec<u8>, Fail> {
    let mut d = flate2::Decompress::new(true);
    let mut out = Vec::with_capacity(body.len().saturating_mul(3).min(MAX_INFLATED));
    loop {
        let before = (d.total_in(), d.total_out());
        if out.len() == out.capacity() {
            if out.len() >= MAX_INFLATED {
                break;
            }
            out.reserve(out.len().clamp(4096, MAX_INFLATED - out.len()));
        }
        let at = d.total_in() as usize;
        match d.decompress_vec(&body[at..], &mut out, flate2::FlushDecompress::None) {
            Ok(flate2::Status::StreamEnd) => return Ok(out),
            Ok(_) => {}
            Err(e) => return Err(swf_err(format!("bad zlib body: {}", zlib_message(&e)))),
        }
        if (d.total_in(), d.total_out()) == before && out.len() < out.capacity() {
            return Ok(out);
        }
    }
    if (d.total_in() as usize) < body.len() {
        return Err(swf_err("decompression bomb"));
    }
    Ok(out)
}

fn lzma_body(raw: &[u8]) -> Result<Vec<u8>, Fail> {
    if raw.len() < 17 {
        return Err(swf_err("truncated lzma header"));
    }
    let props = raw[12];
    let dict = u32::from_le_bytes([raw[13], raw[14], raw[15], raw[16]]);
    // liblzma's texts: bad properties, else any decode failure.
    let mut s = lzma_rust2::LzmaStream::new_with_props(u64::MAX, props, dict, None)
        .map_err(|_| swf_err("bad lzma body: Invalid or unsupported options"))?;
    let bad = |_: &dyn std::fmt::Display| swf_err("bad lzma body: Corrupt input data");
    let input = &raw[17..];
    if input.first().is_some_and(|&b| b != 0) {
        return Err(swf_err("bad lzma body: Corrupt input data"));
    }
    let mut out = Vec::new();
    let mut buf = vec![0u8; 1 << 16];
    let mut pos = 0;
    while out.len() < MAX_INFLATED {
        let r = s.process(&input[pos..], &mut buf, lzma_rust2::Action::Run).map_err(|e| bad(&e))?;
        pos += r.bytes_consumed;
        let take = r.bytes_produced.min(MAX_INFLATED - out.len());
        out.extend_from_slice(&buf[..take]);
        if r.status == lzma_rust2::Status::StreamEnd || (r.bytes_consumed == 0 && r.bytes_produced == 0) {
            break;
        }
    }
    Ok(out)
}

/// `_decompress`: the movie with an inflated body and an `FWS` header.
pub fn decompress(raw: &[u8]) -> Result<Cow<'_, [u8]>, Fail> {
    if raw.len() < 8 {
        return Err(swf_err("truncated header"));
    }
    let mut head = b"FWS".to_vec();
    head.extend_from_slice(&raw[3..8]);
    match &raw[..3] {
        b"FWS" => Ok(Cow::Borrowed(raw)),
        b"CWS" => {
            if let Some(m) = core::movie(raw) {
                return Ok(m);
            }
            head.extend_from_slice(&cws_fallback(&raw[8..])?);
            Ok(Cow::Owned(head))
        }
        b"ZWS" => {
            head.extend_from_slice(&lzma_body(raw)?);
            Ok(Cow::Owned(head))
        }
        sig => Err(swf_err(format!("not a SWF (signature {})", bytes_repr(sig)))),
    }
}

/// `read_header`.
pub fn read_header(raw: &[u8]) -> Result<Header, Fail> {
    let data = decompress(raw)?;
    if data.len() < 9 {
        return Err(swf_err("truncated header"));
    }
    let nbits = u32::from(data[8] >> 3);
    let span = (5 + nbits as usize * 4).div_ceil(8);
    let rect = &data[8..(8 + span).min(data.len())];
    let mut bits = core::BitReader::new(rect);
    let overrun = || swf_err("bit overrun");
    bits.read(5).ok_or_else(overrun)?;
    let mut v = [0i64; 4];
    for x in &mut v {
        *x = bits.read_signed(nbits).ok_or_else(overrun)?;
    }
    let [xmin, xmax, ymin, ymax] = v;
    let tail = data.get(8 + span..12 + span).ok_or_else(|| swf_err("truncated header"))?;
    Ok(Header {
        version: data[3],
        compression: match &raw[..3] {
            b"CWS" => "zlib",
            b"ZWS" => "lzma",
            _ => "none",
        },
        width: (xmax - xmin).max(0).div_euclid(20),
        height: (ymax - ymin).max(0).div_euclid(20),
        frame_rate: f64::from(tail[1]) + f64::from(tail[0]) / 256.0,
        frame_count: u16::from_le_bytes([tail[2], tail[3]]),
    })
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes([*b.get(at)?, *b.get(at + 1)?]))
}

/// `_iter_sprite_tags`: the nested body may come up short; the End tag stops.
pub(crate) fn sprite_tags(body: &[u8]) -> Vec<(u16, &[u8])> {
    let mut out = Vec::new();
    let b = body.get(4..).unwrap_or(&[]);
    let mut p = 0;
    loop {
        let Some(packed) = u16_at(b, p) else { return out };
        p += 2;
        let code = packed >> 6;
        let mut len = usize::from(packed & 0x3F);
        if len == 0x3F {
            let Some(raw) = b.get(p..p + 4) else { return out };
            p += 4;
            len = u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]) as usize;
        }
        let nested = &b[p.min(b.len())..(p.saturating_add(len)).min(b.len())];
        p = p.saturating_add(len);
        if code == 0 {
            return out;
        }
        out.push((code, nested));
    }
}

/// `_sprite_child_refs`; Err where Python's `struct.unpack` raises.
fn sprite_child_refs(body: &[u8]) -> Result<Vec<u16>, Fail> {
    let mut refs = Vec::new();
    let crash = || Fail::Crash("unpack requires a buffer of 2 bytes".into());
    for (code, nested) in sprite_tags(body) {
        if !PLACE_TAGS.contains(&code) || nested.len() < 4 {
            continue;
        }
        match code {
            4 => refs.push(u16_at(nested, 0).ok_or_else(crash)?),
            26 if nested[0] & 0x02 != 0 => refs.push(u16_at(nested, 3).ok_or_else(crash)?),
            70 if nested[0] & 0x02 != 0 => {
                let mut off = 4;
                if nested[1] & 0x08 != 0 || nested[1] & 0x10 != 0 {
                    off = match nested[off..].iter().position(|&c| c == 0) {
                        Some(i) => off + i + 1,
                        None => nested.len(),
                    };
                }
                if off + 2 <= nested.len() {
                    refs.push(u16_at(nested, off).ok_or_else(crash)?);
                }
            }
            _ => {}
        }
    }
    Ok(refs)
}

/// `bytes.decode("utf-8", "replace")`.
pub fn utf8_replace(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// `_read_symbol_names`, appended in tag order (later ids win, first position kept).
fn read_symbol_names(body: &[u8], out: &mut Vec<(u16, String)>, index: &mut HashMap<u16, usize>) {
    let Some(count) = u16_at(body, 0) else { return };
    let mut off = 2;
    for _ in 0..count {
        let Some(id) = u16_at(body, off) else { break };
        off += 2;
        let Some(end) = body[off.min(body.len())..].iter().position(|&c| c == 0) else { break };
        let name = utf8_replace(&body[off..off + end]);
        off += end + 1;
        match index.get(&id) {
            Some(&i) => out[i].1 = name,
            None => {
                index.insert(id, out.len());
                out.push((id, name));
            }
        }
    }
}

/// `_resolve_names`: breadth-first from every named symbol at once.
fn resolve_names(
    bitmaps: &HashSet<u16>,
    symbols: &[(u16, String)],
    edges: &HashMap<u16, Vec<u16>>,
) -> HashMap<u16, String> {
    let mut named: HashMap<u16, String> =
        symbols.iter().filter(|(id, _)| bitmaps.contains(id)).map(|(id, n)| (*id, n.clone())).collect();
    let mut queue: VecDeque<(u16, &str)> = symbols.iter().map(|(id, n)| (*id, n.as_str())).collect();
    let mut seen: HashSet<u16> = symbols.iter().map(|(id, _)| *id).collect();
    while let Some((cid, name)) = queue.pop_front() {
        for &child in edges.get(&cid).map(Vec::as_slice).unwrap_or(&[]) {
            if bitmaps.contains(&child) {
                named.entry(child).or_insert_with(|| name.to_owned());
            }
            if seen.insert(child) {
                queue.push_back((child, name));
            }
        }
    }
    named
}

/// `troveio.inflate(data, strict=True)`: one-shot, must reach the stream end.
fn inflate_strict(data: &[u8]) -> Option<Vec<u8>> {
    let z = kiwi_core::zlib::inflate(data, usize::MAX)?;
    z.eof.then_some(z.data)
}

pub(crate) struct Decoded {
    char_id: u16,
    pub(crate) rgba: Vec<u8>,
    pub(crate) width: usize,
    pub(crate) height: usize,
    codec: String,
    original: Option<Vec<u8>>,
    icc: Option<Vec<u8>>,
}

/// `_decode_lossless`; None where Python raises (the tag is skipped).
pub(crate) fn decode_lossless(body: &[u8], tag: u16) -> Option<Decoded> {
    if body.len() < 7 {
        return None;
    }
    let char_id = u16_at(body, 0)?;
    let fmt = body[2];
    let (w, h) = (usize::from(u16_at(body, 3)?), usize::from(u16_at(body, 5)?));
    if w == 0 || h == 0 || (w * h) as u64 > MAX_PIXELS {
        return None;
    }
    let mut off = 7;
    let mut colors = 0usize;
    if fmt == 3 {
        colors = usize::from(*body.get(off)?) + 1;
        off += 1;
    }
    let data = inflate_strict(body.get(off..)?)?;
    let alpha = tag == 36;
    let px = core::lossless_pixels(fmt, alpha, w, h, colors, &data)?;
    let codec = match fmt {
        3 => format!("indexed/{colors}"),
        4 => "rgb555".into(),
        _ if alpha => "argb32".into(),
        _ => "rgb24".into(),
    };
    Some(Decoded { char_id, rgba: px, width: w, height: h, codec, original: None, icc: None })
}

fn sniff(data: &[u8]) -> &'static str {
    if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        "png"
    } else if data.starts_with(b"GIF") {
        "gif"
    } else {
        "jpeg"
    }
}

fn replace_once(hay: &[u8], needle: &[u8]) -> Option<Vec<u8>> {
    let at = hay.windows(needle.len()).position(|w| w == needle)?;
    let mut out = hay[..at].to_vec();
    out.extend_from_slice(&hay[at + needle.len()..]);
    Some(out)
}

/// `_decode_jpeg`; None where Python raises (the tag is skipped).
pub(crate) fn decode_jpeg(body: &[u8], tag: u16, tables: Option<&[u8]>) -> Option<Decoded> {
    let char_id = u16_at(body, 0)?;
    let mut off = 2;
    let (mut payload, alpha_blob): (Vec<u8>, &[u8]) = if tag == 35 || tag == 90 {
        if tag == 90 {
            off += 2;
        }
        let a = body.get(off..off + 4)?;
        let alpha_off = u32::from_le_bytes([a[0], a[1], a[2], a[3]]) as usize;
        off += 4;
        let end = off.saturating_add(alpha_off);
        (body[off.min(body.len())..end.min(body.len())].to_vec(), &body[end.min(body.len())..])
    } else {
        (body[off..].to_vec(), &[])
    };
    let mut spliced = false;
    if let Some(t) = tables.filter(|t| tag == 6 && !t.is_empty()) {
        spliced = true;
        payload = if t.ends_with(b"\xff\xd9") {
            let mut p = t[..t.len() - 2].to_vec();
            p.extend_from_slice(payload.get(2..).unwrap_or(&[]));
            p
        } else {
            let mut p = t.to_vec();
            p.extend_from_slice(&payload);
            p
        };
    }
    let codec = sniff(&payload);
    if codec == "jpeg"
        && let Some(cleaned) = replace_once(&payload, b"\xff\xd9\xff\xd8")
    {
        payload = cleaned;
        spliced = true;
    }
    let pic = pil::open(&payload, MAX_PIXELS)?;
    let (w, h) = (pic.width, pic.height);
    let mut plane = if alpha_blob.is_empty() { Vec::new() } else { inflate_strict(alpha_blob).unwrap_or_default() };
    if !plane.is_empty() && plane.len() >= w * h {
        plane.truncate(w * h);
        if plane.iter().all(|&a| a == 255) {
            plane.clear();
        }
    } else {
        plane.clear();
    }
    let mut rgba = pil::to_rgba(&pic);
    if !plane.is_empty() {
        for (px, &a) in rgba.chunks_exact_mut(4).zip(plane.iter()) {
            px[3] = a;
        }
        pil::unpremultiply(&mut rgba);
        return Some(Decoded { char_id, rgba, width: w, height: h, codec: codec.into(), original: None, icc: None });
    }
    let original = (!spliced).then_some(payload);
    Some(Decoded { char_id, rgba, width: w, height: h, codec: codec.into(), original, icc: pic.icc })
}

fn mime_of(codec: &str) -> &'static str {
    match codec {
        "png" => "image/png",
        "gif" => "image/gif",
        "jpeg" => "image/jpeg",
        _ => "image/png",
    }
}

fn source_of(code: u16) -> &'static str {
    match code {
        6 => "DefineBits",
        20 => "DefineBitsLossless",
        21 => "DefineBitsJPEG2",
        35 => "DefineBitsJPEG3",
        36 => "DefineBitsLossless2",
        _ => "DefineBitsJPEG4",
    }
}

pub type Inventory = Vec<(&'static str, i64)>;

/// `extract_images`: (header, images sorted by character id, inventory).
pub fn extract_images(raw: &[u8]) -> Result<(Header, Vec<Image>, Inventory), Fail> {
    let data = decompress(raw)?;
    let header = read_header(raw)?;
    let mut tables: Option<&[u8]> = None;
    let mut pending: Vec<(u16, &[u8])> = Vec::new();
    let mut symbols: Vec<(u16, String)> = Vec::new();
    let mut sym_index = HashMap::new();
    let mut edges: HashMap<u16, Vec<u16>> = HashMap::new();
    let mut inventory: Inventory = INVENTORY.iter().map(|(k, _)| (*k, 0)).collect();
    let tags = core::tags(&data).ok_or_else(|| swf_err("truncated rect"))?;
    for &(code, body) in &tags {
        if code == 8 {
            tables = Some(body);
        } else if BITMAP_TAGS.contains(&code) {
            if pending.len() < MAX_IMAGES {
                pending.push((code, body));
            }
        } else if code == 56 || code == 76 {
            read_symbol_names(body, &mut symbols, &mut sym_index);
        } else if let Some(version) = shape_version(code) {
            if let Some(shape_id) = u16_at(body, 0) {
                let refs: Vec<u16> =
                    core::shape_bitmap_fills(body, version).unwrap_or_default().into_iter().map(|(id, _)| id).collect();
                if !refs.is_empty() {
                    edges.entry(shape_id).or_default().extend(refs);
                }
            }
        } else if code == 39
            && let Some(sprite_id) = u16_at(body, 0)
        {
            let refs = sprite_child_refs(body)?;
            edges.entry(sprite_id).or_default().extend(refs);
        }
        if let Some(slot) = INVENTORY.iter().position(|(_, codes)| codes.contains(&code)) {
            inventory[slot].1 += 1;
        }
    }
    let ids: HashSet<u16> = pending.iter().filter_map(|(_, b)| u16_at(b, 0)).collect();
    let names = resolve_names(&ids, &symbols, &edges);

    let mut images = Vec::new();
    for &(code, body) in &pending {
        let decoded =
            if code == 20 || code == 36 { decode_lossless(body, code) } else { decode_jpeg(body, code, tables) };
        let Some(d) = decoded else {
            tracing::info!("swf: skipping tag {code}");
            continue;
        };
        let (data, mime) = match d.original {
            Some(orig) => (orig, mime_of(&d.codec)),
            None => (pil::encode_png(&d.rgba, d.width, d.height, d.icc.as_deref()), "image/png"),
        };
        let thumb = pil::thumbnail(&d.rgba, d.width, d.height, THUMB_BOX)
            .map(|(px, w, h)| pil::encode_png(&px, w, h, d.icc.as_deref()));
        images.push(Image {
            char_id: d.char_id,
            name: names.get(&d.char_id).cloned(),
            source: source_of(code),
            codec: d.codec,
            width: d.width,
            height: d.height,
            data,
            mime,
            thumb,
        });
    }
    images.sort_by_key(|i| i.char_id);
    Ok((header, images, inventory))
}

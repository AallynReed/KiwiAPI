//! `insignia.py`: the sigil beside a player's name, a Power Rank shield on a
//! set of Mastery wings. Each tier is one frame of `ui/nameplate.swf`'s
//! `wings` / `shield` sprites, composited on a shared canvas and stored as a
//! base64 PNG (`optimize=True`).

use base64::Engine;
use indexmap::IndexMap;
use kiwi_core::swf::{self as core, Matrix};
use kiwi_pil::{Rgba, ops, png};

use super::mastery::single;
use crate::{
    decode::{
        json::{J, float_text},
        registry::{Built, Ctx, DecodeError},
        value::{K, W},
    },
    media::swf::extract::{self as swf, Decoded},
    obj,
};

const TABLE: &str = "prefabs/meta/insignia.binfab";
const SWF: &str = "ui/nameplate.swf";
const LAYERS: [(&str, &str, i128); 2] = [("wings", "mastery", 1), ("shield", "power_rank", 0)];
const MAX_PIXELS: usize = 8192 * 8192;

fn swf_err(msg: impl Into<String>) -> DecodeError {
    DecodeError { kind: "SwfError", msg: msg.into() }
}

fn key_err(k: impl std::fmt::Display) -> DecodeError {
    DecodeError { kind: "KeyError", msg: k.to_string() }
}

fn unpack_err(n: usize) -> DecodeError {
    DecodeError { kind: "error", msg: format!("unpack requires a buffer of {n} bytes") }
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes([*b.get(at)?, *b.get(at + 1)?]))
}

/// `struct.unpack("<H", b[at:at + 2])`.
fn unpack_u16(b: &[u8], at: usize) -> Result<u16, DecodeError> {
    u16_at(b, at).ok_or_else(|| unpack_err(2))
}

/// `bytes.find(b"\0", off)`.
fn find_nul(b: &[u8], off: usize) -> Option<usize> {
    b.get(off..)?.iter().position(|&c| c == 0).map(|i| off + i)
}

#[derive(Clone)]
struct Placement {
    char_id: u16,
    name: Option<String>,
    matrix: Matrix,
}

type Frame = IndexMap<u16, Placement>;

/// `_sprite_frames`: what sits at each depth, frame by frame.
fn sprite_frames(body: &[u8]) -> Result<Vec<Frame>, DecodeError> {
    let mut frames = Vec::new();
    let mut live: Frame = IndexMap::new();
    for (code, tag) in swf::sprite_tags(body) {
        match code {
            1 => frames.push(live.clone()),
            28 if tag.len() >= 2 => {
                live.shift_remove(&unpack_u16(tag, 0)?);
            }
            5 if tag.len() >= 4 => {
                live.shift_remove(&unpack_u16(tag, 2)?);
            }
            26 | 70 if tag.len() >= 3 => {
                let flags = tag[0];
                let mut off = if code == 26 { 1 } else { 2 };
                let depth = unpack_u16(tag, off)?;
                off += 2;
                if code == 70 && (tag[1] & 0x08 != 0 || (tag[1] & 0x10 != 0 && flags & 0x02 != 0)) {
                    off = find_nul(tag, off).map_or(tag.len(), |e| e + 1);
                }
                let prev = if flags & 0x01 != 0 { live.get(&depth).cloned() } else { None };
                let mut char_id = prev.as_ref().map(|p| p.char_id);
                if flags & 0x02 != 0 {
                    char_id = Some(unpack_u16(tag, off)?);
                    off += 2;
                }
                let rest = tag.get(off..).unwrap_or(&[]);
                let (read, used) = core::place_bits(rest, flags & 0x04 != 0, flags & 0x08 != 0)
                    .ok_or_else(|| swf_err("bit overrun"))?;
                let matrix = read.or(prev.as_ref().map(|p| p.matrix)).unwrap_or_default();
                off += used;
                if flags & 0x10 != 0 {
                    off += 2;
                }
                let mut name = prev.and_then(|p| p.name);
                if flags & 0x20 != 0 {
                    let end = find_nul(tag, off).unwrap_or(tag.len());
                    name = Some(swf::utf8_replace(tag.get(off..end).unwrap_or(&[])));
                }
                if let Some(char_id) = char_id {
                    live.insert(depth, Placement { char_id, name, matrix });
                }
            }
            _ => {}
        }
    }
    Ok(frames)
}

/// CPython's `zlib.error` text for a stream `zlib.decompress` refuses.
fn zlib_error(data: &[u8]) -> String {
    let mut d = flate2::Decompress::new(true);
    let mut out = Vec::with_capacity(data.len().saturating_mul(4).clamp(64, 1 << 20));
    loop {
        if out.len() == out.capacity() {
            out.reserve(out.len());
        }
        let at = d.total_in() as usize;
        match d.decompress_vec(&data[at..], &mut out, flate2::FlushDecompress::Finish) {
            Ok(flate2::Status::StreamEnd) => {
                return "Error -5 while decompressing data: incomplete or truncated stream".into();
            }
            Ok(_) if d.total_in() as usize == data.len() && out.len() < out.capacity() => {
                return "Error -5 while decompressing data: incomplete or truncated stream".into();
            }
            Ok(_) => {}
            Err(e) => {
                return format!("Error -3 while decompressing data: {}", e.message().unwrap_or("invalid input data"));
            }
        }
    }
}

/// Why `_decode_lossless` raised.
fn lossless_error(body: &[u8]) -> DecodeError {
    if body.len() < 7 {
        return swf_err("short lossless tag");
    }
    let (fmt, w, h) = (
        body[2],
        usize::from(u16::from_le_bytes([body[3], body[4]])),
        usize::from(u16::from_le_bytes([body[5], body[6]])),
    );
    if w == 0 || h == 0 || w * h > MAX_PIXELS {
        return swf_err(format!("bad lossless size {w}x{h}"));
    }
    let off = if fmt == 3 { 8 } else { 7 };
    if fmt == 3 && body.len() < 8 {
        return DecodeError { kind: "IndexError", msg: "index out of range".into() };
    }
    let z = kiwi_core::zlib::inflate(&body[off..], usize::MAX);
    if !z.as_ref().is_some_and(|z| z.eof) {
        return swf_err(format!("bad lossless zlib: {}", zlib_error(&body[off..])));
    }
    match fmt {
        3 => swf_err("truncated palette"),
        4 | 5 => swf_err("lossless decode failed"),
        _ => swf_err(format!("unknown lossless format {fmt}")),
    }
}

/// Why `_decode_jpeg` raised.
fn jpeg_error(body: &[u8], tag: u16) -> DecodeError {
    let off = if tag == 90 { 4 } else { 2 };
    if (tag == 35 || tag == 90) && body.len() < off + 4 {
        return unpack_err(4);
    }
    let payload = body.get(if tag == 35 || tag == 90 { off + 4 } else { 2 }..).unwrap_or(&[]);
    let codec = if payload.starts_with(b"\x89PNG\r\n\x1a\n") {
        "png"
    } else if payload.starts_with(b"GIF") {
        "gif"
    } else {
        "jpeg"
    };
    swf_err(format!("undecodable {codec}"))
}

type Piece = (Rgba, i64, i64);
type Layers = Vec<(&'static str, Vec<Vec<Piece>>)>;

/// `_layers`: each layer's frames as (bitmap, x, y) pieces from the sigil origin.
fn layers(raw: &[u8]) -> Result<Layers, DecodeError> {
    let data = swf::decompress(raw).map_err(|e| match e {
        swf::Fail::Swf(m) => swf_err(m),
        swf::Fail::Crash(m) => DecodeError { kind: "RuntimeError", msg: m },
    })?;
    let mut tables: Option<&[u8]> = None;
    let mut bitmaps: IndexMap<u16, (u16, &[u8])> = IndexMap::new();
    let mut shapes: IndexMap<u16, Vec<(u16, Matrix)>> = IndexMap::new();
    let mut sprites: IndexMap<u16, Vec<Frame>> = IndexMap::new();
    for (code, body) in core::tags(&data).ok_or_else(|| swf_err("truncated rect"))? {
        let id = u16_at(body, 0);
        match (code, id) {
            (8, _) => tables = Some(body),
            (6 | 20 | 21 | 35 | 36 | 90, Some(id)) => {
                bitmaps.insert(id, (code, body));
            }
            (2 | 22 | 32 | 83, Some(id)) => {
                let version = match code {
                    2 => 1,
                    22 => 2,
                    32 => 3,
                    _ => 4,
                };
                shapes.insert(id, core::shape_bitmap_fills(body, version).ok_or_else(|| swf_err("bit overrun"))?);
            }
            (39, Some(id)) => {
                sprites.insert(id, sprite_frames(body)?);
            }
            _ => {}
        }
    }

    let wanted = LAYERS.map(|(n, _, _)| n);
    let mut children: Option<IndexMap<Option<String>, Placement>> = None;
    for frames in sprites.values() {
        let mut c = IndexMap::new();
        if let Some(first) = frames.first() {
            for p in first.values() {
                c.insert(p.name.clone(), p.clone());
            }
        }
        if wanted.iter().all(|n| c.contains_key(&Some((*n).to_owned()))) {
            children = Some(c);
            break;
        }
    }
    let children =
        children.ok_or_else(|| DecodeError::value(format!("{SWF}: no sprite places ['shield', 'wings']")))?;

    let bitmap = |char_id: u16| -> Result<Rgba, DecodeError> {
        let &(code, body) = bitmaps.get(&char_id).ok_or_else(|| key_err(char_id))?;
        let d: Decoded = if code == 20 || code == 36 {
            swf::decode_lossless(body, code).ok_or_else(|| lossless_error(body))?
        } else {
            swf::decode_jpeg(body, code, tables).ok_or_else(|| jpeg_error(body, code))?
        };
        Ok(Rgba {
            width: d.width,
            height: d.height,
            data: d.rgba.chunks_exact(4).map(|p| [p[0], p[1], p[2], p[3]]).collect(),
        })
    };

    let mut out = Vec::new();
    for name in wanted {
        let child = &children[&Some(name.to_owned())];
        let frames = sprites.get(&child.char_id).ok_or_else(|| key_err(child.char_id))?;
        let mut layer = Vec::new();
        for frame in frames {
            let mut depths: Vec<u16> = frame.keys().copied().collect();
            depths.sort_unstable();
            let mut pieces = Vec::new();
            for depth in depths {
                let placed = &frame[&depth];
                let fills = shapes
                    .get(&placed.char_id)
                    .filter(|f| f.len() == 1)
                    .ok_or_else(|| DecodeError::value(format!("{SWF} {name}: frame piece is not one bitmap fill")))?;
                let (char_id, m) = fills[0];
                if (m.sx - 20.0).abs() > 0.01 || (m.sy - 20.0).abs() > 0.01 || m.r0 != 0.0 || m.r1 != 0.0 {
                    return Err(DecodeError::value(format!("{SWF} {name}: bitmap fill is scaled or rotated")));
                }
                let tx = child.matrix.tx + placed.matrix.tx + m.tx;
                let ty = child.matrix.ty + placed.matrix.ty + m.ty;
                let px = |t: i64| (t as f64 / 20.0).round_ties_even() as i64;
                pieces.push((bitmap(char_id)?, px(tx), px(ty)));
            }
            layer.push(pieces);
        }
        out.push((name, layer));
    }
    Ok(out)
}

/// `_thresholds`: each tier's starting value, ascending.
fn thresholds(table: &W) -> Result<Vec<J>, DecodeError> {
    let W::Dict(t) = table else {
        return Err(DecodeError { kind: "TypeError", msg: "insignia table is not a dict".into() });
    };
    let mut keys: Vec<i128> = Vec::new();
    for (k, _) in t.items() {
        match k {
            K::Int(i) => keys.push(*i),
            K::Str(_) => {
                return Err(DecodeError {
                    kind: "TypeError",
                    msg: "'<' not supported between instances of 'str' and 'int'".into(),
                });
            }
        }
    }
    keys.sort_unstable();
    let mut out = Vec::new();
    for k in keys {
        let v = match t.get(k) {
            Some(W::Dict(row)) => row.get(0).ok_or_else(|| key_err(0))?,
            Some(W::List(xs) | W::Tuple(xs)) => {
                xs.first().ok_or_else(|| DecodeError { kind: "IndexError", msg: "list index out of range".into() })?
            }
            _ => return Err(DecodeError { kind: "TypeError", msg: "insignia row is not subscriptable".into() }),
        };
        match v {
            W::Int(_) | W::Float(_) => out.push(J::from(v)),
            _ => return Err(DecodeError { kind: "TypeError", msg: "insignia threshold is not a number".into() }),
        }
    }
    let num = |j: &J| match j {
        J::Int(i) => *i as f64,
        J::Float(f) => *f,
        _ => 0.0,
    };
    if out.windows(2).any(|w| num(&w[1]) < num(&w[0])) {
        let text: Vec<String> = out
            .iter()
            .map(|j| match j {
                J::Int(i) => i.to_string(),
                J::Float(f) => float_text(*f),
                _ => String::new(),
            })
            .collect();
        return Err(DecodeError::value(format!("insignia thresholds not ascending: [{}]", text.join(", "))));
    }
    Ok(out)
}

fn png_b64(img: &Rgba) -> String {
    let flat: Vec<u8> = img.data.iter().flatten().copied().collect();
    base64::engine::general_purpose::STANDARD.encode(png::encode_rgba_optimized(&flat, img.width, img.height))
}

pub fn build(ctx: &Ctx) -> Built {
    let (table, raw) = (ctx.tree.read(TABLE), ctx.tree.read(SWF));
    let (Some(table), Some(raw)) = (table.as_ref(), raw) else {
        return Err(DecodeError::value(format!("missing {}", if table.is_none() { TABLE } else { SWF })));
    };
    let root = single(table).map_err(|_| DecodeError { kind: "WireError", msg: "bad insignia table".into() })?;
    let layers = layers(&raw)?;

    let pieces: Vec<&Piece> = layers.iter().flat_map(|(_, f)| f.iter().flatten()).collect();
    let empty = || DecodeError::value("min() iterable argument is empty");
    let left = pieces.iter().map(|p| p.1).min().ok_or_else(empty)?;
    let top = pieces.iter().map(|p| p.2).min().ok_or_else(empty)?;
    let width = pieces.iter().map(|p| p.1 + p.0.width as i64).max().ok_or_else(empty)? - left;
    let height = pieces.iter().map(|p| p.2 + p.0.height as i64).max().ok_or_else(empty)? - top;

    let mut out = obj! {"size" => vec![J::Int(i128::from(width)), J::Int(i128::from(height))]};
    for (name, key, field) in LAYERS {
        let froms = thresholds(root.as_dict().and_then(|d| d.get(field)).ok_or_else(|| key_err(field))?)?;
        let frames = &layers.iter().find(|(n, _)| *n == name).expect("every layer").1;
        if froms.len() != frames.len() {
            return Err(DecodeError::value(format!("{name}: {} tiers but {} frames", froms.len(), frames.len())));
        }
        let mut tiers = Vec::new();
        for (start, frame) in froms.into_iter().zip(frames) {
            let mut canvas = Rgba::new(width as usize, height as usize, [0, 0, 0, 0]);
            for (im, x, y) in frame {
                ops::alpha_composite(&mut canvas, im, x - left, y - top);
            }
            tiers.push(obj! {"from" => start, "image" => png_b64(&canvas)});
        }
        if let J::Dict(m) = &mut out {
            m.insert(key.into(), J::List(tiers));
        }
    }
    Ok(out)
}

pub fn count(data: &J) -> Option<usize> {
    Some(data.get("power_rank").map_or(0, J::len) + data.get("mastery").map_or(0, J::len))
}

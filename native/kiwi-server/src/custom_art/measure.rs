//! `_measure`: `store.sniff_image` for the type, then the size Pillow 13's
//! `Image.open` reports. The PNG and GIF header walks follow
//! `PngImagePlugin._open` and `GifImagePlugin._open`/`_seek(0)`, JPEG reuses
//! the `JpegImagePlugin._open` port, WebP checks what libwebp's demuxer needs
//! before reading the canvas. None wherever Pillow raises.

use std::io::Cursor;

use flate2::{Decompress, FlushDecompress, Status};

use crate::{images::service::sniff_image, media::pil::jpeg};

/// `Image.MAX_IMAGE_PIXELS`.
const MAX_IMAGE_PIXELS: u64 = 1024 * 1024 * 1024 / 4 / 3;
const MAX_TEXT_CHUNK: usize = 1024 * 1024;
const MAX_TEXT_MEMORY: usize = 64 * MAX_TEXT_CHUNK;

/// `(content_type, width, height)`, or None where `_measure` returns None.
pub fn measure(data: &[u8]) -> Option<(&'static str, u32, u32)> {
    let (content_type, _, _) = sniff_image(data)?;
    let (w, h) = open_size(data)?;
    Some((content_type, w, h))
}

/// `Image.open(data).size`.
pub fn open_size(data: &[u8]) -> Option<(u32, u32)> {
    let size = if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        png(data)?
    } else if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
        gif(data)?
    } else if data.starts_with(b"RIFF") && data.get(8..12) == Some(b"WEBP") {
        webp(data)?
    } else if data.starts_with(b"\xff\xd8\xff") {
        let (h, _) = jpeg::open_header(data)?;
        (h.width, h.height)
    } else {
        return None;
    };
    bomb_ok(size).then_some(size)
}

/// `_decompression_bomb_check` (the error half).
fn bomb_ok((w, h): (u32, u32)) -> bool {
    u64::from(w.max(1)) * u64::from(h.max(1)) <= 2 * MAX_IMAGE_PIXELS
}

struct Reader<'a> {
    b: &'a [u8],
    p: usize,
}

impl<'a> Reader<'a> {
    fn read(&mut self, n: usize) -> &'a [u8] {
        let end = self.p.saturating_add(n).min(self.b.len());
        let s = &self.b[self.p..end];
        self.p = end;
        s
    }

    /// `ImageFile._safe_read`.
    fn safe_read(&mut self, n: usize) -> Option<&'a [u8]> {
        let s = self.read(n);
        (s.len() == n).then_some(s)
    }
}

fn be32(s: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(s.get(at..at + 4)?.try_into().ok()?))
}

fn le16(s: &[u8], at: usize) -> Option<u32> {
    Some(u32::from(u16::from_le_bytes(s.get(at..at + 2)?.try_into().ok()?)))
}

/// `_safe_zlib_decompress`: Err(true) for the ValueError (too large),
/// Err(false) for `zlib.error`.
fn safe_zlib(s: &[u8]) -> Result<usize, bool> {
    let mut d = Decompress::new(true);
    let mut out = vec![0u8; MAX_TEXT_CHUNK];
    match d.decompress(s, &mut out, FlushDecompress::Sync) {
        Ok(Status::StreamEnd) => Ok(d.total_out() as usize),
        Ok(_) => {
            let (consumed, produced) = (d.total_in() as usize, d.total_out() as usize);
            if consumed < s.len() && produced == MAX_TEXT_CHUNK { Err(true) } else { Ok(produced) }
        }
        Err(_) => Err(false),
    }
}

fn png_mode(depth: u8, color: u8) -> Option<&'static str> {
    Some(match (depth, color) {
        (1, 0) => "1",
        (2, 0) | (4, 0) | (8, 0) => "L",
        (16, 0) => "I;16",
        (8, 2) | (16, 2) => "RGB",
        (1, 3) | (2, 3) | (4, 3) | (8, 3) => "P",
        (8, 4) => "LA",
        (16, 4) | (8, 6) | (16, 6) => "RGBA",
        _ => return None,
    })
}

fn split0(s: &[u8]) -> Option<(&[u8], &[u8])> {
    let i = s.iter().position(|&b| b == 0)?;
    Some((&s[..i], &s[i + 1..]))
}

/// `PngImageFile._open` up to the first IDAT/fdAT, then the size checks.
fn png(b: &[u8]) -> Option<(u32, u32)> {
    let mut r = Reader { b, p: 8 };
    let mut size = (0u32, 0u32);
    let mut mode = "";
    let mut n_frames: Option<u32> = None;
    let mut seq: Option<i64> = None;
    let mut text_memory = 0usize;
    loop {
        let head = r.read(8);
        let cid = head.get(4..).unwrap_or_default();
        let length = be32(head, 0)? as usize;
        if cid.len() != 4 || !cid.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'_') {
            return None;
        }
        let s: &[u8] = match cid {
            b"IHDR" => {
                let s = r.safe_read(length)?;
                if length < 13 {
                    return None;
                }
                size = (be32(s, 0)?, be32(s, 4)?);
                if let Some(m) = png_mode(s[8], s[9]) {
                    mode = m;
                }
                if s[11] != 0 {
                    return None;
                }
                s
            }
            b"IDAT" | b"IEND" => break,
            b"fdAT" => {
                if length < 4 {
                    return None;
                }
                let s = r.safe_read(4)?;
                let n = i64::from(be32(s, 0)?);
                if seq != Some(n - 1) {
                    return None;
                }
                break;
            }
            b"tRNS" => {
                let s = r.safe_read(length)?;
                if matches!(mode, "1" | "L" | "I;16" | "RGB") && length < (if mode == "RGB" { 6 } else { 2 }) {
                    return None;
                }
                s
            }
            b"gAMA" | b"cHRM" | b"sRGB" | b"pHYs" | b"acTL" | b"fcTL" => {
                let s = r.safe_read(length)?;
                let min = match cid {
                    b"gAMA" => 4,
                    b"cHRM" => 32,
                    b"sRGB" => 1,
                    b"pHYs" => 9,
                    b"acTL" => 8,
                    _ => 26,
                };
                if length < min {
                    return None;
                }
                if cid == b"acTL" {
                    if n_frames.is_some() {
                        n_frames = None;
                    } else {
                        let n = be32(s, 0)?;
                        if n != 0 && n <= 0x8000_0000 {
                            n_frames = Some(n);
                        }
                    }
                } else if cid == b"fcTL" {
                    let n = i64::from(be32(s, 0)?);
                    if (seq.is_none() && n != 0) || seq.is_some_and(|q| q != n - 1) {
                        return None;
                    }
                    seq = Some(n);
                    let (w, h, px, py) = (be32(s, 4)?, be32(s, 8)?, be32(s, 12)?, be32(s, 16)?);
                    if u64::from(px) + u64::from(w) > u64::from(size.0)
                        || u64::from(py) + u64::from(h) > u64::from(size.1)
                    {
                        return None;
                    }
                }
                s
            }
            b"tEXt" => {
                let s = r.safe_read(length)?;
                let (k, v) = split0(s).unwrap_or((s, &[]));
                if !k.is_empty() {
                    text_memory += v.len();
                    if text_memory > MAX_TEXT_MEMORY {
                        return None;
                    }
                }
                s
            }
            b"zTXt" => {
                let s = r.safe_read(length)?;
                let (k, v) = split0(s).unwrap_or((s, &[]));
                if v.first().is_some_and(|&m| m != 0) {
                    return None;
                }
                let n = match safe_zlib(v.get(1..).unwrap_or_default()) {
                    Ok(n) => n,
                    Err(true) => return None,
                    Err(false) => 0,
                };
                if !k.is_empty() {
                    text_memory += n;
                    if text_memory > MAX_TEXT_MEMORY {
                        return None;
                    }
                }
                s
            }
            b"iTXt" => {
                let s = r.safe_read(length)?;
                if let Some((_, rest)) = split0(s)
                    && rest.len() >= 2
                {
                    let (cf, cm, rest) = (rest[0], rest[1], &rest[2..]);
                    if let Some((lang, rest)) = split0(rest)
                        && let Some((tk, v)) = split0(rest)
                    {
                        let mut text: Option<Vec<u8>> = Some(v.to_vec());
                        if cf != 0 {
                            text = if cm == 0 {
                                let mut d = Decompress::new(true);
                                let mut out = vec![0u8; MAX_TEXT_CHUNK];
                                match d.decompress(v, &mut out, FlushDecompress::Sync) {
                                    Ok(st) => {
                                        let (inp, outp) = (d.total_in() as usize, d.total_out() as usize);
                                        if st != Status::StreamEnd && inp < v.len() && outp == MAX_TEXT_CHUNK {
                                            return None;
                                        }
                                        out.truncate(outp);
                                        Some(out)
                                    }
                                    Err(_) => None,
                                }
                            } else {
                                None
                            };
                        }
                        if let Some(t) = text
                            && std::str::from_utf8(lang).is_ok()
                            && std::str::from_utf8(tk).is_ok()
                            && let Ok(v) = std::str::from_utf8(&t)
                        {
                            text_memory += v.chars().count();
                            if text_memory > MAX_TEXT_MEMORY {
                                return None;
                            }
                        }
                    }
                }
                s
            }
            b"iCCP" => {
                let s = r.safe_read(length)?;
                let i = s.iter().position(|&b| b == 0).map_or(-1, |i| i as isize);
                let comp = *s.get((i + 1) as usize)?;
                if comp != 0 {
                    return None;
                }
                if safe_zlib(s.get((i + 2) as usize..).unwrap_or_default()) == Err(true) {
                    return None;
                }
                s
            }
            _ => r.safe_read(length)?,
        };
        let crc = r.read(4);
        if crc.len() < 4 {
            return None;
        }
        let mut h = crc32fast::Hasher::new();
        h.update(cid);
        h.update(s);
        if h.finalize() != be32(crc, 0)? {
            return None;
        }
    }
    (!mode.is_empty() && size.0 > 0 && size.1 > 0).then_some(size)
}

/// `GifImagePlugin._is_palette_needed`, None where it raises.
fn palette_checked(p: &[u8]) -> Option<()> {
    let mut i = 0;
    while i < p.len() {
        if i / 3 != usize::from(p[i]) {
            return Some(());
        }
        let b1 = *p.get(i + 1)?;
        if p[i] != b1 {
            return Some(());
        }
        let b2 = *p.get(i + 2)?;
        if b1 != b2 {
            return Some(());
        }
        i += 3;
    }
    Some(())
}

/// `GifImagePlugin.data`.
fn sub_block<'a>(r: &mut Reader<'a>) -> Option<&'a [u8]> {
    let n = *r.read(1).first()?;
    if n == 0 {
        return None;
    }
    Some(r.read(usize::from(n)))
}

/// `GifImageFile._open` and `_seek(0)`: the logical screen, grown to the
/// first frame's extent.
fn gif(b: &[u8]) -> Option<(u32, u32)> {
    let mut r = Reader { b, p: 0 };
    let s = r.read(13);
    let (mut w, mut h) = (le16(s, 6)?, le16(s, 8)?);
    let flags = *s.get(10)?;
    if flags & 128 != 0 {
        palette_checked(r.read(3 << ((flags & 7) + 1)))?;
    }
    let mut s: Vec<u8> = r.read(1).to_vec();
    if s.is_empty() || s == b";" {
        return None;
    }
    let mut found = false;
    loop {
        if s.is_empty() {
            s = r.read(1).to_vec();
        }
        if s.is_empty() || s == b";" {
            break;
        }
        if s == b"!" {
            let label = *r.read(1).first()?;
            let block = sub_block(&mut r);
            if label == 249 && block.is_some() {
                let block = block.unwrap_or_default();
                let flags = *block.first()?;
                if flags & 1 != 0 {
                    block.get(3)?;
                }
                le16(block, 1)?;
            } else if label == 254 {
                let mut block = block;
                while block.is_some_and(|b| !b.is_empty()) {
                    block = sub_block(&mut r);
                }
                s.clear();
                continue;
            } else if label == 255
                && let Some(block) = block
                && block.starts_with(b"NETSCAPE2.0")
            {
                sub_block(&mut r);
            }
            while sub_block(&mut r).is_some_and(|b| !b.is_empty()) {}
        } else if s == b"," {
            let d = r.read(9);
            let (x0, y0) = (le16(d, 0)?, le16(d, 2)?);
            let (x1, y1) = (x0 + le16(d, 4)?, y0 + le16(d, 6)?);
            if x1 > w || y1 > h {
                let grown = (x1.max(w), y1.max(h));
                if !bomb_ok(grown) {
                    return None;
                }
                (w, h) = grown;
            }
            let flags = *d.get(8)?;
            if flags & 128 != 0 {
                palette_checked(r.read(3 << ((flags & 7) + 1)))?;
            }
            r.read(1).first()?;
            found = true;
            break;
        }
        s.clear();
    }
    (found && w > 0 && h > 0).then_some((w, h))
}

/// `WebPImageFile._open`: the header size check, then the canvas of a file
/// libwebp's demuxer would take whole.
fn webp(b: &[u8]) -> Option<(u32, u32)> {
    let s = &b[..b.len().min(30)];
    if !matches!(s.get(12..16), Some(b"VP8 " | b"VP8X" | b"VP8L")) {
        return None;
    }
    if let Some(size) = header_size(s)
        && !bomb_ok(size)
    {
        return None;
    }
    let riff = usize::try_from(u32::from_le_bytes(b.get(4..8)?.try_into().ok()?)).ok()?;
    if riff < 8 || riff > u32::MAX as usize - 8 - 1 || riff + 8 > b.len() {
        return None;
    }
    let data = &b[..riff + 8];
    let mut at = 12;
    while at < data.len() {
        let size = u32::from_le_bytes(data.get(at + 4..at + 8)?.try_into().ok()?) as usize;
        let padded = size + (size & 1);
        if at + 8 + padded > data.len() {
            return None;
        }
        at += 8 + padded;
    }
    let dec = image_webp::WebPDecoder::new(Cursor::new(data)).ok()?;
    let (w, h) = dec.dimensions();
    Some((w, h))
}

/// `WebPImagePlugin._get_size`.
fn header_size(s: &[u8]) -> Option<(u32, u32)> {
    let le24 = |at: usize| -> Option<u32> {
        let b = s.get(at..at + 3)?;
        Some(u32::from(b[0]) | u32::from(b[1]) << 8 | u32::from(b[2]) << 16)
    };
    match s.get(12..16)? {
        b"VP8 " if s.len() >= 30 && &s[23..26] == b"\x9d\x01\x2a" => {
            Some((le16(s, 26)? & 0x3FFF, le16(s, 28)? & 0x3FFF))
        }
        b"VP8X" if s.len() >= 30 => Some((le24(24)? + 1, le24(27)? + 1)),
        b"VP8L" if s.len() >= 25 && s[20] == 0x2F => {
            let bits = u32::from_le_bytes(s.get(21..25)?.try_into().ok()?);
            Some(((bits & 0x3FFF) + 1, ((bits >> 14) & 0x3FFF) + 1))
        }
        _ => None,
    }
}

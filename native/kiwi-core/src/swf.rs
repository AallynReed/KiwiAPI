//! SWF pixel and bit-level parsing, mirroring `app/trove/swf/extract.py`.
//!
//! zlib stays with the caller. Every `None` means the Python reference raises.

/// MSB-first bit reader, `extract._BitReader`.
pub struct BitReader<'a> {
    b: &'a [u8],
    p: usize,
}

impl<'a> BitReader<'a> {
    pub fn new(b: &'a [u8]) -> Self {
        BitReader { b, p: 0 }
    }

    pub fn read(&mut self, n: u32) -> Option<u64> {
        let mut v = 0u64;
        for _ in 0..n {
            let byte = *self.b.get(self.p >> 3)?;
            v = (v << 1) | u64::from((byte >> (7 - (self.p & 7))) & 1);
            self.p += 1;
        }
        Some(v)
    }

    pub fn read_signed(&mut self, n: u32) -> Option<i64> {
        let v = self.read(n)? as i64;
        Some(if n > 0 && (v >> (n - 1)) & 1 == 1 { v - (1i64 << n) } else { v })
    }

    pub fn align(&mut self) {
        self.p = (self.p + 7) & !7;
    }

    pub fn byte_pos(&self) -> usize {
        (self.p + 7) >> 3
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Matrix {
    pub sx: f64,
    pub sy: f64,
    pub r0: f64,
    pub r1: f64,
    pub tx: i64,
    pub ty: i64,
}

impl Default for Matrix {
    fn default() -> Self {
        Matrix { sx: 1.0, sy: 1.0, r0: 0.0, r1: 0.0, tx: 0, ty: 0 }
    }
}

pub fn read_matrix(bits: &mut BitReader) -> Option<Matrix> {
    let mut m = Matrix::default();
    if bits.read(1)? == 1 {
        let n = bits.read(5)? as u32;
        m.sx = bits.read_signed(n)? as f64 / 65536.0;
        m.sy = bits.read_signed(n)? as f64 / 65536.0;
    }
    if bits.read(1)? == 1 {
        let n = bits.read(5)? as u32;
        m.r0 = bits.read_signed(n)? as f64 / 65536.0;
        m.r1 = bits.read_signed(n)? as f64 / 65536.0;
    }
    let n = bits.read(5)? as u32;
    m.tx = bits.read_signed(n)?;
    m.ty = bits.read_signed(n)?;
    bits.align();
    Some(m)
}

fn skip_cxform_alpha(bits: &mut BitReader) -> Option<()> {
    let (add, mult) = (bits.read(1)?, bits.read(1)?);
    let n = bits.read(4)?;
    bits.read((n * 4 * (add + mult)) as u32)?;
    bits.align();
    Some(())
}

/// The bit-packed part of a PlaceObject2/3 after the character id:
/// (matrix if `has_matrix`, bytes consumed including the colour transform).
pub fn place_bits(data: &[u8], has_matrix: bool, has_cxform: bool) -> Option<(Option<Matrix>, usize)> {
    let mut bits = BitReader::new(data);
    let m = if has_matrix { Some(read_matrix(&mut bits)?) } else { None };
    if has_cxform {
        skip_cxform_alpha(&mut bits)?;
    }
    Some((m, bits.byte_pos()))
}

/// `_shape_bitmap_fills`: (bitmap id, matrix) per bitmap fill style.
pub fn shape_bitmap_fills(body: &[u8], version: u32) -> Option<Vec<(u16, Matrix)>> {
    let mut bits = BitReader::new(body);
    bits.read(16)?;
    let n = bits.read(5)? as u32;
    bits.read(n * 4)?;
    bits.align();
    if version == 4 {
        let n = bits.read(5)? as u32;
        bits.read(n * 4)?;
        bits.align();
        bits.read(8)?;
    }
    let mut off = bits.byte_pos();
    if off >= body.len() {
        return Some(Vec::new());
    }
    let mut count = usize::from(body[off]);
    off += 1;
    if count == 0xFF {
        if off + 2 > body.len() {
            return Some(Vec::new());
        }
        count = usize::from(u16::from_le_bytes([body[off], body[off + 1]]));
        off += 2;
    }
    let rgba = version >= 3;
    let mut fills = Vec::new();
    for _ in 0..count {
        if off >= body.len() {
            break;
        }
        let style = body[off];
        off += 1;
        match style {
            0x00 => off += if rgba { 4 } else { 3 },
            0x10 | 0x12 | 0x13 => {
                let mut sub = BitReader::new(body.get(off..).unwrap_or(&[]));
                read_matrix(&mut sub)?;
                off += sub.byte_pos();
                if off >= body.len() {
                    break;
                }
                let nrec = usize::from(body[off] & 0x0F);
                off += 1 + nrec * if rgba { 5 } else { 4 };
                if style == 0x13 {
                    off += 2;
                }
            }
            0x40..=0x43 => {
                if off + 2 > body.len() {
                    break;
                }
                let id = u16::from_le_bytes([body[off], body[off + 1]]);
                off += 2;
                let mut sub = BitReader::new(&body[off..]);
                fills.push((id, read_matrix(&mut sub)?));
                off += sub.byte_pos();
            }
            _ => break,
        }
    }
    Some(fills)
}

/// `_decode_lossless` after inflation: RGBA pixels for formats 3, 4 and 5.
pub fn lossless_pixels(
    fmt: u8,
    has_alpha: bool,
    width: usize,
    height: usize,
    colors: usize,
    data: &[u8],
) -> Option<Vec<u8>> {
    let mut out = vec![0u8; width * height * 4];
    match fmt {
        3 => {
            let stride = (width + 3) & !3;
            let psize = if has_alpha { 4 } else { 3 };
            if data.len() < colors * psize {
                return None;
            }
            let (pal, rows) = data.split_at(colors * psize);
            let mut table = vec![0u8; colors * 4];
            for i in 0..colors {
                let c = &pal[i * psize..i * psize + psize];
                table[i * 4..i * 4 + 3].copy_from_slice(&c[..3]);
                table[i * 4 + 3] = if has_alpha { c[3] } else { 0xFF };
            }
            for y in 0..height {
                let start = (y * stride).min(rows.len());
                let row = &rows[start..(start + width).min(rows.len())];
                let base = y * width * 4;
                for (x, &idx) in row.iter().enumerate() {
                    let p = usize::from(idx) * 4;
                    if p + 4 <= table.len() {
                        out[base + x * 4..base + x * 4 + 4].copy_from_slice(&table[p..p + 4]);
                    }
                }
            }
        }
        4 => {
            let stride = (width * 2 + 3) & !3;
            for y in 0..height {
                let start = (y * stride).min(data.len());
                let row = &data[start..(start + width * 2).min(data.len())];
                let base = y * width * 4;
                for x in 0..width.min(row.len() / 2) {
                    let v = (u32::from(row[x * 2]) << 8) | u32::from(row[x * 2 + 1]);
                    let o = base + x * 4;
                    out[o] = (((v >> 10) & 31) * 255 / 31) as u8;
                    out[o + 1] = (((v >> 5) & 31) * 255 / 31) as u8;
                    out[o + 2] = ((v & 31) * 255 / 31) as u8;
                    out[o + 3] = 255;
                }
            }
        }
        5 => {
            for i in 0..width * height {
                let at = |k: usize| data.get(i * 4 + k).copied().unwrap_or(0);
                let [a, r, g, b] = [at(0), at(1), at(2), at(3)];
                let o = i * 4;
                let rgba = if !has_alpha || a == 255 {
                    [r, g, b, 255]
                } else if a == 0 {
                    [0, 0, 0, 0]
                } else {
                    let un = |c: u8| (u32::from(c) * 255 / u32::from(a)).min(255) as u8;
                    [un(r), un(g), un(b), a]
                };
                out[o..o + 4].copy_from_slice(&rgba);
            }
        }
        _ => return None,
    }
    Some(out)
}

/// `extract.MAX_INFLATED`.
pub const MAX_INFLATED: usize = 256 * 1024 * 1024;

/// `extract._decompress` for FWS and CWS movies; None for LZMA (ZWS) and every
/// case Python raises on.
pub fn movie(raw: &[u8]) -> Option<std::borrow::Cow<'_, [u8]>> {
    if raw.len() < 8 {
        return None;
    }
    match &raw[..3] {
        b"FWS" => Some(raw.into()),
        b"CWS" => {
            let body = crate::zlib::inflate(&raw[8..], MAX_INFLATED)?;
            let mut out = Vec::with_capacity(8 + body.data.len());
            out.extend_from_slice(b"FWS");
            out.extend_from_slice(&raw[3..8]);
            out.extend_from_slice(&body.data);
            Some(out.into())
        }
        _ => None,
    }
}

/// `extract._iter_tags`: `(code, body)` per top-level tag.
pub fn tags(data: &[u8]) -> Option<Vec<(u16, &[u8])>> {
    let first = *data.get(8)?;
    let nbits = (first >> 3) as usize;
    let mut p = 8 + (5 + nbits * 4).div_ceil(8) + 4;
    let mut out = Vec::new();
    loop {
        let Some(head) = data.get(p..p + 2) else { return Some(out) };
        p += 2;
        let packed = u16::from_le_bytes([head[0], head[1]]);
        let code = packed >> 6;
        let mut len = (packed & 0x3F) as usize;
        if len == 0x3F {
            let Some(raw) = data.get(p..p + 4) else { return Some(out) };
            p += 4;
            len = u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]) as usize;
        }
        let Some(body) = data.get(p..p + len) else { return Some(out) };
        p += len;
        out.push((code, body));
        if code == 0 {
            return Some(out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signed_bits() {
        let mut r = BitReader::new(&[0b1110_0000]);
        assert_eq!(r.read_signed(3), Some(-1));
        assert_eq!(r.byte_pos(), 1);
    }

    #[test]
    fn premultiplied_alpha_is_undone() {
        let px = lossless_pixels(5, true, 1, 1, 0, &[128, 64, 64, 64]).unwrap();
        assert_eq!(px, vec![127, 127, 127, 128]);
    }
}

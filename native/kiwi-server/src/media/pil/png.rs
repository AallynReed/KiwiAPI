//! PNG in and out the way Pillow 13 does it.
//!
//! - `decode`: `PngImagePlugin` modes and raw modes (16-bit samples keep
//!   their high byte, `I;16` excepted), `tRNS` and `iCCP` into `info`.
//! - `encode`: `save(format="PNG", compress_level=6)` of RGBA: IHDR, iCCP
//!   (CPython's zlib, default level), `ZipEncode.c`'s per-row filter choice
//!   without the optimize-only Average filter, deflate level 6 / memLevel 9 /
//!   Z_FILTERED (zlib-ng in Pillow's wheels, zlib-rs here), IDAT chunks of
//!   the encoder's buffer size.

use super::{Mode, Picture, Trns};

pub fn decode(b: &[u8], max_pixels: u64) -> Option<Picture> {
    let mut dec = ::png::Decoder::new(std::io::Cursor::new(b));
    dec.set_transformations(::png::Transformations::IDENTITY);
    dec.set_ignore_text_chunk(true);
    let mut reader = dec.read_info().ok()?;
    let info = reader.info();
    let (w, h) = (info.width as usize, info.height as usize);
    if w == 0 || h == 0 || (w as u64) * (h as u64) > max_pixels {
        return None;
    }
    let (color, depth) = (info.color_type, info.bit_depth as u8);
    let palette = info.palette.as_ref().map(|p| p.to_vec()).unwrap_or_default();
    let trns_raw = info.trns.as_ref().map(|t| t.to_vec());
    let icc = info.icc_profile.as_ref().map(|p| p.to_vec());
    let mut buf = vec![0u8; reader.output_buffer_size()?];
    let frame = reader.next_frame(&mut buf).ok()?;
    let line = frame.line_size;
    let i16be = |s: &[u8]| u32::from(u16::from_be_bytes([s[0], s[1]]));
    // png narrows sub-16-bit tRNS samples to their low byte, the only byte the
    // `convert_transparent` match looks at.
    let gray_trns = || match trns_raw.as_deref() {
        Some([v]) => Trns::Value(u32::from(*v)),
        Some(t) if t.len() >= 2 => Trns::Value(i16be(t)),
        _ => Trns::None,
    };
    let rgb_trns = || match trns_raw.as_deref() {
        Some([r, g, b]) => Trns::Rgb(u32::from(*r), u32::from(*g), u32::from(*b)),
        Some(t) if t.len() >= 6 => Trns::Rgb(i16be(&t[0..2]), i16be(&t[2..4]), i16be(&t[4..6])),
        _ => Trns::None,
    };

    use ::png::ColorType as C;
    let sample = |row: &[u8], x: usize, bits: u8| -> u8 {
        let per = 8 / bits as usize;
        let byte = row[x / per];
        let shift = 8 - bits as usize * (x % per + 1);
        (byte >> shift) & ((1u16 << bits) - 1) as u8
    };
    let mut data = Vec::new();
    let (mode, trns) = match (color, depth) {
        (C::Grayscale, 1) => {
            for y in 0..h {
                let row = &buf[y * line..];
                data.extend((0..w).map(|x| sample(row, x, 1)));
            }
            (Mode::One, gray_trns())
        }
        (C::Grayscale, 2 | 4 | 8) => {
            let mul = match depth {
                2 => 0x55,
                4 => 0x11,
                _ => 1,
            };
            for y in 0..h {
                let row = &buf[y * line..];
                data.extend((0..w).map(|x| if depth == 8 { row[x] } else { sample(row, x, depth) * mul }));
            }
            (Mode::L, gray_trns())
        }
        (C::Grayscale, 16) => {
            for y in 0..h {
                let row = &buf[y * line..];
                for x in 0..w {
                    data.extend_from_slice(&[row[x * 2 + 1], row[x * 2]]);
                }
            }
            (Mode::I16, gray_trns())
        }
        (C::Rgb, 8 | 16) => {
            let step = if depth == 16 { 2 } else { 1 };
            for y in 0..h {
                let row = &buf[y * line..];
                data.extend((0..w * 3).map(|i| row[i * step]));
            }
            (Mode::Rgb, rgb_trns())
        }
        (C::Indexed, 1 | 2 | 4 | 8) => {
            for y in 0..h {
                let row = &buf[y * line..];
                data.extend((0..w).map(|x| if depth == 8 { row[x] } else { sample(row, x, depth) }));
            }
            (Mode::P, trns_raw.map_or(Trns::None, Trns::Alphas))
        }
        (C::GrayscaleAlpha, 8 | 16) => {
            let step = if depth == 16 { 2 } else { 1 };
            for y in 0..h {
                let row = &buf[y * line..];
                data.extend((0..w * 2).map(|i| row[i * step]));
            }
            (Mode::La, Trns::None)
        }
        (C::Rgba, 8 | 16) => {
            let step = if depth == 16 { 2 } else { 1 };
            for y in 0..h {
                let row = &buf[y * line..];
                data.extend((0..w * 4).map(|i| row[i * step]));
            }
            (Mode::Rgba, Trns::None)
        }
        _ => return None,
    };
    Some(Picture { mode, width: w, height: h, data, icc, palette, trns })
}

fn cost(v: u8) -> i32 {
    if v < 128 { i32::from(v) } else { 256 - i32::from(v) }
}

/// `ImagingZipEncode` rows for `optimize=False`: none, up, sub, paeth.
fn filter_rows(px: &[u8], width: usize, height: usize, bpp: usize) -> Vec<u8> {
    let bytes = width * bpp;
    let mut out = Vec::with_capacity((bytes + 1) * height);
    let mut previous = vec![0u8; bytes + 1];
    let mut buffer = vec![0u8; bytes + 1];
    let (mut prior, mut up, mut paeth) = (vec![1u8; bytes + 1], vec![2u8; bytes + 1], vec![4u8; bytes + 1]);
    for y in 0..height {
        buffer[1..].copy_from_slice(&px[y * bytes..(y + 1) * bytes]);
        let mut choice = 0u8;
        let mut sum: i32 = buffer[1..].iter().map(|&v| cost(v)).sum();
        if sum > 0 {
            let mut s = 0;
            for i in 1..=bytes {
                let v = buffer[i].wrapping_sub(previous[i]);
                up[i] = v;
                s += cost(v);
            }
            if s < sum {
                choice = 2;
                sum = s;
            }
        }
        if sum > 0 {
            let mut s = 0;
            for i in 1..=bytes {
                let v = if i <= bpp { buffer[i] } else { buffer[i].wrapping_sub(buffer[i - bpp]) };
                prior[i] = v;
                s += cost(v);
            }
            if s < sum {
                choice = 1;
                sum = s;
            }
        }
        if sum > 0 {
            let mut s = 0;
            for i in 1..=bytes {
                let v = if i <= bpp {
                    buffer[i].wrapping_sub(previous[i])
                } else {
                    let a = i32::from(buffer[i - bpp]);
                    let b = i32::from(previous[i]);
                    let c = i32::from(previous[i - bpp]);
                    let (pa, pb, pc) = ((b - c).abs(), (a - c).abs(), (a + b - 2 * c).abs());
                    let pred = if pa <= pb && pa <= pc {
                        a
                    } else if pb <= pc {
                        b
                    } else {
                        c
                    };
                    buffer[i].wrapping_sub(pred as u8)
                };
                paeth[i] = v;
                s += cost(v);
            }
            if s < sum {
                choice = 4;
            }
        }
        out.extend_from_slice(match choice {
            0 => &buffer,
            1 => &prior,
            2 => &up,
            _ => &paeth,
        });
        std::mem::swap(&mut buffer, &mut previous);
        buffer[0] = 0;
    }
    out
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut h = crc32fast::Hasher::new();
    h.update(kind);
    h.update(data);
    out.extend_from_slice(&h.finalize().to_be_bytes());
}

/// CPython's `zlib.compress(data)`: stock zlib, level 6, memLevel 8.
pub fn py_zlib_compress(data: &[u8]) -> Vec<u8> {
    let mut len = unsafe { libz_sys::compressBound(data.len() as libz_sys::uLong) };
    let mut out = vec![0u8; len as usize];
    // SAFETY: `out` holds `compressBound` bytes and both buffers outlive the call.
    let rc =
        unsafe { libz_sys::compress2(out.as_mut_ptr(), &mut len, data.as_ptr(), data.len() as libz_sys::uLong, -1) };
    assert_eq!(rc, libz_sys::Z_OK, "zlib compress2 failed");
    out.truncate(len as usize);
    out
}

/// `img.save(out, format="PNG", optimize=False, compress_level=6)` of RGBA.
pub fn encode(rgba: &[u8], width: usize, height: usize, icc: Option<&[u8]>) -> Vec<u8> {
    let raw = filter_rows(rgba, width, height, 4);
    let cfg = zlib_rs::DeflateConfig {
        level: 6,
        method: zlib_rs::Method::Deflated,
        window_bits: 15,
        mem_level: 9,
        strategy: zlib_rs::Strategy::Filtered,
    };
    let parts = zip_encode(&raw, width * 4 + 1, (width * 4).max(65536), cfg);
    let mut out = Vec::with_capacity(parts.iter().map(Vec::len).sum::<usize>() + 128);
    out.extend_from_slice(b"\x89PNG\r\n\x1a\n");
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&(width as u32).to_be_bytes());
    ihdr.extend_from_slice(&(height as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    if let Some(icc) = icc.filter(|p| !p.is_empty()) {
        let mut data = b"ICC Profile\0\0".to_vec();
        data.extend_from_slice(&py_zlib_compress(icc));
        chunk(&mut out, b"iCCP", &data);
    }
    for part in &parts {
        chunk(&mut out, b"IDAT", part);
    }
    chunk(&mut out, b"IEND", &[]);
    out
}

/// `ImagingZipEncode`'s calls, one IDAT per call: each gets `bufsize` bytes of
/// output; a row the last call left half-consumed is finished first, then
/// whole rows go in one `deflate(Z_NO_FLUSH)` each until the output is full,
/// then `Z_FINISH`. zlib-ng's level-6 output depends on where input arrived
/// and where output ran out, so both are replayed as Pillow drives them.
fn zip_encode(raw: &[u8], row: usize, bufsize: usize, cfg: zlib_rs::DeflateConfig) -> Vec<Vec<u8>> {
    use zlib_rs::{DeflateFlush, Status};
    fn step(d: &mut zlib_rs::Deflate, input: &[u8], out: &mut [u8], flush: DeflateFlush) -> (usize, usize, Status) {
        let (i0, o0) = (d.total_in(), d.total_out());
        let st = d.compress(input, out, flush).expect("deflate");
        ((d.total_in() - i0) as usize, (d.total_out() - o0) as usize, st)
    }
    let mut d = zlib_rs::Deflate::new_with_config(cfg);
    let mut parts = Vec::new();
    let (mut next, mut pending): (usize, &[u8]) = (0, &[]);
    let (mut finishing, mut ended) = (false, false);
    loop {
        if ended {
            parts.push(Vec::new());
            return parts;
        }
        let mut buf = vec![0u8; bufsize];
        let mut used = 0;
        if !pending.is_empty() {
            let (n, o, _) = step(&mut d, pending, &mut buf[used..], DeflateFlush::NoFlush);
            pending = &pending[n..];
            used += o;
        }
        while !finishing && used < bufsize {
            if next >= raw.len() {
                finishing = true;
                break;
            }
            let end = (next + row).min(raw.len());
            let (n, o, _) = step(&mut d, &raw[next..end], &mut buf[used..], DeflateFlush::NoFlush);
            pending = &raw[next + n..end];
            next = end;
            used += o;
        }
        if finishing {
            while used < bufsize {
                let (_, o, st) = step(&mut d, &[], &mut buf[used..], DeflateFlush::Finish);
                used += o;
                if st == Status::StreamEnd {
                    ended = true;
                    break;
                }
            }
        }
        buf.truncate(used);
        parts.push(buf);
        if ended && used < bufsize {
            return parts;
        }
    }
}

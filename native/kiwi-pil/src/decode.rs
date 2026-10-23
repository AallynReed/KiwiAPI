//! `Image.open(png).convert("RGBA")` as Pillow 13 does it: `PngImagePlugin`'s
//! mode and rawmode per bit depth and colour type, its tRNS handling and the
//! `Convert.c` paths to RGBA. 16-bit greyscale ("I;16") is not covered.

use crate::image::{Ink, Rgba};

/// `Image.MAX_IMAGE_PIXELS * 2`: above it Pillow raises `DecompressionBombError`.
const BOMB_PIXELS: u64 = 2 * (1024 * 1024 * 1024 / 4 / 3);

/// Bit depth and colour type from the IHDR, if `data` starts like a PNG.
pub fn png_header(data: &[u8]) -> Option<(u8, u8)> {
    if data.len() < 33 || &data[..8] != b"\x89PNG\r\n\x1a\n" || &data[12..16] != b"IHDR" {
        return None;
    }
    Some((data[24], data[25]))
}

/// Whether [`png_to_rgba`] converts this PNG the way Pillow would (everything
/// but 16-bit greyscale, which Pillow opens as "I;16").
pub fn png_supported(data: &[u8]) -> bool {
    matches!(png_header(data), Some((d, c)) if !(c == 0 && d == 16))
}

fn unpack_gray(row: &[u8], width: usize, depth: u8) -> Vec<u8> {
    let per = 8 / depth as usize;
    let scale = match depth {
        1 => 255,
        2 => 0x55,
        4 => 0x11,
        _ => 1,
    };
    (0..width)
        .map(|x| {
            let byte = row[x / per];
            let shift = 8 - depth as usize * (x % per + 1);
            ((byte >> shift) & ((1u16 << depth) - 1) as u8) * scale
        })
        .collect()
}

fn unpack_index(row: &[u8], width: usize, depth: u8) -> Vec<u8> {
    if depth == 8 {
        return row[..width].to_vec();
    }
    let per = 8 / depth as usize;
    (0..width)
        .map(|x| {
            let shift = 8 - depth as usize * (x % per + 1);
            (row[x / per] >> shift) & ((1u16 << depth) - 1) as u8
        })
        .collect()
}

/// The raw tRNS payload (the decoder normalises it; Pillow reads the bytes).
fn raw_trns(data: &[u8]) -> Option<&[u8]> {
    let mut i = 8;
    while i + 8 <= data.len() {
        let n = u32::from_be_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]) as usize;
        let kind = &data[i + 4..i + 8];
        if kind == b"IDAT" {
            return None;
        }
        if kind == b"tRNS" {
            return data.get(i + 8..i + 8 + n);
        }
        i = i.checked_add(12 + n)?;
    }
    None
}

fn be16(s: &[u8], i: usize) -> u16 {
    if s.len() >= i + 2 { u16::from_be_bytes([s[i], s[i + 1]]) } else { 0 }
}

/// `Image.open(BytesIO(data)).convert("RGBA")` for a PNG.
pub fn png_to_rgba(data: &[u8]) -> Result<Rgba, String> {
    if !png_supported(data) {
        return Err("unsupported PNG mode".into());
    }
    let mut dec = png::Decoder::new_with_limits(std::io::Cursor::new(data), png::Limits { bytes: usize::MAX });
    dec.set_transformations(png::Transformations::IDENTITY);
    let mut reader = dec.read_info().map_err(|e| e.to_string())?;
    let (w, h) = (reader.info().width as usize, reader.info().height as usize);
    if (w as u64) * (h as u64) > BOMB_PIXELS {
        return Err("decompression bomb".into());
    }
    let depth = reader.info().bit_depth as u8;
    let color = reader.info().color_type as u8;
    let palette = reader.info().palette.as_ref().map(|p| p.to_vec());
    let trns = raw_trns(data).map(<[u8]>::to_vec);
    let size = reader.output_buffer_size().ok_or("image too large")?;
    let mut buf = vec![0u8; size];
    let info = reader.next_frame(&mut buf).map_err(|e| e.to_string())?;
    let line = info.line_size;
    let row = |y: usize| &buf[y * line..(y + 1) * line];
    let mut out = Rgba::new(w, h, [0, 0, 0, 255]);
    let px = |o: &mut Rgba, x: usize, y: usize, p: Ink| o.data[y * w + x] = p;
    let wide = depth == 16;
    let hi = |r: &[u8], i: usize| if wide { r[i * 2] } else { r[i] };
    match color {
        0 => {
            let key = trns.as_ref().map(|t| (be16(t, 0) & 0xff) as u8);
            for y in 0..h {
                for (x, v) in unpack_gray(row(y), w, depth).into_iter().enumerate() {
                    let a = if key == Some(v) { 0 } else { 255 };
                    px(&mut out, x, y, [v, v, v, a]);
                }
            }
        }
        2 => {
            let key = trns.as_ref().map(|t| [be16(t, 0) as u8, be16(t, 2) as u8, be16(t, 4) as u8]);
            for y in 0..h {
                let r = row(y);
                for x in 0..w {
                    let c = [hi(r, x * 3), hi(r, x * 3 + 1), hi(r, x * 3 + 2)];
                    let a = if key == Some(c) { 0 } else { 255 };
                    px(&mut out, x, y, [c[0], c[1], c[2], a]);
                }
            }
        }
        3 => {
            let mut pal: Vec<Ink> = vec![[0, 0, 0, 255]; 256];
            if let Some(p) = &palette {
                for (i, c) in p.chunks_exact(3).take(256).enumerate() {
                    pal[i] = [c[0], c[1], c[2], 255];
                }
            }
            if let Some(t) = &trns {
                for (i, &a) in t.iter().take(256).enumerate() {
                    pal[i][3] = a;
                }
            }
            for y in 0..h {
                for (x, i) in unpack_index(row(y), w, depth).into_iter().enumerate() {
                    px(&mut out, x, y, pal[i as usize]);
                }
            }
        }
        4 => {
            for y in 0..h {
                let r = row(y);
                for x in 0..w {
                    let (l, a) = (hi(r, x * 2), hi(r, x * 2 + 1));
                    px(&mut out, x, y, [l, l, l, a]);
                }
            }
        }
        6 => {
            for y in 0..h {
                let r = row(y);
                for x in 0..w {
                    px(&mut out, x, y, [hi(r, x * 4), hi(r, x * 4 + 1), hi(r, x * 4 + 2), hi(r, x * 4 + 3)]);
                }
            }
        }
        _ => return Err("unknown PNG mode".into()),
    }
    Ok(out)
}

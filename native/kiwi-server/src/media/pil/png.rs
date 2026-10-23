//! PNG in and out the way Pillow 13 does it.
//!
//! - `decode`: `PngImagePlugin` modes and raw modes (16-bit samples keep
//!   their high byte, `I;16` excepted), `tRNS` and `iCCP` into `info`.
//! - `encode`: `save(format="PNG", compress_level=6)` of RGBA, which is
//!   kiwi-pil's default-save encoder plus the iCCP chunk.

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

/// `img.save(out, format="PNG", optimize=False, compress_level=6)` of RGBA.
pub fn encode(rgba: &[u8], width: usize, height: usize, icc: Option<&[u8]>) -> Vec<u8> {
    kiwi_pil::png::encode_rgba_icc(rgba, width, height, icc)
}

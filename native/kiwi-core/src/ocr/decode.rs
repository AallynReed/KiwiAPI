//! `Image.open(io.BytesIO(data)).convert("RGB")` (Pillow 13) for the formats
//! the OCR route accepts: PNG, JPEG, GIF (first frame), BMP and WebP.

use std::io::Cursor;

use super::cv::Img;

/// Pillow's `MAX_IMAGE_PIXELS`; twice it raises `DecompressionBombError`.
pub const MAX_IMAGE_PIXELS: u64 = 1024 * 1024 * 1024 / 4 / 3;

#[derive(Debug, PartialEq, Eq)]
pub enum DecodeError {
    /// `UnidentifiedImageError` / `OSError`: a 400 with this text.
    Unreadable(String),
    /// `DecompressionBombError`: an unhandled 500.
    Bomb,
}

fn unidentified() -> DecodeError {
    DecodeError::Unreadable("cannot identify image file <_io.BytesIO object at 0x7f0000000000>".into())
}

fn truncated() -> DecodeError {
    DecodeError::Unreadable("image file is truncated".into())
}

fn bomb_check(w: u64, h: u64) -> Result<(), DecodeError> {
    if w * h > 2 * MAX_IMAGE_PIXELS { Err(DecodeError::Bomb) } else { Ok(()) }
}

pub fn decode_rgb(data: &[u8]) -> Result<Img, DecodeError> {
    if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        png(data)
    } else if data.starts_with(b"\xff\xd8\xff") {
        jpeg(data)
    } else if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
        gif(data)
    } else if data.starts_with(b"BM") {
        bmp(data)
    } else if data.len() >= 12 && &data[..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        webp(data)
    } else {
        Err(unidentified())
    }
}

fn png(data: &[u8]) -> Result<Img, DecodeError> {
    let probe = ::png::Decoder::new(Cursor::new(data)).read_info().map_err(|_| unidentified())?;
    let info = probe.info();
    let (w, h, depth) = (info.width as usize, info.height as usize, info.bit_depth);
    bomb_check(w as u64, h as u64)?;
    let sixteen = depth == ::png::BitDepth::Sixteen;
    let mut dec = ::png::Decoder::new_with_limits(Cursor::new(data), ::png::Limits { bytes: usize::MAX });
    dec.set_transformations(if sixteen { ::png::Transformations::IDENTITY } else { ::png::Transformations::EXPAND });
    let mut reader = dec.read_info().map_err(|_| unidentified())?;
    let mut buf = vec![0u8; reader.output_buffer_size().ok_or_else(truncated)?];
    let out = reader.next_frame(&mut buf).map_err(|_| truncated())?;
    let (ct, _) = (out.color_type, out.bit_depth);
    let mut img = Img::new(w, h, 3);
    let line = out.line_size;
    for y in 0..h {
        let row = &buf[y * line..];
        for x in 0..w {
            let px: [u8; 3] = match (ct, sixteen) {
                (::png::ColorType::Grayscale, false) => [row[x]; 3],
                (::png::ColorType::GrayscaleAlpha, false) => [row[2 * x]; 3],
                (::png::ColorType::Rgb, false) => [row[3 * x], row[3 * x + 1], row[3 * x + 2]],
                (::png::ColorType::Rgba, false) => [row[4 * x], row[4 * x + 1], row[4 * x + 2]],
                // I;16 -> RGB clamps; the other 16-bit modes keep the high byte.
                (::png::ColorType::Grayscale, true) => {
                    [u16::from_be_bytes([row[2 * x], row[2 * x + 1]]).min(255) as u8; 3]
                }
                (::png::ColorType::GrayscaleAlpha, true) => [row[4 * x]; 3],
                (::png::ColorType::Rgb, true) => [row[6 * x], row[6 * x + 2], row[6 * x + 4]],
                (::png::ColorType::Rgba, true) => [row[8 * x], row[8 * x + 2], row[8 * x + 4]],
                (::png::ColorType::Indexed, _) => return Err(truncated()),
            };
            img.data[(y * w + x) * 3..(y * w + x) * 3 + 3].copy_from_slice(&px);
        }
    }
    Ok(img)
}

/// libjpeg-turbo's decoder (vendored by mozjpeg-sys, C-only) with Pillow's
/// settings: ISLOW DCT, fancy upsampling, libjpeg's own RGB conversion.
fn jpeg(data: &[u8]) -> Result<Img, DecodeError> {
    let header = std::panic::catch_unwind(|| {
        mozjpeg::Decompress::new_mem(data).map(|d| (d.width(), d.height())).map_err(|_| ())
    });
    let (w, h) = match header {
        Ok(Ok(size)) => size,
        _ => return Err(unidentified()),
    };
    bomb_check(w as u64, h as u64)?;
    let pixels = std::panic::catch_unwind(|| -> std::io::Result<Vec<u8>> {
        let mut started = mozjpeg::Decompress::new_mem(data)?.rgb()?;
        let px = started.read_scanlines::<u8>()?;
        started.finish()?;
        Ok(px)
    });
    match pixels {
        Ok(Ok(px)) if px.len() == w * h * 3 => Ok(Img { w, h, c: 3, data: px }),
        _ => Err(truncated()),
    }
}

fn gif(data: &[u8]) -> Result<Img, DecodeError> {
    let mut opts = ::gif::DecodeOptions::new();
    opts.set_color_output(::gif::ColorOutput::Indexed);
    let mut dec = opts.read_info(Cursor::new(data)).map_err(|_| unidentified())?;
    let (w, h) = (dec.width() as usize, dec.height() as usize);
    bomb_check(w as u64, h as u64)?;
    let global = dec.global_palette().map(<[u8]>::to_vec);
    let frame = dec.read_next_frame().map_err(|_| truncated())?.ok_or_else(truncated)?;
    let palette = frame.palette.clone().or(global).unwrap_or_default();
    let fill = frame.transparent.unwrap_or(0);
    let mut idx = vec![fill; w * h];
    for y in 0..frame.height as usize {
        for x in 0..frame.width as usize {
            let (cx, cy) = (frame.left as usize + x, frame.top as usize + y);
            if cx < w && cy < h {
                idx[cy * w + cx] = frame.buffer[y * frame.width as usize + x];
            }
        }
    }
    let mut img = Img::new(w, h, 3);
    for (i, &p) in idx.iter().enumerate() {
        let o = p as usize * 3;
        for k in 0..3 {
            img.data[i * 3 + k] = palette.get(o + k).copied().unwrap_or(0);
        }
    }
    Ok(img)
}

fn le32(d: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(d.get(o..o + 4)?.try_into().ok()?))
}

fn le16(d: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes(d.get(o..o + 2)?.try_into().ok()?))
}

/// The uncompressed BMP layouts (1/4/8-bit palette, 24-bit, 32-bit).
fn bmp(data: &[u8]) -> Result<Img, DecodeError> {
    let hdr = || -> Option<(usize, u32, i32, i32, u16, u32, u32)> {
        let offset = le32(data, 10)? as usize;
        let size = le32(data, 14)?;
        let w = le32(data, 18)? as i32;
        let h = le32(data, 22)? as i32;
        let bits = le16(data, 28)?;
        let comp = le32(data, 30)?;
        let colors = le32(data, 46).unwrap_or(0);
        (size >= 40).then_some((offset, size, w, h, bits, comp, colors))
    };
    let (offset, size, w, h, bits, comp, colors) = hdr().ok_or_else(unidentified)?;
    if w <= 0 || h == 0 || !(comp == 0 || (comp == 3 && bits == 32)) || ![1, 4, 8, 24, 32].contains(&bits) {
        return Err(unidentified());
    }
    let (w, top_down, hh) = (w as usize, h < 0, h.unsigned_abs() as usize);
    bomb_check(w as u64, hh as u64)?;
    let ncolors = if colors == 0 && bits <= 8 { 1usize << bits } else { colors as usize };
    let pal_start = 14 + size as usize;
    let stride = (w * bits as usize).div_ceil(32) * 4;
    let mut img = Img::new(w, hh, 3);
    for y in 0..hh {
        let src_row = if top_down { y } else { hh - 1 - y };
        let row = data.get(offset + src_row * stride..offset + (src_row + 1) * stride).ok_or_else(truncated)?;
        for x in 0..w {
            let bgr: [u8; 3] = match bits {
                24 => [row[3 * x], row[3 * x + 1], row[3 * x + 2]],
                32 => [row[4 * x], row[4 * x + 1], row[4 * x + 2]],
                _ => {
                    let per = 8 / bits as usize;
                    let byte = row[x / per];
                    let shift = 8 - bits as usize * (x % per + 1);
                    let i = ((byte >> shift) & ((1u16 << bits) - 1) as u8) as usize;
                    if i >= ncolors {
                        [0, 0, 0]
                    } else {
                        let p = data.get(pal_start + 4 * i..pal_start + 4 * i + 3).ok_or_else(truncated)?;
                        [p[0], p[1], p[2]]
                    }
                }
            };
            let o = (y * w + x) * 3;
            img.data[o..o + 3].copy_from_slice(&[bgr[2], bgr[1], bgr[0]]);
        }
    }
    Ok(img)
}

fn webp(data: &[u8]) -> Result<Img, DecodeError> {
    let mut dec = image_webp::WebPDecoder::new(Cursor::new(data)).map_err(|_| unidentified())?;
    let (w, h) = dec.dimensions();
    bomb_check(w as u64, h as u64)?;
    let size = dec.output_buffer_size().ok_or_else(truncated)?;
    let mut buf = vec![0u8; size];
    dec.read_image(&mut buf).map_err(|_| truncated())?;
    let (w, h) = (w as usize, h as usize);
    let ch = size / (w * h).max(1);
    let mut img = Img::new(w, h, 3);
    for i in 0..w * h {
        img.data[i * 3..i * 3 + 3].copy_from_slice(&buf[i * ch..i * ch + 3]);
    }
    Ok(img)
}

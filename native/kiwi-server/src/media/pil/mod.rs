//! The Pillow 13 operations `app/trove/swf/extract.py` runs on a bitmap:
//! `Image.open` + `convert("RGBA")` for JPEG/PNG/GIF payloads, the `RGBa`
//! merge that un-premultiplies a JPEG3 alpha plane, `thumbnail(LANCZOS)` and
//! `save("PNG", compress_level=6)`, each with Pillow's exact arithmetic.

mod gif;
pub mod jpeg;
mod png;
pub mod resample;

pub use self::png::encode as encode_png;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    One,
    L,
    La,
    P,
    Rgb,
    #[default]
    Rgba,
    Cmyk,
    I16,
}

/// `im.info["transparency"]` as the PNG and GIF readers set it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Trns {
    #[default]
    None,
    Value(u32),
    Rgb(u32, u32, u32),
    Alphas(Vec<u8>),
}

/// A loaded image: `data` in the mode's own layout (`I16` little-endian).
#[derive(Clone, Debug, Default)]
pub struct Picture {
    pub mode: Mode,
    pub width: usize,
    pub height: usize,
    pub data: Vec<u8>,
    pub icc: Option<Vec<u8>>,
    /// RGB triples, `P` only; entries past it are Pillow's grey ramp.
    pub palette: Vec<u8>,
    pub trns: Trns,
}

/// `(tmp = a * b + 128, ((tmp >> 8) + tmp) >> 8)`.
#[inline]
pub fn muldiv255(a: u32, b: u32) -> u32 {
    let t = a * b + 128;
    ((t >> 8) + t) >> 8
}

#[inline]
fn clip8(v: i32) -> u8 {
    v.clamp(0, 255) as u8
}

/// `Image.convert("RGBA")`.
pub fn to_rgba(p: &Picture) -> Vec<u8> {
    let n = p.width * p.height;
    let mut out = Vec::with_capacity(n * 4);
    match p.mode {
        Mode::Rgba => out.extend_from_slice(&p.data[..n * 4]),
        Mode::Rgb => {
            for px in p.data[..n * 3].chunks_exact(3) {
                out.extend_from_slice(&[px[0], px[1], px[2], 255]);
            }
        }
        Mode::L | Mode::One | Mode::I16 => {
            for i in 0..n {
                let v = match p.mode {
                    Mode::L => p.data[i],
                    Mode::One => {
                        if p.data[i] != 0 {
                            255
                        } else {
                            0
                        }
                    }
                    _ => {
                        if p.data[i * 2 + 1] == 0 {
                            p.data[i * 2]
                        } else {
                            255
                        }
                    }
                };
                out.extend_from_slice(&[v, v, v, 255]);
            }
        }
        Mode::La => {
            for px in p.data[..n * 2].chunks_exact(2) {
                out.extend_from_slice(&[px[0], px[0], px[0], px[1]]);
            }
        }
        Mode::Cmyk => {
            for px in p.data[..n * 4].chunks_exact(4) {
                let nk = 255 - u32::from(px[3]);
                let ch = |c: u8| clip8(nk as i32 - muldiv255(u32::from(c), nk) as i32);
                out.extend_from_slice(&[ch(px[0]), ch(px[1]), ch(px[2]), 255]);
            }
        }
        Mode::P => {
            let mut table = [[0u8; 4]; 256];
            for (i, e) in table.iter_mut().enumerate() {
                *e = match p.palette.get(i * 3..i * 3 + 3) {
                    Some(c) => [c[0], c[1], c[2], 255],
                    None => [i as u8, i as u8, i as u8, 255],
                };
            }
            match &p.trns {
                Trns::Alphas(a) => {
                    for (e, &v) in table.iter_mut().zip(a.iter()) {
                        e[3] = v;
                    }
                }
                Trns::Value(i) if (*i as usize) < 256 => table[*i as usize][3] = 0,
                _ => {}
            }
            for &i in &p.data[..n] {
                out.extend_from_slice(&table[usize::from(i)]);
            }
        }
    }
    // `convert_transparent`: an exact (r, g, b, 255) match loses its alpha.
    let key = match (&p.mode, &p.trns) {
        (Mode::Rgb, Trns::Rgb(r, g, b)) => Some([*r as u8, *g as u8, *b as u8]),
        (Mode::L | Mode::One | Mode::I16, Trns::Value(v)) => Some([*v as u8; 3]),
        _ => None,
    };
    if let Some(k) = key {
        for px in out.chunks_exact_mut(4) {
            if px[..3] == k && px[3] == 255 {
                px[3] = 0;
            }
        }
    }
    out
}

/// `RGBA -> RGBa` (`rgbA2rgba`).
pub fn premultiply(rgba: &mut [u8]) {
    for px in rgba.chunks_exact_mut(4) {
        let a = u32::from(px[3]);
        for c in &mut px[..3] {
            *c = muldiv255(u32::from(*c), a) as u8;
        }
    }
}

/// `RGBa -> RGBA` (`rgba2rgbA`).
pub fn unpremultiply(rgba: &mut [u8]) {
    for px in rgba.chunks_exact_mut(4) {
        let a = u32::from(px[3]);
        if a != 0 && a != 255 {
            for c in &mut px[..3] {
                *c = (255 * u32::from(*c) / a).min(255) as u8;
            }
        }
    }
}

/// `Image.open(payload)` for the three formats a SWF embeds; None wherever
/// Pillow raises. `max_pixels` rejects early what the caller would reject
/// after decoding anyway.
pub fn open(payload: &[u8], max_pixels: u64) -> Option<Picture> {
    if payload.starts_with(b"\x89PNG\r\n\x1a\n") {
        return png::decode(payload, max_pixels);
    }
    if payload.starts_with(b"GIF87a") || payload.starts_with(b"GIF89a") {
        return gif::decode(payload, max_pixels);
    }
    jpeg::open(payload, max_pixels)
}

/// `img.thumbnail((box, box), LANCZOS)` of an RGBA image: None when it
/// already fits (no resize happens), else the resized pixels and size.
pub fn thumbnail(rgba: &[u8], width: usize, height: usize, bound: usize) -> Option<(Vec<u8>, usize, usize)> {
    let (x, y) = (bound, bound);
    if x >= width && y >= height {
        return None;
    }
    let (w, h) = thumb_size(width, height, x, y);
    if (w, h) == (width, height) {
        return None;
    }
    let mut pre = rgba.to_vec();
    premultiply(&mut pre);
    // `resize(..., reducing_gap=2.0)`: an integer box reduce first when the
    // shrink is at least twice the gap, then LANCZOS over the reduced box.
    let fx = ((width as f64 / w as f64 / 2.0) as usize).max(1);
    let fy = ((height as f64 / h as f64 / 2.0) as usize).max(1);
    let (src, sw, sh, bx) = if fx > 1 || fy > 1 {
        let (r, rw, rh) = resample::reduce(&pre, width, height, fx, fy);
        (r, rw, rh, [0.0, 0.0, (width as f64 / fx as f64) as f32, (height as f64 / fy as f64) as f32])
    } else {
        (pre, width, height, [0.0, 0.0, width as f32, height as f32])
    };
    let mut out = resample::lanczos(&src, sw, sh, w, h, bx);
    unpremultiply(&mut out);
    Some((out, w, h))
}

/// `thumbnail`'s `preserve_aspect_ratio`.
pub fn thumb_size(width: usize, height: usize, x: usize, y: usize) -> (usize, usize) {
    let aspect = width as f64 / height as f64;
    let round_aspect = |number: f64, key: &dyn Fn(f64) -> f64| -> usize {
        let (lo, hi) = (number.floor(), number.ceil());
        let pick = if key(hi) < key(lo) { hi } else { lo };
        (pick as usize).max(1)
    };
    if x as f64 / y as f64 >= aspect {
        let yf = y as f64;
        (round_aspect(yf * aspect, &|n| (aspect - n / yf).abs()), y)
    } else {
        let xf = x as f64;
        (x, round_aspect(xf / aspect, &|n| if n == 0.0 { 0.0 } else { (aspect - xf / n).abs() }))
    }
}

#[cfg(test)]
pub(crate) mod tests;

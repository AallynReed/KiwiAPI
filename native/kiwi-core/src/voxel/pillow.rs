//! The Pillow 13 steps of `_contain_square`: `Image.resize(..., LANCZOS)` on an
//! RGBA image (premultiplied through "RGBa", `Resample.c`'s 8-bit two-pass
//! filter) and `paste(img, box, img)` onto a transparent canvas (`Paste.c`).

use crate::bpedit::py::round_half_even;

/// Straight RGBA, row-major.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    pub w: usize,
    pub h: usize,
    pub px: Vec<u8>,
}

const PRECISION_BITS: u32 = 32 - 8 - 2;

fn muldiv255(a: u32, b: u32) -> u8 {
    let t = a * b + 128;
    (((t >> 8) + t) >> 8) as u8
}

fn div255(a: u32) -> u8 {
    let t = a + 128;
    (((t >> 8) + t) >> 8) as u8
}

fn premultiply(px: &mut [u8]) {
    for p in px.chunks_exact_mut(4) {
        let a = u32::from(p[3]);
        for c in &mut p[..3] {
            *c = muldiv255(u32::from(*c), a);
        }
    }
}

fn unpremultiply(px: &mut [u8]) {
    for p in px.chunks_exact_mut(4) {
        let a = u32::from(p[3]);
        if a != 0 && a != 255 {
            for c in &mut p[..3] {
                *c = ((255 * u32::from(*c)) / a).min(255) as u8;
            }
        }
    }
}

fn sinc(x: f64) -> f64 {
    if x == 0.0 {
        return 1.0;
    }
    let x = x * std::f64::consts::PI;
    x.sin() / x
}

fn lanczos(x: f64) -> f64 {
    if (-3.0..3.0).contains(&x) { sinc(x) * sinc(x / 3.0) } else { 0.0 }
}

/// `precompute_coeffs` + `normalize_coeffs_8bpc`: per output pixel, the first
/// source index and its fixed-point weights.
fn coeffs(in_size: usize, out_size: usize) -> (usize, Vec<(usize, Vec<i32>)>) {
    let (in0, in1) = (0.0f32, in_size as f32);
    let scale = f64::from(in1 - in0) / out_size as f64;
    let filterscale = scale.max(1.0);
    let support = 3.0 * filterscale;
    let ksize = support.ceil() as usize * 2 + 1;
    let mut out = Vec::with_capacity(out_size);
    for xx in 0..out_size {
        let center = f64::from(in0) + (xx as f64 + 0.5) * scale;
        let ss = 1.0 / filterscale;
        let xmin = ((center - support + 0.5) as i64).max(0) as usize;
        let xmax = ((center + support + 0.5) as i64).min(in_size as i64) as usize - xmin;
        let mut k: Vec<f64> = (0..xmax).map(|x| lanczos((x as f64 + xmin as f64 - center + 0.5) * ss)).collect();
        let ww: f64 = k.iter().sum();
        if ww != 0.0 {
            for v in &mut k {
                *v /= ww;
            }
        }
        let fixed = k
            .iter()
            .map(|&v| {
                let scaled = v * f64::from(1u32 << PRECISION_BITS);
                if v < 0.0 { (-0.5 + scaled) as i32 } else { (0.5 + scaled) as i32 }
            })
            .collect();
        out.push((xmin, fixed));
    }
    (ksize, out)
}

fn clip8(v: i32) -> u8 {
    (v >> PRECISION_BITS).clamp(0, 255) as u8
}

/// `ImagingResampleInner` for 8-bit RGBa.
fn resample(src: &Image, w: usize, h: usize) -> Image {
    let need_h = w != src.w;
    let need_v = h != src.h;
    let (_, horiz) = coeffs(src.w, w);
    let (_, vert) = coeffs(src.h, h);
    let first = vert[0].0;
    let last = vert[h - 1].0 + vert[h - 1].1.len();
    let mut cur = Image { w: src.w, h: src.h, px: src.px.clone() };
    let mut row_base = 0usize;
    if need_h {
        let rows = last - first;
        let mut out = vec![0u8; w * rows * 4];
        for yy in 0..rows {
            for (xx, (xmin, k)) in horiz.iter().enumerate() {
                let mut ss = [1i32 << (PRECISION_BITS - 1); 4];
                for (x, &kx) in k.iter().enumerate() {
                    let p = &src.px[((yy + first) * src.w + x + xmin) * 4..][..4];
                    for c in 0..4 {
                        ss[c] += i32::from(p[c]) * kx;
                    }
                }
                for c in 0..4 {
                    out[(yy * w + xx) * 4 + c] = clip8(ss[c]);
                }
            }
        }
        cur = Image { w, h: rows, px: out };
        row_base = first;
    }
    if need_v {
        let mut out = vec![0u8; cur.w * h * 4];
        for (yy, (ymin, k)) in vert.iter().enumerate() {
            let ymin = ymin - row_base;
            for xx in 0..cur.w {
                let mut ss = [1i32 << (PRECISION_BITS - 1); 4];
                for (y, &ky) in k.iter().enumerate() {
                    let p = &cur.px[((y + ymin) * cur.w + xx) * 4..][..4];
                    for c in 0..4 {
                        ss[c] += i32::from(p[c]) * ky;
                    }
                }
                for c in 0..4 {
                    out[(yy * cur.w + xx) * 4 + c] = clip8(ss[c]);
                }
            }
        }
        cur = Image { w: cur.w, h, px: out };
    }
    cur
}

/// `img.resize((w, h), Image.LANCZOS)` for an RGBA image.
pub fn resize_lanczos(img: &Image, w: usize, h: usize) -> Image {
    if (w, h) == (img.w, img.h) {
        return img.clone();
    }
    let mut pre = img.clone();
    premultiply(&mut pre.px);
    let mut out = resample(&pre, w, h);
    unpremultiply(&mut out.px);
    out
}

/// `_contain_square(rgba, dim)`: crop to alpha > 8, scale into the padded
/// square and paste centred on a transparent canvas (the image is its own mask).
pub fn contain_square(rgba: Vec<u8>, dim: usize) -> Vec<u8> {
    let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0usize, 0usize);
    for y in 0..dim {
        for x in 0..dim {
            if rgba[(y * dim + x) * 4 + 3] > 8 {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
        }
    }
    if x0 == usize::MAX {
        return rgba;
    }
    let (cw, ch) = (x1 - x0 + 1, y1 - y0 + 1);
    let mut crop = Vec::with_capacity(cw * ch * 4);
    for y in y0..=y1 {
        crop.extend_from_slice(&rgba[(y * dim + x0) * 4..(y * dim + x1 + 1) * 4]);
    }
    let pad = round_half_even(dim as f64 * 0.06).max(0) as usize;
    let avail = dim.saturating_sub(2 * pad).max(1) as f64;
    let scale = (avail / cw as f64).min(avail / ch as f64);
    let nw = round_half_even(cw as f64 * scale).max(1) as usize;
    let nh = round_half_even(ch as f64 * scale).max(1) as usize;
    let img = resize_lanczos(&Image { w: cw, h: ch, px: crop }, nw, nh);
    let mut canvas = vec![0u8; dim * dim * 4];
    let (dx, dy) = ((dim as i64 - nw as i64).div_euclid(2), (dim as i64 - nh as i64).div_euclid(2));
    for y in 0..nh as i64 {
        let ty = y + dy;
        if !(0..dim as i64).contains(&ty) {
            continue;
        }
        for x in 0..nw as i64 {
            let tx = x + dx;
            if !(0..dim as i64).contains(&tx) {
                continue;
            }
            let s = &img.px[((y as usize) * nw + x as usize) * 4..][..4];
            let a = u32::from(s[3]);
            let d = &mut canvas[((ty as usize) * dim + tx as usize) * 4..][..4];
            for c in 0..4 {
                d[c] = div255(u32::from(d[c]) * (255 - a) + u32::from(s[c]) * a);
            }
        }
    }
    canvas
}

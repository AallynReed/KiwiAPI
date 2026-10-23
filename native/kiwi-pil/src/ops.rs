//! The whole-image operations Image Studio's renderer calls: `linear_gradient`,
//! `resize` (BICUBIC, `Resample.c`), `rotate` (BILINEAR affine, `Geometry.c`),
//! `crop`, `paste` with and without an "L" mask (`Paste.c`), `point` and
//! `alpha_composite` (`AlphaComposite.c`).

use crate::image::{Ink, Mask, Rgba, blend};

/// `Image.linear_gradient("L")`: 256x256, each row its own index.
pub fn linear_gradient() -> Mask {
    let mut m = Mask::new(256, 256);
    for y in 0..256 {
        m.row_mut(y).fill(y as u8);
    }
    m
}

const PRECISION_BITS: u32 = 32 - 8 - 2;

fn bicubic(x: f64) -> f64 {
    let a = -0.5;
    let x = if x < 0.0 { -x } else { x };
    if x < 1.0 {
        return ((a + 2.0) * x - (a + 3.0)) * x * x + 1.0;
    }
    if x < 2.0 {
        return (((x - 5.0) * x + 8.0) * x - 4.0) * a;
    }
    0.0
}

fn clip8(v: i32) -> u8 {
    (v >> PRECISION_BITS).clamp(0, 255) as u8
}

/// `precompute_coeffs` + `normalize_coeffs_8bpc` for the default box.
fn coeffs(in_size: usize, out_size: usize) -> (usize, Vec<(usize, usize)>, Vec<i32>) {
    let (in0, in1) = (0.0f32, in_size as f32);
    let scale = (in1 - in0) as f64 / out_size as f64;
    let filterscale = if scale < 1.0 { 1.0 } else { scale };
    let support = 2.0 * filterscale;
    let ksize = support.ceil() as usize * 2 + 1;
    let mut bounds = Vec::with_capacity(out_size);
    let mut kk = vec![0.0f64; out_size * ksize];
    for xx in 0..out_size {
        let center = in0 as f64 + (xx as f64 + 0.5) * scale;
        let ss = 1.0 / filterscale;
        let mut xmin = (center - support + 0.5) as i64;
        if xmin < 0 {
            xmin = 0;
        }
        let mut xmax = (center + support + 0.5) as i64;
        if xmax > in_size as i64 {
            xmax = in_size as i64;
        }
        xmax -= xmin;
        let k = &mut kk[xx * ksize..(xx + 1) * ksize];
        let mut ww = 0.0;
        for (x, slot) in k.iter_mut().take(xmax.max(0) as usize).enumerate() {
            let w = bicubic(((x as i64 + xmin) as f64 - center + 0.5) * ss);
            *slot = w;
            ww += w;
        }
        for v in k.iter_mut().take(xmax.max(0) as usize) {
            if ww != 0.0 {
                *v /= ww;
            }
        }
        bounds.push((xmin as usize, xmax.max(0) as usize));
    }
    let one = (1i64 << PRECISION_BITS) as f64;
    let ints = kk.iter().map(|&k| if k < 0.0 { (-0.5 + k * one) as i32 } else { (0.5 + k * one) as i32 }).collect();
    (ksize, bounds, ints)
}

/// A band-interleaved 8-bit image, the shape `ImagingResample` works on.
struct Planar {
    w: usize,
    h: usize,
    bands: usize,
    px: Vec<u8>,
}

/// The horizontal pass over rows `first..first + rows` of `src`.
fn pass_h(
    src: &Planar,
    xsize: usize,
    first: usize,
    rows: usize,
    coeffs: &(usize, Vec<(usize, usize)>, Vec<i32>),
) -> Planar {
    let (ksize, bounds, kk) = coeffs;
    let bands = src.bands;
    let mut out = vec![0u8; xsize * rows * bands];
    for yy in 0..rows {
        let row = &src.px[(yy + first) * src.w * bands..(yy + first + 1) * src.w * bands];
        for xx in 0..xsize {
            let (xmin, xmax) = bounds[xx];
            let k = &kk[xx * ksize..];
            for c in 0..bands {
                let mut ss = 1i32 << (PRECISION_BITS - 1);
                for x in 0..xmax {
                    ss = ss.wrapping_add((row[(x + xmin) * bands + c] as i32).wrapping_mul(k[x]));
                }
                out[(yy * xsize + xx) * bands + c] = clip8(ss);
            }
        }
    }
    Planar { w: xsize, h: rows, bands, px: out }
}

fn pass_v(src: &Planar, ysize: usize, ksize: usize, bounds: &[(usize, usize)], kk: &[i32]) -> Planar {
    let (w, bands) = (src.w, src.bands);
    let mut out = vec![0u8; w * ysize * bands];
    for yy in 0..ysize {
        let (ymin, ymax) = bounds[yy];
        let k = &kk[yy * ksize..];
        for xx in 0..w {
            for c in 0..bands {
                let mut ss = 1i32 << (PRECISION_BITS - 1);
                for (y, &ky) in k.iter().take(ymax).enumerate() {
                    ss = ss.wrapping_add((src.px[((y + ymin) * w + xx) * bands + c] as i32).wrapping_mul(ky));
                }
                out[(yy * w + xx) * bands + c] = clip8(ss);
            }
        }
    }
    Planar { w, h: ysize, bands, px: out }
}

/// `ImagingResampleInner`: horizontal first unless the height shrinks by more
/// than twice what the width does.
fn resample(src: Planar, xsize: usize, ysize: usize) -> Planar {
    let need_h = xsize != src.w;
    let need_v = ysize != src.h;
    let (dh, dw) = (src.h as i64 - ysize as i64, src.w as i64 - xsize as i64);
    let horizontal_first = !(dh > 0 && dh > dw * 2);
    let ch = coeffs(src.w, xsize);
    let (ksize_v, mut bounds_v, kk_v) = coeffs(src.h, ysize);
    if horizontal_first {
        let mut cur = src;
        if need_h {
            let first = bounds_v[0].0;
            let last = bounds_v[ysize - 1].0 + bounds_v[ysize - 1].1;
            for b in bounds_v.iter_mut() {
                b.0 -= first;
            }
            cur = pass_h(&cur, xsize, first, last - first, &ch);
        }
        if need_v {
            cur = pass_v(&cur, ysize, ksize_v, &bounds_v, &kk_v);
        }
        return cur;
    }
    let mut cur = src;
    if need_v {
        cur = pass_v(&cur, ysize, ksize_v, &bounds_v, &kk_v);
    }
    if need_h {
        let rows = cur.h;
        cur = pass_h(&cur, xsize, 0, rows, &ch);
    }
    cur
}

/// `im.resize((w, h))` on an "L" image (BICUBIC).
pub fn resize_l(src: &Mask, w: usize, h: usize) -> Mask {
    if (src.width, src.height) == (w, h) || w == 0 || h == 0 || src.width == 0 || src.height == 0 {
        return if (src.width, src.height) == (w, h) { src.clone() } else { Mask::new(w, h) };
    }
    let p = resample(Planar { w: src.width, h: src.height, bands: 1, px: src.data.clone() }, w, h);
    Mask { width: p.w, height: p.h, data: p.px }
}

fn muldiv255(a: u32, b: u32) -> u8 {
    let tmp = a * b + 128;
    (((tmp >> 8) + tmp) >> 8) as u8
}

/// `im.resize((w, h))` on an "RGBA" image: BICUBIC on premultiplied "RGBa".
pub fn resize_rgba(src: &Rgba, w: usize, h: usize) -> Rgba {
    if (src.width, src.height) == (w, h) {
        return src.clone();
    }
    if w == 0 || h == 0 || src.width == 0 || src.height == 0 {
        return Rgba::new(w, h, [0; 4]);
    }
    let mut px = Vec::with_capacity(src.data.len() * 4);
    for p in &src.data {
        let a = p[3] as u32;
        px.extend_from_slice(&[muldiv255(p[0] as u32, a), muldiv255(p[1] as u32, a), muldiv255(p[2] as u32, a), p[3]]);
    }
    let r = resample(Planar { w: src.width, h: src.height, bands: 4, px }, w, h);
    let data =
        r.px.chunks_exact(4)
            .map(|p| {
                let a = p[3] as u32;
                if a == 255 || a == 0 {
                    [p[0], p[1], p[2], p[3]]
                } else {
                    let un = |v: u8| ((255 * v as u32) / a).min(255) as u8;
                    [un(p[0]), un(p[1]), un(p[2]), p[3]]
                }
            })
            .collect();
    Rgba { width: w, height: h, data }
}

fn rotate_quarter(src: &Mask, turns: u32) -> Mask {
    let (w, h) = (src.width, src.height);
    let (ow, oh) = if turns % 2 == 1 { (h, w) } else { (w, h) };
    let mut out = Mask::new(ow, oh);
    for y in 0..oh {
        for x in 0..ow {
            let (sx, sy) = match turns {
                1 => (w - 1 - y, x),
                2 => (w - 1 - x, h - 1 - y),
                _ => (y, h - 1 - x),
            };
            out.data[y * ow + x] = src.data[sy * w + sx];
        }
    }
    out
}

fn floor_i(x: f64) -> i64 {
    if x < 0.0 { x.floor() as i64 } else { x as i64 }
}

/// `bilinear_filter8`.
fn bilinear8(im: &Mask, xin: f64, yin: f64) -> Option<u8> {
    let (w, h) = (im.width as i64, im.height as i64);
    if xin < 0.0 || xin >= w as f64 || yin < 0.0 || yin >= h as f64 {
        return None;
    }
    let (xin, yin) = (xin - 0.5, yin - 0.5);
    let (x, y) = (floor_i(xin), floor_i(yin));
    let (dx, dy) = (xin - x as f64, yin - y as f64);
    let clip = |v: i64, n: i64| v.clamp(0, n - 1);
    let at = |yy: i64, xx: i64| im.data[(yy * w + xx) as usize] as f64;
    let (x0, x1) = (clip(x, w), clip(x + 1, w));
    let row = clip(y, h);
    let lerp = |a: f64, b: f64, d: f64| a + (b - a) * d;
    let v1 = lerp(at(row, x0), at(row, x1), dx);
    let v2 = if y + 1 >= 0 && y + 1 < h { lerp(at(y + 1, x0), at(y + 1, x1), dx) } else { v1 };
    Some(lerp(v1, v2, dy) as u8)
}

/// `im.rotate(angle, resample=BILINEAR)` on an "L" image (no expand).
pub fn rotate_l(src: &Mask, angle: f64) -> Mask {
    let angle = crate::draw::py_mod(angle, 360.0);
    if angle == 0.0 {
        return src.clone();
    }
    if angle == 180.0 {
        return rotate_quarter(src, 2);
    }
    if (angle == 90.0 || angle == 270.0) && src.width == src.height {
        return rotate_quarter(src, if angle == 90.0 { 1 } else { 3 });
    }
    let (w, h) = (src.width, src.height);
    let center = (w as f64 / 2.0, h as f64 / 2.0);
    let a = -(angle * (std::f64::consts::PI / 180.0));
    let r15 = |v: f64| -> f64 {
        if !v.is_finite() {
            return v;
        }
        format!("{v:.15}").parse().unwrap_or(v)
    };
    let mut m = [r15(a.cos()), r15(a.sin()), 0.0, r15(-a.sin()), r15(a.cos()), 0.0];
    let (cx, cy) = (-center.0, -center.1);
    let t0 = m[0] * cx + m[1] * cy + m[2];
    let t1 = m[3] * cx + m[4] * cy + m[5];
    m[2] = t0 + center.0;
    m[5] = t1 + center.1;
    let mut out = Mask::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let (xi, yi) = (x as f64 + 0.5, y as f64 + 0.5);
            let xo = m[0] * xi + m[1] * yi + m[2];
            let yo = m[3] * xi + m[4] * yi + m[5];
            out.data[y * w + x] = bilinear8(src, xo, yo).unwrap_or(0);
        }
    }
    out
}

/// `im.crop((x0, y0, x1, y1))`: zero outside the source.
pub fn crop_l(src: &Mask, x0: i64, y0: i64, x1: i64, y1: i64) -> Mask {
    let (w, h) = ((x1 - x0).max(0) as usize, (y1 - y0).max(0) as usize);
    let mut out = Mask::new(w, h);
    for y in 0..h {
        let sy = y0 + y as i64;
        if sy < 0 || sy >= src.height as i64 {
            continue;
        }
        for x in 0..w {
            let sx = x0 + x as i64;
            if sx >= 0 && sx < src.width as i64 {
                out.data[y * w + x] = src.data[sy as usize * src.width + sx as usize];
            }
        }
    }
    out
}

/// The overlap of a `w`x`h` source placed at `(dx, dy)`: `(dx, dy, sx, sy, xsize, ysize)`
/// as `ImagingPaste` clips it.
fn clip_box(dst: &Rgba, dx: i64, dy: i64, w: usize, h: usize) -> Option<(usize, usize, usize, usize, usize, usize)> {
    let (mut dx, mut dy, mut xs, mut ys, mut sx, mut sy) = (dx, dy, w as i64, h as i64, 0i64, 0i64);
    if dx < 0 {
        xs += dx;
        sx = -dx;
        dx = 0;
    }
    if dx + xs > dst.width as i64 {
        xs = dst.width as i64 - dx;
    }
    if dy < 0 {
        ys += dy;
        sy = -dy;
        dy = 0;
    }
    if dy + ys > dst.height as i64 {
        ys = dst.height as i64 - dy;
    }
    if xs <= 0 || ys <= 0 {
        return None;
    }
    Some((dx as usize, dy as usize, sx as usize, sy as usize, xs as usize, ys as usize))
}

/// `dst.paste(src, (dx, dy))`.
pub fn paste(dst: &mut Rgba, src: &Rgba, dx: i64, dy: i64) {
    let Some((dx, dy, sx, sy, xs, ys)) = clip_box(dst, dx, dy, src.width, src.height) else { return };
    for y in 0..ys {
        let d = (y + dy) * dst.width + dx;
        let s = (y + sy) * src.width + sx;
        dst.data[d..d + xs].copy_from_slice(&src.data[s..s + xs]);
    }
}

/// `dst.paste(src, (dx, dy), mask)` with an "L" mask (`paste_mask_L`).
pub fn paste_masked(dst: &mut Rgba, src: &Rgba, mask: &Mask, dx: i64, dy: i64) {
    let Some((dx, dy, sx, sy, xs, ys)) = clip_box(dst, dx, dy, src.width, src.height) else { return };
    for y in 0..ys {
        for x in 0..xs {
            let m = mask.data[(y + sy) * mask.width + x + sx] as u32;
            let s = src.data[(y + sy) * src.width + x + sx];
            let o = &mut dst.data[(y + dy) * dst.width + x + dx];
            for c in 0..4 {
                o[c] = blend(m, o[c] as u32, s[c] as u32);
            }
        }
    }
}

/// `Image.composite(a, b, mask)` = `b.copy().paste(a, None, mask)`.
pub fn composite(a: &Rgba, b: &Rgba, mask: &Mask) -> Rgba {
    let mut out = b.clone();
    paste_masked(&mut out, a, mask, 0, 0);
    out
}

fn shift_div255(a: u32) -> u32 {
    ((a >> 8) + a) >> 8
}

/// One `ImagingAlphaComposite` pixel.
pub fn over(d: Ink, s: Ink) -> Ink {
    const P: u32 = 7;
    let sa = s[3] as u32;
    if sa == 0 {
        return d;
    }
    let blend = d[3] as u32 * (255 - sa);
    let outa255 = sa * 255 + blend;
    let coef1 = sa * 255 * 255 * (1 << P) / outa255;
    let coef2 = 255 * (1 << P) - coef1;
    let ch = |c: usize| {
        let tmp = s[c] as u32 * coef1 + d[c] as u32 * coef2;
        (shift_div255(tmp + (0x80 << P)) >> P) as u8
    };
    [ch(0), ch(1), ch(2), shift_div255(outa255 + 0x80) as u8]
}

/// `dst.alpha_composite(src, (dx, dy))`.
pub fn alpha_composite(dst: &mut Rgba, src: &Rgba, dx: i64, dy: i64) {
    let Some((dx, dy, sx, sy, xs, ys)) = clip_box(dst, dx, dy, src.width, src.height) else { return };
    for y in 0..ys {
        for x in 0..xs {
            let s = src.data[(y + sy) * src.width + x + sx];
            let o = &mut dst.data[(y + dy) * dst.width + x + dx];
            *o = over(*o, s);
        }
    }
}

/// `im.point(lut)` on an "L" image, the table clipped to 0..255.
pub fn point(src: &Mask, lut: impl Fn(u8) -> i64) -> Mask {
    let table: Vec<u8> = (0..=255u8).map(|v| lut(v).clamp(0, 255) as u8).collect();
    Mask { width: src.width, height: src.height, data: src.data.iter().map(|&v| table[v as usize]).collect() }
}

/// `im.getchannel("A")`.
pub fn alpha(src: &Rgba) -> Mask {
    Mask { width: src.width, height: src.height, data: src.data.iter().map(|p| p[3]).collect() }
}

/// `im.putalpha(mask)`.
pub fn put_alpha(dst: &mut Rgba, a: &Mask) {
    for (p, &v) in dst.data.iter_mut().zip(&a.data) {
        p[3] = v;
    }
}

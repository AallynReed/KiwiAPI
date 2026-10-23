//! `libImaging/Resample.c` (LANCZOS) and `libImaging/Reduce.c` for 4-band
//! 8-bit images: the same coefficients (doubles normalised to 22-bit fixed
//! point), passes, boxes (C floats) and rounding.

const PRECISION_BITS: u32 = 32 - 8 - 2;
const SUPPORT: f64 = 3.0;

fn sinc(x: f64) -> f64 {
    if x == 0.0 {
        return 1.0;
    }
    let x = x * std::f64::consts::PI;
    x.sin() / x
}

fn lanczos_filter(x: f64) -> f64 {
    if (-3.0..3.0).contains(&x) { sinc(x) * sinc(x / 3.0) } else { 0.0 }
}

struct Coeffs {
    ksize: usize,
    bounds: Vec<(usize, usize)>,
    kk: Vec<i32>,
}

/// `precompute_coeffs` + `normalize_coeffs_8bpc`.
fn coeffs(in_size: usize, in0: f32, in1: f32, out_size: usize) -> Coeffs {
    let scale = f64::from(in1 - in0) / out_size as f64;
    let filterscale = scale.max(1.0);
    let support = SUPPORT * filterscale;
    let ksize = support.ceil() as usize * 2 + 1;
    let mut bounds = Vec::with_capacity(out_size);
    let mut kk = vec![0i32; out_size * ksize];
    let mut k = vec![0f64; ksize];
    for xx in 0..out_size {
        let center = f64::from(in0) + (xx as f64 + 0.5) * scale;
        let ss = 1.0 / filterscale;
        let xmin = ((center - support + 0.5) as i64).max(0) as usize;
        let xmax = (((center + support + 0.5) as i64).min(in_size as i64) as usize).saturating_sub(xmin);
        let mut ww = 0.0;
        for (x, slot) in k.iter_mut().enumerate().take(xmax) {
            let w = lanczos_filter((x as f64 + xmin as f64 - center + 0.5) * ss);
            *slot = w;
            ww += w;
        }
        for (x, w) in k.iter().enumerate().take(xmax) {
            let w = if ww != 0.0 { w / ww } else { *w };
            let f = w * f64::from(1u32 << PRECISION_BITS);
            kk[xx * ksize + x] = if w < 0.0 { (-0.5 + f) as i32 } else { (0.5 + f) as i32 };
        }
        bounds.push((xmin, xmax));
    }
    Coeffs { ksize, bounds, kk }
}

#[inline]
fn clip8(v: i32) -> u8 {
    (v >> PRECISION_BITS).clamp(0, 255) as u8
}

const HALF: i32 = 1 << (PRECISION_BITS - 1);

/// `ImagingResample(im, out_w, out_h, LANCZOS, box)` of 4-band pixels,
/// `bx` = (x0, y0, x1, y1) as the C floats Pillow parses.
pub fn lanczos(src: &[u8], width: usize, height: usize, out_w: usize, out_h: usize, bx: [f32; 4]) -> Vec<u8> {
    let need_h = out_w != width || bx[0] != 0.0 || bx[2] != out_w as f32;
    let need_v = out_h != height || bx[1] != 0.0 || bx[3] != out_h as f32;
    let horiz = coeffs(width, bx[0], bx[2], out_w);
    let vert = coeffs(height, bx[1], bx[3], out_h);
    let first = vert.bounds[0].0;
    let last = vert.bounds[out_h - 1].0 + vert.bounds[out_h - 1].1;

    let (mut cur, mut cur_w, mut row0) = (std::borrow::Cow::Borrowed(src), width, 0usize);
    if need_h {
        let rows = last - first;
        let mut tmp = vec![0u8; out_w * rows * 4];
        for yy in 0..rows {
            let line = &src[(yy + first) * width * 4..(yy + first + 1) * width * 4];
            for xx in 0..out_w {
                let (xmin, xmax) = horiz.bounds[xx];
                let k = &horiz.kk[xx * horiz.ksize..];
                let mut ss = [HALF; 4];
                for x in 0..xmax {
                    let px = &line[(x + xmin) * 4..(x + xmin) * 4 + 4];
                    for c in 0..4 {
                        ss[c] = ss[c].wrapping_add(i32::from(px[c]).wrapping_mul(k[x]));
                    }
                }
                let o = (yy * out_w + xx) * 4;
                for c in 0..4 {
                    tmp[o + c] = clip8(ss[c]);
                }
            }
        }
        cur = std::borrow::Cow::Owned(tmp);
        cur_w = out_w;
        row0 = first;
    }
    if !need_v {
        return cur.into_owned();
    }
    let mut out = vec![0u8; cur_w * out_h * 4];
    for yy in 0..out_h {
        let (ymin, ymax) = vert.bounds[yy];
        let ymin = ymin - row0;
        let k = &vert.kk[yy * vert.ksize..];
        for xx in 0..cur_w {
            let mut ss = [HALF; 4];
            for (y, &ky) in k.iter().enumerate().take(ymax) {
                let at = ((y + ymin) * cur_w + xx) * 4;
                for c in 0..4 {
                    ss[c] = ss[c].wrapping_add(i32::from(cur[at + c]).wrapping_mul(ky));
                }
            }
            let o = (yy * cur_w + xx) * 4;
            for c in 0..4 {
                out[o + c] = clip8(ss[c]);
            }
        }
    }
    out
}

/// `division_UINT32(divider, 8)`: a float division truncated, as in C.
fn multiplier(n: usize) -> u64 {
    (4_294_967_296.0f32 / (256 * n) as f32) as u32 as u64
}

/// `ImagingReduce(im, fx, fy, (0, 0, w, h))` of 4-band pixels: each output
/// pixel the rounded mean of its (possibly clipped) block.
pub fn reduce(src: &[u8], width: usize, height: usize, fx: usize, fy: usize) -> (Vec<u8>, usize, usize) {
    let (ow, oh) = (width.div_ceil(fx), height.div_ceil(fy));
    let mut out = vec![0u8; ow * oh * 4];
    for y in 0..oh {
        let (y0, y1) = (y * fy, ((y + 1) * fy).min(height));
        for x in 0..ow {
            let (x0, x1) = (x * fx, ((x + 1) * fx).min(width));
            let n = (y1 - y0) * (x1 - x0);
            let (amend, mul) = ((n / 2) as u64, multiplier(n));
            let mut ss = [amend; 4];
            for yy in y0..y1 {
                for xx in x0..x1 {
                    let at = (yy * width + xx) * 4;
                    for c in 0..4 {
                        ss[c] += u64::from(src[at + c]);
                    }
                }
            }
            let o = (y * ow + x) * 4;
            for c in 0..4 {
                out[o + c] = (((ss[c] * mul) & 0xFFFF_FFFF) >> 24) as u8;
            }
        }
    }
    (out, ow, oh)
}

//! Pillow 13's `Image.resize(size, LANCZOS)` for RGB (`libImaging/Resample.c`).

// Index loops mirror the C/Python they reproduce.
#![allow(clippy::needless_range_loop)]

use super::cv::Img;

const PRECISION_BITS: i32 = 32 - 8 - 2;

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

/// `precompute_coeffs` + `normalize_coeffs_8bpc`: (bounds, fixed-point taps, ksize).
fn coeffs(in_size: usize, out_size: usize) -> (Vec<(usize, usize)>, Vec<i32>, usize) {
    let scale = in_size as f32 as f64 / out_size as f64;
    let filterscale = scale.max(1.0);
    let support = 3.0 * filterscale;
    let ksize = support.ceil() as usize * 2 + 1;
    let mut bounds = Vec::with_capacity(out_size);
    let mut kk = vec![0f64; out_size * ksize];
    for xx in 0..out_size {
        let center = (xx as f64 + 0.5) * scale;
        let ss = 1.0 / filterscale;
        let xmin = ((center - support + 0.5) as i64).max(0) as usize;
        let xmax = ((center + support + 0.5) as i64).min(in_size as i64) as usize - xmin;
        let k = &mut kk[xx * ksize..(xx + 1) * ksize];
        let mut ww = 0.0;
        for (x, slot) in k.iter_mut().enumerate().take(xmax) {
            let w = lanczos((x as f64 + xmin as f64 - center + 0.5) * ss);
            *slot = w;
            ww += w;
        }
        for slot in k.iter_mut().take(xmax) {
            if ww != 0.0 {
                *slot /= ww;
            }
        }
        bounds.push((xmin, xmax));
    }
    let fixed = kk
        .iter()
        .map(|&v| {
            let s = v * (1u32 << PRECISION_BITS) as f64;
            if v < 0.0 { (-0.5 + s) as i32 } else { (0.5 + s) as i32 }
        })
        .collect();
    (bounds, fixed, ksize)
}

fn clip8(v: i32) -> u8 {
    (v >> PRECISION_BITS).clamp(0, 255) as u8
}

/// `Image.fromarray(img).resize((w, h), Image.LANCZOS)` for an RGB image.
pub fn resize_lanczos(src: &Img, w: usize, h: usize) -> Img {
    if src.w == w && src.h == h {
        return src.clone();
    }
    let (bv, kv, ksv) = coeffs(src.h, h);
    let (bh, kh, ksh) = coeffs(src.w, w);
    let c = src.c;
    let need_h = w != src.w;
    let need_v = h != src.h;
    let mut cur = src.clone();
    let mut bv = bv;
    if need_h {
        let first = bv[0].0;
        let last = bv[h - 1].0 + bv[h - 1].1;
        for b in bv.iter_mut() {
            b.0 -= first;
        }
        let rows = last - first;
        let mut tmp = Img::new(w, rows, c);
        for yy in 0..rows {
            let row = &cur.data[(yy + first) * cur.w * c..];
            for xx in 0..w {
                let (xmin, xmax) = bh[xx];
                let k = &kh[xx * ksh..];
                for ch in 0..c {
                    let mut ss: i32 = 1 << (PRECISION_BITS - 1);
                    for x in 0..xmax {
                        ss = ss.wrapping_add(row[(x + xmin) * c + ch] as i32 * k[x]);
                    }
                    tmp.data[(yy * w + xx) * c + ch] = clip8(ss);
                }
            }
        }
        cur = tmp;
    }
    if need_v {
        let mut out = Img::new(cur.w, h, c);
        for yy in 0..h {
            let (ymin, ymax) = bv[yy];
            let k = &kv[yy * ksv..];
            for xx in 0..cur.w {
                for ch in 0..c {
                    let mut ss: i32 = 1 << (PRECISION_BITS - 1);
                    for y in 0..ymax {
                        ss = ss.wrapping_add(cur.data[((y + ymin) * cur.w + xx) * c + ch] as i32 * k[y]);
                    }
                    out.data[(yy * cur.w + xx) * c + ch] = clip8(ss);
                }
            }
        }
        cur = out;
    }
    cur
}

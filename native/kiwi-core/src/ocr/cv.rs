//! The OpenCV 5.0.0 calls RapidOCR makes, reproduced bit for bit for the
//! paths an x86-64 AVX2 build takes: `resize` (INTER_LINEAR, 8-bit),
//! `dilate`, `findContours` (RETR_LIST, CHAIN_APPROX_SIMPLE), `convexHull`,
//! `minAreaRect`, `boxPoints`, `fillPoly`, masked `mean`,
//! `getPerspectiveTransform`, `warpPerspective` (INTER_CUBIC, replicate).

// Index loops mirror the C/Python they reproduce.
#![allow(clippy::needless_range_loop)]

/// An interleaved 8-bit image.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Img {
    pub w: usize,
    pub h: usize,
    pub c: usize,
    pub data: Vec<u8>,
}

impl Img {
    pub fn new(w: usize, h: usize, c: usize) -> Self {
        Img { w, h, c, data: vec![0; w * h * c] }
    }

    pub fn px(&self, x: usize, y: usize) -> &[u8] {
        let o = (y * self.w + x) * self.c;
        &self.data[o..o + self.c]
    }

    /// `arr[y0:y1, x0:x1]`.
    pub fn crop(&self, x0: usize, y0: usize, x1: usize, y1: usize) -> Img {
        let (w, h) = (x1.saturating_sub(x0), y1.saturating_sub(y0));
        let mut out = Img::new(w, h, self.c);
        for y in 0..h {
            let s = ((y0 + y) * self.w + x0) * self.c;
            out.data[y * w * self.c..(y + 1) * w * self.c].copy_from_slice(&self.data[s..s + w * self.c]);
        }
        out
    }
}

/// `cvRound` of a float/double: round half to even.
fn round_i32(x: f64) -> i32 {
    x.round_ties_even() as i32
}

/// `cv::resize(src, (dw, dh))` with INTER_LINEAR on an 8-bit image.
pub fn resize_linear(src: &Img, dw: usize, dh: usize) -> Img {
    if dw == src.w && dh == src.h {
        return src.clone();
    }
    let c = src.c;
    let inv_x = dw as f64 / src.w as f64;
    let inv_y = dh as f64 / src.h as f64;
    let (sx_scale, sy_scale) = (1.0 / inv_x, 1.0 / inv_y);
    let (ix, iy) = (round_i32(sx_scale), round_i32(sy_scale));
    let area_fast = (sx_scale - ix as f64).abs() < f64::EPSILON && (sy_scale - iy as f64).abs() < f64::EPSILON;
    if area_fast && ix == 2 && iy == 2 {
        return area_fast_2x(src, dw, dh);
    }
    let (sw, sh) = (src.w as i32, src.h as i32);
    let mut xofs = vec![0usize; dw];
    let mut alpha = vec![[0i32; 2]; dw];
    let mut xmax = dw;
    for (dx, (xo, al)) in xofs.iter_mut().zip(alpha.iter_mut()).enumerate() {
        let mut fx = ((dx as f64 + 0.5) * sx_scale - 0.5) as f32;
        let mut sx = fx.floor() as i32;
        fx -= sx as f32;
        if sx < 0 {
            fx = 0.0;
            sx = 0;
        }
        if sx + 1 >= sw {
            xmax = xmax.min(dx);
            if sx >= sw - 1 {
                fx = 0.0;
                sx = sw - 1;
            }
        }
        *xo = sx as usize;
        *al = [round_i32(((1.0f32 - fx) * 2048.0) as f64), round_i32((fx * 2048.0) as f64)];
    }
    let hrow = |y: usize| -> Vec<i32> {
        let row = &src.data[y * src.w * c..(y + 1) * src.w * c];
        let mut out = vec![0i32; dw * c];
        for dx in 0..dw {
            let sx = xofs[dx] * c;
            for k in 0..c {
                out[dx * c + k] = if dx < xmax {
                    row[sx + k] as i32 * alpha[dx][0] + row[sx + c + k] as i32 * alpha[dx][1]
                } else {
                    row[sx + k] as i32 * 2048
                };
            }
        }
        out
    };
    let mut dst = Img::new(dw, dh, c);
    let mut cache: Vec<(i32, Vec<i32>)> = Vec::new();
    for dy in 0..dh {
        let mut fy = ((dy as f64 + 0.5) * sy_scale - 0.5) as f32;
        let sy = fy.floor() as i32;
        fy -= sy as f32;
        let b0 = round_i32(((1.0f32 - fy) * 2048.0) as f64);
        let b1 = round_i32((fy * 2048.0) as f64);
        let rows: Vec<i32> = (0..2).map(|k| (sy + k).clamp(0, sh - 1)).collect();
        let mut get = |r: i32| -> Vec<i32> {
            if let Some((_, v)) = cache.iter().find(|(y, _)| *y == r) {
                return v.clone();
            }
            let v = hrow(r as usize);
            cache.push((r, v.clone()));
            if cache.len() > 4 {
                cache.remove(0);
            }
            v
        };
        let (s0, s1) = (get(rows[0]), get(rows[1]));
        let out = &mut dst.data[dy * dw * c..(dy + 1) * dw * c];
        for x in 0..dw * c {
            out[x] = ((((b0 * (s0[x] >> 4)) >> 16) + ((b1 * (s1[x] >> 4)) >> 16) + 2) >> 2) as u8;
        }
    }
    dst
}

fn area_fast_2x(src: &Img, dw: usize, dh: usize) -> Img {
    let c = src.c;
    let mut dst = Img::new(dw, dh, c);
    let sstep = src.w * c;
    for dy in 0..dh {
        let s0 = &src.data[2 * dy * sstep..];
        let s1 = &src.data[(2 * dy + 1) * sstep..];
        for dx in 0..dw {
            for k in 0..c {
                let i = 2 * dx * c + k;
                let sum = s0[i] as u32 + s0[i + c] as u32 + s1[i] as u32 + s1[i + c] as u32;
                dst.data[(dy * dw + dx) * c + k] = ((sum + 2) >> 2) as u8;
            }
        }
    }
    dst
}

/// `cv2.copyMakeBorder(img, top, bottom, 0, 0, BORDER_CONSTANT, 0)`.
pub fn pad_vertical(img: &Img, top: usize, bottom: usize) -> Img {
    let mut out = Img::new(img.w, img.h + top + bottom, img.c);
    let row = img.w * img.c;
    out.data[top * row..(top + img.h) * row].copy_from_slice(&img.data);
    out
}

/// `cv2.rotate(img, ROTATE_180)`.
pub fn rotate180(img: &Img) -> Img {
    let mut out = img.clone();
    let c = img.c;
    let n = img.w * img.h;
    for i in 0..n {
        out.data[i * c..(i + 1) * c].copy_from_slice(&img.data[(n - 1 - i) * c..(n - i) * c]);
    }
    out
}

/// `np.rot90(img)`: 90 degrees counter-clockwise.
pub fn rot90(img: &Img) -> Img {
    let (w, h, c) = (img.w, img.h, img.c);
    let mut out = Img::new(h, w, c);
    for y in 0..w {
        for x in 0..h {
            let s = (x * w + (w - 1 - y)) * c;
            let d = (y * h + x) * c;
            out.data[d..d + c].copy_from_slice(&img.data[s..s + c]);
        }
    }
    out
}

/// `cv2.dilate(mask, np.ones((2, 2)))` on a 0/1 map (anchor at the centre).
pub fn dilate2x2(m: &[u8], w: usize, h: usize) -> Vec<u8> {
    let mut out = vec![0u8; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut v = m[y * w + x];
            if x > 0 {
                v = v.max(m[y * w + x - 1]);
            }
            if y > 0 {
                v = v.max(m[(y - 1) * w + x]);
                if x > 0 {
                    v = v.max(m[(y - 1) * w + x - 1]);
                }
            }
            out[y * w + x] = v;
        }
    }
    out
}

// --- findContours ------------------------------------------------------------

const DELTAS: [(i64, i64); 8] = [(1, 0), (1, -1), (0, -1), (-1, -1), (-1, 0), (-1, 1), (0, 1), (1, 1)];

/// `cv2.findContours(mask, RETR_LIST, CHAIN_APPROX_SIMPLE)[0]` (any non-zero
/// pixel is foreground), in OpenCV's output order.
pub fn find_contours(mask: &[u8], w: usize, h: usize) -> Vec<Vec<(i32, i32)>> {
    // The scanner works on a 1-pixel zero border; values are schar marks.
    let (pw, ph) = (w + 2, h + 2);
    let mut img = vec![0i8; pw * ph];
    for y in 0..h {
        for x in 0..w {
            img[(y + 1) * pw + x + 1] = (mask[y * w + x] != 0) as i8;
        }
    }
    let step = pw as i64;
    let delta = |s: i8| -> i64 {
        let (dx, dy) = DELTAS[(s as usize) % 8];
        dx + dy * step
    };
    let mut found: Vec<Vec<(i32, i32)>> = Vec::new();
    let (width, height) = (pw - 1, ph - 1);
    let (mut x, mut y) = (1usize, 1usize);
    let mut prev = img[y * pw + x - 1] as i32;
    while y < height {
        let mut p: i32 = 0;
        while x < width {
            while x < width {
                p = img[y * pw + x] as i32;
                if p != prev {
                    break;
                }
                x += 1;
            }
            if x >= width {
                break;
            }
            let mut is_hole = false;
            let start = if prev == 0 && p == 1 {
                Some(())
            } else if p == 0 && prev >= 1 {
                is_hole = true;
                Some(())
            } else {
                None
            };
            if start.is_some() {
                let sx = x - is_hole as usize;
                found.push(fetch_contour(&mut img, step, sx, y, is_hole, delta));
                x += 1;
                prev = img[y * pw + x - 1] as i32;
                continue;
            }
            prev = p;
            x += 1;
        }
        y += 1;
        x = 1;
        prev = 0;
    }
    found.reverse();
    found
}

const NEW: i8 = 2;
const RIGHT: i8 = -126; // 0x80 | NEW

fn fetch_contour(
    img: &mut [i8],
    step: i64,
    sx: usize,
    sy: usize,
    is_hole: bool,
    delta: impl Fn(i8) -> i64,
) -> Vec<(i32, i32)> {
    let i0 = sy as i64 * step + sx as i64;
    let mut pt = (sx as i32 - 1, sy as i32 - 1);
    let mut pts = Vec::new();
    let s_end0: i8 = if is_hole { 0 } else { 4 };
    let mut s = s_end0;
    let mut i1;
    loop {
        s = (s - 1) & 7;
        i1 = i0 + delta(s);
        if img[i1 as usize] != 0 || s == s_end0 {
            break;
        }
    }
    if s == s_end0 {
        img[i0 as usize] = RIGHT;
        pts.push(pt);
        return pts;
    }
    let mut i3 = i0;
    let mut prev_s = s ^ 4;
    loop {
        let s_end = s;
        s = s.min(15);
        let mut i4;
        loop {
            s += 1;
            i4 = i3 + delta(s);
            if img[i4 as usize] != 0 || s >= 15 {
                break;
            }
        }
        s &= 7;
        if ((s - 1) as u8 as u32) < (s_end as u8 as u32) {
            img[i3 as usize] = RIGHT;
        } else if img[i3 as usize] == 1 {
            img[i3 as usize] = NEW;
        }
        if s != prev_s {
            pts.push(pt);
        }
        prev_s = s;
        pt.0 += DELTAS[s as usize].0 as i32;
        pt.1 += DELTAS[s as usize].1 as i32;
        if i4 == i0 && i3 == i1 {
            break;
        }
        i3 = i4;
        s = (s + 4) & 7;
    }
    pts
}

// --- convexHull / minAreaRect / boxPoints ------------------------------------

/// `cv2.convexHull(points, clockwise=False, returnPoints=True)` for integer points.
pub fn convex_hull_i(pts: &[(i32, i32)]) -> Vec<(i32, i32)> {
    let total = pts.len();
    if total == 0 {
        return Vec::new();
    }
    let mut ptr: Vec<usize> = (0..total).collect();
    ptr.sort_by(|&a, &b| pts[a].0.cmp(&pts[b].0).then(pts[a].1.cmp(&pts[b].1)).then(a.cmp(&b)));
    let (mut miny, mut maxy) = (0usize, 0usize);
    for i in 1..total {
        let y = pts[ptr[i]].1;
        if pts[ptr[miny]].1 > y {
            miny = i;
        }
        if pts[ptr[maxy]].1 < y {
            maxy = i;
        }
    }
    let p = |i: usize| pts[ptr[i]];
    let mut hull: Vec<usize> = Vec::new();
    if p(0) == p(total - 1) {
        hull.push(0);
    } else {
        let mut stack = vec![0i64; total + 2];
        let tl = sklansky(&p, 0, maxy as i64, &mut stack, 0, -1, 1);
        let tr = sklansky(&p, total as i64 - 1, maxy as i64, &mut stack, tl, -1, -1);
        // clockwise = false swaps the chains: the right one (at `tl`) first.
        for i in 0..tr - 1 {
            hull.push(stack[tl + i] as usize);
        }
        for i in (1..tl).rev() {
            hull.push(stack[i] as usize);
        }
        let stop = if tl > 2 {
            stack[1]
        } else if tr > 2 {
            stack[tl + tr - 2]
        } else {
            -1
        };
        let mut stack2 = vec![0i64; total + 2];
        let bl = sklansky(&p, 0, miny as i64, &mut stack2, 0, 1, -1);
        let br = sklansky(&p, total as i64 - 1, miny as i64, &mut stack2, bl, 1, 1);
        let (mut bl_n, mut br_n) = (bl, br);
        if stop >= 0 {
            let check = if bl_n > 2 {
                stack2[1]
            } else if bl_n + br_n > 2 {
                stack2[bl + 2 - bl_n]
            } else {
                -1
            };
            if check == stop || (check >= 0 && p(check as usize) == p(stop as usize)) {
                bl_n = bl_n.min(2);
                br_n = br_n.min(2);
            }
        }
        for i in 0..bl_n - 1 {
            hull.push(stack2[i] as usize);
        }
        for i in (1..br_n).rev() {
            hull.push(stack2[bl + i] as usize);
        }
    }
    // Indices into the original array, then OpenCV's cyclic shift.
    let mut idx: Vec<usize> = hull.iter().map(|&h| ptr[h]).collect();
    let nout = idx.len();
    if nout >= 3 {
        let (mut min_i, mut max_i, mut lt) = (0usize, 0usize, 0usize);
        for i in 1..nout {
            let v = idx[i];
            lt += (idx[i - 1] < v) as usize;
            if lt > 1 && lt + 2 <= i {
                break;
            }
            if v < idx[min_i] {
                min_i = i;
            }
            if v > idx[max_i] {
                max_i = i;
            }
        }
        let mmdist = min_i.abs_diff(max_i);
        if (mmdist == 1 || mmdist == nout - 1) && (lt <= 1 || lt + 2 >= nout) {
            let ascending = (max_i + 1) % nout == min_i;
            let i0 = if ascending { min_i } else { max_i };
            if i0 > 0 {
                let mut tmp = Vec::with_capacity(nout);
                let mut j = i0;
                let mut ok = true;
                for i in 0..nout {
                    let cur = idx[j];
                    tmp.push(cur);
                    let nj = if j + 1 < nout { j + 1 } else { 0 };
                    if i < nout - 1 && (ascending != (cur < idx[nj])) {
                        ok = false;
                        break;
                    }
                    j = nj;
                }
                if ok {
                    idx = tmp;
                }
            }
        }
    }
    idx.iter().map(|&i| pts[i]).collect()
}

/// `Sklansky_<int, int64>`: writes the chain into `stack[off..]`, returns its length.
fn sklansky(
    p: &impl Fn(usize) -> (i32, i32),
    start: i64,
    end: i64,
    stack: &mut [i64],
    off: usize,
    nsign: i32,
    sign2: i32,
) -> usize {
    let incr: i64 = if end > start { 1 } else { -1 };
    let (mut pprev, mut pcur, mut pnext) = (start, start + incr, start + 2 * incr);
    let mut size = 3usize;
    if start == end || p(start as usize) == p(end as usize) {
        stack[off] = start;
        return 1;
    }
    stack[off] = pprev;
    stack[off + 1] = pcur;
    stack[off + 2] = pnext;
    let end = end + incr;
    while pnext != end {
        let cury = p(pcur as usize).1;
        let nexty = p(pnext as usize).1;
        let by = nexty - cury;
        if by.signum() != nsign {
            let ax = (p(pcur as usize).0 - p(pprev as usize).0) as i64;
            let ay = (cury - p(pprev as usize).1) as i64;
            let bx = (p(pnext as usize).0 - p(pcur as usize).0) as i64;
            let convexity = ay * bx - ax * by as i64;
            if convexity.signum() as i32 == sign2 && (ax != 0 || ay != 0) {
                pprev = pcur;
                pcur = pnext;
                pnext += incr;
                stack[off + size] = pnext;
                size += 1;
            } else if pprev == start {
                pcur = pnext;
                stack[off + 1] = pcur;
                pnext += incr;
                stack[off + 2] = pnext;
            } else {
                stack[off + size - 2] = pnext;
                pcur = pprev;
                pprev = stack[off + size - 4];
                size -= 1;
            }
        } else {
            pnext += incr;
            stack[off + size - 1] = pnext;
        }
    }
    size - 1
}

/// A `cv::RotatedRect`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RotRect {
    pub cx: f32,
    pub cy: f32,
    pub w: f32,
    pub h: f32,
    pub angle: f32,
}

fn rotating_calipers(pts: &[(f32, f32)]) -> [(f32, f32); 3] {
    let n = pts.len();
    let mut vect = vec![(0f32, 0f32); n];
    let mut inv_len = vec![0f32; n];
    let (mut left, mut bottom, mut right, mut top) = (0usize, 0usize, 0usize, 0usize);
    let mut pt0 = pts[0];
    let (mut left_x, mut right_x, mut top_y, mut bottom_y) = (pt0.0, pt0.0, pt0.1, pt0.1);
    for i in 0..n {
        if pt0.0 < left_x {
            left_x = pt0.0;
            left = i;
        }
        if pt0.0 > right_x {
            right_x = pt0.0;
            right = i;
        }
        if pt0.1 > top_y {
            top_y = pt0.1;
            top = i;
        }
        if pt0.1 < bottom_y {
            bottom_y = pt0.1;
            bottom = i;
        }
        let pt = pts[if i + 1 < n { i + 1 } else { 0 }];
        let dx = (pt.0 - pt0.0) as f64;
        let dy = (pt.1 - pt0.1) as f64;
        vect[i] = (dx as f32, dy as f32);
        inv_len[i] = (1.0 / (dx * dx + dy * dy).sqrt()) as f32;
        pt0 = pt;
    }
    let mut seq = [bottom, right, top, left];
    let mut minarea = f32::MAX;
    let mut buf: (usize, f32, f32, f32, f32, usize) = (0, 0.0, 0.0, 0.0, 0.0, 0);
    let rot_cw = |v: (f32, f32)| (v.1, -v.0);
    let first_is_right = |v1: (f32, f32), v2: (f32, f32)| {
        let t = rot_cw(v1);
        t.0 * v2.0 + t.1 * v2.1 < 0.0
    };
    for _ in 0..n {
        let rot =
            [vect[seq[0]], rot_cw(vect[seq[1]]), (-vect[seq[2]].0, -vect[seq[2]].1), (-vect[seq[3]].1, vect[seq[3]].0)];
        let mut main = 0;
        for i in 1..4 {
            if first_is_right(rot[i], rot[main]) {
                main = i;
            }
        }
        let pi = seq[main];
        let lead_x = vect[pi].0 * inv_len[pi];
        let lead_y = vect[pi].1 * inv_len[pi];
        let (base_a, base_b) = match main {
            0 => (lead_x, lead_y),
            1 => (lead_y, -lead_x),
            2 => (-lead_x, -lead_y),
            _ => (-lead_y, lead_x),
        };
        seq[main] += 1;
        if seq[main] == n {
            seq[main] = 0;
        }
        let dx = pts[seq[1]].0 - pts[seq[3]].0;
        let dy = pts[seq[1]].1 - pts[seq[3]].1;
        let width = dx * base_a + dy * base_b;
        let dx = pts[seq[2]].0 - pts[seq[0]].0;
        let dy = pts[seq[2]].1 - pts[seq[0]].1;
        let height = -dx * base_b + dy * base_a;
        let area = width * height;
        if area <= minarea {
            minarea = area;
            buf = (seq[3], base_a, width, base_b, height, seq[0]);
        }
    }
    let (a1, b1) = (buf.1, buf.3);
    let (a2, b2) = (-buf.3, buf.1);
    let c1 = a1 * pts[buf.0].0 + pts[buf.0].1 * b1;
    let c2 = a2 * pts[buf.5].0 + pts[buf.5].1 * b2;
    let idet = 1.0f32 / (a1 * b2 - a2 * b1);
    let px = (c1 * b2 - c2 * b1) * idet;
    let py = (a1 * c2 - a2 * c1) * idet;
    [(px, py), (a1 * buf.2, b1 * buf.2), (a2 * buf.4, b2 * buf.4)]
}

/// `cv2.minAreaRect` of integer points.
pub fn min_area_rect(points: &[(i32, i32)]) -> RotRect {
    let hull: Vec<(f32, f32)> = convex_hull_i(points).iter().map(|&(x, y)| (x as f32, y as f32)).collect();
    let mut angle = -std::f64::consts::PI / 2.0;
    let mut r = RotRect { cx: 0.0, cy: 0.0, w: 0.0, h: 0.0, angle: 0.0 };
    let n = hull.len();
    if n > 2 {
        let out = rotating_calipers(&hull);
        r.cx = out[0].0 + (out[1].0 + out[2].0) * 0.5;
        r.cy = out[0].1 + (out[1].1 + out[2].1) * 0.5;
        r.w = ((out[2].0 as f64) * out[2].0 as f64 + (out[2].1 as f64) * out[2].1 as f64).sqrt() as f32;
        r.h = ((out[1].0 as f64) * out[1].0 as f64 + (out[1].1 as f64) * out[1].1 as f64).sqrt() as f32;
        if out[1].0 == 0.0 && out[1].1 > 0.0 {
            std::mem::swap(&mut r.w, &mut r.h);
        } else {
            angle = -(out[1].0 as f64).atan2(out[1].1 as f64);
        }
    } else if n == 2 {
        r.cx = (hull[0].0 + hull[1].0) * 0.5;
        r.cy = (hull[0].1 + hull[1].1) * 0.5;
        let dx = (hull[0].0 - hull[1].0) as f64;
        let dy = (hull[0].1 - hull[1].1) as f64;
        r.w = 0.0;
        r.h = (dx * dx + dy * dy).sqrt() as f32;
        if dx == 0.0 {
            std::mem::swap(&mut r.w, &mut r.h);
        } else if dy < 0.0 {
            angle = dy.atan2(dx);
            std::mem::swap(&mut r.w, &mut r.h);
        } else if dy > 0.0 {
            angle = -dx.atan2(dy);
        }
    } else if n == 1 {
        r.cx = hull[0].0;
        r.cy = hull[0].1;
    }
    r.angle = (angle * 180.0 / std::f64::consts::PI) as f32;
    r
}

/// `cv2.boxPoints` (`RotatedRect::points`).
pub fn box_points(r: &RotRect) -> [(f32, f32); 4] {
    let a = r.angle as f64 * std::f64::consts::PI / 180.0;
    let b = a.cos() as f32 * 0.5;
    let a = a.sin() as f32 * 0.5;
    let (ah, aw, bh, bw) = (a * r.h, a * r.w, b * r.h, b * r.w);
    [
        (r.cx - ah - bw, r.cy + bh - aw),
        (r.cx + ah - bw, r.cy - bh - aw),
        (r.cx + ah + bw, r.cy - bh + aw),
        (r.cx - ah + bw, r.cy + bh + aw),
    ]
}

// --- fillPoly / mean -------------------------------------------------------------

fn clip_line(w: i64, h: i64, p1: &mut (i64, i64), p2: &mut (i64, i64)) -> bool {
    let (right, bottom) = (w - 1, h - 1);
    if w <= 0 || h <= 0 {
        return false;
    }
    let code =
        |p: (i64, i64)| (p.0 < 0) as i32 + (p.0 > right) as i32 * 2 + (p.1 < 0) as i32 * 4 + (p.1 > bottom) as i32 * 8;
    let (mut c1, mut c2) = (code(*p1), code(*p2));
    if (c1 & c2) == 0 && (c1 | c2) != 0 {
        if c1 & 12 != 0 {
            let a = if c1 < 8 { 0 } else { bottom };
            p1.0 += ((a - p1.1) as f64 * (p2.0 - p1.0) as f64 / (p2.1 - p1.1) as f64) as i64;
            p1.1 = a;
            c1 = (p1.0 < 0) as i32 + (p1.0 > right) as i32 * 2;
        }
        if c2 & 12 != 0 {
            let a = if c2 < 8 { 0 } else { bottom };
            p2.0 += ((a - p2.1) as f64 * (p2.0 - p1.0) as f64 / (p2.1 - p1.1) as f64) as i64;
            p2.1 = a;
            c2 = (p2.0 < 0) as i32 + (p2.0 > right) as i32 * 2;
        }
        if (c1 & c2) == 0 && (c1 | c2) != 0 {
            if c1 != 0 {
                let a = if c1 == 1 { 0 } else { right };
                p1.1 += ((a - p1.0) as f64 * (p2.1 - p1.1) as f64 / (p2.0 - p1.0) as f64) as i64;
                p1.0 = a;
                c1 = 0;
            }
            if c2 != 0 {
                let a = if c2 == 1 { 0 } else { right };
                p2.1 += ((a - p2.0) as f64 * (p2.1 - p1.1) as f64 / (p2.0 - p1.0) as f64) as i64;
                p2.0 = a;
                c2 = 0;
            }
        }
    }
    (c1 | c2) == 0
}

/// `Line(img, pt1, pt2, color, 8)` (LineIterator, left to right).
fn line8(m: &mut [u8], w: usize, h: usize, mut p1: (i64, i64), mut p2: (i64, i64), v: u8) {
    if (p1.0 as u64) >= w as u64 || (p2.0 as u64) >= w as u64 || (p1.1 as u64) >= h as u64 || (p2.1 as u64) >= h as u64
    {
        // `clipLine(Size, Point&, Point&)` works on int points.
        let (mut a, mut b) = ((p1.0 as i32 as i64, p1.1 as i32 as i64), (p2.0 as i32 as i64, p2.1 as i32 as i64));
        if !clip_line(w as i64, h as i64, &mut a, &mut b) {
            return;
        }
        p1 = (a.0 as i32 as i64, a.1 as i32 as i64);
        p2 = (b.0 as i32 as i64, b.1 as i32 as i64);
    }
    let (mut dx, mut dy) = (p2.0 - p1.0, p2.1 - p1.1);
    let mut start = p1;
    let (mut delta_x, mut delta_y) = (1i64, 1i64);
    if dx < 0 {
        dx = -dx;
        dy = -dy;
        start = p2;
    }
    if dy < 0 {
        dy = -dy;
        delta_y = -1;
    }
    let vert = dy > dx;
    if vert {
        std::mem::swap(&mut dx, &mut dy);
        std::mem::swap(&mut delta_x, &mut delta_y);
    }
    let mut err = dx - (dy + dy);
    let plus_delta = dx + dx;
    let minus_delta = -(dy + dy);
    let (mut minus_shift, mut plus_shift, mut minus_step, mut plus_step) = (delta_x, 0i64, 0i64, delta_y);
    if vert {
        std::mem::swap(&mut plus_step, &mut plus_shift);
        std::mem::swap(&mut minus_step, &mut minus_shift);
    }
    let (mut x, mut y) = start;
    for _ in 0..dx + 1 {
        m[y as usize * w + x as usize] = v;
        let mask = if err < 0 { -1i64 } else { 0 };
        err += minus_delta + (plus_delta & mask);
        x += minus_shift + (plus_shift & mask);
        y += minus_step + (plus_step & mask);
    }
}

#[derive(Clone, Copy)]
struct Edge {
    y0: i32,
    y1: i32,
    x: i64,
    dx: i64,
    next: usize,
}

const NIL: usize = usize::MAX;

/// `cv2.fillPoly(mask, [pts], value)` with LINE_8, shift 0.
pub fn fill_poly(m: &mut [u8], w: usize, h: usize, pts: &[(i32, i32)], v: u8) {
    const SHIFT: i64 = 16;
    let n = pts.len();
    if n == 0 {
        return;
    }
    let mut edges: Vec<Edge> = Vec::new();
    let conv = |p: (i32, i32)| ((p.0 as i64) << SHIFT, p.1 as i64);
    let mut pt0 = conv(pts[n - 1]);
    for &p in pts {
        let pt1 = conv(p);
        let mut t0 = ((pt0.0 + (1 << (SHIFT - 1))) >> SHIFT, pt0.1);
        let mut t1 = ((pt1.0 + (1 << (SHIFT - 1))) >> SHIFT, pt1.1);
        line8(m, w, h, t0, t1, v);
        let (mut c0, mut c1) = (pt0, pt1);
        if (t0.0 as u64) >= w as u64
            || (t1.0 as u64) >= w as u64
            || (t0.1 as u64) >= h as u64
            || (t1.1 as u64) >= h as u64
        {
            let (mut a, mut b) = ((t0.0 as i32 as i64, t0.1 as i32 as i64), (t1.0 as i32 as i64, t1.1 as i32 as i64));
            clip_line(w as i64, h as i64, &mut a, &mut b);
            t0 = (a.0 as i32 as i64, a.1 as i32 as i64);
            t1 = (b.0 as i32 as i64, b.1 as i32 as i64);
            if t0.1 != t1.1 {
                c0.1 = t0.1;
                c1.1 = t1.1;
            }
        }
        c0.0 = t0.0 << SHIFT;
        c1.0 = t1.0 << SHIFT;
        if pt0.1 != pt1.1 {
            let dx = (c1.0 - c0.0) / (c1.1 - c0.1);
            let e = if pt0.1 < pt1.1 {
                Edge { y0: pt0.1 as i32, y1: pt1.1 as i32, x: c0.0 + (pt0.1 - c0.1) * dx, dx, next: NIL }
            } else {
                Edge { y0: pt1.1 as i32, y1: pt0.1 as i32, x: c1.0 + (pt1.1 - c1.1) * dx, dx, next: NIL }
            };
            edges.push(e);
        }
        pt0 = pt1;
    }
    fill_edges(m, w, h, edges, v);
}

fn fill_edges(m: &mut [u8], w: usize, h: usize, mut edges: Vec<Edge>, v: u8) {
    const SHIFT: i64 = 16;
    let total = edges.len();
    if total < 2 {
        return;
    }
    let (mut y_max, mut y_min) = (i32::MIN, i32::MAX);
    let (mut x_max, mut x_min) = (-1i64, i64::MAX);
    for e in &edges {
        let x1 = e.x + (e.y1 - e.y0) as i64 * e.dx;
        y_min = y_min.min(e.y0);
        y_max = y_max.max(e.y1);
        x_min = x_min.min(e.x).min(x1);
        x_max = x_max.max(e.x).max(x1);
    }
    if y_max < 0 || y_min >= h as i32 || x_max < 0 || x_min >= ((w as i64) << SHIFT) {
        return;
    }
    edges.sort_by(|a, b| a.y0.cmp(&b.y0).then(a.x.cmp(&b.x)).then(a.dx.cmp(&b.dx)));
    edges.push(Edge { y0: i32::MAX, y1: 0, x: 0, dx: 0, next: NIL });
    // Index `total + 1` is the list head (`tmp`).
    let head = total + 1;
    edges.push(Edge { y0: i32::MAX, y1: 0, x: 0, dx: 0, next: NIL });
    let delta = (1i64 << SHIFT) - 1;
    let mut i = 0usize;
    let mut e = 0usize;
    let y_max = y_max.min(h as i32);
    let mut y = edges[e].y0;
    while y < y_max {
        let mut draw = false;
        let clipline = y < 0;
        let mut prelast = head;
        let mut last = edges[head].next;
        while last != NIL || edges[e].y0 == y {
            if last != NIL && edges[last].y1 == y {
                edges[prelast].next = edges[last].next;
                last = edges[last].next;
                continue;
            }
            let keep_prelast = prelast;
            if last != NIL && (edges[e].y0 > y || edges[last].x < edges[e].x) {
                prelast = last;
                last = edges[last].next;
            } else if i < total {
                edges[prelast].next = e;
                edges[e].next = last;
                prelast = e;
                i += 1;
                e = i;
            } else {
                break;
            }
            if draw {
                if !clipline {
                    let (kx, px) = (edges[keep_prelast].x, edges[prelast].x);
                    let (mut x1, mut x2) = if kx > px {
                        ((px + delta) >> SHIFT, kx >> SHIFT)
                    } else {
                        ((kx + delta) >> SHIFT, px >> SHIFT)
                    };
                    if x1 < w as i64 && x2 >= 0 {
                        x1 = x1.max(0);
                        x2 = x2.min(w as i64 - 1);
                        let row = y as usize * w;
                        for x in x1..=x2 {
                            m[row + x as usize] = v;
                        }
                    }
                }
                edges[keep_prelast].x += edges[keep_prelast].dx;
                edges[prelast].x += edges[prelast].dx;
            }
            draw = !draw;
        }
        // Bubble sort of the active list.
        let mut keep_prelast = NIL;
        loop {
            let mut prelast = head;
            let mut last = edges[head].next;
            let mut last_exchange = NIL;
            while last != keep_prelast && edges[last].next != NIL {
                let te = edges[last].next;
                if edges[last].x > edges[te].x {
                    edges[prelast].next = te;
                    edges[last].next = edges[te].next;
                    edges[te].next = last;
                    prelast = te;
                    last_exchange = prelast;
                } else {
                    prelast = last;
                    last = te;
                }
            }
            if last_exchange == NIL {
                break;
            }
            keep_prelast = last_exchange;
            if keep_prelast == edges[head].next || keep_prelast == head {
                break;
            }
        }
        y += 1;
    }
}

/// `cv2.mean(roi, mask)[0]` of a float map's ROI (`x0..=x1`, `y0..=y1`).
pub fn masked_mean(map: &[f32], mw: usize, x0: usize, y0: usize, x1: usize, y1: usize, mask: &[u8]) -> f64 {
    let rw = x1 - x0 + 1;
    let mut s = 0f64;
    let mut n = 0usize;
    for y in y0..=y1 {
        for x in x0..=x1 {
            if mask[(y - y0) * rw + (x - x0)] != 0 {
                s += map[y * mw + x] as f64;
                n += 1;
            }
        }
    }
    s * if n > 0 { 1.0 / n as f64 } else { 0.0 }
}

// --- perspective -------------------------------------------------------------------

/// `cv2.getPerspectiveTransform(src, dst)` (DECOMP_LU branch).
pub fn perspective_transform(src: &[(f32, f32); 4], dst: &[(f32, f32); 4]) -> [f64; 9] {
    let mut a = [[0f64; 8]; 8];
    let mut b = [0f64; 8];
    for i in 0..4 {
        a[i][0] = src[i].0 as f64;
        a[i + 4][3] = src[i].0 as f64;
        a[i][1] = src[i].1 as f64;
        a[i + 4][4] = src[i].1 as f64;
        a[i][2] = 1.0;
        a[i + 4][5] = 1.0;
        a[i][6] = (-src[i].0 * dst[i].0) as f64;
        a[i][7] = (-src[i].1 * dst[i].0) as f64;
        a[i + 4][6] = (-src[i].0 * dst[i].1) as f64;
        a[i + 4][7] = (-src[i].1 * dst[i].1) as f64;
        b[i] = dst[i].0 as f64;
        b[i + 4] = dst[i].1 as f64;
    }
    let mut m = [0f64; 9];
    let mut lu = a;
    let mut x = b;
    let m8 = 8;
    for i in 0..m8 {
        let mut k = i;
        for j in i + 1..m8 {
            if lu[j][i].abs() > lu[k][i].abs() {
                k = j;
            }
        }
        if lu[k][i].abs() < f64::EPSILON * 100.0 {
            return m;
        }
        if k != i {
            for j in i..m8 {
                let t = lu[i][j];
                lu[i][j] = lu[k][j];
                lu[k][j] = t;
            }
            x.swap(i, k);
        }
        let d = -1.0 / lu[i][i];
        for j in i + 1..m8 {
            let alpha = lu[j][i] * d;
            for kk in i + 1..m8 {
                lu[j][kk] += alpha * lu[i][kk];
            }
            x[j] += alpha * x[i];
        }
    }
    for i in (0..m8).rev() {
        let mut s = x[i];
        for kk in i + 1..m8 {
            s -= lu[i][kk] * x[kk];
        }
        x[i] = s / lu[i][i];
    }
    m[..8].copy_from_slice(&x);
    m[8] = 1.0;
    m
}

/// `cv::invert` of a 3x3 double matrix (closed form).
pub fn invert3(s: &[f64; 9]) -> [f64; 9] {
    let at = |r: usize, c: usize| s[r * 3 + c];
    let d = at(0, 0) * (at(1, 1) * at(2, 2) - at(1, 2) * at(2, 1))
        - at(0, 1) * (at(1, 0) * at(2, 2) - at(1, 2) * at(2, 0))
        + at(0, 2) * (at(1, 0) * at(2, 1) - at(1, 1) * at(2, 0));
    if d == 0.0 {
        return [0.0; 9];
    }
    let d = 1.0 / d;
    [
        (at(1, 1) * at(2, 2) - at(1, 2) * at(2, 1)) * d,
        (at(0, 2) * at(2, 1) - at(0, 1) * at(2, 2)) * d,
        (at(0, 1) * at(1, 2) - at(0, 2) * at(1, 1)) * d,
        (at(1, 2) * at(2, 0) - at(1, 0) * at(2, 2)) * d,
        (at(0, 0) * at(2, 2) - at(0, 2) * at(2, 0)) * d,
        (at(0, 2) * at(1, 0) - at(0, 0) * at(1, 2)) * d,
        (at(1, 0) * at(2, 1) - at(1, 1) * at(2, 0)) * d,
        (at(0, 1) * at(2, 0) - at(0, 0) * at(2, 1)) * d,
        (at(0, 0) * at(1, 1) - at(0, 1) * at(1, 0)) * d,
    ]
}

fn cubic_weights(alpha: f32) -> [f32; 4] {
    const A: f32 = -0.75;
    let a2 = alpha * alpha;
    let b = 1.0f32 - alpha;
    let b2 = b * b;
    let w0 = A * (alpha * b2);
    let w3 = A * (a2 * b);
    let w1 = a2.mul_add((A + 2.0).mul_add(alpha, -(A + 3.0)), 1.0);
    let w2 = ((1.0f32 - w0) - w1) - w3;
    [w0, w1, w2, w3]
}

/// `cv2.warpPerspective(img, M, (w, h), flags=INTER_CUBIC, borderMode=BORDER_REPLICATE)`
/// through OpenCV 5's generic float warp and its AVX2 bicubic kernel.
pub fn warp_perspective_cubic(src: &Img, m: &[f64; 9], dw: usize, dh: usize) -> Img {
    let inv = invert3(m);
    let mf: [f32; 9] = std::array::from_fn(|i| inv[i] as f32);
    let (sw, sh, c) = (src.w as i32, src.h as i32, src.c);
    let mut dst = Img::new(dw, dh, c);
    let big_w = sw.max(16) as f32;
    let big_h = sh.max(16) as f32;
    for y in 0..dh {
        let mx = (y as f32) * mf[1] + mf[2];
        let my = (y as f32) * mf[4] + mf[5];
        let mz = (y as f32) * mf[7] + mf[8];
        for x in 0..dw {
            let xf = x as f64;
            let invz = 1.0 / (mz as f64 + mf[6] as f64 * xf);
            let xs = ((mx as f64 + mf[0] as f64 * xf) * invz) as f32;
            let ys = ((my as f64 + mf[3] as f64 * xf) * invz) as f32;
            let vx = xs.max(-big_w).min(big_w * 2.0);
            let vy = ys.max(-big_h).min(big_h * 2.0);
            let ix = vx.floor() as i32;
            let iy = vy.floor() as i32;
            let wx = cubic_weights(vx - ix as f32);
            let wy = cubic_weights(vy - iy as f32);
            let xs4: [usize; 4] = std::array::from_fn(|k| (ix - 1 + k as i32).clamp(0, sw - 1) as usize);
            let ys4: [usize; 4] = std::array::from_fn(|k| (iy - 1 + k as i32).clamp(0, sh - 1) as usize);
            for ch in 0..c {
                let mut acc = 0f32;
                for (r, &yy) in ys4.iter().enumerate() {
                    let row = &src.data[yy * src.w * c..];
                    let v: [f32; 4] = std::array::from_fn(|k| row[xs4[k] * c + ch] as f32);
                    let mut s = v[0] * wx[0];
                    s = v[1].mul_add(wx[1], s);
                    s = v[2].mul_add(wx[2], s);
                    s = v[3].mul_add(wx[3], s);
                    acc = s.mul_add(wy[r], acc);
                }
                dst.data[(y * dw + x) * c + ch] =
                    acc.round_ties_even().clamp(i16::MIN as f32, i16::MAX as f32).clamp(0.0, 255.0) as u8;
            }
        }
    }
    dst
}

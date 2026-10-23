//! RapidOCR 3.8.4's default pipeline (PP-OCRv4 mobile det/cls/rec on
//! onnxruntime) around an [`Infer`] backend, with NumPy's dtype rules
//! followed step by step so the boxes, crops and tensors match Python's.

use super::{
    clipper,
    cv::{self, Img},
    npsort,
};

/// The three ONNX sessions.
pub trait Infer {
    /// `[1, 3, h, w]` in, the `[1, 1, h, w]` probability map out.
    fn det(&self, input: Vec<f32>, h: usize, w: usize) -> Result<Vec<f32>, String>;
    /// `[n, 3, 48, 192]` in, `[n, 2]` out.
    fn cls(&self, input: Vec<f32>, n: usize) -> Result<Vec<f32>, String>;
    /// `[n, 3, 48, w]` in, `([n, t, c], t, c)` out.
    fn rec(&self, input: Vec<f32>, n: usize, w: usize) -> Result<(Vec<f32>, usize, usize), String>;
    /// The rec model's `character` metadata, already `splitlines()`'d.
    fn characters(&self) -> &[String];
}

/// One recognised line: its corners in the original image and its text.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub boxes: [(f32, f32); 4],
    pub text: String,
    pub score: f64,
}

/// An error Python raises out of RapidOCR (a 500 upstream).
#[derive(Debug, PartialEq)]
pub struct OcrError(pub String);

pub type Quad = [(f32, f32); 4];

fn py_round_i64(x: f64) -> i64 {
    x.round_ties_even() as i64
}

/// `resize_image_within_bounds(img, 30, 2000)`.
fn within_bounds(img: &Img) -> Result<(Img, f64, f64), OcrError> {
    let mut cur = img.clone();
    let (mut rh, mut rw) = (1.0f64, 1.0f64);
    let side = |h: usize, w: usize, limit: f64, shrink: bool| -> (i64, i64) {
        let ratio = if (h > w) == shrink { limit / h as f64 } else { limit / w as f64 };
        let rh = (h as f64 * ratio) as i64;
        let rw = (w as f64 * ratio) as i64;
        (py_round_i64(rh as f64 / 32.0) * 32, py_round_i64(rw as f64 / 32.0) * 32)
    };
    if cur.h.max(cur.w) > 2000 {
        let (h, w) = (cur.h, cur.w);
        let (nh, nw) = side(h, w, 2000.0, true);
        if nh <= 0 || nw <= 0 {
            return Err(OcrError("ResizeImgError".into()));
        }
        cur = cv::resize_linear(&cur, nw as usize, nh as usize);
        rh = h as f64 / nh as f64;
        rw = w as f64 / nw as f64;
    }
    if cur.h.min(cur.w) < 30 {
        let (h, w) = (cur.h, cur.w);
        // `increase_min_side`: the ratio comes from the shorter side.
        let ratio = if h < w { 30.0 / h as f64 } else { 30.0 / w as f64 };
        let nh = py_round_i64(((h as f64 * ratio) as i64) as f64 / 32.0) * 32;
        let nw = py_round_i64(((w as f64 * ratio) as i64) as f64 / 32.0) * 32;
        if nh <= 0 || nw <= 0 {
            return Err(OcrError("ResizeImgError".into()));
        }
        cur = cv::resize_linear(&cur, nw as usize, nh as usize);
        rh = h as f64 / nh as f64;
        rw = w as f64 / nw as f64;
    }
    Ok((cur, rh, rw))
}

/// `apply_vertical_padding(img, op, 8, 30)`: the image and its top padding.
fn vertical_padding(img: &Img) -> (Img, usize) {
    let (h, w) = (img.h, img.w);
    if h <= 30 || w as f64 / h as f64 > 8.0 {
        let new_h = ((w as f64 / 8.0) as i64).max(30) * 2;
        let pad = ((new_h - h as i64).abs() as f64 / 2.0) as usize;
        (cv::pad_vertical(img, pad, pad), pad)
    } else {
        (img.clone(), 0)
    }
}

/// `DetPreProcess` (limit_type "min", 736): the tensor and its size.
fn det_input(img: &Img) -> Option<(Vec<f32>, usize, usize)> {
    let (h, w) = (img.h, img.w);
    let ratio = if h.min(w) < 736 { if h < w { 736.0 / h as f64 } else { 736.0 / w as f64 } } else { 1.0 };
    let rh = py_round_i64(((h as f64 * ratio) as i64) as f64 / 32.0) * 32;
    let rw = py_round_i64(((w as f64 * ratio) as i64) as f64 / 32.0) * 32;
    if rh <= 0 || rw <= 0 {
        return None;
    }
    let (rh, rw) = (rh as usize, rw as usize);
    let r = cv::resize_linear(img, rw, rh);
    let scale = (1.0f64 / 255.0) as f32;
    let mut out = vec![0f32; 3 * rh * rw];
    for y in 0..rh {
        for x in 0..rw {
            for ch in 0..3 {
                let v = r.data[(y * rw + x) * 3 + ch] as f32 * scale;
                out[ch * rh * rw + y * rw + x] = ((v as f64 - 0.5) / 0.5) as f32;
            }
        }
    }
    Some((out, rh, rw))
}

/// `get_mini_boxes`: the ordered corners and the short side.
pub fn mini_box(points: &[(i32, i32)]) -> (Quad, f32) {
    let r = cv::min_area_rect(points);
    let mut pts = cv::box_points(&r).to_vec();
    pts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let (i1, i4) = if pts[1].1 > pts[0].1 { (0, 1) } else { (1, 0) };
    let (i2, i3) = if pts[3].1 > pts[2].1 { (2, 3) } else { (3, 2) };
    ([pts[i1], pts[i2], pts[i3], pts[i4]], r.w.min(r.h))
}

/// `box_score_fast`.
pub fn box_score(pred: &[f32], w: usize, h: usize, quad: &Quad) -> f64 {
    let fmin = |f: fn(&(f32, f32)) -> f32| quad.iter().map(f).fold(f32::INFINITY, f32::min);
    let fmax = |f: fn(&(f32, f32)) -> f32| quad.iter().map(f).fold(f32::NEG_INFINITY, f32::max);
    let clip = |v: f32, hi: usize| (v as i32).clamp(0, hi as i32 - 1) as usize;
    let xmin = clip(fmin(|p| p.0).floor(), w);
    let xmax = clip(fmax(|p| p.0).ceil(), w);
    let ymin = clip(fmin(|p| p.1).floor(), h);
    let ymax = clip(fmax(|p| p.1).ceil(), h);
    let (mw, mh) = (xmax - xmin + 1, ymax - ymin + 1);
    let mut mask = vec![0u8; mw * mh];
    let poly: Vec<(i32, i32)> = quad.iter().map(|p| ((p.0 - xmin as f32) as i32, (p.1 - ymin as f32) as i32)).collect();
    cv::fill_poly(&mut mask, mw, mh, &poly, 1);
    cv::masked_mean(pred, w, xmin, ymin, xmax, ymax, &mask)
}

/// shapely `Polygon(quad).area * 1.6 / .length`.
pub fn unclip_distance(q: &Quad) -> f64 {
    let ring: Vec<(f64, f64)> = q.iter().chain(std::iter::once(&q[0])).map(|&(x, y)| (x as f64, y as f64)).collect();
    let x0 = ring[0].0;
    let mut sum = 0.0;
    for i in 1..ring.len() - 1 {
        sum += (ring[i].0 - x0) * (ring[i - 1].1 - ring[i + 1].1);
    }
    let area = (sum / 2.0).abs();
    let mut len = 0.0;
    for i in 1..ring.len() {
        let (dx, dy) = (ring[i].0 - ring[i - 1].0, ring[i].1 - ring[i - 1].1);
        len += (dx * dx + dy * dy).sqrt();
    }
    area * 1.6 / len
}

/// `DBPostProcess` then `filter_det_res` and `sorted_boxes`.
fn det_boxes(pred: &[f32], w: usize, h: usize, dest_w: usize, dest_h: usize) -> Result<Vec<Quad>, OcrError> {
    let seg: Vec<u8> = pred.iter().map(|&p| (p > 0.3) as u8).collect();
    let mask = cv::dilate2x2(&seg, w, h);
    let contours = cv::find_contours(&mask, w, h);
    let mut boxes: Vec<[(i32, i32); 4]> = Vec::new();
    for contour in contours.iter().take(1000) {
        let (quad, sside) = mini_box(contour);
        if sside < 3.0 {
            continue;
        }
        let score = box_score(pred, w, h, &quad);
        if 0.5 > score {
            continue;
        }
        let path: Vec<(i64, i64)> = quad.iter().map(|p| (p.0 as i64, p.1 as i64)).collect();
        let expanded =
            clipper::offset_round(&path, unclip_distance(&quad)).ok_or_else(|| OcrError("clipper".into()))?;
        let lens: Vec<usize> = expanded.iter().map(Vec::len).collect();
        if lens.windows(2).any(|l| l[0] != l[1]) {
            return Err(OcrError("ragged unclip".into()));
        }
        let pts: Vec<(i32, i32)> = expanded.iter().flatten().map(|&(x, y)| (x as i32, y as i32)).collect();
        let (q2, sside) = mini_box(&pts);
        if sside < 5.0 {
            continue;
        }
        let map = |v: f32, size: usize, dest: usize| -> i32 {
            ((v / size as f32 * dest as f32).round_ties_even()).clamp(0.0, dest as f32) as i32
        };
        boxes.push(std::array::from_fn(|i| (map(q2[i].0, w, dest_w), map(q2[i].1, h, dest_h))));
    }
    let mut kept: Vec<Quad> = Vec::new();
    for b in boxes {
        let mut r = order_clockwise(&b);
        for p in r.iter_mut() {
            p.0 = p.0.max(0.0).min((dest_w - 1) as f32).trunc();
            p.1 = p.1.max(0.0).min((dest_h - 1) as f32).trunc();
        }
        let norm = |a: (f32, f32), b: (f32, f32)| {
            let (dx, dy) = (a.0 - b.0, a.1 - b.1);
            (dx * dx + dy * dy).sqrt() as i64
        };
        if norm(r[0], r[1]) <= 3 || norm(r[0], r[3]) <= 3 {
            continue;
        }
        kept.push(r);
    }
    Ok(sort_boxes(kept))
}

/// `order_points_clockwise`.
fn order_clockwise(b: &[(i32, i32); 4]) -> Quad {
    let xs: Vec<i32> = b.iter().map(|p| p.0).collect();
    let xo = npsort::argsort_i32(&xs);
    let left = [b[xo[0]], b[xo[1]]];
    let right = [b[xo[2]], b[xo[3]]];
    let lo = npsort::argsort_i32(&[left[0].1, left[1].1]);
    let ro = npsort::argsort_i32(&[right[0].1, right[1].1]);
    let (tl, bl) = (left[lo[0]], left[lo[1]]);
    let (tr, br) = (right[ro[0]], right[ro[1]]);
    [tl, tr, br, bl].map(|p| (p.0 as f32, p.1 as f32))
}

/// `TextDetector.sorted_boxes`.
fn sort_boxes(boxes: Vec<Quad>) -> Vec<Quad> {
    let mut by_y: Vec<usize> = (0..boxes.len()).collect();
    by_y.sort_by(|&a, &b| boxes[a][0].1.partial_cmp(&boxes[b][0].1).unwrap_or(std::cmp::Ordering::Equal));
    let mut line = 0i32;
    let mut keyed: Vec<(i32, f32, usize)> = Vec::with_capacity(by_y.len());
    for (k, &i) in by_y.iter().enumerate() {
        if k > 0 && boxes[i][0].1 - boxes[by_y[k - 1]][0].1 >= 10.0 {
            line += 1;
        }
        keyed.push((line, boxes[i][0].0, i));
    }
    keyed.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal)));
    keyed.into_iter().map(|(_, _, i)| boxes[i]).collect()
}

/// `get_rotate_crop_image`.
pub fn crop(img: &Img, q: &Quad) -> Img {
    let norm = |a: (f32, f32), b: (f32, f32)| {
        let (dx, dy) = (a.0 - b.0, a.1 - b.1);
        (dx * dx + dy * dy).sqrt()
    };
    let cw = norm(q[0], q[1]).max(norm(q[2], q[3])) as usize;
    let ch = norm(q[0], q[3]).max(norm(q[1], q[2])) as usize;
    let dst = [(0.0, 0.0), (cw as f32, 0.0), (cw as f32, ch as f32), (0.0, ch as f32)];
    let m = cv::perspective_transform(q, &dst);
    let out = cv::warp_perspective_cubic(img, &m, cw, ch);
    if out.h as f64 * 1.0 / out.w as f64 >= 1.5 { cv::rot90(&out) } else { out }
}

/// `resize_norm_img` for cls (`width` 192) or rec (`width` from the batch).
fn norm_into(img: &Img, width: usize, resized_w: usize, out: &mut [f32]) {
    let r = cv::resize_linear(img, resized_w, 48);
    for ch in 0..3 {
        for y in 0..48 {
            for x in 0..resized_w {
                let v = r.data[(y * resized_w + x) * 3 + ch] as f32 / 255.0f32;
                out[ch * 48 * width + y * width + x] = (v - 0.5) / 0.5;
            }
        }
    }
}

fn resized_w(img: &Img, max_w: usize) -> usize {
    let ratio = img.w as f64 / img.h as f64;
    let c = (48.0 * ratio).ceil();
    if c > max_w as f64 { max_w } else { c as usize }
}

/// `TextClassifier`: rotates the crops it reads as upside down.
fn classify(inf: &dyn Infer, imgs: &mut [Img]) -> Result<(), OcrError> {
    let ratios: Vec<f64> = imgs.iter().map(|i| i.w as f64 / i.h as f64).collect();
    let order = npsort::argsort_f64(&ratios);
    for chunk in order.chunks(6) {
        let mut batch = vec![0f32; chunk.len() * 3 * 48 * 192];
        for (k, &i) in chunk.iter().enumerate() {
            let rw = resized_w(&imgs[i], 192);
            norm_into(&imgs[i], 192, rw, &mut batch[k * 3 * 48 * 192..(k + 1) * 3 * 48 * 192]);
        }
        let probs = inf.cls(batch, chunk.len()).map_err(OcrError)?;
        for (k, &i) in chunk.iter().enumerate() {
            let p = &probs[k * 2..k * 2 + 2];
            let idx = if p[1] > p[0] { 1 } else { 0 };
            if idx == 1 && p[1] > 0.9f32 {
                imgs[i] = cv::rotate180(&imgs[i]);
            }
        }
    }
    Ok(())
}

/// NumPy's pairwise float64 sum (`np.add.reduce`) for a contiguous array.
fn pairwise_sum(a: &[f64]) -> f64 {
    let n = a.len();
    if n < 8 {
        let mut res = 0.0;
        for &v in a {
            res += v;
        }
        res
    } else if n <= 128 {
        let mut r = [a[0], a[1], a[2], a[3], a[4], a[5], a[6], a[7]];
        let mut i = 8;
        while i < n - n % 8 {
            for k in 0..8 {
                r[k] += a[i + k];
            }
            i += 8;
        }
        let mut res = ((r[0] + r[1]) + (r[2] + r[3])) + ((r[4] + r[5]) + (r[6] + r[7]));
        while i < n {
            res += a[i];
            i += 1;
        }
        res
    } else {
        let mut n2 = n / 2;
        n2 -= n2 % 8;
        pairwise_sum(&a[..n2]) + pairwise_sum(&a[n2..])
    }
}

/// Python's `round(x, 5)` (correctly rounded).
fn py_round5(x: f64) -> f64 {
    format!("{x:.5}").parse().unwrap_or(x)
}

/// `CTCLabelDecode` for one sequence: text and `np.mean(confs).round(5)`.
fn ctc_decode(probs: &[f32], t: usize, c: usize, chars: &[String]) -> (String, f64) {
    let mut text = String::new();
    let mut confs: Vec<f64> = Vec::new();
    let mut prev = usize::MAX;
    for step in 0..t {
        let row = &probs[step * c..(step + 1) * c];
        let (mut best, mut best_v) = (0usize, row[0]);
        for (j, &v) in row.iter().enumerate().skip(1) {
            if v > best_v {
                best = j;
                best_v = v;
            }
        }
        if best != prev && best != 0 {
            text.push_str(chars.get(best).map_or("", String::as_str));
            confs.push(py_round5(best_v as f64));
        }
        prev = best;
    }
    if confs.is_empty() {
        confs.push(0.0);
    }
    let mean = pairwise_sum(&confs) / confs.len() as f64;
    ((text), ((mean * 1e5).round_ties_even() / 1e5))
}

/// `TextRecognizer`.
fn recognize(inf: &dyn Infer, imgs: &[Img]) -> Result<Vec<(String, f64)>, OcrError> {
    let mut chars: Vec<String> = Vec::with_capacity(inf.characters().len() + 2);
    chars.push("blank".into());
    chars.extend(inf.characters().iter().cloned());
    chars.push(" ".into());
    let ratios: Vec<f64> = imgs.iter().map(|i| i.w as f64 * 1.0 / i.h as f64).collect();
    let order = npsort::argsort_f64(&ratios);
    let mut out = vec![(String::new(), 0.0f64); imgs.len()];
    for chunk in order.chunks(6) {
        let mut max_ratio = 320.0f64 / 48.0;
        for &i in chunk {
            max_ratio = max_ratio.max(ratios[i]);
        }
        let width = (48.0 * max_ratio) as usize;
        let per = 3 * 48 * width;
        let mut batch = vec![0f32; chunk.len() * per];
        for (k, &i) in chunk.iter().enumerate() {
            let rw = resized_w(&imgs[i], width);
            norm_into(&imgs[i], width, rw, &mut batch[k * per..(k + 1) * per]);
        }
        let (probs, t, c) = inf.rec(batch, chunk.len(), width).map_err(OcrError)?;
        for (k, &i) in chunk.iter().enumerate() {
            out[i] = ctc_decode(&probs[k * t * c..(k + 1) * t * c], t, c, &chars);
        }
    }
    Ok(out)
}

/// `RapidOCR()(arr)` for an RGB array: the kept lines in RapidOCR's order.
pub fn run(inf: &dyn Infer, img: &Img) -> Result<Vec<Line>, OcrError> {
    let (ori_h, ori_w) = (img.h, img.w);
    let (pre, ratio_h, ratio_w) = within_bounds(img)?;
    let (padded, top) = vertical_padding(&pre);
    let Some((input, ih, iw)) = det_input(&padded) else { return Ok(Vec::new()) };
    let pred = inf.det(input, ih, iw).map_err(OcrError)?;
    let quads = det_boxes(&pred, iw, ih, padded.w, padded.h)?;
    if quads.is_empty() {
        return Ok(Vec::new());
    }
    let crops: Vec<Img> = quads.iter().map(|q| crop(&padded, q)).collect();
    for c in &crops {
        if py_round_i64(c.h as f64 * ratio_h) == 0 || py_round_i64(c.w as f64 * ratio_w) == 0 {
            return Err(OcrError("map_img_to_original: empty resize".into()));
        }
    }
    let mut rotated = crops;
    classify(inf, &mut rotated)?;
    let recs = recognize(inf, &rotated)?;
    let (rw32, rh32) = (ratio_w as f32, ratio_h as f32);
    let mut lines = Vec::new();
    for (q, (text, score)) in quads.iter().zip(recs) {
        if crate::pystr::strip(&text).is_empty() || score < 0.5 {
            continue;
        }
        let boxes = q.map(|(x, y)| {
            let (x, y) = (x * rw32, (y - top as f32) * rh32);
            let x = if x < 0.0 { 0.0 } else { x };
            let y = if y < 0.0 { 0.0 } else { y };
            (if x > ori_w as f32 { ori_w as f32 } else { x }, if y > ori_h as f32 { ori_h as f32 } else { y })
        });
        lines.push(Line { boxes, text, score });
    }
    Ok(lines)
}

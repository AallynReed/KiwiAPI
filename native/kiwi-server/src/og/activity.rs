//! `og_image._draw`: the 1200x630 player-activity card.

use chrono::{DateTime, Datelike, Utc};
use kiwi_pil::{Draw, ImageError, Ink, Library, Rgba};
use serde_json::Value;

use super::{
    H, W, pymath,
    text::{Fonts, Text, thousands},
};

pub const BG: [u8; 3] = [13, 17, 23];
const GREEN: [u8; 3] = [93, 208, 120];
const GREEN_HI: [u8; 3] = [142, 229, 160];
pub const MUTE: [u8; 3] = [154, 164, 178];
pub const TEXT: [u8; 3] = [230, 237, 243];
const GRID: [u8; 3] = [40, 48, 58];
const AREA: Ink = [63, 185, 80, 70];
const RESET_C: Ink = [230, 237, 243, 95];
const TROVE_OFFSET: i64 = -11 * 3600;

pub fn rgba(c: [u8; 3], a: u8) -> Ink {
    [c[0], c[1], c[2], a]
}

#[derive(Clone, Copy, Debug)]
pub struct Point {
    pub t: i64,
    pub active: f64,
}

/// What `_draw` reads from `activity_series()` and `estimate_active_players()`.
#[derive(Clone, Debug, Default)]
pub struct Inputs {
    pub points: Vec<Point>,
    pub window_start: i64,
    pub window_end: i64,
    pub bucket_seconds: i64,
    pub latest: Option<f64>,
    pub estimate_24h: Option<f64>,
    pub estimate_7d: Option<f64>,
}

fn num(v: Option<&Value>) -> Option<f64> {
    v.and_then(Value::as_f64)
}

impl Inputs {
    pub fn from_payloads(series: &Value, live: &Value) -> Self {
        let points = series
            .get("points")
            .and_then(Value::as_array)
            .map(|ps| {
                ps.iter()
                    .map(|p| Point {
                        t: p.get("t").and_then(Value::as_i64).unwrap_or(0),
                        active: num(p.get("active")).unwrap_or(0.0),
                    })
                    .collect()
            })
            .unwrap_or_default();
        Inputs {
            points,
            window_start: series.get("window_start").and_then(Value::as_i64).unwrap_or(0),
            window_end: series.get("window_end").and_then(Value::as_i64).unwrap_or(0),
            bucket_seconds: num(series.get("bucket_seconds")).map(|b| b as i64).unwrap_or(0),
            latest: num(series.get("latest")),
            estimate_24h: num(live.get("estimate_24h")),
            estimate_7d: num(live.get("estimate_7d")),
        }
    }
}

/// `_split_gaps`: contiguous runs (indices) and the (before, after) bridges.
pub fn split_gaps(points: &[Point], bucket: i64) -> (Vec<Vec<usize>>, Vec<(usize, usize)>) {
    if points.len() < 2 {
        return (if points.is_empty() { vec![] } else { vec![(0..points.len()).collect()] }, vec![]);
    }
    let (keys, limit): (Vec<f64>, f64) = if bucket > 0 {
        (points.iter().map(|p| p.t.div_euclid(bucket) as f64).collect(), 1.5)
    } else {
        let keys: Vec<i64> = points.iter().map(|p| p.t).collect();
        let mut deltas: Vec<i64> = keys.windows(2).map(|w| w[1] - w[0]).collect();
        deltas.sort_unstable();
        let med = deltas[deltas.len() / 2];
        if med <= 0 {
            return (vec![(0..points.len()).collect()], vec![]);
        }
        (keys.iter().map(|&k| k as f64).collect(), med as f64 * 1.5)
    };
    let mut runs = Vec::new();
    let mut bridges = Vec::new();
    let mut run = vec![0];
    for i in 1..points.len() {
        if keys[i] - keys[i - 1] > limit {
            bridges.push((i - 1, i));
            runs.push(std::mem::take(&mut run));
        }
        run.push(i);
    }
    runs.push(run);
    (runs, bridges)
}

/// `_resets`: daily resets (11:00 UTC) inside [lo, hi].
pub fn resets(lo: i64, hi: i64) -> Vec<f64> {
    let mut r = (lo.div_euclid(86_400) * 86_400 + 11 * 3600) as f64;
    if r < lo as f64 {
        r += 86_400.0;
    }
    let mut out = Vec::new();
    while r <= hi as f64 {
        out.push(r);
        r += 86_400.0;
    }
    out
}

/// `_trove(ts, fmt)` for the axis formats ("%a" / "%b %d").
pub fn trove_label(ts: f64, weekday: bool) -> String {
    let secs = (ts + TROVE_OFFSET as f64).floor() as i64;
    let d = DateTime::<Utc>::from_timestamp(secs, 0).unwrap_or_default();
    if weekday { d.format("%a").to_string() } else { format!("{} {:02}", d.format("%b"), d.day()) }
}

/// `_dashed`.
fn dashed(d: &mut Draw, p0: (f64, f64), p1: (f64, f64), fill: Ink, width: i32) -> Result<(), ImageError> {
    let (dash, gap) = (10.0, 9.0);
    let span = pymath::hypot(p1.0 - p0.0, p1.1 - p0.1);
    if span <= 0.0 {
        return Ok(());
    }
    let step = dash + gap;
    let n = (span / step).floor() as i64 + 1;
    for i in 0..n {
        let a = ((i as f64 * step) / span).min(1.0);
        let b = ((i as f64 * step + dash) / span).min(1.0);
        d.line(
            &[
                (p0.0 + (p1.0 - p0.0) * a, p0.1 + (p1.1 - p0.1) * a),
                (p0.0 + (p1.0 - p0.0) * b, p0.1 + (p1.1 - p0.1) * b),
            ],
            fill,
            width,
            false,
        )?;
    }
    Ok(())
}

/// The card as RGBA pixels (before `convert("RGB")`).
pub fn draw(lib: &Library, fonts: &Fonts, inp: &Inputs, period: &str) -> Result<Rgba, ImageError> {
    let mut img = Rgba::new(W, H, rgba(BG, 255));
    let mut d = Draw::new(&mut img);
    let mut tx = Text::new(lib, fonts);
    let m = 64.0;
    let (wf, hf) = (W as f64, H as f64);

    d.rectangle([0.0, 0.0, wf, 6.0], Some(rgba(GREEN, 255)), None, 1)?;
    let cy = 82.0;
    d.ellipse([m, cy - 8.0, m + 16.0, cy + 8.0], Some(rgba(GREEN_HI, 255)), None, 1)?;
    tx.fit_text(
        &mut d,
        (m + 28.0, cy - 16.0),
        "PLAYER ACTIVITY",
        1200 - 64 - (64 + 28),
        26,
        rgba(MUTE, 255),
        true,
        false,
        None,
    )?;

    let mut n = inp.estimate_24h;
    if n.is_none()
        && let Some(latest) = inp.latest.filter(|l| *l != 0.0)
    {
        n = Some(latest.round_ties_even());
    }
    let num_txt = match n {
        Some(v) => format!("~{}", thousands(v.trunc() as i64)),
        None => "—".to_owned(),
    };
    tx.draw(&mut d, (m, 116.0), &num_txt, 104, true, rgba(GREEN_HI, 255))?;
    let after = 64 + tx.w(&num_txt, 104, true)? + 24;

    let (rx, ry) = (1200 - 64, 92.0);
    let mut stats_left = rx;
    if let Some(val) = inp.estimate_7d {
        let label = "ACTIVE · 7D";
        let vt = format!("~{}", thousands(val.trunc() as i64));
        let w = tx.w(&vt, 30, true)?.max(tx.w(label, 19, true)?);
        stats_left = stats_left.min(rx - w);
        tx.draw(&mut d, ((rx - w) as f64, ry), label, 19, true, rgba(MUTE, 255))?;
        tx.draw(&mut d, ((rx - w) as f64, ry + 24.0), &vt, 30, true, rgba(GREEN_HI, 255))?;
    }
    let sub_max = 160.max(stats_left - 20 - after);
    tx.fit_text(&mut d, (after as f64, 150.0), "active players", sub_max, 33, rgba(TEXT, 255), false, false, None)?;
    tx.fit_text(
        &mut d,
        (after as f64, 192.0),
        "in the last 24 hours",
        sub_max,
        33,
        rgba(MUTE, 255),
        false,
        false,
        None,
    )?;

    let (px0, px1, py0, py1) = (64.0, 1200.0 - 64.0, 322.0, 540.0);
    let pts = &inp.points;
    if pts.len() >= 2 {
        let xmin = inp.window_start.min(pts[0].t);
        let xmax = inp.window_end.max(pts[pts.len() - 1].t);
        let xr = 1.max(xmax - xmin);
        let peak = pts.iter().map(|p| p.active).fold(f64::NEG_INFINITY, |a, b| if b > a { b } else { a });
        let ymax = (if peak == 0.0 { 1.0 } else { peak }) * 1.15;
        let fx = |t: f64| px0 + (t - xmin as f64) / xr as f64 * (px1 - px0);
        let fy = |v: f64| py1 - (v / ymax) * (py1 - py0);
        let at = |p: &Point| (fx(p.t as f64), fy(p.active));

        let (runs, bridges) = split_gaps(pts, inp.bucket_seconds);
        let run_px: Vec<Vec<(f64, f64)>> = runs.iter().map(|r| r.iter().map(|&i| at(&pts[i])).collect()).collect();
        for seg in &run_px {
            if seg.len() >= 2 {
                let mut poly = seg.clone();
                poly.push((seg[seg.len() - 1].0, py1));
                poly.push((seg[0].0, py1));
                d.polygon(&poly, AREA)?;
            }
        }
        d.line(&[(px0, py1), (px1, py1)], rgba(GRID, 255), 2, false)?;

        if period == "7d" {
            for r in resets(xmin, xmax) {
                let x = fx(r);
                if px0 - 1.0 <= x && x <= px1 + 1.0 {
                    let mut y = py0;
                    while y < py1 {
                        d.line(&[(x, y), (x, (y + 9.0).min(py1))], RESET_C, 2, false)?;
                        y += 15.0;
                    }
                }
            }
        }
        for &(a, b) in &bridges {
            dashed(&mut d, at(&pts[a]), at(&pts[b]), rgba(MUTE, 190), 3)?;
        }
        for seg in &run_px {
            if seg.len() >= 2 {
                d.line(seg, rgba(GREEN, 255), 5, true)?;
            } else {
                let (sx, sy) = seg[0];
                d.ellipse([sx - 4.0, sy - 4.0, sx + 4.0, sy + 4.0], Some(rgba(GREEN, 255)), None, 1)?;
            }
        }
        let (lx, ly) = at(&pts[pts.len() - 1]);
        d.ellipse([lx - 7.0, ly - 7.0, lx + 7.0, ly + 7.0], Some(rgba(GREEN_HI, 255)), Some(rgba(BG, 255)), 3)?;

        for frac in [0.0, 0.5, 1.0] {
            let tt = xmin as f64 + frac * xr as f64;
            let lab = trove_label(tt, period == "7d");
            let w = tx.w(&lab, 22, false)?;
            let shift = if frac == 0.0 {
                0.0
            } else if frac == 0.5 {
                w as f64 / 2.0
            } else {
                w as f64
            };
            tx.draw(&mut d, (fx(tt) - shift, py1 + 12.0), &lab, 22, false, rgba(MUTE, 255))?;
        }
        let top = thousands((ymax / 1.15).trunc() as i64);
        tx.draw(&mut d, (px0, py0 - 32.0), &top, 22, false, rgba(MUTE, 170))?;
    } else {
        tx.fit_text(
            &mut d,
            (m, 400.0),
            "Activity data warming up…",
            1200 - 2 * 64,
            33,
            rgba(MUTE, 255),
            false,
            false,
            None,
        )?;
    }

    let site = "trove.aallyn.net/activity";
    tx.draw(&mut d, (m, hf - 54.0), site, 24, false, rgba(MUTE, 255))?;
    let lw = tx.w(site, 24, false)?;
    let label = if period == "1m" { "Last 30 days" } else { "Last 7 days" };
    let fr = format!("{label} · Trove server time (UTC−11)");
    tx.fit_text(
        &mut d,
        (wf - m, hf - 54.0),
        &fr,
        (1200 - 64) - (64 + lw + 24),
        24,
        rgba(MUTE, 210),
        false,
        true,
        None,
    )?;
    Ok(img)
}

//! `app/images/render.py`: a design drawn to a PNG exactly as Pillow 13 draws
//! it. Only PNG blobs are decoded here (see `mod.rs`: anything else defers).

use std::{collections::HashMap, sync::LazyLock};

use chrono::{DateTime, Utc};
use kiwi_core::pystr::strip;
use kiwi_pil::{Draw, ImageError, Ink, Library, Mask, Rgba, decode, draw::textbbox, ops};
use regex::{Captures, Regex};
use serde_json::{Map, Value};

use super::model::{Background, Layer, MAX_DIM, MAX_LAYERS, MIN_DIM};
use crate::{
    feeds::pyval::{py_int, py_str},
    og::text::{Fonts, Text},
};

pub type Ctx = Map<String, Value>;

static VAR_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\{([a-zA-Z0-9_]+)(?::([a-zA-Z]))?\}").expect("placeholder regex"));
static MD_LINK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[([^\]]+)\]\((?:https?:)?[^)]+\)").expect("link regex"));

/// A layer that raised: Python's `except Exception: continue`.
#[derive(Debug)]
pub struct Skip;

impl From<ImageError> for Skip {
    fn from(_: ImageError) -> Self {
        Skip
    }
}

/// `_human_time`; `None` where `datetime.fromtimestamp` raises.
pub fn human_time(unix: i128, style: &str, now: i64) -> Option<String> {
    if style == "R" {
        let diff = unix - now as i128;
        let a = diff.abs();
        let (n, unit) = if a < 60 {
            (a, "second")
        } else if a < 3600 {
            ((a as f64 / 60.0).round_ties_even() as i128, "minute")
        } else if a < 86400 {
            ((a as f64 / 3600.0).round_ties_even() as i128, "hour")
        } else {
            ((a as f64 / 86400.0).round_ties_even() as i128, "day")
        };
        let s = if n == 1 { "" } else { "s" };
        return Some(if diff >= 0 { format!("in {n} {unit}{s}") } else { format!("{n} {unit}{s} ago") });
    }
    let d = DateTime::<Utc>::from_timestamp(i64::try_from(unix).ok()?, 0)?;
    if !(1..=9999).contains(&chrono::Datelike::year(&d)) {
        return None;
    }
    let fmt = match style {
        "t" => "%H:%M",
        "T" => "%H:%M:%S",
        "d" => "%Y-%m-%d",
        "D" => "%d %B %Y",
        "f" => "%d %B %Y %H:%M",
        "F" => "%A, %d %B %Y %H:%M",
        _ => "%Y-%m-%d %H:%M",
    };
    Some(d.format(fmt).to_string())
}

/// `substitute` (the image flavour: timestamps become text).
pub fn substitute(text: &str, ctx: &Ctx, now: i64) -> String {
    if text.is_empty() {
        return String::new();
    }
    VAR_RE
        .replace_all(text, |c: &Captures<'_>| {
            let Some(val) = ctx.get(&c[1]) else { return String::new() };
            let plain = || if val.is_null() { String::new() } else { py_str(val) };
            match c.get(2).map(|m| m.as_str()).filter(|s| "tTdDfFR".contains(s)) {
                Some(style) => match py_int(val).and_then(|i| human_time(i, style, now)) {
                    Some(s) => s,
                    None => plain(),
                },
                None => plain(),
            }
        })
        .into_owned()
}

/// `_strip_md`.
pub fn strip_md(s: &str) -> String {
    MD_LINK.replace_all(s, "$1").replace("**", "").replace(['`', '*'], "")
}

fn char_slice(s: &str, a: usize, b: usize) -> String {
    s.chars().skip(a).take(b - a).collect()
}

/// `_rgba(c, opacity)`; `Err` where `int(opacity * 255)` raises.
pub fn rgba(c: &str, opacity: f64) -> Result<Ink, Skip> {
    let mut s = strip(c).trim_start_matches('#').to_owned();
    if s.chars().count() == 3 {
        s = s.chars().flat_map(|ch| [ch, ch]).collect();
    }
    let hex = |a, b| kiwi_core::feeds::py::int(&char_slice(&s, a, b), 16);
    let (r, g, b) = match (hex(0, 2), hex(2, 4), hex(4, 6)) {
        (Some(r), Some(g), Some(b)) => (r, g, b),
        _ => (255, 255, 255),
    };
    let a = opacity * 255.0;
    if !a.is_finite() {
        return Err(Skip);
    }
    let clip = |v: i128| v.clamp(0, 255) as u8;
    Ok([clip(r), clip(g), clip(b), (a.trunc() as i128).clamp(0, 255) as u8])
}

fn floordiv(a: i64, b: i64) -> i64 {
    a.div_euclid(b)
}

/// `_gradient`.
pub fn gradient(w: usize, h: usize, c1: &str, c2: &str, angle: i64) -> Rgba {
    let diag = (((w * w + h * h) as f64).powf(0.5)) as usize + 2;
    let mask = ops::resize_l(&ops::linear_gradient(), diag, diag);
    let mask = ops::rotate_l(&mask, (angle as i128 - 90) as f64);
    let (left, top) = (floordiv(diag as i64 - w as i64, 2), floordiv(diag as i64 - h as i64, 2));
    let mask = ops::crop_l(&mask, left, top, left + w as i64, top + h as i64);
    let solid = |c: &str| Rgba::new(w, h, rgba(c, 1.0).unwrap_or([255; 4]));
    ops::composite(&solid(c2), &solid(c1), &mask)
}

/// `_fit_image`.
pub fn fit_image(src: &Rgba, w: usize, h: usize, fit: &str) -> Rgba {
    if fit == "stretch" {
        return ops::resize_rgba(src, w, h);
    }
    let (sw, sh) = (src.width as f64, src.height as f64);
    let (sx, sy) = (w as f64 / sw, h as f64 / sh);
    let scale = if fit == "cover" { py_max(sx, sy) } else { py_min(sx, sy) };
    let rw = ((sw * scale) as i64).max(1) as usize;
    let rh = ((sh * scale) as i64).max(1) as usize;
    let src = ops::resize_rgba(src, rw, rh);
    let mut canvas = Rgba::new(w, h, [0; 4]);
    ops::paste(&mut canvas, &src, floordiv(w as i64 - rw as i64, 2), floordiv(h as i64 - rh as i64, 2));
    canvas
}

fn py_max(a: f64, b: f64) -> f64 {
    if b > a { b } else { a }
}

fn py_min(a: f64, b: f64) -> f64 {
    if b < a { b } else { a }
}

/// `Image.open(BytesIO(data)).convert("RGBA")`, for the formats Rust opens.
fn open(data: &[u8]) -> Option<Rgba> {
    decode::png_to_rgba(data).ok()
}

fn draw_background(img: &mut Rgba, bg: &Background, blobs: &HashMap<String, Vec<u8>>) {
    let (w, h) = (img.width, img.height);
    if bg.kind == "gradient" {
        let g = gradient(w, h, &bg.color1, &bg.color2, bg.angle);
        ops::alpha_composite(img, &g, 0, 0);
        return;
    }
    if bg.kind == "image"
        && let Some(sha) = bg.image_sha.as_deref().filter(|s| !s.is_empty())
        && let Some(src) = blobs.get(sha).filter(|d| !d.is_empty()).and_then(|d| open(d))
    {
        let fitted = fit_image(&src, w, h, &bg.fit);
        ops::alpha_composite(img, &fitted, 0, 0);
        return;
    }
    let solid = Rgba::new(w, h, rgba(&bg.color1, 1.0).unwrap_or([255; 4]));
    ops::alpha_composite(img, &solid, 0, 0);
}

fn wrap(text: &str, font: &kiwi_pil::Font, max_w: i64) -> Result<Vec<String>, Skip> {
    let mut lines = Vec::new();
    for para in text.split('\n') {
        let mut cur = String::new();
        for word in para.split(' ') {
            let joined = format!("{cur} {word}");
            let trial = strip(&joined).to_owned();
            if !cur.is_empty() && textbbox((0.0, 0.0), &trial, font)?.2 > max_w as f64 {
                lines.push(std::mem::replace(&mut cur, word.to_owned()));
            } else {
                cur = trial;
            }
        }
        lines.push(cur);
    }
    Ok(lines)
}

fn draw_text(img: &mut Rgba, text_cache: &mut Text<'_>, layer: &Layer, ctx: &Ctx, now: i64) -> Result<(), Skip> {
    let text = strip_md(&substitute(&layer.text, ctx, now));
    if text.is_empty() {
        return Ok(());
    }
    let font = text_cache.font(layer.font_size.clamp(6, 300) as u32, layer.bold)?;
    let fill = rgba(&layer.color, layer.opacity.f())?;
    let lines: Vec<String> = match layer.max_width.filter(|&m| m != 0) {
        Some(m) => wrap(&text, font, m)?,
        None => text.split('\n').map(str::to_owned).collect(),
    };
    let line_h = textbbox((0.0, 0.0), "Ag", font)?.3 + 4.0;
    for (i, line) in lines.iter().enumerate() {
        let y = layer.y.f() + i as f64 * line_h;
        let w = textbbox((0.0, 0.0), line, font)?.2;
        let x = match layer.align.as_str() {
            "center" => layer.x.f() - w / 2.0,
            "right" => layer.x.f() - w,
            _ => layer.x.f(),
        };
        if !x.is_finite() || !y.is_finite() {
            return Err(Skip);
        }
        Draw::new(img).text((x, y), line, font, fill)?;
    }
    Ok(())
}

fn trunc_int(v: f64) -> Result<i64, Skip> {
    if v.is_finite() { Ok(v.trunc() as i64) } else { Err(Skip) }
}

fn draw_rect(img: &mut Rgba, layer: &Layer) -> Result<(), Skip> {
    let (x0, y0) = (layer.x.f(), layer.y.f());
    let (w, h) = (layer.w.f(), layer.h.f());
    let xy = [x0, y0, x0 + w, y0 + h];
    let fill = rgba(&layer.color, layer.opacity.f())?;
    let mut d = Draw::new(img);
    if layer.radius > 0 {
        let r = layer.radius.min(trunc_int(py_min(w, h) / 2.0)?);
        d.rounded_rectangle(xy, r as f64, Some(fill), None, 1)?;
    } else {
        d.rectangle(xy, Some(fill), None, 1)?;
    }
    Ok(())
}

/// The largest layer Rust resizes to (Pillow's bomb ceiling).
const MAX_LAYER_PIXELS: i64 = 178_956_970;

fn draw_image_layer(img: &mut Rgba, layer: &Layer, blobs: &HashMap<String, Vec<u8>>) -> Result<(), Skip> {
    let Some(sha) = layer.image_sha.as_deref().filter(|s| !s.is_empty()) else { return Ok(()) };
    let Some(data) = blobs.get(sha).filter(|d| !d.is_empty()) else { return Ok(()) };
    let Some(src) = open(data) else { return Ok(()) };
    let w = trunc_int(layer.w.f())?.max(1);
    let h = trunc_int(layer.h.f())?.max(1);
    if w.saturating_mul(h) > MAX_LAYER_PIXELS {
        return Err(Skip);
    }
    let (w, h) = (w as usize, h as usize);
    let mut src = ops::resize_rgba(&src, w, h);
    let op = layer.opacity.f();
    let scaled = |m: &Mask| -> Result<Mask, Skip> {
        if op == f64::NEG_INFINITY {
            return Err(Skip);
        }
        Ok(ops::point(m, |a| (a as f64 * op) as i64))
    };
    if op < 1.0 {
        let a = scaled(&ops::alpha(&src))?;
        ops::put_alpha(&mut src, &a);
    }
    let (x, y) = (trunc_int(layer.x.f())?, trunc_int(layer.y.f())?);
    if layer.radius > 0 {
        let mut canvas = Rgba::new(w, h, [0; 4]);
        let r = layer.radius.min((w.min(h) / 2) as i64);
        Draw::new(&mut canvas).rounded_rectangle(
            [0.0, 0.0, (w - 1) as f64, (h - 1) as f64],
            r as f64,
            Some([255; 4]),
            None,
            1,
        )?;
        let mut mask = Mask { width: w, height: h, data: canvas.data.iter().map(|p| p[0]).collect() };
        if op < 1.0 {
            mask = scaled(&mask)?;
        }
        ops::paste_masked(img, &src, &mask, x, y);
    } else {
        ops::alpha_composite(img, &src, x, y);
    }
    Ok(())
}

/// The drawable parts of a design.
pub struct Scene<'a> {
    pub width: i64,
    pub height: i64,
    pub background: &'a Background,
    pub layers: &'a [Layer],
}

/// `render(design, ctx, blob_get, now=now)` before the PNG encode.
pub fn draw(
    lib: &Library,
    fonts: &Fonts,
    scene: &Scene<'_>,
    ctx: &Ctx,
    blobs: &HashMap<String, Vec<u8>>,
    now: i64,
) -> Rgba {
    let w = scene.width.clamp(MIN_DIM, MAX_DIM) as usize;
    let h = scene.height.clamp(MIN_DIM, MAX_DIM) as usize;
    let mut img = Rgba::new(w, h, [0; 4]);
    draw_background(&mut img, scene.background, blobs);
    let mut text = Text::new(lib, fonts);
    for layer in scene.layers.iter().take(MAX_LAYERS) {
        let _ = match layer.kind.as_str() {
            "text" => draw_text(&mut img, &mut text, layer, ctx, now),
            "rect" => draw_rect(&mut img, layer),
            "image" => draw_image_layer(&mut img, layer, blobs),
            _ => Ok(()),
        };
    }
    img
}

/// The PNG `img.save(out, "PNG")` writes.
pub fn encode(img: &Rgba) -> Vec<u8> {
    let raw: Vec<u8> = img.data.iter().flatten().copied().collect();
    kiwi_pil::png::encode_rgba(&raw, img.width, img.height)
}

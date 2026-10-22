//! `ImageDraw.Draw(rgba_image, "RGBA")`: the Python-level logic of `ImageDraw.py`
//! and `ImageText.py` over the `Draw.c` primitives.

use crate::{
    ImageError,
    font::Font,
    image::{Ink, Rgba},
};

/// Python's float `%`.
pub fn py_mod(x: f64, y: f64) -> f64 {
    let m = x % y;
    if m != 0.0 { if (y < 0.0) != (m < 0.0) { m + y } else { m } } else { 0.0f64.copysign(y) }
}

fn ordered(xy: [f64; 4]) -> Result<[i32; 4], ImageError> {
    if xy[2] < xy[0] {
        return Err(ImageError::Value("x1 must be greater than or equal to x0"));
    }
    if xy[3] < xy[1] {
        return Err(ImageError::Value("y1 must be greater than or equal to y0"));
    }
    Ok(xy.map(|v| v as i32))
}

/// `_getink(outline, fill)`: with neither given, the outline is the image's
/// default ink (`draw_ink(-1)`, opaque white on RGBA).
fn getink(outline: Option<Ink>, fill: Option<Ink>) -> (Option<Ink>, Option<Ink>) {
    if outline.is_none() && fill.is_none() { (Some([255; 4]), None) } else { (outline, fill) }
}

/// `start`/`end` cross into C as `float`.
fn f(v: f64) -> f32 {
    v as f32
}

pub struct Draw<'i> {
    pub im: &'i mut Rgba,
}

impl<'i> Draw<'i> {
    pub fn new(im: &'i mut Rgba) -> Self {
        Draw { im }
    }

    pub fn rectangle(
        &mut self,
        xy: [f64; 4],
        fill: Option<Ink>,
        outline: Option<Ink>,
        width: i32,
    ) -> Result<(), ImageError> {
        let (outline, fill) = getink(outline, fill);
        let [x0, y0, x1, y1] = ordered(xy)?;
        if let Some(c) = fill {
            self.im.draw_rectangle(x0, y0, x1, y1, c, true, 1);
        }
        if let Some(c) = outline
            && Some(c) != fill
            && width != 0
        {
            self.im.draw_rectangle(x0, y0, x1, y1, c, false, width);
        }
        Ok(())
    }

    pub fn ellipse(
        &mut self,
        xy: [f64; 4],
        fill: Option<Ink>,
        outline: Option<Ink>,
        width: i32,
    ) -> Result<(), ImageError> {
        let (outline, fill) = getink(outline, fill);
        let [x0, y0, x1, y1] = ordered(xy)?;
        if let Some(c) = fill {
            self.im.draw_ellipse(x0, y0, x1, y1, c, true, 1);
        }
        if let Some(c) = outline
            && Some(c) != fill
            && width != 0
        {
            self.im.draw_ellipse(x0, y0, x1, y1, c, false, width);
        }
        Ok(())
    }

    /// `polygon(xy, fill=...)`.
    pub fn polygon(&mut self, xy: &[(f64, f64)], fill: Ink) -> Result<(), ImageError> {
        if xy.len() < 2 {
            return Err(ImageError::Value("coordinate list must contain at least 2 coordinates"));
        }
        let pts: Vec<(i32, i32)> = xy.iter().map(|&(x, y)| (x as i32, y as i32)).collect();
        self.im.draw_polygon_fill(&pts, fill);
        Ok(())
    }

    pub fn pieslice(&mut self, xy: [f64; 4], start: f64, end: f64, fill: Ink) -> Result<(), ImageError> {
        let [x0, y0, x1, y1] = ordered(xy)?;
        self.im.draw_pieslice_fill(x0, y0, x1, y1, f(start), f(end), fill);
        Ok(())
    }

    /// `line(xy, fill, width, joint)`.
    pub fn line(&mut self, xy: &[(f64, f64)], fill: Ink, width: i32, joint_curve: bool) -> Result<(), ImageError> {
        if width == 0 {
            return Ok(());
        }
        self.im.draw_lines(xy, fill, width);
        if !(joint_curve && width > 4) {
            return Ok(());
        }
        let w = width as f64;
        for i in 1..xy.len().saturating_sub(1) {
            let point = xy[i];
            let ang = |s: (f64, f64), e: (f64, f64)| py_mod((e.0 - s.0).atan2(s.1 - e.1).to_degrees(), 360.0);
            let a0 = ang(xy[i - 1], point);
            let a1 = ang(point, xy[i + 1]);
            if a0 == a1 {
                continue;
            }
            let flipped = (a1 > a0 && a1 - 180.0 > a0) || (a1 < a0 && a1 + 180.0 > a0);
            let coords =
                [point.0 - w / 2.0 + 1.0, point.1 - w / 2.0 + 1.0, point.0 + w / 2.0 - 1.0, point.1 + w / 2.0 - 1.0];
            let (start, end) = if flipped { (a1 + 90.0, a0 + 90.0) } else { (a0 - 90.0, a1 - 90.0) };
            self.pieslice(coords, start - 90.0, end - 90.0, fill)?;
            if width > 8 {
                let at = |c: (f64, f64), angle: f64| {
                    let angle = angle - 90.0;
                    let distance = w / 2.0 - 1.0;
                    let step = |p: f64, d: f64| p + if d > 0.0 { d.floor() } else { d.ceil() };
                    (step(c.0, distance * angle.to_radians().cos()), step(c.1, distance * angle.to_radians().sin()))
                };
                let gap = if flipped {
                    [at(point, a0 + 90.0), point, at(point, a1 + 90.0)]
                } else {
                    [at(point, a0 - 90.0), point, at(point, a1 - 90.0)]
                };
                self.line(&gap, fill, 3, false)?;
            }
        }
        Ok(())
    }

    /// `rounded_rectangle(xy, radius, fill, outline, width)` with all corners.
    pub fn rounded_rectangle(
        &mut self,
        xy: [f64; 4],
        radius: f64,
        fill: Option<Ink>,
        outline: Option<Ink>,
        width: i32,
    ) -> Result<(), ImageError> {
        let [x0, y0, x1, y1] = xy;
        if x1 < x0 {
            return Err(ImageError::Value("x1 must be greater than or equal to x0"));
        }
        if y1 < y0 {
            return Err(ImageError::Value("y1 must be greater than or equal to y0"));
        }
        let mut d = (x1 - x0).min(y1 - y0).min(radius * 2.0);
        let (x0, y0, x1, y1) = (x0.round_ties_even(), y0.round_ties_even(), x1.round_ties_even(), y1.round_ties_even());
        let full_x = d >= x1 - x0 - 1.0;
        if full_x {
            d = x1 - x0;
        }
        let full_y = d >= y1 - y0 - 1.0;
        if full_y {
            d = y1 - y0;
        }
        if full_x && full_y {
            return self.ellipse(xy, fill, outline, width);
        }
        if d == 0.0 {
            return self.rectangle(xy, fill, outline, width);
        }
        let r = (d / 2.0).floor();
        let (outline, fill) = getink(outline, fill);
        let parts: Vec<([f64; 4], f64, f64)> = if full_x {
            vec![([x0, y0, x0 + d, y0 + d], 180.0, 360.0), ([x0, y1 - d, x0 + d, y1], 0.0, 180.0)]
        } else if full_y {
            vec![([x0, y0, x0 + d, y0 + d], 90.0, 270.0), ([x1 - d, y0, x1, y0 + d], 270.0, 90.0)]
        } else {
            vec![
                ([x0, y0, x0 + d, y0 + d], 180.0, 270.0),
                ([x1 - d, y0, x1, y0 + d], 270.0, 360.0),
                ([x1 - d, y1 - d, x1, y1], 0.0, 90.0),
                ([x0, y1 - d, x0 + d, y1], 90.0, 180.0),
            ]
        };
        let rect = |im: &mut Rgba, b: [f64; 4], c: Ink| -> Result<(), ImageError> {
            let [a0, b0, a1, b1] = ordered(b)?;
            im.draw_rectangle(a0, b0, a1, b1, c, true, 1);
            Ok(())
        };
        if let Some(c) = fill {
            for (b, s, e) in &parts {
                self.pieslice(*b, *s, *e, c)?;
            }
            if full_x {
                rect(self.im, [x0, y0 + r + 1.0, x1, y1 - r - 1.0], c)?;
            } else if x1 - r - 1.0 >= x0 + r + 1.0 {
                rect(self.im, [x0 + r + 1.0, y0, x1 - r - 1.0, y1], c)?;
            }
            if !full_x && !full_y {
                rect(self.im, [x0, y0 + r + 1.0, x0 + r, y1 - r - 1.0], c)?;
                rect(self.im, [x1 - r, y0 + r + 1.0, x1, y1 - r - 1.0], c)?;
            }
        }
        if let Some(c) = outline
            && Some(c) != fill
            && width != 0
        {
            for (b, s, e) in &parts {
                let [a0, b0, a1, b1] = ordered(*b)?;
                self.im.draw_arc(a0, b0, a1, b1, f(*s), f(*e), c, width);
            }
            let wf = width as f64;
            if !full_x {
                rect(self.im, [x0 + r + 1.0, y0, x1 - r - 1.0, y0 + wf - 1.0], c)?;
                rect(self.im, [x0 + r + 1.0, y1 - wf + 1.0, x1 - r - 1.0, y1], c)?;
            }
            if !full_y {
                rect(self.im, [x0, y0 + r + 1.0, x0 + wf - 1.0, y1 - r - 1.0], c)?;
                rect(self.im, [x1 - wf + 1.0, y0 + r + 1.0, x1, y1 - r - 1.0], c)?;
            }
        }
        Ok(())
    }

    pub fn textbbox(&self, xy: (f64, f64), text: &str, font: &Font) -> Result<(f64, f64, f64, f64), ImageError> {
        textbbox(xy, text, font)
    }

    /// `text(xy, text, fill=..., font=...)`.
    pub fn text(&mut self, xy: (f64, f64), text: &str, font: &mut Font, fill: Ink) -> Result<(), ImageError> {
        for (lx, ly, line) in split_lines(xy, text, font)? {
            let (x, y) = (lx.trunc(), ly.trunc());
            let start = ((lx - x) as f32, (ly - y) as f32);
            let (mask, (ox, oy)) = font.render(line, start)?;
            self.im.draw_bitmap((x as i64 + ox) as i32, (y as i64 + oy) as i32, &mask, fill);
        }
        Ok(())
    }
}

/// `draw.textbbox(xy, text, font)` with the default "la" anchor and left align.
pub fn textbbox(xy: (f64, f64), text: &str, font: &Font) -> Result<(f64, f64, f64, f64), ImageError> {
    let mut bbox: Option<(f64, f64, f64, f64)> = None;
    for (x, y, line) in split_lines(xy, text, font)? {
        let (l, t, r, b) = font.getsize(line)?.bbox();
        let lb = (l as f64 + x, t as f64 + y, r as f64 + x, b as f64 + y);
        bbox = Some(match bbox {
            None => lb,
            Some(o) => (o.0.min(lb.0), o.1.min(lb.1), o.2.max(lb.2), o.3.max(lb.3)),
        });
    }
    Ok(bbox.unwrap_or((xy.0, xy.1, xy.0, xy.1)))
}

/// `ImageText.Text._split` for anchor "la", align "left", spacing 4.
fn split_lines<'t>(xy: (f64, f64), text: &'t str, font: &Font) -> Result<Vec<(f64, f64, &'t str)>, ImageError> {
    let lines: Vec<&str> = text.split('\n').collect();
    if lines.len() == 1 {
        return Ok(vec![(xy.0, xy.1, text)]);
    }
    let (_, _, _, bottom) = font.getsize("A")?.bbox();
    let spacing = bottom as f64 + 4.0;
    let mut top = xy.1;
    let mut out = Vec::with_capacity(lines.len());
    for line in lines {
        out.push((xy.0, top, line));
        top += spacing;
    }
    Ok(out)
}

//! `ImageFont.FreeTypeFont` with Pillow's basic layout (`_imagingft.c`): what a
//! Pillow without libraqm (the production container: no libfribidi) draws.

use crate::{
    ft::{FT_PIXEL_MODE_GRAY, Face, FtError, Library},
    image::Mask,
};

const FT_LOAD_DEFAULT: i32 = 0;

/// `PIXEL()`: 26.6 to whole pixels, rounding half up.
fn pixel(x: i64) -> i64 {
    ((x + 32) & -64) >> 6
}

#[derive(Clone, Copy)]
struct GlyphInfo {
    index: u32,
    x_advance: i64,
}

pub struct Font<'a> {
    face: Face<'a>,
    kerning: bool,
    ascender: i64,
}

/// `font.getsize()` for horizontal text anchored "la".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextBox {
    pub width: i64,
    pub height: i64,
    pub x_offset: i64,
    pub y_offset: i64,
}

impl TextBox {
    /// `getbbox()` / single-line `textbbox((0, 0))`: (left, top, right, bottom).
    pub fn bbox(&self) -> (i64, i64, i64, i64) {
        (self.x_offset, self.y_offset, self.x_offset + self.width, self.y_offset + self.height)
    }
}

impl<'a> Font<'a> {
    /// `ImageFont.truetype(font_bytes, size)` (face index 0).
    pub fn new(lib: &'a Library, data: &'a [u8], size: f32) -> Result<Self, FtError> {
        Ok(Self::from_face(Face::new(lib, data, 0, size)?))
    }

    /// `ImageFont.truetype(path, size)` (face index 0, read from disk).
    pub fn open(lib: &'a Library, path: &std::path::Path, size: f32) -> Result<Self, FtError> {
        Ok(Self::from_face(Face::open(lib, path, 0, size)?))
    }

    fn from_face(face: Face<'a>) -> Self {
        let kerning = face.has_kerning();
        let (ascender, _) = face.size_metrics();
        Font { face, kerning, ascender }
    }

    /// `text_layout_fallback`, including its kerning quirk: the kerning delta is
    /// rounded to whole pixels and added to the previous advance as 26.6 units.
    fn layout(&self, text: &str) -> Result<Vec<GlyphInfo>, FtError> {
        let mut out: Vec<GlyphInfo> = Vec::with_capacity(text.len());
        let mut last_index = 0u32;
        for ch in text.chars() {
            let index = self.face.char_index(ch as u32);
            let advance = self.face.load_advance(index, FT_LOAD_DEFAULT)?;
            if self.kerning
                && last_index != 0
                && index != 0
                && let Ok((dx, _)) = self.face.kerning(last_index, index)
                && let Some(prev) = out.last_mut()
            {
                prev.x_advance += pixel(dx);
            }
            out.push(GlyphInfo { index, x_advance: advance });
            last_index = index;
        }
        Ok(out)
    }

    /// `bounding_box_and_anchors` for horizontal text and anchor "la".
    fn bounding_box(&self, glyphs: &[GlyphInfo]) -> Result<TextBox, FtError> {
        let (mut position, mut x_min, mut x_max, mut y_min, mut y_max) = (0i64, 0i64, 0i64, 0i64, 0i64);
        for g in glyphs {
            let px = pixel(position);
            position += g.x_advance;
            let advanced = pixel(position);
            if advanced > x_max {
                x_max = advanced;
            }
            let [bx0, by0, bx1, by1] = self.face.cbox(g.index, FT_LOAD_DEFAULT)?;
            if bx1 + px > x_max {
                x_max = bx1 + px;
            }
            if bx0 + px < x_min {
                x_min = bx0 + px;
            }
            if by1 > y_max {
                y_max = by1;
            }
            if by0 < y_min {
                y_min = by0;
            }
        }
        let y_anchor = if glyphs.is_empty() { 0 } else { pixel(self.ascender) };
        Ok(TextBox { width: x_max - x_min, height: y_max - y_min, x_offset: x_min, y_offset: y_anchor - y_max })
    }

    /// `font.getsize(text, "L")`.
    pub fn getsize(&self, text: &str) -> Result<TextBox, FtError> {
        let glyphs = self.layout(text)?;
        self.bounding_box(&glyphs)
    }

    /// `font.getmask2(text, "L", start=start)`: the "L" mask and its offset.
    pub fn render(&mut self, text: &str, start: (f32, f32)) -> Result<(Mask, (i64, i64)), FtError> {
        let glyphs = self.layout(text)?;
        let bx = self.bounding_box(&glyphs)?;
        let width = (bx.width as f64 + (start.0 as f64).ceil()) as i64;
        let height = (bx.height as f64 + (start.1 as f64).ceil()) as i64;
        let mut mask = Mask::new(width.max(0) as usize, height.max(0) as usize);
        let offset = (bx.x_offset, bx.y_offset);
        if glyphs.is_empty() || width == 0 || height == 0 {
            return Ok((mask, offset));
        }

        let (mut x, mut x_min, mut y_max) = (0i64, 0i64, 0i64);
        for g in &glyphs {
            let px = pixel(x);
            let bm = self.face.render(g.index, FT_LOAD_DEFAULT)?;
            if bm.top as i64 > y_max {
                y_max = bm.top as i64;
            }
            if bm.left as i64 + px < x_min {
                x_min = bm.left as i64 + px;
            }
            x += g.x_advance;
        }

        let mut x = (((-x_min) as f32 + 0.0f32 + start.0) * 64.0f32) as f64;
        x = x.round();
        let y = ((((-y_max) as f32 + -0.0f32 - start.1) * 64.0f32) as f64).round() as i64;
        let mut x = x as i64;
        let (mw, mh) = (mask.width as i64, mask.height as i64);
        for g in &glyphs {
            let px = pixel(x);
            let py = pixel(y);
            let bm = self.face.render(g.index, FT_LOAD_DEFAULT)?;
            let xx = px + bm.left as i64;
            let mut yy = -(py + bm.top as i64);
            if bm.has_buffer() {
                if bm.pixel_mode != FT_PIXEL_MODE_GRAY {
                    return Err(FtError(-2));
                }
                let mut x0 = 0i64;
                let mut x1 = bm.width as i64;
                if xx < 0 {
                    x0 = -xx;
                }
                if xx + x1 > mw {
                    x1 = mw - xx;
                }
                for by in 0..bm.rows {
                    if yy >= 0 && yy < mh {
                        let src = bm.row(by);
                        let row = mask.row_mut(yy as usize);
                        for k in x0..x1 {
                            let a = src[k as usize] as u32 * bm.scale;
                            if a > 0 {
                                let t = &mut row[(xx + k) as usize];
                                *t = if *t > 0 { clip8(a + muldiv255(*t as u32, 255 - a)) } else { a as u8 };
                            }
                        }
                    }
                    yy += 1;
                }
            }
            x += g.x_advance;
        }
        Ok((mask, offset))
    }
}

pub(crate) fn muldiv255(a: u32, b: u32) -> u32 {
    let tmp = a * b + 128;
    ((tmp >> 8) + tmp) >> 8
}

fn clip8(v: u32) -> u8 {
    v.min(255) as u8
}

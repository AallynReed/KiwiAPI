//! `og_image.py`'s font and text helpers: `_font`, `_w`, `_truncate_px`,
//! `_fit_font`, `_fit_text`, over kiwi-pil.

use std::sync::OnceLock;

use kiwi_pil::{Draw, Font, ImageError, Ink, Library, draw::textbbox};

/// `_REG` / `_BOLD`: the first candidate is what production loads.
const REGULAR: &str = "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf";
const BOLD: &str = "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf";

pub struct Fonts {
    pub regular: Vec<u8>,
    pub bold: Vec<u8>,
}

static FONTS: OnceLock<Option<Fonts>> = OnceLock::new();

/// DejaVu as production has it, or `None` (the routes then stay in Python,
/// whose fallback chain would pick another face).
pub fn fonts() -> Option<&'static Fonts> {
    FONTS
        .get_or_init(|| {
            let regular = std::fs::read(REGULAR).ok()?;
            let bold = std::fs::read(BOLD).ok()?;
            Some(Fonts { regular, bold })
        })
        .as_ref()
}

/// Thousands separators: Python's `f"{n:,}"`.
pub fn thousands(n: i64) -> String {
    let digits = n.unsigned_abs().to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3 + 1);
    if n < 0 {
        out.push('-');
    }
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// One render's FreeType state: fonts are opened once per (weight, size).
pub struct Text<'a> {
    lib: &'a Library,
    fonts: &'a Fonts,
    cache: Vec<((bool, u32), Font<'a>)>,
}

impl<'a> Text<'a> {
    pub fn new(lib: &'a Library, fonts: &'a Fonts) -> Self {
        Text { lib, fonts, cache: Vec::new() }
    }

    /// `_font(size, bold)`.
    pub fn font(&mut self, size: u32, bold: bool) -> Result<&mut Font<'a>, ImageError> {
        let key = (bold, size);
        let i = match self.cache.iter().position(|(k, _)| *k == key) {
            Some(i) => i,
            None => {
                let data = if bold { &self.fonts.bold } else { &self.fonts.regular };
                self.cache.push((key, Font::new(self.lib, data, size as f32)?));
                self.cache.len() - 1
            }
        };
        Ok(&mut self.cache[i].1)
    }

    /// `_w`: the `textbbox` width.
    pub fn w(&mut self, text: &str, size: u32, bold: bool) -> Result<i64, ImageError> {
        let f = self.font(size, bold)?;
        let (l, _, r, _) = textbbox((0.0, 0.0), text, f)?;
        Ok((r - l) as i64)
    }

    /// `_truncate_px`.
    pub fn truncate_px(&mut self, text: &str, size: u32, bold: bool, max_w: i64) -> Result<String, ImageError> {
        if text.is_empty() || self.w(text, size, bold)? <= max_w {
            return Ok(text.to_owned());
        }
        let mut chars: Vec<char> = text.chars().collect();
        while !chars.is_empty() {
            let trial: String = chars.iter().chain(std::iter::once(&'…')).collect();
            if self.w(&trial, size, bold)? <= max_w {
                break;
            }
            chars.pop();
        }
        let mut out: String = chars.into_iter().collect();
        out.push('…');
        Ok(out)
    }

    /// `_fit_font`: the size it settles on.
    pub fn fit_size(
        &mut self,
        text: &str,
        max_w: i64,
        base: u32,
        bold: bool,
        min_size: Option<u32>,
    ) -> Result<u32, ImageError> {
        let min_size = min_size.unwrap_or_else(|| 11.max((base as f64 * 0.55) as u32));
        let mut size = base;
        while size > min_size && self.w(text, size, bold)? > max_w {
            size -= 2;
        }
        Ok(size)
    }

    /// `d.text(xy, text, font=_font(size, bold), fill=fill)`.
    pub fn draw(
        &mut self,
        d: &mut Draw,
        xy: (f64, f64),
        text: &str,
        size: u32,
        bold: bool,
        fill: Ink,
    ) -> Result<(), ImageError> {
        let f = self.font(size, bold)?;
        d.text(xy, text, f, fill)
    }

    /// `_fit_text`: draws `text` shrunk (then truncated) to `max_w`; `right`
    /// anchors the right edge at `xy.0`. Returns the drawn width.
    #[allow(clippy::too_many_arguments)]
    pub fn fit_text(
        &mut self,
        d: &mut Draw,
        xy: (f64, f64),
        text: &str,
        max_w: i64,
        base: u32,
        fill: Ink,
        bold: bool,
        right: bool,
        min_size: Option<u32>,
    ) -> Result<i64, ImageError> {
        let size = self.fit_size(text, max_w, base, bold, min_size)?;
        let shown = self.truncate_px(text, size, bold, max_w)?;
        let w = self.w(&shown, size, bold)?;
        let x = if right { xy.0 - w as f64 } else { xy.0 };
        self.draw(d, (x, xy.1), &shown, size, bold, fill)?;
        Ok(w)
    }
}

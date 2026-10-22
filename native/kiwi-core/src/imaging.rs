//! RGBA8 images as Pillow handles them: PNG decode + `convert("RGBA")`,
//! `Image.alpha_composite`, and PNG encode (same pixels, not the same bytes).

use std::io::Cursor;

use png::{BitDepth, ColorType, Transformations};

/// Straight (non-premultiplied) RGBA, 8 bits per channel, row-major.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rgba {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

/// `Image.open(png).convert("RGBA")`: palettes, tRNS and low bit depths
/// expanded, 16-bit samples cut to their high byte.
pub fn decode_png(bytes: &[u8]) -> Result<Rgba, String> {
    let mut decoder = png::Decoder::new(Cursor::new(bytes));
    decoder.set_transformations(Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().map_err(|e| e.to_string())?;
    let size = reader.output_buffer_size().ok_or("image too large")?;
    let mut buf = vec![0; size];
    let info = reader.next_frame(&mut buf).map_err(|e| e.to_string())?;
    if info.bit_depth != BitDepth::Eight {
        return Err(format!("unexpected bit depth {:?}", info.bit_depth));
    }
    let (w, h) = (info.width as usize, info.height as usize);
    let channels = info.color_type.samples();
    let mut pixels = Vec::with_capacity(w * h * 4);
    for row in buf.chunks(info.line_size).take(h) {
        for px in row[..w * channels].chunks_exact(channels) {
            pixels.extend_from_slice(&match info.color_type {
                ColorType::Grayscale => [px[0], px[0], px[0], 255],
                ColorType::GrayscaleAlpha => [px[0], px[0], px[0], px[1]],
                ColorType::Rgb => [px[0], px[1], px[2], 255],
                ColorType::Rgba => [px[0], px[1], px[2], px[3]],
                ColorType::Indexed => return Err("palette not expanded".into()),
            });
        }
    }
    Ok(Rgba { width: info.width, height: info.height, pixels })
}

/// `(a + (a >> 8)) >> 8`, Pillow's rounding divide by 255.
fn shift_div255(a: u32) -> u32 {
    ((a >> 8) + a) >> 8
}

const PRECISION_BITS: u32 = 7;

/// `Image.alpha_composite(dst, src)` (libImaging `AlphaComposite.c`).
pub fn alpha_composite(dst: &Rgba, src: &Rgba) -> Result<Rgba, String> {
    if (dst.width, dst.height) != (src.width, src.height) || dst.pixels.len() != src.pixels.len() {
        return Err("images do not match".into());
    }
    let mut pixels = Vec::with_capacity(dst.pixels.len());
    for (d, s) in dst.pixels.chunks_exact(4).zip(src.pixels.chunks_exact(4)) {
        let sa = s[3] as u32;
        if sa == 0 {
            pixels.extend_from_slice(d);
            continue;
        }
        let blend = d[3] as u32 * (255 - sa);
        let outa255 = sa * 255 + blend;
        let coef1 = sa * 255 * 255 * (1 << PRECISION_BITS) / outa255;
        let coef2 = 255 * (1 << PRECISION_BITS) - coef1;
        for c in 0..3 {
            let tmp = s[c] as u32 * coef1 + d[c] as u32 * coef2;
            pixels.push((shift_div255(tmp + (0x80 << PRECISION_BITS)) >> PRECISION_BITS) as u8);
        }
        pixels.push(shift_div255(outa255 + 0x80) as u8);
    }
    Ok(Rgba { width: dst.width, height: dst.height, pixels })
}

/// An 8-bit RGBA PNG.
pub fn encode_png(img: &Rgba) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, img.width, img.height);
    enc.set_color(ColorType::Rgba);
    enc.set_depth(BitDepth::Eight);
    let mut writer = enc.write_header().map_err(|e| e.to_string())?;
    writer.write_image_data(&img.pixels).map_err(|e| e.to_string())?;
    writer.finish().map_err(|e| e.to_string())?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_round_trip() {
        let img = Rgba { width: 3, height: 2, pixels: (0..24).map(|i| (i * 11) as u8).collect() };
        assert_eq!(decode_png(&encode_png(&img).unwrap()).unwrap(), img);
    }

    #[test]
    fn composite_edges() {
        let px = |p: [u8; 4]| Rgba { width: 1, height: 1, pixels: p.to_vec() };
        let c = |d, s| alpha_composite(&px(d), &px(s)).unwrap().pixels;
        assert_eq!(c([1, 2, 3, 4], [9, 9, 9, 0]), [1, 2, 3, 4]);
        assert_eq!(c([1, 2, 3, 4], [9, 8, 7, 255]), [9, 8, 7, 255]);
        assert_eq!(c([0, 0, 0, 0], [200, 100, 50, 128]), [200, 100, 50, 128]);
        assert!(alpha_composite(&px([0; 4]), &Rgba { width: 2, height: 1, pixels: vec![0; 8] }).is_err());
    }
}

//! A GIF's first frame as `GifImagePlugin` loads it: "P" (or "L" when every
//! palette is a plain grey ramp) on a canvas the size of the screen grown to
//! the frame, prefilled with the transparent index, else index 0.

use super::{Mode, Picture, Trns};

fn palette_needed(p: &[u8]) -> bool {
    p.chunks(3).enumerate().any(|(i, c)| c.len() < 3 || !(c[0] as usize == i && c[1] == c[0] && c[2] == c[0]))
}

pub fn decode(b: &[u8], max_pixels: u64) -> Option<Picture> {
    let mut opts = ::gif::DecodeOptions::new();
    opts.set_color_output(::gif::ColorOutput::Indexed);
    opts.check_frame_consistency(false);
    let mut dec = opts.read_info(std::io::Cursor::new(b)).ok()?;
    let (sw, sh) = (usize::from(dec.width()), usize::from(dec.height()));
    let global = dec.global_palette().filter(|p| palette_needed(p)).map(<[u8]>::to_vec);
    let frame = dec.read_next_frame().ok()??.clone();
    let (x0, y0) = (usize::from(frame.left), usize::from(frame.top));
    let (fw, fh) = (usize::from(frame.width), usize::from(frame.height));
    let (w, h) = (sw.max(x0 + fw), sh.max(y0 + fh));
    if w == 0 || h == 0 || (w as u64) * (h as u64) > max_pixels {
        return None;
    }
    let palette = match &frame.palette {
        Some(p) if palette_needed(p) => Some(p.clone()),
        Some(_) => None,
        None => global,
    };
    let fill = frame.transparent.unwrap_or(0);
    let mut data = vec![fill; w * h];
    for y in 0..fh {
        let row = frame.buffer.get(y * fw..(y + 1) * fw)?;
        data[(y0 + y) * w + x0..(y0 + y) * w + x0 + fw].copy_from_slice(row);
    }
    let trns = frame.transparent.map_or(Trns::None, |t| Trns::Value(u32::from(t)));
    Some(Picture {
        mode: if palette.is_some() { Mode::P } else { Mode::L },
        width: w,
        height: h,
        data,
        icc: None,
        palette: palette.unwrap_or_default(),
        trns,
    })
}

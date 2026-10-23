//! `Image.open(jpeg).load()` as Pillow 13 runs it: `JpegImagePlugin._open`'s
//! marker walk (which decides the mode, the size, the ICC profile and what it
//! refuses), then libjpeg-turbo's decoder fed through Pillow's suspending
//! source, so a stream that runs out of data fails instead of being padded.

use std::{
    ffi::{c_int, c_long},
    panic::{AssertUnwindSafe, catch_unwind},
};

use mozjpeg_sys::*;

use super::{Mode, Picture};

pub struct Header {
    pub width: u32,
    pub height: u32,
    pub mode: Mode,
    pub icc: Option<Vec<u8>>,
}

fn be16(b: &[u8], at: usize) -> Option<usize> {
    Some(usize::from(u16::from_be_bytes([*b.get(at)?, *b.get(at + 1)?])))
}

struct Reader<'a> {
    b: &'a [u8],
    p: usize,
}

impl<'a> Reader<'a> {
    fn read(&mut self, n: usize) -> &'a [u8] {
        let end = (self.p + n).min(self.b.len());
        let s = &self.b[self.p..end];
        self.p = end;
        s
    }

    /// `i16(fp.read(2)) - 2` then `_safe_read(fp, n)`; None where Python raises.
    fn segment(&mut self) -> Option<&'a [u8]> {
        let len = be16(self.read(2), 0)? as isize - 2;
        if len <= 0 {
            return Some(&[]);
        }
        let s = self.read(len as usize);
        (s.len() == len as usize).then_some(s)
    }
}

fn app(h: &mut Partial, marker: u8, s: &[u8]) -> Option<()> {
    match marker {
        0xE0 if s.starts_with(b"JFIF") => {
            be16(s, 5)?;
        }
        0xE2 if s.starts_with(b"ICC_PROFILE\0") => h.icclist.push(s.to_vec()),
        0xED if s.starts_with(b"Photoshop 3.0\0") => {
            let mut off = 14;
            while s.get(off..off + 4) == Some(b"8BIM") {
                off += 4;
                if off + 2 > s.len() {
                    return Some(());
                }
                off += 2;
                let name_len = usize::from(*s.get(off)?);
                off += 1 + name_len;
                off += off & 1;
                if off + 4 > s.len() {
                    return Some(());
                }
                let size = u32::from_be_bytes([s[off], s[off + 1], s[off + 2], s[off + 3]]) as usize;
                off += 4;
                off += size;
                off += off & 1;
            }
        }
        0xEE if s.starts_with(b"Adobe") => {
            be16(s, 5)?;
        }
        _ => {}
    }
    Some(())
}

#[derive(Default)]
struct Partial {
    size: Option<(u32, u32)>,
    mode: Option<Mode>,
    icclist: Vec<Vec<u8>>,
    icc: Option<Option<Vec<u8>>>,
}

fn sof(h: &mut Partial, s: &[u8]) -> Option<()> {
    let (w, hgt) = (be16(s, 3)?, be16(s, 1)?);
    h.size = Some((w as u32, hgt as u32));
    if *s.first()? != 8 {
        return None;
    }
    h.mode = Some(match *s.get(5)? {
        1 => Mode::L,
        3 => Mode::Rgb,
        4 => Mode::Cmyk,
        _ => return None,
    });
    if !h.icclist.is_empty() {
        h.icclist.sort();
        let parts = h.icclist.len();
        let first = *h.icclist[0].get(13)?;
        h.icc = Some(if usize::from(first) == parts {
            Some(h.icclist.iter().flat_map(|p| p.get(14..).unwrap_or(&[]).iter().copied()).collect())
        } else {
            None
        });
        h.icclist.clear();
    }
    let mut i = 6;
    while i < s.len() {
        if i + 3 > s.len() {
            return None;
        }
        i += 3;
    }
    Some(())
}

fn dqt(s: &[u8]) -> Option<()> {
    let mut s = s;
    while !s.is_empty() {
        let precision = if s[0] / 16 == 0 { 1 } else { 2 };
        let len = 1 + precision * 64;
        if s.len() < len {
            return None;
        }
        s = &s[len..];
    }
    Some(())
}

/// `JpegImageFile._open`: None wherever `Image.open` would fail.
pub fn header(b: &[u8]) -> Option<Header> {
    if !b.starts_with(b"\xff\xd8\xff") {
        return None;
    }
    let mut r = Reader { b, p: 3 };
    let mut h = Partial::default();
    let mut s: Vec<u8> = vec![0xFF];
    let mut scan = false;
    while !s.is_empty() {
        if s[0] != 0xFF {
            s = r.read(1).to_vec();
            continue;
        }
        s.extend_from_slice(r.read(1));
        if s.len() < 2 {
            break;
        }
        let m = s[1];
        match m {
            0xC0..=0xFE => {
                match m {
                    0xC0 | 0xC1 | 0xC2 | 0xC3 | 0xC5 | 0xC6 | 0xC7 | 0xC9 | 0xCA | 0xCB | 0xCD | 0xCE | 0xCF | 0xDE => {
                        sof(&mut h, r.segment()?)?
                    }
                    0xC4 | 0xCC | 0xDA | 0xDC | 0xDD | 0xDF => {
                        r.segment()?;
                    }
                    0xDB => dqt(r.segment()?)?,
                    0xE0..=0xEF => app(&mut h, m, r.segment()?)?,
                    0xFE => {
                        r.segment()?;
                    }
                    _ => {}
                }
                if m == 0xDA {
                    scan = true;
                    break;
                }
                s = r.read(1).to_vec();
            }
            0xFF => s = vec![0xFF],
            0x00 => s = r.read(1).to_vec(),
            _ => return None,
        }
    }
    let (width, height) = h.size?;
    let mode = h.mode?;
    if width == 0 || height == 0 || !scan {
        return None;
    }
    Some(Header { width, height, mode, icc: h.icc.flatten() })
}

struct Failed;

unsafe extern "C-unwind" fn error_exit(_: &mut jpeg_common_struct) {
    // No panic hook: a corrupt image is an expected outcome, not a crash.
    std::panic::resume_unwind(Box::new(Failed));
}

unsafe extern "C-unwind" fn output_message(_: &mut jpeg_common_struct) {}

unsafe extern "C-unwind" fn init_source(_: &mut jpeg_decompress_struct) {}

unsafe extern "C-unwind" fn fill_input_buffer(_: &mut jpeg_decompress_struct) -> boolean {
    0
}

unsafe extern "C-unwind" fn skip_input_data(cinfo: &mut jpeg_decompress_struct, num_bytes: c_long) {
    if num_bytes <= 0 {
        return;
    }
    // SAFETY: `src` is the manager installed in `decode`, alive for the whole decode.
    let src = unsafe { &mut *cinfo.src };
    let n = num_bytes as usize;
    if n > src.bytes_in_buffer {
        // Pillow keeps the remainder for data that never comes.
        src.next_input_byte = unsafe { src.next_input_byte.add(src.bytes_in_buffer) };
        src.bytes_in_buffer = 0;
    } else {
        src.next_input_byte = unsafe { src.next_input_byte.add(n) };
        src.bytes_in_buffer -= n;
    }
}

unsafe extern "C-unwind" fn term_source(_: &mut jpeg_decompress_struct) {}

const JPEG_SUSPENDED: c_int = 0;
const JPEG_HEADER_TABLES_ONLY: c_int = 2;

/// The decoded pixels in `h.mode`'s layout (L: 1 byte, RGB: 3, CMYK: 4 as
/// libjpeg emits it, before Pillow's `CMYK;I` inversion).
fn decode_raw(b: &[u8], h: &Header) -> Option<Vec<u8>> {
    let mut err: jpeg_error_mgr = unsafe { std::mem::zeroed() };
    let mut cinfo: jpeg_decompress_struct = unsafe { std::mem::zeroed() };
    let mut src = jpeg_source_mgr {
        next_input_byte: b.as_ptr(),
        bytes_in_buffer: b.len(),
        init_source: Some(init_source),
        fill_input_buffer: Some(fill_input_buffer),
        skip_input_data: Some(skip_input_data),
        resync_to_restart: Some(jpeg_resync_to_restart),
        term_source: Some(term_source),
    };
    let (components, space) = match h.mode {
        Mode::L => (1, JCS_GRAYSCALE),
        Mode::Rgb => (3, JCS_RGB),
        Mode::Cmyk => (4, JCS_CMYK),
        _ => return None,
    };
    let (w, hgt) = (h.width as usize, h.height as usize);
    let result = catch_unwind(AssertUnwindSafe(|| unsafe {
        cinfo.common.err = jpeg_std_error(&mut err);
        err.error_exit = Some(error_exit);
        err.output_message = Some(output_message);
        jpeg_create_decompress(&mut cinfo);
        cinfo.src = &mut src;
        let out = (|| {
            loop {
                match jpeg_read_header(&mut cinfo, 0) {
                    JPEG_HEADER_TABLES_ONLY => continue,
                    JPEG_SUSPENDED => return None,
                    _ => break,
                }
            }
            cinfo.out_color_space = space;
            if jpeg_start_decompress(&mut cinfo) == 0 {
                return None;
            }
            let stride = cinfo.output_width as usize * cinfo.output_components as usize;
            if cinfo.output_components != components || stride < w * components as usize {
                return None;
            }
            let mut row = vec![0u8; stride];
            let mut pixels = vec![0u8; w * hgt * components as usize];
            for y in 0..hgt {
                let mut rows = [row.as_mut_ptr()];
                if jpeg_read_scanlines(&mut cinfo, rows.as_mut_ptr(), 1) != 1 {
                    return None;
                }
                let n = w * components as usize;
                pixels[y * n..(y + 1) * n].copy_from_slice(&row[..n]);
            }
            jpeg_finish_decompress(&mut cinfo);
            Some(pixels)
        })();
        jpeg_destroy_decompress(&mut cinfo);
        out
    }));
    match result {
        Ok(out) => out,
        Err(payload) => {
            if !payload.is::<Failed>() {
                std::panic::resume_unwind(payload);
            }
            unsafe { jpeg_destroy_decompress(&mut cinfo) };
            None
        }
    }
}

/// `Image.open(b)` + `load()` for a JPEG; None wherever Pillow raises.
pub fn open(b: &[u8], max_pixels: u64) -> Option<Picture> {
    let h = header(b)?;
    if u64::from(h.width) * u64::from(h.height) > max_pixels {
        return None;
    }
    let mut data = decode_raw(b, &h)?;
    if h.mode == Mode::Cmyk {
        for v in &mut data {
            *v = 255 - *v;
        }
    }
    Some(Picture {
        mode: h.mode,
        width: h.width as usize,
        height: h.height as usize,
        data,
        icc: h.icc,
        ..Default::default()
    })
}

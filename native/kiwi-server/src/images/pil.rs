//! `Image.open(data)` as Pillow 13 runs it for the four formats Image Studio
//! and the Mods Hub take (PNG, JPEG/MPO, GIF, WebP through libwebp 1.6.0, as
//! Pillow's wheels bundle it), plus `n_frames` and the WebP encoder.

use std::ffi::{c_int, c_void};

use libwebp_sys as webp;

use crate::media::pil::{self as mpil, Mode, Trns};

/// `2 * Image.MAX_IMAGE_PIXELS`: above it `Image.open` raises.
pub const BOMB_PIXELS: u64 = 2 * 89_478_485;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Png,
    Jpeg,
    Gif,
    Webp,
}

pub struct Opened {
    pub format: Format,
    pub width: usize,
    pub height: usize,
    /// `convert("RGBA")`.
    pub rgba: Vec<u8>,
    /// `mode in ("RGBA", "LA") or (mode == "P" and "transparency" in info)`.
    pub has_alpha: bool,
}

pub fn format_of(d: &[u8]) -> Option<Format> {
    if d.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(Format::Png)
    } else if d.starts_with(b"GIF87a") || d.starts_with(b"GIF89a") {
        Some(Format::Gif)
    } else if d.starts_with(b"\xff\xd8\xff") {
        Some(Format::Jpeg)
    } else if d.starts_with(b"RIFF")
        && d.get(8..12) == Some(b"WEBP")
        && matches!(d.get(12..16), Some(b"VP8 " | b"VP8X" | b"VP8L"))
    {
        Some(Format::Webp)
    } else {
        None
    }
}

/// `Image.open(data)` + `load()`; None wherever Pillow raises.
pub fn open(data: &[u8]) -> Option<Opened> {
    let format = format_of(data)?;
    let opened = |p: mpil::Picture, has_alpha: bool| Opened {
        format,
        width: p.width,
        height: p.height,
        rgba: mpil::to_rgba(&p),
        has_alpha,
    };
    match format {
        Format::Png => {
            let (_, color) = kiwi_pil::decode::png_header(data)?;
            png_frames(data).ok()?;
            if kiwi_pil::decode::png_supported(data) {
                let img = kiwi_pil::decode::png_to_rgba(data).ok()?;
                let has_alpha = matches!(color, 4 | 6) || (color == 3 && has_trns(data));
                let rgba = img.data.iter().flatten().copied().collect();
                return Some(Opened { format, width: img.width, height: img.height, rgba, has_alpha });
            }
            mpil::open(data, BOMB_PIXELS).map(|p| opened(p, false))
        }
        Format::Gif | Format::Jpeg => mpil::open(data, BOMB_PIXELS).map(|p| {
            let alpha = p.mode == Mode::P && p.trns != Trns::None;
            opened(p, alpha)
        }),
        Format::Webp => {
            let (width, height, rgba, has_alpha, _) = webp_decode(data)?;
            Some(Opened { format, width, height, rgba, has_alpha })
        }
    }
}

/// `getattr(img, "n_frames", 1)` after a successful [`open`]; `Err` where
/// counting raises something other than `EOFError`.
pub fn n_frames(data: &[u8], format: Format) -> Result<usize, ()> {
    match format {
        Format::Png => png_frames(data),
        Format::Jpeg => Ok(mpo_frames(data).unwrap_or(1)),
        Format::Gif => gif_frames(data),
        Format::Webp => webp_decode(data).map(|d| d.4).ok_or(()),
    }
}

fn be32(b: &[u8], at: usize) -> u32 {
    u32::from_be_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

/// The chunks before the first IDAT.
fn png_chunks(d: &[u8]) -> Vec<(&[u8], &[u8])> {
    let mut out = Vec::new();
    let mut i = 8;
    while i + 8 <= d.len() {
        let n = be32(d, i) as usize;
        let kind = &d[i + 4..i + 8];
        if kind == b"IDAT" {
            break;
        }
        let Some(body) = d.get(i + 8..i + 8 + n) else { break };
        out.push((kind, body));
        i += 12 + n;
    }
    out
}

fn has_trns(d: &[u8]) -> bool {
    png_chunks(d).iter().any(|(k, _)| *k == b"tRNS")
}

/// `PngImageFile.n_frames`: the acTL count, plus the default image when no
/// fcTL comes before the IDAT. `Err` where `_open` raises.
fn png_frames(d: &[u8]) -> Result<usize, ()> {
    let (mut frames, mut bbox, mut seq) = (None::<u32>, false, None::<u32>);
    let (w, h) = if d.len() >= 24 { (be32(d, 16), be32(d, 20)) } else { (0, 0) };
    for (kind, s) in png_chunks(d) {
        match kind {
            b"acTL" => {
                if s.len() < 8 {
                    return Err(());
                }
                if frames.is_some() {
                    frames = None;
                    continue;
                }
                let n = be32(s, 0);
                if n != 0 && n <= 0x8000_0000 {
                    frames = Some(n);
                }
            }
            b"fcTL" => {
                if s.len() < 26 {
                    return Err(());
                }
                let n = be32(s, 0);
                if seq.map_or(n != 0, |p| u64::from(p) + 1 != u64::from(n)) {
                    return Err(());
                }
                seq = Some(n);
                let (fw, fh, px, py) = (be32(s, 4), be32(s, 8), be32(s, 12), be32(s, 16));
                if u64::from(px) + u64::from(fw) > u64::from(w) || u64::from(py) + u64::from(fh) > u64::from(h) {
                    return Err(());
                }
                bbox = true;
            }
            _ => {}
        }
    }
    Ok(match frames {
        None => 1,
        Some(n) => n as usize + usize::from(!bbox),
    })
}

/// `MpoImageFile.n_frames` when `jpeg_factory` would adopt the file as an MPO.
fn mpo_frames(d: &[u8]) -> Option<usize> {
    let (mut mp, mut app1) = (None, Vec::new());
    let mut i = 2;
    while i + 4 <= d.len() {
        if d[i] != 0xFF {
            i += 1;
            continue;
        }
        let m = d[i + 1];
        if m == 0xFF {
            i += 1;
            continue;
        }
        if !matches!(m, 0xC0..=0xC7 | 0xC9..=0xCF | 0xDA..=0xEF | 0xFE) {
            i += 2;
            continue;
        }
        let n = usize::from(u16::from_be_bytes([d[i + 2], d[i + 3]])).max(2);
        let seg = d.get(i + 4..i + 2 + n)?;
        if m == 0xE1 {
            app1.push(seg);
        } else if m == 0xE2 && seg.starts_with(b"MPF\0") {
            mp = Some(&seg[4..]);
        }
        if m == 0xDA {
            break;
        }
        i += 2 + n;
    }
    let mp = mp?;
    let head = mp.get(..8)?;
    let big = head.starts_with(b"MM\x00\x2a");
    let ifd_big = match &head[..4] {
        b"MM\x00\x2a" | b"MM\x2a\x00" => true,
        b"II\x2a\x00" | b"II\x00\x2a" => false,
        _ => return None,
    };
    let u16_at = |b: &[u8], at: usize, big: bool| {
        b.get(at..at + 2).map(|s| if big { u16::from_be_bytes([s[0], s[1]]) } else { u16::from_le_bytes([s[0], s[1]]) })
    };
    let u32_at = |b: &[u8], at: usize, big: bool| {
        b.get(at..at + 4).map(|s| {
            let a = [s[0], s[1], s[2], s[3]];
            if big { u32::from_be_bytes(a) } else { u32::from_le_bytes(a) }
        })
    };
    let next = u32_at(head, 4, ifd_big)? as usize;
    let count = usize::from(u16_at(mp, next, ifd_big)?);
    let (mut quant, mut entries) = (None, None);
    for k in 0..count {
        let e = next + 2 + k * 12;
        if mp.len() < e + 12 {
            break;
        }
        let (Some(tag), Some(typ), Some(n)) = (u16_at(mp, e, ifd_big), u16_at(mp, e + 2, ifd_big), u32_at(mp, e + 4, ifd_big))
        else {
            break;
        };
        let unit = match typ {
            1 | 2 | 6 | 7 => 1usize,
            3 | 8 => 2,
            4 | 9 | 11 | 13 => 4,
            5 | 10 | 12 | 16 | 17 | 18 => 8,
            _ => continue,
        };
        let size = unit * n as usize;
        let data = if size > 4 {
            let off = u32_at(mp, e + 8, ifd_big)? as usize;
            match mp.get(off..off + size) {
                Some(s) => s,
                None => continue,
            }
        } else {
            &mp[e + 8..e + 8 + size]
        };
        if data.is_empty() {
            continue;
        }
        match (tag, typ) {
            (0xB001, 1 | 3 | 4 | 6 | 8 | 9 | 13 | 16) if n == 1 => {
                quant = Some(match typ {
                    1 => i64::from(data[0]),
                    6 => i64::from(data[0] as i8),
                    3 => i64::from(u16_at(data, 0, ifd_big)?),
                    8 => i64::from(u16_at(data, 0, ifd_big)? as i16),
                    9 => i64::from(u32_at(data, 0, ifd_big)? as i32),
                    _ => i64::from(u32_at(data, 0, ifd_big)?),
                });
            }
            (0xB001, _) => quant = None,
            (0xB002, 7) => entries = Some(data),
            _ => {}
        }
    }
    let quant = quant?;
    let entries = entries?;
    for k in 0..usize::try_from(quant).ok()? {
        let attr = u32_at(entries, k * 16, big)?;
        entries.get(k * 16..k * 16 + 16)?;
        if (attr >> 24) & 7 != 0 {
            return None;
        }
    }
    if quant <= 1 || app1.iter().any(|s| s.windows(16).any(|w| w == b" hdrgm:Version=\"")) {
        return None;
    }
    Some(quant as usize)
}

/// `GifImageFile.n_frames`: frame 0 plus every later image descriptor.
fn gif_frames(d: &[u8]) -> Result<usize, ()> {
    let mut p = 13usize;
    let flags = *d.get(10).ok_or(())?;
    if flags & 0x80 != 0 {
        p += 3 << ((flags & 7) + 1);
    }
    let byte = |p: &mut usize| -> Option<u8> {
        let b = d.get(*p).copied();
        if b.is_some() {
            *p += 1;
        }
        b
    };
    let skip_blocks = |p: &mut usize| loop {
        match d.get(*p).copied() {
            Some(0) => {
                *p += 1;
                break;
            }
            Some(n) => *p = (*p + 1 + usize::from(n)).min(d.len()),
            None => break,
        }
    };
    let mut frames = 0;
    loop {
        if frames > 0 {
            skip_blocks(&mut p);
        }
        let mut found = false;
        loop {
            match byte(&mut p) {
                None | Some(b';') => break,
                Some(b'!') => {
                    byte(&mut p).ok_or(())?;
                    skip_blocks(&mut p);
                }
                Some(b',') => {
                    let desc = d.get(p..(p + 9).min(d.len())).ok_or(())?;
                    p += desc.len();
                    let f = *desc.get(8).ok_or(())?;
                    if f & 0x80 != 0 {
                        p = (p + (3 << ((f & 7) + 1))).min(d.len());
                    }
                    byte(&mut p).ok_or(())?;
                    found = true;
                    break;
                }
                Some(_) => {}
            }
        }
        if !found {
            return if frames == 0 { Err(()) } else { Ok(frames) };
        }
        frames += 1;
    }
}

/// `WebPAnimDecoder` as `_webp.c` drives it: the first frame's RGBA canvas,
/// whether Pillow calls the mode RGBA, and the frame count.
fn webp_decode(data: &[u8]) -> Option<(usize, usize, Vec<u8>, bool, usize)> {
    unsafe {
        let mut f: webp::WebPBitstreamFeatures = std::mem::zeroed();
        let st = webp::WebPGetFeaturesInternal(
            data.as_ptr(),
            data.len(),
            &mut f,
            webp::WEBP_DECODER_ABI_VERSION as c_int,
        );
        let rgba_mode = st != webp::VP8StatusCode::VP8_STATUS_OK || f.has_alpha != 0;
        let src = webp::WebPData { bytes: data.as_ptr(), size: data.len() };
        let dec = webp::WebPAnimDecoderNewInternal(&src, std::ptr::null(), webp::WEBP_DEMUX_ABI_VERSION as c_int);
        if dec.is_null() {
            return None;
        }
        let out = (|| {
            let mut info: webp::WebPAnimInfo = std::mem::zeroed();
            if webp::WebPAnimDecoderGetInfo(dec, &mut info) == 0 {
                return None;
            }
            let (w, h) = (info.canvas_width as usize, info.canvas_height as usize);
            if (w as u64) * (h as u64) > BOMB_PIXELS {
                return None;
            }
            let mut buf: *mut u8 = std::ptr::null_mut();
            let mut ts: c_int = 0;
            if webp::WebPAnimDecoderGetNext(dec, &mut buf, &mut ts) == 0 || buf.is_null() {
                return None;
            }
            let rgba = std::slice::from_raw_parts(buf, w * h * 4).to_vec();
            Some((w, h, rgba, rgba_mode, info.frame_count as usize))
        })();
        webp::WebPAnimDecoderDelete(dec);
        out
    }
}

/// `img.save(buf, "WEBP", quality=quality, method=method)` of RGB (`alpha`
/// false, the fourth byte ignored) or RGBA pixels: `_webp.WebPEncode` with
/// `use_argb`, then the WebPMux reassembly it always runs.
pub fn webp_encode(rgba: &[u8], width: usize, height: usize, alpha: bool, quality: f32, method: i32) -> Option<Vec<u8>> {
    unsafe {
        let mut config: webp::WebPConfig = std::mem::zeroed();
        if webp::WebPConfigInitInternal(
            &mut config,
            webp::WebPPreset::WEBP_PRESET_DEFAULT,
            75.0,
            webp::WEBP_ENCODER_ABI_VERSION as c_int,
        ) == 0
        {
            return None;
        }
        config.lossless = 0;
        config.quality = quality;
        config.alpha_quality = 100;
        config.method = method;
        config.exact = 0;
        if webp::WebPValidateConfig(&config) == 0 {
            return None;
        }
        let mut pic: webp::WebPPicture = std::mem::zeroed();
        if webp::WebPPictureInitInternal(&mut pic, webp::WEBP_ENCODER_ABI_VERSION as c_int) == 0 {
            return None;
        }
        pic.width = c_int::try_from(width).ok()?;
        pic.height = c_int::try_from(height).ok()?;
        pic.use_argb = 1;
        if webp::WebPPictureAlloc(&mut pic) == 0 {
            return None;
        }
        let stride = pic.argb_stride as usize;
        for y in 0..height {
            let dst = std::slice::from_raw_parts_mut(pic.argb.add(y * stride), width);
            for (x, px) in dst.iter_mut().enumerate() {
                let s = &rgba[(y * width + x) * 4..(y * width + x) * 4 + 4];
                let a = if alpha { u32::from(s[3]) } else { 0xFF };
                *px = u32::from(s[2]) | u32::from(s[1]) << 8 | u32::from(s[0]) << 16 | a << 24;
            }
        }
        let mut writer: webp::WebPMemoryWriter = std::mem::zeroed();
        webp::WebPMemoryWriterInit(&mut writer);
        pic.writer = Some(webp::WebPMemoryWrite);
        pic.custom_ptr = (&mut writer as *mut webp::WebPMemoryWriter).cast::<c_void>();
        let ok = webp::WebPEncode(&config, &mut pic);
        webp::WebPPictureFree(&mut pic);
        if ok == 0 {
            webp::WebPMemoryWriterClear(&mut writer);
            return None;
        }
        let image = webp::WebPData { bytes: writer.mem, size: writer.size };
        let mux = webp::WebPMuxNew();
        let mut assembled = webp::WebPData { bytes: std::ptr::null(), size: 0 };
        if !mux.is_null() {
            webp::WebPMuxSetImage(mux, &image, 0);
            webp::WebPMuxAssemble(mux, &mut assembled);
            webp::WebPMuxDelete(mux);
        }
        webp::WebPMemoryWriterClear(&mut writer);
        if assembled.bytes.is_null() || assembled.size == 0 {
            return None;
        }
        let out = std::slice::from_raw_parts(assembled.bytes, assembled.size).to_vec();
        webp::WebPFree(assembled.bytes as *mut c_void);
        Some(out)
    }
}

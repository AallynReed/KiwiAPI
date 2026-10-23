//! Against Pillow 13 (`gen_media_fixtures.py`): conversion tables, LANCZOS
//! thumbnails, PNG bytes, JPEG/PNG/GIF decoding.

use base64::Engine;
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::*;
use crate::media::tests::golden;

fn sha(b: &[u8]) -> String {
    hex::encode(Sha256::digest(b))
}

pub fn lcg_bytes(seed: u32, n: usize) -> Vec<u8> {
    let mut x = seed & 0x7FFF_FFFF;
    (0..n)
        .map(|_| {
            x = (x.wrapping_mul(1103515245).wrapping_add(12345)) & 0x7FFF_FFFF;
            ((x >> 16) & 0xFF) as u8
        })
        .collect()
}

/// `pattern(w, h, seed, kind)` of the generator.
pub fn pattern(w: usize, h: usize, seed: u32, kind: &str) -> Vec<u8> {
    if kind == "noise" {
        return lcg_bytes(seed, w * h * 4);
    }
    let noise = lcg_bytes(seed, w * h);
    let mut out = vec![0u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) * 4;
            let n = usize::from(noise[y * w + x]);
            out[i] = ((x * 255 / (w.max(2) - 1)) & 0xFF) as u8;
            out[i + 1] = ((y * 255 / (h.max(2) - 1)) & 0xFF) as u8;
            out[i + 2] = ((x * y + n / 8) & 0xFF) as u8;
            out[i + 3] = if kind == "opaque" { 255 } else { (((x + y) * 7 + n / 4) & 0xFF) as u8 };
        }
    }
    out
}

fn b64(v: &Value) -> Vec<u8> {
    base64::engine::general_purpose::STANDARD.decode(v.as_str().unwrap()).unwrap()
}

#[test]
fn conversion_tables_match_pillow() {
    let t = &golden()["pil"]["tables"];
    let grid: Vec<u8> =
        (0..256u32).flat_map(|c| (0..256u32).flat_map(move |a| [c as u8, c as u8, c as u8, a as u8])).collect();
    let mut pre = grid.clone();
    premultiply(&mut pre);
    assert_eq!(sha(&pre), t["premultiply"]);
    let mut un = grid.clone();
    unpremultiply(&mut un);
    assert_eq!(sha(&un), t["unpremultiply"]);
    let cmyk: Vec<u8> = (0..256u32)
        .flat_map(|c| (0..256u32).flat_map(move |k| [c as u8, 255 - c as u8, (c / 2) as u8, k as u8]))
        .collect();
    let pic = Picture { mode: Mode::Cmyk, width: 256, height: 256, data: cmyk, ..Default::default() };
    assert_eq!(sha(&to_rgba(&pic)), t["cmyk"]);
}

#[test]
fn thumbnails_and_png_match_pillow() {
    for c in golden()["pil"]["thumbs"].as_array().unwrap() {
        let (w, h) = (c["w"].as_u64().unwrap() as usize, c["h"].as_u64().unwrap() as usize);
        let px = pattern(w, h, c["seed"].as_u64().unwrap() as u32, c["kind"].as_str().unwrap());
        assert_eq!(sha(&encode_png(&px, w, h, None)), c["png"], "png {w}x{h}");
        let size = c["thumb_size"].as_array().unwrap();
        let (tw, th) = (size[0].as_u64().unwrap() as usize, size[1].as_u64().unwrap() as usize);
        assert_eq!(thumb_size(w, h, 128, 128), (tw, th), "size {w}x{h}");
        let (small, sw, sh) = thumbnail(&px, w, h, 128).unwrap();
        assert_eq!((sw, sh), (tw, th));
        assert_eq!(sha(&small), c["thumb_rgba"], "thumb pixels {w}x{h}");
        assert_eq!(sha(&encode_png(&small, sw, sh, None)), c["thumb"], "thumb png {w}x{h}");
    }
}

fn mode_name(m: Mode) -> &'static str {
    match m {
        Mode::One => "1",
        Mode::L => "L",
        Mode::La => "LA",
        Mode::P => "P",
        Mode::Rgb => "RGB",
        Mode::Rgba => "RGBA",
        Mode::Cmyk => "CMYK",
        Mode::I16 => "I;16",
    }
}

fn check_open(c: &Value, data: &[u8]) {
    let name = c["name"].as_str().unwrap();
    let got = open(data, 1 << 40);
    if c.get("error").is_some() {
        assert!(got.is_none(), "{name}: Pillow fails, Rust decoded");
        return;
    }
    let pic = got.unwrap_or_else(|| panic!("{name}: Pillow decodes, Rust failed"));
    let size = c["size"].as_array().unwrap();
    assert_eq!((pic.width as u64, pic.height as u64), (size[0].as_u64().unwrap(), size[1].as_u64().unwrap()), "{name}");
    assert_eq!(mode_name(pic.mode), c["mode"], "{name}");
    let rgba = to_rgba(&pic);
    assert_eq!(sha(&rgba), c["rgba"], "{name}: pixels");
    assert_eq!(sha(&encode_png(&rgba, pic.width, pic.height, pic.icc.as_deref())), c["png"], "{name}: png");
    if let Some(t) = c.get("thumb") {
        let got = thumbnail(&rgba, pic.width, pic.height, 128)
            .map(|(px, w, h)| sha(&encode_png(&px, w, h, pic.icc.as_deref())));
        assert_eq!(got.as_deref(), t.as_str(), "{name}: thumb");
    }
}

#[test]
fn jpeg_decodes_like_libjpeg_turbo_in_pillow() {
    for c in golden()["pil"]["jpeg"].as_array().unwrap() {
        check_open(c, &b64(&c["jpeg"]));
    }
}

#[test]
fn png_and_gif_payloads_decode_like_pillow() {
    for c in golden()["pil"]["payloads"].as_array().unwrap() {
        check_open(c, &b64(&c["data"]));
    }
}

#[test]
fn round_aspect_prefers_floor_on_ties() {
    assert_eq!(thumb_size(129, 1, 128, 128), (128, 1));
    assert_eq!(thumb_size(1, 300, 128, 128), (1, 128));
    assert_eq!(thumb_size(256, 128, 128, 128), (128, 64));
}

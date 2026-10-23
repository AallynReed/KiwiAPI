//! Replays native/port/parity/gen_pil_ops_fixtures.py: resize, rotate,
//! composite, paste, point, PNG decode and encode against Pillow 13.

use base64::Engine;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::{
    Mask, Rgba,
    decode::png_to_rgba,
    ops::{self, linear_gradient},
    png::encode_rgba,
};

fn cases() -> Value {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata").join("ops.json");
    serde_json::from_slice(&std::fs::read(p).unwrap()).unwrap()
}

fn b64(v: &Value) -> Vec<u8> {
    base64::engine::general_purpose::STANDARD.decode(v.as_str().unwrap()).unwrap()
}

fn dims(v: &Value) -> (usize, usize) {
    (v["w"].as_u64().unwrap() as usize, v["h"].as_u64().unwrap() as usize)
}

fn mask(v: &Value) -> Mask {
    let (w, h) = dims(v);
    Mask { width: w, height: h, data: b64(&v["raw"]) }
}

fn rgba(v: &Value) -> Rgba {
    let (w, h) = dims(v);
    Rgba { width: w, height: h, data: b64(&v["raw"]).chunks_exact(4).map(|p| [p[0], p[1], p[2], p[3]]).collect() }
}

fn rgba_bytes(im: &Rgba) -> Vec<u8> {
    im.data.iter().flatten().copied().collect()
}

fn check(name: &str, w: usize, h: usize, raw: &[u8], want: &Value, fails: &mut Vec<String>) {
    let (ww, wh) = dims(want);
    if (w, h) != (ww, wh) {
        fails.push(format!("{name}: size {w}x{h} != {ww}x{wh}"));
        return;
    }
    let sha = hex(&Sha256::digest(raw));
    if sha != want["sha"].as_str().unwrap() {
        let detail = match want.get("raw") {
            Some(r) => {
                let exp = b64(r);
                let n = raw.iter().zip(&exp).filter(|(a, b)| a != b).count();
                let i = raw.iter().zip(&exp).position(|(a, b)| a != b).unwrap_or(0);
                format!("{n} bytes differ, first at {i}: {} != {}", raw[i], exp[i])
            }
            None => "hash differs".into(),
        };
        fails.push(format!("{name}: {detail}"));
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

#[test]
fn resize_matches_pillow() {
    let mut fails = Vec::new();
    for (i, c) in cases()["resize"].as_array().unwrap().iter().enumerate() {
        let (nw, nh) = (c["size"][0].as_u64().unwrap() as usize, c["size"][1].as_u64().unwrap() as usize);
        let name = format!("resize[{i}] {}", c["mode"]);
        match c["mode"].as_str().unwrap() {
            "L" => {
                let out = ops::resize_l(&mask(&c["src"]), nw, nh);
                check(&name, out.width, out.height, &out.data, &c["out"], &mut fails);
            }
            "RGBA" => {
                let out = ops::resize_rgba(&rgba(&c["src"]), nw, nh);
                check(&name, out.width, out.height, &rgba_bytes(&out), &c["out"], &mut fails);
            }
            _ => {
                let out = ops::resize_l(&linear_gradient(), nw, nh);
                check(&name, out.width, out.height, &out.data, &c["out"], &mut fails);
            }
        }
    }
    assert!(fails.is_empty(), "{fails:#?}");
}

#[test]
fn rotate_matches_pillow() {
    let mut fails = Vec::new();
    for (i, c) in cases()["rotate"].as_array().unwrap().iter().enumerate() {
        let angle = c["angle"].as_f64().unwrap();
        let out = ops::rotate_l(&mask(&c["src"]), angle);
        check(&format!("rotate[{i}] {angle}"), out.width, out.height, &out.data, &c["out"], &mut fails);
    }
    assert!(fails.is_empty(), "{fails:#?}");
}

#[test]
fn compositing_matches_pillow() {
    let mut fails = Vec::new();
    for (i, c) in cases()["composite"].as_array().unwrap().iter().enumerate() {
        let at = |c: &Value| (c["at"][0].as_i64().unwrap(), c["at"][1].as_i64().unwrap());
        let name = format!("composite[{i}] {}", c["op"]);
        match c["op"].as_str().unwrap() {
            "composite" => {
                let out = ops::composite(&rgba(&c["a"]), &rgba(&c["b"]), &mask(&c["mask"]));
                check(&name, out.width, out.height, &rgba_bytes(&out), &c["out"], &mut fails);
            }
            "alpha_composite" => {
                let mut dst = rgba(&c["dst"]);
                let (x, y) = at(c);
                ops::alpha_composite(&mut dst, &rgba(&c["src"]), x, y);
                check(&name, dst.width, dst.height, &rgba_bytes(&dst), &c["out"], &mut fails);
            }
            "paste_masked" => {
                let mut dst = rgba(&c["dst"]);
                let (x, y) = at(c);
                ops::paste_masked(&mut dst, &rgba(&c["src"]), &mask(&c["mask"]), x, y);
                check(&name, dst.width, dst.height, &rgba_bytes(&dst), &c["out"], &mut fails);
            }
            _ => {
                let o = c["opacity"].as_f64().unwrap();
                let out = ops::point(&ops::alpha(&rgba(&c["src"])), |a| (a as f64 * o) as i64);
                check(&name, out.width, out.height, &out.data, &c["out"], &mut fails);
            }
        }
    }
    assert!(fails.is_empty(), "{fails:#?}");
}

#[test]
fn png_decode_matches_pillow() {
    let mut fails = Vec::new();
    for (i, c) in cases()["decode"].as_array().unwrap().iter().enumerate() {
        let data = b64(&c["png"]);
        let name = format!("decode[{i}] depth {} type {}", data[24], data[25]);
        match (png_to_rgba(&data), c["out"].get("error")) {
            (Ok(im), None) => check(&name, im.width, im.height, &rgba_bytes(&im), &c["out"], &mut fails),
            (Err(_), Some(_)) => {}
            (Ok(_), Some(e)) => fails.push(format!("{name}: decoded, Pillow raised {e}")),
            (Err(e), None) => fails.push(format!("{name}: {e}")),
        }
    }
    assert!(fails.is_empty(), "{fails:#?}");
}

#[test]
fn rgba_png_bytes_match_pillow() {
    for c in cases()["encode"].as_array().unwrap() {
        let src = rgba(&c["src"]);
        let got = encode_rgba(&rgba_bytes(&src), src.width, src.height);
        assert!(got == b64(&c["png"]), "{}x{} PNG bytes differ", src.width, src.height);
    }
}

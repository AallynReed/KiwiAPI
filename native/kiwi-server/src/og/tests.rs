//! Replays native/port/parity/gen_og_fixtures.py: the cards Python's
//! `og_image` draws, compared pixel for pixel (and byte for byte).

use serde_json::Value;

use super::{activity, pymath, status, text::Fonts};

fn testdata(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata").join("og").join(name)
}

fn fonts() -> Fonts {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../kiwi-pil/testdata/fonts");
    Fonts {
        regular: std::fs::read(dir.join("DejaVuSans.ttf")).unwrap(),
        bold: std::fs::read(dir.join("DejaVuSans-Bold.ttf")).unwrap(),
    }
}

fn cases() -> Value {
    serde_json::from_slice(&std::fs::read(testdata("cases.json")).unwrap()).unwrap()
}

fn pixels(png: &[u8]) -> (u32, u32, Vec<u8>) {
    let mut reader = png::Decoder::new(std::io::Cursor::new(png)).read_info().unwrap();
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buf).unwrap();
    buf.truncate(info.buffer_size());
    (info.width, info.height, buf)
}

/// Pixels must match exactly; the bytes are reported separately.
fn compare(name: &str, got: &[u8], want_file: &str, failures: &mut Vec<String>, byte_diffs: &mut Vec<String>) {
    let want = std::fs::read(testdata(want_file)).unwrap();
    let (gw, gh, gp) = pixels(got);
    let (ww, wh, wp) = pixels(&want);
    if (gw, gh) != (ww, wh) {
        failures.push(format!("{name}: size {gw}x{gh} != {ww}x{wh}"));
    } else if gp != wp {
        let n = gp.chunks(3).zip(wp.chunks(3)).filter(|(a, b)| a != b).count();
        let first = gp.iter().zip(&wp).position(|(a, b)| a != b).unwrap() / 3;
        failures.push(format!(
            "{name}: {n} pixels differ, first at ({}, {})",
            first % gw as usize,
            first / gw as usize
        ));
    }
    if got != want.as_slice() {
        byte_diffs.push(name.to_owned());
    }
}

#[test]
fn activity_cards_match_pillow() {
    let fonts = fonts();
    let lib = kiwi_pil::Library::new().unwrap();
    let (mut failures, mut bytes) = (Vec::new(), Vec::new());
    for c in cases()["activity"].as_array().unwrap() {
        let name = c["name"].as_str().unwrap();
        let inputs = activity::Inputs::from_payloads(&c["series"], &c["live"]);
        let img = activity::draw(&lib, &fonts, &inputs, c["period"].as_str().unwrap()).unwrap();
        compare(name, &super::encode(&img), &format!("activity_{name}.png"), &mut failures, &mut bytes);
    }
    assert!(failures.is_empty(), "{failures:#?}");
    assert!(bytes.is_empty(), "same pixels, different PNG bytes: {bytes:?}");
}

#[test]
fn status_cards_match_pillow() {
    let fonts = fonts();
    let lib = kiwi_pil::Library::new().unwrap();
    let all = cases();
    let now = all["now"].as_i64().unwrap();
    let (mut failures, mut bytes) = (Vec::new(), Vec::new());
    for c in all["status"].as_array().unwrap() {
        let name = c["name"].as_str().unwrap();
        let u = c["uptimes"].as_array().unwrap();
        let uptimes = [u[0].as_f64(), u[1].as_f64(), u[2].as_f64()];
        let img = status::draw(&lib, &fonts, &c["payload"], uptimes, now).unwrap();
        compare(name, &super::encode(&img), &format!("status_{name}.png"), &mut failures, &mut bytes);
    }
    assert!(failures.is_empty(), "{failures:#?}");
    assert!(bytes.is_empty(), "same pixels, different PNG bytes: {bytes:?}");
}

/// A Python `repr(float)` (serde_json may misround 17-digit numbers).
fn float(v: &Value) -> f64 {
    v.as_str().unwrap().parse().unwrap()
}

#[test]
fn hypot_is_cpython_math_hypot() {
    for row in cases()["hypot"].as_array().unwrap() {
        let (x, y, want) = (float(&row[0]), float(&row[1]), float(&row[2]));
        assert_eq!(pymath::hypot(x, y).to_bits(), want.to_bits(), "hypot({x}, {y})");
    }
}

#[test]
fn fixed2_is_python_format() {
    for row in cases()["fixed2"].as_array().unwrap() {
        let x = float(&row[0]);
        assert_eq!(status::fixed2(x), row[1].as_str().unwrap(), "{x}");
    }
}

#[test]
fn badge_svg_matches() {
    for row in cases()["badges"].as_array().unwrap() {
        let svg = status::badge_svg("trove", row[0].as_str().unwrap(), row[1].as_str().unwrap());
        assert_eq!(svg, row[2].as_str().unwrap());
    }
}

#[test]
fn helpers() {
    assert_eq!(super::text::thousands(0), "0");
    assert_eq!(super::text::thousands(999), "999");
    assert_eq!(super::text::thousands(1000), "1,000");
    assert_eq!(super::text::thousands(-1234567), "-1,234,567");
    assert_eq!(super::activity_period(""), "7d");
    assert_eq!(super::activity_period("1M"), "1m");
    assert_eq!(super::activity_period("1d"), "7d");
    assert_eq!(super::normalize_lang("pt-PT"), "pt-PT");
    assert_eq!(super::normalize_lang("pt"), "en");
    assert_eq!(activity::trove_label(1792708167.0, true), "Thu");
    assert_eq!(activity::trove_label(1792103367.0, false), "Oct 15");
    let uri: axum::http::Uri = "/status/og.png?lang=ja&lang=en".parse().unwrap();
    assert_eq!(super::query_last(&uri, "lang").as_deref(), Some("en"));
}

//! Replays native/port/parity/gen_og_fixtures.py: the cards Python's
//! `og_image` draws, compared pixel for pixel (and byte for byte).

use serde_json::Value;

use super::{
    activity, board,
    i18n::{Arg, Locale, Tr},
    pyformat::{self, FormatError},
    pymath, status,
    text::{Fonts, LangFonts},
};

fn testdata(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata").join("og").join(name)
}

fn dejavu(file: &str) -> Vec<u8> {
    std::fs::read(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../kiwi-pil/testdata/fonts").join(file))
        .unwrap()
}

fn fonts() -> Fonts {
    Fonts { regular: dejavu("DejaVuSans.ttf"), bold: dejavu("DejaVuSans-Bold.ttf") }
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
        let img = status::draw(&lib, &fonts, &Locale::en(), &c["payload"], uptimes, now).unwrap();
        compare(name, &super::encode(&img), &format!("status_{name}.png"), &mut failures, &mut bytes);
    }
    assert!(failures.is_empty(), "{failures:#?}");
    assert!(bytes.is_empty(), "same pixels, different PNG bytes: {bytes:?}");
}

#[test]
fn board_cards_match_pillow() {
    let fonts = fonts();
    let lib = kiwi_pil::Library::new().unwrap();
    let all = cases();
    let (now, hhmm) = (all["now"].as_i64().unwrap(), all["hhmm"].as_str().unwrap());
    let (mut failures, mut bytes) = (Vec::new(), Vec::new());
    for b in all["boards"].as_array().unwrap() {
        let name = b["name"].as_str().unwrap();
        let img = board::draw_board(&lib, &fonts, &Locale::en(), &b["data"], now, hhmm).unwrap();
        compare(name, &super::encode(&img), &format!("board_{name}.png"), &mut failures, &mut bytes);
    }
    assert!(failures.is_empty(), "{failures:#?}");
    assert!(bytes.is_empty(), "same pixels, different PNG bytes: {bytes:?}");
}

#[test]
fn announcements_match_pillow() {
    let fonts = fonts();
    let lib = kiwi_pil::Library::new().unwrap();
    let all = cases();
    let (now, hhmm) = (all["now"].as_i64().unwrap(), all["hhmm"].as_str().unwrap());
    let (mut failures, mut bytes) = (Vec::new(), Vec::new());
    for a in all["announcements"].as_array().unwrap() {
        let kind = a["kind"].as_str().unwrap();
        let src = &all["sources"][a["set"].as_str().unwrap()];
        let mut data = serde_json::Map::new();
        for s in board::sources(kind) {
            data.insert((*s).to_owned(), src[*s].clone());
        }
        let (title, accent, lines) = board::announcement_content(&Tr::en(), kind, &Value::Object(data), now).unwrap();
        let label = format!("{} {kind}", a["set"]);
        assert_eq!(title, a["title"].as_str().unwrap(), "{label}");
        assert_eq!(accent.map(Value::from).to_vec(), *a["accent"].as_array().unwrap(), "{label}");
        let want: Vec<&str> = a["lines"].as_array().unwrap().iter().map(|l| l.as_str().unwrap()).collect();
        assert_eq!(lines, want, "{label}");
        if let Some(png) = a.get("png").and_then(Value::as_str) {
            let img = board::draw_announcement(&lib, &fonts, &Locale::en(), &title, accent, &lines, hhmm).unwrap();
            compare(&label, &super::encode(&img), png, &mut failures, &mut bytes);
        }
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
fn replies() {
    let ok = super::png_reply(Some(bytes::Bytes::from_static(b"png")), "public, max-age=600");
    assert_eq!(ok.status(), 200);
    assert_eq!(ok.headers()["content-type"], "image/png");
    assert_eq!(ok.headers()["cache-control"], "public, max-age=600");
    let failed = super::png_reply(None, "public, max-age=60");
    assert_eq!(failed.status(), 302);
    assert_eq!(failed.headers()["location"], "/static/assets/favicon.png");
    assert!(failed.headers().get("content-type").is_none());
}

/// No `_REG` / `_BOLD` face on disk: `_font` ends on `ImageFont.load_default()`.
#[test]
fn fallback_cards_match_pillow() {
    use sha2::{Digest, Sha256};
    let lib = kiwi_pil::Library::new().unwrap();
    let all = cases();
    let (now, hhmm) = (all["now"].as_i64().unwrap(), all["hhmm"].as_str().unwrap());
    let fb = &all["fallback"];
    let sha: String = Sha256::digest(super::text::DEFAULT_FONT).iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(sha, fb["default_font_sha256"].as_str().unwrap());
    let (mut failures, mut bytes) = (Vec::new(), Vec::new());
    for c in fb["cases"].as_array().unwrap() {
        let face = |present: bool, file: &str| if present { dejavu(file) } else { super::text::DEFAULT_FONT.to_vec() };
        let fonts = Fonts {
            regular: face(c["regular"].as_bool().unwrap(), "DejaVuSans.ttf"),
            bold: face(c["bold"].as_bool().unwrap(), "DejaVuSans-Bold.ttf"),
        };
        let name = c["name"].as_str().unwrap();
        let en = Locale::en();
        let st = all["status"].as_array().unwrap().iter().find(|s| s["name"] == c["status"]).unwrap();
        let u = st["uptimes"].as_array().unwrap();
        let img = status::draw(&lib, &fonts, &en, &st["payload"], [u[0].as_f64(), u[1].as_f64(), u[2].as_f64()], now);
        compare(name, &super::encode(&img.unwrap()), &format!("fallback_{name}_status.png"), &mut failures, &mut bytes);
        let b = all["boards"].as_array().unwrap().iter().find(|b| b["name"] == c["board"]).unwrap();
        let img = board::draw_board(&lib, &fonts, &en, &b["data"], now, hhmm).unwrap();
        compare(name, &super::encode(&img), &format!("fallback_{name}_board.png"), &mut failures, &mut bytes);
        let act = all["activity"].as_array().unwrap().iter().find(|a| a["name"] == c["activity"]).unwrap();
        let inputs = activity::Inputs::from_payloads(&act["series"], &act["live"]);
        let img = activity::draw(&lib, &fonts, &inputs, act["period"].as_str().unwrap()).unwrap();
        compare(name, &super::encode(&img), &format!("fallback_{name}_activity.png"), &mut failures, &mut bytes);
        let src = &all["sources"][c["announce"][0].as_str().unwrap()];
        let kind = c["announce"][1].as_str().unwrap();
        let data = serde_json::json!({ "luxion": src["luxion"] });
        let (title, accent, lines) = board::announcement_content(&Tr::en(), kind, &data, now).unwrap();
        let img = board::draw_announcement(&lib, &fonts, &en, &title, accent, &lines, hhmm).unwrap();
        compare(name, &super::encode(&img), &format!("fallback_{name}_announce.png"), &mut failures, &mut bytes);
    }
    assert!(failures.is_empty(), "{failures:#?}");
    assert!(bytes.is_empty(), "same pixels, different PNG bytes: {bytes:?}");
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
}

fn json_file(name: &str) -> Value {
    serde_json::from_slice(&std::fs::read(testdata(name)).unwrap()).unwrap()
}

/// The snapshot catalogs and subset faces the localized fixtures were drawn with.
fn test_locale(lang: &str) -> Locale {
    let dir = testdata("locales");
    let tr = Tr::load_from(lang, &dir.join("site"), &dir.join("app")).unwrap();
    let f = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../kiwi-pil/testdata/fonts");
    let pair = |r: &str, b: &str| LangFonts { regular: vec![f.join(r)], bold: vec![f.join(b)] };
    let fonts = match lang {
        "ja" | "ko" | "zh-CN" => pair("NotoSansCJK-subset-Regular.ttc", "NotoSansCJK-subset-Bold.ttc"),
        "th" => pair("NotoSansThai-subset-Regular.ttf", "NotoSansThai-subset-Bold.ttf"),
        _ => LangFonts::default(),
    };
    Locale { tr, fonts }
}

#[test]
fn localized_cards_match_pillow() {
    let fonts = fonts();
    let lib = kiwi_pil::Library::new().unwrap();
    let all = cases();
    let (now, hhmm) = (all["now"].as_i64().unwrap(), all["hhmm"].as_str().unwrap());
    let (mut failures, mut bytes) = (Vec::new(), Vec::new());
    let mut drawn = 0;
    for row in json_file("l10n.json").as_array().unwrap() {
        let lang = row["lang"].as_str().unwrap();
        let loc = test_locale(lang);
        let png = row.get("png").and_then(Value::as_str);
        let img = match row["card"].as_str().unwrap() {
            "status" => {
                let c = all["status"].as_array().unwrap().iter().find(|c| c["name"] == row["case"]).unwrap();
                let u = c["uptimes"].as_array().unwrap();
                let uptimes = [u[0].as_f64(), u[1].as_f64(), u[2].as_f64()];
                Some(status::draw(&lib, &fonts, &loc, &c["payload"], uptimes, now))
            }
            "board" => {
                let b = all["boards"].as_array().unwrap().iter().find(|b| b["name"] == row["set"]).unwrap();
                Some(board::draw_board(&lib, &fonts, &loc, &b["data"], now, hhmm))
            }
            _ => {
                let kind = row["kind"].as_str().unwrap();
                let src = &all["sources"][row["set"].as_str().unwrap()];
                let mut data = serde_json::Map::new();
                for s in board::sources(kind) {
                    data.insert((*s).to_owned(), src[*s].clone());
                }
                let (title, accent, lines) =
                    board::announcement_content(&loc.tr, kind, &Value::Object(data), now).unwrap();
                let label = format!("{lang} {} {kind}", row["set"]);
                assert_eq!(title, row["title"].as_str().unwrap(), "{label}");
                assert_eq!(accent.map(Value::from).to_vec(), *row["accent"].as_array().unwrap(), "{label}");
                let want: Vec<&str> = row["lines"].as_array().unwrap().iter().map(|l| l.as_str().unwrap()).collect();
                assert_eq!(lines, want, "{label}");
                png.map(|_| board::draw_announcement(&lib, &fonts, &loc, &title, accent, &lines, hhmm))
            }
        };
        if let (Some(img), Some(png)) = (img, png) {
            compare(png, &super::encode(&img.unwrap()), png, &mut failures, &mut bytes);
            drawn += 1;
        }
    }
    assert!(drawn >= 40, "only {drawn} localized cards drawn");
    assert!(failures.is_empty(), "{failures:#?}");
    assert!(bytes.is_empty(), "same pixels, different PNG bytes: {bytes:?}");
}

fn format_args(v: &Value) -> Vec<(&str, Arg<'_>)> {
    v.as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| {
            let arg = match v {
                Value::String(s) => Arg::Str(s),
                other => Arg::Int(other.as_i64().unwrap()),
            };
            (k.as_str(), arg)
        })
        .collect()
}

#[test]
fn str_format_matches_python() {
    let all = json_file("i18n.json");
    let args = format_args(&all["format_args"]);
    let mut bad = Vec::new();
    for row in all["format"].as_array().unwrap() {
        let tpl = row[0].as_str().unwrap();
        let want = match row[1][0].as_str().unwrap() {
            "ok" => Ok(row[1][1].as_str().unwrap().to_owned()),
            "lookup" => Err(FormatError::Lookup),
            _ => Err(FormatError::Other),
        };
        let got = pyformat::format(tpl, &args);
        if got != want {
            bad.push(format!("{tpl:?}: {got:?} != {want:?}"));
        }
    }
    assert!(bad.is_empty(), "{} differ: {:#?}", bad.len(), &bad[..bad.len().min(30)]);
}

#[test]
fn catalog_edges_match_python() {
    let all = json_file("i18n.json");
    let dir = testdata("locales_edge");
    let mut bad = Vec::new();
    for row in all["edge"].as_array().unwrap() {
        let (lang, text) = (row["lang"].as_str().unwrap(), row["text"].as_str().unwrap());
        let args = format_args(&row["fmt"]);
        let got = Tr::load_from(lang, &dir.join("site"), &dir.join("app"))
            .ok()
            .and_then(|tr| if args.is_empty() { Some(tr.t(text)) } else { tr.tf(text, &args).ok() });
        let want = (row["res"][0] == "ok").then(|| row["res"][1].as_str().unwrap().to_owned());
        if got != want {
            bad.push(format!("{lang} {text:?}: {got:?} != {want:?}"));
        }
    }
    assert!(bad.is_empty(), "{bad:#?}");
    assert_eq!(super::i18n::norm("  a \t b\u{3000}\u{1f}c  "), "a b c");
}

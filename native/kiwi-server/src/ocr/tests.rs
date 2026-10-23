//! Checks against `testdata/ocr/blocks.json` (`gen_ocr_fixtures.py`) and
//! `testdata/ocr/golden.json` (`gen_ocr_golden.py`).

use std::sync::OnceLock;

use kiwi_core::ocr::{
    clipper,
    cv::{self, Img},
    decode, npsort, pil, text,
};
use serde_json::Value;
use sha2::{Digest, Sha256};

fn blocks() -> &'static Value {
    static G: OnceLock<Value> = OnceLock::new();
    G.get_or_init(|| crate::pyjson::loads(include_bytes!("../../testdata/ocr/blocks.json")).expect("fixture"))
}

/// The route's replies that settle before the models run
/// (`testdata/tools/golden.json`, `gen_tools_fixtures.py`).
#[tokio::test]
async fn route_errors_match_python() {
    use axum::response::IntoResponse;
    use base64::{Engine, engine::general_purpose::STANDARD};
    let g: Value = crate::pyjson::loads(include_bytes!("../../testdata/tools/golden.json")).unwrap();
    let addr = regex::Regex::new(r"object at 0x[0-9A-Fa-f]+").unwrap();
    for c in arr(&g["ocr_errors"]) {
        let name = c["name"].as_str().unwrap();
        let body = STANDARD.decode(c["req_b64"].as_str().unwrap()).unwrap();
        let want = String::from_utf8(STANDARD.decode(c["body_b64"].as_str().unwrap()).unwrap()).unwrap();
        let rid = serde_json::from_str::<Value>(&want).unwrap()["error"]["request_id"].as_str().unwrap().to_owned();
        let resp = crate::request_id::scope(rid, async {
            match kiwi_core::form::parse_form(c["req_ctype"].as_str().map(str::as_bytes), &body) {
                Ok(items) => match super::reply(items).await {
                    Ok(v) => crate::pyjson::PydanticJson(v).into_response(),
                    Err(e) => e.into_response(),
                },
                Err(kiwi_core::form::FormError::Http(msg)) => {
                    crate::error::ApiError::http(axum::http::StatusCode::BAD_REQUEST, Some(&msg)).into_response()
                }
            }
        })
        .await;
        assert_eq!(u64::from(resp.status().as_u16()), c["status"].as_u64().unwrap(), "{name}: status");
        let got = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let got = String::from_utf8(got.to_vec()).unwrap();
        assert_eq!(addr.replace_all(&got, "object at 0x?"), addr.replace_all(&want, "object at 0x?"), "{name}: body");
    }
}

fn arr(v: &Value) -> &Vec<Value> {
    v.as_array().unwrap_or_else(|| panic!("array: {v}"))
}

fn u(v: &Value) -> usize {
    v.as_u64().unwrap_or_else(|| panic!("uint: {v}")) as usize
}

fn sha(b: &[u8]) -> String {
    hex::encode(Sha256::digest(b))
}

fn lcg(seed: u32, n: usize) -> Vec<u8> {
    let mut x = seed;
    (0..n)
        .map(|_| {
            x = x.wrapping_mul(1103515245).wrapping_add(12345) & 0x7FFF_FFFF;
            (x >> 16) as u8
        })
        .collect()
}

fn img(seed: u32, w: usize, h: usize) -> Img {
    Img { w, h, c: 3, data: lcg(seed, w * h * 3) }
}

fn f32s(v: &Value) -> Vec<f32> {
    arr(v).iter().map(|b| f32::from_bits(b.as_u64().unwrap() as u32)).collect()
}

#[test]
fn argsort_matches_numpy() {
    for (i, c) in arr(&blocks()["argsort"]).iter().enumerate() {
        let want: Vec<usize> = arr(&c["order"]).iter().map(u).collect();
        let got = if c["kind"] == "f64" {
            let keys: Vec<f64> = arr(&c["thirds"]).iter().map(|k| k.as_i64().unwrap() as f64 / 3.0).collect();
            npsort::argsort_f64(&keys)
        } else {
            let keys: Vec<i32> = arr(&c["keys"]).iter().map(|v| v.as_i64().unwrap() as i32).collect();
            npsort::argsort_i32(&keys)
        };
        assert_eq!(got, want, "argsort #{i} {} {}", c["keys"], c["thirds"]);
    }
}

#[test]
fn resize_matches_opencv() {
    for c in arr(&blocks()["resize"]) {
        let src = img(u(&c["seed"]) as u32, u(&c["w"]), u(&c["h"]));
        let out = cv::resize_linear(&src, u(&c["dw"]), u(&c["dh"]));
        assert_eq!(sha(&out.data), c["sha"].as_str().unwrap(), "resize {c}");
    }
}

fn mask_of(seed: u32, w: usize, h: usize, density: u8) -> Vec<u8> {
    let raw = lcg(seed, w * h);
    let m: Vec<u8> = raw.iter().map(|&v| (v < density) as u8).collect();
    if seed % 2 == 1 {
        // cv2.dilate with a 3x3 kernel.
        let mut out = vec![0u8; w * h];
        for y in 0..h {
            for x in 0..w {
                let mut v = 0;
                for dy in -1i64..=1 {
                    for dx in -1i64..=1 {
                        let (xx, yy) = (x as i64 + dx, y as i64 + dy);
                        if xx >= 0 && yy >= 0 && (xx as usize) < w && (yy as usize) < h {
                            v = v.max(m[yy as usize * w + xx as usize]);
                        }
                    }
                }
                out[y * w + x] = v;
            }
        }
        out
    } else {
        m
    }
}

#[test]
fn contours_and_rects_match_opencv() {
    for c in arr(&blocks()["contours"]) {
        let (w, h) = (u(&c["w"]), u(&c["h"]));
        let m = mask_of(u(&c["seed"]) as u32, w, h, u(&c["density"]) as u8);
        let dil = cv::dilate2x2(&m, w, h);
        assert_eq!(sha(&dil), c["dilated_sha"].as_str().unwrap(), "dilate seed {}", c["seed"]);
        let got = cv::find_contours(&dil, w, h);
        let want: Vec<Vec<(i32, i32)>> = arr(&c["contours"])
            .iter()
            .map(|p| {
                arr(p).chunks(2).map(|xy| (xy[0].as_i64().unwrap() as i32, xy[1].as_i64().unwrap() as i32)).collect()
            })
            .collect();
        assert_eq!(got.len(), want.len(), "contour count seed {}", c["seed"]);
        for (k, (g, wnt)) in got.iter().zip(&want).enumerate() {
            assert_eq!(g, wnt, "contour {k} seed {}", c["seed"]);
        }
        for (k, (contour, r)) in want.iter().zip(arr(&c["rects"])).enumerate() {
            let rect = cv::min_area_rect(contour);
            let wr = f32s(&r["rect"]);
            assert_eq!(
                [rect.cx, rect.cy, rect.w, rect.h, rect.angle].map(f32::to_bits),
                [wr[0], wr[1], wr[2], wr[3], wr[4]].map(f32::to_bits),
                "minAreaRect {k} seed {} {contour:?}",
                c["seed"]
            );
        }
    }
}

#[test]
fn clipper_offsets_match_pyclipper() {
    let s = &blocks()["scores"];
    for (k, c) in arr(&s["cases"]).iter().enumerate() {
        let b = f32s(&c["box"]);
        let path: Vec<(i64, i64)> = b.chunks(2).map(|p| (p[0] as i64, p[1] as i64)).collect();
        let quad: [(f32, f32); 4] = std::array::from_fn(|i| (b[2 * i], b[2 * i + 1]));
        let d = kiwi_core::ocr::rapid::unclip_distance(&quad);
        let got: Vec<i64> =
            clipper::offset_round(&path, d).unwrap().into_iter().flatten().flat_map(|(x, y)| [x, y]).collect();
        let want: Vec<i64> = arr(&c["expanded"]).iter().map(|v| v.as_i64().unwrap()).collect();
        assert_eq!(got, want, "unclip {k}");
    }
}

#[test]
fn box_scores_and_mini_boxes_match() {
    let s = &blocks()["scores"];
    let (w, h) = (u(&s["w"]), u(&s["h"]));
    let raw = lcg(u(&s["pred_seed"]) as u32, 4 * w * h);
    let pred: Vec<f32> = raw[..w * h].iter().map(|&v| v as f32 / 255.0).collect();
    for (k, c) in arr(&s["cases"]).iter().enumerate() {
        let b = f32s(&c["box"]);
        let quad: [(f32, f32); 4] = std::array::from_fn(|i| (b[2 * i], b[2 * i + 1]));
        let score = kiwi_core::ocr::rapid::box_score(&pred, w, h, &quad);
        assert_eq!(score.to_bits(), c["score"].as_u64().unwrap(), "score {k}: {score}");
        let pts: Vec<(i32, i32)> = arr(&c["expanded"])
            .chunks(2)
            .map(|p| (p[0].as_i64().unwrap() as i32, p[1].as_i64().unwrap() as i32))
            .collect();
        let (mini, sside) = kiwi_core::ocr::rapid::mini_box(&pts);
        let want = f32s(&c["mini"]);
        let got: Vec<u32> = mini.iter().flat_map(|p| [p.0.to_bits(), p.1.to_bits()]).collect();
        assert_eq!(got, want.iter().map(|v| v.to_bits()).collect::<Vec<_>>(), "mini box {k}");
        assert_eq!(sside.to_bits(), c["sside"].as_u64().unwrap() as u32, "sside {k}");
    }
}

#[test]
fn crops_match_opencv() {
    let s = &blocks()["crops"];
    let src = img(u(&s["seed"]) as u32, u(&s["w"]), u(&s["h"]));
    for (k, c) in arr(&s["cases"]).iter().enumerate() {
        let b = f32s(&c["box"]);
        let quad: [(f32, f32); 4] = std::array::from_fn(|i| (b[2 * i], b[2 * i + 1]));
        let out = kiwi_core::ocr::rapid::crop(&src, &quad);
        let shape: Vec<usize> = arr(&c["shape"]).iter().map(u).collect();
        assert_eq!(vec![out.h, out.w, out.c], shape, "crop {k} shape");
        assert_eq!(sha(&out.data), c["sha"].as_str().unwrap(), "crop {k}");
    }
}

#[test]
fn lanczos_matches_pillow() {
    for c in arr(&blocks()["lanczos"]) {
        let src = img(u(&c["seed"]) as u32, u(&c["w"]), u(&c["h"]));
        let out = pil::resize_lanczos(&src, u(&c["dw"]), u(&c["dh"]));
        assert_eq!(sha(&out.data), c["sha"].as_str().unwrap(), "lanczos {c}");
    }
}

#[test]
fn decoders_match_pillow() {
    let mut bad = Vec::new();
    for c in arr(&blocks()["decode"]) {
        let data = hex::decode(c["file"].as_str().unwrap()).unwrap();
        let name = c["name"].as_str().unwrap();
        match decode::decode_rgb(&data) {
            Ok(img) if sha(&img.data) == c["sha"].as_str().unwrap() => {}
            Ok(img) => bad.push(format!("{name}: {}x{} pixels differ", img.w, img.h)),
            Err(e) => bad.push(format!("{name}: {e:?}")),
        }
    }
    assert!(bad.is_empty(), "decoders differ: {bad:?}");
}

#[test]
fn text_parsing_matches_python() {
    let data = &blocks()["text"];
    let vocab = super::vocab().expect("character_stats.json");
    for pair in arr(&data["normalize"]) {
        assert_eq!(text::normalize(pair[0].as_str().unwrap()), pair[1].as_str().unwrap(), "normalize {pair}");
    }
    for pair in arr(&data["labels"]) {
        let got = vocab.match_label(pair[0].as_str().unwrap());
        let want = pair[1].as_array().map(|a| (a[0].as_str().unwrap().to_owned(), a[1].as_f64().unwrap()));
        assert_eq!(got, want, "match_label {pair}");
    }
    for pair in arr(&data["numbers"]) {
        let got = text::parse_number(pair[0].as_str().unwrap());
        let want = pair[1].as_array().map(|a| (a[0].as_f64().unwrap(), a[1].as_bool().unwrap()));
        assert_eq!(got, want, "parse_number {pair}");
    }
    for c in arr(&data["cases"]) {
        let lines: Vec<String> = arr(&c["lines"]).iter().map(|l| l.as_str().unwrap().to_owned()).collect();
        let stats = text::extract(vocab, &lines);
        let mut m = serde_json::Map::new();
        for (k, s) in &stats {
            m.insert(k.clone(), text::stat_json(s));
        }
        let got = serde_json::json!({"stats": m, "matched": stats.len(), "total_known": vocab.defs.len()});
        assert_eq!(crate::pyjson::pydantic(&got), crate::pyjson::pydantic(&c["result"]), "lines {lines:?}");
    }
}

fn golden() -> &'static Value {
    static G: OnceLock<Value> = OnceLock::new();
    G.get_or_init(|| crate::pyjson::loads(include_bytes!("../../testdata/ocr/golden.json")).expect("fixture"))
}

/// The real models, when `KIWI_ORT_DYLIB` and `KIWI_OCR_MODEL_DIR` point at
/// them (the trove-prod venv's onnxruntime and rapidocr folders locally).
fn local_engine() -> Option<&'static super::engine::OrtEngine> {
    let set = |k: &str| std::env::var(k).ok().is_some_and(|v| std::path::Path::new(&v).exists());
    if !(set("KIWI_ORT_DYLIB") && set("KIWI_OCR_MODEL_DIR")) {
        eprintln!("skipping: KIWI_ORT_DYLIB / KIWI_OCR_MODEL_DIR not set");
        return None;
    }
    super::engine::get()
}

fn boxes(v: &Value) -> Vec<f64> {
    arr(v).iter().flat_map(|p| arr(p).iter().map(|v| v.as_f64().unwrap())).collect()
}

#[test]
fn pipeline_matches_rapidocr() {
    let Some(engine) = local_engine() else { return };
    let vocab = super::vocab().expect("character_stats.json");
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/ocr");
    for c in arr(golden()) {
        let name = c["file"].as_str().unwrap();
        let data = std::fs::read(dir.join(name)).unwrap();
        let img = decode::decode_rgb(&data).unwrap();
        let lines = super::run(engine, &img).unwrap();
        let got: Vec<(String, Vec<f64>)> = lines
            .iter()
            .map(|l| (l.text.clone(), l.boxes.iter().flat_map(|p| [p.0 as f64, p.1 as f64]).collect()))
            .collect();
        let want: Vec<(String, Vec<f64>)> =
            arr(&c["elems"]).iter().map(|e| (e["text"].as_str().unwrap().to_owned(), boxes(&e["box"]))).collect();
        assert_eq!(got, want, "{name}: lines");
        for row in arr(&c["rows"]) {
            let b = boxes(&row["box"]);
            let quad: [(f32, f32); 4] = std::array::from_fn(|i| (b[2 * i] as f32, b[2 * i + 1] as f32));
            let texts = super::ocr_row(engine, &img, &quad).unwrap();
            let want: Vec<String> = arr(&row["texts"]).iter().map(|t| t.as_str().unwrap().to_owned()).collect();
            assert_eq!(texts, want, "{name}: ocr_row {}", row["text"]);
        }
        let body = super::process(engine, vocab, &data).unwrap();
        assert_eq!(crate::pyjson::pydantic(&body), c["body_raw"].as_str().unwrap(), "{name}: body");
    }
}

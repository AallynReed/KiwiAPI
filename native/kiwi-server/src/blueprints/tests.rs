//! Checks against `testdata/blueprints` (`native/port/parity/gen_blueprints_fixtures.py`).
//! v5 files are compared by their inflated body: the fixtures come from
//! CPython's zlib, production (and this port) deflate with kiwi-core's.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::OnceLock,
};

use base64::Engine;
use kiwi_core::bpedit::{Fault, codec, editor};
use serde_json::{Value, json};

use super::{cache, pack};
use crate::{pyjson, request_id};

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata").join("blueprints")
}

fn load(name: &str) -> Value {
    serde_json::from_slice(&std::fs::read(dir().join(name)).expect(name)).expect(name)
}

fn engine() -> &'static Value {
    static E: OnceLock<Value> = OnceLock::new();
    E.get_or_init(|| load("engine.json"))
}

fn input(name: &str) -> Vec<u8> {
    std::fs::read(dir().join("bp").join(name)).expect(name)
}

fn b64(v: &Value) -> Vec<u8> {
    B64.decode(v.as_str().expect("b64 string")).expect("b64")
}

fn s(v: &Value) -> &str {
    v.as_str().unwrap_or_else(|| panic!("string: {v}"))
}

/// Same file: exact for v3/v4, same header and inflated body for v5.
fn same_blueprint(a: &[u8], b: &[u8]) -> bool {
    if a.len() > 9 && b.len() > 9 && a[..9] == b[..9] && a[5..9] == 5u32.to_le_bytes() {
        let inf = |d: &[u8]| kiwi_core::zlib::inflate(&d[9..], usize::MAX).map(|z| z.data);
        return inf(a).is_some() && inf(a) == inf(b);
    }
    a == b
}

/// Python's exception class for a fault.
fn class_matches(f: &Fault, py: &str) -> bool {
    match f {
        Fault::Editor(_) => py == "EditorError",
        Fault::Blueprint(_) => matches!(py, "BlueprintError" | "BlueprintEmpty" | "BlueprintTooLarge"),
        Fault::Transform(_) => py == "TransformError",
        Fault::Merge(_) => py == "MergeError",
        Fault::Qb(_) => py == "QbError",
        Fault::Unhandled(_) => !py.ends_with("Error") || matches!(py, "KeyError" | "ZeroDivisionError" | "error"),
    }
}

struct Check {
    bad: Vec<String>,
}

impl Check {
    fn fault(&mut self, label: &str, case: &Value, got: &Fault) {
        let Some(class) = case["error"].as_str() else {
            self.bad.push(format!("{label}: got {got:?}, want ok"));
            return;
        };
        let msg_ok = matches!(got, Fault::Unhandled(_)) || got.message() == s(&case["message"]);
        if !class_matches(got, class) || !msg_ok {
            self.bad.push(format!("{label}: got {got:?}, want {class}: {}", case["message"]));
        }
    }

    fn value(&mut self, label: &str, case: &Value, got: Result<Value, Fault>) {
        match got {
            Err(f) => self.fault(label, case, &f),
            Ok(v) if case.get("ok") != Some(&v) => {
                self.bad.push(format!("{label}: got {}\n  want {}", short(&v), short(&case["ok"])))
            }
            Ok(_) => {}
        }
    }

    fn blob(&mut self, label: &str, case: &Value, got: Result<(Vec<u8>, Value), Fault>) {
        match got {
            Err(f) => self.fault(label, case, &f),
            Ok((bytes, summary)) => {
                let Some(ok) = case.get("ok") else {
                    self.bad.push(format!("{label}: got ok, want {}: {}", case["error"], case["message"]));
                    return;
                };
                if !same_blueprint(&bytes, &b64(&ok["bytes"])) {
                    self.bad.push(format!("{label}: bytes differ"));
                }
                // `bytes` is the deflated length, which only production's zlib reproduces.
                let (mut want, mut summary) = (ok["summary"].clone(), summary);
                if let (Some(w), Some(g)) = (want.as_object_mut(), summary.as_object_mut())
                    && g.get("version") == Some(&json!(5))
                    && g.get("bytes") == Some(&json!(bytes.len()))
                {
                    w.remove("bytes");
                    g.remove("bytes");
                }
                if summary != want {
                    self.bad.push(format!("{label}: summary {summary}\n  want {want}"));
                }
            }
        }
    }

    fn done(self) {
        assert!(self.bad.is_empty(), "{} mismatches:\n{}", self.bad.len(), self.bad.join("\n"));
    }
}

fn short(v: &Value) -> String {
    let t = v.to_string();
    if t.len() > 400 { format!("{}...", &t[..400]) } else { t }
}

fn cases(section: &str) -> &'static Vec<Value> {
    engine()[section].as_array().unwrap_or_else(|| panic!("section {section}"))
}

#[test]
fn inspect_and_decode() {
    let mut c = Check { bad: Vec::new() };
    for case in cases("inspect") {
        let name = s(&case["file"]);
        c.value(&format!("inspect {name}"), case, editor::inspect(&input(name), name));
    }
    for case in cases("decode") {
        let name = s(&case["file"]);
        let data = input(name);
        if codec::is_empty(&data) != case["empty"].as_bool().unwrap() {
            c.bad.push(format!("is_empty {name}"));
        }
        let got = codec::decode(&data).map(|d| json!([d.voxels.len(), d.size]));
        c.value(&format!("decode {name}"), case, got);
    }
    c.done();
}

fn edit_set(name: &str) -> Value {
    let sets: BTreeMap<&str, Value> = [
        ("none", json!([])),
        (
            "paint",
            json!([{"i": 0, "rgb": 0x123456}, {"i": 1, "type": 18, "w": 9}, {"i": 1, "rgb": "65280"},
                   {"i": 2, "del": 1}, {"i": 3, "type": 55, "w": 4, "rgb": 0}]),
        ),
        (
            "grow",
            json!([{"add": [-1, 0, -2], "type": 21, "w": 3, "rgb": 0xFF00FF}, {"add": [0, 0, 0], "type": 56, "w": 7, "rgb": 1},
                   {"add": ["40", 2.9, 1], "type": "54", "w": 1, "rgb": 99}]),
        ),
        ("erase_edge", json!([{"i": 0, "del": true}, {"i": 0, "del": true}])),
        ("bool_idx", json!([{"i": true, "rgb": 5}, {"i": 2.7, "type": 21, "w": 1}])),
        ("not_list", json!({"i": 0})),
        ("not_dict", json!([3])),
        ("missing_i", json!([{"rgb": 1}])),
        ("out_of_range", json!([{"i": 999999, "rgb": 1}])),
        ("bad_type", json!([{"i": 0, "type": 99, "w": 0}])),
        ("half_material", json!([{"i": 0, "w": 2}])),
        ("bad_colour", json!([{"i": 0, "rgb": 0x1000000}])),
        ("str_colour", json!([{"i": 0, "rgb": "red"}])),
        ("bad_coords", json!([{"add": [1, 2], "type": 21, "w": 0, "rgb": 1}])),
        ("coords_type", json!([{"add": [1, null, 2], "type": 21, "w": 0, "rgb": 1}])),
        ("far_add", json!([{"add": [5000, 0, 0], "type": 21, "w": 0, "rgb": 1}])),
        ("add_no_rgb", json!([{"add": [1, 1, 1], "type": 21, "w": 0}])),
        ("add_no_material", json!([{"add": [1, 1, 1], "rgb": 1}])),
    ]
    .into_iter()
    .collect();
    sets[name].clone()
}

#[test]
fn edits_transforms_and_stacks() {
    let mut c = Check { bad: Vec::new() };
    for case in cases("edits") {
        let (name, set) = (s(&case["file"]), s(&case["edits"]));
        let got = editor::apply_edits(&input(name), &edit_set(set)).map(|(d, m)| (d, Value::Object(m)));
        c.blob(&format!("edits {name} {set}"), case, got);
    }
    for case in cases("transform") {
        let name = s(&case["file"]);
        let got = editor::transform(&input(name), &json!([]), &case["ops"]);
        c.blob(&format!("transform {name} {}", case["ops"]), case, got);
    }
    for case in cases("composite") {
        let layers: Vec<Vec<u8>> = case["layers"].as_array().unwrap().iter().map(|l| input(s(l))).collect();
        let refs: Vec<&[u8]> = layers.iter().map(Vec::as_slice).collect();
        let got = editor::composite(
            &input(s(&case["base"])),
            &case["edits"],
            &refs,
            &case["specs"],
            case["anchor_at"].as_i64().unwrap(),
        )
        .map(|(d, m)| (d, Value::Object(m)));
        c.blob(&format!("composite {}", case["name"]), case, got);
    }
    c.done();
}

#[test]
fn qb_export_import_and_lint() {
    let mut c = Check { bad: Vec::new() };
    for case in cases("export") {
        let name = s(&case["file"]);
        let stem = name.split('.').next().unwrap();
        let got = editor::export_qb(&input(name), &json!([]), &[], &json!([]), 0, stem).map(|(files, summary)| {
            let files: serde_json::Map<String, Value> =
                files.into_iter().map(|(n, d)| (n, Value::String(B64.encode(d)))).collect();
            json!({"files": files, "notes": summary["notes"], "attachment": summary["attachment"], "size": summary["size"]})
        });
        c.value(&format!("export {name}"), case, got);
    }
    for case in cases("import") {
        let files: Vec<(String, Vec<u8>)> =
            case["files"].as_object().unwrap().iter().map(|(k, v)| (k.clone(), b64(v))).collect();
        c.blob(&format!("import {}", case["case"]), case, editor::import_qb(&files));
    }
    for case in cases("check") {
        let (name, kind) = (s(&case["file"]), s(&case["kind"]));
        c.value(
            &format!("check {name} {kind}"),
            case,
            editor::check(&input(name), &json!([]), kind, &[], &json!([]), 0),
        );
    }
    c.done();
}

#[test]
fn viewer_payloads() {
    let mut c = Check { bad: Vec::new() };
    for case in cases("pack") {
        let name = s(&case["file"]);
        let got =
            kiwi_core::voxel::pack(&input(name), &format!("blueprints/{name}"), kiwi_core::voxel::RENDER_VOXEL_CAP);
        let got = got.map_err(|e| match e {
            kiwi_core::voxel::PackError::Empty => {
                Fault::Blueprint("This blueprint is an empty placeholder (no voxels to show).".into())
            }
            kiwi_core::voxel::PackError::TooLarge(n) => Fault::Blueprint(format!("too large {n}")),
            kiwi_core::voxel::PackError::Invalid(m) => Fault::Blueprint(m),
            kiwi_core::voxel::PackError::Unhandled(m) => Fault::Unhandled(m),
        });
        c.value(&format!("pack {name}"), case, got);
    }
    for case in load("render.json")["kvx"].as_array().unwrap() {
        let got = pack::kvx_encode(case["payload"].as_object().unwrap());
        if got != b64(&case["kvx"]) {
            c.bad.push(format!("kvx {}", short(&case["payload"])));
        }
        if pack::kvx_decode(&got).as_ref() != Some(&case["payload"]) {
            c.bad.push("kvx round trip".into());
        }
    }
    c.done();
}

fn rgba(png: &[u8]) -> (u32, u32, Vec<u8>) {
    let img = kiwi_core::imaging::decode_png(png).expect("png");
    (img.width, img.height, img.pixels)
}

/// The fixtures come out byte-identical to Pillow's PNGs. On other models a
/// rare edge sample may land differently (numpy/BLAS can round the projection
/// in the last bit), so the pixel tolerance is what has to hold.
#[test]
fn thumbnails() {
    let mut bad = Vec::new();
    let mut identical = 0;
    let renders = load("render.json");
    for case in renders["renders"].as_array().unwrap() {
        let (name, dim) = (s(&case["file"]), case["dim"].as_u64().unwrap() as usize);
        let got = kiwi_core::voxel::render_thumbnail(&input(name), dim)
            .map(|px| kiwi_pil::png::encode_rgba_optimized(&px, dim, dim));
        let Some(png) = case["png"].as_str() else {
            match got {
                Ok(_) => bad.push(format!("{name}: rendered, want {}", case["error"])),
                Err(f) if !class_matches(&f, s(&case["error"])) => {
                    bad.push(format!("{name}: {f:?} vs {}", case["error"]))
                }
                Err(f) if !matches!(f, Fault::Unhandled(_)) && f.message() != s(&case["message"]) => {
                    bad.push(format!("{name}: {} vs {}", f.message(), case["message"]))
                }
                Err(_) => {}
            }
            continue;
        };
        let want_png = std::fs::read(dir().join("render").join(png)).unwrap();
        let got = got.expect("render");
        identical += usize::from(got == want_png);
        let ((w, h, want), (gw, gh, have)) = (rgba(&want_png), rgba(&got));
        if (w, h) != (gw, gh) {
            bad.push(format!("{png}: size {gw}x{gh} vs {w}x{h}"));
            continue;
        }
        let off: Vec<i32> = want.iter().zip(&have).map(|(a, b)| (i32::from(*a) - i32::from(*b)).abs()).collect();
        let differing = off.chunks(4).filter(|p| p.iter().any(|&d| d > 0)).count();
        let worst = off.iter().copied().max().unwrap_or(0);
        if differing * 100 > (w * h) as usize || (worst > 64 && differing > 4) {
            bad.push(format!("{png}: {differing} pixels differ, worst channel {worst}"));
        }
    }
    eprintln!("thumbnails byte-identical to Pillow: {identical}");
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

/// The BRDF atlas conversion, byte for byte. Block-compressed DDS is the one
/// layout Pillow reads that this doesn't (the atlas ships uncompressed).
#[test]
fn brdf_png() {
    for case in load("render.json")["brdf"].as_array().unwrap() {
        let name = s(&case["file"]);
        let data = std::fs::read(dir().join("dds").join(name)).unwrap();
        let got = super::dds::to_rgb(&data).map(|(w, h, rgb)| kiwi_pil::png::encode_rgb(&rgb, w, h));
        match (got, case.get("ok")) {
            (Some(png), Some(want)) => assert_eq!(png, b64(want), "{name}"),
            (None, None) => {}
            (None, Some(_)) => assert_eq!(name, "dxt1.dds", "{name}: Pillow reads it"),
            (Some(_), None) => panic!("{name}: Pillow refuses it"),
        }
    }
    if let Ok(real) = std::fs::read("E:/Trove/textures/brdfmap.dds") {
        let (w, h, _) = super::dds::to_rgb(&real).expect("game atlas");
        assert_eq!((w, h), (1024, 512));
    }
}

/// Normalizes what a Windows-made zip differs in: the clock and `create_system`.
fn zip_normalized(z: &[u8]) -> Vec<u8> {
    let mut z = z.to_vec();
    let mut i = 0;
    while i + 4 <= z.len() {
        match &z[i..i + 4] {
            b"PK\x03\x04" => {
                z[i + 10..i + 14].fill(0);
                i += 30;
            }
            b"PK\x01\x02" => {
                z[i + 5] = 0;
                z[i + 12..i + 16].fill(0);
                i += 46;
            }
            _ => i += 1,
        }
    }
    z
}

#[tokio::test]
async fn editor_routes_replay() {
    let mut bad = Vec::new();
    for case in load("routes.json").as_array().unwrap() {
        let name = s(&case["name"]);
        let route = s(&case["path"]).rsplit('/').next().unwrap().to_owned();
        let items =
            kiwi_core::form::parse_form(Some(s(&case["content_type"]).as_bytes()), &b64(&case["body"])).unwrap();
        let resp = request_id::scope(s(&case["request_id"]).to_owned(), super::editor::handle(&route, items)).await;
        let status = resp.status().as_u16();
        let headers = resp.headers().clone();
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap().to_vec();
        let want = b64(&case["response"]);
        if u64::from(status) != case["status"].as_u64().unwrap() {
            bad.push(format!("{name}: status {status} vs {}: {}", case["status"], String::from_utf8_lossy(&body)));
            continue;
        }
        let same = if route == "export-qb" && status == 200 {
            zip_normalized(&body) == zip_normalized(&want)
        } else if headers.get("content-type").is_some_and(|v| v == "application/octet-stream") {
            same_blueprint(&body, &want)
        } else {
            body == want
        };
        if !same {
            bad.push(format!(
                "{name}: body differs\n  got  {}\n  want {}",
                String::from_utf8_lossy(&body[..body.len().min(300)]),
                String::from_utf8_lossy(&want[..want.len().min(300)])
            ));
        }
        for h in case["headers"].as_array().unwrap() {
            let (k, v) = (s(&h[0]), s(&h[1]));
            let got = headers.get(k).map(|g| g.as_bytes().iter().map(|&b| char::from(b)).collect::<String>());
            if got.as_deref() != Some(v) {
                bad.push(format!("{name}: header {k} {got:?} vs {v:?}"));
            }
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

#[test]
fn small_helpers() {
    assert_eq!(cache::key_for_file("abc", " /Blueprints\\X.blueprint "), "v3:file:abc:blueprints/x.blueprint");
    assert_eq!(cache::key_for_assembly("sig", "game:Foo"), "v3:a22:asm:sig:game:foo");
    assert!(cache::etag_matches(Some("W/\"a\", \"b\""), "\"a\""));
    assert!(!cache::etag_matches(Some("W/W/\"a\""), "\"a\""));
    assert_eq!(super::render::blueprint_key("pts", "A\\B", 64), "render:bp:r2:pts:64:a/b");
    assert_eq!(super::render::creature_key("live-us", "Prefabs\\X", 160), "render:rig:a22:s2:live-us:160:prefabs/x");
    assert_eq!(
        super::editor::disposition("Épée \"best\".blueprint", "edited.blueprint"),
        "attachment; filename=\"_p_e best.blueprint\"; filename*=UTF-8''%C3%89p%C3%A9e%20%22best%22.blueprint"
    );
    let gz = pack::gzip(b"hello hello hello", 6, 7).unwrap();
    assert_eq!(&gz[..10], &[0x1f, 0x8b, 8, 0, 7, 0, 0, 0, 0, 255]);
    assert_eq!(pack::gunzip(&gz).unwrap(), b"hello hello hello");
    let text = pyjson::dumps_compact_ascii(&json!({"p": char::from_u32(0xfc).unwrap().to_string(), "a": [1, 2]}));
    assert_eq!(text, concat!(r#"{"p":""#, "\\", r#"u00fc","a":[1,2]}"#));
}

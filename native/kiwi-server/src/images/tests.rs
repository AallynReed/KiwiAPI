//! Replays native/port/parity/gen_images_fixtures.py.

use std::collections::HashMap;

use base64::Engine;
use mongodb::bson::{Bson, doc, oid::ObjectId};
use serde_json::Value;

use super::{
    contexts,
    model::{Design, DesignBody},
    render::{self, Scene},
    service,
};
use crate::{
    og::text::Fonts,
    validation::{BodyInput, validate},
};

fn golden() -> Value {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/images/golden.json");
    serde_json::from_slice(&std::fs::read(p).unwrap()).unwrap()
}

fn b64(v: &Value) -> Vec<u8> {
    base64::engine::general_purpose::STANDARD.decode(v.as_str().unwrap()).unwrap()
}

fn fonts() -> Fonts {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../kiwi-pil/testdata/fonts");
    Fonts {
        regular: std::fs::read(dir.join("DejaVuSans.ttf")).unwrap(),
        bold: std::fs::read(dir.join("DejaVuSans-Bold.ttf")).unwrap(),
    }
}

fn body(v: &Value) -> DesignBody {
    validate::<DesignBody>(&BodyInput::Json(v.clone())).unwrap()
}

fn pixels(png: &[u8]) -> (u32, u32, Vec<u8>) {
    let mut reader = png::Decoder::new(std::io::Cursor::new(png)).read_info().unwrap();
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buf).unwrap();
    buf.truncate(info.buffer_size());
    (info.width, info.height, buf)
}

#[test]
fn renders_match_pillow() {
    let g = golden();
    let blobs: HashMap<String, Vec<u8>> =
        g["blobs"].as_object().unwrap().iter().map(|(k, v)| (k.clone(), b64(v))).collect();
    let ctx = g["ctx"].as_object().unwrap().clone();
    let now = g["now"].as_i64().unwrap();
    let fonts = fonts();
    let lib = kiwi_pil::Library::new().unwrap();
    let mut fails = Vec::new();
    for c in g["renders"].as_array().unwrap() {
        let name = c["name"].as_str().unwrap();
        let b = body(&c["body"]);
        let scene = Scene { width: b.width, height: b.height, background: &b.background, layers: &b.layers };
        let got = render::encode(&render::draw(&lib, &fonts, &scene, &ctx, &blobs, now));
        let want = b64(&c["png"]);
        if let Ok(dir) = std::env::var("IMAGES_DEBUG_DIR") {
            std::fs::write(format!("{dir}/{name}.got.png"), &got).unwrap();
            std::fs::write(format!("{dir}/{name}.want.png"), &want).unwrap();
        }
        if got != want {
            let (gp, wp) = (pixels(&got), pixels(&want));
            if (gp.0, gp.1) != (wp.0, wp.1) {
                fails.push(format!("{name}: size {}x{} != {}x{}", gp.0, gp.1, wp.0, wp.1));
            } else if gp.2 != wp.2 {
                let n = gp.2.chunks(4).zip(wp.2.chunks(4)).filter(|(a, b)| a != b).count();
                let i = gp.2.iter().zip(&wp.2).position(|(a, b)| a != b).unwrap() / 4;
                fails.push(format!(
                    "{name}: {n} pixels differ, first at ({}, {})",
                    i % gp.0 as usize,
                    i / gp.0 as usize
                ));
            } else {
                fails.push(format!("{name}: same pixels, PNG bytes differ"));
            }
        }
    }
    assert!(fails.is_empty(), "{fails:#?}");
}

fn canonical(d: mongodb::bson::Document) -> Bson {
    Bson::Document(d)
}

/// Python's canonical extended JSON back to BSON (doubles compare by value).
fn ext(v: &Value) -> Bson {
    Bson::try_from(v.clone()).unwrap()
}

#[test]
fn stored_documents_and_dtos_match() {
    let g = golden();
    let owner = ObjectId::parse_str("65a1b2c3d4e5f60718293a4b").unwrap();
    let id = ObjectId::parse_str("65c000000000000000000001").unwrap();
    let now = chrono::DateTime::parse_from_rfc3339("2026-10-22T09:30:15.123456Z").unwrap().to_utc();
    let created = chrono::DateTime::parse_from_rfc3339("2026-10-01T08:00:00.250Z").unwrap().to_utc();
    let asset = "https://trove.aallyn.net";
    for (i, c) in g["models"].as_array().unwrap().iter().enumerate() {
        let b = body(&c["body"]);
        let mut d = Design::create(id, owner, &b, now);
        d.clamp(contexts::is_bindable);
        let mut stored = doc! {"_id": id};
        stored.extend(d.fields());
        assert_eq!(canonical(stored.clone()), ext(&c["created"]["stored"]), "case {i}: created document");
        assert_eq!(
            crate::pyjson::pydantic(&d.dto(asset)),
            c["created"]["dto"].as_str().unwrap(),
            "case {i}: created dto"
        );

        let mut loaded = Design::from_doc(&stored).unwrap();
        loaded.created_at = created;
        loaded.assign(&b);
        loaded.clamp(contexts::is_bindable);
        loaded.updated_at = now;
        let mut set = doc! {"_id": id};
        set.extend(loaded.fields());
        assert_eq!(canonical(set.clone()), ext(&c["updated"]["stored"]), "case {i}: updated document");
        // The PUT response: Beanie's save() merges the stored document back.
        let reloaded = Design::from_doc(&set).unwrap();
        assert_eq!(
            crate::pyjson::pydantic(&reloaded.dto(asset)),
            c["reloaded_dto"].as_str().unwrap(),
            "case {i}: reload"
        );
    }
}

#[test]
fn substitution_matches() {
    let g = golden();
    let ctx = g["ctx"].as_object().unwrap().clone();
    let now = g["now"].as_i64().unwrap();
    for c in g["substitute"].as_array().unwrap() {
        let text = c["text"].as_str().unwrap();
        let out = render::substitute(text, &ctx, now);
        assert_eq!(out, c["out"].as_str().unwrap(), "{text}");
        assert_eq!(render::strip_md(&out), c["stripped"].as_str().unwrap(), "{text}");
    }
}

#[test]
fn bindings_match() {
    let g = golden();
    let got = crate::pyjson::pydantic(&serde_json::json!({"bindings": contexts::bindings()}));
    assert_eq!(got, g["bindings"].as_str().unwrap());
}

#[test]
fn sniff_matches_store() {
    for c in golden()["sniff"].as_array().unwrap() {
        let data = b64(&c["data"]);
        let got = service::sniff_image(&data).map(|(t, w, h)| serde_json::json!([t, w, h]));
        assert_eq!(got.unwrap_or(Value::Null), c["out"], "{:?}", c["data"]);
    }
}

#[test]
fn blob_paths_stay_in_the_store() {
    assert!(service::blob_path("data/mods", "../../etc/passwd").is_none());
    assert!(service::blob_path("data/mods", "a").is_none());
    let p = service::blob_path("data/mods", &"ab".repeat(32)).unwrap();
    assert!(p.ends_with(format!("objects/ab/{}", "ab".repeat(32))));
}

#[test]
fn kinds_resolve_from_announcement_keys() {
    assert_eq!(contexts::kind("challenge_racing"), Some("challenge"));
    assert_eq!(contexts::kind("chaos_chest"), Some("chaos"));
    assert_eq!(contexts::kind("game_update"), Some("game_update"));
    assert!(!contexts::is_bindable("nope"));
}

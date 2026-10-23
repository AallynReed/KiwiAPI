//! Golden checks against `testdata/mod_releases/golden.json`
//! (`native/port/parity/gen_mod_releases_fixtures.py`, Python 3.13).

use std::{collections::HashSet, sync::LazyLock};

use base64::{Engine, engine::general_purpose::STANDARD};
use mongodb::bson::{Bson, Document};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::{
    artifact, event,
    model::{Doc, PROJECT, RELEASE, V},
    read, rig, vfx, write,
};
use crate::pyjson;

static GOLDEN: LazyLock<Value> = LazyLock::new(|| {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/testdata/mod_releases/golden.json");
    serde_json::from_slice(&std::fs::read(path).expect("golden.json")).expect("golden json")
});

fn g(k: &str) -> &'static Value {
    &GOLDEN[k]
}

fn b64(v: &Value) -> Vec<u8> {
    STANDARD.decode(v.as_str().expect("b64 string")).expect("base64")
}

fn bson_doc(v: &Value) -> Document {
    match Bson::try_from(v.clone()).expect("extended JSON") {
        Bson::Document(d) => d,
        other => panic!("not a document: {other:?}"),
    }
}

/// Byte-equal as Python's `json.dumps` would write both (key order counts).
fn same(a: &Value, b: &Value, ctx: &str) {
    assert_eq!(pyjson::dumps(a), pyjson::dumps(b), "{ctx}");
}

fn pairs(v: &Value) -> Vec<(String, String)> {
    v.as_array()
        .expect("pairs")
        .iter()
        .map(|p| (p[0].as_str().unwrap_or_default().to_owned(), p[1].as_str().unwrap_or_default().to_owned()))
        .collect()
}

fn strs(v: &Value) -> Vec<String> {
    v.as_array().map(|a| a.iter().map(|s| s.as_str().unwrap_or_default().to_owned()).collect()).unwrap_or_default()
}

#[test]
fn artifact_views_match_python() {
    for a in g("artifacts").as_array().expect("artifacts") {
        let name = a["name"].as_str().unwrap_or_default();
        let data = b64(&a["tmod_b64"]);
        let r = Doc::load(RELEASE, &bson_doc(&a["release"])).expect("release loads");
        same(&Value::Object(read::release_dto(&r)), &a["dto"], &format!("{name} dto"));
        let meta = artifact::read(&data, false).expect("metadata read");
        let full = artifact::read(&data, true).expect("full read");
        same(&read::files_value(&meta), &a["files"], &format!("{name} files"));
        same(&read::inspect_value(&r, Some(&data)).expect("inspect"), &a["inspect"], &format!("{name} inspect"));
        same(&read::cfgs_value(read::release_props(&r), &meta), &a["cfgs"], &format!("{name} cfgs"));
        same(&read::by_ext(&meta, ".swf"), &a["swfs"], &format!("{name} swfs"));
        same(&read::by_ext(&meta, ".bnk"), &a["banks"], &format!("{name} banks"));
        let (items, _) = vfx::pkfx_and_index(&data).expect("pkfx index");
        same(&vfx::list_value(items), &a["vfx"], &format!("{name} vfx"));
        let (items, fns) = read::blueprint_parts(&full);
        let bps = read::blueprints_value(items, &fns, Value::Null, json!([]), &HashSet::new());
        same(&bps, &a["blueprints"], &format!("{name} blueprints"));
        for (path, want) in a["file_downloads"].as_object().expect("downloads") {
            let got = read::file_bytes(artifact::read(&data, true).expect("read"), path);
            match want.get("b64") {
                Some(w) => assert_eq!(got, Some(b64(w)), "{name} file {path}"),
                None => assert_eq!(got, None, "{name} file {path}"),
            }
        }
        for (path, want) in a["cfg_names"].as_object().expect("cfg names") {
            assert_eq!(artifact::cfg_download_name(read::release_props(&r), path), want.as_str().unwrap_or_default());
        }
        for (want_path, want) in a["bank_files"].as_object().expect("banks") {
            let got = read::bank_files(&full, want_path);
            match want {
                Value::Null => assert!(got.is_none(), "{name} bank {want_path}"),
                w => {
                    let (raw, text) = got.expect("bank found");
                    assert_eq!(hex::encode(Sha256::digest(&raw)), w["sha"].as_str().unwrap_or_default());
                    assert_eq!(text.as_deref(), w["text"].as_str());
                }
            }
        }
    }
}

#[test]
fn zip_reader_matches_zipfile() {
    for z in g("zips").as_array().expect("zips") {
        let name = z["name"].as_str().unwrap_or_default();
        let data = b64(&z["b64"]);
        assert_eq!(artifact::is_zipfile(&data), Ok(z["is_zipfile"].as_bool().unwrap_or(false)), "{name} is_zipfile");
        match (&z["entries"], artifact::zip_entries(&data)) {
            (Value::String(s), Err(e)) => {
                let want = if s == "bad" { artifact::ZipFail::Bad } else { artifact::ZipFail::Unhandled };
                assert_eq!(e, want, "{name}");
            }
            (Value::Array(want), Ok(got)) => {
                let got: Vec<Value> = got.into_iter().map(|(n, s)| json!([n, s])).collect();
                assert_eq!(&got, want, "{name}");
            }
            (want, got) => panic!("{name}: want {want:?}, got {got:?}"),
        }
    }
}

#[test]
fn names_and_headers_match_python() {
    let n = g("names");
    for c in n["safe"].as_array().expect("safe") {
        assert_eq!(artifact::safe_filename(c[0].as_str().unwrap_or_default()), c[1].as_str().unwrap_or_default());
    }
    for c in n["cfg"].as_array().expect("cfg") {
        let got = artifact::cfg_download_name(&pairs(&c["props"]), c["path"].as_str().unwrap_or_default());
        assert_eq!(got, c["out"].as_str().unwrap_or_default(), "{c}");
    }
    for c in n["declared"].as_array().expect("declared") {
        assert_eq!(artifact::declared_config_path(&pairs(&c["props"])), c["out"].as_str().unwrap_or_default());
    }
    for c in n["previews"].as_array().expect("previews") {
        assert_eq!(artifact::preview_path(&pairs(&c["props"])), c["out"].as_str().unwrap_or_default());
    }
    for c in n["flags"].as_array().expect("flags") {
        let props = vec![("flags".to_owned(), c["value"].as_str().unwrap_or_default().to_owned())];
        let flags = artifact::flags_value(&props);
        assert_eq!(json!(flags), c["flags"], "{c}");
        assert_eq!(json!(artifact::categories(flags)), c["categories"], "{c}");
    }
    for c in n["downloads"].as_array().expect("downloads") {
        let got = artifact::download_filename(
            c["format"].as_str().unwrap_or_default(),
            c["stored"].as_str().unwrap_or_default(),
            &pairs(&c["props"]),
        );
        assert_eq!(got, c["out"].as_str().unwrap_or_default());
    }
    for c in n["authors"].as_array().expect("authors") {
        assert_eq!(write::clean_author(c[0].as_str(), "Owner"), c[1].as_str().unwrap_or_default(), "{c}");
    }
    for c in n["thousands"].as_array().expect("thousands") {
        assert_eq!(read::thousands(c[0].as_u64().unwrap_or(0) as usize), c[1].as_str().unwrap_or_default());
    }
}

fn project_with(title: &str, tags: Vec<String>) -> Doc {
    Doc::build(
        PROJECT,
        vec![
            ("slug", V::Str("My Slug!".into())),
            ("title", V::Str(title.into())),
            ("tags", V::List(tags)),
            ("owner_username", V::Str("Owner".into())),
        ],
    )
}

#[test]
fn release_builds_match_python() {
    let b = g("builds");
    for c in b["releases"].as_array().expect("releases") {
        let project = project_with(c["title"].as_str().unwrap_or_default(), strs(&c["tags"]));
        let mut props = write::release_properties(
            &project,
            "v2",
            c["changelog"].as_str().unwrap_or_default(),
            c["author"].as_str(),
        );
        let mut files: write::Files = vec![
            ("UI/Thing.swf".into(), lcg(6, 9)),
            ("ui/My Slug!.png".into(), b"old".to_vec()),
            ("Blueprints/A.Blueprint".into(), b"bp".to_vec()),
        ];
        write::pack_preview(&mut files, &mut props, b"newpreview".to_vec(), "png", project.str("slug"));
        let err = write::inject_config(&mut files, &mut props, b"[thing.swf]\n".to_vec()).err().map(|e| e.message);
        assert_eq!(err.as_deref(), c["config_error"].as_str());
        assert_eq!(props, pairs(&c["props"]));
        let want_files: Vec<(String, Vec<u8>)> = c["files"]
            .as_array()
            .expect("files")
            .iter()
            .map(|f| (f[0].as_str().unwrap_or_default().to_owned(), b64(&f[1])))
            .collect();
        assert_eq!(files, want_files);
        let built =
            crate::tools::tmod::build(1, &props, &files, crate::tools::tmod::KIWI_MOD_LOADER, true).expect("build");
        assert_eq!(built, b64(&c["tmod_b64"]), "{} tmod bytes", c["title"]);
    }
    for c in b["configs"].as_array().expect("configs") {
        let mut files: write::Files = strs(&c["files"]).into_iter().map(|f| (f, Vec::new())).collect();
        let mut props = pairs(&c["props"]);
        let len = c["data_len"].as_u64().unwrap_or(0) as usize;
        let data = match len {
            2 => b"a\0".to_vec(),
            3 => b"cfg".to_vec(),
            _ => vec![b'x'; len],
        };
        match (write::inject_config(&mut files, &mut props, data), c.get("error")) {
            (Err(e), Some(want)) => assert_eq!(e.message, want.as_str().unwrap_or_default()),
            (Ok(()), None) => {
                assert_eq!(files.iter().map(|f| f.0.clone()).collect::<Vec<_>>(), strs(&c["out_files"]));
                assert_eq!(props, pairs(&c["out_props"]));
            }
            (got, want) => panic!("{c}: got {:?}, want {want:?}", got.err()),
        }
    }
    for c in b["repacks"].as_array().expect("repacks") {
        let got =
            write::repack_with_config(&b64(&c["in_b64"]), c["cfg"].as_str().unwrap_or_default().as_bytes().to_vec());
        match (got, c.get("error")) {
            (Err(e), Some(want)) => assert_eq!(e.message, want.as_str().unwrap_or_default()),
            (Ok((art, props)), None) => {
                assert_eq!(art, b64(&c["out_b64"]), "{} repack bytes", c["name"]);
                assert_eq!(props, pairs(&c["props"]));
            }
            (got, want) => panic!("{}: got {:?}, want {want:?}", c["name"], got.err()),
        }
    }
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

#[test]
fn i18n_cleaning_matches_python() {
    for c in g("i18n").as_array().expect("i18n") {
        let base = c["base"].as_str().unwrap_or_default();
        let max = if base == "title" { 160 } else { 20_000 };
        match (write::clean_i18n(&pairs(&c["in"]), base, max), c.get("error")) {
            (Err(e), Some(w)) => assert_eq!(e.message, w.as_str().unwrap_or_default()),
            (Ok(out), None) => assert_eq!(out, pairs(&c["out"])),
            (got, want) => panic!("{c}: got {:?}, want {want:?}", got.err()),
        }
    }
}

#[test]
fn release_event_matches_python() {
    let e = g("event");
    let u = event::Urls {
        api: e["api_url"].as_str().unwrap_or_default().trim_end_matches('/').to_owned(),
        site: e["app_url"].as_str().unwrap_or_default().trim_end_matches('/').to_owned(),
    };
    for c in e["cases"].as_array().expect("cases") {
        let project = Doc::load(PROJECT, &bson_doc(&c["project"])).expect("project");
        let release = Doc::load(RELEASE, &bson_doc(&c["release"])).expect("release");
        let previous =
            c["previous"].as_object().map(|_| Doc::load(RELEASE, &bson_doc(&c["previous"])).expect("previous"));
        let count = c["count"].as_u64().unwrap_or(0);
        same(&Value::Object(event::public_mod_dto(&u, &project)), &c["public_mod"], "public_mod");
        same(&Value::Object(event::event_project(&u, &project)), &c["event_project"], "event_project");
        same(&Value::Object(event::event_release(&u, &release)), &c["event_release"], "event_release");
        same(&event::event_change(&release, previous.as_ref(), count), &c["change"], "change");
        if let Some(p) = &previous {
            same(&Value::Object(event::event_release(&u, p)), &c["previous_release"], "previous_release");
        }
        let data = event::event_data(&u, &project, &release, previous.as_ref(), count);
        assert_eq!(data["image_url"], json!(format!("{}/v1/mods/hub/image/{}", u.api, "ab".repeat(32))));
    }
}

#[test]
fn model_round_trips_stored_documents() {
    let a = &g("artifacts")[0];
    let raw = bson_doc(&a["release"]);
    let r = Doc::load(RELEASE, &raw).expect("release");
    let mut back = r.insert_doc();
    assert_eq!(back.remove("_id"), raw.get("_id").cloned());
    let mut want = raw.clone();
    want.remove("_id");
    assert_eq!(back, want);
    let mut bad = raw.clone();
    bad.insert("status", "gone");
    assert!(Doc::load(RELEASE, &bad).is_none());
}

#[test]
fn rig_resolution_matches_python() {
    let r = g("rig");
    let mut map = rig::ByBlueprint::new();
    for row in r["rows"].as_array().expect("rows") {
        let s = |i: usize| row[i].as_str().unwrap_or_default().to_owned();
        map.entry(s(1)).or_insert((s(2), s(3)));
    }
    for c in r["cases"].as_array().expect("cases") {
        let got = rig::resolve(&strs(&c["fns"]), &map);
        match (got, c["skeleton"].as_str()) {
            (None, None) => {}
            (Some((skel, attach)), Some(want)) => {
                assert_eq!(skel, want);
                assert_eq!(attach, pairs(&c["attach"]));
            }
            (got, want) => panic!("{c}: got {got:?}, want {want:?}"),
        }
    }
}

#[tokio::test]
async fn vfx_manifest_matches_python() {
    let v = g("vfx");
    let a = &g("artifacts")[0];
    let data = b64(&a["tmod_b64"]);
    let (_, index) = vfx::pkfx_and_index(&data).expect("index");
    let game: Vec<(String, String)> = pairs(&v["game"]);
    let in_game = |bn: &str| game.iter().any(|(k, _)| k == bn);
    let m = &v["manifest"];
    let raw = index.iter().find(|(k, _)| k == "spark.pkfx").map(|(_, d)| d.clone()).expect("spark");
    let text = String::from_utf8_lossy(&raw).into_owned();
    assert_eq!(json!(text), m["pkfx"]);
    let (deps, missing) = vfx::manifest_deps(&index, &text, in_game);
    assert_eq!(Value::Array(deps), m["deps"]);
    assert_eq!(Value::Array(missing), m["missing"]);
    let set = vfx::depset(
        &index,
        |bn| game.iter().find(|(k, _)| k == bn).map(|(_, p)| vfx::Where::Game(p.clone())),
        |_| async { Ok(None) },
    )
    .await
    .expect("depset");
    let mut set: Vec<String> = set.into_iter().collect();
    set.sort();
    assert_eq!(set, strs(&v["depset"]));
    assert_eq!(json!(kiwi_core::updates::pkfx::extract_refs(&text)), v["refs"]);
    for c in v["media"].as_array().expect("media") {
        assert_eq!(
            kiwi_core::updates::pkfx::media_type_for(c[0].as_str().unwrap_or_default()),
            c[1].as_str().unwrap_or_default()
        );
    }
}

#[test]
fn compilable_paths_follow_trove_layout() {
    assert!(artifact::is_compilable("ui/a.swf"));
    assert!(artifact::is_compilable("Blueprints/x.blueprint"));
    assert!(!artifact::is_compilable("readme.md"));
    assert!(!artifact::is_compilable("bin/a.swf"));
    assert!(!artifact::is_compilable("ui/a.cfg"));
    assert!(!artifact::is_compilable("ui/notes.TXT"));
    assert!(artifact::is_compilable("ui/noext"));
}

#[test]
fn upload_forms_match_fastapi() {
    let latin1 = |s: &str| s.chars().map(|c| c as u32 as u8).collect::<Vec<u8>>();
    for (route, rows) in g("forms").as_object().expect("forms") {
        for (n, row) in rows.as_array().expect("rows").iter().enumerate() {
            let ctype = row["ctype"].as_str().unwrap_or_default();
            let body = latin1(row["body"].as_str().unwrap_or_default());
            let got = match kiwi_core::form::parse_form(Some(ctype.as_bytes()), &body) {
                Err(kiwi_core::form::FormError::Http(m)) => {
                    (400, json!({"code": "bad_request", "message": m, "details": null}))
                }
                Ok(items) => {
                    let checked = if route == "up" {
                        write::upload_form(&items).map(|u| {
                            json!({"tag": u.tag, "title": u.title, "changelog": u.changelog, "status": u.status,
                                   "branch": u.branch, "silent": u.silent, "filename": u.file.filename,
                                   "size": u.file.data.len(), "config": u.config.map(|c| c.data.len())})
                        })
                    } else {
                        write::config_form(&items).map(|d| json!({"size": d.len()}))
                    };
                    match checked {
                        Ok(v) => (200, v),
                        Err(e) => {
                            let details: Vec<Value> = e.0.iter().map(|i| i.to_value()).collect();
                            (
                                422,
                                json!({"code": "validation_error", "message": "Request validation failed", "details": details}),
                            )
                        }
                    }
                }
            };
            assert_eq!(got, (row["status"].as_i64().unwrap_or(0), row["result"].clone()), "{route} case {n}");
        }
    }
}

#[test]
fn thumbnails_match_pillow() {
    let mut bad = Vec::new();
    for case in crate::images::tests::pil_cases() {
        let name = case["name"].as_str().unwrap();
        let data = b64(&case["data"]);
        for (w, want) in case["thumbs"].as_object().unwrap() {
            let got = super::images::render_thumbnail(&data, w.parse().unwrap()).expect("raised");
            let got = got.map(|t| (t.len() as u64, hex::encode(Sha256::digest(&t))));
            let want = want.as_object().map(|o| (o["len"].as_u64().unwrap(), o["sha"].as_str().unwrap().to_owned()));
            if got != want {
                bad.push(format!("{name} w={w}: got {got:?} want {want:?}"));
            }
        }
    }
    assert!(
        bad.is_empty(),
        "{}",
        bad.join(
            "
"
        )
    );
}

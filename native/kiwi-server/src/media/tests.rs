//! SWF and audio against `gen_media_fixtures.py` (Python 3.13 / Pillow 13):
//! manifests, conversions, zips, and every `/site` route through the router
//! with the archive faked and both stores in temp dirs.

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, OnceLock},
};

use axum::{body::Body, http::Request};
use base64::Engine;
use http_body_util::BodyExt;
use serde_json::Value;
use sha2::{Digest, Sha256};
use tower::ServiceExt;

use super::{audio, cas::ContentStore, swf, zipfile::ZipWriter};
use crate::{config::Settings, db::Db, pyjson, state::AppState, state::Inner};

/// The fake `get_file_meta` behind [`super::file_sha`] while tests run.
pub static ARCHIVE: OnceLock<HashMap<String, String>> = OnceLock::new();

pub fn golden() -> &'static Value {
    static G: OnceLock<Value> = OnceLock::new();
    G.get_or_init(|| pyjson::loads(include_bytes!("../../testdata/media/golden.json")).expect("golden.json"))
}

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata").join("media")
}

fn data(name: &str) -> Vec<u8> {
    std::fs::read(dir().join(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
}

fn codebooks() -> Vec<u8> {
    std::fs::read(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/audio/packed_codebooks_aoTuV_603.bin"))
        .expect("codebook library")
}

fn sha(b: &[u8]) -> String {
    hex::encode(Sha256::digest(b))
}

fn b64(s: &str) -> Vec<u8> {
    base64::engine::general_purpose::STANDARD.decode(s).unwrap()
}

fn temp_dir(tag: &str) -> PathBuf {
    let mut raw = [0u8; 6];
    getrandom::fill(&mut raw).unwrap();
    let p = std::env::temp_dir().join(format!("kiwi-media-{tag}-{}", hex::encode(raw)));
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn same_json(got: &Value, want: &Value, what: &str) {
    assert_eq!(pyjson::dumps(got), pyjson::dumps(want), "{what}");
}

#[test]
fn swf_manifests_match_python() {
    for c in golden()["swf"].as_array().unwrap() {
        let name = c["file"].as_str().unwrap();
        let store = ContentStore::new(temp_dir("swf"));
        let got = swf::build_manifest(&data(name), &store);
        match (got, c.get("manifest"), c.get("crash")) {
            (Ok(m), Some(want), _) => same_json(&m, want, name),
            (Err(swf::extract::Fail::Crash(_)), None, Some(_)) => {}
            (Err(swf::extract::Fail::Swf(e)), None, None) => assert_eq!(e, c["swf_error"].as_str().unwrap(), "{name}"),
            (other, ..) => panic!("{name}: {:?}", other.map(|_| ()).err()),
        }
        if let Some(h) = c.get("header") {
            let got = swf::extract::read_header(&data(name)).unwrap();
            assert_eq!(got.compression, h[1].as_str().unwrap(), "{name}");
            assert_eq!(got.frame_rate, h[4].as_f64().unwrap(), "{name}");
        }
    }
}

#[test]
fn swf_headers_and_errors_match_python() {
    for c in golden()["swf_headers"].as_array().unwrap() {
        let raw = b64(c["raw"].as_str().unwrap());
        match (swf::extract::read_header(&raw), c.get("ok")) {
            (Ok(h), Some(ok)) => {
                let got = serde_json::json!([h.version, h.compression, h.width, h.height, h.frame_rate, h.frame_count]);
                same_json(&got, ok, "header");
            }
            (Err(swf::extract::Fail::Swf(e)), None) => assert_eq!(e, c["error"].as_str().unwrap()),
            (got, _) => panic!("{:?}: {:?}", c, got.map(|_| ()).err()),
        }
    }
}

#[test]
fn wem_parse_and_convert_match_python() {
    let lib = codebooks();
    for (name, c) in golden()["audio"]["convert"].as_object().unwrap() {
        let raw = data(name);
        match (kiwi_core::audio::parse(&raw), c.get("info")) {
            (Ok(i), Some(want)) => {
                let got = serde_json::json!([
                    i.codec_name(),
                    i.channels,
                    i.rate,
                    i.bits,
                    i.align,
                    i.avg,
                    i.samples,
                    i.duration(),
                    i.data_off,
                    i.data_size
                ]);
                same_json(&got, want, name);
            }
            (Err(e), None) => assert_eq!(e, c["parse_error"].as_str().unwrap(), "{name}"),
            (got, _) => panic!("{name}: parse {got:?}"),
        }
        match kiwi_core::audio::convert_detailed(&raw, Ok(&lib)) {
            Ok(conv) => {
                assert_eq!(sha(&conv.bytes), c["sha"].as_str().unwrap(), "{name}");
                assert_eq!((conv.mime, conv.ext), (c["mime"].as_str().unwrap(), c["ext"].as_str().unwrap()));
            }
            Err(kiwi_core::audio::ConvertError::Wem(e)) => assert_eq!(e, c["error"].as_str().unwrap(), "{name}"),
            Err(kiwi_core::audio::ConvertError::Crash(_)) => assert!(c.get("crash").is_some(), "{name} crashed"),
        }
    }
}

#[test]
fn bank_manifest_names_and_helpers_match_python() {
    let a = &golden()["audio"];
    let bank = data("synthetic.bnk");
    let side = String::from_utf8(data("synthetic.txt")).unwrap();
    let store = ContentStore::new(temp_dir("bank"));
    let m = audio::build_manifest(&bank, Some(&side), &store).ok().unwrap();
    same_json(&m, &a["manifest"], "manifest");
    let m = audio::build_manifest(&bank, None, &store).ok().unwrap();
    same_json(&m, &a["manifest_nosidecar"], "manifest without sidecar");
    let (names, events) = audio::names::parse(&side);
    assert_eq!(events as u64, a["events"].as_u64().unwrap());
    let want = a["names"].as_object().unwrap();
    assert_eq!(names.len(), want.len());
    for (id, n) in want {
        let got = &names[id];
        let v = n.as_array().unwrap();
        assert_eq!(
            [&got.name, &got.path, &got.group, &got.source, &got.notes].map(String::as_str),
            [
                v[0].as_str().unwrap(),
                v[1].as_str().unwrap(),
                v[2].as_str().unwrap(),
                v[3].as_str().unwrap(),
                v[4].as_str().unwrap()
            ]
        );
    }
    for (n, id) in a["event_ids"].as_object().unwrap() {
        assert_eq!(u64::from(audio::names::event_id(n)), id.as_u64().unwrap(), "{n}");
    }
    for (t, want) in a["titles"].as_object().unwrap() {
        assert_eq!(audio::studio::safe_title(t), want.as_str().unwrap(), "{t:?}");
    }
    let bad: [(&str, &[u8]); 10] = [
        ("empty", b""),
        ("not_bkhd", b"DIDX\0\0\0\0"),
        ("short_bkhd", b"BKHD\x04\0\0\0\x80\0\0\0"),
        ("version", b"BKHD\x08\0\0\0\x7f\0\0\0\0\0\0\0"),
        ("overrun", b"BKHD\xff\0\0\0"),
        ("didx_odd", b"BKHD\x08\0\0\0\x80\0\0\0\0\0\0\0DIDX\x05\0\0\0abcde"),
        ("no_data", b"BKHD\x08\0\0\0\x80\0\0\0\0\0\0\0DIDX\x0c\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0"),
        ("hirc_small", b"BKHD\x08\0\0\0\x80\0\0\0\0\0\0\0HIRC\x02\0\0\0ab"),
        ("hirc_mid", b"BKHD\x08\0\0\0\x80\0\0\0\0\0\0\0HIRC\x06\0\0\0\x05\0\0\0\x02\0"),
        ("nonascii", b"BKHD\x08\0\0\0\x80\0\0\0\0\0\0\0\xffXYZ\0\0\0\0"),
    ];
    for (name, raw) in bad {
        let got = audio::bank::parse(raw).err();
        assert_eq!(got.as_deref(), a["bad_banks"][name].as_str(), "{name}");
    }
    let pcm = |frames: usize, ch: usize| crate::media::pil::tests::lcg_bytes(frames as u32, frames * ch * 2);
    let enc = &a["encode"];
    for (name, frames, ch, rate, codec) in [
        ("mono_adpcm", 1000, 1, 24000, "adpcm"),
        ("stereo_adpcm", 130, 2, 48000, "adpcm"),
        ("mono_pcm", 77, 1, 11025, "pcm"),
        ("stereo_pcm", 64, 2, 96000, "pcm"),
    ] {
        let got = audio::encode::encode(&pcm(frames, ch), ch as u16, rate, codec).unwrap();
        assert_eq!(sha(&got), enc[name].as_str().unwrap(), "{name}");
    }
    assert_eq!(sha(&audio::encode::silence()), enc["silence"].as_str().unwrap());
}

#[test]
fn zips_match_python_with_a_fixed_clock() {
    let z = &golden()["zips"];
    let store = ContentStore::new(temp_dir("zip"));
    let m = swf::build_manifest(&data("synthetic_fws.swf"), &store).ok().unwrap();
    let mut assets = m["assets"].as_array().unwrap().clone();
    let mut missing = assets[0].clone();
    missing["sha"] = Value::String("0".repeat(64));
    assets.push(missing);
    let clock = Some([2026, 10, 22, 13, 37, 59]);
    let got = swf_zip(&assets, &store, clock);
    assert_eq!((sha(&got), got.len() as u64), (z["swf"].as_str().unwrap().to_owned(), z["swf_len"].as_u64().unwrap()));
    let bstore = ContentStore::new(temp_dir("bzip"));
    let side = String::from_utf8(data("synthetic.txt")).unwrap();
    let bm = audio::build_manifest(&data("synthetic.bnk"), Some(&side), &bstore).ok().unwrap();
    let got = bnk_zip(bm["sounds"].as_array().unwrap(), &bstore, clock);
    let names: Vec<String> = zip_names(&got);
    let want: Vec<&str> = z["bnk_names"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
    assert_eq!(names, want);
    assert_eq!((sha(&got), got.len() as u64), (z["bnk"].as_str().unwrap().to_owned(), z["bnk_len"].as_u64().unwrap()));
}

/// `build_zip` with the writer's clock pinned (the routes use local time).
fn swf_zip(assets: &[Value], store: &ContentStore, clock: Option<[u16; 6]>) -> Vec<u8> {
    let real = swf::build_zip(assets, store).unwrap();
    rezip(&real, clock)
}

fn bnk_zip(sounds: &[Value], store: &ContentStore, clock: Option<[u16; 6]>) -> Vec<u8> {
    rezip(&audio::build_zip(sounds, store).unwrap(), clock)
}

fn zip_names(zip: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let mut p = 0;
    while zip.get(p..p + 4) == Some(b"PK\x03\x04") {
        let size = u32::from_le_bytes(zip[p + 18..p + 22].try_into().unwrap()) as usize;
        let n = u16::from_le_bytes(zip[p + 26..p + 28].try_into().unwrap()) as usize;
        out.push(String::from_utf8(zip[p + 30..p + 30 + n].to_vec()).unwrap());
        p += 30 + n + size;
    }
    out
}

/// The same entries rewritten with a fixed clock (names and data read back
/// from the stored archive).
fn rezip(zip: &[u8], clock: Option<[u16; 6]>) -> Vec<u8> {
    let mut w = ZipWriter::new();
    w.clock = clock;
    let mut p = 0;
    while zip.get(p..p + 4) == Some(b"PK\x03\x04") {
        let size = u32::from_le_bytes(zip[p + 18..p + 22].try_into().unwrap()) as usize;
        let n = u16::from_le_bytes(zip[p + 26..p + 28].try_into().unwrap()) as usize;
        let name = std::str::from_utf8(&zip[p + 30..p + 30 + n]).unwrap().to_owned();
        let start = p + 30 + n;
        w.writestr(&name, &zip[start..start + size]);
        p = start + size;
    }
    w.finish()
}

// --- routes -----------------------------------------------------------------------

struct Env {
    state: AppState,
}

fn env() -> &'static Env {
    static E: OnceLock<Env> = OnceLock::new();
    E.get_or_init(|| {
        let arch = &golden()["archive"];
        let files: HashMap<String, String> = arch["files"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_owned()))
            .collect();
        let _ = ARCHIVE.set(files);
        let updates = temp_dir("updates");
        let store = ContentStore::new(&updates);
        for (s, b) in arch["blobs"].as_object().unwrap() {
            let bytes = match b.get("file") {
                Some(f) => data(f.as_str().unwrap()),
                None => b64(b["b64"].as_str().unwrap()),
            };
            assert_eq!(&store.put(&bytes).unwrap().0, s);
        }
        let mods = temp_dir("mods");
        let settings = Settings::from_vars([
            ("TROVE_UPDATE_STORE_DIR", updates.display().to_string()),
            ("MODS_STORE_DIR", mods.display().to_string()),
        ])
        .unwrap();
        let state = AppState { inner: Arc::new(Inner { settings, db: Db { mongo: None, redis: None, pg: None } }) };
        Env { state }
    })
}

const KEPT: [&str; 6] =
    ["content-type", "cache-control", "etag", "content-disposition", "x-kiwi-sounds-replaced", "x-kiwi-sounds-added"];

async fn send(req: Request<Body>) -> (u16, Vec<(String, String)>, Vec<u8>) {
    let app = super::router().with_state(env().state.clone());
    let res = app.oneshot(req).await.unwrap();
    let status = res.status().as_u16();
    let headers = res
        .headers()
        .iter()
        .filter(|(k, _)| KEPT.contains(&k.as_str()))
        .map(|(k, v)| (k.as_str().to_owned(), String::from_utf8_lossy(v.as_bytes()).into_owned()))
        .collect();
    let body = res.into_body().collect().await.unwrap().to_bytes().to_vec();
    (status, headers, body)
}

fn masked(v: &Value) -> Value {
    let mut v = v.clone();
    if let Some(e) = v.get_mut("error").and_then(Value::as_object_mut) {
        e.insert("request_id".into(), Value::String("-".into()));
    }
    v
}

fn check(c: &Value, (status, headers, body): (u16, Vec<(String, String)>, Vec<u8>)) {
    let name = c["name"].as_str().unwrap();
    let json_body = || pyjson::loads(&body).unwrap_or_else(|_| panic!("{name}: {}", String::from_utf8_lossy(&body)));
    assert_eq!(status as u64, c["status"].as_u64().unwrap(), "{name}: {}", String::from_utf8_lossy(&body));
    let mut want: Vec<(String, String)> = c["headers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| (h[0].as_str().unwrap().to_owned(), h[1].as_str().unwrap().to_owned()))
        .collect();
    let mut got = headers;
    want.sort();
    got.sort();
    assert_eq!(got, want, "{name}: headers");
    if let Some(j) = c.get("json") {
        let got = json_body();
        if j.get("error").is_some() {
            same_json(&masked(&got), &masked(j), name);
        } else {
            assert_eq!(String::from_utf8_lossy(&body), pyjson::dumps(j), "{name}: body");
        }
    } else if let Some(s) = c.get("sha") {
        assert_eq!(sha(&body), s.as_str().unwrap(), "{name}: body");
    } else {
        assert_eq!(body.len() as u64, c["len"].as_u64().unwrap(), "{name}");
    }
}

#[tokio::test]
async fn site_routes_match_python() {
    for c in golden()["routes"].as_array().unwrap() {
        let mut req = Request::builder().method(c["method"].as_str().unwrap()).uri(c["url"].as_str().unwrap());
        if let Some(e) = c.get("if_none_match") {
            req = req.header("if-none-match", e.as_str().unwrap());
        }
        check(c, send(req.body(Body::empty()).unwrap()).await);
    }
}

fn multipart(fields: &[(String, Option<String>, Vec<u8>)]) -> (String, Vec<u8>) {
    let boundary = "kiwiBoundary7MA4YWxkTrZu0gW";
    let mut out = Vec::new();
    for (name, filename, data) in fields {
        let mut disp = format!("form-data; name=\"{name}\"");
        if let Some(f) = filename {
            disp.push_str(&format!("; filename=\"{f}\""));
        }
        out.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: {disp}\r\n").as_bytes());
        if filename.is_some() {
            out.extend_from_slice(b"Content-Type: application/octet-stream\r\n");
        }
        out.extend_from_slice(b"\r\n");
        out.extend_from_slice(data);
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), out)
}

fn field(v: &Value) -> (String, Option<String>, Vec<u8>) {
    let a = v.as_array().unwrap();
    (a[0].as_str().unwrap().to_owned(), a[1].as_str().map(str::to_owned), b64(a[2].as_str().unwrap()))
}

#[tokio::test]
async fn sound_studio_matches_python() {
    for c in golden()["studio"].as_array().unwrap() {
        let fields: Vec<_> = match c.get("form") {
            Some(f) => f.as_array().unwrap().iter().map(field).collect(),
            None => {
                let mut f = vec![("spec".to_owned(), None, c["spec"].as_str().unwrap().as_bytes().to_vec())];
                f.extend(c["clips"].as_array().unwrap().iter().map(field));
                f
            }
        };
        let (ctype, body) = multipart(&fields);
        let req = Request::builder()
            .method("POST")
            .uri("/site/sound-studio/build")
            .header("content-type", ctype)
            .body(Body::from(body))
            .unwrap();
        check(c, send(req).await);
    }
}

#[test]
fn cache_rows_are_the_documents_beanie_inserts() {
    use mongodb::bson::Document;
    let rows = &golden()["cache_rows"];
    let ok = super::bpcache::row("s3:swf:abc", Some(&"f".repeat(64)), 123, 7, None);
    let e = crate::error::ApiError::new(axum::http::StatusCode::UNPROCESSABLE_ENTITY, "bad_request", "Nope.");
    let err = super::bpcache::row("b1:bnk:abc:-", None, 0, 0, Some(&e));
    for (name, mut doc) in [("ok", ok), ("err", err)] {
        assert_eq!(doc.keys().next().map(String::as_str), Some("_id"));
        assert!(matches!(doc.get("created_at"), Some(mongodb::bson::Bson::DateTime(_))));
        doc.remove("_id");
        doc.remove("created_at");
        let want = Document::from_reader(&mut b64(rows[name]["bson"].as_str().unwrap()).as_slice()).unwrap();
        assert_eq!(doc, want, "{name}");
        assert_eq!(doc.keys().collect::<Vec<_>>(), want.keys().collect::<Vec<_>>(), "{name}");
    }
}

#[test]
fn cached_blobs_are_python_gzip_of_compact_ascii_json() {
    let v = serde_json::json!({"name": "Émoji ✓", "f": 29.5, "n": [1, null]});
    let gz = super::bpcache::gzip(pyjson::dumps_compact_ascii(&v).as_bytes(), 1_700_000_000);
    assert_eq!(&gz[..10], &[0x1f, 0x8b, 8, 0, 0x00, 0xf1, 0x53, 0x65, 0, 255]);
    let body = kiwi_core::zlib::gunzip(&gz).unwrap();
    let text = String::from_utf8(body).unwrap();
    assert!(text.is_ascii() && text.contains("u00c9moji") && text.contains("u2713"), "{text}");
    assert!(text.ends_with(r#"","f":29.5,"n":[1,null]}"#), "{text}");
}

#[test]
fn decompile_helpers_follow_python() {
    let s = Settings::default();
    assert_eq!(swf::decompile::version("JPEXS Free Flash Decompiler v.25.1.3\n"), "ffdec 25.1.3");
    assert_eq!(swf::decompile::version("nothing"), "ffdec");
    let root = temp_dir("export");
    std::fs::create_dir_all(root.join("b/Sub")).unwrap();
    std::fs::write(root.join("b/Sub/Z.as"), "zz").unwrap();
    std::fs::write(root.join("a.as"), b"\xff\xfe x").unwrap();
    std::fs::write(root.join("B.as"), "bbb").unwrap();
    let (scripts, truncated) = swf::decompile::read_export(&root, 10, 1000);
    let paths: Vec<&str> = scripts.iter().map(|s| s["path"].as_str().unwrap()).collect();
    assert_eq!((paths, truncated), (vec!["a.as", "B.as", "b/Sub/Z.as"], false));
    assert_eq!(scripts[0]["source"], "\u{fffd}\u{fffd} x");
    assert!(swf::decompile::read_export(&root, 2, 1000).1);
    assert_eq!(swf::decompile::read_export(&root, 10, 5).0.len(), 1);
    let _ = s;
}

#[tokio::test]
#[ignore = "needs Java and JPEXS FFDec at the path the fixture was made with"]
async fn decompile_matches_python_ffdec_run() {
    let Some(g) = golden().get("decompile").filter(|v| !v.is_null()) else { return };
    let s = Settings::from_vars([("FFDEC_JAR", g["jar"].as_str().unwrap())]).unwrap();
    for c in g["cases"].as_array().unwrap() {
        let raw = match c.get("file") {
            Some(f) => data(f.as_str().unwrap()),
            None => b64(c["raw"].as_str().unwrap()),
        };
        match (swf::decompile::decompile_scripts(&s, &raw).await, c.get("result")) {
            (Ok(r), Some(want)) => {
                assert_eq!(r["count"], want["count"]);
                assert_eq!(r["decompiler"], want["decompiler"]);
                let got: Vec<Value> = r["scripts"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|s| serde_json::json!([s["path"], s["size"], sha(s["source"].as_str().unwrap().as_bytes())]))
                    .collect();
                assert_eq!(Value::from(got), want["scripts"]);
            }
            (Err(swf::decompile::Fail::Error(e)), None) => assert_eq!(e, c["error"][1].as_str().unwrap()),
            (got, _) => panic!("{c:?}: {got:?}"),
        }
    }
}

#[test]
fn pyval_int_and_str_follow_python() {
    use audio::pyval::{self, PyVal};
    let v = pyval::loads(r#"{"a": [1, 2.5, null, true, "x'y"], "b": {"c": -0}, "a": 1e400}"#).unwrap();
    assert_eq!(v.repr(), "{'a': inf, 'b': {'c': 0}}");
    let l = pyval::loads(r#"[1, 2.5, null, true, "x'y", -0.0, 1e16]"#).unwrap();
    assert_eq!(l.str(), "[1, 2.5, None, True, \"x'y\", -0.0, 1e+16]");
    for (text, want) in [("1e20", Some("100000000000000000000")), ("-7.9", Some("-7")), ("\" 1_2 \"", Some("12"))] {
        let got = pyval::int(&pyval::loads(text).unwrap()).ok().map(|i| i.to_string());
        assert_eq!(got.as_deref(), want, "{text}");
    }
    for bad in ["\"1__2\"", "\"_1\"", "\"\"", "null", "[]", "\"1.0\""] {
        assert!(pyval::int(&pyval::loads(bad).unwrap()).is_err(), "{bad}");
    }
    assert_eq!(pyval::int(&PyVal::Float(f64::INFINITY)), Err(pyval::IntErr::Overflow));
    assert!(pyval::loads("\u{feff}{}").is_err());
    assert!(pyval::loads("[01]").is_err());
}

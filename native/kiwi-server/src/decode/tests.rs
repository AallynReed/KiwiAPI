use std::path::{Path, PathBuf};

use super::{
    json::{J, dumps},
    registry::{self, DECODERS},
    tree::dir_tree,
};
use crate::obj;

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn baseline() -> PathBuf {
    repo().join("app/trove/gamedata")
}

fn testdata() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/decode")
}

#[test]
fn dumps_like_python() {
    let v = obj! {"a" => vec![J::Int(1), J::Float(0.1), J::Float(1e-5), J::Float(1e16)], "b" => obj! {}, "c" => Vec::<J>::new(), "é" => "x\n"};
    assert_eq!(dumps(&v, None), "{\"a\": [1, 0.1, 1e-05, 1e+16], \"b\": {}, \"c\": [], \"é\": \"x\\n\"}");
    assert_eq!(
        dumps(&obj! {"a" => vec![J::Int(1)], "b" => obj! {"c" => J::Null}}, Some(1)),
        "{\n \"a\": [\n  1\n ],\n \"b\": {\n  \"c\": null\n }\n}"
    );
    assert_eq!(dumps(&J::Float(f64::NAN), None), "NaN");
}

#[test]
fn python_equality() {
    assert_eq!(J::Int(1), J::Float(1.0));
    assert_eq!(J::Bool(true), J::Int(1));
    assert_ne!(J::Str("1".into()), J::Int(1));
    assert_eq!(obj! {"a" => 1, "b" => 2}, obj! {"b" => 2.0, "a" => true});
}

#[test]
fn code_versions_match_python() {
    let want: serde_json::Value =
        serde_json::from_slice(&std::fs::read(testdata().join("code_versions.json")).expect("code_versions.json"))
            .expect("json");
    let src = registry::source_dir(&baseline());
    for d in DECODERS {
        assert_eq!(registry::code_version(d.name, &src).as_deref(), want[d.name].as_str(), "{}", d.name);
    }
}

/// Every fixture tree under testdata/decode/<name>[@case]/game, against the
/// output Python built from it (gen_decode_fixtures.py fixtures).
#[test]
fn fixtures_match_python() {
    let mut ran = 0;
    for entry in std::fs::read_dir(testdata()).expect("testdata") {
        let case = entry.expect("entry").path();
        if !case.join("game").is_dir() {
            continue;
        }
        let file = case.file_name().unwrap().to_string_lossy().into_owned();
        let name = file.split('@').next().unwrap();
        let d = registry::get(name).expect("decoder");
        let tree = dir_tree(&case.join("game"), d.prefixes);
        let want = String::from_utf8(std::fs::read(case.join("expected.json")).expect("expected.json")).expect("utf8");
        let got = match d.run(&tree, &baseline()).expect("ported") {
            Ok(data) => d.dump(&data),
            Err(e) => format!("!{e}"),
        };
        assert!(got == want, "{file}: output differs from Python's");
        ran += 1;
    }
    assert!(ran > 0);
}

fn view_text(rel: &str, data: &[u8]) -> String {
    match super::view::describe_upload(data) {
        Ok(serde_json::Value::Object(payload)) => {
            let mut m = serde_json::Map::new();
            m.insert("path".into(), serde_json::Value::String(rel.to_owned()));
            m.extend(payload);
            crate::pyjson::dumps(&serde_json::Value::Object(m))
        }
        Ok(_) => unreachable!(),
        Err(msg) => format!("!{msg}"),
    }
}

fn view_case(src: &Path, out: &Path, bad: &mut Vec<String>) {
    for entry in walk_files(src) {
        let rel = entry.strip_prefix(src).unwrap().to_string_lossy().replace('\\', "/");
        let want = std::fs::read_to_string(out.join(format!("{}.json", rel.replace('/', "__")))).expect("golden");
        if view_text(&rel, &std::fs::read(&entry).unwrap()) != want {
            bad.push(rel);
        }
    }
}

fn walk_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).expect("dir").flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(walk_files(&p));
        } else {
            out.push(p);
        }
    }
    out.sort();
    out
}

/// The Workshop binfab preview against Python's (`gen_decode_fixtures.py view`).
#[test]
fn view_matches_python() {
    let dir = testdata().join("view");
    let mut bad = Vec::new();
    view_case(&dir.join("binfabs"), &dir.join("expected"), &mut bad);
    assert!(bad.is_empty(), "differ: {bad:?}");
    assert_eq!(view_text("x", &vec![0u8; super::view::MAX_BYTES + 1]), "!This file is too large to lay out.");
}

/// A larger sample: `KIWI_DECODE_VIEW=<binfabs dir>` and `KIWI_DECODE_VIEW_GOLD=<its view output>`.
#[test]
#[ignore]
fn view_sample_matches_python() {
    let (Ok(src), Ok(gold)) = (std::env::var("KIWI_DECODE_VIEW"), std::env::var("KIWI_DECODE_VIEW_GOLD")) else {
        panic!("set KIWI_DECODE_VIEW and KIWI_DECODE_VIEW_GOLD");
    };
    let mut bad = Vec::new();
    view_case(Path::new(&src), Path::new(&gold), &mut bad);
    assert!(bad.is_empty(), "{} differ: {:?}", bad.len(), &bad[..bad.len().min(20)]);
}

#[test]
fn defers_unported_runs_to_python() {
    use axum::http::{Method, Uri};
    let run = |q: &str| super::routes::defers("/admin/gamedata/run", &Method::POST, &q.parse::<Uri>().unwrap());
    assert!(run("/admin/gamedata/run"));
    assert!(run("/admin/gamedata/run?name=insignia"));
    assert!(run("/admin/gamedata/run?name=titles&name=insignia"));
    assert!(!run("/admin/gamedata/run?name=titles"));
    assert!(!run("/admin/gamedata/run?name=nope"));
    assert!(!super::routes::defers("/admin/gamedata", &Method::GET, &"/admin/gamedata".parse::<Uri>().unwrap()));
}

#[test]
fn run_rows_round_trip_and_report() {
    use super::runner::{Run, run_dto, state_of};
    use chrono::TimeZone;
    let started = chrono::Utc.with_ymd_and_hms(2026, 10, 1, 11, 0, 0).unwrap() + chrono::Duration::milliseconds(123);
    let mut run = Run {
        id: mongodb::bson::oid::ObjectId::new(),
        name: "titles".into(),
        trigger: "manual".into(),
        ordinal: 7,
        version_tag: None,
        code_version: "abc".into(),
        started_at: started,
        finished_at: None,
        ok: None,
        entries: None,
        error: None,
        traceback: None,
    };
    let d = run.to_doc();
    let keys: Vec<&str> = d.keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        [
            "_id",
            "name",
            "trigger",
            "ordinal",
            "version_tag",
            "code_version",
            "started_at",
            "finished_at",
            "ok",
            "entries",
            "error",
            "traceback"
        ]
    );
    assert_eq!(Run::from_doc(&d), Some(run.clone()));
    assert_eq!(state_of(Some(&run), started + chrono::Duration::seconds(60)), "running");
    assert_eq!(state_of(Some(&run), started + chrono::Duration::seconds(1801)), "interrupted");
    run.finished_at = Some(started + chrono::Duration::milliseconds(1234));
    run.ok = Some(true);
    run.entries = Some(640);
    assert_eq!(state_of(Some(&run), started), "ok");
    let dto = run_dto(Some(&run));
    assert_eq!(dto["started_at"], "2026-10-01T11:00:00.123000+00:00");
    assert_eq!(dto["finished_at"], "2026-10-01T11:00:01.357000+00:00");
    assert_eq!(dto["duration_ms"], 1234);
    assert_eq!(state_of(None, started), "never");
}

#[test]
fn tree_lookups_ignore_case() {
    let t = super::tree::mem_tree(vec![
        ("prefabs/Class/Knight.binfab", b"k".to_vec()),
        ("prefabs/class/bard.binfab", b"b".to_vec()),
        ("prefabs/class/sub/x.binfab", b"x".to_vec()),
        ("languages/en/ui.binfab", b"u".to_vec()),
    ]);
    assert_eq!(t.read("prefabs/class/knight.binfab"), Some(b"k".to_vec()));
    assert_eq!(
        t.files("prefabs/class/", ".binfab"),
        ["prefabs/class/bard.binfab", "prefabs/Class/Knight.binfab", "prefabs/class/sub/x.binfab"]
    );
    assert_eq!(t.dirs("prefabs/class"), ["sub"]);
    assert_eq!(t.only(&["languages/"]).len(), 1);
}

#[test]
fn atomic_write_replaces() {
    let dir = std::env::temp_dir().join(format!("kiwi-decode-write-{}", std::process::id()));
    let target = dir.join("titles.json");
    super::runner::write(&target, "[1]").unwrap();
    super::runner::write(&target, "[2]").unwrap();
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "[2]");
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
    let _ = std::fs::remove_dir_all(dir);
}

/// The whole extracted client against Python's full outputs:
/// `KIWI_DECODE_GAME=<tree> KIWI_DECODE_GOLD=<gen_decode_fixtures.py full output>`.
#[test]
#[ignore]
fn full_tree_matches_python() {
    let (Ok(game), Ok(gold)) = (std::env::var("KIWI_DECODE_GAME"), std::env::var("KIWI_DECODE_GOLD")) else {
        panic!("set KIWI_DECODE_GAME and KIWI_DECODE_GOLD");
    };
    let only: Vec<String> =
        std::env::var("KIWI_DECODE_ONLY").map(|s| s.split(',').map(str::to_owned).collect()).unwrap_or_default();
    let prefixes: Vec<&str> = {
        let mut p: Vec<&str> = DECODERS.iter().flat_map(|d| d.prefixes.iter().copied()).collect();
        p.sort();
        p.dedup();
        p
    };
    let tree = dir_tree(Path::new(&game), &prefixes);
    let out = Path::new(&gold).join("_rust");
    let _ = std::fs::create_dir_all(&out);
    let mut bad = Vec::new();
    for d in DECODERS {
        if !only.is_empty() && !only.iter().any(|o| o == d.name) {
            continue;
        }
        let t0 = std::time::Instant::now();
        let Some(result) = d.run(&tree, &baseline()) else { continue };
        let got = match result {
            Ok(data) => d.dump(&data),
            Err(e) => format!("!{e}"),
        };
        let want = std::fs::read(Path::new(&gold).join(d.output)).unwrap_or_default();
        let same = got.as_bytes() == want.as_slice();
        eprintln!("{:20} {:6} {:.1}s", d.name, if same { "same" } else { "DIFF" }, t0.elapsed().as_secs_f64());
        if !same {
            std::fs::write(out.join(d.output), &got).expect("write");
            bad.push(d.name);
        }
    }
    assert!(bad.is_empty(), "differ: {bad:?}");
}

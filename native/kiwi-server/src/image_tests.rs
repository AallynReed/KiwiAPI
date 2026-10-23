//! The Dockerfile against the paths the binaries read: the `rust` image must
//! carry every file they open under assets/ (at the same path below /app) and
//! put OCR's library and models where the engine looks.

use std::{collections::HashMap, path::PathBuf};

use crate::{gamedata, i18n, media::audio, ocr::engine, og, rigs, web::geo};

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(name: &str) -> String {
    std::fs::read_to_string(repo().join(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
}

/// Instructions with continuations joined and comments dropped.
fn instructions(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        match line.strip_suffix('\\') {
            Some(body) => {
                cur.push_str(body);
                cur.push(' ');
            }
            None => {
                cur.push_str(line);
                out.push(std::mem::take(&mut cur));
            }
        }
    }
    out
}

struct Dockerfile {
    args: HashMap<String, String>,
    stages: HashMap<String, Vec<String>>,
}

fn dockerfile() -> Dockerfile {
    let mut args = HashMap::new();
    let mut stages: HashMap<String, Vec<String>> = HashMap::new();
    let mut current: Option<String> = None;
    for ins in instructions(&read("Dockerfile")) {
        let words: Vec<&str> = ins.split_whitespace().collect();
        if words[0].eq_ignore_ascii_case("FROM") {
            let name = words.iter().position(|w| w.eq_ignore_ascii_case("AS")).map(|i| words[i + 1]);
            current = Some(name.expect("every stage is named").to_ascii_lowercase());
            continue;
        }
        match &current {
            Some(stage) => stages.entry(stage.clone()).or_default().push(ins),
            None if words[0] == "ARG" => {
                let (k, v) = words[1].split_once('=').expect("global ARG with a default");
                args.insert(k.to_owned(), v.to_owned());
            }
            None => panic!("{ins} before the first FROM"),
        }
    }
    Dockerfile { args, stages }
}

/// `(sources, destination)` of each COPY that reads the build context.
fn copies(stage: &[String]) -> Vec<(Vec<String>, String)> {
    stage
        .iter()
        .filter(|i| i.starts_with("COPY "))
        .filter(|i| !i.contains("--from="))
        .map(|i| {
            let mut words: Vec<String> = i.split_whitespace().skip(1).map(str::to_owned).collect();
            let dest = words.pop().expect("COPY destination");
            (words, dest)
        })
        .collect()
}

fn env(stage: &[String]) -> HashMap<String, String> {
    stage
        .iter()
        .filter_map(|i| i.strip_prefix("ENV "))
        .flat_map(|rest| rest.split_whitespace().filter_map(|kv| kv.split_once('=')))
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect()
}

/// A COPY source covers `path`: the same file, a directory above it, or `dir/*.ext`.
fn covers(src: &str, path: &str) -> bool {
    let src = src.trim_end_matches('/');
    if let Some((dir, ext)) = src.split_once("/*") {
        return path
            .strip_prefix(dir)
            .and_then(|rest| rest.strip_prefix('/'))
            .is_some_and(|file| !file.contains('/') && file.ends_with(ext));
    }
    path == src || path.starts_with(&format!("{src}/"))
}

fn parent(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(dir, _)| dir)
}

#[test]
fn copy_sources_exist_and_are_not_ignored() {
    let ignored: Vec<String> = read(".dockerignore")
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#') && !l.contains('*'))
        .map(|l| l.trim_end_matches('/').to_owned())
        .collect();
    let df = dockerfile();
    for (name, stage) in &df.stages {
        for (sources, _) in copies(stage) {
            for src in sources {
                let found = match src.split_once("/*") {
                    Some((dir, ext)) => std::fs::read_dir(repo().join(dir))
                        .map(|rd| rd.flatten().any(|e| e.file_name().to_string_lossy().ends_with(ext)))
                        .unwrap_or(false),
                    None => repo().join(&src).exists(),
                };
                assert!(found, "stage {name}: COPY source {src} does not exist");
                for pat in &ignored {
                    assert!(!covers(pat, src.trim_end_matches('/')), "stage {name}: {src} is in .dockerignore ({pat})");
                }
            }
        }
    }
}

#[test]
fn rust_image_carries_every_runtime_file() {
    let df = dockerfile();
    let stage = &df.stages["rust"];
    assert!(stage.iter().any(|i| i == "WORKDIR /app"), "the binaries resolve assets/ from /app");
    let copies = copies(stage);
    for (sources, dest) in &copies {
        for src in sources {
            let src = src.trim_end_matches('/');
            // A directory's contents land in dest; a file (or glob) lands in dest/ by name.
            let same_path = if repo().join(src).is_dir() || !dest.ends_with('/') {
                dest.trim_end_matches('/') == src
            } else {
                dest.trim_end_matches('/') == parent(src)
            };
            assert!(same_path, "COPY {src} {dest}: not at the same path below /app");
        }
    }
    let needed = [
        gamedata::BASELINE_DIR,
        rigs::RIG_DIR,
        audio::CODEBOOKS_FILE,
        i18n::BOT_LOCALES,
        og::i18n::BOT_DIR,
        geo::PAGE_PATH,
    ];
    for path in needed {
        assert!(path.starts_with("assets/"), "{path} is outside assets/");
        assert!(
            copies.iter().any(|(sources, _)| sources.iter().any(|s| covers(s, path))),
            "the rust image does not COPY {path}"
        );
    }
}

#[test]
fn ocr_and_tzdata_paths_match_the_image() {
    let df = dockerfile();
    let rust = env(&df.stages["rust"]);
    let version = &df.args["ORT_VERSION"];
    assert_eq!(rust["KIWI_ORT_DYLIB"].replace("${ORT_VERSION}", version), engine::DEFAULT_DYLIB);
    assert_eq!(rust["KIWI_OCR_MODEL_DIR"], engine::DEFAULT_MODELS);
    let fetch = df.stages["fetch"].join("\n");
    for model in [engine::DET, engine::CLS, engine::REC] {
        assert!(fetch.contains(&format!("  {model}\"")), "{model} is not fetched with a checksum");
    }
    let copied_from_fetch: Vec<&str> = df.stages["rust"]
        .iter()
        .filter(|i| i.starts_with("COPY --from=fetch "))
        .filter_map(|i| i.split_whitespace().last())
        .collect();
    for key in ["KIWI_OCR_MODEL_DIR", "KIWI_TZDATA_DIR"] {
        assert!(copied_from_fetch.contains(&rust[key].as_str()), "{key} is not a fetched directory");
    }
    assert!(copied_from_fetch.contains(&parent(engine::DEFAULT_DYLIB)), "the onnxruntime lib is not copied");
}

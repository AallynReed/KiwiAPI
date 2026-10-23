//! The Mod Workshop's placement rules and header handling
//! (`app/trove/mods_hub/trove_layout.py`, `workshop.py`, and the Mods Hub's
//! `_inject_config` / `pack_preview`), over paths and bytes already in memory.

use std::collections::HashMap;

use serde_json::{Map, Value, json};

use crate::bpedit::py::{str_of, truthy};
use crate::pystr;

pub const TROVE_DIRECTORIES: [&str; 11] = [
    "audio",
    "blueprints",
    "fonts",
    "languages",
    "models",
    "movies",
    "particles",
    "prefabs",
    "shadersunified",
    "textures",
    "ui",
];

pub const COMPILE_IGNORED_EXTENSIONS: [&str; 14] =
    [".tfi", ".tfa", ".exe", ".dll", ".tmod", ".zip", ".cfg", ".txt", ".log", ".ini", ".toml", ".json", ".xml", ".dat"];

pub const LIVE_BRANCH: &str = "live-us";
pub const MAX_FILES: usize = 4000;
pub const MAX_UNPACKED_BYTES: u64 = 192 * 1024 * 1024;
pub const MAX_WRAPPER_DEPTH: usize = 4;
pub const MAX_PREVIEW_BYTES: usize = 8 * 1024 * 1024;
const CONFIG_MAX_BYTES: usize = 256 * 1024;
const JUNK_NAMES: [&str; 4] = [".ds_store", "thumbs.db", "desktop.ini", ".gitkeep"];
const JUNK_DIRS: [&str; 3] = ["__macosx/", ".git/", ".svn/"];

/// `{filename.lower(): first archived path}`.
pub type GameMap = HashMap<String, String>;

fn lower(s: &str) -> String {
    s.to_lowercase()
}

fn tail(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// `_ext(path)`.
pub fn ext(path: &str) -> String {
    let name = tail(path);
    match name.split_once('.') {
        Some(_) => format!(".{}", lower(name.rsplit('.').next().unwrap_or_default())),
        None => String::new(),
    }
}

pub fn is_compilable(path: &str) -> bool {
    let parts: Vec<&str> = path.split('/').collect();
    parts.len() >= 2
        && TROVE_DIRECTORIES.contains(&lower(parts[0]).as_str())
        && !COMPILE_IGNORED_EXTENSIONS.contains(&ext(path).as_str())
}

pub fn skip_reason(path: &str) -> String {
    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() < 2 {
        return "root file (only files inside a Trove folder are compiled)".into();
    }
    if !TROVE_DIRECTORIES.contains(&lower(parts[0]).as_str()) {
        return format!("'{}' is not a Trove folder", parts[0]);
    }
    let e = ext(path);
    if COMPILE_IGNORED_EXTENSIONS.contains(&e.as_str()) {
        return format!("'{e}' files aren't included in a build");
    }
    "ignored".into()
}

/// `find_misplaced(paths, game_map)` as `{path: expected}`.
pub fn find_misplaced(paths: &[String], game_map: &GameMap) -> HashMap<String, String> {
    let mut out = HashMap::new();
    if game_map.is_empty() {
        return out;
    }
    for p in paths {
        if let Some(expected) = game_map.get(&lower(tail(p)))
            && lower(expected) != lower(p)
        {
            out.insert(p.clone(), expected.clone());
        }
    }
    out
}

/// `nearest_path(candidates, hint)`.
pub fn nearest_path<'a>(candidates: &'a [String], hint: &str) -> Option<&'a String> {
    if candidates.is_empty() {
        return None;
    }
    if candidates.len() == 1 || hint.is_empty() {
        return candidates.iter().min();
    }
    let segs = |p: &str| -> Vec<String> {
        let low = lower(&p.replace('\\', "/"));
        let mut parts: Vec<String> = low.split('/').map(str::to_owned).collect();
        parts.pop();
        parts.into_iter().filter(|s| !s.is_empty()).collect()
    };
    let want: std::collections::HashSet<String> = segs(hint).into_iter().collect();
    let score = |p: &String| -> usize {
        let mine: std::collections::HashSet<String> = segs(p).into_iter().collect();
        want.intersection(&mine).count()
    };
    candidates.iter().min_by(|a, b| score(b).cmp(&score(a)).then_with(|| a.cmp(b)))
}

/// `norm_path(path)`.
pub fn norm_path(path: &str) -> String {
    let p = path.replace('\\', "/");
    p.split('/').filter(|s| !s.is_empty() && *s != "." && *s != "..").collect::<Vec<_>>().join("/")
}

/// `is_junk(path)`.
pub fn is_junk(path: &str) -> bool {
    let low = lower(path);
    JUNK_NAMES.contains(&tail(&low)) || JUNK_DIRS.iter().any(|d| low == d[..d.len() - 1] || low.starts_with(d))
}

/// `strip_wrapper(paths)`.
pub fn strip_wrapper(paths: &[String]) -> (String, Vec<String>) {
    let mut prefix = String::new();
    let mut current = paths.to_vec();
    for _ in 0..MAX_WRAPPER_DEPTH {
        let live: Vec<&String> = current.iter().filter(|p| !p.is_empty()).collect();
        if live.is_empty() {
            break;
        }
        let first = live[0].split('/').next().unwrap_or_default();
        if live.iter().any(|p| p.split('/').next().unwrap_or_default() != first) {
            break;
        }
        if TROVE_DIRECTORIES.contains(&lower(first).as_str()) {
            break;
        }
        if !live.iter().all(|p| p.contains('/')) {
            break;
        }
        prefix.push_str(first);
        prefix.push('/');
        current = current.iter().map(|p| p.split_once('/').map_or(p.clone(), |(_, rest)| rest.to_owned())).collect();
    }
    (prefix, current)
}

/// One `plan()` result. `mapping` is index -> final path, in index order.
#[derive(Clone, Debug)]
pub struct Plan {
    pub wrapper: String,
    pub entries: Vec<Map<String, Value>>,
    pub counts: [i64; 4],
    pub mapping: Vec<(usize, String)>,
    pub game_index_available: bool,
}

const STATUSES: [&str; 4] = ["ready", "moved", "misplaced", "skipped"];

impl Plan {
    pub fn packed(&self) -> usize {
        self.mapping.len()
    }

    pub fn count(&self, status: &str) -> i64 {
        STATUSES.iter().position(|s| *s == status).map_or(0, |i| self.counts[i])
    }

    pub fn final_for(&self, index: usize) -> Option<&String> {
        self.mapping.iter().find(|(i, _)| *i == index).map(|(_, f)| f)
    }

    /// The plan with `mapping` popped, keys in Python's order.
    pub fn to_map(&self) -> Map<String, Value> {
        let mut counts = Map::new();
        for (s, n) in STATUSES.iter().zip(self.counts) {
            counts.insert((*s).into(), n.into());
        }
        let mut m = Map::new();
        m.insert("wrapper".into(), self.wrapper.clone().into());
        m.insert("total".into(), self.entries.len().into());
        m.insert("entries".into(), Value::Array(self.entries.iter().cloned().map(Value::Object).collect()));
        m.insert("counts".into(), Value::Object(counts));
        m.insert("packed".into(), self.packed().into());
        m.insert("game_index_available".into(), self.game_index_available.into());
        m.insert("buildable".into(), (!self.mapping.is_empty()).into());
        m
    }
}

/// `plan(paths, fix=, keep=)` against an already-loaded game index.
pub fn plan(paths: &[String], fix: bool, keep: &[String], game_map: &GameMap) -> Plan {
    let originals: Vec<String> = paths.iter().map(|p| norm_path(p)).collect();
    let (wrapper, rel) = strip_wrapper(&originals);
    let keep: std::collections::HashSet<String> = keep.iter().map(|k| norm_path(k)).collect();
    let misplaced = find_misplaced(&rel, game_map);
    let mut entries = Vec::with_capacity(rel.len());
    let mut mapping = Vec::new();
    let mut taken: HashMap<String, String> = HashMap::new();
    for (i, (original, path)) in originals.iter().zip(&rel).enumerate() {
        let mut entry = Map::new();
        entry.insert("index".into(), i.into());
        entry.insert("path".into(), original.clone().into());
        entry.insert("name".into(), tail(path).into());
        let skipped = |mut e: Map<String, Value>, reason: String| {
            e.insert("status".into(), "skipped".into());
            e.insert("reason".into(), reason.into());
            e
        };
        if path.is_empty() || is_junk(path) {
            entries.push(skipped(entry, "archive housekeeping, not a mod file".into()));
            continue;
        }
        let expected = misplaced.get(path);
        if let Some(e) = expected {
            entry.insert("expected".into(), e.clone().into());
        }
        let moved = expected.is_some() && fix && !keep.contains(original);
        let fin = if moved { expected.cloned().unwrap_or_default() } else { path.clone() };
        if !is_compilable(&fin) {
            entries.push(skipped(entry, skip_reason(&fin)));
            continue;
        }
        if let Some(clash) = taken.get(&fin) {
            let reason = format!("'{clash}' already lands at {fin}");
            entries.push(skipped(entry, reason));
            continue;
        }
        taken.insert(fin.clone(), original.clone());
        mapping.push((i, fin.clone()));
        let status = if moved {
            "moved"
        } else if expected.is_some() {
            "misplaced"
        } else {
            "ready"
        };
        entry.insert("final".into(), fin.into());
        entry.insert("status".into(), status.into());
        entries.push(entry);
    }
    let mut counts = [0i64; 4];
    for e in &entries {
        let s = e.get("status").and_then(Value::as_str).unwrap_or_default();
        if let Some(i) = STATUSES.iter().position(|x| *x == s) {
            counts[i] += 1;
        }
    }
    Plan { wrapper, entries, counts, mapping, game_index_available: !game_map.is_empty() }
}

/// `preview(paths)`: the fixing plan, each would-be-moved entry carrying `alt`.
pub fn preview(paths: &[String], game_map: &GameMap) -> Plan {
    let mut main = plan(paths, true, &[], game_map);
    if !main.entries.iter().any(|e| e.contains_key("expected")) {
        return main;
    }
    let other = plan(paths, false, &[], game_map);
    for entry in &mut main.entries {
        if !entry.contains_key("expected") {
            continue;
        }
        let idx = entry.get("index").and_then(Value::as_u64).unwrap_or_default() as usize;
        let mut alt = Map::new();
        if let Some(o) = other.entries.get(idx) {
            for k in ["status", "final", "reason"] {
                if let Some(v) = o.get(k) {
                    alt.insert(k.into(), v.clone());
                }
            }
        }
        entry.insert("alt".into(), Value::Object(alt));
    }
    main
}

pub fn config_candidates(paths: &[String]) -> Vec<String> {
    let names: Vec<String> = paths.iter().map(|p| norm_path(p)).collect();
    if !names.iter().any(|p| lower(p).ends_with(".swf")) {
        return Vec::new();
    }
    names.into_iter().filter(|p| lower(p).ends_with(".cfg")).collect()
}

pub fn preview_extension(path: &str) -> Option<&'static str> {
    match ext(path).as_str() {
        ".png" => Some("png"),
        ".jpg" | ".jpeg" => Some("jpg"),
        ".gif" => Some("gif"),
        _ => None,
    }
}

pub fn preview_candidates(paths: &[String]) -> Vec<String> {
    paths.iter().map(|p| norm_path(p)).filter(|p| preview_extension(p).is_some()).collect()
}

/// `str(value or "")` of a `json.loads` value.
pub fn py_str(v: Option<&Value>) -> String {
    match v {
        Some(v) if truthy(v) => str_of(v),
        _ => String::new(),
    }
}

fn strip_dots(s: &str) -> &str {
    s.trim_matches('.')
}

fn sanitize_title(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .filter(|&c| !matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' | '\0'..='\x1f'))
        .collect();
    pystr::prefix(strip_dots(pystr::strip(&cleaned)), 120).to_owned()
}

/// `safe_title(value)`.
pub fn safe_title(value: &str) -> String {
    let t = sanitize_title(value);
    if t.is_empty() { "My Mod".into() } else { t }
}

/// The Mods Hub's `_safe_filename(name)`.
pub fn safe_filename(name: &str) -> String {
    let t = sanitize_title(name);
    if t.is_empty() { "mod".into() } else { t }
}

/// `_clean(value, limit, multiline=)` on an already-`str()`ed value.
fn clean(text: &str, limit: usize, multiline: bool) -> String {
    let mut t: String =
        text.chars().filter(|&c| !matches!(c, '\0'..='\x08' | '\x0b' | '\x0c' | '\x0e'..='\x1f')).collect();
    if !multiline {
        t = t.replace(['\n', '\r'], " ");
    }
    pystr::prefix(pystr::strip(&t), limit).to_owned()
}

/// An ordered `dict[str, str]` header.
pub type Props = Vec<(String, String)>;

pub fn prop<'a>(props: &'a Props, key: &str) -> Option<&'a str> {
    props.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

pub fn set_prop(props: &mut Props, key: &str, value: String) {
    match props.iter_mut().find(|(k, _)| k == key) {
        Some(slot) => slot.1 = value,
        None => props.push((key.to_owned(), value)),
    }
}

/// What escaped the engine: the 400 text, or a Python exception nobody catches.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fail {
    Bad(String),
    Unhandled(String),
}

/// `clean_properties(raw)`; `raw` is the `properties` value (`None` = `{}`).
pub fn clean_properties(raw: Option<&Value>) -> Result<Props, Fail> {
    let empty = Map::new();
    let raw = match raw {
        Some(v) if truthy(v) => v.as_object().ok_or_else(|| Fail::Unhandled("AttributeError: no .get".into()))?,
        _ => &empty,
    };
    let mut props: Props = vec![("title".into(), safe_title(&py_str(raw.get("title"))))];
    for (key, limit) in [("author", 200), ("modVersion", 40)] {
        let v = clean(&py_str(raw.get(key)), limit, false);
        if !v.is_empty() {
            props.push((key.into(), v));
        }
    }
    let notes = clean(&py_str(raw.get("notes")), 4000, true);
    if !notes.is_empty() {
        props.push(("notes".into(), notes));
    }
    let raw_tags: Vec<String> = match raw.get("tags") {
        Some(v) if truthy(v) => match v {
            Value::String(s) => s.split(',').map(str::to_owned).collect(),
            Value::Array(a) => a.iter().map(|x| py_str(Some(x))).collect(),
            Value::Object(o) => o.keys().cloned().collect(),
            _ => return Err(Fail::Unhandled("TypeError: tags not iterable".into())),
        },
        _ => Vec::new(),
    };
    let joined = raw_tags.iter().map(|t| clean(t, 40, false)).filter(|t| !t.is_empty()).collect::<Vec<_>>().join(", ");
    let tags = pystr::prefix(&joined, 400).to_owned();
    if !tags.is_empty() {
        props.push(("tags".into(), tags));
    }
    let preview = lower(&norm_path(&clean(&py_str(raw.get("previewPath")), 260, false)));
    if !preview.is_empty() {
        props.push(("previewPath".into(), preview));
    }
    let config = norm_path(&clean(&py_str(raw.get("configPath")), 260, false));
    if !config.is_empty() {
        props.push(("configPath".into(), config));
    }
    Ok(props)
}

fn norm_lower(p: &str) -> String {
    lower(p.replace('\\', "/").trim_start_matches('/'))
}

/// `_inject_config(files, props, data)`.
pub fn inject_config(files: &mut Vec<(String, Vec<u8>)>, props: &mut Props, data: Vec<u8>) -> Result<String, Fail> {
    if data.is_empty() {
        return Err(Fail::Bad("The config file is empty.".into()));
    }
    if data.len() > CONFIG_MAX_BYTES {
        return Err(Fail::Bad(format!("A config file can be at most {} KB.", CONFIG_MAX_BYTES / 1024)));
    }
    if data.contains(&0) {
        return Err(Fail::Bad("That doesn't look like a text config file.".into()));
    }
    if !files.iter().any(|(p, _)| lower(p).ends_with(".swf")) {
        return Err(Fail::Bad("Only a mod with a Flash UI (.swf) can carry a config file.".into()));
    }
    let title = pystr::strip(prop(props, "title").unwrap_or_default()).to_owned();
    if title.is_empty() {
        return Err(Fail::Bad("This build has no title in its header to name the config after.".into()));
    }
    let path = format!("ui/{}.cfg", safe_filename(&title));
    let low = lower(&path);
    files.retain(|(p, _)| norm_lower(p) != low);
    files.push((path.clone(), data));
    set_prop(props, "configPath", path.clone());
    Ok(path)
}

/// `pack_preview(files, props, data, ext, stem)`.
pub fn pack_preview(
    files: &mut Vec<(String, Vec<u8>)>,
    props: &mut Props,
    data: Vec<u8>,
    ext: &str,
    stem: &str,
) -> String {
    let path = lower(&format!("ui/{}.{ext}", safe_filename(stem)));
    files.retain(|(p, _)| norm_lower(p) != path);
    files.push((path.clone(), data));
    set_prop(props, "previewPath", path.clone());
    path
}

pub const NOT_BUILDABLE: &str = "Nothing here would load in-game. Trove only reads files inside its own folders (blueprints/, ui/, prefabs/, textures/, …) - put the mod's files under one of those, or let the workshop place them for you.";

/// What `build_mod` hands `build_tmod`: the header, the packed files and the plan.
pub struct Built {
    pub props: Props,
    pub packed: Vec<(String, Vec<u8>)>,
    pub plan: Plan,
}

pub struct BuildArgs<'a> {
    pub properties: Option<&'a Value>,
    pub fix: bool,
    pub keep: &'a [String],
    pub config_path: &'a str,
    pub preview_path: &'a str,
    pub attached: Vec<(String, Vec<u8>)>,
}

/// `build_mod(...)` up to the `build_tmod` call.
pub fn build_mod(files: Vec<(String, Vec<u8>)>, args: BuildArgs<'_>, game_map: &GameMap) -> Result<Built, Fail> {
    let names: Vec<String> = files.iter().map(|(p, _)| p.clone()).collect();
    let plan = plan(&names, args.fix, args.keep, game_map);
    if plan.mapping.is_empty() {
        return Err(Fail::Bad(NOT_BUILDABLE.into()));
    }
    let mut packed: Vec<(String, Vec<u8>)> =
        plan.mapping.iter().map(|(i, fin)| (fin.clone(), files[*i].1.clone())).collect();
    let mut props = clean_properties(args.properties)?;
    let available: std::collections::HashSet<&str> = names.iter().map(String::as_str).collect();
    let carried = |key: &str| prop(&props, key).filter(|v| available.contains(v)).unwrap_or_default().to_owned();
    let config_path =
        Some(norm_path(args.config_path)).filter(|s| !s.is_empty()).unwrap_or_else(|| carried("configPath"));
    let preview_path =
        Some(norm_path(args.preview_path)).filter(|s| !s.is_empty()).unwrap_or_else(|| carried("previewPath"));
    let pick = |path: &str, what: &str| -> Result<Vec<u8>, Fail> {
        args.attached
            .iter()
            .chain(files.iter())
            .find(|(p, _)| p == path)
            .map(|(_, d)| d.clone())
            .ok_or_else(|| Fail::Bad(format!("No file '{path}' to pack as the {what}.")))
    };
    if !config_path.is_empty() {
        let data = pick(&config_path, "config")?;
        inject_config(&mut packed, &mut props, data)?;
    }
    if !preview_path.is_empty() {
        let Some(ext) = preview_extension(&preview_path) else {
            return Err(Fail::Bad(
                "A preview image has to be a PNG, JPG or GIF - Trove can't show anything else.".into(),
            ));
        };
        let image = pick(&preview_path, "preview")?;
        if image.is_empty() {
            return Err(Fail::Bad("That preview image is empty.".into()));
        }
        if image.len() > MAX_PREVIEW_BYTES {
            return Err(Fail::Bad(format!("A preview image can be at most {} MB.", MAX_PREVIEW_BYTES / (1024 * 1024))));
        }
        let title = prop(&props, "title").unwrap_or_default().to_owned();
        pack_preview(&mut packed, &mut props, image, ext, &title);
    }
    let finals: std::collections::HashSet<String> = packed.iter().map(|(p, _)| lower(p)).collect();
    for key in ["previewPath", "configPath"] {
        if let Some(v) = prop(&props, key)
            && !v.is_empty()
            && !finals.contains(&lower(v))
        {
            props.retain(|(k, _)| k != key);
        }
    }
    Ok(Built { props, packed, plan })
}

/// `_preview_path(properties)`.
pub fn preview_path(props: &Map<String, Value>) -> String {
    let v = props.get("previewPath").and_then(Value::as_str).unwrap_or_default();
    lower(pystr::strip(v))
}

/// `_declared_config_path(props)`.
pub fn declared_config_path(props: &Map<String, Value>) -> String {
    let declared =
        props.iter().find(|(k, _)| lower(k) == "configpath").and_then(|(_, v)| v.as_str()).unwrap_or_default();
    lower(pystr::strip(declared.replace('\\', "/").trim_start_matches('/')))
}

/// `int(properties.get("flags", 0) or 0)`, 0 on a `ValueError`.
pub fn flags_of(props: &Map<String, Value>) -> i64 {
    match props.get("flags").and_then(Value::as_str) {
        Some(s) if !s.is_empty() => crate::pynum::int(s).unwrap_or(0),
        _ => 0,
    }
}

/// `tags_from_flags(flags)` for a possibly negative int.
pub fn categories(flags: i64) -> Value {
    json!(crate::mod_categories::tags_from_flags(flags as u64))
}

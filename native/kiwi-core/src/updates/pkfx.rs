//! `app/trove/mods_hub/vfx.py`: the PopcornFX `.pkfx` reference parser and
//! the media types its assets are served with.

use std::sync::LazyLock;

use regex::Regex;

use crate::store::py::splitlines;

pub const ASSET_EXTS: [&str; 11] = ["dds", "png", "tga", "pkat", "pkmm", "fbx", "pkfx", "pkma", "pkml", "pkcf", "pkan"];

static REF_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!(r#"(?i)"([^"\n]+\.(?:{}))""#, ASSET_EXTS.join("|"))).expect("ref regex"));
static ANIM_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\bAnimResource\b").expect("anim regex"));
static HEADER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(C[A-Za-z0-9_]+)[ \t]+\$LOCAL\$").expect("header regex"));

/// `re.sub(r"\.[^.\\/]+$", ".pkan", r)`.
fn to_pkan(r: &str) -> String {
    match r.rfind('.') {
        Some(p) if p + 1 < r.len() && !r[p + 1..].contains(['/', '\\']) => format!("{}.pkan", &r[..p]),
        _ => r.to_owned(),
    }
}

/// `extract_refs(text)`: quoted asset paths, editor-only objects skipped,
/// an `AnimResource` mesh swapped for its `.pkan`, deduped case-insensitively.
pub fn extract_refs(text: &str) -> Vec<String> {
    let mut keys = std::collections::HashSet::new();
    let mut out = Vec::new();
    let mut current: Option<String> = None;
    for line in splitlines(text) {
        if let Some(h) = HEADER_RE.captures(line) {
            current = Some(h[1].to_owned());
            continue;
        }
        if current.as_deref().is_some_and(|c| c.starts_with("CNEdEditor")) {
            continue;
        }
        let is_anim = ANIM_RE.is_match(line);
        for m in REF_RE.captures_iter(line) {
            let r = &m[1];
            if r.starts_with("$LOCAL$") {
                continue;
            }
            let r = if is_anim { to_pkan(r) } else { r.to_owned() };
            if keys.insert(r.to_lowercase()) {
                out.push(r);
            }
        }
    }
    out
}

/// `basename(path)`.
pub fn basename(path: &str) -> String {
    let p = path.replace('\\', "/");
    p.rsplit('/').next().unwrap_or_default().to_owned()
}

/// `media_type_for(path)`.
pub fn media_type_for(path: &str) -> &'static str {
    let ext = if path.contains('.') {
        let b = basename(path);
        b.rsplit('.').next().unwrap_or_default().to_lowercase()
    } else {
        String::new()
    };
    match ext.as_str() {
        "dds" => "image/vnd.ms-dds",
        "png" => "image/png",
        "tga" => "image/x-tga",
        "pkat" | "pkfx" | "pkan" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

/// `ref.lower().endswith(_ASSET_SUFFIXES)`.
pub fn is_asset(r: &str) -> bool {
    let low = r.to_lowercase();
    ASSET_EXTS.iter().any(|e| low.strip_suffix(e).is_some_and(|rest| rest.ends_with('.')))
}

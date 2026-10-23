//! The pure halves of `app/trove/updates/read.py`: one directory level from a
//! flat path list, and how the file viewer classifies a path.

use std::collections::HashMap;

pub const VIEW_MAX_BYTES: i64 = 1024 * 1024;
const IMAGE_EXTS: [&str; 8] = ["png", "jpg", "jpeg", "gif", "webp", "bmp", "ico", "apng"];
const OWN_VIEWER: [&str; 5] = ["dds", "blueprint", "swf", "bnk", "pkfx"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub path: String,
    pub size: i64,
    pub last_ordinal: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Child {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub file_count: i64,
    pub size: i64,
    pub last_ordinal: i64,
}

/// `directory_listing(entries, prefix)`: directories first, then files, each
/// by name.
pub fn directory_listing(rows: &[Row], prefix: &str) -> Vec<Child> {
    let mut children: HashMap<String, Child> = HashMap::new();
    for e in rows {
        let Some(rest) = e.path.strip_prefix(prefix) else { continue };
        if rest.is_empty() {
            continue;
        }
        let slash = rest.find('/');
        let name = slash.map_or(rest, |s| &rest[..s]);
        let child = children.entry(name.to_owned()).or_insert_with(|| Child {
            name: name.to_owned(),
            path: format!("{prefix}{name}"),
            is_dir: slash.is_some(),
            file_count: 0,
            size: 0,
            last_ordinal: 0,
        });
        if slash.is_some() {
            child.is_dir = true;
            child.path = format!("{prefix}{name}/");
        }
        child.file_count += 1;
        child.size += e.size;
        child.last_ordinal = child.last_ordinal.max(e.last_ordinal);
    }
    let mut out: Vec<Child> = children.into_values().collect();
    out.sort_by(|x, y| (!x.is_dir, &x.name).cmp(&(!y.is_dir, &y.name)));
    out
}

/// What the viewer shows without reading the blob, or `None` when it must
/// read it (text, binary or missing).
pub fn view_kind(path: &str, size: i64) -> Option<&'static str> {
    let base = path.rsplit('/').next().unwrap_or_default();
    let ext =
        if base.contains('.') { path.rsplit('.').next().unwrap_or_default().to_lowercase() } else { String::new() };
    if IMAGE_EXTS.contains(&ext.as_str()) {
        return Some("image");
    }
    if let Some(k) = OWN_VIEWER.iter().find(|k| **k == ext) {
        return Some(k);
    }
    (size > VIEW_MAX_BYTES).then_some("too_large")
}

/// The viewer's verdict on a blob it read: `Some(text)` for UTF-8 without
/// NUL bytes, else binary.
pub fn view_text(data: &[u8]) -> Option<&str> {
    if data.contains(&0) {
        return None;
    }
    std::str::from_utf8(data).ok()
}

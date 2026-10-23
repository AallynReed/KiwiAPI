//! The game-file archiver's pure parts (`app/trove/updates/cdn.py`,
//! `archive.py`, `diff.py`): the CDN pointer and manifest parsers, the URL
//! builders, the `.tfi` index reader, the `.tfa` inflate and the two-level diff.
//! Error texts are Python's `str(exc)`, since they land in `last_error`.

use std::collections::{HashMap, HashSet};

use regex::Regex;

use crate::{pynum, pystr::strip, zlib};

/// `cdn.BRANCHES`: timeline -> bootstrap pointer file, in probe order.
pub const BRANCHES: [(&str, &str); 2] = [("live-us", "kiwi-live-us.txt"), ("pts", "kiwi-pts.txt")];

/// An insertion-ordered map with Python `dict` semantics: re-inserting a key
/// replaces its value and keeps its first position.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Ordered<V> {
    items: Vec<(String, V)>,
    index: HashMap<String, usize>,
}

impl<V> Ordered<V> {
    pub fn new() -> Self {
        Ordered { items: Vec::new(), index: HashMap::new() }
    }

    pub fn insert(&mut self, key: String, value: V) {
        match self.index.get(&key) {
            Some(&i) => self.items[i].1 = value,
            None => {
                self.index.insert(key.clone(), self.items.len());
                self.items.push((key, value));
            }
        }
    }

    pub fn get(&self, key: &str) -> Option<&V> {
        self.index.get(key).map(|&i| &self.items[i].1)
    }

    pub fn contains_key(&self, key: &str) -> bool {
        self.index.contains_key(key)
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &V)> {
        self.items.iter().map(|(k, v)| (k, v))
    }

    pub fn keys(&self) -> impl Iterator<Item = &String> {
        self.items.iter().map(|(k, _)| k)
    }

    pub fn values(&self) -> impl Iterator<Item = &V> {
        self.items.iter().map(|(_, v)| v)
    }
}

impl<V> FromIterator<(String, V)> for Ordered<V> {
    fn from_iter<I: IntoIterator<Item = (String, V)>>(iter: I) -> Self {
        let mut m = Ordered::new();
        for (k, v) in iter {
            m.insert(k, v);
        }
        m
    }
}

// --- CDN layers -------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pointer {
    pub version: String,
    pub content_path: String,
    pub motd: String,
}

/// `parse_pointer`; Err is the `CdnError` text.
pub fn parse_pointer(text: &str) -> Result<Pointer, String> {
    let fields: Vec<&str> = strip(text).split('|').collect();
    if fields.len() < 2 || strip(fields[0]).is_empty() || strip(fields[1]).is_empty() {
        return Err("malformed bootstrap pointer".into());
    }
    let cp = strip(fields[1]);
    Ok(Pointer {
        version: strip(fields[0]).to_owned(),
        content_path: cp.strip_prefix("content/").unwrap_or(cp).to_owned(),
        motd: fields.get(3).map_or("", |f| strip(f)).to_owned(),
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestEntry {
    pub path: String,
    pub sha1: String,
    pub size: i64,
}

/// `parse_manifest`: the `version` tag and every well-formed `path:sha1:size`
/// line (`rsplit(':', 2)`, `int()` sizes); Err is the `CdnError` text.
pub fn parse_manifest(text: &str) -> Result<(String, Vec<ManifestEntry>), String> {
    let text = text.replace("\r\n", "\n");
    let mut lines = text.split('\n');
    let first = lines.next().unwrap_or("");
    let Some(version) = first.strip_prefix("version ") else {
        return Err("manifest missing 'version' line".into());
    };
    let mut entries = Vec::new();
    for line in lines {
        let line = strip(line);
        if line.is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.rsplitn(3, ':').collect();
        let [size, sha1, path] = parts[..] else { continue };
        let Some(size) = pynum::int(size) else { continue };
        entries.push(ManifestEntry { path: path.replace('\\', "/"), sha1: sha1.to_owned(), size });
    }
    Ok((strip(version).to_owned(), entries))
}

pub fn pointer_url(base: &str, prefix: &str, pointer_file: &str) -> String {
    format!("{base}{prefix}/public/{pointer_file}")
}

pub fn manifest_url(base: &str, prefix: &str, content_path: &str, version: &str) -> String {
    format!("{base}{prefix}/content/{content_path}/{version}.manifest")
}

pub fn file_url(base: &str, prefix: &str, content_path: &str, path: &str, sha1: &str) -> String {
    format!("{base}{prefix}/content/{content_path}/recovery/{}?sha1={sha1}", quote(path))
}

/// `urllib.parse.quote(s)` (safe `/`).
pub fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-' | b'~' | b'/') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

// --- .tfi / .tfa --------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TfiEntry {
    pub name: String,
    pub archive_index: u32,
    pub offset: u32,
    pub size: u32,
    pub fnv_hash: u32,
}

/// `troveio.read_leb128`: masked to 32 bits; Err is the exception text.
fn read_leb128(buf: &[u8], mut pos: usize) -> Result<(u32, usize), &'static str> {
    let mut result: u64 = 0;
    let mut shift = 0u32;
    loop {
        let byte = *buf.get(pos).ok_or("index out of range")?;
        pos += 1;
        result |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok((result as u32, pos));
        }
        shift += 7;
        if shift >= 64 {
            return Err("varint too long");
        }
    }
}

/// `str(UnicodeDecodeError)` of a strict `bytes.decode('utf-8')`.
pub fn utf8_error_text(bytes: &[u8], err: std::str::Utf8Error) -> String {
    let start = err.valid_up_to();
    let (end, reason) = match err.error_len() {
        None => (bytes.len(), "unexpected end of data"),
        Some(n) => {
            let b = bytes[start];
            let bad_start = matches!(b, 0x80..=0xc1 | 0xf5..=0xff);
            (start + n, if bad_start { "invalid start byte" } else { "invalid continuation byte" })
        }
    };
    if end - start == 1 {
        format!("'utf-8' codec can't decode byte 0x{:02x} in position {start}: {reason}", bytes[start])
    } else {
        format!("'utf-8' codec can't decode bytes in position {start}-{}: {reason}", end - 1)
    }
}

/// `archive.parse_tfi`; Err is the `ArchiveError` text.
pub fn parse_tfi(data: &[u8]) -> Result<Vec<TfiEntry>, String> {
    let mut entries = Vec::new();
    let mut pos = 0usize;
    let n = data.len();
    let fail = |e: &str| format!("malformed .tfi: {e}");
    while pos < n {
        let (name_len, p) = read_leb128(data, pos).map_err(fail)?;
        let end = p.saturating_add(name_len as usize);
        let raw = &data[p.min(n)..end.min(n)];
        pos = end;
        let raw = raw.split(|&b| b == 0).next().unwrap_or(&[]);
        let name = std::str::from_utf8(raw).map_err(|e| fail(&utf8_error_text(raw, e)))?.replace('\\', "/");
        let (archive_index, p) = read_leb128(data, pos).map_err(fail)?;
        let (offset, p) = read_leb128(data, p).map_err(fail)?;
        let (size, p) = read_leb128(data, p).map_err(fail)?;
        let (fnv_hash, p) = read_leb128(data, p).map_err(fail)?;
        pos = p;
        entries.push(TfiEntry { name, archive_index, offset, size, fnv_hash });
    }
    Ok(entries)
}

/// `archive.archive_size`: the last file's end in that archive, padded to 4.
pub fn archive_size<'a>(entries: impl IntoIterator<Item = &'a TfiEntry>, archive_index: u32) -> u64 {
    let end = entries
        .into_iter()
        .filter(|e| e.archive_index == archive_index)
        .map(|e| u64::from(e.offset) + u64::from(e.size))
        .max()
        .unwrap_or(0);
    end.div_ceil(4) * 4
}

/// `content[offset:offset + size]` (clamped like a Python slice).
pub fn slice_range(len: usize, offset: u32, size: u32) -> std::ops::Range<usize> {
    let start = (offset as usize).min(len);
    let end = (offset as usize).saturating_add(size as usize).min(len);
    start..end.max(start)
}

/// `str(zlib.error)` for a stream `decompressobj()` rejects.
fn zlib_error_text(input: &[u8]) -> String {
    use flate2::{Decompress, FlushDecompress};
    let mut d = Decompress::new(true);
    let mut out = vec![0u8; 64 * 1024];
    loop {
        let before = (d.total_in(), d.total_out());
        match d.decompress(&input[d.total_in() as usize..], &mut out, FlushDecompress::None) {
            Ok(flate2::Status::StreamEnd) => return "Error -3 while decompressing data".into(),
            Ok(_) if (d.total_in(), d.total_out()) == before => return "Error -3 while decompressing data".into(),
            Ok(_) => {}
            Err(e) if e.needs_dictionary().is_some() => return "Error 2 while decompressing data".into(),
            Err(e) => {
                let msg = e.message().unwrap_or("invalid input data");
                return format!("Error -3 while decompressing data: {msg}");
            }
        }
    }
}

/// `archive.decompress_tfa(raw, size_hint)` through `troveio.inflate`
/// (`decompressobj()`: trailing bytes ignored, a truncated stream yields what
/// it decoded); Err is the `ArchiveError` text.
pub fn decompress_tfa(raw: &[u8], size_hint: u64) -> Result<Vec<u8>, String> {
    if size_hint > 0
        && let Ok(n) = usize::try_from(size_hint)
    {
        let mut buf = vec![0u8; n];
        if zlib::inflate_exact(raw, &mut buf).is_some() {
            return Ok(buf);
        }
    }
    match zlib::inflate(raw, usize::MAX) {
        Some(z) => Ok(z.data),
        None => Err(format!("could not inflate .tfa: {}", zlib_error_text(raw))),
    }
}

// --- diff ---------------------------------------------------------------------------

pub fn is_tfa(path: &str) -> bool {
    path.ends_with(".tfa")
}

pub fn is_tfi(path: &str) -> bool {
    path.ends_with(".tfi")
}

pub fn parent_dir(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(d, _)| d)
}

/// `archive(\d+)\.tfa$` searched in `path` (Unicode digits, like `re`).
pub fn archive_index_of(path: &str) -> Option<i64> {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"archive(\d+)\.tfa$").expect("static regex"));
    let digits = re.captures(path)?.get(1)?.as_str();
    pynum::int_of_digits(digits).ok()
}

pub fn join_logical(directory: &str, name: &str) -> String {
    if directory.is_empty() { name.to_owned() } else { format!("{directory}/{name}") }
}

/// Last-seen manifest row (`update_manifest`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sidecar {
    pub sha1: String,
    pub size: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirWork {
    pub directory: String,
    pub tfi_path: Option<String>,
    pub changed_archives: Vec<i64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub changed_loose: Vec<String>,
    pub removed_loose: Vec<String>,
    /// In sorted directory order.
    pub dir_work: Vec<DirWork>,
    pub new_map: Ordered<ManifestEntry>,
}

impl Plan {
    pub fn any(&self) -> bool {
        !self.changed_loose.is_empty() || !self.removed_loose.is_empty() || !self.dir_work.is_empty()
    }
}

/// `classify_manifest_diff(old, new_entries)`.
pub fn classify_manifest_diff(old: &Ordered<Sidecar>, new_entries: &[ManifestEntry]) -> Plan {
    let new_map: Ordered<ManifestEntry> = new_entries.iter().map(|e| (e.path.clone(), e.clone())).collect();
    let changed: Vec<&String> =
        new_map.iter().filter(|(p, e)| old.get(p).map(|o| &o.sha1) != Some(&e.sha1)).map(|(p, _)| p).collect();
    let removed: Vec<&String> = old.keys().filter(|p| !new_map.contains_key(p)).collect();
    let loose = |ps: &[&String]| {
        let mut v: Vec<String> = ps.iter().filter(|p| !is_tfa(p) && !is_tfi(p)).map(|p| (*p).clone()).collect();
        v.sort();
        v
    };
    let changed_loose = loose(&changed);
    let removed_loose = loose(&removed);
    let mut arch_dirs: Vec<&str> = changed
        .iter()
        .chain(&removed)
        .filter(|p| is_tfa(p) || is_tfi(p))
        .map(|p| parent_dir(p))
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    arch_dirs.sort();
    let dir_work = arch_dirs
        .into_iter()
        .map(|d| {
            let tfi_path = new_map.keys().find(|p| is_tfi(p) && parent_dir(p) == d).cloned();
            let mut changed_archives: Vec<i64> = changed
                .iter()
                .filter(|p| is_tfa(p) && parent_dir(p) == d)
                .filter_map(|p| archive_index_of(p))
                .collect();
            changed_archives.sort();
            DirWork { directory: d.to_owned(), tfi_path, changed_archives }
        })
        .collect();
    Plan { changed_loose, removed_loose, dir_work, new_map }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LogicalDiff {
    pub added: Vec<String>,
    pub modified: Vec<String>,
    pub removed: Vec<String>,
}

/// `diff_logical(old_fnv, new_fnv)`; a stored `None` hash differs from any.
pub fn diff_logical(old: &Ordered<Option<i64>>, new: &Ordered<u32>) -> LogicalDiff {
    let mut d = LogicalDiff::default();
    for (p, &h) in new.iter() {
        match old.get(p) {
            None => d.added.push(p.clone()),
            Some(o) if *o != Some(i64::from(h)) => d.modified.push(p.clone()),
            Some(_) => {}
        }
    }
    d.removed = old.keys().filter(|p| !new.contains_key(p)).cloned().collect();
    d.added.sort();
    d.modified.sort();
    d.removed.sort();
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leb128_masks_and_limits() {
        assert_eq!(read_leb128(&[0x80, 0x01], 0), Ok((128, 2)));
        assert_eq!(read_leb128(&[0xff, 0xff, 0xff, 0xff, 0x7f], 0), Ok((u32::MAX, 5)));
        assert_eq!(read_leb128(&[0x80; 10], 0), Err("varint too long"));
        assert_eq!(read_leb128(&[0x80; 9], 0), Err("index out of range"));
    }

    #[test]
    fn archive_size_pads() {
        let e = |ai, offset, size| TfiEntry { name: String::new(), archive_index: ai, offset, size, fnv_hash: 0 };
        let es = [e(0, 0, 5), e(0, 5, 2), e(1, 0, 100)];
        assert_eq!(archive_size(&es, 0), 8);
        assert_eq!(archive_size(&es, 1), 100);
        assert_eq!(archive_size(&es, 2), 0);
    }

    #[test]
    fn slices_clamp() {
        assert_eq!(slice_range(10, 8, 5), 8..10);
        assert_eq!(slice_range(10, 12, 5), 10..10);
    }

    #[test]
    fn ordered_keeps_first_position() {
        let mut m = Ordered::new();
        m.insert("b".into(), 1);
        m.insert("a".into(), 2);
        m.insert("b".into(), 3);
        assert_eq!(m.iter().map(|(k, v)| (k.as_str(), *v)).collect::<Vec<_>>(), [("b", 3), ("a", 2)]);
    }
}

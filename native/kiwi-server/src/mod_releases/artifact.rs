//! Reading a release artifact the way `service.py` does: `read_tmod` (header
//! table, file stream), `zipfile`'s table of contents, and the pure helpers
//! that name and describe what is inside.

use kiwi_core::pystr::{prefix, strip};

use crate::tools::tmod as tools_tmod;

pub struct TFile {
    pub path: String,
    pub size: u32,
    pub data: Vec<u8>,
}

pub struct Tmod {
    pub version: u16,
    /// `dict(pairs)`: first position, last value.
    pub props: Vec<(String, String)>,
    pub files: Vec<TFile>,
}

impl Tmod {
    pub fn prop(&self, k: &str) -> Option<&str> {
        self.props.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str())
    }
}

/// `read_tmod(data, metadata_only=not full)`; `Err` is the `TmodError` text.
pub fn read(data: &[u8], full: bool) -> Result<Tmod, String> {
    if data.len() < 12 {
        return Err("file too small to be a .tmod".into());
    }
    let header_size = u64::from_le_bytes(data[0..8].try_into().unwrap_or_default());
    let version = u16::from_le_bytes([data[8], data[9]]);
    let prop_count = u16::from_le_bytes([data[10], data[11]]);
    if !(12..=data.len() as u64).contains(&header_size) {
        return Err("declared header size is outside the file".into());
    }
    let hs = header_size as usize;
    let Some(h) = kiwi_core::tmod::header(data, hs, prop_count) else {
        return Err(tools_tmod::read(data, true).err().unwrap_or_default());
    };
    let mut props: Vec<(String, String)> = Vec::with_capacity(h.properties.len());
    for (k, v) in h.properties {
        match props.iter_mut().find(|(n, _)| *n == k) {
            Some(slot) => slot.1 = v,
            None => props.push((k, v)),
        }
    }
    let mut contents = if full { kiwi_core::tmod::raw_contents(&data[hs..], &h.entries) } else { Vec::new() };
    contents.reverse();
    let files = h
        .entries
        .into_iter()
        .map(|e| TFile { path: e.path, size: e.size, data: contents.pop().unwrap_or_default() })
        .collect();
    Ok(Tmod { version, props, files })
}

/// `str.lower()`.
pub fn lower(s: &str) -> String {
    s.to_lowercase()
}

fn rsplit_last(s: &str) -> &str {
    s.rsplit('/').next().unwrap_or(s)
}

/// `vfx.basename`.
pub fn basename(p: &str) -> String {
    rsplit_last(&p.replace('\\', "/")).to_owned()
}

/// `_safe_filename`.
pub fn safe_filename(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .filter(|c| !matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' | '\0'..='\x1f'))
        .collect();
    let cleaned = strip(&cleaned).trim_matches('.');
    let out = prefix(cleaned, 120);
    if out.is_empty() { "mod".into() } else { out.to_owned() }
}

/// `release_download_filename`.
pub fn download_filename(format: &str, stored: &str, props: &[(String, String)]) -> String {
    if format == "zip" {
        return stored.to_owned();
    }
    let title = props.iter().find(|(k, _)| k == "title").map_or("", |(_, v)| strip(v));
    if title.is_empty() { stored.to_owned() } else { format!("{}.tmod", safe_filename(title)) }
}

/// `_preview_path`.
pub fn preview_path(props: &[(String, String)]) -> String {
    let p = props.iter().find(|(k, _)| k == "previewPath").map_or("", |(_, v)| v.as_str());
    lower(strip(p))
}

fn config_prop(props: &[(String, String)]) -> Option<&str> {
    props.iter().find(|(k, _)| lower(k) == "configpath").map(|(_, v)| v.as_str())
}

/// `_declared_config_path`.
pub fn declared_config_path(props: &[(String, String)]) -> String {
    let declared = config_prop(props).unwrap_or("").replace('\\', "/");
    lower(strip(declared.trim_start_matches('/')))
}

/// `_cfg_download_name(release, path)` (`props` = the release's stored header).
pub fn cfg_download_name(props: &[(String, String)], path: &str) -> String {
    let base = rsplit_last(path);
    let title = props.iter().find(|(k, _)| k == "title").map_or("", |(_, v)| strip(v));
    if !title.is_empty() && lower(base) == format!("{}.cfg", lower(title)) {
        return format!("{}.cfg", safe_filename(title));
    }
    let declared = declared_config_path(props);
    let declared_base = rsplit_last(&declared);
    if !declared_base.is_empty() && declared_base == lower(base) {
        let raw = config_prop(props).unwrap_or("").replace('\\', "/");
        return safe_filename(strip(rsplit_last(&raw)));
    }
    safe_filename(base)
}

/// `int(properties.get("flags", 0) or 0)`, 0 where `int()` raises.
pub fn flags_value(props: &[(String, String)]) -> i64 {
    match props.iter().find(|(k, _)| k == "flags") {
        Some((_, v)) if !v.is_empty() => kiwi_core::pynum::int(v).unwrap_or(0),
        _ => 0,
    }
}

/// `mod_categories.tags_from_flags(flags)` for any Python int.
pub fn categories(flags: i64) -> Vec<&'static str> {
    kiwi_core::mod_categories::tags_from_flags((flags as u64) & kiwi_core::mod_categories::ALL_MASK)
}

// --- zipfile ---------------------------------------------------------------

/// Why `zipfile.ZipFile` refused: `BadZipFile`/`ValueError` (caught by the
/// inspector) or `NotImplementedError` (a 500).
#[derive(Debug, PartialEq)]
pub enum ZipFail {
    Bad,
    Unhandled,
}

struct EndRec {
    size_cd: u64,
    offset_cd: u64,
    location: u64,
}

fn u16_at(d: &[u8], i: usize) -> u64 {
    u64::from(u16::from_le_bytes([d[i], d[i + 1]]))
}

fn u32_at(d: &[u8], i: usize) -> u64 {
    u64::from(u32::from_le_bytes([d[i], d[i + 1], d[i + 2], d[i + 3]]))
}

fn u64_at(d: &[u8], i: usize) -> u64 {
    u64::from_le_bytes(d[i..i + 8].try_into().unwrap_or_default())
}

/// `_EndRecData64`.
fn end_rec64(data: &[u8], offset: i64, mut rec: EndRec) -> Result<EndRec, ZipFail> {
    let offset = offset - 20;
    if offset < 0 {
        return Ok(rec);
    }
    let loc = &data[offset as usize..offset as usize + 20];
    if &loc[0..4] != b"PK\x06\x07" {
        return Ok(rec);
    }
    let (diskno, reloff, disks) = (u32_at(loc, 4), u64_at(loc, 8), u32_at(loc, 16));
    if diskno != 0 || disks > 1 {
        return Err(ZipFail::Bad);
    }
    let offset = offset - 56;
    if offset < 0 || reloff > offset as u64 {
        return Err(ZipFail::Bad);
    }
    let read56 = |at: u64| data.get(at as usize..at as usize + 56).ok_or(ZipFail::Bad);
    let mut extrasz = offset as u64 - reloff;
    let mut rec64 = read56(reloff)?;
    if !rec64.starts_with(b"PK\x06\x06") && reloff != offset as u64 {
        extrasz = 0;
        rec64 = read56(offset as u64)?;
    }
    if !rec64.starts_with(b"PK\x06\x06") {
        return Err(ZipFail::Bad);
    }
    let sz = u64_at(rec64, 4);
    let (dirsize, diroffset) = (u64_at(rec64, 40), u64_at(rec64, 48));
    if diroffset.checked_add(dirsize) != Some(reloff) || sz.checked_add(12) != Some(56 + extrasz) {
        return Err(ZipFail::Bad);
    }
    rec.size_cd = dirsize;
    rec.offset_cd = diroffset;
    rec.location = offset as u64 - extrasz;
    Ok(rec)
}

fn rec_at(d: &[u8], i: usize, location: u64) -> EndRec {
    EndRec { size_cd: u32_at(d, i + 12), offset_cd: u32_at(d, i + 16), location }
}

/// `_EndRecData`.
fn end_rec(data: &[u8]) -> Result<Option<EndRec>, ZipFail> {
    let n = data.len();
    if n >= 22 {
        let tail = &data[n - 22..];
        if &tail[0..4] == b"PK\x05\x06" && tail[20..22] == [0, 0] {
            let rec = rec_at(tail, 0, (n - 22) as u64);
            return end_rec64(data, (n - 22) as i64, rec).map(Some);
        }
    }
    let start = n.saturating_sub(65535 + 22);
    let window = &data[start..];
    let Some(pos) = memchr::memmem::rfind(window, b"PK\x05\x06") else { return Ok(None) };
    if window.len() - pos < 22 {
        return Ok(None);
    }
    let rec = rec_at(window, pos, (start + pos) as u64);
    end_rec64(data, (start + pos) as i64, rec).map(Some)
}

/// `zipfile.is_zipfile(BytesIO(data))`; `Err` is the `BadZipFile` it lets
/// escape.
pub fn is_zipfile(data: &[u8]) -> Result<bool, ZipFail> {
    end_rec(data).map(|r| r.is_some())
}

const CP437: [char; 128] = [
    'Ç', 'ü', 'é', 'â', 'ä', 'à', 'å', 'ç', 'ê', 'ë', 'è', 'ï', 'î', 'ì', 'Ä', 'Å', 'É', 'æ', 'Æ', 'ô', 'ö', 'ò', 'û',
    'ù', 'ÿ', 'Ö', 'Ü', '¢', '£', '¥', '₧', 'ƒ', 'á', 'í', 'ó', 'ú', 'ñ', 'Ñ', 'ª', 'º', '¿', '⌐', '¬', '½', '¼', '¡',
    '«', '»', '░', '▒', '▓', '│', '┤', '╡', '╢', '╖', '╕', '╣', '║', '╗', '╝', '╜', '╛', '┐', '└', '┴', '┬', '├', '─',
    '┼', '╞', '╟', '╚', '╔', '╩', '╦', '╠', '═', '╬', '╧', '╨', '╤', '╥', '╙', '╘', '╒', '╓', '╫', '╪', '┘', '┌', '█',
    '▄', '▌', '▐', '▀', 'α', 'ß', 'Γ', 'π', 'Σ', 'σ', 'µ', 'τ', 'Φ', 'Θ', 'Ω', 'δ', '∞', 'φ', 'ε', '∩', '≡', '±', '≥',
    '≤', '⌠', '⌡', '÷', '≈', '°', '∙', '·', '√', 'ⁿ', '²', '■', '\u{a0}',
];

fn cp437(b: &[u8]) -> String {
    b.iter().map(|&c| if c < 0x80 { char::from(c) } else { CP437[usize::from(c - 0x80)] }).collect()
}

fn sanitize(name: String) -> String {
    match name.find('\0') {
        Some(i) => name[..i].to_owned(),
        None => name,
    }
}

/// `[(i.filename, i.file_size) for i in ZipFile(BytesIO(data)).infolist()]`.
pub fn zip_entries(data: &[u8]) -> Result<Vec<(String, u64)>, ZipFail> {
    let rec = end_rec(data)?.ok_or(ZipFail::Bad)?;
    let concat = rec.location as i128 - rec.size_cd as i128 - rec.offset_cd as i128;
    let start = rec.offset_cd as i128 + concat;
    if start < 0 {
        return Err(ZipFail::Bad);
    }
    let start = (start as usize).min(data.len());
    let end = start.saturating_add(rec.size_cd as usize).min(data.len());
    let cd = &data[start..end];
    let mut pos = 0usize;
    let mut total: u64 = 0;
    let mut out = Vec::new();
    let take = |pos: &mut usize, n: usize| -> &[u8] {
        let a = (*pos).min(cd.len());
        let b = a.saturating_add(n).min(cd.len());
        *pos = b;
        &cd[a..b]
    };
    while total < rec.size_cd {
        let head = take(&mut pos, 46);
        if head.len() != 46 || &head[0..4] != b"PK\x01\x02" {
            return Err(ZipFail::Bad);
        }
        let (extract_version, flags) = (head[6], u16_at(head, 8));
        let (compress_size, mut file_size) = (u32_at(head, 20), u32_at(head, 24));
        let (fn_len, extra_len, comment_len) =
            (u16_at(head, 28) as usize, u16_at(head, 30) as usize, u16_at(head, 32) as usize);
        let header_offset = u32_at(head, 42);
        let raw_name = take(&mut pos, fn_len).to_vec();
        let mut name = if flags & 0x800 != 0 {
            String::from_utf8(raw_name.clone()).map_err(|_| ZipFail::Bad)?
        } else {
            cp437(&raw_name)
        };
        name = sanitize(name);
        let extra = take(&mut pos, extra_len).to_vec();
        take(&mut pos, comment_len);
        if extract_version > 63 {
            return Err(ZipFail::Unhandled);
        }
        let mut ex = extra.as_slice();
        while ex.len() >= 4 {
            let (tp, ln) = (u16_at(ex, 0), u16_at(ex, 2) as usize);
            if ln + 4 > ex.len() {
                return Err(ZipFail::Bad);
            }
            let body = &ex[4..ln + 4];
            if tp == 0x0001 {
                let mut b = body;
                let mut next = || -> Result<u64, ZipFail> {
                    let v = b.get(..8).map(|s| u64_at(s, 0)).ok_or(ZipFail::Bad)?;
                    b = &b[8..];
                    Ok(v)
                };
                if file_size == 0xFFFF_FFFF {
                    file_size = next()?;
                }
                if compress_size == 0xFFFF_FFFF {
                    next()?;
                }
                if header_offset == 0xFFFF_FFFF {
                    next()?;
                }
            } else if tp == 0x7075 {
                if body.len() < 5 {
                    return Err(ZipFail::Bad);
                }
                if body[0] == 1 && u32_at(body, 1) as u32 == crc32fast::hash(&raw_name) {
                    let uni = std::str::from_utf8(&body[5..]).map_err(|_| ZipFail::Bad)?;
                    if !uni.is_empty() {
                        name = sanitize(uni.to_owned());
                    }
                }
            }
            ex = &ex[ln + 4..];
        }
        out.push((name, file_size));
        total += 46 + fn_len as u64 + extra_len as u64 + comment_len as u64;
    }
    Ok(out)
}

/// `_build_zip(files)`: `ZipFile(..., ZIP_DEFLATED)` + `writestr` per file.
pub fn build_zip(files: &[(String, Vec<u8>)]) -> Option<Vec<u8>> {
    crate::blueprints::pack::zip(files, crate::blueprints::pack::now_date_time())
}

// --- trove_layout ----------------------------------------------------------

const TROVE_DIRECTORIES: [&str; 11] = [
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
const COMPILE_IGNORED: [&str; 14] =
    [".tfi", ".tfa", ".exe", ".dll", ".tmod", ".zip", ".cfg", ".txt", ".log", ".ini", ".toml", ".json", ".xml", ".dat"];

fn ext(path: &str) -> String {
    let name = rsplit_last(path);
    match name.rfind('.') {
        Some(i) => format!(".{}", lower(&name[i + 1..])),
        None => String::new(),
    }
}

/// `trove_layout.is_compilable`.
pub fn is_compilable(path: &str) -> bool {
    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() < 2 || !TROVE_DIRECTORIES.contains(&lower(parts[0]).as_str()) {
        return false;
    }
    !COMPILE_IGNORED.contains(&ext(path).as_str())
}

//! `app/trove/audio/names.py`: sound names from the `.txt` Wwise writes
//! beside a bank, and the FNV-1 event id hash.

use std::collections::HashMap;

use kiwi_core::pystr::strip;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SoundName {
    pub name: String,
    pub path: String,
    pub group: String,
    pub source: String,
    pub notes: String,
}

/// `str.splitlines()`.
pub fn splitlines(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut it = text.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        let brk =
            matches!(c, '\n' | '\r' | '\x0b' | '\x0c' | '\x1c' | '\x1d' | '\x1e' | '\u{85}' | '\u{2028}' | '\u{2029}');
        if !brk {
            continue;
        }
        out.push(&text[start..i]);
        let mut next = i + c.len_utf8();
        if c == '\r' && it.peek().is_some_and(|&(_, n)| n == '\n') {
            it.next();
            next += 1;
        }
        start = next;
    }
    if start < text.len() {
        out.push(&text[start..]);
    }
    out
}

type Row = HashMap<String, String>;

fn rows(text: &str) -> HashMap<String, Vec<Row>> {
    let mut sections: HashMap<String, Vec<Row>> = HashMap::new();
    let mut columns: Vec<String> = Vec::new();
    let mut current: Option<String> = None;
    for line in splitlines(text) {
        if strip(line).is_empty() {
            continue;
        }
        let cells: Vec<&str> = line.split('\t').collect();
        if !strip(cells[0]).is_empty() {
            columns = cells.iter().map(|c| strip(c).to_owned()).collect();
            let key = strip(cells[0]).to_owned();
            sections.entry(key.clone()).or_default();
            current = Some(key);
            continue;
        }
        let Some(key) = &current else { continue };
        let row: Row =
            (0..columns.len().min(cells.len())).map(|i| (columns[i].clone(), strip(cells[i]).to_owned())).collect();
        sections.entry(key.clone()).or_default().push(row);
    }
    sections
}

fn group(path: &str) -> String {
    let parts: Vec<&str> = path.split('\\').filter(|p| !p.is_empty()).collect();
    if parts.len() >= 2 { parts[parts.len() - 2].to_owned() } else { String::new() }
}

/// `int(s)` of a `str.isdigit()` id, as its canonical decimal text.
fn digits_id(s: &str) -> Option<String> {
    if s.is_empty() || !kiwi_core::pynum::is_decimal(s) {
        return None;
    }
    let ascii: String =
        s.chars().filter_map(|c| kiwi_core::pynum::decimal(c).and_then(|d| char::from_digit(d, 10))).collect();
    let trimmed = ascii.trim_start_matches('0');
    Some(if trimmed.is_empty() { "0".into() } else { trimmed.into() })
}

/// `parse`: (media id text -> name, number of distinct event ids).
pub fn parse(text: &str) -> (HashMap<String, SoundName>, usize) {
    let sections = rows(text);
    let get = |r: &Row, k: &str| r.get(k).cloned().unwrap_or_default();
    let mut sounds = HashMap::new();
    for row in sections.get("In Memory Audio").map(Vec::as_slice).unwrap_or(&[]) {
        let (raw_id, name) = (get(row, "ID"), get(row, "Name"));
        let Some(id) = digits_id(&raw_id) else { continue };
        if name.is_empty() {
            continue;
        }
        let path = get(row, "Wwise Object Path");
        let source = get(row, "Audio source file").replace('/', "\\");
        let source = source.rsplit('\\').next().unwrap_or("").to_owned();
        sounds.insert(id, SoundName { name, group: group(&path), path, source, notes: get(row, "Notes") });
    }
    let mut events = std::collections::HashSet::new();
    for row in sections.get("Event").map(Vec::as_slice).unwrap_or(&[]) {
        let (raw_id, name) = (get(row, "ID"), get(row, "Name"));
        if let Some(id) = digits_id(&raw_id)
            && !name.is_empty()
        {
            events.insert(id);
        }
    }
    (sounds, events.len())
}

/// `event_id`: 32-bit FNV-1 over the lowercased UTF-8 name.
pub fn event_id(name: &str) -> u32 {
    let mut v: u32 = 2166136261;
    for b in name.to_lowercase().bytes() {
        v = v.wrapping_mul(16777619);
        v ^= u32::from(b);
    }
    v
}

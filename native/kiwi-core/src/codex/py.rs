//! The Python `str`/`bytes` operations the decoders lean on.

use crate::pystr;

/// `str.strip()`.
pub fn strip(s: &str) -> &str {
    pystr::strip(s)
}

/// `str.strip(chars)`.
pub fn strip_chars<'a>(s: &'a str, chars: &str) -> &'a str {
    s.trim_matches(|c| chars.contains(c))
}

/// `str.rstrip(chars)`.
pub fn rstrip_chars<'a>(s: &'a str, chars: &str) -> &'a str {
    s.trim_end_matches(|c| chars.contains(c))
}

/// `str.lstrip(chars)`.
pub fn lstrip_chars<'a>(s: &'a str, chars: &str) -> &'a str {
    s.trim_start_matches(|c| chars.contains(c))
}

/// `str.lower()`.
pub fn lower(s: &str) -> String {
    s.to_lowercase()
}

/// `s.rsplit(sep, 1)[-1]`.
pub fn last_part<'a>(s: &'a str, sep: &str) -> &'a str {
    s.rsplit_once(sep).map_or(s, |(_, t)| t)
}

/// `str.title()`: a cased character after a cased one is lowered, any other
/// is title-cased.
pub fn title(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_cased = false;
    for c in s.chars() {
        if prev_cased {
            out.extend(c.to_lowercase());
        } else {
            out.extend(c.to_uppercase());
        }
        prev_cased = c.is_lowercase() || c.is_uppercase();
    }
    out
}

/// `bytes.decode("latin1")`.
pub fn latin1(b: &[u8]) -> String {
    b.iter().map(|&x| x as char).collect()
}

/// `bytes.decode("ascii", errors="ignore")`.
pub fn ascii_ignore(b: &[u8]) -> String {
    b.iter().filter(|x| x.is_ascii()).map(|&x| x as char).collect()
}

/// `bytes.decode("utf-8", errors="ignore")`.
pub fn utf8_ignore(mut b: &[u8]) -> String {
    let mut out = String::with_capacity(b.len());
    loop {
        match std::str::from_utf8(b) {
            Ok(s) => {
                out.push_str(s);
                return out;
            }
            Err(e) => {
                let good = e.valid_up_to();
                out.push_str(std::str::from_utf8(&b[..good]).unwrap_or_default());
                match e.error_len() {
                    Some(n) => b = &b[good + n..],
                    None => return out,
                }
            }
        }
    }
}

/// `PurePosixPath(p).stem`.
pub fn path_stem(p: &str) -> String {
    let name = p.split('/').rev().find(|s| !s.is_empty() && *s != ".").unwrap_or("");
    match name.rfind('.') {
        Some(i) if i > 0 && i < name.len() - 1 => name[..i].to_owned(),
        _ => name.to_owned(),
    }
}

/// `bytes.find(needle, start)`, `None` for -1.
pub fn find(hay: &[u8], needle: &[u8], start: usize) -> Option<usize> {
    if start > hay.len() || needle.len() > hay.len() - start {
        return None;
    }
    if needle.is_empty() {
        return Some(start);
    }
    hay[start..].windows(needle.len()).position(|w| w == needle).map(|i| i + start)
}

/// `bytes.rfind(needle)`.
pub fn rfind(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.len() > hay.len() {
        return None;
    }
    hay.windows(needle.len()).rposition(|w| w == needle)
}

/// `needle in hay` for bytes.
pub fn contains(hay: &[u8], needle: &[u8]) -> bool {
    find(hay, needle, 0).is_some()
}

/// A bytes regex (`re.compile(rb"...")`): byte-oriented, so `\xNN` is a byte.
pub fn bytes_re(pattern: &str) -> regex::bytes::Regex {
    regex::bytes::RegexBuilder::new(pattern).unicode(false).build().expect("static regex")
}

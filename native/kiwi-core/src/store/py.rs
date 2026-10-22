//! The Python string builtins the StoreLog parser leans on.

use crate::feeds::py::decimal;
use crate::pystr::strip;

/// `str.splitlines()`.
pub fn splitlines(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut it = s.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        let brk =
            matches!(c, '\n' | '\r' | '\x0b' | '\x0c' | '\x1c' | '\x1d' | '\x1e' | '\u{85}' | '\u{2028}' | '\u{2029}');
        if !brk {
            continue;
        }
        out.push(&s[start..i]);
        let mut end = i + c.len_utf8();
        if c == '\r'
            && let Some(&(j, '\n')) = it.peek()
        {
            it.next();
            end = j + 1;
        }
        start = end;
    }
    if start < s.len() {
        out.push(&s[start..]);
    }
    out
}

fn hex(b: u8) -> Option<u8> {
    (b as char).to_digit(16).map(|d| d as u8)
}

/// `urllib.parse.unquote(s)`: each run of ASCII characters is %-decoded to
/// bytes and read as UTF-8 with replacement; anything else passes through.
pub fn unquote(s: &str) -> String {
    if !s.contains('%') {
        return s.to_owned();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while !rest.is_empty() {
        let split = rest.find(|c: char| !c.is_ascii()).unwrap_or(rest.len());
        let (ascii, tail) = rest.split_at(split);
        out.push_str(&String::from_utf8_lossy(&percent_bytes(ascii.as_bytes())));
        let non_ascii = tail.find(|c: char| c.is_ascii()).unwrap_or(tail.len());
        out.push_str(&tail[..non_ascii]);
        rest = &tail[non_ascii..];
    }
    out
}

fn percent_bytes(b: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && let (Some(h), Some(l)) = (b.get(i + 1).and_then(|&c| hex(c)), b.get(i + 2).and_then(|&c| hex(c)))
        {
            out.push(h * 16 + l);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

/// `float(s)`: Unicode digits and surrounding whitespace allowed, single
/// underscores between digits, `inf`/`infinity`/`nan` in any case. `None`
/// where Python raises ValueError.
pub fn float(s: &str) -> Option<f64> {
    let mut ascii = String::with_capacity(s.len());
    for c in strip(s).chars() {
        match decimal(c) {
            Some(d) if !c.is_ascii() => ascii.push(char::from_digit(d, 10)?),
            _ if c.is_ascii() => ascii.push(c),
            _ => return None,
        }
    }
    let b = ascii.as_bytes();
    let mut clean = String::with_capacity(ascii.len());
    let mut prev = 0u8;
    for &c in b {
        if c == b'_' {
            if !prev.is_ascii_digit() {
                return None;
            }
        } else {
            if prev == b'_' && !c.is_ascii_digit() {
                return None;
            }
            clean.push(c as char);
        }
        prev = c;
    }
    if prev == b'_' || clean.is_empty() {
        return None;
    }
    if clean.bytes().any(|c| c.is_ascii_whitespace()) {
        return None;
    }
    clean.parse::<f64>().ok()
}

/// `int(s)` within `i64` (larger values are treated as unparseable).
pub fn int(s: &str) -> Option<i64> {
    crate::feeds::py::int(s, 10).and_then(|v| i64::try_from(v).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splitlines_matches_python() {
        assert_eq!(splitlines(""), Vec::<&str>::new());
        assert_eq!(splitlines("a\r\nb\rc\n"), vec!["a", "b", "c"]);
        assert_eq!(splitlines("a\n\nb"), vec!["a", "", "b"]);
        assert_eq!(splitlines("a\u{2028}b\x1cc\u{85}"), vec!["a", "b", "c"]);
        assert_eq!(splitlines("\r\r\n"), vec!["", ""]);
    }

    #[test]
    fn unquote_matches_python() {
        assert_eq!(unquote("a%20b"), "a b");
        assert_eq!(unquote("%zz%4%"), "%zz%4%");
        assert_eq!(unquote("é%C3%A9"), "éé");
        assert_eq!(unquote("%E2é%82%AC"), "\u{fffd}é\u{fffd}\u{fffd}");
        assert_eq!(unquote("a+b"), "a+b");
        assert_eq!(unquote("%ED%A0%80"), "\u{fffd}\u{fffd}\u{fffd}");
    }

    #[test]
    fn float_matches_python() {
        assert_eq!(float(" 1_000 "), Some(1000.0));
        assert_eq!(float("1__0"), None);
        assert_eq!(float("_1"), None);
        assert_eq!(float("1_"), None);
        assert_eq!(float("1._5"), None);
        assert_eq!(float("1e1_0"), Some(1e10));
        assert_eq!(float("\u{661}\u{662}"), Some(12.0));
        assert_eq!(float("-Infinity"), Some(f64::NEG_INFINITY));
        assert!(float("nan").is_some_and(f64::is_nan));
        assert_eq!(float("0x10"), None);
        assert_eq!(float("."), None);
        assert_eq!(float("5."), Some(5.0));
        assert_eq!(float("1 5"), None);
        assert_eq!(float(""), None);
    }
}

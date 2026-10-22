//! `html.unescape` and the entity lookups `html.parser` / BeautifulSoup make.

use std::borrow::Cow;

use super::py::char_offset;
use super::tables::{HTML5, INVALID_CHARREFS, INVALID_CODEPOINTS};

/// `html.entities.html5[name]` (`name` with or without the `;`).
pub fn html5(name: &str) -> Option<&'static str> {
    HTML5.binary_search_by(|(k, _)| (*k).cmp(name)).ok().map(|i| HTML5[i].1)
}

/// BeautifulSoup's `HTML_ENTITY_TO_CHARACTER[name]` (names without `;`).
pub fn bs4_entity(name: &str) -> Option<&'static str> {
    html5(name).or_else(|| html5(&format!("{name};")))
}

/// The text of `&#<num>;` as `html.unescape` gives it.
pub fn numeric_ref(num: u128) -> Cow<'static, str> {
    if let Ok(i) = INVALID_CHARREFS.binary_search_by(|(k, _)| (*k as u128).cmp(&num)) {
        return Cow::Borrowed(INVALID_CHARREFS[i].1);
    }
    if (0xd800..=0xdfff).contains(&num) || num > 0x10ffff {
        return Cow::Borrowed("\u{fffd}");
    }
    if INVALID_CODEPOINTS.binary_search(&(num as u32)).is_ok() {
        return Cow::Borrowed("");
    }
    Cow::Owned(char::from_u32(num as u32).map(String::from).unwrap_or_default())
}

fn run(b: &[u8], from: usize, pred: impl Fn(u8) -> bool) -> usize {
    b[from..].iter().position(|&c| !pred(c)).map_or(b.len(), |p| from + p)
}

fn parse_radix(digits: &str, radix: u32) -> u128 {
    digits.bytes().fold(0u128, |acc, d| {
        acc.saturating_mul(radix as u128).saturating_add((d as char).to_digit(radix).unwrap_or(0) as u128)
    })
}

/// One `_charref` match at the `&` at `amp`: its end and replacement.
fn charref_at(s: &str, amp: usize) -> Option<(usize, Cow<'static, str>)> {
    let b = s.as_bytes();
    let semi = |end: usize| if b.get(end) == Some(&b';') { end + 1 } else { end };
    if b.get(amp + 1) == Some(&b'#') {
        let digits_end = run(b, amp + 2, |c| c.is_ascii_digit());
        if digits_end > amp + 2 {
            return Some((semi(digits_end), numeric_ref(parse_radix(&s[amp + 2..digits_end], 10))));
        }
        if matches!(b.get(amp + 2), Some(b'x' | b'X')) {
            let hex_end = run(b, amp + 3, |c| c.is_ascii_hexdigit());
            if hex_end > amp + 3 {
                return Some((semi(hex_end), numeric_ref(parse_radix(&s[amp + 3..hex_end], 16))));
            }
        }
        return None;
    }
    let rest = &s[amp + 1..];
    let mut end = 0;
    for (n, (i, c)) in rest.char_indices().enumerate() {
        if n == 32 || matches!(c, '\t' | '\n' | '\x0c' | ' ' | '<' | '&' | '#' | ';') {
            break;
        }
        end = i + c.len_utf8();
    }
    if end == 0 {
        return None;
    }
    let end = if rest.as_bytes().get(end) == Some(&b';') { end + 1 } else { end };
    let name = &rest[..end];
    let text = if let Some(v) = html5(name) {
        Cow::Borrowed(v)
    } else {
        let count = name.chars().count();
        (2..count)
            .rev()
            .find_map(|x| {
                let cut = char_offset(name, x);
                html5(&name[..cut]).map(|v| Cow::Owned(format!("{v}{}", &name[cut..])))
            })
            .unwrap_or_else(|| Cow::Owned(format!("&{name}")))
    };
    Some((amp + 1 + end, text))
}

/// `html.unescape(s)`.
pub fn unescape(s: &str) -> Cow<'_, str> {
    if !s.contains('&') {
        return Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len());
    let (mut last, mut i) = (0, 0);
    while let Some(off) = s[i..].find('&') {
        let amp = i + off;
        match charref_at(s, amp) {
            Some((end, text)) => {
                out.push_str(&s[last..amp]);
                out.push_str(&text);
                last = end;
                i = end;
            }
            None => i = amp + 1,
        }
    }
    out.push_str(&s[last..]);
    Cow::Owned(out)
}

/// html.parser's `_unescape_attrvalue` (Python 3.13+): numeric references
/// always, named ones only on an exact match not followed by `=`.
pub fn unescape_attr(s: &str) -> Cow<'_, str> {
    if !s.contains('&') {
        return Cow::Borrowed(s);
    }
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let (mut last, mut i) = (0, 0);
    while let Some(off) = s[i..].find('&') {
        let amp = i + off;
        let body_end = match b.get(amp + 1) {
            Some(b'#') => {
                let digits_end = run(b, amp + 2, |c| c.is_ascii_digit());
                if digits_end > amp + 2 {
                    Some(digits_end)
                } else if matches!(b.get(amp + 2), Some(b'x' | b'X')) {
                    let hex_end = run(b, amp + 3, |c| c.is_ascii_hexdigit());
                    (hex_end > amp + 3).then_some(hex_end)
                } else {
                    None
                }
            }
            Some(c) if c.is_ascii_alphabetic() => Some(run(b, amp + 2, |c| c.is_ascii_alphanumeric())),
            _ => None,
        };
        let Some(body_end) = body_end else {
            i = amp + 1;
            continue;
        };
        let end = if matches!(b.get(body_end), Some(b';' | b'=')) { body_end + 1 } else { body_end };
        let reference = &s[amp..end];
        let replace = reference.starts_with("&#") || (!reference.ends_with('=') && html5(&reference[1..]).is_some());
        if replace {
            out.push_str(&s[last..amp]);
            out.push_str(&unescape(reference));
            last = end;
        }
        i = end;
    }
    out.push_str(&s[last..]);
    Cow::Owned(out)
}

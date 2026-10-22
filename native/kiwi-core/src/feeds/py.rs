//! Python `str` and `int()` semantics the parsers depend on.

use super::tables::DECIMAL_ZEROS;

pub use crate::pystr::{is_space, strip};

/// `unicodedata.decimal(c)`.
pub fn decimal(c: char) -> Option<u32> {
    let cp = c as u32;
    let zero = DECIMAL_ZEROS[..DECIMAL_ZEROS.partition_point(|&z| z <= cp)].last()?;
    (cp - zero < 10).then_some(cp - zero)
}

/// `int(s)` (radix 10) or `int(s, 16)`: surrounding whitespace, one sign,
/// Unicode decimal digits, single underscores between digits (and after a
/// `0x` prefix). `None` where Python raises ValueError; huge values saturate.
pub fn int(s: &str, radix: u32) -> Option<i128> {
    let ascii: String = strip(s)
        .chars()
        .map(|c| match decimal(c) {
            Some(d) if !c.is_ascii() => char::from_digit(d, 10).unwrap_or(c),
            _ => c,
        })
        .collect();
    let (neg, mut rest) = match ascii.as_bytes().first()? {
        b'-' => (true, &ascii[1..]),
        b'+' => (false, &ascii[1..]),
        _ => (false, ascii.as_str()),
    };
    let mut prefixed = false;
    if radix == 16
        && let Some(r) = rest.strip_prefix("0x").or_else(|| rest.strip_prefix("0X"))
    {
        rest = r;
        prefixed = true;
    }
    let b = rest.as_bytes();
    if b.is_empty() || b[b.len() - 1] == b'_' || (b[0] == b'_' && !prefixed) {
        return None;
    }
    let mut acc: i128 = 0;
    let mut digits = 0;
    for (i, &c) in b.iter().enumerate() {
        if c == b'_' {
            if b.get(i + 1) == Some(&b'_') {
                return None;
            }
            continue;
        }
        let d = (c as char).to_digit(radix)?;
        acc = acc.saturating_mul(radix as i128).saturating_add(d as i128);
        digits += 1;
    }
    (digits > 0).then_some(if neg { -acc } else { acc })
}

/// Byte offset of the `n`th char, so `&s[..char_offset(s, n)]` is `s[:n]`.
pub fn char_offset(s: &str, n: usize) -> usize {
    s.char_indices().nth(n).map_or(s.len(), |(i, _)| i)
}

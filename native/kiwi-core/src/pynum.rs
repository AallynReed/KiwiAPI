//! Python's number parsing over `str`: `str.isdecimal()` / `re`'s `\d`, `int()`
//! of a digit string and `float()`, with CPython 3.13's grammar (Unicode 15.1).

use crate::pystr::is_space;

/// First code point of every run of ten decimal digits (`unicodedata.decimal == 0`).
#[rustfmt::skip]
const DECIMAL_ZEROS: [u32; 68] = [
    0x30, 0x660, 0x6f0, 0x7c0, 0x966, 0x9e6, 0xa66, 0xae6, 0xb66, 0xbe6, 0xc66, 0xce6,
    0xd66, 0xde6, 0xe50, 0xed0, 0xf20, 0x1040, 0x1090, 0x17e0, 0x1810, 0x1946, 0x19d0, 0x1a80,
    0x1a90, 0x1b50, 0x1bb0, 0x1c40, 0x1c50, 0xa620, 0xa8d0, 0xa900, 0xa9d0, 0xa9f0, 0xaa50, 0xabf0,
    0xff10, 0x104a0, 0x10d30, 0x11066, 0x110f0, 0x11136, 0x111d0, 0x112f0, 0x11450, 0x114d0, 0x11650, 0x116c0,
    0x11730, 0x118e0, 0x11950, 0x11c50, 0x11d50, 0x11da0, 0x11f50, 0x16a60, 0x16ac0, 0x16b50, 0x1d7ce, 0x1d7d8,
    0x1d7e2, 0x1d7ec, 0x1d7f6, 0x1e140, 0x1e2f0, 0x1e4f0, 0x1e950, 0x1fbf0,
];

/// The digit value of a decimal character (`unicodedata.decimal`).
pub fn decimal(c: char) -> Option<u32> {
    let n = c as u32;
    if c.is_ascii() {
        return c.to_digit(10);
    }
    let i = DECIMAL_ZEROS.partition_point(|&z| z <= n);
    let zero = *DECIMAL_ZEROS.get(i.checked_sub(1)?)?;
    (n - zero < 10).then_some(n - zero)
}

/// `s.isdecimal()`.
pub fn is_decimal(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| decimal(c).is_some())
}

/// `sys.int_info.default_max_str_digits`.
pub const MAX_STR_DIGITS: usize = 4300;

/// `int(s)` of a string of decimal digits, saturated to `i64`; Err is the
/// `ValueError` Python raises past 4300 digits.
pub fn int_of_digits(s: &str) -> Result<i64, String> {
    let n = s.chars().count();
    if n > MAX_STR_DIGITS {
        return Err(format!(
            "Exceeds the limit ({MAX_STR_DIGITS} digits) for integer string conversion: value has {n} digits; \
             use sys.set_int_max_str_digits() to increase the limit"
        ));
    }
    let mut v: i64 = 0;
    for c in s.chars() {
        v = v.saturating_mul(10).saturating_add(i64::from(decimal(c).unwrap_or(0)));
    }
    Ok(v)
}

fn ascii_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\x0b' | b'\x0c' | b'\r')
}

/// `_PyUnicode_TransformDecimalAndSpaceToASCII`; None where it gives up.
fn to_ascii(s: &str) -> Option<Vec<u8>> {
    let mut buf = Vec::with_capacity(s.len());
    for c in s.chars() {
        if (c as u32) < 127 {
            buf.push(c as u8);
        } else if is_space(c) {
            buf.push(b' ');
        } else {
            buf.push(b'0' + decimal(c)? as u8);
        }
    }
    (!buf.contains(&0)).then_some(buf)
}

/// `int(s)` (base 10), saturated to `i64`; None where Python raises.
pub fn int(s: &str) -> Option<i64> {
    let buf = to_ascii(s)?;
    let start = buf.iter().position(|&b| !ascii_space(b))?;
    let end = buf.iter().rposition(|&b| !ascii_space(b))? + 1;
    let t = &buf[start..end];
    let (neg, digits) = match t.first()? {
        b'-' => (true, &t[1..]),
        b'+' => (false, &t[1..]),
        _ => (false, t),
    };
    if !digits.first()?.is_ascii_digit() || !digits.last()?.is_ascii_digit() {
        return None;
    }
    let mut v: i64 = 0;
    let mut n = 0;
    for (i, &b) in digits.iter().enumerate() {
        if b == b'_' {
            if !digits[i + 1].is_ascii_digit() {
                return None;
            }
            continue;
        }
        if !b.is_ascii_digit() {
            return None;
        }
        n += 1;
        v = v.saturating_mul(10).saturating_add(i64::from(b - b'0'));
    }
    if n > MAX_STR_DIGITS {
        return None;
    }
    Some(if neg { -v } else { v })
}

/// `float(s)`; None where Python raises `ValueError`.
pub fn float(s: &str) -> Option<f64> {
    let mut buf = to_ascii(s)?;
    if buf.contains(&b'_') {
        // _Py_string_to_number_with_underscores
        let mut out = Vec::with_capacity(buf.len());
        let mut prev = 0u8;
        for &b in &buf {
            if b == b'_' {
                if !prev.is_ascii_digit() {
                    return None;
                }
            } else {
                if prev == b'_' && !b.is_ascii_digit() {
                    return None;
                }
                out.push(b);
            }
            prev = b;
        }
        if prev == b'_' {
            return None;
        }
        buf = out;
    }
    let start = buf.iter().position(|&b| !ascii_space(b))?;
    let end = buf.iter().rposition(|&b| !ascii_space(b)).map_or(start, |e| e + 1);
    strtod(std::str::from_utf8(&buf[start..end]).ok()?)
}

/// `_Py_dg_strtod` then `_Py_parse_inf_or_nan`, the whole string consumed.
fn strtod(s: &str) -> Option<f64> {
    let b = s.as_bytes();
    let (neg, body) = match b.first()? {
        b'-' => (true, &s[1..]),
        b'+' => (false, &s[1..]),
        _ => (false, s),
    };
    let lower = body.to_ascii_lowercase();
    match lower.as_str() {
        "inf" | "infinity" => return Some(if neg { f64::NEG_INFINITY } else { f64::INFINITY }),
        "nan" => return Some(if neg { -f64::NAN } else { f64::NAN }),
        _ => {}
    }
    let d = body.as_bytes();
    let mut i = 0;
    let int_digits = d.iter().take_while(|c| c.is_ascii_digit()).count();
    i += int_digits;
    let mut frac_digits = 0;
    if d.get(i) == Some(&b'.') {
        i += 1;
        frac_digits = d[i..].iter().take_while(|c| c.is_ascii_digit()).count();
        i += frac_digits;
    }
    if int_digits + frac_digits == 0 {
        return None;
    }
    if matches!(d.get(i), Some(b'e' | b'E')) {
        let mut j = i + 1;
        if matches!(d.get(j), Some(b'+' | b'-')) {
            j += 1;
        }
        let exp_digits = d[j.min(d.len())..].iter().take_while(|c| c.is_ascii_digit()).count();
        if exp_digits > 0 {
            i = j + exp_digits;
        }
    }
    if i != d.len() {
        return None;
    }
    let v: f64 = body.parse().ok()?;
    Some(if neg { -v } else { v })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decimals() {
        assert_eq!(decimal('7'), Some(7));
        assert_eq!(decimal('\u{663}'), Some(3));
        assert_eq!(decimal('\u{1fbf9}'), Some(9));
        assert_eq!(decimal('²'), None);
        assert_eq!(decimal('\u{2167}'), None);
        assert!(is_decimal("١٢3") && !is_decimal("") && !is_decimal("1²"));
        assert_eq!(int_of_digits("١٢"), Ok(12));
        assert_eq!(int_of_digits("007"), Ok(7));
        assert!(int_of_digits(&"1".repeat(4301)).unwrap_err().contains("value has 4301 digits"));
        assert_eq!(int(" +1_700_000_000 "), Some(1_700_000_000));
        assert_eq!(int("-٣"), Some(-3));
        for bad in ["", " ", "1__0", "_1", "1_", "1.0", "0x1", "+"] {
            assert_eq!(int(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn floats_like_python() {
        let ok = [
            ("1_0", 10.0),
            (" 1.5 ", 1.5),
            ("\u{a0}1\u{3000}", 1.0),
            ("١٢", 12.0),
            ("+.5", 0.5),
            ("5.", 5.0),
            ("1e400", f64::INFINITY),
            ("1e1_0", 1e10),
            ("١.٥e٢", 150.0),
            ("Infinity", f64::INFINITY),
            ("-inf", f64::NEG_INFINITY),
        ];
        for (s, want) in ok {
            assert_eq!(float(s), Some(want), "{s:?}");
        }
        let bad =
            ["1__0", "_1", "1_", "1_.5", "1._5", "infinit", "1e", ".", "0x10", "\x1c2\x1f", "1\0", "1_e10", "ⅷ", ""];
        for s in bad {
            assert_eq!(float(s), None, "{s:?}");
        }
        let nan = float("-nan").unwrap();
        assert!(nan.is_nan() && nan.is_sign_negative());
        assert!(float("nan").unwrap().is_sign_positive());
    }
}

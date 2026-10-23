//! Just enough of Python's object model for a `json.loads`'d request spec:
//! arbitrary-size ints, NaN and infinities, `str()`/`repr()`, truthiness and
//! `int()` with the exception Python would raise.

use std::fmt;

use kiwi_core::{pynum, pystr};

/// A Python int as sign and decimal digits (no leading zeros).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PyInt {
    neg: bool,
    digits: String,
}

impl PyInt {
    fn new(neg: bool, digits: &str) -> Self {
        let d = digits.trim_start_matches('0');
        let digits = if d.is_empty() { "0".to_owned() } else { d.to_owned() };
        PyInt { neg: neg && digits != "0", digits }
    }

    pub fn from_i64(v: i64) -> Self {
        PyInt::new(v < 0, &v.unsigned_abs().to_string())
    }

    pub fn to_i128(&self) -> Option<i128> {
        let m: i128 = self.digits.parse().ok()?;
        Some(if self.neg { -m } else { m })
    }

    pub fn to_u32(&self) -> Option<u32> {
        self.to_i128().and_then(|v| u32::try_from(v).ok())
    }
}

impl fmt::Display for PyInt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.neg {
            f.write_str("-")?;
        }
        f.write_str(&self.digits)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum PyVal {
    None,
    Bool(bool),
    Int(PyInt),
    Float(f64),
    Str(String),
    List(Vec<PyVal>),
    Dict(Vec<(String, PyVal)>),
}

impl PyVal {
    /// `d.get(key)` on a dict; None for a missing key or a non-dict.
    pub fn get(&self, key: &str) -> Option<&PyVal> {
        match self {
            PyVal::Dict(items) => items.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn truthy(&self) -> bool {
        match self {
            PyVal::None => false,
            PyVal::Bool(b) => *b,
            PyVal::Int(i) => i.digits != "0",
            PyVal::Float(f) => *f != 0.0,
            PyVal::Str(s) => !s.is_empty(),
            PyVal::List(l) => !l.is_empty(),
            PyVal::Dict(d) => !d.is_empty(),
        }
    }

    fn float_repr(f: f64) -> String {
        if f.is_nan() {
            "nan".into()
        } else if f.is_infinite() {
            if f > 0.0 { "inf".into() } else { "-inf".into() }
        } else {
            crate::pyfloat::repr(f)
        }
    }

    /// `repr(v)`.
    pub fn repr(&self) -> String {
        match self {
            PyVal::Str(s) => pystr::repr(s),
            PyVal::List(l) => format!("[{}]", l.iter().map(PyVal::repr).collect::<Vec<_>>().join(", ")),
            PyVal::Dict(d) => format!(
                "{{{}}}",
                d.iter().map(|(k, v)| format!("{}: {}", pystr::repr(k), v.repr())).collect::<Vec<_>>().join(", ")
            ),
            other => other.str(),
        }
    }

    /// `str(v)`.
    pub fn str(&self) -> String {
        match self {
            PyVal::None => "None".into(),
            PyVal::Bool(true) => "True".into(),
            PyVal::Bool(false) => "False".into(),
            PyVal::Int(i) => i.to_string(),
            PyVal::Float(f) => Self::float_repr(*f),
            PyVal::Str(s) => s.clone(),
            PyVal::List(_) | PyVal::Dict(_) => self.repr(),
        }
    }
}

/// `str(v or default)` for an optional field.
pub fn str_or(v: Option<&PyVal>, default: &str) -> String {
    match v {
        Some(v) if v.truthy() => v.str(),
        _ => default.to_owned(),
    }
}

/// What `int(v)` raises.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntErr {
    /// `TypeError` / `ValueError`.
    Invalid,
    /// `OverflowError` (an infinite float).
    Overflow,
}

/// The exact integer value of a finite float, truncated toward zero.
fn float_digits(f: f64) -> PyInt {
    let t = f.trunc();
    if t.abs() < 9.2e18 {
        return PyInt::from_i64(t as i64);
    }
    let bits = t.abs().to_bits();
    let exp = ((bits >> 52) & 0x7FF) as i32 - 1075;
    let mant = (bits & ((1u64 << 52) - 1)) | (1u64 << 52);
    let mut limbs: Vec<u32> = vec![
        (mant % 1_000_000_000) as u32,
        (mant / 1_000_000_000 % 1_000_000_000) as u32,
        (mant / 1_000_000_000_000_000_000) as u32,
    ];
    for _ in 0..exp {
        let mut carry = 0u64;
        for l in &mut limbs {
            let v = u64::from(*l) * 2 + carry;
            *l = (v % 1_000_000_000) as u32;
            carry = v / 1_000_000_000;
        }
        if carry > 0 {
            limbs.push(carry as u32);
        }
    }
    while limbs.len() > 1 && limbs.last() == Some(&0) {
        limbs.pop();
    }
    let mut s = limbs.last().map_or(String::new(), u32::to_string);
    for l in limbs.iter().rev().skip(1) {
        s.push_str(&format!("{l:09}"));
    }
    PyInt::new(t < 0.0, &s)
}

/// `int(s)` for a str, base 10.
fn str_int(s: &str) -> Result<PyInt, IntErr> {
    let ascii: String = s
        .chars()
        .map(|c| {
            if pystr::is_space(c) {
                ' '
            } else if let Some(d) = pynum::decimal(c) {
                char::from_digit(d, 10).unwrap_or('?')
            } else if c.is_ascii() {
                c
            } else {
                '?'
            }
        })
        .collect();
    let t = ascii.trim_matches(|c: char| matches!(c, ' ' | '\t' | '\n' | '\x0b' | '\x0c' | '\r'));
    let (neg, body) = match t.as_bytes().first() {
        Some(b'-') => (true, &t[1..]),
        Some(b'+') => (false, &t[1..]),
        _ => (false, t),
    };
    if body.is_empty() || body.starts_with('_') || body.ends_with('_') || body.contains("__") {
        return Err(IntErr::Invalid);
    }
    if !body.bytes().all(|b| b.is_ascii_digit() || b == b'_') {
        return Err(IntErr::Invalid);
    }
    let digits: String = body.chars().filter(|c| *c != '_').collect();
    if digits.len() > 4300 {
        return Err(IntErr::Invalid);
    }
    Ok(PyInt::new(neg, &digits))
}

/// `int(v)`.
pub fn int(v: &PyVal) -> Result<PyInt, IntErr> {
    match v {
        PyVal::Bool(b) => Ok(PyInt::from_i64(i64::from(*b))),
        PyVal::Int(i) => Ok(i.clone()),
        PyVal::Float(f) if f.is_nan() => Err(IntErr::Invalid),
        PyVal::Float(f) if f.is_infinite() => Err(IntErr::Overflow),
        PyVal::Float(f) => Ok(float_digits(*f)),
        PyVal::Str(s) => str_int(s),
        PyVal::None | PyVal::List(_) | PyVal::Dict(_) => Err(IntErr::Invalid),
    }
}

/// Why `json.loads(text)` failed: a `ValueError` (`JSONDecodeError`) or a
/// `RecursionError` from nesting too deep.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoadErr {
    Invalid,
    TooDeep,
}

/// How deep the C scanner gets before `RecursionError`: CPython 3.13's C
/// recursion budget on Linux (as `pyjson::loads` takes it).
const MAX_DEPTH: usize = 10_000;

struct Scanner<'a> {
    s: &'a [u8],
    text: &'a str,
}

fn ws(s: &[u8], mut i: usize) -> usize {
    while i < s.len() && matches!(s[i], b' ' | b'\t' | b'\n' | b'\r') {
        i += 1;
    }
    i
}

impl Scanner<'_> {
    fn value(&self, i: usize, depth: usize) -> Result<(PyVal, usize), LoadErr> {
        stacker::maybe_grow(64 * 1024, 1024 * 1024, || self.value_inner(i, depth))
    }

    fn value_inner(&self, i: usize, depth: usize) -> Result<(PyVal, usize), LoadErr> {
        let s = self.s;
        let rest = &s[i.min(s.len())..];
        match rest.first() {
            Some(b'"') => self.string(i + 1).map(|(v, e)| (PyVal::Str(v), e)),
            Some(b'{') => self.object(i + 1, depth + 1),
            Some(b'[') => self.array(i + 1, depth + 1),
            Some(b'n') if rest.starts_with(b"null") => Ok((PyVal::None, i + 4)),
            Some(b't') if rest.starts_with(b"true") => Ok((PyVal::Bool(true), i + 4)),
            Some(b'f') if rest.starts_with(b"false") => Ok((PyVal::Bool(false), i + 5)),
            Some(b'N') if rest.starts_with(b"NaN") => Ok((PyVal::Float(f64::NAN), i + 3)),
            Some(b'I') if rest.starts_with(b"Infinity") => Ok((PyVal::Float(f64::INFINITY), i + 8)),
            Some(b'-') if rest.starts_with(b"-Infinity") => Ok((PyVal::Float(f64::NEG_INFINITY), i + 9)),
            Some(b'-' | b'0'..=b'9') => self.number(i),
            _ => Err(LoadErr::Invalid),
        }
    }

    fn number(&self, start: usize) -> Result<(PyVal, usize), LoadErr> {
        let s = self.s;
        let mut i = start;
        if s.get(i) == Some(&b'-') {
            i += 1;
        }
        match s.get(i) {
            Some(b'0') => i += 1,
            Some(b'1'..=b'9') => {
                while s.get(i).is_some_and(u8::is_ascii_digit) {
                    i += 1;
                }
            }
            _ => return Err(LoadErr::Invalid),
        }
        let int_end = i;
        let mut is_float = false;
        if s.get(i) == Some(&b'.') && s.get(i + 1).is_some_and(u8::is_ascii_digit) {
            i += 1;
            while s.get(i).is_some_and(u8::is_ascii_digit) {
                i += 1;
            }
            is_float = true;
        }
        if matches!(s.get(i), Some(b'e' | b'E')) {
            let mut j = i + 1;
            if matches!(s.get(j), Some(b'+' | b'-')) {
                j += 1;
            }
            if s.get(j).is_some_and(u8::is_ascii_digit) {
                while s.get(j).is_some_and(u8::is_ascii_digit) {
                    j += 1;
                }
                i = j;
                is_float = true;
            }
        }
        let text = &self.text[start..i];
        if is_float {
            let f: f64 = text.parse().map_err(|_| LoadErr::Invalid)?;
            return Ok((PyVal::Float(f), i));
        }
        let (neg, digits) = match text.strip_prefix('-') {
            Some(d) => (true, d),
            None => (false, text),
        };
        if digits.len() > 4300 {
            return Err(LoadErr::Invalid);
        }
        let _ = int_end;
        Ok((PyVal::Int(PyInt::new(neg, digits)), i))
    }

    fn string(&self, mut i: usize) -> Result<(String, usize), LoadErr> {
        let s = self.s;
        let mut out = String::new();
        loop {
            let start = i;
            while i < s.len() && s[i] != b'"' && s[i] != b'\\' && s[i] >= 0x20 {
                i += 1;
            }
            out.push_str(&self.text[start..i]);
            match s.get(i) {
                Some(b'"') => return Ok((out, i + 1)),
                Some(b'\\') => {
                    let c = *s.get(i + 1).ok_or(LoadErr::Invalid)?;
                    i += 2;
                    match c {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\x08'),
                        b'f' => out.push('\x0c'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let hex = |at: usize| -> Result<u32, LoadErr> {
                                let h = self.text.get(at..at + 4).ok_or(LoadErr::Invalid)?;
                                if !h.bytes().all(|b| b.is_ascii_hexdigit()) {
                                    return Err(LoadErr::Invalid);
                                }
                                u32::from_str_radix(h, 16).map_err(|_| LoadErr::Invalid)
                            };
                            let mut cp = hex(i)?;
                            i += 4;
                            if (0xD800..0xDC00).contains(&cp) && s.get(i..i + 2) == Some(b"\\u") {
                                let lo = hex(i + 2)?;
                                if (0xDC00..0xE000).contains(&lo) {
                                    cp = 0x10000 + ((cp - 0xD800) << 10) + (lo - 0xDC00);
                                    i += 6;
                                }
                            }
                            out.push(char::from_u32(cp).unwrap_or('\u{FFFD}'));
                        }
                        _ => return Err(LoadErr::Invalid),
                    }
                }
                _ => return Err(LoadErr::Invalid),
            }
        }
    }

    fn array(&self, i: usize, depth: usize) -> Result<(PyVal, usize), LoadErr> {
        if depth > MAX_DEPTH {
            return Err(LoadErr::TooDeep);
        }
        let s = self.s;
        let mut out = Vec::new();
        let mut i = ws(s, i);
        if s.get(i) == Some(&b']') {
            return Ok((PyVal::List(out), i + 1));
        }
        loop {
            let (v, e) = self.value(i, depth)?;
            out.push(v);
            i = ws(s, e);
            match s.get(i) {
                Some(b']') => return Ok((PyVal::List(out), i + 1)),
                Some(b',') => i = ws(s, i + 1),
                _ => return Err(LoadErr::Invalid),
            }
        }
    }

    fn object(&self, i: usize, depth: usize) -> Result<(PyVal, usize), LoadErr> {
        if depth > MAX_DEPTH {
            return Err(LoadErr::TooDeep);
        }
        let s = self.s;
        let mut out: Vec<(String, PyVal)> = Vec::new();
        let mut i = ws(s, i);
        if s.get(i) == Some(&b'}') {
            return Ok((PyVal::Dict(out), i + 1));
        }
        loop {
            if s.get(i) != Some(&b'"') {
                return Err(LoadErr::Invalid);
            }
            let (k, e) = self.string(i + 1)?;
            i = ws(s, e);
            if s.get(i) != Some(&b':') {
                return Err(LoadErr::Invalid);
            }
            let (v, e) = self.value(ws(s, i + 1), depth)?;
            match out.iter_mut().find(|(key, _)| *key == k) {
                Some(slot) => slot.1 = v,
                None => out.push((k, v)),
            }
            i = ws(s, e);
            match s.get(i) {
                Some(b'}') => return Ok((PyVal::Dict(out), i + 1)),
                Some(b',') => i = ws(s, i + 1),
                _ => return Err(LoadErr::Invalid),
            }
        }
    }
}

/// `json.loads(text)` for a `str`.
pub fn loads(text: &str) -> Result<PyVal, LoadErr> {
    if text.starts_with('\u{FEFF}') {
        return Err(LoadErr::Invalid);
    }
    let sc = Scanner { s: text.as_bytes(), text };
    let (v, end) = sc.value(ws(sc.s, 0), 0)?;
    if ws(sc.s, end) != sc.s.len() {
        return Err(LoadErr::Invalid);
    }
    Ok(v)
}

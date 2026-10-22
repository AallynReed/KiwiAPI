//! JSON exactly as Python writes it. Values are `serde_json::Value` with
//! `preserve_order`, so key order is insertion order like a Python dict.
//!
//! - [`dumps`]: Starlette `JSONResponse` = `json.dumps(ensure_ascii=False,
//!   allow_nan=False, separators=(",", ":"))` with Python float repr.
//! - [`dumps_spaced`]: plain `json.dumps(obj)` (`", "`/`": "`, ASCII-escaped),
//!   what Python writes into Redis and SSE payloads.
//! - [`pydantic`]: pydantic-core `dump_json`, what FastAPI sends for routes with
//!   a response model or return annotation (same as `dumps` except floats).
//!
//! [`PyJson`] and [`PydanticJson`] are the matching response types, and
//! [`loads`] is `json.loads(bytes)` with CPython 3.13's error messages and
//! positions (what FastAPI reports for a malformed request body).

use std::{borrow::Cow, cell::Cell, fmt::Write};

use axum::{
    body::Body,
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde_json::{Map, Number, Value};

use crate::pyfloat;

#[derive(Clone, Copy)]
enum Floats {
    Repr,
    Pydantic,
}

#[derive(Clone, Copy)]
struct Style {
    item_sep: &'static str,
    key_sep: &'static str,
    ascii: bool,
    floats: Floats,
}

const COMPACT: Style = Style { item_sep: ",", key_sep: ":", ascii: false, floats: Floats::Repr };
const SPACED: Style = Style { item_sep: ", ", key_sep: ": ", ascii: true, floats: Floats::Repr };
const PYDANTIC: Style = Style { item_sep: ",", key_sep: ":", ascii: false, floats: Floats::Pydantic };

/// `json.dumps(value, ensure_ascii=False, separators=(",", ":"))`.
pub fn dumps(value: &Value) -> String {
    render(value, COMPACT)
}

/// `json.dumps(value)` with Python's defaults (spaced separators, ASCII only).
pub fn dumps_spaced(value: &Value) -> String {
    render(value, SPACED)
}

/// pydantic-core `dump_json` of an already-built value.
pub fn pydantic(value: &Value) -> String {
    render(value, PYDANTIC)
}

fn render(value: &Value, style: Style) -> String {
    let mut out = String::with_capacity(128);
    write_value(&mut out, value, style);
    out
}

fn write_value(out: &mut String, value: &Value, style: Style) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                let _ = write!(out, "{i}");
            } else if let Some(u) = n.as_u64() {
                let _ = write!(out, "{u}");
            } else {
                let f = n.as_f64().unwrap_or(f64::NAN);
                out.push_str(&match style.floats {
                    Floats::Repr => pyfloat::repr(f),
                    Floats::Pydantic => pyfloat::pydantic(f),
                });
            }
        }
        Value::String(s) => write_str(out, s, style.ascii),
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(style.item_sep);
                }
                write_value(out, item, style);
            }
            out.push(']');
        }
        Value::Object(map) => {
            out.push('{');
            for (i, (k, v)) in map.iter().enumerate() {
                if i > 0 {
                    out.push_str(style.item_sep);
                }
                write_str(out, k, style.ascii);
                out.push_str(style.key_sep);
                write_value(out, v, style);
            }
            out.push('}');
        }
    }
}

const HEX: &[u8; 16] = b"0123456789abcdef";

fn push_u16_escape(out: &mut String, u: u16) {
    out.push_str("\\u");
    for shift in [12, 8, 4, 0] {
        out.push(HEX[((u >> shift) & 0xf) as usize] as char);
    }
}

/// A JSON string literal the way Python's encoder writes it: `"`, `\`,
/// `\b\f\n\r\t` get short escapes, other control characters `\u00XX`
/// (lowercase hex); with `ascii`, DEL and every non-ASCII character too
/// (astral ones as a surrogate pair).
pub fn write_str(out: &mut String, s: &str, ascii: bool) {
    out.push('"');
    let mut start = 0;
    for (i, c) in s.char_indices() {
        let short = match c {
            '"' => Some("\\\""),
            '\\' => Some("\\\\"),
            '\n' => Some("\\n"),
            '\r' => Some("\\r"),
            '\t' => Some("\\t"),
            '\u{08}' => Some("\\b"),
            '\u{0c}' => Some("\\f"),
            _ => None,
        };
        let needs = short.is_some() || c < ' ' || (ascii && c > '~');
        if !needs {
            continue;
        }
        out.push_str(&s[start..i]);
        start = i + c.len_utf8();
        if let Some(esc) = short {
            out.push_str(esc);
        } else {
            let mut units = [0u16; 2];
            for u in c.encode_utf16(&mut units) {
                push_u16_escape(out, *u);
            }
        }
    }
    out.push_str(&s[start..]);
    out.push('"');
}

const APPLICATION_JSON: HeaderValue = HeaderValue::from_static("application/json");

/// An already-rendered JSON body.
pub fn json_response(body: impl Into<Body>) -> Response {
    ([(header::CONTENT_TYPE, APPLICATION_JSON)], body.into()).into_response()
}

/// The 204 FastAPI sends for a `-> None` route: JSON content type, no body.
pub fn no_content() -> Response {
    (StatusCode::NO_CONTENT, [(header::CONTENT_TYPE, APPLICATION_JSON)]).into_response()
}

/// A Starlette `JSONResponse` (`Content-Type: application/json`, body
/// [`dumps`]). Combine with a status as `(StatusCode::CREATED, PyJson(v))`.
#[derive(Clone, Debug)]
pub struct PyJson(pub Value);

impl IntoResponse for PyJson {
    fn into_response(self) -> Response {
        json_response(dumps(&self.0))
    }
}

/// A FastAPI response-model response (`Content-Type: application/json`, body
/// [`pydantic`]).
#[derive(Clone, Debug)]
pub struct PydanticJson(pub Value);

impl IntoResponse for PydanticJson {
    fn into_response(self) -> Response {
        json_response(pydantic(&self.0))
    }
}

/// Why [`loads`] rejected a document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoadsError {
    /// `json.JSONDecodeError`: `msg` and `pos` (a character index), which
    /// FastAPI turns into a 422 `json_invalid` error.
    Syntax { msg: &'static str, pos: usize },
    /// A non-`JSONDecodeError` failure (undecodable bytes, an integer over
    /// 4300 digits, nesting too deep): FastAPI's 400 "There was an error
    /// parsing the body".
    Decode,
    /// A valid document holding `NaN`, `Infinity` or a float overflow, which
    /// `serde_json::Value` cannot hold (Python accepts them).
    NonFinite,
}

/// Nesting limit; CPython's is its C recursion budget (~10000 on Linux).
const MAX_DEPTH: usize = 10_000;
/// `sys.int_info.default_max_str_digits`.
const MAX_INT_DIGITS: usize = 4300;

/// `json.loads(bytes)`: encoding detection (UTF-8, UTF-8 with BOM, UTF-16/32),
/// then CPython's C scanner. Integers outside `i64`/`u64` become floats and
/// lone `\uD800`-style surrogates become U+FFFD; Python keeps both.
pub fn loads(bytes: &[u8]) -> Result<Value, LoadsError> {
    let text = decode_bytes(bytes).ok_or(LoadsError::Decode)?;
    let sc = Scanner { s: &text, b: text.as_bytes(), nonfinite: Cell::new(false) };
    let syntax = |msg, at: usize| LoadsError::Syntax { msg, pos: text[..at].chars().count() };
    let start = skip_ws(sc.b, 0);
    let (value, end) = sc.value(start).map_err(|e| match e {
        ScanErr::Stop(at) => syntax("Expecting value", at),
        ScanErr::Msg(msg, at) => syntax(msg, at),
        ScanErr::Decode => LoadsError::Decode,
    })?;
    let end = skip_ws(sc.b, end);
    if end != sc.b.len() {
        return Err(syntax("Extra data", end));
    }
    if sc.nonfinite.get() {
        return Err(LoadsError::NonFinite);
    }
    Ok(value)
}

/// `bytes.decode(json.detect_encoding(b))`.
fn decode_bytes(b: &[u8]) -> Option<Cow<'_, str>> {
    fn units<const N: usize>(b: &[u8], be: bool) -> Option<Vec<u32>> {
        if !b.len().is_multiple_of(N) {
            return None;
        }
        let word = |c: &[u8]| {
            let it = c.iter().map(|&x| x as u32);
            if be { it.fold(0, |a, x| a << 8 | x) } else { it.rev().fold(0, |a, x| a << 8 | x) }
        };
        Some(b.chunks_exact(N).map(word).collect())
    }
    fn utf16(b: &[u8], be: bool) -> Option<Cow<'static, str>> {
        let u = units::<2>(b, be)?;
        char::decode_utf16(u.into_iter().map(|x| x as u16)).collect::<Result<String, _>>().ok().map(Cow::Owned)
    }
    fn utf32(b: &[u8], be: bool) -> Option<Cow<'static, str>> {
        units::<4>(b, be)?.into_iter().map(char::from_u32).collect::<Option<String>>().map(Cow::Owned)
    }
    let utf8 = |b| match std::str::from_utf8(b) {
        Ok(s) => Some(Cow::Borrowed(s)),
        Err(_) => utf8_surrogatepass(b).map(Cow::Owned),
    };
    match b {
        [0, 0, 0xfe, 0xff, rest @ ..] => utf32(rest, true),
        [0xff, 0xfe, 0, 0, rest @ ..] => utf32(rest, false),
        [0xfe, 0xff, rest @ ..] => utf16(rest, true),
        [0xff, 0xfe, rest @ ..] => utf16(rest, false),
        [0xef, 0xbb, 0xbf, rest @ ..] => utf8(rest),
        [0, b1, _, _, ..] if *b1 != 0 => utf16(b, true),
        [0, _, _, _, ..] => utf32(b, true),
        [_, 0, b2, b3, ..] if *b2 != 0 || *b3 != 0 => utf16(b, false),
        [_, 0, _, _, ..] => utf32(b, false),
        [0, _] => utf16(b, true),
        [_, 0] => utf16(b, false),
        _ => utf8(b),
    }
}

/// UTF-8 with Python's `surrogatepass`: encoded surrogates (`ED A0..BF ..`)
/// are accepted; they become U+FFFD, the stand-in for a lone surrogate here.
fn utf8_surrogatepass(mut b: &[u8]) -> Option<String> {
    let mut out = String::with_capacity(b.len());
    loop {
        match std::str::from_utf8(b) {
            Ok(s) => {
                out.push_str(s);
                return Some(out);
            }
            Err(e) => {
                let (good, bad) = b.split_at(e.valid_up_to());
                out.push_str(std::str::from_utf8(good).ok()?);
                let [0xed, 0xa0..=0xbf, 0x80..=0xbf, tail @ ..] = bad else {
                    return None;
                };
                out.push('\u{fffd}');
                b = tail;
            }
        }
    }
}

fn skip_ws(b: &[u8], mut i: usize) -> usize {
    while i < b.len() && matches!(b[i], b' ' | b'\t' | b'\n' | b'\r') {
        i += 1;
    }
    i
}

enum ScanErr {
    /// `StopIteration(idx)`, reported as "Expecting value".
    Stop(usize),
    Msg(&'static str, usize),
    Decode,
}

/// Byte-indexed port of `scan_once_unicode` and friends (`Modules/_json.c`).
/// Every structural character is ASCII, so byte positions are converted to
/// character positions only when an error is reported. Non-finite numbers
/// parse (as null) and are reported once the whole document is valid.
struct Scanner<'s> {
    s: &'s str,
    b: &'s [u8],
    nonfinite: Cell<bool>,
}

impl Scanner<'_> {
    fn non_finite(&self) -> Value {
        self.nonfinite.set(true);
        Value::Null
    }

    fn float(&self, text: &str) -> Result<Value, ScanErr> {
        let f: f64 = text.parse().map_err(|_| ScanErr::Decode)?;
        Ok(Number::from_f64(f).map(Value::Number).unwrap_or_else(|| self.non_finite()))
    }
}

enum Frame {
    Array(Vec<Value>),
    Object(Map<String, Value>, String),
}

impl Scanner<'_> {
    fn value(&self, mut idx: usize) -> Result<(Value, usize), ScanErr> {
        let b = self.b;
        let mut stack: Vec<Frame> = Vec::new();
        'scan: loop {
            let rest = &b[idx.min(b.len())..];
            let (mut val, mut end) = match rest.first() {
                None => return Err(ScanErr::Stop(idx)),
                Some(b'"') => {
                    let (s, e) = self.string(idx + 1)?;
                    (Value::String(s), e)
                }
                Some(&open @ (b'{' | b'[')) => {
                    if stack.len() >= MAX_DEPTH {
                        return Err(ScanErr::Decode);
                    }
                    let i = skip_ws(b, idx + 1);
                    match (open, b.get(i)) {
                        (b'{', Some(b'}')) => (Value::Object(Map::new()), i + 1),
                        (b'[', Some(b']')) => (Value::Array(Vec::new()), i + 1),
                        (b'{', _) => {
                            let (key, j) = self.key(i)?;
                            stack.push(Frame::Object(Map::new(), key));
                            idx = j;
                            continue 'scan;
                        }
                        _ => {
                            stack.push(Frame::Array(Vec::new()));
                            idx = i;
                            continue 'scan;
                        }
                    }
                }
                _ if rest.starts_with(b"null") => (Value::Null, idx + 4),
                _ if rest.starts_with(b"true") => (Value::Bool(true), idx + 4),
                _ if rest.starts_with(b"false") => (Value::Bool(false), idx + 5),
                _ if rest.starts_with(b"NaN") => (self.non_finite(), idx + 3),
                _ if rest.starts_with(b"Infinity") => (self.non_finite(), idx + 8),
                _ if rest.starts_with(b"-Infinity") => (self.non_finite(), idx + 9),
                _ => self.number(idx)?,
            };
            loop {
                let Some(top) = stack.last_mut() else {
                    return Ok((val, end));
                };
                let close = match top {
                    Frame::Array(items) => {
                        items.push(val);
                        b']'
                    }
                    Frame::Object(map, key) => {
                        map.insert(std::mem::take(key), val);
                        b'}'
                    }
                };
                let i = skip_ws(b, end);
                match b.get(i) {
                    Some(&c) if c == close => {
                        val = match stack.pop() {
                            Some(Frame::Array(items)) => Value::Array(items),
                            Some(Frame::Object(map, _)) => Value::Object(map),
                            None => unreachable!("frame checked above"),
                        };
                        end = i + 1;
                    }
                    Some(b',') => {
                        let j = skip_ws(b, i + 1);
                        if b.get(j) == Some(&close) {
                            let msg = if close == b']' {
                                "Illegal trailing comma before end of array"
                            } else {
                                "Illegal trailing comma before end of object"
                            };
                            return Err(ScanErr::Msg(msg, i));
                        }
                        idx = j;
                        if let Some(Frame::Object(_, key)) = stack.last_mut() {
                            let (k, after) = self.key(j)?;
                            *key = k;
                            idx = after;
                        }
                        continue 'scan;
                    }
                    _ => return Err(ScanErr::Msg("Expecting ',' delimiter", i)),
                }
            }
        }
    }

    /// A key, the `:` delimiter and the whitespace after it.
    fn key(&self, i: usize) -> Result<(String, usize), ScanErr> {
        let b = self.b;
        if b.get(i) != Some(&b'"') {
            return Err(ScanErr::Msg("Expecting property name enclosed in double quotes", i));
        }
        let (key, j) = self.string(i + 1)?;
        let j = skip_ws(b, j);
        if b.get(j) != Some(&b':') {
            return Err(ScanErr::Msg("Expecting ':' delimiter", j));
        }
        Ok((key, skip_ws(b, j + 1)))
    }

    /// `_match_number_unicode`.
    fn number(&self, start: usize) -> Result<(Value, usize), ScanErr> {
        let b = self.b;
        let n = b.len();
        let digit = |i: usize| i < n && b[i].is_ascii_digit();
        let mut idx = start;
        if b[idx] == b'-' {
            idx += 1;
            if idx >= n {
                return Err(ScanErr::Stop(start));
            }
        }
        match b[idx] {
            b'1'..=b'9' => {
                idx += 1;
                while digit(idx) {
                    idx += 1;
                }
            }
            b'0' => idx += 1,
            _ => return Err(ScanErr::Stop(start)),
        }
        let mut is_float = false;
        if idx + 1 < n && b[idx] == b'.' && b[idx + 1].is_ascii_digit() {
            is_float = true;
            idx += 2;
            while digit(idx) {
                idx += 1;
            }
        }
        if idx + 1 < n && (b[idx] == b'e' || b[idx] == b'E') {
            let e_start = idx;
            idx += 1;
            if idx + 1 < n && (b[idx] == b'-' || b[idx] == b'+') {
                idx += 1;
            }
            while digit(idx) {
                idx += 1;
            }
            if b[idx - 1].is_ascii_digit() {
                is_float = true;
            } else {
                idx = e_start;
            }
        }
        let text = &self.s[start..idx];
        let value = if is_float {
            self.float(text)?
        } else if text.trim_start_matches('-').len() > MAX_INT_DIGITS {
            return Err(ScanErr::Decode);
        } else if let Ok(i) = text.parse::<i64>() {
            Value::from(i)
        } else if let Ok(u) = text.parse::<u64>() {
            Value::from(u)
        } else {
            self.float(text)?
        };
        Ok((value, idx))
    }

    /// `scanstring_unicode` (strict): `end` is the index after the opening quote.
    fn string(&self, mut end: usize) -> Result<(String, usize), ScanErr> {
        let b = self.b;
        let len = b.len();
        let begin = end - 1;
        let unterminated = || ScanErr::Msg("Unterminated string starting at", begin);
        let bad_u = |at| ScanErr::Msg("Invalid \\uXXXX escape", at);
        let mut out = String::new();
        loop {
            let mut next = end;
            while next < len && b[next] != b'"' && b[next] != b'\\' {
                if b[next] <= 0x1f {
                    return Err(ScanErr::Msg("Invalid control character at", next));
                }
                next += 1;
            }
            if next >= len {
                return Err(unterminated());
            }
            out.push_str(&self.s[end..next]);
            let c = b[next];
            next += 1;
            if c == b'"' {
                return Ok((out, next));
            }
            if next == len {
                return Err(unterminated());
            }
            if b[next] != b'u' {
                end = next + 1;
                out.push(match b[next] {
                    b'"' => '"',
                    b'\\' => '\\',
                    b'/' => '/',
                    b'b' => '\u{8}',
                    b'f' => '\u{c}',
                    b'n' => '\n',
                    b'r' => '\r',
                    b't' => '\t',
                    _ => return Err(ScanErr::Msg("Invalid \\escape", end - 2)),
                });
                continue;
            }
            next += 1;
            end = next + 4;
            if end > len {
                return Err(bad_u(next - 1));
            }
            let mut c = hex4(&b[next..end]).ok_or_else(|| bad_u(end - 5))?;
            if (0xd800..0xdc00).contains(&c) && end + 6 <= len && b[end] == b'\\' && b[end + 1] == b'u' {
                let c2 = hex4(&b[end + 2..end + 6]).ok_or_else(|| bad_u(end + 1))?;
                if (0xdc00..0xe000).contains(&c2) {
                    c = 0x10000 + ((c - 0xd800) << 10) + (c2 - 0xdc00);
                    end += 6;
                }
            }
            out.push(char::from_u32(c).unwrap_or('\u{fffd}'));
        }
    }
}

fn hex4(b: &[u8]) -> Option<u32> {
    b.iter().try_fold(0u32, |acc, &d| Some(acc << 4 | (d as char).to_digit(16)?))
}

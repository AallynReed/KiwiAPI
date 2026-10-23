//! `str.format(**kwargs)` over `str` and `int` keyword arguments, as CPython
//! 3.13 runs it (`unicode_format.h`, `formatter_unicode.c`): fields, conversions
//! and the format-spec mini-language, failing at the same point with the same
//! class of exception.

use kiwi_core::{pynum, pystr};

#[derive(Clone, Copy, Debug)]
pub enum Arg<'a> {
    Int(i64),
    Str(&'a str),
}

/// Which exception `str.format` raises.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FormatError {
    /// `KeyError`, `IndexError` or `ValueError`.
    Lookup,
    /// `AttributeError`, `TypeError`, `OverflowError` or `MemoryError`.
    Other,
}

use FormatError::{Lookup, Other};

#[derive(Clone)]
enum Obj {
    Int(i64),
    Str(String),
}

#[derive(Default)]
struct Auto {
    state: u8,
    next: usize,
}

const MAX_WIDTH: usize = 1 << 24;

pub fn format(template: &str, kwargs: &[(&str, Arg<'_>)]) -> Result<String, FormatError> {
    let chars: Vec<char> = template.chars().collect();
    build(&chars, kwargs, 2, &mut Auto::default())
}

fn build(s: &[char], kw: &[(&str, Arg<'_>)], depth: i32, auto: &mut Auto) -> Result<String, FormatError> {
    if depth <= 0 {
        return Err(Lookup);
    }
    let mut out = String::new();
    let n = s.len();
    let mut i = 0;
    while i < n {
        let start = i;
        let mut c = '\0';
        let mut markup = false;
        while i < n {
            c = s[i];
            i += 1;
            if c == '{' || c == '}' {
                markup = true;
                break;
            }
        }
        let at_end = i >= n;
        let mut len = i - start;
        if c == '}' && markup && (at_end || s[i] != '}') {
            return Err(Lookup);
        }
        if at_end && markup && c == '{' {
            return Err(Lookup);
        }
        if !at_end && markup {
            if s[i] == c {
                i += 1;
                markup = false;
            } else {
                len -= 1;
            }
        }
        out.extend(&s[start..start + len]);
        if !markup {
            continue;
        }
        let field = parse_field(s, &mut i)?;
        let mut obj = field_object(&s[field.name.0..field.name.1], kw, auto)?;
        match field.conversion {
            None | Some('\0') => {}
            Some('s') => obj = Obj::Str(to_str(&obj)),
            Some('r') => obj = Obj::Str(repr(&obj)),
            Some('a') => obj = Obj::Str(ascii(&repr(&obj))),
            Some(_) => return Err(Lookup),
        }
        let spec: Vec<char> = if field.expand {
            build(&s[field.spec.0..field.spec.1], kw, depth - 1, auto)?.chars().collect()
        } else {
            s[field.spec.0..field.spec.1].to_vec()
        };
        out.push_str(&render(&obj, &spec)?);
    }
    Ok(out)
}

struct Field {
    name: (usize, usize),
    conversion: Option<char>,
    spec: (usize, usize),
    expand: bool,
}

/// `parse_field`, from just after the opening brace.
fn parse_field(s: &[char], i: &mut usize) -> Result<Field, FormatError> {
    let n = s.len();
    let start = *i;
    let mut c = '\0';
    while *i < n {
        c = s[*i];
        *i += 1;
        match c {
            '{' => return Err(Lookup),
            '[' => {
                while *i < n && s[*i] != ']' {
                    *i += 1;
                }
            }
            '}' | ':' | '!' => break,
            _ => {}
        }
    }
    let name = (start, (*i).saturating_sub(1).max(start));
    if c == '!' || c == ':' {
        let mut conversion = None;
        if c == '!' {
            if *i >= n {
                return Err(Lookup);
            }
            conversion = Some(s[*i]);
            *i += 1;
            if *i < n {
                let c2 = s[*i];
                *i += 1;
                if c2 == '}' {
                    return Ok(Field { name, conversion, spec: (*i, *i), expand: false });
                }
                if c2 != ':' {
                    return Err(Lookup);
                }
            }
        }
        let spec_start = *i;
        let (mut count, mut expand) = (1, false);
        while *i < n {
            let c3 = s[*i];
            *i += 1;
            if c3 == '{' {
                expand = true;
                count += 1;
            } else if c3 == '}' {
                count -= 1;
                if count == 0 {
                    return Ok(Field { name, conversion, spec: (spec_start, *i - 1), expand });
                }
            }
        }
        return Err(Lookup);
    }
    if c != '}' {
        return Err(Lookup);
    }
    Ok(Field { name, conversion: None, spec: (*i, *i), expand: false })
}

/// The field-name `get_integer`: `None` for an empty or non-decimal name.
fn field_integer(s: &[char]) -> Result<Option<usize>, FormatError> {
    if s.is_empty() {
        return Ok(None);
    }
    let mut acc: usize = 0;
    for &c in s {
        let Some(d) = pynum::decimal(c) else { return Ok(None) };
        acc = acc
            .checked_mul(10)
            .and_then(|a| a.checked_add(d as usize))
            .filter(|&a| a <= isize::MAX as usize)
            .ok_or(Lookup)?;
    }
    Ok(Some(acc))
}

/// `get_field_object` with an empty positional tuple.
fn field_object(name: &[char], kw: &[(&str, Arg<'_>)], auto: &mut Auto) -> Result<Obj, FormatError> {
    let first_end = name.iter().position(|&c| c == '.' || c == '[').unwrap_or(name.len());
    let first = &name[..first_end];
    let idx = field_integer(first)?;
    let empty = first.is_empty();
    if empty || idx.is_some() {
        if auto.state == 0 {
            auto.state = if empty { 1 } else { 2 };
        }
        if (auto.state == 2 && empty) || (auto.state == 1 && !empty) {
            return Err(Lookup);
        }
        if empty {
            auto.next += 1;
        }
        return Err(Lookup);
    }
    let key: String = first.iter().collect();
    let mut obj = match kw.iter().rev().find(|(k, _)| *k == key) {
        Some((_, Arg::Int(v))) => Obj::Int(*v),
        Some((_, Arg::Str(v))) => Obj::Str((*v).to_owned()),
        None => return Err(Lookup),
    };
    let rest = &name[first_end..];
    let mut i = 0;
    while i < rest.len() {
        let c = rest[i];
        i += 1;
        match c {
            '.' => {
                let st = i;
                while i < rest.len() && rest[i] != '.' && rest[i] != '[' {
                    i += 1;
                }
                if st == i {
                    return Err(Lookup);
                }
                let attr: String = rest[st..i].iter().collect();
                obj = match (&obj, attr.as_str()) {
                    (Obj::Int(v), "real" | "numerator") => Obj::Int(*v),
                    (Obj::Int(_), "imag") => Obj::Int(0),
                    (Obj::Int(_), "denominator") => Obj::Int(1),
                    _ => return Err(Other),
                };
            }
            '[' => {
                let st = i;
                while i < rest.len() && rest[i] != ']' {
                    i += 1;
                }
                if i >= rest.len() {
                    return Err(Lookup);
                }
                let item = &rest[st..i];
                i += 1;
                if item.is_empty() {
                    return Err(Lookup);
                }
                let idx = field_integer(item)?;
                obj = match (&obj, idx) {
                    (Obj::Str(s), Some(k)) => Obj::Str(s.chars().nth(k).ok_or(Lookup)?.to_string()),
                    _ => return Err(Other),
                };
            }
            _ => return Err(Lookup),
        }
    }
    Ok(obj)
}

fn to_str(o: &Obj) -> String {
    match o {
        Obj::Int(v) => v.to_string(),
        Obj::Str(s) => s.clone(),
    }
}

fn repr(o: &Obj) -> String {
    match o {
        Obj::Int(v) => v.to_string(),
        Obj::Str(s) => pystr::repr(s),
    }
}

/// `ascii()` from `repr()`: the non-ASCII characters it kept, escaped.
fn ascii(r: &str) -> String {
    let mut out = String::with_capacity(r.len());
    for c in r.chars() {
        let n = c as u32;
        match n {
            0..0x80 => out.push(c),
            0x80..0x100 => out.push_str(&format!("\\x{n:02x}")),
            0x100..0x10000 => out.push_str(&format!("\\u{n:04x}")),
            _ => out.push_str(&format!("\\U{n:08x}")),
        }
    }
    out
}

#[derive(Clone, Copy, PartialEq)]
enum Sep {
    None,
    Comma,
    Under,
    UnderFour,
}

struct Spec {
    fill: char,
    align: char,
    alternate: bool,
    no_neg_0: bool,
    sign: char,
    width: Option<usize>,
    sep: Sep,
    precision: Option<usize>,
    kind: char,
}

/// The format-spec `get_integer`: digits consumed and their value.
fn spec_integer(s: &[char], pos: &mut usize) -> Result<(usize, usize), FormatError> {
    let (mut acc, mut n): (usize, usize) = (0, 0);
    while *pos < s.len() {
        let Some(d) = pynum::decimal(s[*pos]) else { break };
        acc = acc
            .checked_mul(10)
            .and_then(|a| a.checked_add(d as usize))
            .filter(|&a| a <= isize::MAX as usize)
            .ok_or(Lookup)?;
        *pos += 1;
        n += 1;
    }
    Ok((n, acc))
}

/// `parse_internal_render_format_spec`.
fn parse_spec(s: &[char], default_kind: char, default_align: char) -> Result<Spec, FormatError> {
    let align_token = |c: char| matches!(c, '<' | '>' | '=' | '^');
    let mut f = Spec {
        fill: ' ',
        align: default_align,
        alternate: false,
        no_neg_0: false,
        sign: '\0',
        width: None,
        sep: Sep::None,
        precision: None,
        kind: default_kind,
    };
    let end = s.len();
    let mut pos = 0;
    let (mut fill_given, mut align_given) = (false, false);
    if end >= 2 && align_token(s[1]) {
        f.align = s[1];
        f.fill = s[0];
        fill_given = true;
        align_given = true;
        pos = 2;
    } else if end >= 1 && align_token(s[0]) {
        f.align = s[0];
        align_given = true;
        pos = 1;
    }
    if pos < end && matches!(s[pos], ' ' | '+' | '-') {
        f.sign = s[pos];
        pos += 1;
    }
    if pos < end && s[pos] == 'z' {
        f.no_neg_0 = true;
        pos += 1;
    }
    if pos < end && s[pos] == '#' {
        f.alternate = true;
        pos += 1;
    }
    if !fill_given && pos < end && s[pos] == '0' {
        f.fill = '0';
        if !align_given && default_align == '>' {
            f.align = '=';
        }
        pos += 1;
    }
    let (n, width) = spec_integer(s, &mut pos)?;
    if n > 0 {
        f.width = Some(width);
    }
    if pos < end && s[pos] == ',' {
        f.sep = Sep::Comma;
        pos += 1;
    }
    if pos < end && s[pos] == '_' {
        if f.sep != Sep::None {
            return Err(Lookup);
        }
        f.sep = Sep::Under;
        pos += 1;
    }
    if pos < end && s[pos] == ',' && f.sep == Sep::Under {
        return Err(Lookup);
    }
    if pos < end && s[pos] == '.' {
        pos += 1;
        let (n, p) = spec_integer(s, &mut pos)?;
        if n == 0 {
            return Err(Lookup);
        }
        f.precision = Some(p);
    }
    if end - pos > 1 {
        return Err(Lookup);
    }
    if end - pos == 1 {
        f.kind = s[pos];
    }
    if f.sep != Sep::None {
        match f.kind {
            'd' | 'e' | 'f' | 'g' | 'E' | 'G' | '%' | 'F' | '\0' => {}
            'b' | 'o' | 'x' | 'X' if f.sep == Sep::Under => f.sep = Sep::UnderFour,
            _ => return Err(Lookup),
        }
    }
    if f.width.is_some_and(|w| w > MAX_WIDTH) {
        return Err(Other);
    }
    Ok(f)
}

fn render(o: &Obj, spec: &[char]) -> Result<String, FormatError> {
    match o {
        Obj::Str(s) if spec.is_empty() => Ok(s.clone()),
        Obj::Int(v) if spec.is_empty() => Ok(v.to_string()),
        Obj::Str(s) => {
            let f = parse_spec(spec, 's', '<')?;
            if f.kind != 's' {
                return Err(Lookup);
            }
            format_str(s, &f)
        }
        Obj::Int(v) => {
            let f = parse_spec(spec, 'd', '>')?;
            match f.kind {
                'b' | 'c' | 'd' | 'o' | 'x' | 'X' | 'n' => format_int(*v, &f),
                'e' | 'E' | 'f' | 'F' | 'g' | 'G' | '%' => format_float(*v as f64, &f),
                _ => Err(Lookup),
            }
        }
    }
}

fn pad(out: &mut String, fill: char, n: usize) {
    out.extend(std::iter::repeat_n(fill, n));
}

/// `format_string_internal`.
fn format_str(s: &str, f: &Spec) -> Result<String, FormatError> {
    if f.sign != '\0' || f.no_neg_0 || f.alternate || f.align == '=' {
        return Err(Lookup);
    }
    let mut chars: Vec<char> = s.chars().collect();
    if let Some(p) = f.precision
        && chars.len() > p
    {
        chars.truncate(p);
    }
    let len = chars.len();
    let total = f.width.filter(|&w| w > len).unwrap_or(len);
    let left = match f.align {
        '>' => total - len,
        '^' => (total - len) / 2,
        _ => 0,
    };
    let mut out = String::new();
    pad(&mut out, f.fill, left);
    out.extend(chars);
    pad(&mut out, f.fill, total - len - left);
    Ok(out)
}

/// `_PyUnicode_InsertThousandsGrouping` over plain ASCII digits.
fn group(digits: &str, min_width: isize, sep: Sep) -> String {
    let (size, sep_s): (isize, &str) = match sep {
        Sep::None => (0, ""),
        Sep::Comma => (3, ","),
        Sep::Under => (3, "_"),
        Sep::UnderFour => (4, "_"),
    };
    let sep_len = sep_s.len() as isize;
    let bytes = digits.as_bytes();
    let mut remaining = bytes.len() as isize;
    let mut min_width = min_width;
    let mut chunks: Vec<String> = Vec::new();
    let mut take = |n_zeros: isize, n_chars: isize, remaining: isize, use_sep: bool| {
        let mut c = "0".repeat(n_zeros.max(0) as usize);
        c.push_str(std::str::from_utf8(&bytes[(remaining - n_chars) as usize..remaining as usize]).unwrap_or(""));
        if use_sep {
            c.push_str(sep_s);
        }
        chunks.push(c);
    };
    let mut use_sep = false;
    let mut broken = false;
    if size > 0 {
        loop {
            let l = size.min(remaining.max(min_width).max(1));
            let n_zeros = (l - remaining).max(0);
            let n_chars = remaining.min(l).max(0);
            take(n_zeros, n_chars, remaining, use_sep);
            use_sep = true;
            remaining -= n_chars;
            min_width -= l;
            if remaining <= 0 && min_width <= 0 {
                broken = true;
                break;
            }
            min_width -= sep_len;
        }
    }
    if !broken {
        let l = remaining.max(min_width).max(1);
        let n_zeros = (l - remaining).max(0);
        let n_chars = remaining.min(l).max(0);
        take(n_zeros, n_chars, remaining, use_sep);
    }
    chunks.iter().rev().map(String::as_str).collect()
}

/// `calc_number_widths` + `fill_number` for `[sign][prefix][digits][remainder]`.
fn layout(f: &Spec, negative: bool, prefix: &str, digits: &str, remainder: &str, decimal: bool) -> String {
    let sign = match (f.sign, negative) {
        (_, true) => Some('-'),
        ('+', false) => Some('+'),
        (' ', false) => Some(' '),
        _ => None,
    };
    let n_non_digit = sign.is_some() as isize
        + prefix.chars().count() as isize
        + decimal as isize
        + remainder.chars().count() as isize;
    let width = f.width.map_or(-1, |w| w as isize);
    let min_width = if f.fill == '0' && f.align == '=' { width - n_non_digit } else { 0 };
    let grouped = if digits.is_empty() { String::new() } else { group(digits, min_width, f.sep) };
    let padding = width - (n_non_digit + grouped.chars().count() as isize);
    let (mut l, mut s, mut r) = (0, 0, 0);
    if padding > 0 {
        let p = padding as usize;
        match f.align {
            '<' => r = p,
            '^' => {
                l = p / 2;
                r = p - l;
            }
            '=' => s = p,
            _ => l = p,
        }
    }
    let mut out = String::new();
    pad(&mut out, f.fill, l);
    out.extend(sign);
    out.push_str(prefix);
    pad(&mut out, f.fill, s);
    out.push_str(&grouped);
    if decimal {
        out.push('.');
    }
    out.push_str(remainder);
    pad(&mut out, f.fill, r);
    out
}

/// `format_long_internal`.
fn format_int(v: i64, f: &Spec) -> Result<String, FormatError> {
    if f.precision.is_some() || f.no_neg_0 {
        return Err(Lookup);
    }
    if f.kind == 'c' {
        if f.sign != '\0' || f.alternate {
            return Err(Lookup);
        }
        let ch = u32::try_from(v).ok().filter(|&x| x <= 0x10ffff).ok_or(Other)?;
        let ch = char::from_u32(ch).ok_or(Other)?;
        return Ok(layout(f, false, "", "", &ch.to_string(), false));
    }
    let magnitude = v.unsigned_abs();
    let (digits, prefix) = match f.kind {
        'b' => (format!("{magnitude:b}"), "0b"),
        'o' => (format!("{magnitude:o}"), "0o"),
        'x' => (format!("{magnitude:x}"), "0x"),
        'X' => (format!("{magnitude:X}"), "0X"),
        _ => (magnitude.to_string(), ""),
    };
    let prefix = if f.alternate { prefix } else { "" };
    let sep = if f.kind == 'n' { Sep::None } else { f.sep };
    let spec = Spec { sep, ..*f };
    Ok(layout(&spec, v < 0, prefix, &digits, "", false))
}

/// `PyOS_double_to_string` for the `e`, `f` and `g` families of a finite value.
fn double_to_string(x: f64, kind: char, precision: usize, alternate: bool) -> String {
    let exp_form = |p: usize| {
        let s = format!("{x:.p$e}");
        let (mant, exp) = s.split_once('e').unwrap_or((&s, "0"));
        let e: i32 = exp.parse().unwrap_or(0);
        let mut mant = mant.to_owned();
        if alternate && !mant.contains('.') {
            mant.push('.');
        }
        (mant, e)
    };
    let exp_str = |e: i32| format!("e{}{:02}", if e < 0 { '-' } else { '+' }, e.unsigned_abs());
    let out = match kind.to_ascii_lowercase() {
        'f' => {
            let mut s = format!("{x:.precision$}");
            if alternate && !s.contains('.') {
                s.push('.');
            }
            s
        }
        'e' => {
            let (mant, e) = exp_form(precision);
            mant + &exp_str(e)
        }
        _ => {
            let p = precision.max(1);
            let (_, e) = exp_form(p - 1);
            let strip = |s: String| {
                if alternate || !s.contains('.') {
                    return s;
                }
                let t = s.trim_end_matches('0');
                t.strip_suffix('.').unwrap_or(t).to_owned()
            };
            if -4 <= e && (e as i64) < p as i64 {
                let decimals = (p as i64 - 1 - e as i64) as usize;
                let mut s = format!("{x:.decimals$}");
                if alternate && !s.contains('.') {
                    s.push('.');
                }
                strip(s)
            } else {
                let (mant, e) = exp_form(p - 1);
                strip(mant) + &exp_str(e)
            }
        }
    };
    if kind.is_ascii_uppercase() { out.to_uppercase() } else { out }
}

/// `format_float_internal` for an `int` given a float presentation type.
fn format_float(x: f64, f: &Spec) -> Result<String, FormatError> {
    if f.precision.is_some_and(|p| p > i32::MAX as usize) {
        return Err(Lookup);
    }
    let (mut kind, mut val, mut pct) = (f.kind, x, false);
    if kind == '%' {
        kind = 'f';
        val *= 100.0;
        pct = true;
    }
    let mut s = double_to_string(val, kind, f.precision.unwrap_or(6), f.alternate);
    if f.no_neg_0 && s.starts_with('-') && s[1..].chars().all(|c| matches!(c, '0' | '.' | 'e' | 'E' | '+' | '-')) {
        s.remove(0);
    }
    if pct {
        s.push('%');
    }
    let negative = s.starts_with('-');
    let body = if negative { &s[1..] } else { &s[..] };
    let int_end = body.find(|c: char| !c.is_ascii_digit()).unwrap_or(body.len());
    let (digits, rest) = body.split_at(int_end);
    let (decimal, remainder) = match rest.strip_prefix('.') {
        Some(r) => (true, r),
        None => (false, rest),
    };
    Ok(layout(f, negative, "", digits, remainder, decimal))
}

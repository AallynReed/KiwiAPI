//! `await request.form()` as Starlette 1.x runs it over python-multipart
//! 0.0.31: the `Content-Type` options parser, the multipart state machine
//! (fed the whole body as one chunk), the urlencoded parser, and the
//! `FormData` items they produce. Header and field bytes are Latin-1 text.

use crate::pystr::{is_space as py_isspace, strip as py_strip};

pub fn latin1(b: &[u8]) -> String {
    b.iter().map(|&c| c as char).collect()
}

fn to_latin1(s: &str) -> Vec<u8> {
    s.chars().map(|c| c as u32 as u8).collect()
}

/// Python slice bounds (`str.count`/`str.find` start and end) over `len`.
fn py_bounds(len: usize, start: isize, end: isize) -> (usize, usize) {
    let norm = |x: isize| if x < 0 { (x + len as isize).max(0) as usize } else { (x as usize).min(len) };
    (norm(start), norm(end))
}

fn py_find(s: &[char], c: char, start: isize, end: isize) -> isize {
    let (a, b) = py_bounds(s.len(), start, end);
    (a..b).find(|&i| s[i] == c).map_or(-1, |i| i as isize)
}

fn py_count(s: &[char], pat: &[char], start: isize, end: isize) -> isize {
    let (mut i, b) = py_bounds(s.len(), start, end);
    let mut n = 0;
    while i + pat.len() <= b {
        if s[i..i + pat.len()] == *pat {
            n += 1;
            i += pat.len();
        } else {
            i += 1;
        }
    }
    n
}

/// `email.message._parseparam` as vendored by python-multipart.
fn parseparam(s: &[char]) -> Vec<String> {
    let s: Vec<char> = std::iter::once(';').chain(s.iter().copied()).collect();
    let len = s.len() as isize;
    let mut out = Vec::new();
    let mut start: isize = 0;
    while py_find(&s, ';', start, len) == start {
        start += 1;
        let mut end = py_find(&s, ';', start, len);
        let (mut ind, mut diff) = (start, 0);
        while end > 0 {
            diff += py_count(&s, &['"'], ind, end) - py_count(&s, &['\\', '"'], ind, end);
            if diff % 2 == 0 {
                break;
            }
            (end, ind) = (ind, py_find(&s, ';', end + 1, len));
        }
        if end < 0 {
            end = len;
        }
        let (a, b) = (start as usize, end as usize);
        let i = py_find(&s, '=', start, end);
        let f: String = if i == -1 {
            s[a..b].iter().collect()
        } else {
            let key: String = s[a..i as usize].iter().collect();
            let val: String = s[i as usize + 1..b].iter().collect();
            format!("{}={}", key.trim_end_matches(py_isspace).to_lowercase(), val.trim_start_matches(py_isspace))
        };
        out.push(py_strip(&f).to_owned());
        start = end;
    }
    out
}

/// Header parameters or part headers: (name, value) bytes in order.
pub type Pairs = Vec<(Vec<u8>, Vec<u8>)>;

/// `parse_options_header`: the value (lowercased only without options) and
/// the options in header order (a later duplicate wins on lookup).
pub fn parse_options_header(value: Option<&[u8]>) -> (Vec<u8>, Pairs) {
    let Some(value) = value.filter(|v| !v.is_empty()) else { return (Vec::new(), Vec::new()) };
    let text = latin1(value);
    if !text.contains(';') {
        return (to_latin1(py_strip(&text.to_lowercase())), Vec::new());
    }
    let chars: Vec<char> = text.chars().collect();
    let mut parts = parseparam(&chars).into_iter();
    let ctype = parts.next().unwrap_or_default();
    let mut options = Vec::new();
    for seg in parts {
        let (key, val) = seg.split_once('=').unwrap_or((&seg, ""));
        if key.contains('*') {
            continue;
        }
        let mut val = val.to_owned();
        if val.chars().count() >= 2 && val.starts_with('"') && val.ends_with('"') {
            val = val[1..val.len() - 1].replace("\\\\", "\\").replace("\\\"", "\"");
        }
        if key == "filename" {
            let c: Vec<char> = val.chars().collect();
            if c.get(1..3) == Some(&[':', '\\']) || c.get(..2) == Some(&['\\', '\\']) {
                val = val.rsplit('\\').next().unwrap_or("").to_owned();
            }
        }
        options.push((to_latin1(key), to_latin1(&val)));
    }
    (to_latin1(&ctype), options)
}

pub fn option<'a>(options: &'a [(Vec<u8>, Vec<u8>)], key: &[u8]) -> Option<&'a [u8]> {
    options.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v.as_slice())
}

/// A file part: Starlette's `UploadFile`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UploadFile {
    pub filename: String,
    /// Part headers, names lowercased, in order.
    pub headers: Pairs,
    pub data: Vec<u8>,
}

impl UploadFile {
    /// `UploadFile.content_type`: the first `content-type` header.
    pub fn content_type(&self) -> Option<String> {
        self.headers.iter().find(|(k, _)| k == b"content-type").map(|(_, v)| latin1(v))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FormValue {
    Text(String),
    File(UploadFile),
}

/// Why `request.form()` failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FormError {
    /// Starlette's `MultiPartException`, raised as `HTTPException(400, message)`.
    Http(String),
    /// Any other parser error: FastAPI's 400 "There was an error parsing the body".
    Parse,
}

/// `_user_safe_decode`: the UTF-8 codec names, else Latin-1 (what ASCII and
/// Latin-1 decode to, and the fallback for an unknown codec or bad bytes).
fn decode(b: &[u8], charset: &str) -> String {
    let mut norm = String::new();
    let mut punct = false;
    for c in charset.to_ascii_lowercase().chars() {
        if c.is_ascii_alphanumeric() || c == '.' {
            if punct && !norm.is_empty() {
                norm.push('_');
            }
            norm.push(c);
            punct = false;
        } else {
            punct = true;
        }
    }
    match norm.as_str() {
        "utf_8" | "utf8" | "u8" | "utf" | "utf8_ucs2" | "utf8_ucs4" | "cp65001" => {
            std::str::from_utf8(b).map(str::to_owned).unwrap_or_else(|_| latin1(b))
        }
        _ => latin1(b),
    }
}

const MAX_HEADER_COUNT: usize = 8;
const MAX_HEADER_SIZE: usize = 4096 + 128;
const MAX_BOUNDARY: usize = 256;
const MAX_PART: usize = 1024 * 1024;
const MAX_FILES: usize = 1000;
const MAX_FIELDS: usize = 1000;

fn is_token(c: u8) -> bool {
    c.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&c)
}

#[derive(Default)]
struct Part {
    disposition: Option<Vec<u8>>,
    name: String,
    data: Vec<u8>,
    file: Option<UploadFile>,
    headers: Pairs,
}

/// Starlette's `MultiPartParser` callbacks.
struct Builder<'a> {
    charset: &'a str,
    items: Vec<(String, FormValue)>,
    part: Part,
    header_name: Vec<u8>,
    header_value: Vec<u8>,
    files: usize,
    fields: usize,
}

impl Builder<'_> {
    fn part_begin(&mut self) {
        self.part = Part::default();
    }

    fn part_data(&mut self, d: &[u8]) -> Result<(), FormError> {
        if d.is_empty() {
            return Ok(());
        }
        match &mut self.part.file {
            None => {
                if self.part.data.len() + d.len() > MAX_PART {
                    return Err(FormError::Http(format!("Part exceeded maximum size of {}KB.", MAX_PART / 1024)));
                }
                self.part.data.extend_from_slice(d);
            }
            Some(f) => f.data.extend_from_slice(d),
        }
        Ok(())
    }

    fn part_end(&mut self) {
        let part = std::mem::take(&mut self.part);
        let value = match part.file {
            None => FormValue::Text(decode(&part.data, self.charset)),
            Some(f) => FormValue::File(f),
        };
        self.items.push((part.name, value));
    }

    fn header_end(&mut self) {
        let name = self.header_name.to_ascii_lowercase();
        let value = std::mem::take(&mut self.header_value);
        if name == b"content-disposition" {
            self.part.disposition = Some(value.clone());
        }
        self.part.headers.push((name, value));
        self.header_name.clear();
    }

    fn headers_finished(&mut self) -> Result<(), FormError> {
        let (_, options) = parse_options_header(self.part.disposition.as_deref());
        let name = option(&options, b"name")
            .ok_or_else(|| FormError::Http("The Content-Disposition header field \"name\" must be provided.".into()))?;
        self.part.name = decode(name, self.charset);
        if let Some(filename) = option(&options, b"filename") {
            self.files += 1;
            if self.files > MAX_FILES {
                return Err(FormError::Http(format!("Too many files. Maximum number of files is {MAX_FILES}.")));
            }
            let filename = decode(filename, self.charset);
            self.part.file = Some(UploadFile { filename, headers: self.part.headers.clone(), data: Vec::new() });
        } else {
            self.fields += 1;
            if self.fields > MAX_FIELDS {
                return Err(FormError::Http(format!("Too many fields. Maximum number of fields is {MAX_FIELDS}.")));
            }
            self.part.file = None;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum St {
    Start,
    StartBoundary,
    HeaderFieldStart,
    HeaderField,
    HeaderValueStart,
    HeaderValue,
    HeaderValueAlmostDone,
    HeadersAlmostDone,
    PartDataStart,
    PartData,
    EndBoundary,
    End,
}

const FLAG_PART: u8 = 1;
const FLAG_LAST: u8 = 2;

/// `MultipartParser.write(body)` (one chunk) driving Starlette's callbacks.
fn run_multipart(data: &[u8], boundary_opt: &[u8], b: &mut Builder<'_>) -> Result<(), FormError> {
    let parse = || FormError::Parse;
    let mut boundary = b"\r\n--".to_vec();
    boundary.extend_from_slice(boundary_opt);
    let blen = boundary.len();
    let length = data.len();
    let mut state = St::Start;
    let (mut index, mut flags) = (0usize, 0u8);
    let (mut header_count, mut header_size) = (0usize, 0usize);
    let mut mark_field: Option<usize> = None;
    let mut mark_value: Option<usize> = None;
    let mut mark_data: Option<usize> = None;
    let mut i: isize = 0;
    let at = |i: isize| data[i as usize];
    while (i as usize) < length {
        let mut c = at(i);
        match state {
            St::Start => {
                if c == b'\r' || c == b'\n' {
                    match data[i as usize..].iter().position(|&x| x == b'-') {
                        Some(p) => {
                            i += p as isize;
                            continue;
                        }
                        None => break,
                    }
                }
                index = 0;
                state = St::StartBoundary;
                i -= 1;
            }
            St::StartBoundary => {
                if index == blen - 2 {
                    if c == b'-' {
                        state = St::EndBoundary;
                    } else if c != b'\r' {
                        return Err(parse());
                    }
                    index += 1;
                } else if index == blen - 1 {
                    if c != b'\n' {
                        return Err(parse());
                    }
                    index = 0;
                    b.part_begin();
                    header_count = 0;
                    header_size = 0;
                    state = St::HeaderFieldStart;
                } else {
                    if c != boundary[index + 2] {
                        return Err(parse());
                    }
                    index += 1;
                }
            }
            St::HeaderFieldStart => {
                index = 0;
                if c != b'\r' {
                    header_count += 1;
                    if header_count > MAX_HEADER_COUNT {
                        return Err(parse());
                    }
                    header_size = 0;
                }
                mark_field = Some(i as usize);
                state = St::HeaderField;
                i -= 1;
            }
            St::HeaderField => {
                if c == b'\r' && index == 0 {
                    mark_field = None;
                    state = St::HeadersAlmostDone;
                    i += 1;
                    continue;
                }
                let iu = i as usize;
                let colon = data[iu..].iter().position(|&x| x == b':').map(|p| iu + p);
                let end = colon.unwrap_or(length);
                header_size += if colon.is_none() { end - iu } else { end - iu + 1 };
                if header_size > MAX_HEADER_SIZE {
                    return Err(parse());
                }
                if !data[iu..end].iter().all(|&x| is_token(x)) {
                    return Err(parse());
                }
                index += end - iu;
                match colon {
                    None => i = length as isize,
                    Some(colon) => {
                        if index == 0 {
                            return Err(parse());
                        }
                        i = colon as isize;
                        if let Some(m) = mark_field.take()
                            && colon > m
                        {
                            b.header_name.extend_from_slice(&data[m..colon]);
                        }
                        state = St::HeaderValueStart;
                    }
                }
            }
            St::HeaderValueStart => {
                if c == b' ' {
                    header_size += 1;
                    if header_size > MAX_HEADER_SIZE {
                        return Err(parse());
                    }
                    i += 1;
                    continue;
                }
                mark_value = Some(i as usize);
                state = St::HeaderValue;
                i -= 1;
            }
            St::HeaderValue => {
                let iu = i as usize;
                let cr = data[iu..].iter().position(|&x| x == b'\r').map(|p| iu + p);
                let end = cr.unwrap_or(length);
                header_size += end - iu;
                if header_size > MAX_HEADER_SIZE {
                    return Err(parse());
                }
                match cr {
                    Some(cr) => {
                        i = cr as isize;
                        if let Some(m) = mark_value.take()
                            && cr > m
                        {
                            b.header_value.extend_from_slice(&data[m..cr]);
                        }
                        b.header_end();
                        header_size = 0;
                        state = St::HeaderValueAlmostDone;
                    }
                    None => i = length as isize,
                }
            }
            St::HeaderValueAlmostDone => {
                if c != b'\n' {
                    return Err(parse());
                }
                state = St::HeaderFieldStart;
            }
            St::HeadersAlmostDone => {
                if c != b'\n' {
                    return Err(parse());
                }
                b.headers_finished()?;
                state = St::PartDataStart;
            }
            St::PartDataStart => {
                mark_data = Some(i as usize);
                state = St::PartData;
                i -= 1;
            }
            St::PartData => {
                let mut prev_index = index;
                if index == 0 {
                    let iu = i as usize;
                    match find(&data[iu..length], &boundary) {
                        Some(p) => {
                            index = blen - 1;
                            i = (iu + p + blen - 1) as isize;
                        }
                        None => {
                            let start = iu.max(length.saturating_sub(blen));
                            let j = if start < length - 1 {
                                data[start..length - 1].iter().position(|&x| x == boundary[0]).map(|p| start + p)
                            } else {
                                None
                            };
                            i = j.unwrap_or(length - 1) as isize;
                        }
                    }
                    c = at(i);
                }
                if index < blen {
                    if boundary[index] == c {
                        index += 1;
                    } else {
                        index = 0;
                    }
                } else if index == blen {
                    index += 1;
                    if c == b'\r' {
                        flags |= FLAG_PART;
                    } else if c == b'-' {
                        flags |= FLAG_LAST;
                    } else {
                        index = 0;
                    }
                } else if index == blen + 1 {
                    if flags & FLAG_PART != 0 {
                        if c == b'\n' {
                            flags &= !FLAG_PART;
                            emit_data(data, &mut mark_data, i - index as isize, b)?;
                            b.part_end();
                            b.part_begin();
                            header_count = 0;
                            header_size = 0;
                            index = 0;
                            state = St::HeaderFieldStart;
                            i += 1;
                            continue;
                        }
                        index = 0;
                        flags &= !FLAG_PART;
                    } else if flags & FLAG_LAST != 0 {
                        if c == b'-' {
                            emit_data(data, &mut mark_data, i - index as isize, b)?;
                            b.part_end();
                            state = St::End;
                        } else {
                            index = 0;
                        }
                    }
                }
                if index == 0 && prev_index > 0 {
                    prev_index = 0;
                    let _ = prev_index;
                    i -= 1;
                }
            }
            St::EndBoundary => {
                if index == blen - 1 {
                    if c != b'-' {
                        return Err(parse());
                    }
                    index += 1;
                    state = St::End;
                }
            }
            St::End => break,
        }
        i += 1;
    }
    if let Some(m) = mark_field
        && length > m
    {
        b.header_name.extend_from_slice(&data[m..length]);
    }
    if let Some(m) = mark_value
        && length > m
    {
        b.header_value.extend_from_slice(&data[m..length]);
    }
    emit_data(data, &mut mark_data, length as isize - index as isize, b)?;
    Ok(())
}

fn emit_data(data: &[u8], mark: &mut Option<usize>, end: isize, b: &mut Builder<'_>) -> Result<(), FormError> {
    if let Some(m) = mark.take()
        && end > m as isize
    {
        b.part_data(&data[m..end as usize])?;
    }
    Ok(())
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.len() > hay.len() {
        return None;
    }
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Python `urllib.parse.unquote_plus` of a Latin-1 decoded string: `%XX`
/// runs inside ASCII stretches decode as UTF-8 with replacement.
pub fn unquote_plus(s: &str) -> String {
    let s = s.replace('+', " ");
    if !s.contains('%') {
        return s;
    }
    let mut out = String::with_capacity(s.len());
    let mut run: Vec<u8> = Vec::new();
    let flush = |run: &mut Vec<u8>, out: &mut String| {
        if !run.is_empty() {
            out.push_str(&String::from_utf8_lossy(&unquote_bytes(run)));
            run.clear();
        }
    };
    for c in s.chars() {
        if c.is_ascii() {
            run.push(c as u8);
        } else {
            flush(&mut run, &mut out);
            out.push(c);
        }
    }
    flush(&mut run, &mut out);
    out
}

fn unquote_bytes(b: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(b.len());
    let hex = |c: u8| (c as char).to_digit(16).map(|d| d as u8);
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

/// Starlette's `FormParser` over python-multipart's `QuerystringParser`.
fn parse_urlencoded(body: &[u8]) -> Vec<(String, FormValue)> {
    body.split(|&c| c == b'&')
        .filter(|f| !f.is_empty())
        .map(|f| {
            let (name, value) = match f.iter().position(|&c| c == b'=') {
                Some(p) => (&f[..p], &f[p + 1..]),
                None => (f, &[][..]),
            };
            (unquote_plus(&latin1(name)), FormValue::Text(unquote_plus(&latin1(value))))
        })
        .collect()
}

/// `request.form()` for a body with this `Content-Type` header.
pub fn parse_form(content_type: Option<&[u8]>, body: &[u8]) -> Result<Vec<(String, FormValue)>, FormError> {
    let (ctype, options) = parse_options_header(content_type);
    if ctype == b"multipart/form-data" {
        let charset = option(&options, b"charset").map(latin1).unwrap_or_else(|| "utf-8".into());
        let boundary =
            option(&options, b"boundary").ok_or_else(|| FormError::Http("Missing boundary in multipart.".into()))?;
        if boundary.len() > MAX_BOUNDARY {
            return Err(FormError::Parse);
        }
        let mut b = Builder {
            charset: &charset,
            items: Vec::new(),
            part: Part::default(),
            header_name: Vec::new(),
            header_value: Vec::new(),
            files: 0,
            fields: 0,
        };
        run_multipart(body, boundary, &mut b)?;
        Ok(b.items)
    } else if ctype == b"application/x-www-form-urlencoded" {
        Ok(parse_urlencoded(body))
    } else {
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(items: &[(String, FormValue)], name: &str) -> Option<String> {
        items.iter().rev().find(|(k, _)| k == name).and_then(|(_, v)| match v {
            FormValue::Text(t) => Some(t.clone()),
            FormValue::File(_) => None,
        })
    }

    #[test]
    fn options_header() {
        let (ct, opts) = parse_options_header(Some(b"multipart/form-data; boundary=\"a;b\"; charset=UTF-8"));
        assert_eq!(ct, b"multipart/form-data");
        assert_eq!(option(&opts, b"boundary"), Some(&b"a;b"[..]));
        let (ct, _) = parse_options_header(Some(b" Multipart/Form-Data "));
        assert_eq!(ct, b"multipart/form-data");
        let (ct, _) = parse_options_header(Some(b"Multipart/Form-Data; boundary=x"));
        assert_eq!(ct, b"Multipart/Form-Data");
        let (_, opts) = parse_options_header(Some(b"form-data; name=\"f\"; filename=\"C:\\\\x\\\\y.png\""));
        assert_eq!(option(&opts, b"filename"), Some(&b"y.png"[..]));
    }

    #[test]
    fn multipart_parts() {
        let body = b"\r\n--XyZ\r\nContent-Disposition: form-data; name=\"message\"\r\n\r\nhello\r\n--XyZ\r\n\
            Content-Disposition: form-data; name=\"attachments\"; filename=\"a.png\"\r\nContent-Type: image/png\r\n\r\n\
            \x89PNG\r\n--XyZ--\r\nepilogue";
        let items = parse_form(Some(b"multipart/form-data; boundary=XyZ"), body).unwrap();
        assert_eq!(text(&items, "message").as_deref(), Some("hello"));
        let FormValue::File(f) = &items[1].1 else { panic!() };
        assert_eq!(
            (f.filename.as_str(), f.content_type().as_deref(), f.data.as_slice()),
            ("a.png", Some("image/png"), &b"\x89PNG"[..])
        );
    }

    #[test]
    fn multipart_errors() {
        assert_eq!(
            parse_form(Some(b"multipart/form-data"), b"x"),
            Err(FormError::Http("Missing boundary in multipart.".into()))
        );
        assert_eq!(parse_form(Some(b"multipart/form-data; boundary=XyZ"), b"garbage"), Err(FormError::Parse));
        let unterminated = b"--XyZ\r\nContent-Disposition: form-data; name=\"message\"\r\n\r\nhello world";
        assert_eq!(parse_form(Some(b"multipart/form-data; boundary=XyZ"), unterminated), Ok(Vec::new()));
    }

    #[test]
    fn urlencoded() {
        let items = parse_form(Some(b"application/x-www-form-urlencoded"), b"a=1+2&&b&c=%E2%9C%93%zz&=x\xe9").unwrap();
        let got: Vec<(String, String)> = items
            .into_iter()
            .map(|(k, v)| match v {
                FormValue::Text(t) => (k, t),
                FormValue::File(_) => unreachable!(),
            })
            .collect();
        assert_eq!(
            got,
            [
                ("a".into(), "1 2".into()),
                ("b".into(), String::new()),
                ("c".into(), "✓%zz".into()),
                (String::new(), "xé".into())
            ]
        );
    }
}

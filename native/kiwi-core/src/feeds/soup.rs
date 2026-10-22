//! `BeautifulSoup(markup, "html.parser")`: CPython's `html.parser` tokenizer
//! (the 3.13+/3.14 version production runs, fed once and then closed) driving
//! bs4 4.12's tree builder, plus the few soupsieve selectors the Bilibili
//! scrape uses. Offsets are bytes; every byte the grammar tests is ASCII, and
//! the few length checks Python makes in characters count characters.

use std::collections::HashMap;

use super::py;
use super::unescape::{bs4_entity, unescape_attr};

/// The `NavigableString` subclass a text node was created as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StrKind {
    NavigableString,
    Comment,
    Doctype,
    Declaration,
    CData,
    ProcessingInstruction,
    Script,
    Stylesheet,
    TemplateString,
    RubyTextString,
    RubyParenthesisString,
}

impl StrKind {
    pub fn name(self) -> &'static str {
        match self {
            StrKind::NavigableString => "NavigableString",
            StrKind::Comment => "Comment",
            StrKind::Doctype => "Doctype",
            StrKind::Declaration => "Declaration",
            StrKind::CData => "CData",
            StrKind::ProcessingInstruction => "ProcessingInstruction",
            StrKind::Script => "Script",
            StrKind::Stylesheet => "Stylesheet",
            StrKind::TemplateString => "TemplateString",
            StrKind::RubyTextString => "RubyTextString",
            StrKind::RubyParenthesisString => "RubyParenthesisString",
        }
    }

    /// `HTMLTreeBuilder.DEFAULT_STRING_CONTAINERS`.
    fn container(tag: &str) -> Option<StrKind> {
        Some(match tag {
            "rt" => StrKind::RubyTextString,
            "rp" => StrKind::RubyParenthesisString,
            "style" => StrKind::Stylesheet,
            "script" => StrKind::Script,
            "template" => StrKind::TemplateString,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AttrValue {
    Str(String),
    /// A multi-valued attribute (`class`, `rel`, ...) split on whitespace.
    List(Vec<String>),
}

#[derive(Clone, Debug)]
pub enum Node {
    Tag(usize),
    Text(StrKind, String),
}

#[derive(Clone, Debug)]
pub struct Tag {
    pub name: String,
    pub attrs: Vec<(String, AttrValue)>,
    pub children: Vec<Node>,
    pub parent: Option<usize>,
}

impl Tag {
    pub fn get(&self, name: &str) -> Option<&AttrValue> {
        self.attrs.iter().find(|(k, _)| k == name).map(|(_, v)| v)
    }

    /// `tag.get(name, "")` for a single-valued attribute.
    pub fn get_str(&self, name: &str) -> String {
        match self.get(name) {
            Some(AttrValue::Str(s)) => s.clone(),
            Some(AttrValue::List(v)) => v.join(" "),
            None => String::new(),
        }
    }

    pub fn has_class(&self, class: &str) -> bool {
        match self.get("class") {
            Some(AttrValue::List(v)) => v.iter().any(|c| c == class),
            Some(AttrValue::Str(s)) => s.split(py::is_space).any(|c| c == class),
            None => false,
        }
    }
}

/// BeautifulSoup gave up on the markup (a ValueError or rejected markup).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError(pub String);

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ParseError {}

/// The parsed document; `tags[ROOT]` is the `[document]` object.
#[derive(Clone, Debug)]
pub struct Soup {
    pub tags: Vec<Tag>,
}

pub const ROOT: usize = 0;

const EMPTY_ELEMENTS: [&str; 24] = [
    "area", "base", "br", "col", "embed", "hr", "img", "input", "keygen", "link", "menuitem", "meta", "param",
    "source", "track", "wbr", "basefont", "bgsound", "command", "frame", "image", "isindex", "nextid", "spacer",
];

fn is_list_attr(tag: &str, attr: &str) -> bool {
    matches!(attr, "class" | "accesskey" | "dropzone")
        || matches!(
            (tag, attr),
            ("a" | "link", "rel" | "rev")
                | ("td" | "th", "headers")
                | ("form", "accept-charset")
                | ("object", "archive")
                | ("area", "rel")
                | ("icon", "sizes")
                | ("iframe", "sandbox")
                | ("output", "for")
        )
}

/// `windows-1252` for the bytes 0x80..=0x9f (`None` where the codec has no char).
const CP1252_HIGH: [Option<char>; 32] = [
    Some('\u{20ac}'),
    None,
    Some('\u{201a}'),
    Some('\u{0192}'),
    Some('\u{201e}'),
    Some('\u{2026}'),
    Some('\u{2020}'),
    Some('\u{2021}'),
    Some('\u{02c6}'),
    Some('\u{2030}'),
    Some('\u{0160}'),
    Some('\u{2039}'),
    Some('\u{0152}'),
    None,
    Some('\u{017d}'),
    None,
    None,
    Some('\u{2018}'),
    Some('\u{2019}'),
    Some('\u{201c}'),
    Some('\u{201d}'),
    Some('\u{2022}'),
    Some('\u{2013}'),
    Some('\u{2014}'),
    Some('\u{02dc}'),
    Some('\u{2122}'),
    Some('\u{0161}'),
    Some('\u{203a}'),
    Some('\u{0153}'),
    None,
    Some('\u{017e}'),
    Some('\u{0178}'),
];

fn ws5(c: u8) -> bool {
    matches!(c, b'\t' | b'\n' | b'\r' | 0x0c | b' ')
}

/// Builder state of `BeautifulSoup` + `BeautifulSoupHTMLParser`.
struct Builder {
    tags: Vec<Tag>,
    stack: Vec<usize>,
    open: HashMap<String, usize>,
    preserve_ws: Vec<usize>,
    containers: Vec<usize>,
    data: Option<String>,
    already_closed: Vec<String>,
}

impl Builder {
    fn new() -> Self {
        let root = Tag { name: "[document]".into(), attrs: Vec::new(), children: Vec::new(), parent: None };
        Builder {
            tags: vec![root],
            stack: vec![ROOT],
            open: HashMap::new(),
            preserve_ws: Vec::new(),
            containers: Vec::new(),
            data: None,
            already_closed: Vec::new(),
        }
    }

    fn current(&self) -> usize {
        *self.stack.last().unwrap_or(&ROOT)
    }

    fn handle_data(&mut self, s: &str) {
        self.data.get_or_insert_with(String::new).push_str(s);
    }

    fn end_data(&mut self, kind: Option<StrKind>) {
        let Some(mut data) = self.data.take() else { return };
        if self.preserve_ws.is_empty() && data.bytes().all(ws5) {
            data = if data.contains('\n') { "\n".into() } else { " ".into() };
        }
        let kind = match kind {
            Some(k) if k != StrKind::NavigableString => k,
            _ => self
                .containers
                .last()
                .and_then(|&t| StrKind::container(&self.tags[t].name))
                .unwrap_or(StrKind::NavigableString),
        };
        let cur = self.current();
        self.tags[cur].children.push(Node::Text(kind, data));
    }

    fn push_tag(&mut self, id: usize) {
        let cur = self.current();
        self.tags[cur].children.push(Node::Tag(id));
        self.stack.push(id);
        let name = self.tags[id].name.clone();
        if matches!(name.as_str(), "pre" | "textarea") {
            self.preserve_ws.push(id);
        }
        if StrKind::container(&name).is_some() {
            self.containers.push(id);
        }
        *self.open.entry(name).or_default() += 1;
    }

    fn pop_tag(&mut self) {
        let Some(id) = self.stack.pop() else { return };
        if let Some(n) = self.open.get_mut(&self.tags[id].name) {
            *n = n.saturating_sub(1);
        }
        if self.preserve_ws.last() == Some(&id) {
            self.preserve_ws.pop();
        }
        if self.containers.last() == Some(&id) {
            self.containers.pop();
        }
    }

    fn pop_to_tag(&mut self, name: &str) {
        for i in (1..self.stack.len()).rev() {
            if self.open.get(name).copied().unwrap_or(0) == 0 {
                break;
            }
            let hit = self.tags[self.stack[i]].name == name;
            self.pop_tag();
            if hit {
                break;
            }
        }
    }

    fn handle_starttag(&mut self, name: String, raw_attrs: Vec<(String, Option<String>)>, empty_element: bool) {
        let mut attrs: Vec<(String, AttrValue)> = Vec::new();
        for (k, v) in raw_attrs {
            let v = v.unwrap_or_default();
            match attrs.iter_mut().find(|(ek, _)| *ek == k) {
                Some(slot) => slot.1 = AttrValue::Str(v),
                None => attrs.push((k, AttrValue::Str(v))),
            }
        }
        for (k, v) in attrs.iter_mut() {
            if is_list_attr(&name, k)
                && let AttrValue::Str(s) = v
            {
                *v = AttrValue::List(s.split(py::is_space).filter(|p| !p.is_empty()).map(str::to_owned).collect());
            }
        }
        self.end_data(None);
        let id = self.tags.len();
        let void = EMPTY_ELEMENTS.contains(&name.as_str());
        self.tags.push(Tag { name: name.clone(), attrs, children: Vec::new(), parent: Some(self.current()) });
        self.push_tag(id);
        if void && empty_element {
            self.handle_endtag(&name, false);
            self.already_closed.push(name);
        }
    }

    fn handle_endtag(&mut self, name: &str, check_already_closed: bool) {
        if check_already_closed && let Some(i) = self.already_closed.iter().position(|n| n == name) {
            self.already_closed.remove(i);
            return;
        }
        self.end_data(None);
        self.pop_to_tag(name);
    }

    fn handle_startendtag(&mut self, name: String, attrs: Vec<(String, Option<String>)>) {
        self.handle_starttag(name.clone(), attrs, false);
        self.handle_endtag(&name, true);
    }

    fn special(&mut self, data: &str, kind: StrKind) {
        self.end_data(None);
        self.handle_data(data);
        self.end_data(Some(kind));
    }

    fn handle_comment(&mut self, data: &str) {
        self.special(data, StrKind::Comment);
    }

    fn handle_decl(&mut self, data: &str) {
        self.special(&data[py::char_offset(data, 8)..], StrKind::Doctype);
    }

    fn unknown_decl(&mut self, data: &str) {
        if data.len() >= 6 && data[..6].eq_ignore_ascii_case("CDATA[") {
            self.special(&data[6..], StrKind::CData);
        } else {
            self.special(data, StrKind::Declaration);
        }
    }

    fn handle_pi(&mut self, data: &str) {
        self.special(data, StrKind::ProcessingInstruction);
    }

    fn handle_charref(&mut self, name: &str) -> Result<(), ParseError> {
        let real = match name.as_bytes().first() {
            Some(b'x') => py::int(name.trim_start_matches('x'), 16),
            Some(b'X') => py::int(name.trim_start_matches('X'), 16),
            _ => py::int(name, 10),
        }
        .ok_or_else(|| ParseError(format!("invalid character reference {name:?}")))?;
        if real < 0 {
            return Err(ParseError(format!("invalid character reference {name:?}")));
        }
        let ch = match real {
            0x80..=0x9f => CP1252_HIGH[(real - 0x80) as usize],
            _ => None,
        }
        .or_else(|| u32::try_from(real).ok().and_then(char::from_u32))
        .unwrap_or('\u{fffd}');
        self.handle_data(ch.encode_utf8(&mut [0; 4]));
        Ok(())
    }

    fn handle_entityref(&mut self, name: &str) {
        match bs4_entity(name) {
            Some(v) => self.handle_data(v),
            None => {
                self.handle_data("&");
                self.handle_data(name);
            }
        }
    }
}

struct Parser<'a> {
    raw: &'a str,
    b: &'a [u8],
    pos: usize,
    cdata_elem: Option<String>,
    escapable: bool,
    out: Builder,
}

/// `[a-zA-Z][^\t\n\r\f />]*`: end of a tag name starting at `p`.
fn name_end(b: &[u8], p: usize) -> usize {
    let mut j = p + 1;
    while j < b.len() && !ws5(b[j]) && b[j] != b'/' && b[j] != b'>' {
        j += 1;
    }
    j
}

/// The optional `\s*=\s*(value)` of an attribute at `q`: `(value range, end)`.
fn value_group(b: &[u8], q: usize) -> Option<((usize, usize), usize)> {
    let n = b.len();
    let mut r = q;
    while r < n && ws5(b[r]) {
        r += 1;
    }
    if b.get(r) != Some(&b'=') {
        return None;
    }
    let after_eq = r + 1;
    let mut v = after_eq;
    while v < n && ws5(b[v]) {
        v += 1;
    }
    loop {
        let end = match b.get(v) {
            Some(&q @ (b'\'' | b'"')) => b[v + 1..].iter().position(|&c| c == q).map(|p| v + 1 + p + 1),
            _ => {
                let mut e = v;
                while e < n && b[e] != b'>' && !ws5(b[e]) {
                    e += 1;
                }
                Some(e)
            }
        };
        if let Some(end) = end {
            return Some(((v, end), end));
        }
        if v == after_eq {
            return None;
        }
        v -= 1;
    }
}

/// One attribute match (`(?<=['"\s/])name(=value)?`).
struct Attr {
    name_end: usize,
    value: Option<(usize, usize)>,
    end: usize,
}

fn attribute(b: &[u8], k: usize) -> Option<Attr> {
    let n = b.len();
    if k == 0 || k >= n || !(matches!(b[k - 1], b'\'' | b'"' | b'/') || ws5(b[k - 1])) {
        return None;
    }
    if ws5(b[k]) || b[k] == b'/' || b[k] == b'>' {
        return None;
    }
    let mut q = k + 1;
    while q < n && !ws5(b[q]) && !matches!(b[q], b'/' | b'=' | b'>') {
        q += 1;
    }
    Some(match value_group(b, q) {
        Some((value, end)) => Attr { name_end: q, value: Some(value), end },
        None => Attr { name_end: q, value: None, end: q },
    })
}

/// `locatetagend.match(rawdata, p).end()`.
fn locatetagend(b: &[u8], p: usize) -> usize {
    let n = b.len();
    let mut j = name_end(b, p);
    while j < n && (ws5(b[j]) || b[j] == b'/') {
        j += 1;
    }
    while let Some(attr) = attribute(b, j) {
        j = attr.end;
        while j < n && (ws5(b[j]) || b[j] == b'/') {
            j += 1;
        }
    }
    if b.get(j) == Some(&b'>') {
        j += 1;
    }
    j
}

/// `(?:[\t\n\r\f ]|/(?!>))*` from `j`.
fn skip_ws_slash(b: &[u8], mut j: usize) -> usize {
    while j < b.len() && (ws5(b[j]) || (b[j] == b'/' && b.get(j + 1) != Some(&b'>'))) {
        j += 1;
    }
    j
}

impl<'a> Parser<'a> {
    fn starts(&self, i: usize, s: &str) -> bool {
        self.b[i..].starts_with(s.as_bytes())
    }

    fn letter_at(&self, i: usize) -> bool {
        self.b.get(i).is_some_and(u8::is_ascii_alphabetic)
    }

    /// Python's `i + k < n` with `n` counted in characters.
    fn more_than(&self, i: usize, k: usize) -> bool {
        self.raw[i..].chars().nth(k).is_some()
    }

    fn find(&self, from: usize, needle: &str) -> Option<usize> {
        self.raw.get(from..)?.find(needle).map(|p| from + p)
    }

    /// `self.interesting.search(rawdata, i)`.
    fn interesting(&self, i: usize) -> Option<usize> {
        let Some(elem) = &self.cdata_elem else {
            return self.b[i..].iter().position(|&c| c == b'&' || c == b'<').map(|p| i + p);
        };
        if elem == "plaintext" {
            return Some(self.b.len());
        }
        let e = elem.as_bytes();
        (i..self.b.len()).find(|&p| {
            let c = self.b[p];
            if c == b'&' && self.escapable {
                return true;
            }
            c == b'<'
                && self.b.get(p + 1) == Some(&b'/')
                && self.b.len() > p + 2 + e.len()
                && self.b[p + 2..p + 2 + e.len()].eq_ignore_ascii_case(e)
                && {
                    let next = self.b[p + 2 + e.len()];
                    ws5(next) || next == b'/' || next == b'>'
                }
        })
    }

    fn goahead(&mut self, end: bool) -> Result<(), ParseError> {
        let n = self.b.len();
        let mut i = self.pos;
        while i < n {
            let j = match self.interesting(i) {
                Some(j) => j,
                None if self.cdata_elem.is_some() => break,
                None => n,
            };
            if i < j {
                self.out.handle_data(&self.raw[i..j]);
            }
            i = j;
            if i == n {
                break;
            }
            if self.b[i] == b'<' {
                let k = if self.letter_at(i + 1) {
                    self.parse_starttag(i)
                } else if self.starts(i, "</") {
                    self.parse_endtag(i)
                } else if self.starts(i, "<!--") {
                    self.parse_comment(i)
                } else if self.starts(i, "<?") {
                    self.parse_pi(i)
                } else if self.starts(i, "<!") {
                    self.parse_html_declaration(i)
                } else if self.more_than(i, 1) || end {
                    self.out.handle_data("<");
                    Some(i + 1)
                } else {
                    break;
                };
                i = match k {
                    Some(k) => k,
                    None if !end => break,
                    None => {
                        self.eof_construct(i);
                        n
                    }
                };
            } else if self.starts(i, "&#") {
                if let Some((name, k)) = self.charref(i) {
                    self.out.handle_charref(name)?;
                    i = k;
                    continue;
                }
                let incomplete = self.b.get(i + 2).is_some_and(u8::is_ascii_digit)
                    || (matches!(self.b.get(i + 2), Some(b'x' | b'X'))
                        && self.b.get(i + 3).is_some_and(u8::is_ascii_hexdigit));
                if incomplete {
                    if end {
                        self.out.handle_charref(&self.raw[i + 2..])?;
                        i = n;
                    }
                    break;
                } else if self.more_than(i, 3) {
                    self.out.handle_data("&#");
                    i += 2;
                } else {
                    break;
                }
            } else {
                if let Some((name, k)) = self.entityref(i) {
                    self.out.handle_entityref(name);
                    i = k;
                    continue;
                }
                if self.letter_at(i + 1) {
                    if end {
                        self.out.handle_entityref(&self.raw[i + 1..]);
                        i = n;
                    }
                    break;
                } else if self.more_than(i, 1) {
                    self.out.handle_data("&");
                    i += 1;
                } else {
                    break;
                }
            }
        }
        if end && i < n {
            self.out.handle_data(&self.raw[i..]);
            i = n;
        }
        self.pos = i;
        Ok(())
    }

    /// What `goahead(end=True)` does with a construct cut off by the end.
    fn eof_construct(&mut self, i: usize) {
        let raw = self.raw;
        if self.letter_at(i + 1) {
        } else if self.starts(i, "</") {
            if raw[i..].chars().count() == 2 {
                self.out.handle_data("</");
            } else if !self.letter_at(i + 2) {
                self.out.handle_comment(&raw[i + 2..]);
            }
        } else if self.starts(i, "<!--") {
            let body = &raw[i + 4..];
            let cut = ["--!", "--", "-"].iter().find(|s| body.ends_with(*s)).map_or(0, |s| s.len());
            self.out.handle_comment(&body[..body.len() - cut]);
        } else if self.starts(i, "<![CDATA[") {
            self.out.unknown_decl(&raw[i + 3..]);
        } else if self.b.len() >= i + 9 && self.b[i..i + 9].eq_ignore_ascii_case(b"<!doctype") {
            self.out.handle_decl(&raw[i + 2..]);
        } else if self.starts(i, "<!") {
            self.out.handle_comment(&raw[i + 2..]);
        } else if self.starts(i, "<?") {
            self.out.handle_pi(&raw[i + 2..]);
        }
    }

    /// `charref` (`&#(?:[0-9]+|[xX][0-9a-fA-F]+)[^0-9a-fA-F]`): name and where
    /// parsing resumes (after a `;` terminator, else at the terminator).
    fn charref(&self, i: usize) -> Option<(&'a str, usize)> {
        let b = self.b;
        let n = b.len();
        let mut d = i + 2;
        while d < n && b[d].is_ascii_digit() {
            d += 1;
        }
        let stop = if d > i + 2 {
            d
        } else if matches!(b.get(i + 2), Some(b'x' | b'X')) {
            let mut h = i + 3;
            while h < n && b[h].is_ascii_hexdigit() {
                h += 1;
            }
            if h == i + 3 {
                return None;
            }
            h
        } else {
            return None;
        };
        if stop >= n || b[stop].is_ascii_hexdigit() {
            return None;
        }
        Some((&self.raw[i + 2..stop], if b[stop] == b';' { stop + 1 } else { stop }))
    }

    /// `entityref` (`&([a-zA-Z][-.a-zA-Z0-9]*)[^a-zA-Z0-9]`), backtracking to a
    /// `-`/`.` inside the name when the run reaches the end of input.
    fn entityref(&self, i: usize) -> Option<(&'a str, usize)> {
        let b = self.b;
        if !self.letter_at(i + 1) {
            return None;
        }
        let name_ok = |c: u8| c.is_ascii_alphanumeric() || c == b'-' || c == b'.';
        let mut e = i + 2;
        while e < b.len() && name_ok(b[e]) {
            e += 1;
        }
        if e == b.len() {
            e = (i + 2..b.len()).rev().find(|&p| b[p] == b'-' || b[p] == b'.')?;
        }
        Some((&self.raw[i + 1..e], if b[e] == b';' { e + 1 } else { e }))
    }

    fn parse_starttag(&mut self, i: usize) -> Option<usize> {
        let b = self.b;
        let endpos = locatetagend(b, i + 1);
        if b[endpos - 1] != b'>' {
            return None;
        }
        let tname_end = name_end(b, i + 1);
        let tag = self.raw[i + 1..tname_end].to_lowercase();
        let mut k = skip_ws_slash(b, tname_end);
        let mut attrs = Vec::new();
        while k < endpos {
            let Some(attr) = attribute(b, k) else { break };
            let end = skip_ws_slash(b, attr.end);
            let name = self.raw[k..attr.name_end].to_lowercase();
            let value = attr.value.map(|(s, e)| {
                let v = &self.raw[s..e];
                let v =
                    if v.len() >= 2 && (v.starts_with('\'') || v.starts_with('"')) { &v[1..v.len() - 1] } else { v };
                unescape_attr(v).into_owned()
            });
            attrs.push((name, value));
            k = end;
        }
        let rest = py::strip(&self.raw[k..endpos]);
        if rest != ">" && rest != "/>" {
            self.out.handle_data(&self.raw[i..endpos]);
            return Some(endpos);
        }
        if rest == "/>" {
            self.out.handle_startendtag(tag, attrs);
        } else {
            self.out.handle_starttag(tag.clone(), attrs, true);
            const RAWTEXT: [&str; 7] = ["script", "style", "xmp", "iframe", "noembed", "noframes", "plaintext"];
            if RAWTEXT.contains(&tag.as_str()) {
                self.cdata_elem = Some(tag);
                self.escapable = false;
            } else if tag == "textarea" || tag == "title" {
                self.cdata_elem = Some(tag);
                self.escapable = true;
            }
        }
        Some(endpos)
    }

    fn parse_endtag(&mut self, i: usize) -> Option<usize> {
        let b = self.b;
        self.find(i + 2, ">")?;
        if !self.letter_at(i + 2) {
            if b.get(i + 2) == Some(&b'>') {
                return Some(i + 3);
            }
            return self.parse_bogus_comment(i);
        }
        let j = locatetagend(b, i + 2);
        if b[j - 1] != b'>' {
            return None;
        }
        let tag = self.raw[i + 2..name_end(b, i + 2)].to_lowercase();
        self.out.handle_endtag(&tag, true);
        self.cdata_elem = None;
        self.escapable = true;
        Some(j)
    }

    fn parse_comment(&mut self, i: usize) -> Option<usize> {
        let b = self.b;
        let close = (i + 4..b.len()).find(|&p| b[p..].starts_with(b"-->") || b[p..].starts_with(b"--!>"));
        let (text_end, end) = match close {
            Some(p) => (p, if b[p + 2] == b'!' { p + 4 } else { p + 3 }),
            None if b.get(i + 4) == Some(&b'>') => (i + 4, i + 5),
            None if b[i + 4..].starts_with(b"->") => (i + 4, i + 6),
            None => return None,
        };
        self.out.handle_comment(&self.raw[i + 4..text_end]);
        Some(end)
    }

    fn parse_bogus_comment(&mut self, i: usize) -> Option<usize> {
        let pos = self.find(i + 2, ">")?;
        self.out.handle_comment(&self.raw[i + 2..pos]);
        Some(pos + 1)
    }

    fn parse_pi(&mut self, i: usize) -> Option<usize> {
        let j = self.find(i + 2, ">")?;
        self.out.handle_pi(&self.raw[i + 2..j]);
        Some(j + 1)
    }

    fn parse_html_declaration(&mut self, i: usize) -> Option<usize> {
        if self.starts(i, "<!--") {
            return self.parse_comment(i);
        }
        if self.starts(i, "<![CDATA[") {
            let j = self.find(i + 9, "]]>")?;
            self.out.unknown_decl(&self.raw[i + 3..j]);
            return Some(j + 3);
        }
        if self.b.len() >= i + 9 && self.b[i..i + 9].eq_ignore_ascii_case(b"<!doctype") {
            let gt = self.find(i + 9, ">")?;
            self.out.handle_decl(&self.raw[i + 2..gt]);
            return Some(gt + 1);
        }
        self.parse_bogus_comment(i)
    }
}

impl Soup {
    /// `BeautifulSoup(html, "html.parser")`.
    pub fn parse(html: &str) -> Result<Soup, ParseError> {
        let mut p =
            Parser { raw: html, b: html.as_bytes(), pos: 0, cdata_elem: None, escapable: true, out: Builder::new() };
        p.goahead(false)?;
        p.goahead(true)?;
        let mut out = p.out;
        out.end_data(None);
        out.already_closed.clear();
        Ok(Soup { tags: out.tags })
    }

    pub fn tag(&self, id: usize) -> &Tag {
        &self.tags[id]
    }

    /// Tag ids under `id`, in document order.
    pub fn descendants(&self, id: usize) -> Vec<usize> {
        let mut out = Vec::new();
        let mut stack: Vec<usize> = Vec::new();
        let push_children = |stack: &mut Vec<usize>, t: usize| {
            for node in self.tags[t].children.iter().rev() {
                if let Node::Tag(c) = node {
                    stack.push(*c);
                }
            }
        };
        push_children(&mut stack, id);
        while let Some(t) = stack.pop() {
            out.push(t);
            push_children(&mut stack, t);
        }
        out
    }

    /// `el.select(...)` for a selector `pred` decides.
    pub fn select(&self, id: usize, pred: impl Fn(&Tag) -> bool) -> Vec<usize> {
        self.descendants(id).into_iter().filter(|&t| pred(&self.tags[t])).collect()
    }

    pub fn select_one(&self, id: usize, pred: impl Fn(&Tag) -> bool) -> Option<usize> {
        self.descendants(id).into_iter().find(|&t| pred(&self.tags[t]))
    }

    /// Whether some ancestor of `id` is a `name` element.
    pub fn has_ancestor(&self, id: usize, name: &str) -> bool {
        let mut cur = self.tags[id].parent;
        while let Some(p) = cur {
            if p != ROOT && self.tags[p].name == name {
                return true;
            }
            cur = self.tags[p].parent;
        }
        false
    }

    /// `tag.text` (`get_text()`): the strings of the tag's own string types.
    pub fn text(&self, id: usize) -> String {
        let wanted = StrKind::container(&self.tags[id].name);
        let keep = |k: StrKind| match wanted {
            Some(w) => k == w,
            None => matches!(k, StrKind::NavigableString | StrKind::CData),
        };
        let mut out = String::new();
        let mut stack: Vec<std::slice::Iter<'_, Node>> = vec![self.tags[id].children.iter()];
        while let Some(top) = stack.last_mut() {
            match top.next() {
                None => {
                    stack.pop();
                }
                Some(Node::Text(k, s)) => {
                    if keep(*k) {
                        out.push_str(s);
                    }
                }
                Some(Node::Tag(t)) => stack.push(self.tags[*t].children.iter()),
            }
        }
        out
    }
}

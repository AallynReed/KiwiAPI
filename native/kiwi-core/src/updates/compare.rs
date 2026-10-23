//! `app/trove/updates/compare.py`: decode two archived blobs into lines and
//! build the structured unified-diff hunks the compare routes return.

use super::difflib::{Tag, opcodes};
use crate::store::py::splitlines;

pub const MAX_DIFF_BYTES: usize = 1024 * 1024;
pub const HUNK_CONTEXT: usize = 3;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecodedSide {
    pub lines: Vec<String>,
    pub is_text: bool,
    pub reason: Option<&'static str>,
}

/// `_escape_ctrl`: C0 controls (bar TAB/CR/LF/FF) and DEL as `\xNN`.
pub fn escape_ctrl(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        let o = c as u32;
        if matches!(c, '\t' | '\r' | '\n' | '\x0c') {
            out.push(c);
        } else if o < 0x20 || o == 0x7f {
            out.push_str(&format!("\\x{o:02x}"));
        } else {
            out.push(c);
        }
    }
    out
}

/// `bytes.decode("utf-8")`, falling back to latin-1.
pub fn decode_text(data: &[u8]) -> String {
    match std::str::from_utf8(data) {
        Ok(s) => s.to_owned(),
        Err(_) => data.iter().map(|&b| b as char).collect(),
    }
}

/// `decode_blob(data)`.
pub fn decode_blob(data: Option<&[u8]>) -> DecodedSide {
    let Some(data) = data else {
        return DecodedSide { lines: Vec::new(), is_text: true, reason: None };
    };
    if data.len() > MAX_DIFF_BYTES {
        return DecodedSide { lines: Vec::new(), is_text: false, reason: Some("too large") };
    }
    let text = escape_ctrl(&decode_text(data));
    DecodedSide { lines: splitlines(&text).into_iter().map(str::to_owned).collect(), is_text: true, reason: None }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineKind {
    Equal,
    Remove,
    Add,
}

impl LineKind {
    pub fn as_str(self) -> &'static str {
        match self {
            LineKind::Equal => "equal",
            LineKind::Remove => "remove",
            LineKind::Add => "add",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HunkLine<'a> {
    pub kind: LineKind,
    pub left: Option<usize>,
    pub right: Option<usize>,
    pub text: &'a str,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hunk<'a> {
    pub left_start: usize,
    pub right_start: usize,
    pub lines: Vec<HunkLine<'a>>,
}

/// `make_hunks(a_lines, b_lines, context)`.
pub fn make_hunks<'a, S: AsRef<str> + Eq + std::hash::Hash>(a: &'a [S], b: &'a [S], context: usize) -> Vec<Hunk<'a>> {
    let ops = opcodes(a, b);
    let mut hunks = Vec::new();
    let mut i = 0;
    while i < ops.len() {
        if ops[i].tag == Tag::Equal {
            i += 1;
            continue;
        }
        let (ei2, ej2, ctx_before) = if i > 0 && ops[i - 1].tag == Tag::Equal {
            let p = ops[i - 1];
            (p.i2, p.j2, context.min(p.i2 - p.i1))
        } else {
            (ops[i].i1, ops[i].j1, 0)
        };
        let left_start = ei2 - ctx_before;
        let right_start = ej2 - ctx_before;

        let mut run: Vec<(Tag, usize, usize, usize, usize)> = Vec::new();
        if ctx_before > 0 {
            run.push((Tag::Equal, left_start, ei2, right_start, ej2));
        }
        let mut j = i;
        while j < ops.len() {
            let o = ops[j];
            if o.tag == Tag::Equal {
                if j + 1 < ops.len() && o.i2 - o.i1 <= 2 * context {
                    run.push((o.tag, o.i1, o.i2, o.j1, o.j2));
                    j += 1;
                    continue;
                }
                let after = context.min(o.i2 - o.i1);
                if after > 0 {
                    run.push((Tag::Equal, o.i1, o.i1 + after, o.j1, o.j1 + after));
                }
                j += 1;
                break;
            }
            run.push((o.tag, o.i1, o.i2, o.j1, o.j2));
            j += 1;
        }

        let mut lines = Vec::new();
        let remove = |lines: &mut Vec<HunkLine<'a>>, i1: usize, i2: usize| {
            for (k, t) in a[i1..i2].iter().enumerate() {
                lines.push(HunkLine { kind: LineKind::Remove, left: Some(i1 + k + 1), right: None, text: t.as_ref() });
            }
        };
        let add = |lines: &mut Vec<HunkLine<'a>>, j1: usize, j2: usize| {
            for (k, t) in b[j1..j2].iter().enumerate() {
                lines.push(HunkLine { kind: LineKind::Add, left: None, right: Some(j1 + k + 1), text: t.as_ref() });
            }
        };
        for (tag, i1, i2, j1, j2) in run {
            match tag {
                Tag::Equal => {
                    for k in 0..i2 - i1 {
                        lines.push(HunkLine {
                            kind: LineKind::Equal,
                            left: Some(i1 + k + 1),
                            right: Some(j1 + k + 1),
                            text: a[i1 + k].as_ref(),
                        });
                    }
                }
                Tag::Delete => remove(&mut lines, i1, i2),
                Tag::Insert => add(&mut lines, j1, j2),
                Tag::Replace => {
                    remove(&mut lines, i1, i2);
                    add(&mut lines, j1, j2);
                }
            }
        }
        hunks.push(Hunk { left_start: left_start + 1, right_start: right_start + 1, lines });
        i = j;
    }
    hunks
}

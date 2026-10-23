//! The idna 3.20 functions email-validator calls (`uts46_remap`, `alabel`,
//! `encode`, `decode`) and Python's `punycode` codec, with their messages.

use super::{
    table as t,
    unidata::{bidirectional, category, combining, has_name, in_ranges, nfc},
};
use crate::pystr;

const MAX_INPUT: usize = 1024;
const MAX_DOMAIN: usize = 253;

pub struct IdnaError(pub String);

type R<T> = Result<T, IdnaError>;

fn err<T>(m: impl Into<String>) -> R<T> {
    Err(IdnaError(m.into()))
}

fn unot(cp: u32) -> String {
    format!("U+{cp:04X}")
}

/// `repr()` of a str that may hold lone surrogates.
pub fn repr(s: &[u32]) -> String {
    let has = |c: u32| s.contains(&c);
    let quote = if has(0x27) && !has(0x22) { '"' } else { '\'' };
    let mut out = String::from(quote);
    for &n in s {
        match char::from_u32(n) {
            Some('\\') => out.push_str("\\\\"),
            Some('\t') => out.push_str("\\t"),
            Some('\n') => out.push_str("\\n"),
            Some('\r') => out.push_str("\\r"),
            Some(c) if c == quote => {
                out.push('\\');
                out.push(c);
            }
            Some(c) if pystr::is_printable(c) => out.push(c),
            _ if n <= 0xff => out.push_str(&format!("\\x{n:02x}")),
            _ if n <= 0xffff => out.push_str(&format!("\\u{n:04x}")),
            _ => out.push_str(&format!("\\U{n:08x}")),
        }
    }
    out.push(quote);
    out
}

fn is_dot(c: u32) -> bool {
    matches!(c, 0x2E | 0x3002 | 0xFF0E | 0xFF61)
}

fn joining_type(cp: u32) -> Option<u8> {
    let i = t::JOINING.partition_point(|&(_, hi, _)| hi < cp);
    t::JOINING.get(i).filter(|&&(lo, _, _)| lo <= cp).map(|e| e.2)
}

fn check_bidi(label: &[u32]) -> R<()> {
    const RTL: [&str; 3] = ["R", "AL", "AN"];
    const RTL_OK: [&str; 10] = ["R", "AL", "AN", "EN", "ES", "CS", "ET", "ON", "BN", "NSM"];
    const LTR_OK: [&str; 8] = ["L", "EN", "ES", "CS", "ET", "ON", "BN", "NSM"];
    if label.len() > MAX_INPUT {
        return err("Label too long");
    }
    let mut bidi_label = false;
    for (i, &cp) in label.iter().enumerate() {
        let d = bidirectional(cp);
        if d.is_empty() {
            return err(format!("Unknown directionality in label {} at position {}", repr(label), i + 1));
        }
        if RTL.contains(&d) {
            bidi_label = true;
        }
    }
    if !bidi_label {
        return Ok(());
    }
    let rtl = match bidirectional(label[0]) {
        "R" | "AL" => true,
        "L" => false,
        _ => return err(format!("First codepoint in label {} must be directionality L, R or AL", repr(label))),
    };
    let mut valid_ending = false;
    let mut number_type: Option<&str> = None;
    for (i, &cp) in label.iter().enumerate() {
        let d = bidirectional(cp);
        if rtl {
            if !RTL_OK.contains(&d) {
                return err(format!("Invalid direction for codepoint at position {} in a right-to-left label", i + 1));
            }
            if matches!(d, "R" | "AL" | "EN" | "AN") {
                valid_ending = true;
            } else if d != "NSM" {
                valid_ending = false;
            }
            if matches!(d, "AN" | "EN") {
                match number_type {
                    None => number_type = Some(d),
                    Some(n) if n != d => return err("Can not mix numeral types in a right-to-left label"),
                    _ => {}
                }
            }
        } else {
            if !LTR_OK.contains(&d) {
                return err(format!("Invalid direction for codepoint at position {} in a left-to-right label", i + 1));
            }
            if matches!(d, "L" | "EN") {
                valid_ending = true;
            } else if d != "NSM" {
                valid_ending = false;
            }
        }
    }
    if !valid_ending {
        return err("Label ends with illegal codepoint directionality");
    }
    Ok(())
}

/// `_combining_class`: `None` where it raises `ValueError`.
fn combining_class(cp: u32) -> Option<u8> {
    let v = combining(cp);
    (v != 0 || has_name(cp)).then_some(v)
}

fn valid_contextj(label: &[u32], pos: usize) -> Option<bool> {
    let cp = label[pos];
    if cp == 0x200C {
        if pos > 0 && combining_class(label[pos - 1])? == 9 {
            return Some(true);
        }
        let mut ok = false;
        for &c in label[..pos].iter().rev() {
            match joining_type(c) {
                Some(b'T') => continue,
                Some(b'L' | b'D') => ok = true,
                _ => {}
            }
            break;
        }
        if !ok {
            return Some(false);
        }
        for &c in &label[pos + 1..] {
            match joining_type(c) {
                Some(b'T') => continue,
                Some(b'R' | b'D') => return Some(true),
                _ => return Some(false),
            }
        }
        return Some(false);
    }
    if cp == 0x200D {
        return Some(pos > 0 && combining_class(label[pos - 1])? == 9);
    }
    Some(false)
}

fn valid_contexto(label: &[u32], pos: usize) -> bool {
    let cp = label[pos];
    let script = |c: u32, r: &[(u32, u32)]| in_ranges(r, c);
    match cp {
        0xB7 => pos > 0 && pos + 1 < label.len() && label[pos - 1] == 0x6C && label[pos + 1] == 0x6C,
        0x375 => pos + 1 < label.len() && script(label[pos + 1], t::SCRIPT_GREEK),
        0x5F3 | 0x5F4 => pos > 0 && script(label[pos - 1], t::SCRIPT_HEBREW),
        0x30FB => label.iter().any(|&c| {
            c != 0x30FB && (script(c, t::SCRIPT_HIRAGANA) || script(c, t::SCRIPT_KATAKANA) || script(c, t::SCRIPT_HAN))
        }),
        0x660..=0x669 => !label.iter().any(|c| (0x6F0..=0x6F9).contains(c)),
        0x6F0..=0x6F9 => !label.iter().any(|c| (0x660..=0x669).contains(c)),
        _ => false,
    }
}

pub fn check_label(label: &[u32]) -> R<()> {
    if label.len() > MAX_INPUT {
        return err("Label too long");
    }
    if label.is_empty() {
        return err("Empty Label");
    }
    if label.len() > MAX_DOMAIN + 1 {
        return err("Label too long");
    }
    if nfc(label) != label {
        return err("Label must be in Normalization Form C");
    }
    if label.len() >= 4 && label[2] == 0x2D && label[3] == 0x2D {
        return err("Label has disallowed hyphens in 3rd and 4th position");
    }
    if label[0] == 0x2D || label[label.len() - 1] == 0x2D {
        return err("Label must not start or end with a hyphen");
    }
    if category(label[0]).starts_with('M') {
        return err("Label begins with an illegal combining character");
    }
    for (pos, &cp) in label.iter().enumerate() {
        if in_ranges(t::PVALID, cp) {
            continue;
        }
        if in_ranges(t::CONTEXTJ, cp) {
            match valid_contextj(label, pos) {
                None => {
                    return err(format!(
                        "Unknown codepoint adjacent to joiner {} at position {} in {}",
                        unot(cp),
                        pos + 1,
                        repr(label)
                    ));
                }
                Some(false) => {
                    return err(format!("Joiner {} not allowed at position {} in {}", unot(cp), pos + 1, repr(label)));
                }
                Some(true) => {}
            }
        } else if in_ranges(t::CONTEXTO, cp) {
            if !valid_contexto(label, pos) {
                return err(format!("Codepoint {} not allowed at position {} in {}", unot(cp), pos + 1, repr(label)));
            }
        } else {
            return err(format!("Codepoint {} at position {} of {} not allowed", unot(cp), pos + 1, repr(label)));
        }
    }
    check_bidi(label)
}

fn adapt(mut delta: i64, first: bool, numchars: i64) -> i64 {
    delta /= if first { 700 } else { 2 };
    delta += delta / numchars;
    let mut divisions = 0;
    while delta > 455 {
        delta /= 35;
        divisions += 36;
    }
    divisions + 36 * delta / (delta + 38)
}

fn threshold(j: i64, bias: i64) -> i64 {
    (36 * (j + 1) - bias).clamp(1, 26)
}

/// `s.encode("punycode")`.
pub fn punycode_encode(s: &[u32]) -> Vec<u8> {
    let base: Vec<u8> = s.iter().filter(|&&c| c < 0x80).map(|&c| c as u8).collect();
    let mut extended: Vec<u32> = s.iter().copied().filter(|&c| c >= 0x80).collect();
    extended.sort_unstable();
    extended.dedup();
    let mut deltas = Vec::new();
    let (mut oldchar, mut oldindex) = (0x80i64, -1i64);
    for &c in &extended {
        let (mut index, mut pos) = (-1i64, -1i64);
        let curlen = s.iter().filter(|&&x| x < c).count() as i64;
        let mut delta = (curlen + 1) * (i64::from(c) - oldchar);
        loop {
            loop {
                pos += 1;
                if pos as usize == s.len() {
                    index = -1;
                    break;
                }
                let x = s[pos as usize];
                if x == c {
                    index += 1;
                    break;
                }
                if x < c {
                    index += 1;
                }
            }
            if index == -1 {
                break;
            }
            delta += index - oldindex;
            deltas.push(delta - 1);
            oldindex = index;
            delta = 0;
        }
        oldchar = i64::from(c);
    }
    const DIGITS: &[u8; 36] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let mut out = Vec::new();
    let mut bias = 72;
    for (points, &delta) in deltas.iter().enumerate() {
        let mut n = delta;
        let mut j = 0;
        loop {
            let t = threshold(j, bias);
            if n < t {
                out.push(DIGITS[n as usize]);
                break;
            }
            out.push(DIGITS[(t + (n - t) % (36 - t)) as usize]);
            n = (n - t) / (36 - t);
            j += 1;
        }
        bias = adapt(delta, points == 0, base.len() as i64 + points as i64 + 1);
    }
    if base.is_empty() {
        return out;
    }
    let mut r = base;
    r.push(b'-');
    r.extend(out);
    r
}

/// `text.decode("punycode")` of ASCII bytes; `None` where it raises.
fn punycode_decode(text: &[u8]) -> Option<Vec<u32>> {
    const LIMIT: i64 = 1 << 40;
    let (mut base, ext): (Vec<u32>, &[u8]) = match text.iter().rposition(|&b| b == b'-') {
        None => (Vec::new(), text),
        Some(p) => (text[..p].iter().map(|&b| u32::from(b)).collect(), &text[p + 1..]),
    };
    let ext: Vec<u8> = ext.to_ascii_uppercase();
    let (mut ch, mut pos, mut bias, mut extpos) = (0x80i64, -1i64, 72i64, 0usize);
    while extpos < ext.len() {
        let (mut result, mut w, mut j) = (0i64, 1i64, 0i64);
        let mut at = extpos;
        let delta = loop {
            let c = *ext.get(at)?;
            at += 1;
            let digit = match c {
                b'A'..=b'Z' => i64::from(c - b'A'),
                b'0'..=b'9' => i64::from(c) - 22,
                _ => return None,
            };
            let t = threshold(j, bias);
            result += digit * w;
            if result > LIMIT {
                return None;
            }
            if digit < t {
                break result;
            }
            w *= 36 - t;
            if w > LIMIT {
                return None;
            }
            j += 1;
        };
        pos += delta + 1;
        let n = base.len() as i64 + 1;
        ch += pos / n;
        if ch > 0x10FFFF {
            return None;
        }
        pos %= n;
        base.insert(pos as usize, ch as u32);
        bias = adapt(delta, extpos == 0, base.len() as i64);
        extpos = at;
    }
    Some(base)
}

fn ulabel(label: &[u32]) -> R<Vec<u32>> {
    if label.len() > MAX_INPUT {
        return err("Label too long");
    }
    if label.iter().any(|&c| c >= 0x80) {
        check_label(label)?;
        return Ok(label.to_vec());
    }
    let lower: Vec<u8> = label.iter().map(|&c| (c as u8).to_ascii_lowercase()).collect();
    let Some(rest) = lower.strip_prefix(b"xn--") else {
        let cps: Vec<u32> = lower.iter().map(|&b| u32::from(b)).collect();
        check_label(&cps)?;
        return Ok(cps);
    };
    if rest.is_empty() {
        return err("Malformed A-label, no Punycode eligible content found");
    }
    if rest.ends_with(b"-") {
        return err("A-label must not end with a hyphen");
    }
    let Some(decoded) = punycode_decode(rest) else { return err("Invalid A-label") };
    if punycode_encode(&decoded) != rest {
        return err("A-label is not the canonical Punycode encoding of its U-label");
    }
    check_label(&decoded)?;
    Ok(decoded)
}

pub fn alabel(label: &[u32]) -> R<Vec<u8>> {
    if label.len() > MAX_INPUT {
        return err("Label too long");
    }
    if label.iter().all(|&c| c < 0x80) {
        ulabel(label)?;
        if label.len() > 63 {
            return err("Label too long");
        }
        return Ok(label.iter().map(|&c| c as u8).collect());
    }
    check_label(label)?;
    let mut out = b"xn--".to_vec();
    out.extend(punycode_encode(label));
    if out.len() > 63 {
        return err("Label too long");
    }
    Ok(out)
}

fn split_labels(s: &[u32]) -> R<(Vec<&[u32]>, bool)> {
    let mut labels: Vec<&[u32]> = s.split(|&c| is_dot(c)).collect();
    if labels.len() == 1 && labels[0].is_empty() {
        return err("Empty domain");
    }
    let trailing = labels.last().is_some_and(|l| l.is_empty());
    if trailing {
        labels.pop();
    }
    Ok((labels, trailing))
}

/// `idna.encode(s)`.
pub fn encode(s: &[u32]) -> R<Vec<u8>> {
    if s.len() > MAX_INPUT || s.len() > MAX_DOMAIN + 1 {
        return err("Domain too long");
    }
    let (labels, trailing) = split_labels(s)?;
    let mut parts = Vec::new();
    for label in labels {
        let a = alabel(label)?;
        if a.is_empty() {
            return err("Empty label");
        }
        parts.push(a);
    }
    if trailing {
        parts.push(Vec::new());
    }
    let out = parts.join(&b'.');
    if out.len() > MAX_DOMAIN + usize::from(trailing) {
        return err("Domain too long");
    }
    Ok(out)
}

/// `idna.decode(s)`.
pub fn decode(s: &[u32]) -> R<Vec<u32>> {
    if s.len() > MAX_INPUT || s.len() > MAX_DOMAIN + 1 {
        return err("Domain too long");
    }
    let (labels, trailing) = split_labels(s)?;
    let mut parts = Vec::new();
    for label in labels {
        let u = ulabel(label)?;
        if u.is_empty() {
            return err("Empty label");
        }
        parts.push(u);
    }
    if trailing {
        parts.push(Vec::new());
    }
    Ok(parts.join(&0x2E))
}

/// `idna.uts46_remap(domain, std3_rules=False, transitional=False)`.
pub fn uts46_remap(domain: &[u32]) -> R<Vec<u32>> {
    if domain.len() > MAX_INPUT {
        return err("Domain too long");
    }
    if domain.iter().all(|&c| c < 0x80) {
        return Ok(domain.iter().map(|&c| if (0x41..=0x5A).contains(&c) { c + 32 } else { c }).collect());
    }
    let mut out = Vec::with_capacity(domain.len());
    for (pos, &cp) in domain.iter().enumerate() {
        let i = if cp < 256 { cp as usize } else { t::UTS46_STARTS.partition_point(|&s| s <= cp) - 1 };
        match t::UTS46_STATUS[i] {
            b'V' | b'D' => out.push(cp),
            b'M' => out.extend(t::UTS46_REPL[i].chars().map(u32::from)),
            b'I' => {}
            _ => {
                return err(format!("Codepoint {} not allowed at position {} in {}", unot(cp), pos + 1, repr(domain)));
            }
        }
    }
    Ok(nfc(&out))
}

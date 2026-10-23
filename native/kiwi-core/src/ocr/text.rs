//! `app/trove/ocr/vocabulary.py` and `parse.py`: the closed stat vocabulary,
//! the fuzzy label match (difflib's `SequenceMatcher.ratio`) and the
//! line parser that pairs numbers with labels.

// Index loops mirror the C/Python they reproduce.
#![allow(clippy::needless_range_loop)]

use std::{collections::HashMap, sync::LazyLock};

use regex::Regex;
use serde_json::{Map, Value};
use unicode_normalization::{UnicodeNormalization, char::canonical_combining_class};

use crate::{pynum, pystr};

#[derive(Clone, Debug, PartialEq)]
pub struct StatDef {
    pub key: String,
    pub percent: bool,
    pub min: f64,
    pub max: f64,
}

pub struct Vocab {
    pub defs: Vec<StatDef>,
    syns: Vec<(String, String)>,
}

/// `normalize`: NFKD, drop combining marks, lowercase, `[^a-z0-9]+` -> space, strip.
pub fn normalize(text: &str) -> String {
    let no_marks: String = text.nfkd().filter(|&c| canonical_combining_class(c) == 0).collect();
    let mut out = String::new();
    let mut gap = false;
    for c in no_marks.to_lowercase().chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            if gap && !out.is_empty() {
                out.push(' ');
            }
            gap = false;
            out.push(c);
        } else {
            gap = true;
        }
    }
    out
}

fn num(v: Option<&Value>, default: f64) -> f64 {
    v.and_then(Value::as_f64).unwrap_or(default)
}

impl Vocab {
    /// `_load` over `character_stats.json`.
    pub fn from_json(data: &Value) -> Option<Vocab> {
        let mut defs = Vec::new();
        let mut syns = Vec::new();
        for s in data.get("stats")?.as_array()? {
            let s = s.as_object()?;
            let key = s.get("key")?.as_str()?.to_owned();
            let ty = s.get("type").and_then(Value::as_str).unwrap_or("int");
            let def = StatDef {
                key: key.clone(),
                percent: ty == "percent",
                min: num(s.get("min"), 0.0),
                max: num(s.get("max"), 1e12),
            };
            match defs.iter_mut().find(|d: &&mut StatDef| d.key == key) {
                Some(d) => *d = def,
                None => defs.push(def),
            }
            for (field, vals) in s {
                if matches!(field.as_str(), "key" | "type" | "max" | "min") {
                    continue;
                }
                let Some(vals) = vals.as_array() else { continue };
                for label in vals {
                    let norm = normalize(label.as_str()?);
                    if !norm.is_empty() {
                        syns.push((key.clone(), norm));
                    }
                }
            }
        }
        syns.sort_by_key(|(_, n)| std::cmp::Reverse(n.chars().count()));
        Some(Vocab { defs, syns })
    }

    pub fn stat_def(&self, key: &str) -> Option<&StatDef> {
        self.defs.iter().find(|d| d.key == key)
    }

    /// `match_label(text)` with the 0.82 threshold.
    pub fn match_label(&self, text: &str) -> Option<(String, f64)> {
        let norm = normalize(text);
        if norm.is_empty() {
            return None;
        }
        let a: Vec<char> = norm.chars().collect();
        let mut best: Option<(String, f64)> = None;
        for (key, syn) in &self.syns {
            let score = if *syn == norm {
                1.0
            } else if norm.contains(syn.as_str()) {
                0.97
            } else {
                let b: Vec<char> = syn.chars().collect();
                ratio(&a, &b)
            };
            if score >= 0.82 && best.as_ref().is_none_or(|(_, s)| score > *s) {
                best = Some((key.clone(), score));
            }
        }
        best
    }
}

/// `difflib.SequenceMatcher(None, a, b).ratio()` (no junk; `b` under 200 items).
pub fn ratio(a: &[char], b: &[char]) -> f64 {
    let mut b2j: HashMap<char, Vec<usize>> = HashMap::new();
    for (j, &c) in b.iter().enumerate() {
        b2j.entry(c).or_default().push(j);
    }
    if b.len() >= 200 {
        let ntest = b.len() / 100 + 1;
        b2j.retain(|_, v| v.len() <= ntest);
    }
    let longest = |alo: usize, ahi: usize, blo: usize, bhi: usize| -> (usize, usize, usize) {
        let (mut besti, mut bestj, mut bestsize) = (alo, blo, 0usize);
        let mut j2len: HashMap<usize, usize> = HashMap::new();
        for i in alo..ahi {
            let mut new: HashMap<usize, usize> = HashMap::new();
            if let Some(js) = b2j.get(&a[i]) {
                for &j in js {
                    if j < blo {
                        continue;
                    }
                    if j >= bhi {
                        break;
                    }
                    let k = j.checked_sub(1).and_then(|p| j2len.get(&p)).copied().unwrap_or(0) + 1;
                    new.insert(j, k);
                    if k > bestsize {
                        besti = i + 1 - k;
                        bestj = j + 1 - k;
                        bestsize = k;
                    }
                }
            }
            j2len = new;
        }
        while besti > alo && bestj > blo && a[besti - 1] == b[bestj - 1] {
            besti -= 1;
            bestj -= 1;
            bestsize += 1;
        }
        while besti + bestsize < ahi && bestj + bestsize < bhi && a[besti + bestsize] == b[bestj + bestsize] {
            bestsize += 1;
        }
        (besti, bestj, bestsize)
    };
    let mut queue = vec![(0usize, a.len(), 0usize, b.len())];
    let mut matches = 0usize;
    while let Some((alo, ahi, blo, bhi)) = queue.pop() {
        let (i, j, k) = longest(alo, ahi, blo, bhi);
        if k > 0 {
            matches += k;
            if alo < i && blo < j {
                queue.push((alo, i, blo, j));
            }
            if i + k < ahi && j + k < bhi {
                queue.push((i + k, ahi, j + k, bhi));
            }
        }
    }
    let length = a.len() + b.len();
    if length > 0 { 2.0 * matches as f64 / length as f64 } else { 1.0 }
}

// --- parse.py ----------------------------------------------------------------

static NUMBER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\d[\d.,]*\d[\s\x1c-\x1f]*%?|\d[\s\x1c-\x1f]*%?").expect("static pattern"));

/// `_parse_number`: `(value, is_percent)`.
pub fn parse_number(token: &str) -> Option<(f64, bool)> {
    let t = pystr::strip(token).replace(' ', "");
    let (t, pct) = match t.strip_suffix('%') {
        Some(rest) => (rest.to_owned(), true),
        None => (t, false),
    };
    if t.is_empty() {
        return None;
    }
    let num_str = match t.char_indices().rfind(|&(_, c)| c == '.' || c == ',') {
        Some((last, _)) => {
            let frac = &t[last + 1..];
            let n = frac.chars().count();
            if !frac.is_empty() && frac.chars().all(|c| pynum::decimal(c).is_some()) && (1..=2).contains(&n) {
                format!("{}.{frac}", t[..last].replace([',', '.'], ""))
            } else {
                t.replace([',', '.'], "")
            }
        }
        None => t,
    };
    pynum::float(&num_str).map(|v| (v, pct))
}

#[derive(Debug)]
enum Elem {
    Num { start: usize, value: f64, pct: bool, raw: String },
    Label { start: usize, key: String, score: f64 },
}

impl Elem {
    fn start(&self) -> usize {
        match self {
            Elem::Num { start, .. } | Elem::Label { start, .. } => *start,
        }
    }
}

fn line_elements(v: &Vocab, line: &str) -> Vec<Elem> {
    let mut elems = Vec::new();
    let mut last = 0;
    let add_label = |elems: &mut Vec<Elem>, chunk: &str, start: usize| {
        if !pystr::strip(chunk).is_empty()
            && let Some((key, score)) = v.match_label(chunk)
        {
            elems.push(Elem::Label { start, key, score });
        }
    };
    for m in NUMBER_RE.find_iter(line) {
        let Some((value, pct)) = parse_number(m.as_str()) else { continue };
        add_label(&mut elems, &line[last..m.start()], last);
        elems.push(Elem::Num { start: m.start(), value, pct, raw: pystr::strip(m.as_str()).to_owned() });
        last = m.end();
    }
    add_label(&mut elems, &line[last..], last);
    elems.sort_by_key(Elem::start);
    elems
}

/// A recognised stat as `OcrStatValue` fields.
#[derive(Clone, Debug, PartialEq)]
pub struct Stat {
    /// `int` stats hold an integral value; `percent` stats a float.
    pub value: f64,
    pub is_int: bool,
    pub unit: &'static str,
    pub raw: String,
    pub confidence: f64,
    pub in_range: bool,
    pub type_match: bool,
    pub derived: bool,
}

/// Python's `round(x, 3)`.
pub fn round3(x: f64) -> f64 {
    format!("{x:.3}").parse().unwrap_or(x)
}

fn record(v: &Vocab, key: &str, value: f64, pct: bool, raw: &str, base: f64) -> Stat {
    let sd = v.stat_def(key);
    let type_match = sd.is_none_or(|d| d.percent == pct);
    let in_range = sd.is_none_or(|d| d.min <= value && value <= d.max);
    let mut conf = base;
    if !in_range {
        conf *= 0.4;
    }
    if !type_match {
        conf *= 0.6;
    }
    let is_int = sd.is_some_and(|d| !d.percent);
    Stat {
        value: if is_int { value.round_ties_even() } else { value },
        is_int,
        unit: if sd.is_some_and(|d| d.percent) { "percent" } else { "count" },
        raw: raw.to_owned(),
        confidence: round3(conf),
        in_range,
        type_match,
        derived: false,
    }
}

/// Insertion-ordered stats (`dict[str, dict]`).
pub type Stats = Vec<(String, Stat)>;

fn insert(out: &mut Stats, key: &str, rec: Stat) {
    match out.iter_mut().find(|(k, _)| k == key) {
        Some((_, prev)) => {
            if rec.confidence > prev.confidence {
                *prev = rec;
            }
        }
        None => out.push((key.to_owned(), rec)),
    }
}

/// `app.trove.stats.compute_coefficient` over OCR'd values.
fn coefficient(phys: Option<f64>, magic: Option<f64>, crit: Option<f64>) -> Option<f64> {
    let mut best: Option<f64> = None;
    for v in [phys, magic].into_iter().flatten() {
        if best.is_none_or(|b| v > b) {
            best = Some(v);
        }
    }
    let (base, crit) = (best?, crit?);
    let v: f64 = format!("{:.6}", base * (1.0 + crit / 100.0)).parse().ok()?;
    Some(v.trunc())
}

fn get<'a>(out: &'a Stats, key: &str) -> Option<&'a Stat> {
    out.iter().find(|(k, _)| k == key).map(|(_, s)| s)
}

fn reconcile_coefficient(v: &Vocab, out: &mut Stats) {
    let val = |k: &str| get(out, k).map(|s| s.value);
    let Some(derived) = coefficient(val("physical_damage"), val("magic_damage"), val("critical_damage")) else {
        return;
    };
    let sd = v.stat_def("coefficient");
    let in_range = sd.is_none_or(|d| d.min <= derived && derived <= d.max);
    let input_conf = ["physical_damage", "magic_damage", "critical_damage"]
        .iter()
        .filter_map(|k| get(out, k))
        .map(|s| s.confidence)
        .fold(f64::INFINITY, f64::min);
    match out.iter_mut().find(|(k, _)| k == "coefficient") {
        None => out.push((
            "coefficient".into(),
            Stat {
                value: derived,
                is_int: true,
                unit: "count",
                raw: String::new(),
                confidence: round3(input_conf),
                in_range,
                type_match: true,
                derived: true,
            },
        )),
        Some((_, read)) => {
            let tol = ((derived * 0.005).round_ties_even()).max(5.0);
            if (read.value - derived).abs() <= tol {
                read.value = derived;
                read.is_int = true;
                read.confidence = round3(read.confidence.max(input_conf));
            } else {
                read.confidence = round3(read.confidence * 0.5);
            }
        }
    }
}

/// `extract_character_stats(lines)`: the stats in Python's dict order.
pub fn extract(v: &Vocab, lines: &[String]) -> Stats {
    let mut out: Stats = Vec::new();
    let mut unpaired: Vec<(usize, String, f64)> = Vec::new();
    let mut orphans: Vec<(usize, f64, bool, String)> = Vec::new();
    for (li, line) in lines.iter().enumerate() {
        let elems = line_elements(v, line);
        let mut consumed = vec![false; elems.len()];
        for (i, el) in elems.iter().enumerate() {
            let Elem::Label { key, score, .. } = el else { continue };
            let sd = v.stat_def(key);
            let cands: Vec<usize> = [i.checked_sub(1), Some(i + 1)]
                .into_iter()
                .flatten()
                .filter(|&j| j < elems.len() && matches!(elems[j], Elem::Num { .. }) && !consumed[j])
                .collect();
            if cands.is_empty() {
                unpaired.push((li, key.clone(), *score));
                continue;
            }
            let rank = |j: usize| {
                let Elem::Num { pct, .. } = elems[j] else { return (false, 0i64) };
                (sd.is_none_or(|d| d.percent == pct), -(j as i64 - i as i64).abs())
            };
            let mut best_j = cands[0];
            for &j in &cands[1..] {
                if rank(j) > rank(best_j) {
                    best_j = j;
                }
            }
            consumed[best_j] = true;
            if let Elem::Num { value, pct, raw, .. } = &elems[best_j] {
                let rec = record(v, key, *value, *pct, raw, *score);
                insert(&mut out, key, rec);
            }
        }
        for (idx, el) in elems.iter().enumerate() {
            if let Elem::Num { value, pct, raw, .. } = el
                && !consumed[idx]
            {
                orphans.push((li, *value, *pct, raw.clone()));
            }
        }
    }
    let mut used = vec![false; orphans.len()];
    for (li, key, score) in &unpaired {
        if get(&out, key).is_some() {
            continue;
        }
        let sd = v.stat_def(key);
        let mut best: Option<(usize, usize)> = None;
        for (oi, (oli, _, opct, _)) in orphans.iter().enumerate() {
            if used[oi] || oli.abs_diff(*li) > 4 {
                continue;
            }
            if sd.is_some_and(|d| d.percent != *opct) {
                continue;
            }
            let dist = oli.abs_diff(*li);
            if best.is_none_or(|(d, _)| dist < d) {
                best = Some((dist, oi));
            }
        }
        let Some((_, oi)) = best else { continue };
        used[oi] = true;
        let (_, oval, opct, oraw) = &orphans[oi];
        let rec = record(v, key, *oval, *opct, oraw, score * 0.85);
        insert(&mut out, key, rec);
    }
    reconcile_coefficient(v, &mut out);
    out
}

/// One `OcrStatValue` as pydantic serializes it.
pub fn stat_json(s: &Stat) -> Value {
    let mut m = Map::new();
    m.insert("value".into(), if s.is_int { int_value(s.value) } else { Value::from(s.value) });
    m.insert("unit".into(), Value::from(s.unit));
    m.insert("raw".into(), Value::from(s.raw.clone()));
    m.insert("confidence".into(), Value::from(s.confidence));
    m.insert("in_range".into(), Value::from(s.in_range));
    m.insert("type_match".into(), Value::from(s.type_match));
    m.insert("derived".into(), Value::from(s.derived));
    Value::Object(m)
}

/// An integral float as a JSON int (big values through their exact digits).
fn int_value(v: f64) -> Value {
    if v.abs() < 9.0e18 {
        Value::from(v as i64)
    } else {
        crate::stats::py_int_digits(v).and_then(|d| serde_json::from_str(&d).ok()).unwrap_or(Value::Null)
    }
}

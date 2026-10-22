//! The Possible-cheaters / Alt-clusters pass of
//! `app/trove/leaderboards/detection.py` over captures already loaded: the
//! per-board checks, the cluster producers, fusion, confidence and the payload
//! (built as Python dicts in insertion order). Store access stays with the
//! caller; the kernels it shares with Python live in [`crate::detection`].

use std::collections::{BTreeSet, HashMap};

use serde_json::{Map, Number, Value};

use crate::{detection, pystr};

pub const ALT_CEILING: f64 = 0.95;
pub const COMOVEMENT_CEILING: f64 = 0.97;
pub const SCHEDULE_CEILING: f64 = 0.85;
pub const FUSION_CEILING: f64 = 0.98;
const WEEKLY_UPTIME_MIN_SLOTS: usize = 48;
const VELOCITY_PEER_TOP_N: usize = 50;
const RANK_GAP_THRESHOLD: f64 = 10.0;
const CAP: usize = 60;
const RESET_HOUR: i64 = 11 * 3600;

pub const METHOD: &str = "Four independent checks: Modified Z-score (MAD-based, Iglewicz & Hoaglin 1993), \
rank-gap ratio, and velocity vs peer p95 flag individual outliers; alt-cluster detection groups \
similarly-named accounts sitting at near-identical scores. A player flagged by multiple checks or on \
multiple boards - or a cluster tighter, larger, and on more boards - is higher confidence.";

// ---------------------------------------------------------------------------
// Python number semantics

pub fn py_max(a: f64, b: f64) -> f64 {
    if b > a { b } else { a }
}

pub fn py_min(a: f64, b: f64) -> f64 {
    if b < a { b } else { a }
}

/// `round(x, n)`: the exact binary value rounded half-even, read back.
pub fn py_round(x: f64, n: usize) -> f64 {
    if !x.is_finite() {
        return x;
    }
    format!("{x:.n$}").parse().unwrap_or(x)
}

/// Builtin `sum()` of floats (Neumaier-compensated since Python 3.12).
pub fn py_sum(xs: impl IntoIterator<Item = f64>) -> f64 {
    let mut it = xs.into_iter();
    let Some(first) = it.next() else { return 0.0 };
    let (mut f, mut c) = (0.0 + first, 0.0);
    for x in it {
        let t = f + x;
        if f.abs() >= x.abs() {
            c += (f - t) + x;
        } else {
            c += (x - t) + f;
        }
        f = t;
    }
    if c != 0.0 && c.is_finite() {
        f += c;
    }
    f
}

fn sort_floats(xs: &mut [f64]) {
    xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
}

/// `statistics.median`.
pub fn median(xs: &[f64]) -> f64 {
    let mut s = xs.to_vec();
    sort_floats(&mut s);
    let n = s.len();
    if n % 2 == 1 { s[n / 2] } else { (s[n / 2 - 1] + s[n / 2]) / 2.0 }
}

pub fn fv(x: f64) -> Value {
    Number::from_f64(x).map(Value::Number).unwrap_or(Value::Null)
}

/// `_round`: integral floats become ints, the rest round to 2 places.
pub fn round_val(v: f64) -> Value {
    if v.is_finite() && v == v.trunc() && v.abs() < 9.2e18 { Value::from(v as i64) } else { fv(py_round(v, 2)) }
}

fn group_digits(s: &str) -> String {
    let (sign, rest) = s.strip_prefix('-').map_or(("", s), |r| ("-", r));
    let (int, frac) = rest.split_once('.').map_or((rest, None), |(i, f)| (i, Some(f)));
    let mut out = String::with_capacity(s.len() + int.len() / 3);
    out.push_str(sign);
    for (i, c) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    if let Some(f) = frac {
        out.push('.');
        out.push_str(f);
    }
    out
}

/// `f"{v:.Nf}"`.
pub fn fixed(v: f64, n: usize) -> String {
    if v.is_nan() {
        return "nan".into();
    }
    format!("{v:.n$}")
}

/// `_fmt`: thousands separators, no `.0` on integral values.
pub fn fmt_num(v: f64) -> String {
    if v.is_finite() && v == v.trunc() {
        if v.abs() < 9.2e18 { group_digits(&(v as i64).to_string()) } else { group_digits(&format!("{v:.0}")) }
    } else {
        group_digits(&fixed(v, 2))
    }
}

/// `f"{v:.3g}"`.
pub fn g3(v: f64) -> String {
    if !v.is_finite() {
        return fixed(v, 0);
    }
    if v == 0.0 {
        return if v.is_sign_negative() { "-0".into() } else { "0".into() };
    }
    let e = format!("{v:.2e}");
    let (mant, exp) = e.split_once('e').unwrap_or((&e, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    let strip = |s: &str| -> String {
        if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_string() } else { s.to_string() }
    };
    if (-4..3).contains(&exp) {
        strip(&format!("{v:.*}", (2 - exp) as usize))
    } else {
        format!("{}e{}{:02}", strip(mant), if exp < 0 { '-' } else { '+' }, exp.abs())
    }
}

#[rustfmt::skip]
const DECIMAL: &[(u32, u32)] = &[
    (0x30, 0x39), (0x660, 0x669), (0x6f0, 0x6f9), (0x7c0, 0x7c9), (0x966, 0x96f), (0x9e6, 0x9ef),
    (0xa66, 0xa6f), (0xae6, 0xaef), (0xb66, 0xb6f), (0xbe6, 0xbef), (0xc66, 0xc6f), (0xce6, 0xcef),
    (0xd66, 0xd6f), (0xde6, 0xdef), (0xe50, 0xe59), (0xed0, 0xed9), (0xf20, 0xf29), (0x1040, 0x1049),
    (0x1090, 0x1099), (0x17e0, 0x17e9), (0x1810, 0x1819), (0x1946, 0x194f), (0x19d0, 0x19d9),
    (0x1a80, 0x1a89), (0x1a90, 0x1a99), (0x1b50, 0x1b59), (0x1bb0, 0x1bb9), (0x1c40, 0x1c49),
    (0x1c50, 0x1c59), (0xa620, 0xa629), (0xa8d0, 0xa8d9), (0xa900, 0xa909), (0xa9d0, 0xa9d9),
    (0xa9f0, 0xa9f9), (0xaa50, 0xaa59), (0xabf0, 0xabf9), (0xff10, 0xff19), (0x104a0, 0x104a9),
    (0x10d30, 0x10d39), (0x11066, 0x1106f), (0x110f0, 0x110f9), (0x11136, 0x1113f), (0x111d0, 0x111d9),
    (0x112f0, 0x112f9), (0x11450, 0x11459), (0x114d0, 0x114d9), (0x11650, 0x11659), (0x116c0, 0x116c9),
    (0x11730, 0x11739), (0x118e0, 0x118e9), (0x11950, 0x11959), (0x11c50, 0x11c59), (0x11d50, 0x11d59),
    (0x11da0, 0x11da9), (0x11f50, 0x11f59), (0x16a60, 0x16a69), (0x16ac0, 0x16ac9), (0x16b50, 0x16b59),
    (0x1d7ce, 0x1d7ff), (0x1e140, 0x1e149), (0x1e2f0, 0x1e2f9), (0x1e4f0, 0x1e4f9), (0x1e950, 0x1e959),
    (0x1fbf0, 0x1fbf9),
];

/// `str.isdecimal()` (Unicode 15.1, Python 3.13): the digit's value.
pub fn decimal_value(c: char) -> Option<u32> {
    let n = c as u32;
    let i = DECIMAL.partition_point(|&(_, hi)| hi < n);
    DECIMAL.get(i).filter(|&&(lo, _)| lo <= n).map(|&(lo, _)| (n - lo) % 10)
}

/// Python `int(s)` of a stripped decimal string.
pub fn py_int(s: &str) -> Option<i64> {
    let (neg, digits) = match s.chars().next()? {
        '-' => (true, &s[1..]),
        '+' => (false, &s[1..]),
        _ => (false, s),
    };
    if digits.is_empty() || digits.starts_with('_') || digits.ends_with('_') || digits.contains("__") {
        return None;
    }
    let mut n: i64 = 0;
    for c in digits.chars() {
        if c == '_' {
            continue;
        }
        n = n.checked_mul(10)?.checked_add(decimal_value(c)? as i64)?;
    }
    Some(if neg { -n } else { n })
}

/// `_parse_excluded`: the board uuids of a CSV, non-numeric tokens skipped.
pub fn parse_excluded(csv: &str) -> BTreeSet<i64> {
    csv.split(',').map(pystr::strip).filter(|s| !s.is_empty()).filter_map(py_int).collect()
}

// ---------------------------------------------------------------------------
// Boards

const DAILY_RESET: &[i64] = &[32000];
#[rustfmt::skip]
const WEEKLY_RESET: &[i64] = &[
    2001, 2002, 2004, 2011, 2013, 2014, 2021, 2024,
    2300, 2301, 2302, 2303, 2304, 2305, 2306, 2307, 2308, 2309, 2310, 2311, 2312, 2313, 2314, 2315, 2316, 2317,
    2400, 2401, 2402, 2403, 2404, 2405, 2406, 2407, 2408, 2409, 2410, 2411, 2412, 2413, 2414, 2415, 2416, 2417,
    4000, 4001, 4002, 4003, 4004, 4005, 4006, 4007, 4008, 4009, 4010, 4011, 4012, 4013, 4014, 4015, 4016, 4017,
    5000, 5001, 5002, 5003, 5004, 5005, 5006, 5007, 5008, 5009, 5010, 5011, 5012, 5013, 5014, 5015, 5016, 5017,
    10004, 10009, 10012, 10019, 21004, 21005, 21012, 30001, 30002, 30003, 30004, 30005, 33001, 33002, 50000,
];

/// `models.reset_kind`: the hardcoded cadence.
pub fn reset_kind(uuid: i64) -> &'static str {
    if DAILY_RESET.contains(&uuid) {
        "daily"
    } else if WEEKLY_RESET.contains(&uuid) {
        "weekly"
    } else {
        "default"
    }
}

/// `pg_store._effective_reset_kind`.
pub fn effective_reset_kind(over: Option<&str>, uuid: i64) -> String {
    match over {
        Some(k @ ("daily" | "weekly" | "none")) => k.to_string(),
        _ => reset_kind(uuid).to_string(),
    }
}

pub fn is_player_board(uuid: i64) -> bool {
    !matches!(uuid, 1100 | 21012)
}

pub fn is_lifetime_kind(kind: &str) -> bool {
    matches!(kind, "default" | "none")
}

/// `_reset_boundary_before`: the last reset at or before `anchor` (0 = any).
pub fn reset_boundary_before(anchor: i64, kind: &str) -> i64 {
    let eleven = (anchor - RESET_HOUR).div_euclid(86400) * 86400 + RESET_HOUR;
    match kind {
        "daily" => eleven,
        "weekly" => eleven - ((eleven.div_euclid(86400) + 3).rem_euclid(7)) * 86400,
        _ => 0,
    }
}

/// One row of `list_boards_at`.
#[derive(Clone, Debug, PartialEq)]
pub struct Board {
    pub uuid: i64,
    pub name_id: String,
    pub name: String,
    pub category_id: String,
    pub category: String,
    pub contest_type: Option<String>,
    pub reset_kind: String,
}

impl Board {
    fn display_name(&self) -> String {
        if !self.name.is_empty() { self.name.clone() } else { self.uuid.to_string() }
    }

    fn display_category(&self) -> String {
        if !self.category.is_empty() { self.category.clone() } else { self.category_id.clone() }
    }

    fn contest(&self) -> Value {
        self.contest_type.clone().map_or(Value::Null, Value::String)
    }

    /// `_board_meta`.
    fn meta(&self) -> Map<String, Value> {
        let name = if !self.name.is_empty() {
            self.name.clone()
        } else if !self.name_id.is_empty() {
            self.name_id.clone()
        } else {
            self.uuid.to_string()
        };
        let mut m = Map::new();
        m.insert("uuid".into(), self.uuid.into());
        m.insert("name".into(), name.into());
        m.insert("category".into(), self.display_category().into());
        m.insert("reset_kind".into(), self.reset_kind.clone().into());
        m.insert("contest_type".into(), self.contest());
        m
    }
}

/// One `entries_by_board_at` row.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub player_name: String,
    pub rank: i64,
    pub score: f64,
}

fn obj<const N: usize>(pairs: [(&str, Value); N]) -> Map<String, Value> {
    pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect()
}

fn str_of(m: &Map<String, Value>, k: &str) -> String {
    m.get(k).and_then(Value::as_str).unwrap_or("").to_string()
}

/// The `(category.lower(), name.lower())` sort every board list uses.
fn sort_boards(boards: &mut [Map<String, Value>]) {
    boards.sort_by_cached_key(|b| (str_of(b, "category").to_lowercase(), str_of(b, "name").to_lowercase()));
}

fn sort_by_confidence(clusters: &mut [Map<String, Value>]) {
    let key = |c: &Map<String, Value>| {
        (
            c.get("confidence").and_then(Value::as_f64).unwrap_or(0.0),
            c.get("member_count").and_then(Value::as_i64).unwrap_or(0),
        )
    };
    clusters.sort_by(|a, b| {
        let ((ca, na), (cb, nb)) = (key(a), key(b));
        cb.partial_cmp(&ca).unwrap_or(std::cmp::Ordering::Equal).then(nb.cmp(&na))
    });
}

fn sample(names: &[String]) -> String {
    if names.len() <= 4 {
        names.join(", ")
    } else {
        format!("{}, \u{2026} {}", names[..3].join(", "), names[names.len() - 1])
    }
}

// ---------------------------------------------------------------------------
// Name stems

/// `_name_stem`, for any name (the ASCII kernel, else the Unicode rules).
pub fn name_stem(name: &str) -> String {
    if let Some(s) = detection::name_stem(name) {
        return s;
    }
    let lower: Vec<char> = pystr::strip(name).to_lowercase().chars().collect();
    let in_run = |c: char| decimal_value(c).is_some() || pystr::is_space(c) || matches!(c, '_' | '-' | '.' | '#');
    let mut start = lower.len();
    while start > 0 && in_run(lower[start - 1]) {
        start -= 1;
    }
    let cut = if lower[start..].iter().any(|c| decimal_value(*c).is_some()) { start } else { lower.len() };
    lower[..cut]
        .iter()
        .map(|&c| match c {
            '4' | '@' => 'a',
            '3' => 'e',
            '1' | '!' | '|' => 'i',
            '0' => 'o',
            '5' | '$' => 's',
            '7' => 't',
            '8' => 'b',
            '9' => 'g',
            c => c,
        })
        .collect()
}

/// `_dominant_stem`: the most common stem of 2+ chars and its count.
pub fn dominant_stem(names: &[String]) -> (String, usize) {
    let mut stems: Vec<(String, usize)> = Vec::new();
    let mut at: HashMap<String, usize> = HashMap::new();
    for nm in names {
        let st = name_stem(nm);
        if st.chars().count() >= 2 {
            match at.get(&st) {
                Some(&i) => stems[i].1 += 1,
                None => {
                    at.insert(st.clone(), stems.len());
                    stems.push((st, 1));
                }
            }
        }
    }
    let mut best: Option<&(String, usize)> = None;
    for s in &stems {
        if best.is_none_or(|b| s.1 > b.1) {
            best = Some(s);
        }
    }
    best.cloned().unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Per-player checks

/// `_detect_direction`: true when higher scores are better.
pub fn higher_is_better(entries: &[Entry]) -> bool {
    let rank1 = entries.iter().find(|e| e.rank == 1).map_or(entries[0].score, |e| e.score);
    let (mut hi, mut lo) = (entries[0].score, entries[0].score);
    for e in &entries[1..] {
        hi = py_max(hi, e.score);
        lo = py_min(lo, e.score);
    }
    if rank1 == hi && hi != lo {
        return true;
    }
    !(rank1 == lo && hi != lo)
}

fn ranked(entries: &[Entry]) -> Vec<&Entry> {
    let mut r: Vec<&Entry> = entries.iter().collect();
    r.sort_by_key(|e| e.rank);
    r
}

fn velocity(cur: f64, prev: f64, cur_anchor: i64, prev_anchor: i64, hib: bool) -> Option<f64> {
    let dt = (cur_anchor - prev_anchor) as f64 / 3600.0;
    if dt <= 0.0 {
        return None;
    }
    let ds = if hib { cur - prev } else { prev - cur };
    if ds <= 0.0 {
        return None;
    }
    Some(ds / dt)
}

/// The thresholds a pass runs with.
#[derive(Clone, Debug)]
pub struct Checks {
    pub z_threshold: f64,
    /// `str(z_threshold)`, for the score-outlier summary.
    pub z_repr: String,
    pub velocity_multiplier: f64,
    pub min_board_size: i64,
    pub cohort_pct: f64,
    pub excluded: BTreeSet<i64>,
}

struct FlagBoard {
    uuid: i64,
    name: String,
    category: String,
    contest_type: Value,
    rank: i64,
    score: Value,
    evidence: Vec<Map<String, Value>>,
}

/// What a board still needs for its velocity check: the previous captures of
/// `need` (in the board's reset cycle), then [`Detector::velocity`].
pub struct VelocityPlan {
    pub higher_is_better: bool,
    pub need: Vec<String>,
    flagged: Vec<String>,
}

/// The per-player half of `_compute`, fed one board at a time.
#[derive(Default)]
pub struct Detector {
    players: Vec<(String, Vec<FlagBoard>)>,
    index: HashMap<String, usize>,
    pub boards_analyzed: i64,
    pub boards_excluded: i64,
    analyzed: Vec<Map<String, Value>>,
    skipped: Vec<Map<String, Value>>,
}

impl Detector {
    pub fn new() -> Self {
        Self::default()
    }

    /// `_add_evidence`.
    fn add(&mut self, player: &str, board: &Board, entry: &Entry, evidence: Map<String, Value>) {
        let i = match self.index.get(player) {
            Some(&i) => i,
            None => {
                self.index.insert(player.to_string(), self.players.len());
                self.players.push((player.to_string(), Vec::new()));
                self.players.len() - 1
            }
        };
        let boards = &mut self.players[i].1;
        let b = match boards.iter().position(|b| b.uuid == board.uuid) {
            Some(j) => &mut boards[j],
            None => {
                boards.push(FlagBoard {
                    uuid: board.uuid,
                    name: board.name.clone(),
                    category: board.category.clone(),
                    contest_type: board.contest(),
                    rank: entry.rank,
                    score: round_val(entry.score),
                    evidence: Vec::new(),
                });
                boards.last_mut().expect("just pushed")
            }
        };
        b.evidence.push(evidence);
    }

    fn flagged_on(&self, uuid: i64) -> Vec<String> {
        self.players.iter().filter(|(_, bs)| bs.iter().any(|b| b.uuid == uuid)).map(|(n, _)| n.clone()).collect()
    }

    /// One board of the per-player loop: bookkeeping, score-outlier and
    /// rank-gap; `Some` when the velocity check has players to look at.
    pub fn scan(&mut self, board: &Board, entries: &[Entry], c: &Checks) -> Option<VelocityPlan> {
        if c.excluded.contains(&board.uuid) {
            self.boards_excluded += 1;
            let mut m = board.meta();
            m.insert("reason".into(), "admin_excluded".into());
            self.skipped.push(m);
            return None;
        }
        if (entries.len() as i64) < c.min_board_size {
            let mut m = board.meta();
            m.insert("reason".into(), "below_min_size".into());
            m.insert("entries".into(), entries.len().into());
            self.skipped.push(m);
            return None;
        }
        self.boards_analyzed += 1;
        let mut m = board.meta();
        m.insert("entries".into(), entries.len().into());
        self.analyzed.push(m);
        if entries.is_empty() {
            return None;
        }

        let hib = higher_is_better(entries);
        if !is_lifetime_kind(&board.reset_kind) {
            self.score_outliers(board, entries, c, hib);
            self.rank_gaps(board, entries);
        }
        let flagged = self.flagged_on(board.uuid);
        if flagged.is_empty() {
            return None;
        }
        let mut need: Vec<String> =
            ranked(entries).iter().take(VELOCITY_PEER_TOP_N).map(|e| e.player_name.clone()).collect();
        need.extend(flagged.iter().cloned());
        need.sort();
        need.dedup();
        Some(VelocityPlan { higher_is_better: hib, need, flagged })
    }

    /// `_score_outlier_check`.
    fn score_outliers(&mut self, board: &Board, entries: &[Entry], c: &Checks, hib: bool) {
        let ranked = ranked(entries);
        let n = ranked.len();
        let cohort_size = (50i64.max((n as f64 * c.cohort_pct) as i64) as usize).min(n);
        let cohort = &ranked[..cohort_size];
        let scores: Vec<f64> = cohort.iter().map(|e| e.score).collect();
        if scores.iter().take(5).all(|s| *s == scores[0]) {
            return;
        }
        let log = |s: f64| if s >= 0.0 { (s + 1.0).log10() } else { 0.0 };
        let logs: Vec<f64> = scores.iter().map(|&s| log(s)).collect();
        let log_median = median(&logs);
        let devs: Vec<f64> = logs.iter().map(|x| (x - log_median).abs()).collect();
        let log_mad = median(&devs);
        if log_mad == 0.0 {
            return;
        }
        let med = median(&scores);
        let check_n = 10usize.max(cohort_size / 2);
        for e in cohort.iter().take(check_n) {
            let mz = 0.6745 * (log(e.score) - log_median) / log_mad;
            let outlier = if hib { mz > c.z_threshold } else { mz < -c.z_threshold };
            if !outlier {
                continue;
            }
            let ratio = if med > 0.0 { e.score / med } else { f64::INFINITY };
            let summary = format!(
                "Score {} is {}\u{d7} the TOP-{cohort_size} median of {} on this {n}-player board ({} robust \
                 log-z-scores {}, threshold: {}). Log-space MAD-Z on the elite cohort resists both cheater \
                 self-pollution and the natural heavy-tail of leaderboard shapes.",
                fmt_num(e.score),
                fixed(ratio, 1),
                fmt_num(med),
                fixed(mz.abs(), 1),
                if hib { "above" } else { "below" },
                c.z_repr,
            );
            let measurements = obj([
                ("player_score", round_val(e.score)),
                ("elite_median", round_val(med)),
                ("modified_z_score", fv(py_round(mz, 2))),
                ("threshold", fv(c.z_threshold)),
                ("median_multiplier", fv(py_round(ratio, 2))),
                ("higher_is_better", hib.into()),
                ("cohort_size", cohort_size.into()),
                ("board_size", n.into()),
            ]);
            let ev = obj([
                ("type", "score_outlier".into()),
                ("summary", summary.into()),
                ("measurements", Value::Object(measurements)),
            ]);
            self.add(&e.player_name, board, e, ev);
        }
    }

    /// `_rank_gap_check`.
    fn rank_gaps(&mut self, board: &Board, entries: &[Entry]) {
        let ranked = ranked(entries);
        if ranked.len() < 4 {
            return;
        }
        let gaps: Vec<f64> = ranked
            .windows(2)
            .map(|w| {
                let (a, b) = (w[0].score, w[1].score);
                let bigger = py_max(a.abs(), b.abs());
                if bigger > 0.0 { (a - b).abs() / bigger } else { 0.0 }
            })
            .collect();
        let tail = if gaps.len() > 3 { &gaps[3..] } else { &gaps[..] };
        let typical = median(tail);
        if typical <= 0.0 {
            return;
        }
        for i in 0..gaps.len().min(3) {
            if gaps[i] <= 0.0 {
                continue;
            }
            let multiplier = gaps[i] / typical;
            if multiplier < RANK_GAP_THRESHOLD {
                continue;
            }
            let (e, nxt) = (ranked[i], ranked[i + 1]);
            let summary = format!(
                "Rank-{} score {} is {}\u{d7} the typical between-rank gap on this board ({}% vs typical {}%). \
                 Next-rank score: {}.",
                e.rank,
                fmt_num(e.score),
                fixed(multiplier, 0),
                fixed(gaps[i] * 100.0, 0),
                fixed(typical * 100.0, 1),
                fmt_num(nxt.score),
            );
            let measurements = obj([
                ("player_rank", e.rank.into()),
                ("player_score", round_val(e.score)),
                ("next_rank", nxt.rank.into()),
                ("next_rank_score", round_val(nxt.score)),
                ("gap_fraction", fv(py_round(gaps[i], 4))),
                ("typical_gap_fraction", fv(py_round(typical, 4))),
                ("gap_multiplier", fv(py_round(multiplier, 1))),
                ("threshold_multiplier", fv(RANK_GAP_THRESHOLD)),
            ]);
            let ev = obj([
                ("type", "rank_gap".into()),
                ("summary", summary.into()),
                ("measurements", Value::Object(measurements)),
            ]);
            self.add(&e.player_name, board, e, ev);
        }
    }

    /// `_velocity_check` once `prev` holds the plan's previous captures
    /// (`{name: (score, anchor)}`).
    pub fn velocity(
        &mut self,
        board: &Board,
        entries: &[Entry],
        anchor: i64,
        multiplier: f64,
        plan: &VelocityPlan,
        prev: &HashMap<String, (f64, i64)>,
    ) {
        let hib = plan.higher_is_better;
        let ranked = ranked(entries);
        let mut peers: Vec<f64> = ranked
            .iter()
            .take(VELOCITY_PEER_TOP_N)
            .filter_map(|e| {
                let (ps, pa) = prev.get(&e.player_name)?;
                velocity(e.score, *ps, anchor, *pa, hib).filter(|v| *v > 0.0)
            })
            .collect();
        if peers.len() < 5 {
            return;
        }
        sort_floats(&mut peers);
        let idx = ((0.95 * peers.len() as f64) as i64 - 1).max(0) as usize;
        let p95 = peers[idx];
        if p95 <= 0.0 {
            return;
        }
        for name in &plan.flagged {
            let Some(e) = ranked.iter().find(|x| &x.player_name == name) else { continue };
            let Some(&(prev_score, prev_anchor)) = prev.get(name) else { continue };
            let Some(v) = velocity(e.score, prev_score, anchor, prev_anchor, hib) else { continue };
            if v < p95 * multiplier {
                continue;
            }
            let dt = (anchor - prev_anchor) as f64 / 3600.0;
            let ds = if hib { e.score - prev_score } else { prev_score - e.score };
            let summary = format!(
                "Score gained {} in {}h (rate {}/h). This board's peer p95 rate is {}/h - this player is {}\u{d7} faster.",
                fmt_num(ds),
                fixed(dt, 1),
                fmt_num(v),
                fmt_num(p95),
                fixed(v / p95, 0),
            );
            let measurements = obj([
                ("score_delta", round_val(ds)),
                ("duration_hours", fv(py_round(dt, 2))),
                ("rate_per_hour", round_val(v)),
                ("peer_p95_rate_per_hour", round_val(p95)),
                ("rate_multiplier", fv(py_round(v / p95, 1))),
                ("threshold_multiplier", fv(multiplier)),
                ("previous_anchor", prev_anchor.into()),
                ("previous_score", round_val(prev_score)),
            ]);
            let ev = obj([
                ("type", "velocity_outlier".into()),
                ("summary", summary.into()),
                ("measurements", Value::Object(measurements)),
            ]);
            self.add(name, board, e, ev);
        }
    }

    /// Folds the weekly-uptime flags into the per-player evidence.
    pub fn sustained(&mut self, flags: &[PlayerFlag], boards: &[Board], by_board: &HashMap<i64, Vec<Entry>>) {
        for pf in flags {
            let Some(board) = boards.iter().find(|b| b.uuid == pf.board_uuid) else { continue };
            let Some(entry) =
                by_board.get(&pf.board_uuid).and_then(|es| es.iter().find(|e| e.player_name == pf.player_name))
            else {
                continue;
            };
            let summary = format!(
                "Score rose in {} of the last {} hourly captures since the weekly reset ({}% uptime). No human plays \
                 {}%+ of every hour for days - this account essentially never stops, the signature of a no-sleep bot. \
                 Invisible to the per-hour check (each hour looks normal alone).",
                pf.active_hours,
                pf.num_slots,
                fixed(pf.active_frac * 100.0, 0),
                fixed(pf.threshold_frac * 100.0, 0),
            );
            let measurements = obj([
                ("active_hours", pf.active_hours.into()),
                ("captures_since_reset", pf.num_slots.into()),
                ("uptime_fraction", fv(py_round(pf.active_frac, 3))),
                ("threshold_fraction", fv(pf.threshold_frac)),
            ]);
            let ev = obj([
                ("type", "sustained_velocity".into()),
                ("summary", summary.into()),
                ("measurements", Value::Object(measurements)),
            ]);
            self.add(&pf.player_name, board, entry, ev);
        }
    }

    /// `_format` of this pass.
    pub fn finish(self, head: &Head, clusters: Vec<Map<String, Value>>, clusters_boards_scanned: i64) -> Value {
        let mut players: Vec<(Map<String, Value>, f64, usize)> = Vec::new();
        for (name, boards) in self.players {
            let mut out_boards = Vec::with_capacity(boards.len());
            let mut confs: Vec<(Vec<f64>, Vec<String>)> = Vec::new();
            let mut evidence_count = 0;
            for b in boards {
                let mut evs = Vec::with_capacity(b.evidence.len());
                let mut cs = Vec::new();
                let mut types = Vec::new();
                for mut ev in b.evidence {
                    let conf = evidence_confidence(&ev);
                    ev.insert("confidence".into(), fv(conf));
                    cs.push(conf);
                    if let Some(t) = ev.get("type").and_then(Value::as_str).filter(|t| !t.is_empty()) {
                        types.push(t.to_string());
                    }
                    evs.push(Value::Object(ev));
                }
                evidence_count += evs.len();
                let board_conf = cs.iter().copied().fold(None, |m: Option<f64>, c| Some(m.map_or(c, |m| py_max(m, c))));
                let m = obj([
                    ("uuid", b.uuid.into()),
                    ("name", b.name.into()),
                    ("category", b.category.into()),
                    ("contest_type", b.contest_type),
                    ("rank", b.rank.into()),
                    ("score", b.score),
                    ("evidence", Value::Array(evs)),
                    ("confidence", fv(board_conf.unwrap_or(0.0))),
                ]);
                out_boards.push(Value::Object(m));
                confs.push((cs, types));
            }
            let conf = player_confidence(&confs);
            let p = obj([
                ("player_name", name.into()),
                ("leaderboards", Value::Array(out_boards)),
                ("confidence", fv(conf)),
            ]);
            players.push((p, conf, evidence_count));
        }
        players.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal).then(b.2.cmp(&a.2)));
        let players: Vec<Value> = players.into_iter().map(|(p, _, _)| Value::Object(p)).collect();
        let (mut analyzed, mut skipped) = (self.analyzed, self.skipped);
        sort_boards(&mut analyzed);
        sort_boards(&mut skipped);
        payload(
            head,
            players,
            clusters,
            clusters_boards_scanned,
            self.boards_analyzed,
            self.boards_excluded,
            analyzed,
            skipped,
        )
    }
}

/// The fields every payload shares.
#[derive(Clone, Debug)]
pub struct Head {
    pub anchor: Option<i64>,
    pub z_threshold: f64,
    pub velocity_multiplier: f64,
    pub min_board_size: i64,
    pub cheater_detection_enabled: bool,
    pub alt_clusters_enabled: bool,
    pub computed_at: i64,
}

#[allow(clippy::too_many_arguments)]
fn payload(
    h: &Head,
    players: Vec<Value>,
    clusters: Vec<Map<String, Value>>,
    clusters_boards_scanned: i64,
    boards_analyzed: i64,
    boards_excluded: i64,
    analyzed: Vec<Map<String, Value>>,
    skipped: Vec<Map<String, Value>>,
) -> Value {
    let total = players.len();
    let config = obj([
        ("z_threshold", fv(h.z_threshold)),
        ("velocity_multiplier", fv(h.velocity_multiplier)),
        ("min_board_size", h.min_board_size.into()),
    ]);
    let objs = |v: Vec<Map<String, Value>>| Value::Array(v.into_iter().map(Value::Object).collect());
    Value::Object(obj([
        ("players", Value::Array(players)),
        ("clusters", objs(clusters)),
        ("clusters_boards_scanned", clusters_boards_scanned.into()),
        ("cheater_detection_enabled", h.cheater_detection_enabled.into()),
        ("alt_clusters_enabled", h.alt_clusters_enabled.into()),
        ("computed_at", h.computed_at.into()),
        ("anchor", h.anchor.map_or(Value::Null, Value::from)),
        ("method", METHOD.into()),
        ("config", Value::Object(config)),
        ("total_flagged", total.into()),
        ("boards_analyzed", boards_analyzed.into()),
        ("boards_excluded", boards_excluded.into()),
        ("analyzed_boards", objs(analyzed)),
        ("excluded_boards", objs(skipped)),
    ]))
}

/// `_empty` / `_disabled_payload` (the head carries the two switches).
pub fn empty(h: &Head) -> Value {
    payload(h, Vec::new(), Vec::new(), 0, 0, 0, Vec::new(), Vec::new())
}

// ---------------------------------------------------------------------------
// Confidence

fn ceiling(kind: &str) -> f64 {
    match kind {
        "velocity_outlier" => 0.99,
        "sustained_velocity" => 0.95,
        "rank_gap" => 0.85,
        "score_outlier" => 0.60,
        _ => 0.99,
    }
}

fn m_f64(m: Option<&Map<String, Value>>, k: &str, default: f64) -> f64 {
    m.and_then(|m| m.get(k)).and_then(|v| v.as_f64().or_else(|| v.as_bool().map(f64::from))).unwrap_or(default)
}

/// `_evidence_confidence`.
pub fn evidence_confidence(ev: &Map<String, Value>) -> f64 {
    let m = ev.get("measurements").and_then(Value::as_object);
    let kind = ev.get("type").and_then(Value::as_str).unwrap_or("");
    if kind == "sustained_velocity" {
        let frac = m_f64(m, "uptime_fraction", 0.0);
        let thr = m_f64(m, "threshold_fraction", 0.85);
        if thr >= 1.0 || frac < thr {
            return 0.5;
        }
        let raw = 0.7 + 0.3 * ((frac - thr) / (1.0 - thr));
        return py_round(py_min(ceiling(kind), raw), 3);
    }
    let (threshold, magnitude) = match kind {
        "score_outlier" => (m_f64(m, "threshold", 5.0), m_f64(m, "modified_z_score", 0.0).abs()),
        "rank_gap" => (m_f64(m, "threshold_multiplier", 10.0), m_f64(m, "gap_multiplier", 0.0)),
        "velocity_outlier" => (m_f64(m, "threshold_multiplier", 10.0), m_f64(m, "rate_multiplier", 0.0)),
        _ => return 0.5,
    };
    if threshold <= 0.0 || magnitude < threshold {
        return 0.5;
    }
    let ratio = magnitude / threshold;
    let raw = 0.5 + 0.5 * (1.0 - (-(ratio - 1.0)).exp());
    py_round(py_min(ceiling(kind), raw), 3)
}

/// `_player_confidence` over each board's (evidence confidences, types).
fn player_confidence(boards: &[(Vec<f64>, Vec<String>)]) -> f64 {
    let mut inv = 1.0;
    let mut saw = false;
    let mut types: BTreeSet<&str> = BTreeSet::new();
    for (confs, ts) in boards {
        if confs.is_empty() {
            continue;
        }
        types.extend(ts.iter().map(String::as_str));
        let board_conf = confs[1..].iter().fold(confs[0], |m, c| py_max(m, *c));
        inv *= 1.0 - board_conf;
        saw = true;
    }
    if !saw {
        return 0.0;
    }
    let mut raw = 1.0 - inv;
    if types.len() == 1 {
        raw = py_min(raw, ceiling(types.iter().next().copied().unwrap_or("")));
    }
    py_round(raw, 3)
}

// ---------------------------------------------------------------------------
// Name-stem clusters

#[derive(Clone, Debug)]
pub struct ClusterParams {
    pub min_size: i64,
    pub band_pct: f64,
    pub max_edit: i64,
    pub size_full: i64,
}

fn spread_of(lo: f64, hi: f64) -> f64 {
    (hi - lo) / py_max(py_max(hi.abs(), lo.abs()), 1e-9)
}

/// One name family: canonical stem, every member, its per-board occurrences.
type Family = (String, BTreeSet<String>, Vec<Map<String, Value>>);

/// `_detect_clusters`: the name-stem clusters and how many boards were scanned.
pub fn detect_clusters(
    boards: &[Board],
    by_board: &HashMap<i64, Vec<Entry>>,
    excluded: &BTreeSet<i64>,
    p: &ClusterParams,
) -> (Vec<Map<String, Value>>, i64) {
    let mut family: Vec<Family> = Vec::new();
    let mut at: HashMap<String, usize> = HashMap::new();
    let mut scanned = 0;
    let empty = Vec::new();
    let min_size = p.min_size.max(0) as usize;
    for board in boards {
        if excluded.contains(&board.uuid) {
            continue;
        }
        let entries = by_board.get(&board.uuid).unwrap_or(&empty);
        if (entries.len() as i64) < p.min_size {
            continue;
        }
        scanned += 1;
        let stems: Vec<String> = entries.iter().map(|e| name_stem(&e.player_name)).collect();
        let rows: Vec<detection::Entry> = entries
            .iter()
            .zip(&stems)
            .map(|(e, s)| detection::Entry { name: &e.player_name, stem: s, score: e.score, rank: e.rank })
            .collect();
        for (cstem, idxs) in detection::board_families(&rows, min_size, p.band_pct, p.max_edit) {
            let subset: Vec<&Entry> = idxs.iter().map(|&i| &entries[i]).collect();
            let (mut lo, mut hi) = (subset[0].score, subset[0].score);
            for e in &subset[1..] {
                lo = py_min(lo, e.score);
                hi = py_max(hi, e.score);
            }
            let names: BTreeSet<String> = subset.iter().map(|e| e.player_name.clone()).collect();
            let i = *at.entry(cstem.clone()).or_insert_with(|| {
                family.push((cstem.clone(), BTreeSet::new(), Vec::new()));
                family.len() - 1
            });
            family[i].1.extend(names.iter().cloned());
            family[i].2.push(obj([
                ("uuid", board.uuid.into()),
                ("name", board.display_name().into()),
                ("category", board.display_category().into()),
                ("contest_type", board.contest()),
                ("members", subset.len().into()),
                ("member_names", names.into_iter().take(CAP).collect::<Vec<_>>().into()),
                ("score_min", round_val(lo)),
                ("score_max", round_val(hi)),
                ("spread", fv(py_round(spread_of(lo, hi), 6))),
                ("rank_min", subset.iter().map(|e| e.rank).min().unwrap_or(0).into()),
                ("rank_max", subset.iter().map(|e| e.rank).max().unwrap_or(0).into()),
            ]));
        }
    }
    let mut clusters = Vec::new();
    for (cstem, members, mut occ) in family {
        let member_count = members.len();
        if (member_count as i64) < p.min_size || occ.is_empty() {
            continue;
        }
        let (conf, terms) = cluster_confidence(&occ, member_count, p);
        let names: Vec<String> = members.into_iter().collect();
        let tightest = terms.get("tightest_spread_pct").and_then(Value::as_f64).unwrap_or(0.0);
        let summary = format!(
            "{member_count} similarly-named accounts ({}) cluster within {}% of each other on {} board(s). \
             Coordinated multi-account ('alt army') pattern: near-identical scores under a shared name stem '{cstem}'.",
            sample(&names),
            g3(tightest),
            occ.len(),
        );
        let board_count = occ.len();
        sort_boards(&mut occ);
        clusters.push(obj([
            ("stem", cstem.clone().into()),
            ("label", format!("{cstem}*").into()),
            ("method", "name_stem".into()),
            ("member_count", member_count.into()),
            ("members", names.iter().take(CAP).cloned().collect::<Vec<_>>().into()),
            ("members_truncated", member_count.saturating_sub(CAP).into()),
            ("board_count", board_count.into()),
            ("boards", Value::Array(occ.into_iter().map(Value::Object).collect())),
            ("confidence", fv(conf)),
            ("summary", summary.into()),
            ("measurements", Value::Object(terms)),
        ]));
    }
    sort_by_confidence(&mut clusters);
    (clusters, scanned)
}

/// `_cluster_confidence`.
fn cluster_confidence(occ: &[Map<String, Value>], member_count: usize, p: &ClusterParams) -> (f64, Map<String, Value>) {
    let spreads: Vec<f64> = occ.iter().filter_map(|b| b.get("spread").and_then(Value::as_f64)).collect();
    let tightest = spreads.iter().copied().reduce(py_min).unwrap_or(p.band_pct);
    let closeness = if p.band_pct > 0.0 { py_max(0.0, 1.0 - (tightest / p.band_pct)) } else { 0.0 };
    let denom = (p.size_full - p.min_size).max(1);
    let size_term = py_min(1.0, py_max(0.0, (member_count as i64 - p.min_size) as f64 / denom as f64));
    let board_term = 1.0 - 0.5f64.powf(occ.len() as f64);
    let raw = 0.5 + 0.49 * (0.50 * closeness + 0.25 * size_term + 0.25 * board_term);
    let conf = py_round(py_min(ALT_CEILING, raw), 3);
    let terms = obj([
        ("member_count", member_count.into()),
        ("board_count", occ.len().into()),
        ("tightest_spread_pct", fv(py_round(tightest * 100.0, 4))),
        ("score_band_pct", fv(py_round(p.band_pct * 100.0, 4))),
        ("closeness", fv(py_round(closeness, 3))),
        ("size_term", fv(py_round(size_term, 3))),
        ("board_term", fv(py_round(board_term, 3))),
        ("ceiling", fv(ALT_CEILING)),
    ]);
    (conf, terms)
}

// ---------------------------------------------------------------------------
// History signals: co-movement, schedule, weekly uptime

/// The `cheaters_comovement_*` / schedule / fusion knobs.
#[derive(Clone, Debug, PartialEq)]
pub struct CmParams {
    pub candidate_top_n: i64,
    pub min_hourly_gain: f64,
    pub gain_percentile: f64,
    pub gain_tolerance: f64,
    pub min_matching_hours: i64,
    pub min_match_ratio: f64,
    pub min_density: f64,
    pub min_group_size: i64,
    pub max_cell_accounts: i64,
    pub recompute_seconds: i64,
    pub schedule_min_active_hours: i64,
    pub schedule_min_similarity: f64,
    pub fusion_corroboration_bonus: f64,
    pub footprint_min_jaccard: f64,
    pub weekly_uptime_fraction: f64,
}

/// One account's capture series on a board: `(name, [(anchor, score)])`.
pub type Series = Vec<(String, Vec<(i64, f64)>)>;

#[derive(Clone, Debug, PartialEq)]
pub struct PlayerFlag {
    pub player_name: String,
    pub board_uuid: i64,
    pub active_hours: usize,
    pub num_slots: usize,
    pub active_frac: f64,
    pub threshold_frac: f64,
}

/// An insertion-ordered `{name: set}`.
#[derive(Clone, Debug, Default)]
pub struct NameSets<T: Ord> {
    pub items: Vec<(String, BTreeSet<T>)>,
    at: HashMap<String, usize>,
}

impl<T: Ord> NameSets<T> {
    fn from_items(items: Vec<(String, BTreeSet<T>)>) -> Self {
        let at = items.iter().enumerate().map(|(i, (n, _))| (n.clone(), i)).collect();
        NameSets { items, at }
    }

    pub fn get(&self, name: &str) -> Option<&BTreeSet<T>> {
        self.at.get(name).map(|&i| &self.items[i].1)
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

/// `_cluster_history_signals`' output.
#[derive(Clone, Debug, Default)]
pub struct Signals {
    pub comovement: Vec<Map<String, Value>>,
    pub schedule: Vec<Map<String, Value>>,
    pub active_hours: NameSets<usize>,
    pub board_sets: NameSets<i64>,
    pub player_flags: Vec<PlayerFlag>,
}

fn jaccard<T: Ord>(a: Option<&BTreeSet<T>>, b: Option<&BTreeSet<T>>) -> f64 {
    let (la, lb) = (a.map_or(0, BTreeSet::len), b.map_or(0, BTreeSet::len));
    if la == 0 && lb == 0 {
        return 0.0;
    }
    let inter = match (a, b) {
        (Some(a), Some(b)) => a.intersection(b).count(),
        _ => 0,
    };
    inter as f64 / (la + lb - inter) as f64
}

fn pair_mean<T: Ord>(names: &[String], sets: &NameSets<T>) -> Option<f64> {
    let mut sims = Vec::new();
    for (i, a) in names.iter().enumerate() {
        for b in &names[i + 1..] {
            sims.push(jaccard(sets.get(a), sets.get(b)));
        }
    }
    if sims.is_empty() { None } else { Some(py_sum(sims.iter().copied()) / sims.len() as f64) }
}

/// The CPU half of `_comovement_clusters` (the native `det_history` path).
pub fn history_signals(
    series_by_board: &[(i64, Series)],
    meta: &HashMap<i64, Board>,
    anchors: &[i64],
    cm: &CmParams,
) -> Signals {
    let slot_of: HashMap<i64, usize> = anchors.iter().enumerate().map(|(i, a)| (*a, i)).collect();
    let boards: Vec<detection::Board> = series_by_board
        .iter()
        .map(|(uuid, series)| {
            let names = series
                .iter()
                .map(|(name, pts)| {
                    let mut p: Vec<(usize, f64)> =
                        pts.iter().filter_map(|(a, v)| slot_of.get(a).map(|s| (*s, *v))).collect();
                    p.sort_by_key(|x| x.0);
                    (name.clone(), p)
                })
                .collect();
            (*uuid, names)
        })
        .collect();
    let params = detection::Params {
        min_gain: cm.min_hourly_gain,
        pct: cm.gain_percentile,
        tol: cm.gain_tolerance,
        min_match: cm.min_matching_hours,
        min_ratio: cm.min_match_ratio,
        min_density: cm.min_density,
        min_group: cm.min_group_size.max(0) as usize,
        max_cell: cm.max_cell_accounts.max(0) as usize,
    };
    let (hours, on) = detection::history_features(&boards, cm.min_hourly_gain);
    let groups = detection::comovement_groups(&boards, &params);
    let sched = if cm.schedule_min_active_hours > 0 {
        detection::schedule_groups(
            &hours,
            cm.schedule_min_active_hours as usize,
            cm.schedule_min_similarity,
            params.min_group,
            cm.min_density,
            params.max_cell,
            anchors.len(),
        )
    } else {
        Vec::new()
    };
    let active_hours = NameSets::from_items(hours);
    let board_sets = NameSets::from_items(on.into_iter().map(|(n, u)| (n, u.into_iter().collect())).collect());
    let comovement = merge_board_groups(&groups, meta, cm.min_group_size, cm.min_matching_hours);
    let schedule = if cm.schedule_min_active_hours > 0 {
        sched.iter().map(|mem| schedule_cluster(mem, &active_hours, &board_sets, meta)).collect()
    } else {
        Vec::new()
    };
    let player_flags =
        weekly_uptime_flags(&active_hours, &board_sets, anchors.len().saturating_sub(1), cm.weekly_uptime_fraction);
    Signals { comovement, schedule, active_hours, board_sets, player_flags }
}

/// `_weekly_uptime_flags`.
fn weekly_uptime_flags(
    active: &NameSets<usize>,
    boards: &NameSets<i64>,
    num_slots: usize,
    uptime_frac: f64,
) -> Vec<PlayerFlag> {
    if uptime_frac <= 0.0 || num_slots < WEEKLY_UPTIME_MIN_SLOTS {
        return Vec::new();
    }
    let mut out = Vec::new();
    for (name, hrs) in &active.items {
        let frac = hrs.len() as f64 / num_slots as f64;
        if frac < uptime_frac {
            continue;
        }
        let Some(&board_uuid) = boards.get(name).and_then(|s| s.iter().next()) else { continue };
        out.push(PlayerFlag {
            player_name: name.clone(),
            board_uuid,
            active_hours: hrs.len(),
            num_slots,
            active_frac: frac,
            threshold_frac: uptime_frac,
        });
    }
    out
}

/// `_merge_board_groups`.
fn merge_board_groups(
    groups: &[detection::Group],
    meta: &HashMap<i64, Board>,
    min_group: i64,
    min_match: i64,
) -> Vec<Map<String, Value>> {
    if groups.is_empty() {
        return Vec::new();
    }
    let mut parent: Vec<usize> = (0..groups.len()).collect();
    fn find(p: &mut [usize], mut x: usize) -> usize {
        while p[x] != x {
            p[x] = p[p[x]];
            x = p[x];
        }
        x
    }
    let mut by_member: HashMap<&str, usize> = HashMap::new();
    for (idx, (_, members, _, _)) in groups.iter().enumerate() {
        for m in members {
            match by_member.get(m.as_str()) {
                Some(&other) => {
                    let (ra, rb) = (find(&mut parent, idx), find(&mut parent, other));
                    if ra != rb {
                        parent[ra.max(rb)] = ra.min(rb);
                    }
                }
                None => {
                    by_member.insert(m, idx);
                }
            }
        }
    }
    let mut merged: Vec<(usize, Vec<usize>)> = Vec::new();
    for idx in 0..groups.len() {
        let root = find(&mut parent, idx);
        match merged.iter_mut().find(|(r, _)| *r == root) {
            Some((_, v)) => v.push(idx),
            None => merged.push((root, vec![idx])),
        }
    }
    let mut clusters = Vec::new();
    for (_, idxs) in merged {
        let mut members: BTreeSet<String> = BTreeSet::new();
        let mut boards_info = Vec::new();
        let mut max_match = 0usize;
        let mut gains = Vec::new();
        for gi in idxs {
            let (uuid, mem, mh, gain) = &groups[gi];
            members.extend(mem.iter().cloned());
            max_match = max_match.max(*mh);
            gains.push(*gain);
            let bm = &meta[uuid];
            boards_info.push(obj([
                ("uuid", (*uuid).into()),
                ("name", bm.display_name().into()),
                ("category", bm.display_category().into()),
                ("contest_type", bm.contest()),
                ("members", mem.len().into()),
                ("member_names", mem.iter().take(CAP).cloned().collect::<Vec<_>>().into()),
                ("matching_hours", (*mh).into()),
                ("avg_hourly_gain", round_val(*gain)),
            ]));
        }
        if (members.len() as i64) < min_group {
            continue;
        }
        let avg_gain = if gains.is_empty() { 0.0 } else { py_sum(gains.iter().copied()) / gains.len() as f64 };
        clusters.push(comovement_cluster(members, boards_info, max_match, avg_gain, min_match));
    }
    sort_by_confidence(&mut clusters);
    clusters
}

/// `_build_comovement_cluster`.
fn comovement_cluster(
    members: BTreeSet<String>,
    mut boards_info: Vec<Map<String, Value>>,
    matching_hours: usize,
    avg_gain: f64,
    min_match: i64,
) -> Map<String, Value> {
    let names: Vec<String> = members.into_iter().collect();
    let n = names.len();
    let (top_stem, top_count) = dominant_stem(&names);
    let corroborated = top_count >= 2usize.max(n / 2);
    let mh = matching_hours as i64;
    let matching_term = if mh >= min_match { 1.0 - 0.6f64.powf((mh - min_match + 1) as f64) } else { 0.0 };
    let size_term = 1.0 - 0.5f64.powf((n as i64 - 1) as f64);
    let mut raw = 0.5 + 0.47 * (0.6 * matching_term + 0.4 * size_term);
    if corroborated {
        raw += 0.08;
    }
    let conf = py_round(py_min(COMOVEMENT_CEILING, raw), 3);
    let board_count = boards_info.len();
    let summary = format!(
        "{n} accounts gained in LOCKSTEP - matching hourly score deltas across {matching_hours} hour(s) since the \
         weekly reset on {board_count} board(s) ({}). Avg matched gain ~{}/hr. Coordinated alts/bots progress \
         together regardless of name{}",
        sample(&names),
        fmt_num(avg_gain),
        if corroborated { format!("; this group also shares the name stem '{top_stem}'.") } else { ".".into() },
    );
    sort_boards(&mut boards_info);
    obj([
        ("stem", if corroborated { top_stem.clone() } else { String::new() }.into()),
        ("label", if corroborated { format!("{top_stem}*") } else { format!("{} +{}", names[0], n - 1) }.into()),
        ("method", if corroborated { "both" } else { "co_movement" }.into()),
        ("member_count", n.into()),
        ("members", names.iter().take(CAP).cloned().collect::<Vec<_>>().into()),
        ("members_truncated", n.saturating_sub(CAP).into()),
        ("board_count", board_count.into()),
        ("boards", Value::Array(boards_info.into_iter().map(Value::Object).collect())),
        ("confidence", fv(conf)),
        ("summary", summary.into()),
        (
            "measurements",
            Value::Object(obj([
                ("matching_hours", matching_hours.into()),
                ("group_size", n.into()),
                ("board_count", board_count.into()),
                ("avg_hourly_gain", round_val(avg_gain)),
                ("name_corroborated", corroborated.into()),
                ("name_stem", if corroborated { top_stem.into() } else { Value::Null }),
                ("matching_term", fv(py_round(matching_term, 3))),
                ("size_term", fv(py_round(size_term, 3))),
                ("ceiling", fv(COMOVEMENT_CEILING)),
            ])),
        ),
    ])
}

/// `_build_schedule_cluster` (`names` sorted).
fn schedule_cluster(
    names: &[String],
    active: &NameSets<usize>,
    boards: &NameSets<i64>,
    meta: &HashMap<i64, Board>,
) -> Map<String, Value> {
    let n = names.len();
    let cohesion = pair_mean(names, active).unwrap_or(0.0);
    let total_hours: usize = names.iter().map(|m| active.get(m).map_or(0, BTreeSet::len)).sum();
    let avg_hours = total_hours as f64 / n as f64;
    let mut board_members: Vec<(i64, Vec<String>)> = Vec::new();
    for m in names {
        for u in boards.get(m).into_iter().flatten() {
            match board_members.iter_mut().find(|(x, _)| x == u) {
                Some((_, v)) => v.push(m.clone()),
                None => board_members.push((*u, vec![m.clone()])),
            }
        }
    }
    let mut boards_info: Vec<Map<String, Value>> = board_members
        .into_iter()
        .map(|(u, mut ms)| {
            let bm = meta.get(&u);
            ms.sort();
            obj([
                ("uuid", u.into()),
                ("name", bm.filter(|b| !b.name.is_empty()).map_or(u.to_string(), |b| b.name.clone()).into()),
                ("category", bm.map_or(String::new(), |b| b.category.clone()).into()),
                ("contest_type", bm.map_or(Value::Null, Board::contest)),
                ("members", ms.len().into()),
                ("member_names", ms.into_iter().take(CAP).collect::<Vec<_>>().into()),
            ])
        })
        .collect();
    let size_term = 1.0 - 0.5f64.powf((n as i64 - 1) as f64);
    let raw = 0.5 + 0.35 * (0.6 * cohesion + 0.4 * size_term);
    let conf = py_round(py_min(SCHEDULE_CEILING, raw), 3);
    let board_count = boards_info.len();
    sort_boards(&mut boards_info);
    let summary = format!(
        "{n} accounts share a play SCHEDULE - active (and idle) in nearly the same hours since the weekly reset ({}% \
         hour overlap, ~{} active hours each) ({}). They log in and out together regardless of which content they \
         grind. Weak alone; corroborated signals raise the confidence.",
        fixed(cohesion * 100.0, 0),
        fixed(avg_hours, 0),
        sample(names),
    );
    obj([
        ("stem", "".into()),
        ("label", format!("{} +{}", names[0], n - 1).into()),
        ("method", "schedule".into()),
        ("member_count", n.into()),
        ("members", names.iter().take(CAP).cloned().collect::<Vec<_>>().into()),
        ("members_truncated", n.saturating_sub(CAP).into()),
        ("board_count", board_count.into()),
        ("boards", Value::Array(boards_info.into_iter().map(Value::Object).collect())),
        ("confidence", fv(conf)),
        ("summary", summary.into()),
        (
            "measurements",
            Value::Object(obj([
                ("group_size", n.into()),
                ("schedule_cohesion", fv(py_round(cohesion, 3))),
                ("avg_active_hours", fv(py_round(avg_hours, 1))),
                ("ceiling", fv(SCHEDULE_CEILING)),
            ])),
        ),
    ])
}

// ---------------------------------------------------------------------------
// Fusion

fn members_of(c: &Map<String, Value>) -> Vec<String> {
    c.get("members")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .unwrap_or_default()
}

fn f_of(c: &Map<String, Value>, k: &str) -> f64 {
    c.get(k).and_then(Value::as_f64).unwrap_or(0.0)
}

/// `_fuse`: merge the producers by shared members, then score each group by
/// how many independent signals agree on it.
pub fn fuse(name_clusters: Vec<Map<String, Value>>, signals: &Signals, cm: &CmParams) -> Vec<Map<String, Value>> {
    let producers: Vec<Map<String, Value>> =
        signals.comovement.iter().chain(&signals.schedule).cloned().chain(name_clusters).collect();
    if producers.is_empty() {
        return Vec::new();
    }
    let mut parent: Vec<usize> = (0..producers.len()).collect();
    fn find(p: &mut [usize], mut x: usize) -> usize {
        while p[x] != x {
            p[x] = p[p[x]];
            x = p[x];
        }
        x
    }
    let mut by_member: HashMap<String, usize> = HashMap::new();
    for (idx, c) in producers.iter().enumerate() {
        for m in members_of(c) {
            match by_member.get(&m) {
                Some(&other) => {
                    let (ra, rb) = (find(&mut parent, idx), find(&mut parent, other));
                    if ra != rb {
                        parent[ra.max(rb)] = ra.min(rb);
                    }
                }
                None => {
                    by_member.insert(m, idx);
                }
            }
        }
    }
    let mut groups: Vec<(usize, Vec<usize>)> = Vec::new();
    for idx in 0..producers.len() {
        let root = find(&mut parent, idx);
        match groups.iter_mut().find(|(r, _)| *r == root) {
            Some((_, v)) => v.push(idx),
            None => groups.push((root, vec![idx])),
        }
    }
    let mut out = Vec::new();
    for (_, idxs) in groups {
        let parts: Vec<&Map<String, Value>> = idxs.iter().map(|&i| &producers[i]).collect();
        let key = |c: &Map<String, Value>| {
            (c.get("member_count").and_then(Value::as_i64).unwrap_or(0), f_of(c, "confidence"))
        };
        let mut base = parts[0];
        for c in &parts[1..] {
            let (kb, kc) = (key(base), key(c));
            if kc.0 > kb.0 || (kc.0 == kb.0 && kc.1 > kb.1) {
                base = c;
            }
        }
        let members: Vec<String> =
            parts.iter().flat_map(|c| members_of(c)).collect::<BTreeSet<_>>().into_iter().collect();
        let mut fused = base.clone();
        fused.insert("members".into(), members.iter().take(CAP).cloned().collect::<Vec<_>>().into());
        fused.insert("member_count".into(), members.len().into());
        fused.insert("members_truncated".into(), members.len().saturating_sub(CAP).into());

        let methods: BTreeSet<&str> = parts.iter().filter_map(|c| c.get("method").and_then(Value::as_str)).collect();
        let mut corroborated: Vec<&str> = Vec::new();
        if methods.contains("co_movement") || methods.contains("both") {
            corroborated.push("co_movement");
        }
        if methods.contains("schedule") {
            corroborated.push("schedule");
        }
        let (stem, stem_count) = dominant_stem(&members);
        if methods.contains("name_stem") || methods.contains("both") || stem_count >= 2usize.max(members.len() / 2) {
            corroborated.push("name_stem");
        }
        if !corroborated.contains(&"schedule")
            && !signals.active_hours.is_empty()
            && pair_mean(&members, &signals.active_hours).is_some_and(|m| m >= cm.schedule_min_similarity)
        {
            corroborated.push("schedule");
        }
        if !signals.board_sets.is_empty()
            && pair_mean(&members, &signals.board_sets).is_some_and(|m| m >= cm.footprint_min_jaccard)
        {
            corroborated.push("footprint");
        }
        let mut seen = BTreeSet::new();
        corroborated.retain(|c| seen.insert(*c));
        let base_conf = parts[1..].iter().fold(f_of(parts[0], "confidence"), |m, c| py_max(m, f_of(c, "confidence")));
        let extra = corroborated.len().saturating_sub(1);
        let conf = py_round(py_min(FUSION_CEILING, base_conf + extra as f64 * cm.fusion_corroboration_bonus), 3);
        fused.insert("confidence".into(), fv(conf));
        fused.insert("corroborated_by".into(), corroborated.iter().map(|s| s.to_string()).collect::<Vec<_>>().into());
        let has = |s: &str| corroborated.contains(&s);
        let method = if (has("co_movement") || has("schedule")) && has("name_stem") {
            "both"
        } else if has("co_movement") {
            "co_movement"
        } else if has("schedule") {
            "schedule"
        } else {
            "name_stem"
        };
        fused.insert("method".into(), method.into());
        if !stem.is_empty() && matches!(method, "both" | "name_stem") {
            fused.insert("label".into(), format!("{stem}*").into());
            fused.insert("stem".into(), stem.into());
        }
        out.push(fused);
    }
    sort_by_confidence(&mut out);
    out
}

#[cfg(test)]
mod tests;

//! The `unicodedata` calls email-validator and idna make, over code points
//! (a decoded Punycode label can hold a lone surrogate, as a Python str can).

use super::table as t;

fn run(runs: &[(u32, u8)], cp: u32) -> u8 {
    runs[runs.partition_point(|&(s, _)| s <= cp) - 1].1
}

pub fn in_ranges(r: &[(u32, u32)], cp: u32) -> bool {
    let i = r.partition_point(|&(_, hi)| hi < cp);
    r.get(i).is_some_and(|&(lo, _)| lo <= cp)
}

pub fn category(cp: u32) -> &'static str {
    t::CATEGORIES[run(t::CATEGORY, cp) as usize]
}

pub fn bidirectional(cp: u32) -> &'static str {
    t::BIDIS[run(t::BIDI, cp) as usize]
}

pub fn combining(cp: u32) -> u8 {
    run(t::CCC, cp)
}

/// `unicodedata.name(c)` for marks, separators and format characters.
pub fn name(cp: u32) -> Option<&'static str> {
    t::NAMES.binary_search_by_key(&cp, |e| e.0).ok().map(|i| t::NAMES[i].1)
}

pub fn has_name(cp: u32) -> bool {
    !in_ranges(t::NAMELESS, cp)
}

/// `"002E" in unicodedata.decomposition(c).split(" ")`.
pub fn decomposes_to_dot(cp: u32) -> bool {
    t::DOT_DECOMP.binary_search(&cp).is_ok()
}

static SECONDS: std::sync::LazyLock<Vec<u32>> = std::sync::LazyLock::new(|| {
    let mut v: Vec<u32> = t::COMPOSE.iter().map(|e| e.1).collect();
    v.sort_unstable();
    v.dedup();
    v
});

/// A starter NFC never changes or merges backwards: `NFC(x + y) ==
/// NFC(x) + NFC(y)` when `y` begins with one.
pub fn stable_starter(cp: u32) -> bool {
    combining(cp) == 0
        && !(S_BASE..S_BASE + S_COUNT).contains(&cp)
        && !(V_BASE..V_BASE + V_COUNT).contains(&cp)
        && !(T_BASE..T_BASE + T_COUNT).contains(&cp)
        && t::DECOMP.binary_search_by_key(&cp, |e| e.0).is_err()
        && SECONDS.binary_search(&cp).is_err()
}

const S_BASE: u32 = 0xAC00;
const L_BASE: u32 = 0x1100;
const V_BASE: u32 = 0x1161;
const T_BASE: u32 = 0x11A7;
const L_COUNT: u32 = 19;
const V_COUNT: u32 = 21;
const T_COUNT: u32 = 28;
const N_COUNT: u32 = V_COUNT * T_COUNT;
const S_COUNT: u32 = L_COUNT * N_COUNT;

fn decompose(cp: u32, out: &mut Vec<u32>) {
    if (S_BASE..S_BASE + S_COUNT).contains(&cp) {
        let s = cp - S_BASE;
        out.push(L_BASE + s / N_COUNT);
        out.push(V_BASE + (s % N_COUNT) / T_COUNT);
        if !s.is_multiple_of(T_COUNT) {
            out.push(T_BASE + s % T_COUNT);
        }
        return;
    }
    match t::DECOMP.binary_search_by_key(&cp, |e| e.0) {
        Ok(i) => {
            let (_, a, b) = t::DECOMP[i];
            decompose(a, out);
            if b != 0 {
                decompose(b, out);
            }
        }
        Err(_) => out.push(cp),
    }
}

fn compose(a: u32, b: u32) -> Option<u32> {
    if (L_BASE..L_BASE + L_COUNT).contains(&a) && (V_BASE..V_BASE + V_COUNT).contains(&b) {
        return Some(S_BASE + ((a - L_BASE) * V_COUNT + (b - V_BASE)) * T_COUNT);
    }
    if (S_BASE..S_BASE + S_COUNT).contains(&a)
        && (a - S_BASE).is_multiple_of(T_COUNT)
        && b > T_BASE
        && b < T_BASE + T_COUNT
    {
        return Some(a + (b - T_BASE));
    }
    t::COMPOSE.binary_search_by(|e| (e.0, e.1).cmp(&(a, b))).ok().map(|i| t::COMPOSE[i].2)
}

/// `unicodedata.normalize("NFC", s)`.
pub fn nfc(s: &[u32]) -> Vec<u32> {
    let mut d = Vec::with_capacity(s.len());
    for &c in s {
        decompose(c, &mut d);
    }
    let mut i = 0;
    while i < d.len() {
        if combining(d[i]) == 0 {
            i += 1;
            continue;
        }
        let start = i;
        while i < d.len() && combining(d[i]) != 0 {
            i += 1;
        }
        d[start..i].sort_by_key(|&c| combining(c));
    }
    let mut out: Vec<u32> = Vec::with_capacity(d.len());
    let mut starter: Option<usize> = None;
    let mut last_cc: Option<u8> = None;
    for c in d {
        let cc = combining(c);
        if let Some(si) = starter
            && last_cc.is_none_or(|p| p < cc)
            && let Some(p) = compose(out[si], c)
        {
            out[si] = p;
            continue;
        }
        if cc == 0 {
            starter = Some(out.len());
            last_cc = None;
        } else {
            last_cc = Some(cc);
        }
        out.push(c);
    }
    out
}

pub fn nfc_str(s: &str) -> String {
    let cps: Vec<u32> = s.chars().map(u32::from).collect();
    to_string(&nfc(&cps))
}

/// Code points known to be scalar values.
pub fn to_string(cps: &[u32]) -> String {
    cps.iter().filter_map(|&c| char::from_u32(c)).collect()
}

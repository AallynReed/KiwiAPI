//! Alt-cluster kernels, mirroring `app/trove/leaderboards/detection.py`: name
//! stems, the fuzzy stem merge, densest score bands, lockstep (co-movement)
//! groups, shared-schedule groups and per-account activity features. Every
//! ordered map below reproduces a Python dict's insertion order, since cluster
//! order and union-find roots follow it.

use std::collections::{BTreeSet, HashMap};

use rustc_hash::FxHashMap;

/// An insertion-ordered map (a Python dict).
struct Ordered<K, V> {
    at: FxHashMap<K, usize>,
    items: Vec<(K, V)>,
}

impl<K: std::hash::Hash + Eq + Clone, V> Ordered<K, V> {
    fn new() -> Self {
        Ordered { at: FxHashMap::default(), items: Vec::new() }
    }

    fn entry(&mut self, k: K, default: impl FnOnce() -> V) -> &mut V {
        let i = *self.at.entry(k.clone()).or_insert_with(|| {
            self.items.push((k, default()));
            self.items.len() - 1
        });
        &mut self.items[i].1
    }

    fn get(&self, k: &K) -> Option<&V> {
        self.at.get(k).map(|&i| &self.items[i].1)
    }
}

/// Python's ordering of two floats in a sort (equal when neither is less).
fn py_cmp(a: f64, b: f64) -> std::cmp::Ordering {
    a.partial_cmp(&b).unwrap_or(std::cmp::Ordering::Equal)
}

/// Python `str.isspace` over ASCII.
fn py_space(b: u8) -> bool {
    matches!(b, 9..=13 | 28..=32)
}

/// `_name_stem` for ASCII names; None for anything else (the caller asks Python).
pub fn name_stem(name: &str) -> Option<String> {
    if !name.is_ascii() {
        return None;
    }
    let b = name.as_bytes();
    let (mut lo, mut hi) = (0, b.len());
    while lo < hi && py_space(b[lo]) {
        lo += 1;
    }
    while hi > lo && py_space(b[hi - 1]) {
        hi -= 1;
    }
    let lower: Vec<u8> = b[lo..hi].iter().map(|c| c.to_ascii_lowercase()).collect();
    let in_run = |c: u8| c.is_ascii_digit() || py_space(c) || matches!(c, b'_' | b'-' | b'.' | b'#');
    let mut start = lower.len();
    while start > 0 && in_run(lower[start - 1]) {
        start -= 1;
    }
    let cut = if lower[start..].iter().any(u8::is_ascii_digit) { start } else { lower.len() };
    let out = lower[..cut]
        .iter()
        .map(|&c| match c {
            b'4' | b'@' => b'a',
            b'3' => b'e',
            b'1' | b'!' | b'|' => b'i',
            b'0' => b'o',
            b'5' | b'$' => b's',
            b'7' => b't',
            b'8' => b'b',
            b'9' => b'g',
            c => c,
        })
        .collect();
    String::from_utf8(out).ok()
}

/// `_edit_distance_le` over code points.
pub fn edit_distance_le(a: &[char], b: &[char], max_d: usize) -> bool {
    let (la, lb) = (a.len(), b.len());
    if la.abs_diff(lb) > max_d {
        return false;
    }
    if a == b {
        return true;
    }
    let mut prev: Vec<usize> = (0..=lb).collect();
    let mut cur = vec![0usize; lb + 1];
    for i in 1..=la {
        cur[0] = i;
        let ca = a[i - 1];
        let mut row_min = cur[0];
        for j in 1..=lb {
            let cost = usize::from(ca != b[j - 1]);
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
            row_min = row_min.min(cur[j]);
        }
        if row_min > max_d {
            return false;
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[lb] <= max_d
}

const STEM_BLOCK_CAP: usize = 300;

/// `_merge_stems`: the canonical stem for each of `stems`, in order.
pub fn merge_stems(stems: &[String], max_edit: i64, candidates: Option<&[String]>) -> Vec<String> {
    let index: HashMap<&str, usize> = stems.iter().enumerate().map(|(i, s)| (s.as_str(), i)).collect();
    let mut parent: Vec<usize> = (0..stems.len()).collect();
    fn find(parent: &mut [usize], mut x: usize) -> usize {
        while parent[x] != x {
            parent[x] = parent[parent[x]];
            x = parent[x];
        }
        x
    }
    if max_edit > 0 {
        let pool = candidates.unwrap_or(stems);
        let chars: Vec<Vec<char>> = pool.iter().map(|s| s.chars().collect()).collect();
        let mut blocks: Ordered<(char, usize), Vec<usize>> = Ordered::new();
        for (i, c) in chars.iter().enumerate() {
            if let Some(&first) = c.first() {
                blocks.entry((first, c.len()), Vec::new).push(i);
            }
        }
        for ((c, ln), group) in &blocks.items {
            let next = blocks.get(&(*c, ln + 1)).map_or(&[][..], |v| v.as_slice());
            let cand: Vec<usize> = group.iter().chain(next).copied().collect();
            if cand.len() > STEM_BLOCK_CAP {
                continue;
            }
            for (i, &a) in group.iter().enumerate() {
                for &b in &cand[i + 1..] {
                    if pool[a] != pool[b] && edit_distance_le(&chars[a], &chars[b], max_edit as usize) {
                        let (Some(&ia), Some(&ib)) = (index.get(pool[a].as_str()), index.get(pool[b].as_str())) else {
                            continue;
                        };
                        let (ra, rb) = (find(&mut parent, ia), find(&mut parent, ib));
                        if ra != rb {
                            let (small, big) = if stems[ra] < stems[rb] { (ra, rb) } else { (rb, ra) };
                            parent[big] = small;
                        }
                    }
                }
            }
        }
    }
    (0..stems.len()).map(|i| stems[find(&mut parent, i)].clone()).collect()
}

/// One leaderboard entry for the family kernel.
pub struct Entry<'a> {
    pub name: &'a str,
    pub stem: &'a str,
    pub score: f64,
    pub rank: i64,
}

/// `_densest_band` over entry indices; returns indices in band order.
fn densest_band(entries: &[Entry], idxs: &[usize], band_pct: f64, min_size: usize) -> Option<Vec<usize>> {
    let mut by_name: Ordered<&str, usize> = Ordered::new();
    for &i in idxs {
        let cur = by_name.entry(entries[i].name, || i);
        if entries[i].rank < entries[*cur].rank {
            *cur = i;
        }
    }
    let mut uniq: Vec<usize> = by_name.items.iter().map(|(_, i)| *i).collect();
    uniq.sort_by(|a, b| py_cmp(entries[*a].score, entries[*b].score));
    let n = uniq.len();
    if n < min_size {
        return None;
    }
    let (mut left, mut best): (usize, Option<(usize, usize)>) = (0, None);
    for right in 0..n {
        while left < right {
            let (lo, hi) = (entries[uniq[left]].score, entries[uniq[right]].score);
            let denom = hi.abs().max(lo.abs()).max(1e-9);
            if (hi - lo) / denom <= band_pct {
                break;
            }
            left += 1;
        }
        let size = right - left + 1;
        if size >= min_size && best.is_none_or(|(l, r)| size > r - l + 1) {
            best = Some((left, right));
        }
    }
    best.map(|(l, r)| uniq[l..=r].to_vec())
}

/// `_board_families_py`: `(canonical stem, subset entry indices)` per family.
/// A family: canonical stem and the entry indices in its densest band.
pub type Family = (String, Vec<usize>);

pub fn board_families(entries: &[Entry], min_size: usize, band_pct: f64, max_edit: i64) -> Vec<Family> {
    let mut groups: Ordered<&str, Vec<usize>> = Ordered::new();
    for (i, e) in entries.iter().enumerate() {
        if e.stem.chars().count() >= 2 {
            groups.entry(e.stem, Vec::new).push(i);
        }
    }
    if groups.items.is_empty() {
        return Vec::new();
    }
    let distinct =
        |idxs: &[usize]| idxs.iter().map(|&i| entries[i].name).collect::<std::collections::HashSet<_>>().len();
    let keys: Vec<String> = groups.items.iter().map(|(s, _)| s.to_string()).collect();
    let candidates: Vec<String> =
        groups.items.iter().filter(|(_, v)| distinct(v) >= 2).map(|(s, _)| s.to_string()).collect();
    let canon = merge_stems(&keys, max_edit, Some(&candidates));
    let mut merged: Ordered<String, Vec<usize>> = Ordered::new();
    for ((_, idxs), c) in groups.items.iter().zip(canon) {
        merged.entry(c, Vec::new).extend_from_slice(idxs);
    }
    let mut out = Vec::new();
    for (cstem, idxs) in merged.items {
        if distinct(&idxs) < min_size {
            continue;
        }
        if let Some(subset) = densest_band(entries, &idxs, band_pct, min_size) {
            out.push((cstem, subset));
        }
    }
    out
}

/// `_tighten_component` with the deterministic `(degree, name)` tie-break.
fn tighten(members: &BTreeSet<String>, adj: &Ordered<String, BTreeSet<String>>, min_density: f64) -> BTreeSet<String> {
    let mut mem = members.clone();
    while mem.len() > 2 {
        let deg: Vec<(usize, &String)> =
            mem.iter().map(|m| (adj.get(m).map_or(0, |n| n.iter().filter(|x| mem.contains(*x)).count()), m)).collect();
        let edges = deg.iter().map(|(d, _)| d).sum::<usize>() / 2;
        let possible = (mem.len() * (mem.len() - 1)) as f64 / 2.0;
        if possible <= 0.0 || edges as f64 / possible >= min_density {
            break;
        }
        let drop = deg.iter().min().map(|(_, m)| (*m).clone()).expect("non-empty");
        mem.remove(&drop);
    }
    mem
}

/// Python's `max(a, b)` / `min(a, b)` union over names.
fn find_s(parent: &mut HashMap<String, String>, x: &str) -> String {
    let mut x = x.to_string();
    parent.entry(x.clone()).or_insert_with(|| x.clone());
    loop {
        let p = parent[&x].clone();
        if p == x {
            return x;
        }
        let gp = parent[&p].clone();
        parent.insert(x.clone(), gp.clone());
        x = gp;
    }
}

fn union_s(parent: &mut HashMap<String, String>, a: &str, b: &str) {
    let (ra, rb) = (find_s(parent, a), find_s(parent, b));
    if ra != rb {
        let (small, big) = if ra < rb { (ra, rb) } else { (rb, ra) };
        parent.insert(big, small);
    }
}

/// Components of `adj` (ordered by first node), tightened, kept at `min_group`+.
fn tightened_components(
    parent: &mut HashMap<String, String>,
    adj: &Ordered<String, BTreeSet<String>>,
    min_density: f64,
    min_group: usize,
) -> Vec<BTreeSet<String>> {
    let mut comps: Ordered<String, BTreeSet<String>> = Ordered::new();
    for (node, _) in &adj.items {
        let root = find_s(parent, node);
        comps.entry(root, BTreeSet::new).insert(node.clone());
    }
    comps.items.into_iter().map(|(_, mem)| tighten(&mem, adj, min_density)).filter(|m| m.len() >= min_group).collect()
}

/// One account's scores at capture slots, sorted by slot.
pub type Points = Vec<(usize, f64)>;

/// One board's candidates: `(uuid, [(name, points)])` in dict order.
pub type Board = (i64, Vec<(String, Points)>);

pub struct Params {
    pub min_gain: f64,
    pub pct: f64,
    pub tol: f64,
    pub min_match: i64,
    pub min_ratio: f64,
    pub min_density: f64,
    pub min_group: usize,
    pub max_cell: usize,
}

/// `_history_features`: active slots per account (first-activity order) and the
/// boards each account appears on (first-seen order).
/// Active slots per account, and the boards each account is on.
pub type Features = (Vec<(String, BTreeSet<usize>)>, Vec<(String, Vec<i64>)>);

pub fn history_features(boards: &[Board], min_gain: f64) -> Features {
    let mut active: Ordered<String, BTreeSet<usize>> = Ordered::new();
    let mut on: Ordered<String, Vec<i64>> = Ordered::new();
    for (uuid, series) in boards {
        for (name, pts) in series {
            let b = on.entry(name.clone(), Vec::new);
            if !b.contains(uuid) {
                b.push(*uuid);
            }
            for w in pts.windows(2) {
                let ((s0, v0), (s1, v1)) = (w[0], w[1]);
                if s1 == s0 + 1 && (v1 - v0) >= min_gain {
                    active.entry(name.clone(), BTreeSet::new).insert(s1);
                }
            }
        }
    }
    (active.items, on.items)
}

/// One board's lockstep group: (uuid, sorted members, matching hours, avg gain).
pub type Group = (i64, Vec<String>, usize, f64);

/// `_comovement_board_groups`.
pub fn comovement_groups(boards: &[Board], p: &Params) -> Vec<Group> {
    let band = if p.tol > 0.0 { 1.0 + p.tol } else { 1.0 };
    let mut out = Vec::new();
    for (uuid, series) in boards {
        let mut slot_gains: Ordered<usize, Vec<(&str, f64)>> = Ordered::new();
        for (name, pts) in series {
            for w in pts.windows(2) {
                let ((s0, v0), (s1, v1)) = (w[0], w[1]);
                if s1 != s0 + 1 {
                    continue;
                }
                let delta = v1 - v0;
                if delta < p.min_gain {
                    continue;
                }
                slot_gains.entry(s1, Vec::new).push((name, delta));
            }
        }

        let mut cells: Ordered<(usize, usize), BTreeSet<&str>> = Ordered::new();
        let mut acct_gain: HashMap<&str, f64> = HashMap::new();
        for (slot, gains) in &slot_gains.items {
            let thr = if p.pct > 0.0 && p.pct < 1.0 && gains.len() >= 2 {
                let mut vals: Vec<f64> = gains.iter().map(|(_, g)| *g).collect();
                vals.sort_by(|a, b| py_cmp(*a, *b));
                vals[(vals.len() - 1).min((p.pct * vals.len() as f64) as usize)]
            } else {
                f64::NEG_INFINITY
            };
            let mut kept: Vec<(f64, &str)> = gains.iter().filter(|(_, g)| *g >= thr).map(|(n, g)| (*g, *n)).collect();
            kept.sort_by(|a, b| py_cmp(a.0, b.0).then(a.1.cmp(b.1)));
            let (mut band_idx, mut anchor) = (0usize, kept.first().map_or(0.0, |k| k.0));
            for (g, nm) in kept {
                if g > anchor * band {
                    band_idx += 1;
                    anchor = g;
                }
                cells.entry((*slot, band_idx), BTreeSet::new).insert(nm);
                *acct_gain.entry(nm).or_insert(0.0) += g;
            }
        }

        let mut co: Ordered<(&str, &str), i64> = Ordered::new();
        let mut active: HashMap<&str, i64> = HashMap::new();
        let mut usable: Vec<&BTreeSet<&str>> = Vec::new();
        for (_, names) in &cells.items {
            if names.len() < 2 || names.len() > p.max_cell {
                continue;
            }
            usable.push(names);
            let members: Vec<&str> = names.iter().copied().collect();
            for m in &members {
                *active.entry(m).or_insert(0) += 1;
            }
            for i in 0..members.len() {
                for k in i + 1..members.len() {
                    *co.entry((members[i], members[k]), || 0) += 1;
                }
            }
        }

        let mut parent: HashMap<String, String> = HashMap::new();
        let mut adj: Ordered<String, BTreeSet<String>> = Ordered::new();
        for ((a, b), c) in &co.items {
            if *c < p.min_match {
                continue;
            }
            let denom = (*active.get(a).unwrap_or(&1)).min(*active.get(b).unwrap_or(&1));
            let denom = if denom == 0 { 1 } else { denom };
            if (*c as f64) / (denom as f64) < p.min_ratio {
                continue;
            }
            union_s(&mut parent, a, b);
            adj.entry(a.to_string(), BTreeSet::new).insert(b.to_string());
            adj.entry(b.to_string(), BTreeSet::new).insert(a.to_string());
        }
        if adj.items.is_empty() {
            continue;
        }
        for mem in tightened_components(&mut parent, &adj, p.min_density, p.min_group) {
            let mh = usable.iter().filter(|names| names.iter().filter(|n| mem.contains(**n)).count() >= 2).count();
            let total = mem.iter().fold(0.0, |acc, m| acc + acct_gain.get(m.as_str()).copied().unwrap_or(0.0));
            out.push((*uuid, mem.iter().cloned().collect(), mh, total / mem.len() as f64));
        }
    }
    out
}

/// `_schedule_groups`: sorted member lists. `active` is in `active_hours` order.
pub fn schedule_groups(
    active: &[(String, BTreeSet<usize>)],
    min_hours: usize,
    min_sim: f64,
    min_group: usize,
    min_density: f64,
    max_cell: usize,
    num_slots: usize,
) -> Vec<Vec<String>> {
    let max_active = min_hours.max((num_slots as f64 * 0.85) as usize);
    let cands: Vec<&(String, BTreeSet<usize>)> =
        active.iter().filter(|(_, h)| min_hours <= h.len() && h.len() <= max_active).collect();
    if cands.len() < min_group {
        return Vec::new();
    }
    let mut buckets: Ordered<Vec<usize>, Vec<usize>> = Ordered::new();
    for (i, (_, hrs)) in cands.iter().enumerate() {
        let sig: Vec<usize> = hrs.iter().map(|s| s / 6).collect::<BTreeSet<_>>().into_iter().collect();
        buckets.entry(sig, Vec::new).push(i);
    }
    let jaccard = |a: &BTreeSet<usize>, b: &BTreeSet<usize>| -> f64 {
        if a.is_empty() && b.is_empty() {
            return 0.0;
        }
        let inter = a.intersection(b).count();
        inter as f64 / (a.len() + b.len() - inter) as f64
    };
    let mut parent: HashMap<String, String> = HashMap::new();
    let mut adj: Ordered<String, BTreeSet<String>> = Ordered::new();
    for (_, names) in &buckets.items {
        if names.len() < 2 || names.len() > max_cell {
            continue;
        }
        for i in 0..names.len() {
            for j in i + 1..names.len() {
                let (a, b) = (cands[names[i]], cands[names[j]]);
                if jaccard(&a.1, &b.1) >= min_sim {
                    union_s(&mut parent, &a.0, &b.0);
                    adj.entry(a.0.clone(), BTreeSet::new).insert(b.0.clone());
                    adj.entry(b.0.clone(), BTreeSet::new).insert(a.0.clone());
                }
            }
        }
    }
    if adj.items.is_empty() {
        return Vec::new();
    }
    tightened_components(&mut parent, &adj, min_density, min_group)
        .into_iter()
        .map(|m| m.into_iter().collect())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stems_like_python() {
        assert_eq!(name_stem("Anana17").as_deref(), Some("anana"));
        assert_eq!(name_stem(" Player 001 ").as_deref(), Some("player"));
        assert_eq!(name_stem("Dr4g0n").as_deref(), Some("dragon"));
        assert_eq!(name_stem("abc_-").as_deref(), Some("abc_-"));
        assert_eq!(name_stem("café"), None);
    }

    #[test]
    fn edit_distance() {
        let c = |s: &str| s.chars().collect::<Vec<_>>();
        assert!(edit_distance_le(&c("anana"), &c("annna"), 1));
        assert!(!edit_distance_le(&c("anana"), &c("bob"), 1));
    }
}

//! The structured `.binfab` reader, mirroring `app/trove/decode/wire.py` exactly:
//! the same cheapest-first search, visiting order and tie-breaks, so every parse
//! picks the reading Python picks.
//!
//! Two outcomes Python does not have: a step/state budget and a nesting limit.
//! Either one aborts the parse (`Outcome::Abort`), and the caller decides what to
//! do: rerun Python, or refuse an upload.

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::hash::Hash;
use std::rc::Rc;

use rustc_hash::FxHashMap;

const END: i128 = 0x0F;
const OBJ: i128 = 0x17;
const ARR: i128 = 0x1F;
const MAP: i128 = 0x27;
const CUSTOM: i128 = 0x2F;
const BIGMAP: i128 = 0x37;
const STRMAP: i128 = 0x3F;
const MAX_SECTIONS: u8 = 8;
const MAX_FIELD: i128 = 255;
const COUNT_PENALTY: f64 = 1000.0;
const GARBAGE_PENALTY: f64 = 100_000.0;
const ORDER_PENALTY: f64 = 1e-3;
/// Python spends 12 frames per nesting level against a 20,000 limit; stopping
/// here keeps every reading Rust finishes inside what Python can finish.
pub const MAX_DEPTH: usize = 1500;

pub type V = Rc<Val>;

/// Containers keep the search's shared chain and are only laid out by the
/// accessors below, once a reading has won.
pub enum Val {
    Int(i128),
    Float(f64),
    Str(Rc<str>),
    Bytes(Rc<[u8]>),
    Packed(Vec<Val>),
    /// One class section (see `section_fields`).
    Sec(Chain<i128, V>),
    /// Sections, root class first (see `obj_sections`).
    Obj(Chain<(), V>),
    /// Array slots (see `list_values`).
    List(Chain<i128, Option<V>>),
    /// Map entries (see `map_items`).
    Map(Chain<MapKey, Option<V>>),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum MapKey {
    Int(i128),
    Str(Rc<str>),
}

pub enum Prefab {
    Entity { type_id: i128, components: Vec<(i128, V)> },
    Object(V),
}

pub enum Outcome {
    Parsed(Prefab),
    /// Python raises WireError here too.
    NoReading,
    /// Budget or nesting limit hit: no verdict.
    Abort,
}

#[derive(Debug)]
enum Fail {
    Wire,
    Abort,
}

type R<T> = Result<T, Fail>;

pub struct Link<K, T> {
    parent: Option<Rc<Link<K, T>>>,
    key: K,
    val: T,
}

pub type Chain<K, T> = Option<Rc<Link<K, T>>>;

fn push<K, T>(parent: &Chain<K, T>, key: K, val: T) -> Chain<K, T> {
    Some(Rc::new(Link { parent: parent.clone(), key, val }))
}

fn unchain<K: Clone, T: Clone>(chain: &Chain<K, T>) -> Vec<(K, T)> {
    let mut out = Vec::new();
    let mut cur = chain.clone();
    while let Some(link) = cur {
        out.push((link.key.clone(), link.val.clone()));
        cur = link.parent.clone();
    }
    out.reverse();
    out
}

/// `_section`: fields in first-write order; more than one value is a `Repeated`.
pub fn section_fields(chain: &Chain<i128, V>) -> Vec<(i128, Vec<V>)> {
    let mut out: Vec<(i128, Vec<V>)> = Vec::new();
    let mut at: HashMap<i128, usize> = HashMap::new();
    for (idx, v) in unchain(chain) {
        match at.get(&idx) {
            Some(&i) => out[i].1.push(v),
            None => {
                at.insert(idx, out.len());
                out.push((idx, vec![v]));
            }
        }
    }
    out
}

pub fn obj_sections(chain: &Chain<(), V>) -> Vec<V> {
    unchain(chain).into_iter().map(|(_, s)| s).collect()
}

/// Array values ordered by slot.
pub fn list_values(chain: &Chain<i128, Option<V>>) -> Vec<Option<V>> {
    let mut items = slots(chain);
    items.sort_by_key(|(k, _)| *k);
    items.into_iter().map(|(_, v)| v).collect()
}

pub fn map_items(chain: &Chain<MapKey, Option<V>>) -> Vec<(MapKey, Option<V>)> {
    slots(chain)
}

/// `_slots`: write order kept; a created-empty slot never overwrites a value.
fn slots<K: Clone + Eq + Hash>(chain: &Chain<K, Option<V>>) -> Vec<(K, Option<V>)> {
    let mut out: Vec<(K, Option<V>)> = Vec::new();
    let mut at: HashMap<K, usize> = HashMap::new();
    for (k, v) in unchain(chain) {
        match at.entry(k.clone()) {
            Entry::Occupied(e) => {
                if v.is_some() {
                    out[*e.get()].1 = v;
                }
            }
            Entry::Vacant(e) => {
                e.insert(out.len());
                out.push((k, v));
            }
        }
    }
    out
}

fn garbled(text: &str) -> bool {
    text.chars().any(|c| c == '\u{fffd}' || ((c as u32) < 0x20 && !matches!(c, '\t' | '\n' | '\r')))
}

fn decode_str(b: &[u8]) -> Rc<str> {
    Rc::from(String::from_utf8_lossy(b).as_ref())
}

/// Python's `round(x, 6)`: correctly rounded, ties to even, back to the nearest double.
fn round6(x: f64) -> f64 {
    if !x.is_finite() {
        return x;
    }
    format!("{x:.6}").parse().unwrap_or(x)
}

/// (end offset, section count; 0 outside `objn`) -> (value, cost), insertion-ordered.
type Ends = Vec<((usize, u8), V, f64)>;

enum Y<S, A> {
    Done((usize, u8), V, f64),
    Next(S, A, f64),
}

struct Reader<'a> {
    d: &'a [u8],
    e: usize,
    memo: FxHashMap<(u8, usize), Rc<Ends>>,
    depth: usize,
    budget: u64,
}

/// `_Reader._search`: LIFO queue, cheapest accumulator per state, first-found wins ties.
fn search<S, A>(
    r: &mut Reader,
    start: S,
    start_acc: A,
    mut step: impl FnMut(&mut Reader, &S, &A, &mut Vec<Y<S, A>>) -> R<()>,
) -> R<Ends>
where
    S: Clone + Eq + Hash,
    A: Clone,
{
    let mut best: FxHashMap<S, (f64, A)> = FxHashMap::default();
    best.insert(start.clone(), (0.0, start_acc));
    let mut queue = vec![start];
    let mut done: Ends = Vec::new();
    let mut done_at: FxHashMap<(usize, u8), usize> = FxHashMap::default();
    let mut out = Vec::new();
    while let Some(state) = queue.pop() {
        r.spend(1)?;
        let (cost, acc) = best[&state].clone();
        out.clear();
        step(r, &state, &acc, &mut out)?;
        for y in out.drain(..) {
            match y {
                Y::Done(key, val, extra) => {
                    let total = cost + extra;
                    match done_at.get(&key) {
                        Some(&i) => {
                            if total < done[i].2 {
                                done[i].1 = val;
                                done[i].2 = total;
                            }
                        }
                        None => {
                            done_at.insert(key, done.len());
                            done.push((key, val, total));
                        }
                    }
                }
                Y::Next(key, val, extra) => {
                    let total = cost + extra;
                    let better = best.get(&key).is_none_or(|(c, _)| total < *c);
                    if better {
                        r.spend(1)?;
                        best.insert(key.clone(), (total, val));
                        queue.push(key);
                    }
                }
            }
        }
    }
    Ok(done)
}

impl<'a> Reader<'a> {
    fn new(d: &'a [u8], e: usize, budget: u64) -> Self {
        Reader { d, e, memo: FxHashMap::default(), depth: 0, budget }
    }

    fn spend(&mut self, n: u64) -> R<()> {
        if self.budget < n {
            return Err(Fail::Abort);
        }
        self.budget -= n;
        Ok(())
    }

    fn uvar(&self, mut p: usize, limit: u32) -> R<(i128, usize)> {
        let (mut v, mut s) = (0i128, 0u32);
        loop {
            if p >= self.e || s > limit {
                return Err(Fail::Wire);
            }
            let b = self.d[p];
            p += 1;
            v |= i128::from(b & 0x7F) << s;
            s += 7;
            if b < 0x80 {
                return Ok((v, p));
            }
        }
    }

    fn zz(&self, p: usize, limit: u32) -> R<(i128, usize)> {
        let (v, p) = self.uvar(p, limit)?;
        Ok(((v >> 1) ^ -(v & 1), p))
    }

    fn key(&self, p: usize) -> R<(i128, i128, usize)> {
        let (k, p) = self.zz(p, 35)?;
        if k < 0 {
            return Err(Fail::Wire);
        }
        let (mut wt, mut idx) = (k & 7, k >> 3);
        if wt == 7 {
            (wt, idx) = (k & 0x3F, k >> 6);
        }
        Ok((idx, wt, p))
    }

    fn take(&self, p: usize, n: i128) -> R<&'a [u8]> {
        if n < 0 || p as i128 + n > self.e as i128 {
            return Err(Fail::Wire);
        }
        Ok(&self.d[p..p + n as usize])
    }

    fn scalar(&self, p: usize, wt: i128) -> R<(Val, usize)> {
        match wt {
            0 => self.zz(p, 35).map(|(v, q)| (Val::Int(v), q)),
            1 => self.uvar(p, 35).map(|(v, q)| (Val::Int(v), q)),
            2 => {
                let b = self.take(p, 4)?;
                let f = f32::from_le_bytes([b[0], b[1], b[2], b[3]]);
                Ok((Val::Float(round6(f64::from(f))), p + 4))
            }
            3 => {
                let b: [u8; 8] = self.take(p, 8)?.try_into().map_err(|_| Fail::Wire)?;
                if matches!(b[7], 0 | 0xFF) && matches!(b[6], 0 | 0xFF) {
                    Ok((Val::Int(i128::from(i64::from_le_bytes(b))), p + 8))
                } else {
                    Ok((Val::Float(f64::from_le_bytes(b)), p + 8))
                }
            }
            4 => {
                let (n, q) = self.uvar(p, 35)?;
                let t = self.take(q, n)?;
                Ok((Val::Str(decode_str(t)), q + n as usize))
            }
            6 => {
                let (n, ewt, mut q) = self.key(p)?;
                if n > 256 || !(0..=3).contains(&ewt) {
                    return Err(Fail::Wire);
                }
                let mut out = Vec::with_capacity(n as usize);
                for _ in 0..n {
                    let (v, r) = self.scalar(q, ewt)?;
                    out.push(v);
                    q = r;
                }
                Ok((Val::Packed(out), q))
            }
            CUSTOM => {
                let (n, q) = self.uvar(p, 35)?;
                let t = self.take(q, n)?;
                Ok((Val::Bytes(Rc::from(t)), q + n as usize))
            }
            _ => Err(Fail::Wire),
        }
    }

    fn value_ends(&mut self, p: usize, wt: i128) -> R<Rc<Ends>> {
        match wt {
            OBJ => self.memoised(1, p),
            ARR => self.memoised(3, p),
            MAP => self.memoised(4, p),
            BIGMAP => self.memoised(5, p),
            STRMAP => self.memoised(6, p),
            _ => {
                let (v, q) = self.scalar(p, wt)?;
                let cost = match &v {
                    Val::Str(s) if wt == 4 && garbled(s) => GARBAGE_PENALTY,
                    _ => 0.0,
                };
                Ok(Rc::new(vec![((q, 0), Rc::new(v), cost)]))
            }
        }
    }

    fn memoised(&mut self, kind: u8, p: usize) -> R<Rc<Ends>> {
        if let Some(hit) = self.memo.get(&(kind, p)) {
            return Ok(hit.clone());
        }
        self.memo.insert((kind, p), Rc::new(Vec::new()));
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(Fail::Abort);
        }
        let built = stacker::maybe_grow(256 * 1024, 8 * 1024 * 1024, || match kind {
            0 => self.build_section(p),
            1 => self.build_obj(p, false),
            2 => self.build_obj(p, true),
            3 => self.build_array(p),
            _ => self.build_map(p, [MAP, BIGMAP, STRMAP][usize::from(kind - 4)]),
        });
        self.depth -= 1;
        let hit = Rc::new(match built {
            Ok(v) => v,
            Err(Fail::Wire) => Vec::new(),
            Err(Fail::Abort) => return Err(Fail::Abort),
        });
        self.memo.insert((kind, p), hit.clone());
        Ok(hit)
    }

    fn build_section(&mut self, p: usize) -> R<Ends> {
        type S = (usize, [u64; 4], i128);
        search::<S, Chain<i128, V>>(self, (p, [0; 4], -1), None, |r, &(pos, used, last), chain, out| {
            let (idx, wt, q) = match r.key(pos) {
                Ok(k) => k,
                Err(Fail::Wire) => return Ok(()),
                Err(e) => return Err(e),
            };
            if wt == END {
                out.push(Y::Done((q, 0), Rc::new(Val::Sec(chain.clone())), 0.0));
                return Ok(());
            }
            let seen = (0..=MAX_FIELD).contains(&idx) && used[(idx >> 6) as usize] & (1 << (idx & 63)) != 0;
            if idx > MAX_FIELD || (seen && idx != last) {
                return Ok(());
            }
            let alts = match r.value_ends(q, wt) {
                Ok(a) => a,
                Err(Fail::Wire) => return Ok(()),
                Err(e) => return Err(e),
            };
            let order = if idx < last { ORDER_PENALTY } else { 0.0 };
            let mut grown = used;
            grown[(idx >> 6) as usize] |= 1 << (idx & 63);
            for (end, v, c) in alts.iter() {
                out.push(Y::Next((end.0, grown, idx), push(chain, idx, v.clone()), c + order));
            }
            Ok(())
        })
    }

    fn build_obj(&mut self, p: usize, by_sections: bool) -> R<Ends> {
        search::<(usize, u8), Chain<(), V>>(self, (p, 0), None, |r, &(pos, n), secs, out| {
            if n >= MAX_SECTIONS {
                return Ok(());
            }
            let ends = r.memoised(0, pos)?;
            for (end, sec, c) in ends.iter() {
                let grown = push(secs, (), sec.clone());
                let obj = Rc::new(Val::Obj(grown.clone()));
                let key = if by_sections { (end.0, n + 1) } else { (end.0, 0) };
                out.push(Y::Done(key, obj, c + 1.0));
                out.push(Y::Next((end.0, n + 1), grown, c + 1.0));
            }
            Ok(())
        })
    }

    fn build_array(&mut self, p: usize) -> R<Ends> {
        let (count, ewt, q) = self.key(p)?;
        search::<(usize, i128), Chain<i128, Option<V>>>(self, (q, 0), None, |r, &(pos, written), chain, out| {
            let (k, rr) = match r.zz(pos, 70) {
                Ok(v) => v,
                Err(Fail::Wire) => return Ok(()),
                Err(e) => return Err(e),
            };
            let (op, slot) = (k & 7, k >> 3);
            if op == 4 {
                out.push(Y::Done(
                    (rr, 0),
                    Rc::new(Val::List(chain.clone())),
                    COUNT_PENALTY * (count - written).abs() as f64,
                ));
                return Ok(());
            }
            if op == 0 {
                out.push(Y::Next((rr, written), push(chain, slot, None), 0.0));
                return Ok(());
            }
            if op != 2 || written > count {
                return Ok(());
            }
            let alts = match r.value_ends(rr, ewt) {
                Ok(a) => a,
                Err(Fail::Wire) => return Ok(()),
                Err(e) => return Err(e),
            };
            for (end, v, c) in alts.iter() {
                out.push(Y::Next((end.0, written + 1), push(chain, slot, Some(v.clone())), *c));
            }
            Ok(())
        })
    }

    fn build_map(&mut self, p: usize, kind: i128) -> R<Ends> {
        let (_, vwt, q) = self.key(p)?;
        if vwt != OBJ {
            return Err(Fail::Wire);
        }
        search::<usize, Chain<MapKey, Option<V>>>(self, q, None, |r, &pos, chain, out| {
            let read = |r: &Reader| -> R<(i128, MapKey, usize)> {
                if kind == MAP {
                    let (k, rr) = r.zz(pos, 70)?;
                    return Ok((k & 7, MapKey::Int(k >> 3), rr));
                }
                let op = i128::from(r.take(pos, 1)?[0]);
                if kind == BIGMAP {
                    let (mk, rr) = r.zz(pos + 1, 70)?;
                    return Ok((op, MapKey::Int(mk), rr));
                }
                let (n, rr) = r.uvar(pos + 1, 35)?;
                let t = r.take(rr, n)?;
                Ok((op, MapKey::Str(decode_str(t)), rr + n as usize))
            };
            let (op, mk, rr) = match read(r) {
                Ok(v) => v,
                Err(Fail::Wire) => return Ok(()),
                Err(e) => return Err(e),
            };
            if op == 4 {
                out.push(Y::Done((rr, 0), Rc::new(Val::Map(chain.clone())), 0.0));
                return Ok(());
            }
            if op == 0 || op == 3 {
                out.push(Y::Next(rr, push(chain, mk, None), 0.0));
                return Ok(());
            }
            if op != 1 && op != 2 {
                return Ok(());
            }
            let alts = match r.value_ends(rr, vwt) {
                Ok(a) => a,
                Err(Fail::Wire) => return Ok(()),
                Err(e) => return Err(e),
            };
            for (end, v, c) in alts.iter() {
                out.push(Y::Next(end.0, push(chain, mk.clone(), Some(v.clone())), *c));
            }
            Ok(())
        })
    }
}

/// `read_object`; the budget is shared with the caller.
fn read_object(data: &[u8], start: usize, end: usize, sections: Option<i128>, budget: &mut u64) -> R<V> {
    let mut r = Reader::new(data, end, *budget);
    let result = (|| {
        if let Some(s) = sections.filter(|&s| s != 0) {
            let hit = r.memoised(2, start)?;
            if let Some((_, v, _)) = hit.iter().find(|(k, _, _)| k.0 == end && i128::from(k.1) == s) {
                return Ok(v.clone());
            }
        }
        let hit = r.memoised(1, start)?;
        hit.iter().find(|(k, _, _)| k.0 == end).map(|(_, v, _)| v.clone()).ok_or(Fail::Wire)
    })();
    *budget = r.budget;
    result
}

fn entity(data: &[u8], sections: &HashMap<i128, i128>, budget: &mut u64) -> R<Option<Prefab>> {
    let head = Reader::new(data, data.len(), 0);
    let Ok((tid, p)) = head.zz(0, 35) else { return Ok(None) };
    let Ok((_, p)) = head.zz(p, 70) else { return Ok(None) };
    let Ok((size, mut p)) = head.uvar(p, 35) else { return Ok(None) };
    if size <= 0 || p as i128 + size != data.len() as i128 {
        return Ok(None);
    }
    let mut comps = Vec::new();
    while p < data.len() {
        let Ok((cid, q)) = head.zz(p, 35) else { return Ok(None) };
        let Ok((n, q)) = head.uvar(q, 35) else { return Ok(None) };
        if q as i128 + n > data.len() as i128 {
            return Ok(None);
        }
        let n = n as usize;
        match read_object(data, q, q + n, sections.get(&cid).copied(), budget) {
            Ok(obj) => comps.push((cid, obj)),
            Err(Fail::Wire) => return Ok(None),
            Err(Fail::Abort) => return Err(Fail::Abort),
        }
        p = q + n;
    }
    Ok(Some(Prefab::Entity { type_id: tid, components: comps }))
}

/// `wire.parse`. `sections` is `COMPONENT_SECTIONS`; `budget` caps search steps.
pub fn parse(data: &[u8], sections: &HashMap<i128, i128>, mut budget: u64) -> Outcome {
    match entity(data, sections, &mut budget) {
        Ok(Some(pf)) => return Outcome::Parsed(pf),
        Ok(None) => {}
        Err(_) => return Outcome::Abort,
    }
    match read_object(data, 0, data.len(), None, &mut budget) {
        Ok(v) => Outcome::Parsed(Prefab::Object(v)),
        Err(Fail::Wire) => Outcome::NoReading,
        Err(Fail::Abort) => Outcome::Abort,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uv(mut v: u64) -> Vec<u8> {
        let mut out = Vec::new();
        loop {
            let b = (v & 0x7F) as u8;
            v >>= 7;
            if v == 0 {
                out.push(b);
                return out;
            }
            out.push(b | 0x80);
        }
    }

    fn key(idx: u64, wt: u64) -> Vec<u8> {
        let k = if wt >= 7 { (idx << 6) | wt } else { (idx << 3) | wt };
        uv(k << 1)
    }

    #[test]
    fn reads_a_flat_object() {
        let mut d = key(1, 1);
        d.extend(uv(42));
        d.extend(key(2, 4));
        d.extend(uv(2));
        d.extend(b"hi");
        d.extend(key(0, 0x0F));
        let Outcome::Parsed(Prefab::Object(v)) = parse(&d, &HashMap::new(), 1_000_000) else { panic!() };
        let Val::Obj(chain) = &*v else { panic!() };
        let secs = obj_sections(chain);
        assert_eq!(secs.len(), 1);
        let Val::Sec(sec) = &*secs[0] else { panic!() };
        let fields = section_fields(sec);
        assert!(matches!(*fields[0].1[0], Val::Int(42)));
        assert!(matches!(&*fields[1].1[0], Val::Str(s) if &**s == "hi"));
    }

    #[test]
    fn round6_matches_python() {
        assert_eq!(round6(0.1f32 as f64), 0.1);
        assert_eq!(round6(-1e-9), -0.0);
    }
}

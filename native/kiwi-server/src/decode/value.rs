//! A parsed `.binfab` in Python's shapes (`app/trove/decode/wire.py`): sections
//! and maps are dicts, objects are `Obj` lists of sections, repeated indices a
//! `Repeated`, packed scalars a tuple, empty array slots `None`.

use std::{
    collections::HashMap,
    sync::{Arc, OnceLock},
};

use kiwi_core::wire as kw;

use super::sections::COMPONENT_SECTIONS;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum K {
    Int(i128),
    Str(Arc<str>),
}

#[derive(Clone, Debug)]
pub enum W {
    None,
    Int(i128),
    Float(f64),
    Str(Arc<str>),
    Bytes(Arc<[u8]>),
    Tuple(Arc<[W]>),
    Dict(Arc<Dict>),
    Obj(Arc<[W]>),
    Rep(Arc<[W]>),
    List(Arc<[W]>),
}

/// An insertion-ordered dict keyed by int or str, indexed once it is large.
#[derive(Debug, Default)]
pub struct Dict {
    items: Vec<(K, W)>,
    ints: Option<HashMap<i128, usize>>,
    strs: Option<HashMap<Arc<str>, usize>>,
}

pub static EMPTY: Dict = Dict { items: Vec::new(), ints: None, strs: None };

impl Dict {
    pub fn new(items: Vec<(K, W)>) -> Dict {
        let mut d = Dict { items, ints: None, strs: None };
        if d.items.len() > 16 {
            let (mut ints, mut strs) = (HashMap::new(), HashMap::new());
            for (i, (k, _)) in d.items.iter().enumerate() {
                match k {
                    K::Int(n) => {
                        ints.entry(*n).or_insert(i);
                    }
                    K::Str(s) => {
                        strs.entry(s.clone()).or_insert(i);
                    }
                }
            }
            d.ints = Some(ints);
            d.strs = Some(strs);
        }
        d
    }

    pub fn get(&self, idx: i128) -> Option<&W> {
        match &self.ints {
            Some(m) => m.get(&idx).map(|&i| &self.items[i].1),
            None => self.items.iter().find(|(k, _)| *k == K::Int(idx)).map(|(_, v)| v),
        }
    }

    pub fn get_s(&self, key: &str) -> Option<&W> {
        match &self.strs {
            Some(m) => m.get(key).map(|&i| &self.items[i].1),
            None => self.items.iter().find(|(k, _)| matches!(k, K::Str(s) if &**s == key)).map(|(_, v)| v),
        }
    }

    pub fn get_k(&self, key: &K) -> Option<&W> {
        match key {
            K::Int(i) => self.get(*i),
            K::Str(s) => self.get_s(s),
        }
    }

    /// `d.get(idx)` as a str.
    pub fn str(&self, idx: i128) -> Option<&str> {
        self.get(idx).and_then(W::as_str)
    }

    pub fn int(&self, idx: i128) -> Option<i128> {
        self.get(idx).and_then(W::as_int)
    }

    /// `d.get(idx, 0)` for an int field.
    pub fn int0(&self, idx: i128) -> i128 {
        self.int(idx).unwrap_or(0)
    }

    pub fn has(&self, idx: i128) -> bool {
        self.get(idx).is_some()
    }

    /// `d.get(idx) or []` iterated.
    pub fn iter_of(&self, idx: i128) -> Vec<W> {
        self.get(idx).map(W::iter).unwrap_or_default()
    }

    pub fn items(&self) -> &[(K, W)] {
        &self.items
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn values(&self) -> impl Iterator<Item = &W> {
        self.items.iter().map(|(_, v)| v)
    }

    pub fn int_keys(&self) -> impl Iterator<Item = i128> + '_ {
        self.items.iter().filter_map(|(k, _)| match k {
            K::Int(i) => Some(*i),
            K::Str(_) => None,
        })
    }

    pub fn has_key(&self, idx: i128) -> bool {
        self.get(idx).is_some()
    }
}

impl W {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            W::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_arc_str(&self) -> Option<Arc<str>> {
        match self {
            W::Str(s) => Some(s.clone()),
            _ => None,
        }
    }

    pub fn as_int(&self) -> Option<i128> {
        match self {
            W::Int(i) => Some(*i),
            _ => None,
        }
    }

    pub fn as_float(&self) -> Option<f64> {
        match self {
            W::Float(f) => Some(*f),
            _ => None,
        }
    }

    /// `isinstance(v, (int, float))`.
    pub fn as_num(&self) -> Option<f64> {
        match self {
            W::Int(i) => Some(*i as f64),
            W::Float(f) => Some(*f),
            _ => None,
        }
    }

    pub fn as_dict(&self) -> Option<&Dict> {
        match self {
            W::Dict(d) => Some(d),
            _ => None,
        }
    }

    pub fn is_obj(&self) -> bool {
        matches!(self, W::Obj(_))
    }

    pub fn is_none(&self) -> bool {
        matches!(self, W::None)
    }

    /// `isinstance(v, list)`.
    pub fn is_list(&self) -> bool {
        matches!(self, W::Obj(_) | W::Rep(_) | W::List(_))
    }

    /// An `Obj`'s sections (empty for anything else).
    pub fn sections(&self) -> &[W] {
        match self {
            W::Obj(s) => s,
            _ => &[],
        }
    }

    /// Section `i` of an `Obj` as a dict.
    pub fn section(&self, i: usize) -> &Dict {
        self.sections().get(i).and_then(W::as_dict).unwrap_or(&EMPTY)
    }

    /// `v.leaf if isinstance(v, Obj) else {}`.
    pub fn leaf(&self) -> &Dict {
        match self {
            W::Obj(s) => s.last().and_then(W::as_dict).unwrap_or(&EMPTY),
            _ => &EMPTY,
        }
    }

    /// `Obj.get(idx)`: a field of the leaf section.
    pub fn get(&self, idx: i128) -> Option<&W> {
        self.leaf().get(idx)
    }

    /// `len(v)` of a container (0 for a scalar).
    pub fn len(&self) -> usize {
        match self {
            W::Tuple(x) | W::Obj(x) | W::Rep(x) | W::List(x) => x.len(),
            W::Dict(d) => d.len(),
            W::Str(s) => s.chars().count(),
            W::Bytes(b) => b.len(),
            _ => 0,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Python truthiness.
    pub fn truthy(&self) -> bool {
        match self {
            W::None => false,
            W::Int(i) => *i != 0,
            W::Float(f) => *f != 0.0,
            _ => !self.is_empty(),
        }
    }

    /// `for x in (v or [])`: elements, a dict's keys, a str's characters.
    pub fn iter(&self) -> Vec<W> {
        match self {
            W::Tuple(x) | W::Obj(x) | W::Rep(x) | W::List(x) => x.to_vec(),
            W::Dict(d) => d
                .items
                .iter()
                .map(|(k, _)| match k {
                    K::Int(i) => W::Int(*i),
                    K::Str(s) => W::Str(s.clone()),
                })
                .collect(),
            W::Str(s) => s.chars().map(|c| W::Str(Arc::from(c.to_string().as_str()))).collect(),
            W::Bytes(b) => b.iter().map(|&x| W::Int(i128::from(x))).collect(),
            _ => Vec::new(),
        }
    }

    /// The elements of a list or tuple, borrowed.
    pub fn elems(&self) -> &[W] {
        match self {
            W::Tuple(x) | W::Obj(x) | W::Rep(x) | W::List(x) => x,
            _ => &[],
        }
    }

    /// `ability.walk_values`: this node, then everything under it, depth first.
    pub fn walk(&self, f: &mut impl FnMut(&W)) {
        f(self);
        match self {
            W::Dict(d) => d.values().for_each(|v| v.walk(f)),
            W::Tuple(x) | W::Obj(x) | W::Rep(x) | W::List(x) => x.iter().for_each(|v| v.walk(f)),
            _ => {}
        }
    }

    /// `wire.walk`: every scalar under this value, depth first.
    pub fn scalars(&self, f: &mut impl FnMut(&W)) {
        match self {
            W::Dict(d) => d.values().for_each(|v| v.scalars(f)),
            W::Tuple(x) | W::Obj(x) | W::Rep(x) | W::List(x) => x.iter().for_each(|v| v.scalars(f)),
            _ => f(self),
        }
    }

    /// `wire.strings`.
    pub fn strings(&self) -> Vec<Arc<str>> {
        let mut out = Vec::new();
        self.scalars(&mut |v| {
            if let W::Str(s) = v {
                out.push(s.clone());
            }
        });
        out
    }
}

/// Python `==` between parsed values (lists compare across list subclasses).
pub fn py_eq(a: &W, b: &W) -> bool {
    match (a, b) {
        (W::None, W::None) => true,
        (W::Str(x), W::Str(y)) => x == y,
        (W::Bytes(x), W::Bytes(y)) => x == y,
        (W::Tuple(x), W::Tuple(y)) => x.len() == y.len() && x.iter().zip(y.iter()).all(|(p, q)| py_eq(p, q)),
        (W::Obj(x) | W::Rep(x) | W::List(x), W::Obj(y) | W::Rep(y) | W::List(y)) => {
            x.len() == y.len() && x.iter().zip(y.iter()).all(|(p, q)| py_eq(p, q))
        }
        (W::Dict(x), W::Dict(y)) => {
            x.len() == y.len() && x.items().iter().all(|(k, v)| y.get_k(k).is_some_and(|w| py_eq(v, w)))
        }
        _ => match (a.as_num(), b.as_num()) {
            (Some(p), Some(q)) => match (a, b) {
                (W::Int(i), W::Int(j)) => i == j,
                _ => p == q,
            },
            _ => false,
        },
    }
}

/// `wire.strings` over several values (`strings(pf.components)`).
pub fn strings_of<'a>(values: impl IntoIterator<Item = &'a W>) -> Vec<Arc<str>> {
    values.into_iter().flat_map(W::strings).collect()
}

#[derive(Debug, Default, Clone)]
pub struct Prefab {
    pub entity: bool,
    pub type_id: Option<i128>,
    pub components: Vec<(i128, W)>,
    pub root: Option<W>,
}

impl Prefab {
    pub fn component(&self, cid: i128) -> Option<&W> {
        self.components.iter().find(|(c, _)| *c == cid).map(|(_, o)| o)
    }

    /// `pf.root.leaf` (empty when there is no root).
    pub fn root_leaf(&self) -> &Dict {
        self.root.as_ref().map_or(&EMPTY, W::leaf)
    }

    /// Every component object, then the root.
    pub fn objects(&self) -> impl Iterator<Item = &W> {
        self.components.iter().map(|(_, o)| o).chain(self.root.iter())
    }
}

#[derive(Debug, Clone, Copy)]
pub struct WireError;

pub fn sections() -> &'static HashMap<i128, i128> {
    static S: OnceLock<HashMap<i128, i128>> = OnceLock::new();
    S.get_or_init(|| COMPONENT_SECTIONS.iter().copied().collect())
}

/// `wire.parse` (Rust reader, no budget: Python falls back to its own reader,
/// which reaches the same reading however long it takes).
pub fn parse(data: &[u8]) -> Result<Prefab, WireError> {
    match kw::parse(data, sections(), u64::MAX) {
        kw::Outcome::Parsed(kw::Prefab::Entity { type_id, components }) => Ok(Prefab {
            entity: true,
            type_id: Some(type_id),
            components: components.iter().map(|(c, v)| (*c, conv(v))).collect(),
            root: None,
        }),
        kw::Outcome::Parsed(kw::Prefab::Object(v)) => Ok(Prefab { root: Some(conv(&v)), ..Prefab::default() }),
        _ => Err(WireError),
    }
}

/// `wire.parse` under a step budget: `None` when the budget ran out.
pub fn parse_budget(data: &[u8], budget: u64) -> Option<Result<Prefab, WireError>> {
    match kw::parse(data, sections(), budget) {
        kw::Outcome::Abort => None,
        kw::Outcome::NoReading => Some(Err(WireError)),
        kw::Outcome::Parsed(kw::Prefab::Entity { type_id, components }) => Some(Ok(Prefab {
            entity: true,
            type_id: Some(type_id),
            components: components.iter().map(|(c, v)| (*c, conv(v))).collect(),
            root: None,
        })),
        kw::Outcome::Parsed(kw::Prefab::Object(v)) => Some(Ok(Prefab { root: Some(conv(&v)), ..Prefab::default() })),
    }
}

/// `wire.read_object(data, start, end, sections)`.
pub fn read_object(data: &[u8], start: usize, end: usize, sections: Option<i128>) -> Result<W, WireError> {
    kw::read_object_span(data, start, end, sections, u64::MAX).map(|v| conv(&v)).ok_or(WireError)
}

fn conv(v: &kw::Val) -> W {
    match v {
        kw::Val::Int(i) => W::Int(*i),
        kw::Val::Float(f) => W::Float(*f),
        kw::Val::Str(s) => W::Str(Arc::from(&**s)),
        kw::Val::Bytes(b) => W::Bytes(Arc::from(&**b)),
        kw::Val::Packed(xs) => W::Tuple(xs.iter().map(conv).collect()),
        kw::Val::Sec(chain) => W::Dict(Arc::new(Dict::new(
            kw::section_fields(chain)
                .into_iter()
                .map(|(i, xs)| {
                    let v = if xs.len() == 1 { conv(&xs[0]) } else { W::Rep(xs.iter().map(|x| conv(x)).collect()) };
                    (K::Int(i), v)
                })
                .collect(),
        ))),
        kw::Val::Obj(chain) => W::Obj(kw::obj_sections(chain).iter().map(|s| conv(s)).collect()),
        kw::Val::List(chain) => {
            W::List(kw::list_values(chain).iter().map(|x| x.as_ref().map_or(W::None, |x| conv(x))).collect())
        }
        kw::Val::Map(chain) => W::Dict(Arc::new(Dict::new(
            kw::map_items(chain)
                .into_iter()
                .map(|(k, x)| {
                    let key = match k {
                        kw::MapKey::Int(i) => K::Int(i),
                        kw::MapKey::Str(s) => K::Str(Arc::from(&*s)),
                    };
                    (key, x.as_ref().map_or(W::None, |x| conv(x)))
                })
                .collect(),
        ))),
    }
}

/// A uvarint at `p` (`_Reader.uvar`, reading `data[..end]`).
pub fn uvar(data: &[u8], mut p: usize, limit: u32) -> Result<(i128, usize), WireError> {
    let (mut v, mut s) = (0i128, 0u32);
    loop {
        if p >= data.len() || s > limit {
            return Err(WireError);
        }
        let b = data[p];
        p += 1;
        v |= i128::from(b & 0x7F) << s;
        s += 7;
        if b < 0x80 {
            return Ok((v, p));
        }
    }
}

pub fn zz(data: &[u8], p: usize, limit: u32) -> Result<(i128, usize), WireError> {
    let (v, p) = uvar(data, p, limit)?;
    Ok(((v >> 1) ^ -(v & 1), p))
}

//! Literal arrays from AVM2 static initialisers, for `app/trove/swf/abc.py`
//! (`_parse` + `_literal_arrays` over one ABC block).

#[derive(Clone, Debug, PartialEq)]
pub enum Val {
    None,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    List(Vec<Val>),
}

/// Ordered `name -> value`; a repeated name keeps its first position.
pub type Ordered<T> = Vec<(String, T)>;

fn put<T>(map: &mut Ordered<T>, key: String, value: T) {
    match map.iter_mut().find(|(k, _)| *k == key) {
        Some(slot) => slot.1 = value,
        None => map.push((key, value)),
    }
}

struct Buf<'a> {
    d: &'a [u8],
    p: usize,
}

impl<'a> Buf<'a> {
    fn u8(&mut self) -> Option<u8> {
        let b = *self.d.get(self.p)?;
        self.p += 1;
        Some(b)
    }

    fn u30(&mut self) -> Option<u64> {
        let mut out = 0u64;
        for shift in (0..35).step_by(7) {
            let b = self.u8()?;
            out |= ((b & 0x7F) as u64) << shift;
            if b & 0x80 == 0 {
                break;
            }
        }
        Some(out)
    }

    fn idx(&mut self) -> Option<usize> {
        self.u30().map(|v| v as usize)
    }

    fn bytes(&mut self, n: usize) -> Option<&'a [u8]> {
        let s = self.d.get(self.p..self.p + n)?;
        self.p += n;
        Some(s)
    }
}

fn traits(b: &mut Buf) -> Option<()> {
    for _ in 0..b.u30()? {
        b.u30()?;
        let kind = b.u8()?;
        match kind & 0x0F {
            0 | 6 => {
                b.u30()?;
                b.u30()?;
                if b.u30()? != 0 {
                    b.u8()?;
                }
            }
            1..=5 => {
                b.u30()?;
                b.u30()?;
            }
            _ => return None,
        }
        if kind >> 4 & 0x04 != 0 {
            for _ in 0..b.u30()? {
                b.u30()?;
            }
        }
    }
    Some(())
}

fn count(b: &mut Buf) -> Option<u64> {
    Some(b.u30()?.saturating_sub(1))
}

/// `abc.static_arrays` over a whole movie; None for LZMA movies and wherever
/// Python raises.
pub fn movie_static_arrays(swf: &[u8]) -> Option<Ordered<Ordered<Val>>> {
    let movie = crate::swf::movie(swf)?;
    let mut out = Vec::new();
    for (code, body) in crate::swf::tags(&movie)? {
        let block = match code {
            82 => {
                let rest = body.get(4..)?;
                &rest[rest.iter().position(|&b| b == 0)? + 1..]
            }
            72 => body,
            _ => continue,
        };
        for (class, arrays) in static_arrays(block)? {
            put(&mut out, class, arrays);
        }
    }
    Some(out)
}

/// `class name -> {static name: array}` for one ABC block; None where Python raises.
pub fn static_arrays(abc: &[u8]) -> Option<Ordered<Ordered<Val>>> {
    let b = &mut Buf { d: abc, p: 4 };
    if abc.len() < 4 {
        return None;
    }
    let mut ints = vec![0i64];
    for _ in 0..count(b)? {
        ints.push(b.u30()? as i64);
    }
    let mut uints = vec![0i64];
    for _ in 0..count(b)? {
        uints.push(b.u30()? as i64);
    }
    let mut doubles = vec![0.0f64];
    for _ in 0..count(b)? {
        doubles.push(f64::from_le_bytes(b.bytes(8)?.try_into().ok()?));
    }
    let mut strings = vec![String::new()];
    for _ in 0..count(b)? {
        let n = b.idx()?;
        strings.push(String::from_utf8_lossy(b.bytes(n)?).into_owned());
    }
    for _ in 0..count(b)? {
        b.u8()?;
        b.u30()?;
    }
    for _ in 0..count(b)? {
        for _ in 0..b.u30()? {
            b.u30()?;
        }
    }
    let mut names = vec![String::new()];
    for _ in 0..count(b)? {
        let name = match b.u8()? {
            0x07 | 0x0D => {
                b.u30()?;
                strings.get(b.idx()?)?.clone()
            }
            0x0F | 0x10 => strings.get(b.idx()?)?.clone(),
            0x11 | 0x12 => String::new(),
            0x09 | 0x0E => {
                let s = strings.get(b.idx()?)?.clone();
                b.u30()?;
                s
            }
            0x1B | 0x1C => {
                b.u30()?;
                String::new()
            }
            0x1D => {
                b.u30()?;
                for _ in 0..b.u30()? {
                    b.u30()?;
                }
                String::new()
            }
            _ => return None,
        };
        names.push(name);
    }
    for _ in 0..b.u30()? {
        let params = b.u30()?;
        b.u30()?;
        for _ in 0..params {
            b.u30()?;
        }
        b.u30()?;
        let flags = b.u8()?;
        if flags & 0x08 != 0 {
            for _ in 0..b.u30()? {
                b.u30()?;
                b.u8()?;
            }
        }
        if flags & 0x80 != 0 {
            for _ in 0..params {
                b.u30()?;
            }
        }
    }
    for _ in 0..b.u30()? {
        b.u30()?;
        for _ in 0..b.u30()? {
            b.u30()?;
            b.u30()?;
        }
    }
    let classes = b.u30()? as usize;
    let mut class_names = Vec::new();
    for _ in 0..classes {
        class_names.push(names.get(b.idx()?)?.clone());
        b.u30()?;
        if b.u8()? & 0x08 != 0 {
            b.u30()?;
        }
        for _ in 0..b.u30()? {
            b.u30()?;
        }
        b.u30()?;
        traits(b)?;
    }
    let mut cinits: rustc_hash::FxHashMap<u64, usize> = Default::default();
    for i in 0..classes {
        cinits.insert(b.u30()?, i);
        traits(b)?;
    }
    for _ in 0..b.u30()? {
        b.u30()?;
        traits(b)?;
    }
    let pools = Pools { strings: &strings, ints: &ints, uints: &uints, doubles: &doubles, names: &names };
    let mut out = Vec::new();
    for _ in 0..b.u30()? {
        let method = b.u30()?;
        for _ in 0..4 {
            b.u30()?;
        }
        let size = b.idx()?;
        let code = b.bytes(size)?;
        for _ in 0..b.u30()? {
            for _ in 0..5 {
                b.u30()?;
            }
        }
        traits(b)?;
        if let Some(&i) = cinits.get(&method) {
            let arrays = literal_arrays(code, &pools)?;
            if !arrays.is_empty() {
                put(&mut out, class_names[i].clone(), arrays);
            }
        }
    }
    Some(out)
}

struct Pools<'a> {
    strings: &'a [String],
    ints: &'a [i64],
    uints: &'a [i64],
    doubles: &'a [f64],
    names: &'a [String],
}

/// `(u30 operands, pops, pushes)` for the ops `_OPS` lists.
fn op_shape(op: u8) -> Option<(u8, usize, usize)> {
    Some(match op {
        0xD0..=0xD3 | 0x20 | 0x21 | 0x28 => (0, 0, 1),
        0x30 | 0x29 => (0, 1, 0),
        0x1D => (0, 0, 0),
        0x5D | 0x5E | 0x60 | 0x62 => (1, 0, 1),
        0x66 | 0x6C | 0x80 | 0x58 => (1, 1, 1),
        0x63 => (1, 1, 0),
        0x6D => (1, 2, 0),
        _ => return None,
    })
}

fn truncate_back(stack: &mut Vec<Val>, n: usize) {
    stack.truncate(stack.len().saturating_sub(n));
}

fn literal_arrays(code: &[u8], pools: &Pools) -> Option<Ordered<Val>> {
    let b = &mut Buf { d: code, p: 0 };
    let mut stack: Vec<Val> = Vec::new();
    let mut out = Vec::new();
    while b.p < code.len() {
        let op = b.u8()?;
        match op {
            0x2C => stack.push(Val::Str(pools.strings.get(b.idx()?)?.clone())),
            0x2D => stack.push(Val::Int(*pools.ints.get(b.idx()?)?)),
            0x2E => stack.push(Val::Int(*pools.uints.get(b.idx()?)?)),
            0x2F => stack.push(Val::Float(*pools.doubles.get(b.idx()?)?)),
            0x24 => stack.push(Val::Int(b.u8()? as i8 as i64)),
            0x25 => stack.push(Val::Int(b.u30()? as i64)),
            0x26 => stack.push(Val::Bool(true)),
            0x27 => stack.push(Val::Bool(false)),
            0x56 => {
                let n = b.idx()?;
                let len = stack.len();
                let items = if n == 0 { Vec::new() } else { stack[len.saturating_sub(n)..].to_vec() };
                // `del stack[len - n:]`: a negative start counts from the end again.
                let start = if n <= len { len - n } else { (2 * len).saturating_sub(n) };
                stack.truncate(start);
                stack.push(Val::List(items));
            }
            0x68 | 0x61 => {
                let name = pools.names.get(b.idx()?)?.clone();
                let value = stack.pop();
                stack.pop();
                if let Some(Val::List(items)) = value {
                    put(&mut out, name, Val::List(items));
                }
            }
            0x2A => stack.push(stack.last().cloned().unwrap_or(Val::None)),
            0x55 => {
                let n = b.idx()?;
                truncate_back(&mut stack, n.saturating_mul(2));
                stack.push(Val::None);
            }
            0x42 => {
                let n = b.idx()?;
                truncate_back(&mut stack, n.saturating_add(1));
                stack.push(Val::None);
            }
            0x46 | 0x4A | 0x4F => {
                b.u30()?;
                let n = b.idx()?;
                truncate_back(&mut stack, n.saturating_add(1));
                if op != 0x4F {
                    stack.push(Val::None);
                }
            }
            _ => match op_shape(op) {
                Some((operands, pops, pushes)) => {
                    for _ in 0..operands {
                        b.u30()?;
                    }
                    truncate_back(&mut stack, pops);
                    stack.extend(std::iter::repeat_n(Val::None, pushes));
                }
                None => break,
            },
        }
    }
    Some(out)
}

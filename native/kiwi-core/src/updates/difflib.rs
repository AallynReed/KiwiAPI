//! `difflib.SequenceMatcher(a=, b=, autojunk=False).get_opcodes()`: no junk,
//! no popular-element pruning, so every match is found the way Python finds it.

use std::hash::Hash;

use rustc_hash::FxHashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tag {
    Equal,
    Replace,
    Delete,
    Insert,
}

impl Tag {
    pub fn as_str(self) -> &'static str {
        match self {
            Tag::Equal => "equal",
            Tag::Replace => "replace",
            Tag::Delete => "delete",
            Tag::Insert => "insert",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Opcode {
    pub tag: Tag,
    pub i1: usize,
    pub i2: usize,
    pub j1: usize,
    pub j2: usize,
}

struct Matcher {
    a: Vec<u32>,
    b: Vec<u32>,
    b2j: Vec<Vec<usize>>,
    prev: Vec<usize>,
    cur: Vec<usize>,
}

impl Matcher {
    fn new<'a, T: Eq + Hash>(a: &'a [T], b: &'a [T]) -> Self {
        let mut ids: FxHashMap<&'a T, u32> = FxHashMap::default();
        let mut id = |x: &'a T| -> u32 {
            let n = ids.len() as u32;
            *ids.entry(x).or_insert(n)
        };
        let bi: Vec<u32> = b.iter().map(&mut id).collect();
        let ai: Vec<u32> = a.iter().map(&mut id).collect();
        let n = ids.len();
        let mut b2j = vec![Vec::new(); n];
        for (j, &x) in bi.iter().enumerate() {
            b2j[x as usize].push(j);
        }
        let lb = bi.len();
        Matcher { a: ai, b: bi, b2j, prev: vec![0; lb + 1], cur: vec![0; lb + 1] }
    }

    fn find_longest_match(&mut self, alo: usize, ahi: usize, blo: usize, bhi: usize) -> (usize, usize, usize) {
        let (mut besti, mut bestj, mut bestsize) = (alo, blo, 0);
        let mut prev_touched: Vec<usize> = Vec::new();
        let mut cur_touched: Vec<usize> = Vec::new();
        for i in alo..ahi {
            for &j in &self.b2j[self.a[i] as usize] {
                if j < blo {
                    continue;
                }
                if j >= bhi {
                    break;
                }
                let k = self.prev[j] + 1;
                self.cur[j + 1] = k;
                cur_touched.push(j + 1);
                if k > bestsize {
                    besti = i + 1 - k;
                    bestj = j + 1 - k;
                    bestsize = k;
                }
            }
            for t in prev_touched.drain(..) {
                self.prev[t] = 0;
            }
            std::mem::swap(&mut self.prev, &mut self.cur);
            std::mem::swap(&mut prev_touched, &mut cur_touched);
        }
        for t in prev_touched {
            self.prev[t] = 0;
        }
        while besti > alo && bestj > blo && self.a[besti - 1] == self.b[bestj - 1] {
            besti -= 1;
            bestj -= 1;
            bestsize += 1;
        }
        while besti + bestsize < ahi && bestj + bestsize < bhi && self.a[besti + bestsize] == self.b[bestj + bestsize] {
            bestsize += 1;
        }
        (besti, bestj, bestsize)
    }

    fn matching_blocks(&mut self) -> Vec<(usize, usize, usize)> {
        let (la, lb) = (self.a.len(), self.b.len());
        let mut queue = vec![(0, la, 0, lb)];
        let mut blocks = Vec::new();
        while let Some((alo, ahi, blo, bhi)) = queue.pop() {
            let (i, j, k) = self.find_longest_match(alo, ahi, blo, bhi);
            if k > 0 {
                blocks.push((i, j, k));
                if alo < i && blo < j {
                    queue.push((alo, i, blo, j));
                }
                if i + k < ahi && j + k < bhi {
                    queue.push((i + k, ahi, j + k, bhi));
                }
            }
        }
        blocks.sort_unstable();
        let (mut i1, mut j1, mut k1) = (0, 0, 0);
        let mut out = Vec::new();
        for (i2, j2, k2) in blocks {
            if i1 + k1 == i2 && j1 + k1 == j2 {
                k1 += k2;
            } else {
                if k1 > 0 {
                    out.push((i1, j1, k1));
                }
                (i1, j1, k1) = (i2, j2, k2);
            }
        }
        if k1 > 0 {
            out.push((i1, j1, k1));
        }
        out.push((la, lb, 0));
        out
    }
}

/// `SequenceMatcher(None, a, b, autojunk=False).get_opcodes()`.
pub fn opcodes<T: Eq + Hash>(a: &[T], b: &[T]) -> Vec<Opcode> {
    let mut m = Matcher::new(a, b);
    let (mut i, mut j) = (0, 0);
    let mut out = Vec::new();
    for (ai, bj, size) in m.matching_blocks() {
        let tag = if i < ai && j < bj {
            Some(Tag::Replace)
        } else if i < ai {
            Some(Tag::Delete)
        } else if j < bj {
            Some(Tag::Insert)
        } else {
            None
        };
        if let Some(tag) = tag {
            out.push(Opcode { tag, i1: i, i2: ai, j1: j, j2: bj });
        }
        (i, j) = (ai + size, bj + size);
        if size > 0 {
            out.push(Opcode { tag: Tag::Equal, i1: ai, i2: i, j1: bj, j2: j });
        }
    }
    out
}

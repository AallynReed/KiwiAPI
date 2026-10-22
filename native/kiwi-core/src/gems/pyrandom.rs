//! CPython's `random` module: the MT19937 generator and the exact draws of
//! `random()`, `getrandbits`, `_randbelow`, `choice`, `randint` and weighted
//! `choices`, so a seeded run reproduces Python's sequence.

const N: usize = 624;
const M: usize = 397;
const MATRIX_A: u32 = 0x9908_b0df;
const UPPER: u32 = 0x8000_0000;
const LOWER: u32 = 0x7fff_ffff;

pub struct PyRandom {
    mt: [u32; N],
    index: usize,
}

impl PyRandom {
    fn genrand(s: u32) -> Self {
        let mut mt = [0u32; N];
        mt[0] = s;
        for i in 1..N {
            mt[i] = 1_812_433_253u32.wrapping_mul(mt[i - 1] ^ (mt[i - 1] >> 30)).wrapping_add(i as u32);
        }
        PyRandom { mt, index: N }
    }

    /// `init_by_array(key)`; an empty key is treated as `[0]`.
    pub fn from_key(key: &[u32]) -> Self {
        let key = if key.is_empty() { &[0u32][..] } else { key };
        let mut r = Self::genrand(19_650_218);
        let mt = &mut r.mt;
        let (mut i, mut j) = (1usize, 0usize);
        for _ in 0..N.max(key.len()) {
            mt[i] = (mt[i] ^ (mt[i - 1] ^ (mt[i - 1] >> 30)).wrapping_mul(1_664_525))
                .wrapping_add(key[j])
                .wrapping_add(j as u32);
            i += 1;
            j += 1;
            if i >= N {
                mt[0] = mt[N - 1];
                i = 1;
            }
            if j >= key.len() {
                j = 0;
            }
        }
        for _ in 0..N - 1 {
            mt[i] = (mt[i] ^ (mt[i - 1] ^ (mt[i - 1] >> 30)).wrapping_mul(1_566_083_941)).wrapping_sub(i as u32);
            i += 1;
            if i >= N {
                mt[0] = mt[N - 1];
                i = 1;
            }
        }
        mt[0] = 0x8000_0000;
        r
    }

    /// `random.seed(n)` for a non-negative int.
    pub fn seed(n: u64) -> Self {
        let (lo, hi) = (n as u32, (n >> 32) as u32);
        if hi == 0 { Self::from_key(&[lo]) } else { Self::from_key(&[lo, hi]) }
    }

    fn next_u32(&mut self) -> u32 {
        if self.index >= N {
            let mt = &mut self.mt;
            let mag = |y: u32| if y & 1 == 1 { MATRIX_A } else { 0 };
            for kk in 0..N - M {
                let y = (mt[kk] & UPPER) | (mt[kk + 1] & LOWER);
                mt[kk] = mt[kk + M] ^ (y >> 1) ^ mag(y);
            }
            for kk in N - M..N - 1 {
                let y = (mt[kk] & UPPER) | (mt[kk + 1] & LOWER);
                mt[kk] = mt[kk + M - N] ^ (y >> 1) ^ mag(y);
            }
            let y = (mt[N - 1] & UPPER) | (mt[0] & LOWER);
            mt[N - 1] = mt[M - 1] ^ (y >> 1) ^ mag(y);
            self.index = 0;
        }
        let mut y = self.mt[self.index];
        self.index += 1;
        y ^= y >> 11;
        y ^= (y << 7) & 0x9d2c_5680;
        y ^= (y << 15) & 0xefc6_0000;
        y ^ (y >> 18)
    }

    /// `random()`: 53 random bits in [0, 1).
    pub fn random(&mut self) -> f64 {
        let a = (self.next_u32() >> 5) as f64;
        let b = (self.next_u32() >> 6) as f64;
        (a * 67_108_864.0 + b) * (1.0 / 9_007_199_254_740_992.0)
    }

    /// `getrandbits(k)` for `k <= 64`.
    pub fn getrandbits(&mut self, k: u32) -> u64 {
        if k == 0 {
            return 0;
        }
        if k <= 32 {
            return (self.next_u32() >> (32 - k)) as u64;
        }
        let lo = self.next_u32() as u64;
        let hi = (self.next_u32() >> (64 - k)) as u64;
        hi << 32 | lo
    }

    /// `_randbelow(n)` for `n > 0`.
    pub fn randbelow(&mut self, n: u64) -> u64 {
        let k = 64 - n.leading_zeros();
        loop {
            let r = self.getrandbits(k);
            if r < n {
                return r;
            }
        }
    }

    /// The index `choice(seq)` picks; `None` for an empty sequence (Python's IndexError).
    pub fn choice(&mut self, len: usize) -> Option<usize> {
        (len > 0).then(|| self.randbelow(len as u64) as usize)
    }

    /// `randint(a, b)` for `a <= b`.
    pub fn randint(&mut self, a: i64, b: i64) -> i64 {
        a + self.randbelow((b - a + 1) as u64) as i64
    }

    /// The index `choices(population, weights=w)[0]` picks, or Python's
    /// ValueError message.
    pub fn weighted_choice(&mut self, weights: &[f64]) -> Result<usize, &'static str> {
        let mut cum = Vec::with_capacity(weights.len());
        let mut acc = 0.0;
        for (i, &w) in weights.iter().enumerate() {
            acc = if i == 0 { w } else { acc + w };
            cum.push(acc);
        }
        let total = cum.last().copied().unwrap_or(0.0);
        if total <= 0.0 {
            return Err("Total of weights must be greater than zero");
        }
        if !total.is_finite() {
            return Err("Total of weights must be finite");
        }
        let x = self.random() * total;
        let hi = weights.len() - 1;
        Ok(cum[..hi].partition_point(|&c| c <= x))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_cpython() {
        assert_eq!(PyRandom::seed(0).random(), 0.8444218515250481);
        assert_eq!(PyRandom::seed((1 << 40) + 5).random(), 0.5043802970418443);
        let mut r = PyRandom::seed(7);
        assert_eq!((0..5).map(|_| r.randint(0, 2)).collect::<Vec<_>>(), [1, 0, 1, 2, 0]);
        let mut r = PyRandom::seed(9);
        let w = [0.1, 0.0, 2.5, 1.0];
        assert_eq!((0..6).map(|_| r.weighted_choice(&w).unwrap()).collect::<Vec<_>>(), [2, 2, 2, 3, 0, 2]);
        assert_eq!(PyRandom::seed(3).getrandbits(40), 649_562_111_997);
    }
}

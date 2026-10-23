//! NumPy 2.4's default `np.argsort` on an AVX2 x86-64 CPU: x86-simd-sort's
//! `avx2_argsort` (is-sorted shortcut, then a key/index bitonic network of
//! 4-lane registers). Equal keys never swap, so ties land where the network
//! leaves them, which is what the batching downstream depends on.

const LANES: usize = 4;

#[derive(Clone, Copy)]
struct Kv<T> {
    k: T,
    i: usize,
}

/// A compare-exchange: `a` keeps the smaller key, swapping only on `>`.
fn ce<T: PartialOrd + Copy>(v: &mut [[Kv<T>; LANES]], a: (usize, usize), b: (usize, usize)) {
    if v[a.0][a.1].k > v[b.0][b.1].k {
        let t = v[a.0][a.1];
        v[a.0][a.1] = v[b.0][b.1];
        v[b.0][b.1] = t;
    }
}

fn sort_reg<T: PartialOrd + Copy>(v: &mut [[Kv<T>; LANES]], r: usize) {
    for (x, y) in [(0, 1), (2, 3), (0, 3), (1, 2), (0, 1), (2, 3)] {
        ce(v, (r, x), (r, y));
    }
}

fn merge_reg<T: PartialOrd + Copy>(v: &mut [[Kv<T>; LANES]], r: usize) {
    for (x, y) in [(0, 2), (1, 3), (0, 1), (2, 3)] {
        ce(v, (r, x), (r, y));
    }
}

fn coex<T: PartialOrd + Copy>(v: &mut [[Kv<T>; LANES]], a: usize, b: usize) {
    for l in 0..LANES {
        ce(v, (a, l), (b, l));
    }
}

/// `bitonic_merge_n_vec<numPer>` on the registers `base..base+per`.
fn merge_n<T: PartialOrd + Copy>(v: &mut [[Kv<T>; LANES]], base: usize, per: usize) {
    for i in 0..per / 2 {
        for l in 0..LANES {
            ce(v, (base + i, l), (base + per - 1 - i, LANES - 1 - l));
        }
    }
    let mut num = per / 2;
    while num >= 2 {
        for j in (0..per).step_by(num) {
            for i in 0..num / 2 {
                coex(v, base + i + j, base + i + j + num / 2);
            }
        }
        num /= 2;
    }
    for r in base..base + per {
        merge_reg(v, r);
    }
}

/// `argsort_n<256>` over the indices in `arg`, in place.
fn network<T: PartialOrd + Copy>(keys: &[T], arg: &mut [usize], pad: T) {
    let n = arg.len();
    let mut nvec = 256 / LANES;
    while nvec > 1 && n * 2 <= nvec * LANES {
        nvec /= 2;
    }
    let mut v: Vec<[Kv<T>; LANES]> = (0..nvec)
        .map(|r| {
            std::array::from_fn(|l| {
                let p = r * LANES + l;
                if p < n { Kv { k: keys[arg[p]], i: arg[p] } } else { Kv { k: pad, i: usize::MAX } }
            })
        })
        .collect();
    for r in 0..nvec {
        sort_reg(&mut v, r);
    }
    let mut per = 2;
    while per <= nvec {
        for g in 0..nvec / per {
            merge_n(&mut v, g * per, per);
        }
        per *= 2;
    }
    for (slot, kv) in arg.iter_mut().zip(v.iter().flatten()) {
        *slot = kv.i;
    }
}

/// `partition_vec_avx2`: lanes under the pivot first (in lane order), the
/// rest reversed at the end; written at `l` and again at `r`. Returns how
/// many were `>=` the pivot.
fn partition_vec<T: PartialOrd + Copy>(
    keys: &[T],
    arg: &mut [usize],
    l: usize,
    r: usize,
    idx: [usize; 4],
    pivot: T,
) -> usize {
    let mut tmp = [0usize; 4];
    let (mut lo, mut hi) = (0usize, 4usize);
    for &i in &idx {
        if keys[i] >= pivot {
            hi -= 1;
            tmp[hi] = i;
        } else {
            tmp[lo] = i;
            lo += 1;
        }
    }
    arg[l..l + 4].copy_from_slice(&tmp);
    arg[r..r + 4].copy_from_slice(&tmp);
    4 - lo
}

fn load(arg: &[usize], at: usize) -> [usize; 4] {
    [arg[at], arg[at + 1], arg[at + 2], arg[at + 3]]
}

/// The scalar prefix both partitions start with.
fn scalar_prefix<T: PartialOrd + Copy>(
    keys: &[T],
    arg: &mut [usize],
    left: &mut usize,
    right: &mut usize,
    count: usize,
    pivot: T,
) {
    for _ in 0..count {
        if keys[arg[*left]].partial_cmp(&pivot) != Some(std::cmp::Ordering::Less) {
            *right -= 1;
            arg.swap(*left, *right);
        } else {
            *left += 1;
        }
    }
}

/// `argpartition` / `argpartition_unrolled<4>` over `arg[left..right]`.
fn partition<T: PartialOrd + Copy>(
    keys: &[T],
    arg: &mut [usize],
    mut left: usize,
    mut right: usize,
    pivot: T,
) -> usize {
    let unroll = if right - left <= 8 * 4 * LANES { 1 } else { 4 };
    let block = unroll * LANES;
    let count = (right - left) % block;
    scalar_prefix(keys, arg, &mut left, &mut right, count, pivot);
    if left == right {
        return left;
    }
    if unroll == 1 && right - left == LANES {
        let v = load(arg, left);
        return left + LANES - partition_vec(keys, arg, left, left, v, pivot);
    }
    let lefts: Vec<[usize; 4]> = (0..unroll).map(|k| load(arg, left + LANES * k)).collect();
    let rights: Vec<[usize; 4]> = (0..unroll).map(|k| load(arg, right - LANES * (unroll - k))).collect();
    let (mut r_store, mut l_store) = (right - LANES, left);
    left += block;
    right -= block;
    while right != left {
        let at = if (r_store + LANES) - right < left - l_store {
            right -= block;
            right
        } else {
            left += block;
            left - block
        };
        let vecs: Vec<[usize; 4]> = (0..unroll).map(|k| load(arg, at + k * LANES)).collect();
        for v in vecs {
            let ge = partition_vec(keys, arg, l_store, r_store, v, pivot);
            l_store += LANES - ge;
            r_store -= ge;
        }
    }
    if unroll == 1 {
        let ge = partition_vec(keys, arg, l_store, r_store, lefts[0], pivot);
        l_store += LANES - ge;
        let ge = partition_vec(keys, arg, l_store, l_store, rights[0], pivot);
        return l_store + LANES - ge;
    }
    for v in lefts.into_iter().chain(rights) {
        let ge = partition_vec(keys, arg, l_store, r_store, v, pivot);
        l_store += LANES - ge;
        r_store -= ge;
    }
    l_store
}

/// `argsort_` (no OpenMP) over `arg[left..=right]`.
fn quicksort<T: PartialOrd + Copy>(
    keys: &[T],
    arg: &mut [usize],
    left: usize,
    right: usize,
    iters: i64,
    pad: T,
    lowest: T,
) {
    if iters <= 0 {
        // `std::sort` fallback; ties here are not reproduced.
        arg[left..=right].sort_by(|&a, &b| keys[a].partial_cmp(&keys[b]).unwrap_or(std::cmp::Ordering::Equal));
        return;
    }
    if right + 1 - left <= 256 {
        network(keys, &mut arg[left..=right], pad);
        return;
    }
    let size = (right - left) / 4;
    let mut s: Vec<T> = (1..=4).map(|k| keys[arg[left + k * size]]).collect();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let pivot = s[2];
    let (mut smallest, mut biggest) = (pad, lowest);
    for &i in &arg[left..=right] {
        if keys[i] < smallest {
            smallest = keys[i];
        }
        if keys[i] > biggest {
            biggest = keys[i];
        }
    }
    let p = partition(keys, arg, left, right + 1, pivot);
    if pivot != smallest {
        quicksort(keys, arg, left, p - 1, iters - 1, pad, lowest);
    }
    if pivot != biggest {
        quicksort(keys, arg, p, right, iters - 1, pad, lowest);
    }
}

/// `np.argsort(keys)` for NaN-free keys; `pad`/`lowest` are the type's
/// extremes (masked lanes load `pad`).
pub fn argsort<T: PartialOrd + Copy>(keys: &[T], pad: T, lowest: T) -> Vec<usize> {
    let n = keys.len();
    let mut arg: Vec<usize> = (0..n).collect();
    if n <= 1 || keys.windows(2).all(|w| w[1].partial_cmp(&w[0]) != Some(std::cmp::Ordering::Less)) {
        return arg;
    }
    let iters = 2 * (n as f64).log2() as i64;
    quicksort(keys, &mut arg, 0, n - 1, iters, pad, lowest);
    arg
}

pub fn argsort_f64(keys: &[f64]) -> Vec<usize> {
    argsort(keys, f64::INFINITY, f64::NEG_INFINITY)
}

pub fn argsort_i32(keys: &[i32]) -> Vec<usize> {
    argsort(keys, i32::MAX, i32::MIN)
}

//! Python number semantics the gem code depends on: values that stay `int`
//! until a float joins them, `sum()` as CPython 3.12/3.13 computes it, and the
//! `min(x, 1)` that hands back the int.

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PyNum {
    Int(i64),
    Float(f64),
}

impl PyNum {
    pub fn f64(self) -> f64 {
        match self {
            PyNum::Int(i) => i as f64,
            PyNum::Float(f) => f,
        }
    }
}

/// `sum(items)`: an exact int prefix, then Neumaier compensation for float
/// terms only (int terms after the first float are added plainly, as before
/// CPython 3.14).
pub fn py_sum(items: impl IntoIterator<Item = PyNum>) -> PyNum {
    let mut it = items.into_iter();
    let mut acc: i64 = 0;
    let mut f = loop {
        match it.next() {
            None => return PyNum::Int(acc),
            Some(PyNum::Int(i)) => acc = acc.saturating_add(i),
            Some(PyNum::Float(x)) => break acc as f64 + x,
        }
    };
    let mut c = 0.0;
    for item in it {
        match item {
            PyNum::Float(x) => {
                let t = f + x;
                if f.abs() >= x.abs() {
                    c += (f - t) + x;
                } else {
                    c += (x - t) + f;
                }
                f = t;
            }
            PyNum::Int(i) => f += i as f64,
        }
    }
    if c != 0.0 && c.is_finite() {
        f += c;
    }
    PyNum::Float(f)
}

/// `sum()` over floats.
pub fn py_fsum(items: impl IntoIterator<Item = f64>) -> f64 {
    py_sum(items.into_iter().map(PyNum::Float)).f64()
}

/// `min(x, 1)`: the int 1 when `x` is larger, else `x`.
pub fn min_one(x: f64) -> PyNum {
    if 1.0 < x { PyNum::Int(1) } else { PyNum::Float(x) }
}

/// `min(1.0, x)`.
pub fn cap_one(x: f64) -> f64 {
    if x < 1.0 { x } else { 1.0 }
}

/// `max(lo, x)`: `x` only when strictly larger.
pub fn py_max(lo: f64, x: f64) -> f64 {
    if x > lo { x } else { lo }
}

/// `max(0.0, min(1.0, x))`.
pub fn clamp01(x: f64) -> f64 {
    py_max(0.0, cap_one(x))
}

/// Python `round(x)` to an int (half to even); `None` where Python raises
/// (NaN, infinity, outside `i64`).
pub fn round_int(x: f64) -> Option<i64> {
    let r = x.round_ties_even();
    (r.is_finite() && r >= i64::MIN as f64 && r < i64::MAX as f64).then_some(r as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sum_like_cpython_313() {
        assert_eq!(py_sum([]), PyNum::Int(0));
        assert_eq!(py_sum([PyNum::Int(2), PyNum::Int(3)]), PyNum::Int(5));
        assert_eq!(py_fsum([0.1, 1e16, -1e16]), 0.1);
        assert_eq!(py_sum([PyNum::Float(0.1), PyNum::Int(10_i64.pow(16)), PyNum::Int(-(10_i64.pow(16)))]).f64(), 0.0);
        assert_eq!(py_fsum([0.1; 10]), 1.0);
    }

    #[test]
    fn min_and_round() {
        assert_eq!(min_one(1.5), PyNum::Int(1));
        assert_eq!(min_one(1.0), PyNum::Float(1.0));
        assert_eq!(cap_one(2.0), 1.0);
        assert_eq!(round_int(2.5), Some(2));
        assert_eq!(round_int(-3.5), Some(-4));
        assert_eq!(round_int(f64::NAN), None);
    }
}

//! CPython 3.13 `math` functions whose results differ from libm's.

/// `math.hypot(x, y)`: CPython's `vector_norm` (compensated, with a
/// differential correction), not C `hypot`.
pub fn hypot(x: f64, y: f64) -> f64 {
    let v = [x.abs(), y.abs()];
    let max = v[0].max(v[1]);
    if max.is_infinite() {
        return max;
    }
    if x.is_nan() || y.is_nan() {
        return f64::NAN;
    }
    if max == 0.0 {
        return max;
    }
    norm(&v, max)
}

fn dl_fast_sum(a: f64, b: f64) -> (f64, f64) {
    let x = a + b;
    (x, (a - x) + b)
}

fn dl_mul(x: f64, y: f64) -> (f64, f64) {
    let z = x * y;
    (z, x.mul_add(y, -z))
}

fn norm(vec: &[f64], max: f64) -> f64 {
    let max_e = frexp_exp(max);
    if max_e < -1023 {
        let scaled: Vec<f64> = vec.iter().map(|v| v / f64::MIN_POSITIVE).collect();
        return f64::MIN_POSITIVE * norm(&scaled, max / f64::MIN_POSITIVE);
    }
    let scale = if max_e >= 0 { 0.5f64.powi(max_e) } else { 2f64.powi(-max_e) };
    let (mut csum, mut frac1, mut frac2) = (1.0f64, 0.0f64, 0.0f64);
    for &v in vec {
        let x = v * scale;
        let pr = dl_mul(x, x);
        let sm = dl_fast_sum(csum, pr.0);
        csum = sm.0;
        frac1 += pr.1;
        frac2 += sm.1;
    }
    let mut h = (csum - 1.0 + (frac1 + frac2)).sqrt();
    let pr = dl_mul(-h, h);
    let sm = dl_fast_sum(csum, pr.0);
    csum = sm.0;
    frac1 += pr.1;
    frac2 += sm.1;
    let x = csum - 1.0 + (frac1 + frac2);
    h += x / (2.0 * h);
    h / scale
}

/// The exponent `frexp` returns (`max = m * 2**e`, 0.5 <= m < 1).
fn frexp_exp(v: f64) -> i32 {
    let bits = v.to_bits();
    let exp = ((bits >> 52) & 0x7ff) as i32;
    if exp == 0 {
        let lz = (bits << 12).leading_zeros() as i32;
        -1022 - lz
    } else {
        exp - 1022
    }
}

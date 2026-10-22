//! Python float text and rounding.
//!
//! - [`repr`]: `repr(float)`, which is what `json.dumps` writes.
//! - [`pydantic`]: pydantic-core `dump_json` (ryu layout: decimal for
//!   `1e-5 <= |x| < 1e16`, else `1e+16` / `1.5e-7`).
//! - [`display`]: the number inside pydantic error messages (Rust `Display`).
//! - [`round`] / [`round_int`]: Python `round(x, n)` / `round(x)`.

use std::fmt::Write;

/// Mantissa digits and decimal point position of a `{:e}`-formatted number.
fn split_exp(s: &str) -> (String, i32) {
    let (mant, exp) = s.split_once('e').expect("LowerExp always has an exponent");
    let exp: i32 = exp.parse().expect("LowerExp exponent is an integer");
    (mant.replace('.', ""), exp + 1)
}

/// Shortest round-trip digits of `|x|` and the decimal point position:
/// `|x| = 0.DIGITS * 10^decpt`. Zero gives `("0", 1)`.
fn shortest(x: f64) -> (String, i32) {
    let x = x.abs();
    let (digits, decpt) = split_exp(&format!("{x:e}"));
    match even_tie(x, &digits, decpt) {
        Some(even) => (even, decpt),
        None => (digits, decpt),
    }
}

/// When `x` sits exactly halfway between two shortest candidates, CPython's
/// dtoa and ryu keep the even last digit; Rust's formatter rounds up.
fn even_tie(x: f64, d: &str, decpt: i32) -> Option<String> {
    if (d.as_bytes().last()? - b'0').is_multiple_of(2) {
        return None;
    }
    let bits = x.to_bits();
    let biased = ((bits >> 52) & 0x7ff) as i32;
    let frac = bits & ((1u64 << 52) - 1);
    let (m, e) = if biased == 0 { (frac, -1074) } else { (frac | 1 << 52, biased - 1075) };
    if m == 0 {
        return None;
    }
    // x = odd / 2^j exactly, i.e. the digits of odd * 5^j shifted j places.
    // A tie has one digit more than the shortest form (at most 18), which
    // rules out integers and every j > 25.
    let tz = m.trailing_zeros();
    let j = -(e + tz as i32);
    if !(1..=25).contains(&j) {
        return None;
    }
    let exact = ((m >> tz) as u128 * 5u128.pow(j as u32)).to_string();
    let n = d.len();
    if exact.len() != n + 1 || exact.len() as i32 - j != decpt {
        return None;
    }
    let mut lo = exact.as_bytes()[..n].to_vec();
    match lo[n - 1] - b'0' {
        9 => return None,
        l if !l.is_multiple_of(2) => lo[n - 1] += 1,
        _ => {}
    }
    while lo.len() > 1 && lo.last() == Some(&b'0') {
        lo.pop();
    }
    String::from_utf8(lo).ok()
}

fn push_zeros(out: &mut String, n: i32) {
    for _ in 0..n.max(0) {
        out.push('0');
    }
}

/// Python `repr(x)`: shortest round-trip digits, exponent form when the
/// decimal exponent is below -4 or at least 16 (`1e-05`, `1e+16`), and a
/// trailing `.0` on integral values.
pub fn repr(x: f64) -> String {
    if x.is_nan() {
        return "nan".into();
    }
    if x.is_infinite() {
        return if x > 0.0 { "inf" } else { "-inf" }.into();
    }
    let (d, decpt) = shortest(x);
    let n = d.len() as i32;
    let mut out = String::with_capacity(d.len() + 8);
    if x.is_sign_negative() {
        out.push('-');
    }
    if decpt <= -4 || decpt > 16 {
        out.push_str(&d[..1]);
        if n > 1 {
            out.push('.');
            out.push_str(&d[1..]);
        }
        let e = decpt - 1;
        let _ = write!(out, "e{}{:02}", if e < 0 { '-' } else { '+' }, e.abs());
    } else if decpt <= 0 {
        out.push_str("0.");
        push_zeros(&mut out, -decpt);
        out.push_str(&d);
    } else if decpt >= n {
        out.push_str(&d);
        push_zeros(&mut out, decpt - n);
        out.push_str(".0");
    } else {
        out.push_str(&d[..decpt as usize]);
        out.push('.');
        out.push_str(&d[decpt as usize..]);
    }
    out
}

/// pydantic-core `dump_json` float text. Non-finite values serialize as
/// `null` (pydantic's default `ser_json_inf_nan='null'`).
pub fn pydantic(x: f64) -> String {
    if !x.is_finite() {
        return "null".into();
    }
    let (d, kk) = shortest(x);
    let n = d.len() as i32;
    let k = kk - n;
    let mut out = String::with_capacity(d.len() + 8);
    if x.is_sign_negative() {
        out.push('-');
    }
    if k >= 0 && kk <= 16 {
        out.push_str(&d);
        push_zeros(&mut out, k);
        out.push_str(".0");
    } else if (1..=16).contains(&kk) {
        out.push_str(&d[..kk as usize]);
        out.push('.');
        out.push_str(&d[kk as usize..]);
    } else if (-4..=0).contains(&kk) {
        out.push_str("0.");
        push_zeros(&mut out, -kk);
        out.push_str(&d);
    } else {
        out.push_str(&d[..1]);
        if n > 1 {
            out.push('.');
            out.push_str(&d[1..]);
        }
        let e = kk - 1;
        let _ = write!(out, "e{}{}", if e < 0 { "-" } else { "+" }, e.abs());
    }
    out
}

/// A number as pydantic prints it inside an error message
/// (`Input should be less than or equal to 1` for a `le=1.0` constraint).
pub fn display(x: f64) -> String {
    format!("{x}")
}

/// Python `round(x, ndigits)` for `ndigits >= 0`: the exact binary value
/// rounded half-to-even to `ndigits` decimals, read back as a float.
pub fn round(x: f64, ndigits: u32) -> f64 {
    if !x.is_finite() || ndigits > 323 {
        return x;
    }
    format!("{x:.*}", ndigits as usize).parse().unwrap_or(x)
}

/// Python `round(x)` (no `ndigits`): nearest integer, ties to even
/// (saturating outside the `i64` range, where Python returns a big int).
pub fn round_int(x: f64) -> i64 {
    x.round_ties_even() as i64
}

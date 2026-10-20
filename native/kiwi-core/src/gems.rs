//! The gem build ranking loop of `app/trove/gems/builds.py`
//! (`GemOptimizerEngine.calculate_builds`), float for float.

/// `[(base, per boost)]` for the first, second and third stat lines, Empowered then Lesser.
pub struct GemLines {
    pub empowered: [[f64; 2]; 3],
    pub lesser: [[f64; 2]; 3],
}

/// The build's stat totals before gems (`first` .. `sixth` in Python).
pub struct Base {
    pub first: f64,
    pub second: f64,
    pub third: f64,
    pub fourth: f64,
    pub fifth: f64,
    pub sixth: f64,
}

/// One ranked layout: the ten boost counts in Python's flattened order, then
/// `cfirst`, `csecond`, rounded light, `final` and the rounded coefficient.
pub type Ranked = ([u8; 10], f64, f64, f64, f64, f64);

/// Python's `round(value, digits)`: the exact binary value rounded half-even.
pub fn py_round(value: f64, digits: usize) -> f64 {
    const POW10: [f64; 9] = [1.0, 1e1, 1e2, 1e3, 1e4, 1e5, 1e6, 1e7, 1e8];
    if !value.is_finite() {
        return value;
    }
    // value * 10^d lands within half an ulp of the true product, so unless that
    // product sits next to a .5 the nearest integer is the right one, and n / 10^d
    // (both exact) is the double the decimal string parses to.
    if let Some(&p) = POW10.get(digits) {
        let y = value * p;
        if y.abs() < 4.0e15 {
            let f = y.floor();
            let frac = y - f;
            if (frac - 0.5).abs() > y.abs() * 2.5e-16 + 1e-300 {
                let n = if frac > 0.5 { f + 1.0 } else { f };
                return (n / p).copysign(value);
            }
        }
    }
    format!("{value:.digits$}").parse().unwrap_or(value)
}

/// Python's float ordering for a sort key: -0.0 equals 0.0.
fn cmp(a: f64, b: f64) -> std::cmp::Ordering {
    a.partial_cmp(&b).unwrap_or(std::cmp::Ordering::Equal)
}

fn combinations(farm: bool) -> Vec<[u8; 10]> {
    let mut third = Vec::new();
    for x in 0..4u8 {
        for y in 0..4u8 {
            for z in 0..4u8 {
                if x + y + z == 3 && (farm || z == 3) {
                    third.push([x, y, z]);
                }
            }
        }
    }
    let mut fourth = Vec::new();
    for x in 0..7u8 {
        for y in 0..7u8 {
            for z in 0..7u8 {
                if x + y + z == 6 && (farm || z == 6) {
                    fourth.push([x, y, z]);
                }
            }
        }
    }
    let mut out = Vec::with_capacity(190 * third.len() * fourth.len());
    for i in 0..10u8 {
        for j in 0..19u8 {
            for t in &third {
                for f in &fourth {
                    out.push([i, 9 - i, j, 18 - j, t[0], t[1], t[2], f[0], f[1], f[2]]);
                }
            }
        }
    }
    out
}

fn gem_stats(g: &GemLines, b: &[u8; 10]) -> (f64, f64, f64) {
    let [fe, se, te] = g.empowered;
    let [fl, sl, tl] = g.lesser;
    let n = |i: usize| b[i] as f64;
    let mut first = 3.0 * fe[0] + 6.0 * fl[0];
    let mut second = 3.0 * se[0] + 6.0 * sl[0];
    let mut third = 1.0 * te[0] + 2.0 * tl[0];
    let mut cosmic_first = 1.0 * fe[0] + 2.0 * fl[0];
    let mut cosmic_second = 1.0 * se[0] + 2.0 * sl[0];
    first += fe[1] * n(0);
    second += se[1] * n(1);
    first += fl[1] * n(2);
    second += sl[1] * n(3);
    cosmic_first += fe[1] * n(4);
    cosmic_second += se[1] * n(5);
    third += te[1] * n(6);
    cosmic_first += fl[1] * n(7);
    cosmic_second += sl[1] * n(8);
    third += tl[1] * n(9);
    ((first + cosmic_first) * 1.1, (second + cosmic_second) * 1.1, third * 1.1)
}

/// The top `limit` layouts, ranked exactly as `calculate_builds` sorts them.
/// `gems` None is Python's missing gem data (every gem line reads 0).
pub fn rank(
    gems: Option<&GemLines>,
    base: &Base,
    class_bonus: Option<f64>,
    farm: bool,
    light_target: i64,
    precise: bool,
    limit: usize,
) -> Vec<Ranked> {
    let rd = |v: f64, d: usize| py_round(v, if precise { 8 } else { d });
    let mut rows: Vec<Ranked> = combinations(farm)
        .into_iter()
        .map(|b| {
            let (gf, gs, gt) = gems.map_or((0.0, 0.0, 0.0), |g| gem_stats(g, &b));
            let cfirst = base.first + gf;
            let csecond = base.second + gs;
            let cthird = base.third + gt;
            let mut fin = cfirst * (1.0 + base.fourth / 100.0);
            if let Some(cb) = class_bonus {
                fin *= 1.0 + cb / 100.0;
            }
            let coefficient = rd(fin * (1.0 + (csecond * (base.fifth / 100.0)) / 100.0), 2);
            (b, cfirst, csecond, rd(cthird * (base.sixth / 100.0), 2), fin, coefficient)
        })
        .collect();
    if light_target != 0 {
        let t = light_target as f64;
        rows.sort_by(|a, b| cmp((a.3 - t).abs(), (b.3 - t).abs()).then(cmp(b.5, a.5)));
    } else {
        rows.sort_by(|a, b| cmp(b.5, a.5));
    }
    rows.truncate(limit);
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounds_like_python() {
        assert_eq!(py_round(0.125, 2), 0.12);
        assert_eq!(py_round(0.375, 2), 0.38);
        assert_eq!(py_round(2.675, 2), 2.67);
        assert_eq!(py_round(-1.005, 2), -1.0);
        assert_eq!(py_round(12345.678901234, 8), 12345.67890123);
    }

    #[test]
    fn fast_round_matches_exact() {
        let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
        for i in 0..2_000_000u64 {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            let scale = [1.0, 1e3, 1e6, 1e-3][(i % 4) as usize];
            let v = ((x >> 11) as f64 / (1u64 << 53) as f64 - 0.5) * scale;
            let v = if i % 5 == 0 { (v * 1000.0).round() / 1000.0 + 0.005 } else { v };
            for d in [1, 2, 8] {
                let exact: f64 = format!("{v:.d$}").parse().unwrap();
                assert_eq!(py_round(v, d).to_bits(), exact.to_bits(), "{v} {d}");
            }
        }
    }

    #[test]
    fn farm_has_every_layout() {
        assert_eq!(combinations(true).len(), 10 * 19 * 10 * 28);
        assert_eq!(combinations(false).len(), 190);
    }
}

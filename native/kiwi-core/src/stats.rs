//! `app/trove/stats.py` math: the character Coefficient and the sigil tiers.

use crate::gems::py_round;

/// `COEFFICIENT_FORMULA`, the text the API returns with every result.
pub const COEFFICIENT_FORMULA: &str = "floor(max(physical_damage, magic_damage) * (1 + critical_damage / 100))";

/// `compute_coefficient` before its `int()`: `round(D * (1 + crit / 100), 6)`
/// with `D` the higher damage (physical on a tie), or `None` without one.
pub fn coefficient(physical: Option<f64>, magic: Option<f64>, crit: f64) -> Option<(f64, &'static str)> {
    let mut best: Option<(f64, &'static str)> = None;
    for (v, name) in [(physical, "physical"), (magic, "magic")] {
        if let Some(v) = v
            && best.is_none_or(|(b, _)| v > b)
        {
            best = Some((v, name));
        }
    }
    let (base, which) = best?;
    Some((py_round(base * (1.0 + crit / 100.0), 6), which))
}

/// `str(int(x))`: truncated toward zero, exact at any magnitude; `None`
/// where Python raises (inf, nan).
pub fn py_int_digits(x: f64) -> Option<String> {
    if !x.is_finite() {
        return None;
    }
    let t = x.trunc();
    if t.abs() < 9.0e18 {
        return Some((t as i64).to_string());
    }
    // |t| = mant * 2^exp exactly, in base-1e9 limbs (least significant first).
    const BASE: u64 = 1_000_000_000;
    let bits = t.abs().to_bits();
    let mut exp = ((bits >> 52) & 0x7ff) as i32 - 1075;
    let mant = (bits & ((1 << 52) - 1)) | (1 << 52);
    let mut limbs = vec![mant % BASE, mant / BASE % BASE, mant / BASE / BASE];
    while exp > 0 {
        let shift = exp.min(32);
        let mut carry = 0u64;
        for limb in limbs.iter_mut() {
            let v = (*limb << shift) + carry;
            *limb = v % BASE;
            carry = v / BASE;
        }
        while carry > 0 {
            limbs.push(carry % BASE);
            carry /= BASE;
        }
        exp -= shift;
    }
    while limbs.len() > 1 && limbs.last() == Some(&0) {
        limbs.pop();
    }
    let mut out = String::from(if t < 0.0 { "-" } else { "" });
    let mut iter = limbs.iter().rev();
    out.push_str(&iter.next().map(u64::to_string).unwrap_or_default());
    for limb in iter {
        out.push_str(&format!("{limb:09}"));
    }
    Some(out)
}

/// A sigil tier's `from` threshold as Python compares it (`True` is 1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Threshold {
    Int(i64),
    Float(f64),
}

/// `_tier`: `max(i for i, t in enumerate(tiers) if t["from"] <= value)`,
/// `None` when no tier qualifies (Python's `max()` of nothing).
pub fn tier(thresholds: &[Threshold], value: i64) -> Option<usize> {
    thresholds.iter().rposition(|t| match *t {
        Threshold::Int(from) => from <= value,
        Threshold::Float(from) => from <= value as f64,
    })
}

/// `_BOARD_CLASS_ORDER`: class index -> tech_name in the game's release order
/// (boards `1000+i` Power Rank, `4000+i` Effort, `5000+i` Paragon). Verbatim.
pub const BOARD_CLASS_ORDER: [&str; 18] = [
    "knight",
    "gunslinger",
    "faetrickster",
    "dracolyte",
    "neonninja",
    "candybarbarian",
    "icemage",
    "shadowhunter",
    "piratelord",
    "adventurer",
    "tombraiser",
    "lunarlancer",
    "spirittank",
    "chloromancer",
    "dinotamer",
    "crimefighter",
    "bard",
    "solarion",
];

pub const POWER_RANK_BASE: i64 = 1000;
pub const EFFORT_BASE: i64 = 4000;
pub const PARAGON_BASE: i64 = 5000;

/// `class_index_for_board`: `uuid % 1000` (Python's floor modulo).
pub fn class_index_for_board(uuid: i64) -> i64 {
    uuid.rem_euclid(1000)
}

/// `class_board_uuids`: every Effort board, then every Paragon board.
pub fn class_board_uuids() -> Vec<i64> {
    let n = BOARD_CLASS_ORDER.len() as i64;
    (0..n).map(|i| EFFORT_BASE + i).chain((0..n).map(|i| PARAGON_BASE + i)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coefficient_picks_higher_damage() {
        assert_eq!(coefficient(Some(100.0), Some(50.0), 100.0), Some((200.0, "physical")));
        assert_eq!(coefficient(Some(50.0), Some(100.0), 0.0), Some((100.0, "magic")));
        assert_eq!(coefficient(Some(7.0), Some(7.0), 0.0), Some((7.0, "physical")));
        assert_eq!(coefficient(None, Some(3.0), 0.0), Some((3.0, "magic")));
        assert_eq!(coefficient(None, None, 10.0), None);
    }

    #[test]
    fn int_digits() {
        assert_eq!(py_int_digits(-0.0).as_deref(), Some("0"));
        assert_eq!(py_int_digits(12.9).as_deref(), Some("12"));
        assert_eq!(py_int_digits(-12.9).as_deref(), Some("-12"));
        assert_eq!(py_int_digits(1e20).as_deref(), Some("100000000000000000000"));
        assert_eq!(py_int_digits(2f64.powi(70)).as_deref(), Some("1180591620717411303424"));
        assert_eq!(py_int_digits(f64::INFINITY), None);
        assert_eq!(py_int_digits(f64::MAX).map(|s| s.len()), Some(309));
    }

    #[test]
    fn tiers() {
        let t = [Threshold::Int(0), Threshold::Int(10), Threshold::Float(30.5)];
        assert_eq!(tier(&t, 0), Some(0));
        assert_eq!(tier(&t, 30), Some(1));
        assert_eq!(tier(&t, 31), Some(2));
        assert_eq!(tier(&[Threshold::Int(5)], 4), None);
    }
}

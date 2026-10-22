//! `app/trove/gems/evaluator.py`: score a typed-in gem and cost out
//! perfecting it.

use super::catalog::{self as cat, COSMIC, EMPOWERED, LIGHT, MAGIC_DAMAGE, PHYSICAL_DAMAGE, WATER};
use super::pynum::{clamp01, py_max, round_int};
use super::upgrades::Upgrades;
use super::{GemError, py_round};

const PROGRESS_TOLERANCE: f64 = 0.05;

/// `FOCUS_OPTIONS`: key, label, focus values, focus keys.
const FOCUS_OPTIONS: [(&str, &str, &[f64], &[&str]); 3] = [
    ("optimized_all", "Superior + Precise + Rough", &[12.5, 5.0, 2.5], &["superior", "precise", "rough"]),
    ("optimized_precise_rough", "Precise + Rough", &[5.0, 2.5], &["precise", "rough"]),
    ("rough_only", "Rough Only", &[2.5], &["rough"]),
];

/// `FOCUS_RECIPES[focus]["materials"]`, in order.
fn recipe(focus: usize) -> &'static [(&'static str, i64)] {
    match focus {
        0 => &[("bound_brilliance", 1), ("diamond_dragonite", 30), ("titan_soul", 3), ("flux", 50000)],
        1 => &[
            ("bound_brilliance", 1),
            ("water_gem_dust", 3000),
            ("air_gem_dust", 3000),
            ("fire_gem_dust", 3000),
            ("flux", 2000),
        ],
        _ => &[("bound_brilliance", 1), ("heart_of_darkness", 4), ("flux", 1200)],
    }
}

/// Focus counts (superior, precise, rough) and the materials they take.
#[derive(Clone, Debug, PartialEq)]
pub struct FocusCost {
    pub key: &'static str,
    pub label: &'static str,
    pub counts: [i64; 3],
    pub recipe_totals: Vec<(&'static str, i64)>,
}

impl FocusCost {
    pub fn total(&self) -> i64 {
        self.counts.iter().fold(0i64, |a, &c| a.saturating_add(c))
    }
}

fn focus_index(key: &str) -> usize {
    match key {
        "superior" => 0,
        "precise" => 1,
        _ => 2,
    }
}

fn add_material(totals: &mut Vec<(&'static str, i64)>, key: &'static str, amount: i64) {
    match totals.iter_mut().find(|e| e.0 == key) {
        Some(e) => e.1 = e.1.saturating_add(amount),
        None => totals.push((key, amount)),
    }
}

/// `_calculate_focus_counts`.
fn focus_counts(remaining_percent: f64, values: &[f64], keys: &[&str]) -> [i64; 3] {
    let mut counts = [0i64; 3];
    let mut units = ((remaining_percent - 1e-9) / 2.5).ceil().max(0.0) as i64;
    let unit_values: Vec<i64> = values.iter().map(|v| (v / 2.5).round_ties_even() as i64).collect();
    for (key, &unit) in keys.iter().zip(&unit_values) {
        counts[focus_index(key)] = units.div_euclid(unit);
        units = units.rem_euclid(unit);
    }
    if units > 0 {
        let last = unit_values[unit_values.len() - 1];
        counts[focus_index(keys[keys.len() - 1])] += (units as f64 / last as f64).ceil() as i64;
    }
    counts
}

/// `_build_focus_plan(...)["totals"]`: per option, the counts for every container.
fn focus_plan(display_progress: f64, containers: i64) -> Vec<FocusCost> {
    let current = py_round(display_progress * 100.0, 2);
    let remaining = py_max(0.0, 100.0 - current);
    FOCUS_OPTIONS
        .iter()
        .map(|&(key, label, values, keys)| {
            let per = focus_counts(remaining, values, keys);
            let counts = per.map(|c| c.saturating_mul(containers));
            let mut recipe_totals = Vec::new();
            for (f, &count) in counts.iter().enumerate() {
                if count <= 0 {
                    continue;
                }
                for &(mat, amount) in recipe(f) {
                    add_material(&mut recipe_totals, mat, amount.saturating_mul(count));
                }
            }
            FocusCost { key, label, counts, recipe_totals }
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq)]
pub struct EvalStat {
    pub stat: u8,
    pub entered_value: f64,
    pub extra_containers: i64,
    pub containers: i64,
    pub progress: f64,
    pub quality_percent: f64,
    pub estimated_pr_contribution: f64,
    pub is_within_range: bool,
    pub raw_progress: f64,
    pub threshold_progress: f64,
    focus: Vec<FocusCost>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Evaluation {
    pub tier: u8,
    pub gem_type: u8,
    pub element: u8,
    pub restriction: Option<u8>,
    pub level: i64,
    pub quality: f64,
    pub quality_percent: f64,
    pub calculated_power_rank: i64,
    pub issues: Vec<String>,
    pub stats: Vec<EvalStat>,
    pub focus_totals: Vec<FocusCost>,
    pub available_extra_containers: i64,
    pub guessed_distribution: Vec<i64>,
}

/// One entered stat: type id, the in-game value, extra containers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StatInput {
    pub stat: i64,
    pub value: f64,
    pub extra_containers: i64,
}

fn zero_div() -> GemError {
    GemError::ZeroDivision("float division by zero".into())
}

fn candidate(
    data: &Upgrades,
    tier: u8,
    gem_type: u8,
    level: i64,
    stats: &[(u8, f64, i64)],
) -> Result<Evaluation, GemError> {
    let element = if stats.iter().any(|s| s.0 == LIGHT) { COSMIC } else { WATER };
    let restriction = if stats.iter().any(|s| s.0 == PHYSICAL_DAMAGE) {
        Some(cat::FIERCE)
    } else if stats.iter().any(|s| s.0 == MAGIC_DAMAGE) {
        Some(2)
    } else {
        None
    };
    let increments = data.increment_total(tier, level)?;
    let mut estimated = (if gem_type == EMPOWERED { 100 } else { 0 } + increments * 3) as f64;
    let [p0, p1] = cat::pr_threshold(tier, gem_type);
    let (mut progress_sum, mut total_containers) = (0.0, 0i64);
    let mut per_stat = Vec::with_capacity(stats.len());
    let mut issues = Vec::new();
    for &(stat, value, extra) in stats {
        let containers = extra.saturating_add(1);
        let base = data.stat_base(tier, gem_type, element, stat)?;
        let [t0, t1] = data.threshold(tier, gem_type, element, stat)?;
        if base == 0.0 {
            return Err(zero_div());
        }
        let threshold_progress = ((value / base) - increments as f64) / containers as f64;
        if t1 - t0 == 0.0 {
            return Err(zero_div());
        }
        let raw = (threshold_progress - t0) / (t1 - t0);
        let display = clamp01(raw);
        let contribution = (p0 as f64 + (p1 - p0) as f64 * display) * containers as f64;
        estimated += contribution;
        progress_sum += display * containers as f64;
        total_containers = total_containers.saturating_add(containers);
        let name = cat::name(&cat::STATS, stat);
        if raw < -PROGRESS_TOLERANCE {
            issues.push(format!("{name} is below the minimum possible value for this level and proc spread."));
        } else if raw > 1.0 + PROGRESS_TOLERANCE {
            issues.push(format!("{name} is above the maximum possible value for this level and proc spread."));
        }
        per_stat.push(EvalStat {
            stat,
            entered_value: value,
            extra_containers: extra,
            containers,
            progress: py_round(display, 4),
            quality_percent: py_round(display * 100.0, 2),
            estimated_pr_contribution: py_round(contribution, 2),
            is_within_range: (-PROGRESS_TOLERANCE..=1.0 + PROGRESS_TOLERANCE).contains(&raw),
            raw_progress: py_round(raw, 4),
            threshold_progress: py_round(threshold_progress, 4),
            focus: focus_plan(display, containers),
        });
    }
    let quality = if total_containers != 0 { progress_sum / total_containers as f64 } else { 0.0 };
    let focus_totals = FOCUS_OPTIONS
        .iter()
        .enumerate()
        .map(|(o, &(key, label, _, _))| {
            let mut counts = [0i64; 3];
            let mut recipe_totals = Vec::new();
            for s in &per_stat {
                for (c, n) in counts.iter_mut().zip(s.focus[o].counts) {
                    *c = c.saturating_add(n);
                }
                for &(mat, amount) in &s.focus[o].recipe_totals {
                    add_material(&mut recipe_totals, mat, amount);
                }
            }
            FocusCost { key, label, counts, recipe_totals }
        })
        .collect();
    Ok(Evaluation {
        tier,
        gem_type,
        element,
        restriction,
        level,
        quality: py_round(quality, 4),
        quality_percent: py_round(quality * 100.0, 2),
        calculated_power_rank: round_int(estimated).ok_or_else(|| GemError::Unhandled("power rank overflow".into()))?,
        issues,
        stats: per_stat,
        focus_totals,
        available_extra_containers: 0,
        guessed_distribution: Vec::new(),
    })
}

/// `_distribution_score`: (penalty, -total, spread), compared in order.
fn score(c: &Evaluation, total: i64) -> (f64, i64, f64) {
    let mut penalty = 0.0;
    for s in &c.stats {
        penalty += (s.raw_progress - clamp01(s.raw_progress)).powf(2.0);
    }
    let progress: Vec<f64> = c.stats.iter().map(|s| s.progress).collect();
    let max = progress.iter().skip(1).fold(progress[0], |m, &x| if x > m { x } else { m });
    let min = progress.iter().skip(1).fold(progress[0], |m, &x| if x < m { x } else { m });
    let spread = (max - min).powf(2.0) * 0.25;
    (py_round(penalty, 9), -total, py_round(spread, 9))
}

fn less(a: (f64, i64, f64), b: (f64, i64, f64)) -> bool {
    if a.0 != b.0 {
        return a.0 < b.0;
    }
    if a.1 != b.1 {
        return a.1 < b.1;
    }
    a.2 < b.2
}

fn guess(
    data: &Upgrades,
    tier: u8,
    gem_type: u8,
    level: i64,
    stats: &[(u8, f64, i64)],
    available: i64,
) -> Result<Vec<i64>, GemError> {
    let mut best: Option<(Vec<i64>, (f64, i64, f64))> = None;
    for a in 0..4 {
        for b in 0..4 {
            for c in 0..4 {
                let total = a + b + c;
                if total > available {
                    continue;
                }
                let dist = [a, b, c];
                let with: Vec<(u8, f64, i64)> = stats.iter().zip(dist).map(|(s, e)| (s.0, s.1, e)).collect();
                let s = score(&candidate(data, tier, gem_type, level, &with)?, total);
                if best.as_ref().is_none_or(|b| less(s, b.1)) {
                    best = Some((dist.to_vec(), s));
                }
            }
        }
    }
    Ok(best.map_or_else(|| vec![0, 0, 0], |b| b.0))
}

/// `evaluate_gem`.
pub fn evaluate(
    data: &Upgrades,
    tier: i64,
    gem_type: i64,
    level: i64,
    stats: &[StatInput],
    auto_guess: bool,
) -> Result<Evaluation, GemError> {
    let tier = cat::tier(tier)?;
    let gem_type = cat::gem_type(gem_type)?;
    if stats.len() != 3 {
        return Err(GemError::Evaluator("Exactly 3 stats are required.".into()));
    }
    let available = data.boosts_at(tier, gem_type, level)?;
    let mut selected: Vec<u8> = Vec::new();
    let mut extra_total: i64 = 0;
    for s in stats {
        let st = cat::stat_type(s.stat)?;
        if selected.contains(&st) {
            return Err(GemError::Evaluator("Gem stats must be unique.".into()));
        }
        selected.push(st);
        if s.extra_containers < 0 {
            return Err(GemError::Evaluator("Extra containers cannot be negative.".into()));
        }
        extra_total = extra_total.saturating_add(s.extra_containers);
    }
    if selected.contains(&PHYSICAL_DAMAGE) && selected.contains(&MAGIC_DAMAGE) {
        return Err(GemError::Evaluator("Physical Damage and Magic Damage cannot be on the same gem.".into()));
    }
    let mut payload: Vec<(u8, f64, i64)> =
        stats.iter().zip(&selected).map(|(s, &st)| (st, s.value, s.extra_containers)).collect();
    let guessed = if auto_guess || extra_total > available {
        let g = guess(data, tier, gem_type, level, &payload, available)?;
        for (p, &e) in payload.iter_mut().zip(&g) {
            p.2 = e;
        }
        g
    } else {
        payload.iter().map(|p| p.2).collect()
    };
    let mut out = candidate(data, tier, gem_type, level, &payload)?;
    out.available_extra_containers = available;
    out.guessed_distribution = guessed;
    Ok(out)
}

#[derive(Clone, Debug, PartialEq)]
pub struct SimpleEvaluation {
    pub tier: u8,
    pub gem_type: u8,
    pub level: i64,
    pub power_rank: i64,
    pub min_power_rank: i64,
    pub max_power_rank: i64,
    pub quality: f64,
    pub quality_percent: f64,
    pub is_within_range: bool,
    /// Can exceed `i64` (a Python int), never negative.
    pub distance: u64,
    pub containers: i64,
    pub focus_totals: Vec<FocusCost>,
}

/// `evaluate_gem_simple`: quality read off where the Power Rank falls.
pub fn evaluate_simple(
    data: &Upgrades,
    tier: i64,
    gem_type: i64,
    power_rank: i64,
    level: i64,
) -> Result<SimpleEvaluation, GemError> {
    let tier = cat::tier(tier)?;
    let gem_type = cat::gem_type(gem_type)?;
    let level = level.max(1);
    let max_level = data.max_level(tier, gem_type)?;
    if level > max_level {
        return Err(GemError::Evaluator(format!("Level {level} is above the max ({max_level}) for this gem.")));
    }
    let [t0, t1] = cat::pr_threshold(tier, gem_type);
    let base_pr = data.increment_total(tier, level)? * 3 + if gem_type == EMPOWERED { 100 } else { 0 };
    let band = |n: i64| (t0 * n + base_pr, t1 * n + base_pr);
    let mut containers = data.boosts_at(tier, gem_type, level)? + 3;
    let (mut min_pr, mut max_pr) = band(containers);
    for n in (3..=containers).rev() {
        let (lo, hi) = band(n);
        if lo <= power_rank && power_rank <= hi {
            (containers, min_pr, max_pr) = (n, lo, hi);
            break;
        }
    }
    let (raw, distance) = if power_rank < min_pr {
        (-1.0, (i128::from(min_pr) - i128::from(power_rank)) as u64)
    } else if power_rank > max_pr {
        (1.0, (i128::from(power_rank) - i128::from(max_pr)) as u64)
    } else if max_pr > min_pr {
        ((power_rank - min_pr) as f64 / (max_pr - min_pr) as f64, 0)
    } else {
        (0.0, 0)
    };
    let progress = clamp01(raw);
    Ok(SimpleEvaluation {
        tier,
        gem_type,
        level,
        power_rank,
        min_power_rank: min_pr,
        max_power_rank: max_pr,
        quality: py_round(progress, 4),
        quality_percent: py_round(progress * 100.0, 2),
        is_within_range: min_pr <= power_rank && power_rank <= max_pr,
        distance,
        containers,
        focus_totals: focus_plan(progress, containers),
    })
}

#[derive(Clone, Debug, PartialEq)]
pub struct StatRange {
    pub stat: u8,
    pub element: u8,
    pub containers: i64,
    pub min_value: f64,
    pub max_value: f64,
    pub stat_base: f64,
    pub thresholds: [f64; 2],
}

/// `gem_stat_range`.
pub fn stat_range(
    data: &Upgrades,
    tier: i64,
    gem_type: i64,
    stat: i64,
    level: i64,
    extra_containers: i64,
    element: Option<i64>,
) -> Result<StatRange, GemError> {
    let tier = cat::tier(tier)?;
    let gem_type = cat::gem_type(gem_type)?;
    let stat = cat::stat_type(stat)?;
    let level = level.max(1);
    let containers = extra_containers.max(0).saturating_add(1);
    let element = match element {
        Some(e) => cat::element(e)?,
        None if stat == LIGHT => COSMIC,
        None => WATER,
    };
    let increments = data.increment_total(tier, level)? as f64;
    let base = data.stat_base(tier, gem_type, element, stat)?;
    let thresholds = data.threshold(tier, gem_type, element, stat)?;
    Ok(StatRange {
        stat,
        element,
        containers,
        min_value: py_round(base * (thresholds[0] * containers as f64 + increments), 6),
        max_value: py_round(base * (thresholds[1] * containers as f64 + increments), 6),
        stat_base: base,
        thresholds,
    })
}

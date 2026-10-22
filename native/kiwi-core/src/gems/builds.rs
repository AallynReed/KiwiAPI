//! `app/trove/gems/builds.py` around the ranking loop: the stat assembly
//! before [`rank`], the result rows after it, and the Health Optimizer.

use super::upgrades::GemStats;
use super::{Base, GemLines, py_round, rank};

/// A class from `classes.json`: damage type, stat sheet (None reads 0) and bonuses.
#[derive(Clone, Debug, Default)]
pub struct ClassInfo {
    pub name: String,
    pub magic: bool,
    pub stats: Vec<(String, f64)>,
    pub bonuses: Vec<(String, Option<f64>)>,
}

/// A food or ally stat line.
#[derive(Clone, Debug)]
pub struct BonusStat {
    pub name: String,
    pub value: f64,
    pub percentage: bool,
}

/// A parsed star-chart code: stat name -> (flat, pct), and the Bounty Hunt
/// buff's (physical, magic) values when the chart unlocks it.
#[derive(Clone, Debug, Default)]
pub struct Chart {
    pub stats: Vec<(String, f64, f64)>,
    pub bounty: Option<(f64, f64)>,
}

impl Chart {
    /// `_merged`: a stat plus its `… Bonus` twin.
    pub fn merged(&self, name: &str) -> (f64, f64) {
        let get = |n: &str| self.stats.iter().find(|s| s.0 == n).map_or((0.0, 0.0), |s| (s.1, s.2));
        let (bf, bp) = get(name);
        let (xf, xp) = get(&format!("{name} Bonus"));
        (bf + xf, bp + xp)
    }
}

/// `_stat_value`: a named stat with its `… Bonus` twin, from 0.0.
pub fn stat_value(stats: &[(String, f64)], name: &str) -> f64 {
    let bonus = format!("{name} Bonus");
    let mut total = 0.0;
    for (n, v) in stats {
        if n == name || *n == bonus {
            total += v;
        }
    }
    total
}

/// The data files the engine reads (`sum_file_values` already applied).
pub struct BuildData<'a> {
    pub classes: &'a [ClassInfo],
    pub foods: &'a [(String, Vec<BonusStat>)],
    pub allies: &'a [(String, Vec<BonusStat>)],
    pub face_damage: f64,
    /// `sum_file_values(name)` for every `builds/<name>.json` the engine reads.
    pub sums: &'a dyn Fn(&str) -> f64,
    pub gem_stats: &'a GemStats,
}

/// The fields of `BuildConfigRequest` the engine reads.
#[derive(Clone, Debug)]
pub struct BuildConfig {
    pub build_type: String,
    pub character: String,
    pub subclass: String,
    pub food: String,
    pub ally: String,
    pub ally_buff: bool,
    pub berserker_battler: bool,
    pub critical_damage_count: i64,
    pub no_face: bool,
    pub light: i64,
    pub subclass_active: bool,
    pub litany: bool,
    pub bounty_hunt: bool,
    pub high_precision: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BuildRow {
    pub rank: usize,
    pub layout: String,
    pub base_dmg: f64,
    pub crit_dmg: f64,
    pub light: f64,
    pub bonus_dmg: f64,
    pub crit_bonus: f64,
    pub total_dmg: f64,
    pub class_bonus: Option<f64>,
    pub coefficient: f64,
}

/// `LILYPAD_MULTIPLIERS`.
fn lilypad(name: &str, value: f64, active: bool) -> f64 {
    if !active {
        return value;
    }
    value
        * match name {
            "Light" => 1.155,
            "Physical Damage" | "Magic Damage" | "Critical Damage" => 1.31,
            _ => 1.0,
        }
}

fn find<'a>(classes: &'a [ClassInfo], name: &str) -> Option<&'a ClassInfo> {
    classes.iter().rev().find(|c| c.name == name)
}

/// `GemOptimizerEngine.calculate_builds`, with the native ranking path. `Err`
/// is a `BuildError` message.
pub fn calculate(d: &BuildData<'_>, c: &BuildConfig, chart: Option<&Chart>) -> Result<Vec<BuildRow>, String> {
    if d.classes.is_empty() {
        return Err("Class data not loaded.".into());
    }
    let class = find(d.classes, &c.character).ok_or_else(|| format!("Unknown class '{}'.", c.character))?;
    if find(d.classes, &c.subclass).is_none() {
        return Err(format!("Unknown subclass '{}'.", c.subclass));
    }
    let health = c.build_type == "Health";
    let mut damage_type = if class.magic { "Magic Damage" } else { "Physical Damage" };
    let sum = d.sums;
    let (mut first, mut second, mut third, mut fourth, mut fifth, mut sixth);
    if health {
        first = sum("health") + stat_value(&class.stats, "Maximum Health");
        second = sum("health_per") + stat_value(&class.stats, "Maximum Health %");
        (third, fourth, fifth, sixth) = (0.0, 0.0, 100.0, 100.0);
        damage_type = "Maximum Health";
    } else {
        first = sum("damage");
        second = sum("critical_damage");
        third = sum("light");
        fourth = sum("bonus_damage");
        (fifth, sixth) = (100.0, 100.0);
        first += stat_value(&class.stats, damage_type);
        second += stat_value(&class.stats, "Critical Damage");
        if !c.no_face {
            first += d.face_damage;
        }
        let folder = if damage_type == "Magic Damage" { "magic_damage" } else { "physical_damage" };
        first += sum(&format!("{folder}/dragons_damage")) + sum("dragons_damage");
        second += sum("dragons_critical_damage");
        if !c.food.is_empty()
            && let Some((_, stats)) = d.foods.iter().find(|f| f.0 == c.food)
        {
            for s in stats {
                if s.name == damage_type {
                    fourth += if s.percentage { s.value } else { 0.0 };
                    first += if s.percentage { 0.0 } else { s.value };
                }
                if s.name == "Critical Damage" {
                    second += s.value;
                }
                if s.name == "Light" {
                    third += s.value;
                }
            }
        }
        if !c.ally.is_empty()
            && let Some((_, stats)) = d.allies.iter().find(|a| a.0 == c.ally)
        {
            for s in stats {
                let value = lilypad(&s.name, s.value, c.ally_buff);
                if s.name == damage_type {
                    fourth += if s.percentage { value } else { 0.0 };
                    first += if s.percentage { 0.0 } else { value };
                }
                if s.name == "Critical Damage" {
                    fifth += if s.percentage { value } else { 0.0 };
                    second += if s.percentage { 0.0 } else { value };
                }
                if s.name == "Light" {
                    third += value;
                }
            }
        }
        second -= 48.1 * (3 - c.critical_damage_count) as f64;
        if c.character == "Solarion" || c.subclass == "Solarion" {
            third += 140.0;
        }
        if damage_type == "Physical Damage" && c.subclass == "Lunar Lancer" {
            first += 750.0;
        }
        if damage_type == "Magic Damage" && matches!(c.subclass.as_str(), "Ice Sage" | "Shadow Hunter") {
            first += 750.0;
        }
        if matches!(c.subclass.as_str(), "Bard" | "Boomeranger") {
            second += 20.0;
        }
        if c.subclass_active {
            match c.subclass.as_str() {
                "Bard" => {
                    fourth += 45.0;
                    second += 45.0;
                }
                "Gunslinger" => fourth += 5.5,
                "Lunar Lancer" | "Candy Barbarian" => fourth += 30.0,
                _ => {}
            }
        }
        if c.berserker_battler {
            third += 750.0;
        }
        if c.litany {
            sixth += 1.0;
        }
        if let Some(chart) = chart {
            let dmg = chart.merged(damage_type);
            let crit = chart.merged("Critical Damage");
            let light = chart.merged("Light");
            first += dmg.0;
            second += crit.0;
            third += light.0;
            fourth += dmg.1;
            fifth += crit.1;
            sixth += light.1;
            if c.bounty_hunt
                && let Some((physical, magic)) = chart.bounty
            {
                fourth += if damage_type == "Magic Damage" { magic } else { physical };
            }
        }
    }
    let class_bonus = class.bonuses.iter().find(|b| b.0 == damage_type).and_then(|b| b.1);
    let names: [&str; 3] =
        if health { ["Maximum Health", "Maximum Health %", "Light"] } else { ["Damage", "Critical Damage", "Light"] };
    let lines = |t: u8| names.map(|n| d.gem_stats.get(t, n));
    let gems = GemLines { empowered: lines(super::catalog::EMPOWERED), lesser: lines(super::catalog::LESSER) };
    let base = Base { first, second, third, fourth, fifth, sixth };
    let ranked = rank(Some(&gems), &base, class_bonus, c.build_type == "Farm", c.light, c.high_precision, 200);
    let rd = |v: f64, digits: usize| py_round(v, if c.high_precision { 8 } else { digits });
    Ok(ranked
        .into_iter()
        .enumerate()
        .map(|(i, (b, cfirst, csecond, light, fin, coefficient))| {
            let mut boosts: Vec<u8> = b.to_vec();
            if c.light == 0 || health {
                boosts.remove(9);
                boosts.remove(6);
            }
            if c.light == 0 && !health {
                boosts.truncate(4);
            }
            let join = |xs: &[u8]| xs.iter().map(u8::to_string).collect::<Vec<_>>().join("/");
            let mut layout = join(&boosts[..4.min(boosts.len())]);
            if boosts.len() > 4 {
                layout = format!("{layout} + {}", join(&boosts[4..]));
            }
            BuildRow {
                rank: i + 1,
                layout,
                base_dmg: rd(cfirst, 2),
                crit_dmg: rd(csecond, 1),
                light,
                bonus_dmg: rd(fourth, 8),
                crit_bonus: rd(fifth - 100.0, 8),
                total_dmg: rd(fin, 2),
                class_bonus,
                coefficient,
            }
        })
        .collect())
}

/// `builds/health_optimizer.json`: per-line source values (summed like
/// Python's `sum()`), and gear lines `(flag, line, value)`.
#[derive(Clone, Debug, Default)]
pub struct HealthSources {
    pub sums: [f64; 3],
    pub gear_lines: Vec<(String, usize, f64)>,
}

pub const HEALTH_LINES: [&str; 3] = ["Critical Hit", "Maximum Health", "Maximum Health %"];

#[derive(Clone, Debug, PartialEq)]
pub struct HealthRow {
    pub totals: [f64; 3],
    pub rank: usize,
    pub empowered: [usize; 3],
    pub lesser: [usize; 3],
    pub health: f64,
}

impl HealthRow {
    pub fn layout(&self) -> String {
        let j = |c: [usize; 3]| c.map(|n| n.to_string()).join("/");
        format!("{} {}", j(self.empowered), j(self.lesser))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct HealthPlan {
    pub base: [f64; 3],
    pub results: Vec<HealthRow>,
}

/// `itertools.combinations_with_replacement(range(3), n)` as per-value counts, in order.
fn combos(n: usize) -> Vec<[usize; 3]> {
    let mut out = Vec::new();
    for a in 0..=n {
        for b in 0..=n - a {
            out.push([n - a - b, b, a]);
        }
    }
    out.sort_by(|x, y| {
        let key = |c: &[usize; 3]| {
            let mut v = Vec::new();
            for (i, &k) in c.iter().enumerate() {
                v.extend(std::iter::repeat_n(i, k));
            }
            v
        };
        key(x).cmp(&key(y))
    });
    out
}

/// `GemOptimizerEngine.optimize_health` after its class lookup: `flag` reads a
/// config flag, `chart` is the parsed star chart when one was given.
pub fn optimize_health(
    class: &ClassInfo,
    sources: &HealthSources,
    flag: &dyn Fn(&str) -> bool,
    chart: Option<&Chart>,
    gem_stats: &GemStats,
    precise: bool,
) -> HealthPlan {
    let mut base = [0.0; 3];
    for (i, line) in HEALTH_LINES.iter().enumerate() {
        base[i] = stat_value(&class.stats, line) + sources.sums[i];
    }
    for (name, line, value) in &sources.gear_lines {
        if flag(name) {
            base[*line] += value;
        }
    }
    if let Some(chart) = chart {
        let crit = chart.merged("Critical Hit");
        let health = chart.merged("Maximum Health");
        base[0] += crit.0;
        base[1] += health.0;
        base[2] += health.1;
    }
    let value = |t: u8| HEALTH_LINES.map(|s| gem_stats.get(t, s)[0] * 1.1);
    let (emp_v, les_v) = (value(super::catalog::EMPOWERED), value(super::catalog::LESSER));
    type Ranked = (f64, [f64; 3], [usize; 3], [usize; 3]);
    let mut ranked: Vec<Ranked> = Vec::new();
    for emp in combos(3) {
        for les in combos(6) {
            let total: [f64; 3] =
                std::array::from_fn(|i| base[i] + emp_v[i] * emp[i] as f64 + les_v[i] * les[i] as f64);
            if total[0] < 100.0 - 1e-9 {
                continue;
            }
            ranked.push((total[1] * (1.0 + total[2] / 100.0), total, emp, les));
        }
    }
    ranked.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.1[0].partial_cmp(&b.1[0]).unwrap_or(std::cmp::Ordering::Equal))
    });
    let rd = |v: f64| py_round(v, if precise { 8 } else { 2 });
    HealthPlan {
        base: base.map(rd),
        results: ranked
            .into_iter()
            .enumerate()
            .map(|(i, (health, total, empowered, lesser))| HealthRow {
                totals: total.map(rd),
                rank: i + 1,
                empowered,
                lesser,
                health: rd(health),
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn combinations_with_replacement_order() {
        let c3 = combos(3);
        assert_eq!(c3.len(), 10);
        assert_eq!(c3[0], [3, 0, 0]);
        assert_eq!(c3[1], [2, 1, 0]);
        assert_eq!(c3[2], [2, 0, 1]);
        assert_eq!(c3[3], [1, 2, 0]);
        assert_eq!(c3[9], [0, 0, 3]);
        assert_eq!(combos(6).len(), 28);
    }
}

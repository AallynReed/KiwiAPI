//! `app/trove/gems/bases.py`: stat rolls, the level schedule, level-up odds
//! and costs from the decoded `gem_upgrades.json`.

use std::collections::HashMap;

use super::catalog::{self as cat, COSMIC, LESSER, MYSTIC, WATER};
use super::{GemError, py_round};

#[derive(Clone, Debug, Default)]
pub struct StatRow {
    pub stat: String,
    pub percent: bool,
    pub min: f64,
    pub max: f64,
    pub step: f64,
}

#[derive(Clone, Debug, Default)]
pub struct PoolRow {
    pub stat: String,
    pub percent: bool,
    pub weight: f64,
}

#[derive(Clone, Debug, Default)]
pub struct Cost {
    pub item: String,
    pub count: i64,
}

#[derive(Clone, Debug, Default)]
pub struct LevelRow {
    pub level: i64,
    pub chance: f64,
    pub double_chance: f64,
    pub stat_steps: i64,
    pub boost: bool,
    pub cost: Vec<Cost>,
    pub dust: i64,
    pub amber: i64,
}

#[derive(Clone, Debug, Default)]
pub struct Record {
    pub max_level: i64,
    pub stats: Vec<StatRow>,
    pub pool: Vec<PoolRow>,
    pub levels: Vec<LevelRow>,
}

#[derive(Clone, Debug, Default)]
pub struct BoosterRow {
    pub item: String,
    pub chance_multiplier: f64,
    pub double_multiplier: f64,
}

/// A level-up booster as `boosters()` lists it.
#[derive(Clone, Debug)]
pub struct Booster {
    pub id: String,
    pub name: String,
    pub item: String,
    pub chance_multiplier: f64,
    pub double_multiplier: f64,
}

/// The decoded file. `index` maps (size, color, file tier) to a record, the
/// last listing winning like Python's dict comprehension.
#[derive(Clone, Debug, Default)]
pub struct Upgrades {
    pub records: Vec<Record>,
    pub index: HashMap<(String, String, i64), usize>,
    /// color -> (dust item, amber item); empty strings are absent.
    pub materials: HashMap<String, (String, String)>,
    pub boosters: Vec<BoosterRow>,
    pub focuses: Vec<(String, f64)>,
    pub names: HashMap<String, String>,
}

/// One item an attempt spends.
#[derive(Clone, Debug, PartialEq)]
pub struct ItemCount {
    pub item: String,
    pub name: String,
    pub count: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlanStep {
    pub level: i64,
    pub chance: f64,
    pub double_chance: f64,
    pub karma_per_fail: i64,
    pub expected_attempts: f64,
    pub max_attempts: Option<i64>,
    pub cost: Vec<ItemCount>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LevelPlan {
    pub levels: Vec<PlanStep>,
    pub expected_total: Vec<(String, i64)>,
}

/// Max-roll Mystic stats for the build calculator: `[at max level, per boost]`
/// for Damage, Critical Damage, Critical Hit, Maximum Health, Maximum Health %
/// and Light, Lesser then Empowered.
#[derive(Clone, Debug, PartialEq)]
pub struct GemStats {
    pub lesser: [[f64; 2]; 6],
    pub empowered: [[f64; 2]; 6],
}

pub const BUILD_STAT_NAMES: [&str; 6] =
    ["Damage", "Critical Damage", "Critical Hit", "Maximum Health", "Maximum Health %", "Light"];

impl GemStats {
    pub fn get(&self, gem_type: u8, name: &str) -> [f64; 2] {
        let i = BUILD_STAT_NAMES.iter().position(|n| *n == name).expect("known build stat");
        if gem_type == LESSER { self.lesser[i] } else { self.empowered[i] }
    }
}

impl Upgrades {
    pub fn upgrade_data(&self, tier: u8, gem_type: u8, element: u8) -> Result<&Record, GemError> {
        let ft = cat::file_tier(tier);
        let key = if gem_type == LESSER { ("small", ft) } else { ("large", ft + 100) };
        self.index
            .get(&(key.0.to_owned(), cat::color(element).to_owned(), key.1))
            .map(|&i| &self.records[i])
            .ok_or_else(|| {
                GemError::Value(format!(
                    "no game data for {} {} gems",
                    cat::name(&cat::TYPES, gem_type),
                    cat::name(&cat::TIERS, tier)
                ))
            })
    }

    pub fn item_name(&self, item: &str) -> String {
        match self.names.get(item) {
            Some(n) if !n.is_empty() => n.clone(),
            _ => item.rsplit('/').next().unwrap_or(item).to_owned(),
        }
    }

    pub fn boosters(&self) -> Vec<Booster> {
        self.boosters
            .iter()
            .map(|b| Booster {
                id: b.item.rsplit('/').next().unwrap_or(&b.item).to_owned(),
                name: self.item_name(&b.item),
                item: b.item.clone(),
                chance_multiplier: b.chance_multiplier,
                double_multiplier: b.double_multiplier,
            })
            .collect()
    }

    pub fn booster(&self, id: &str) -> Result<Booster, GemError> {
        self.boosters()
            .into_iter()
            .find(|b| b.id == id)
            .ok_or_else(|| GemError::Value(format!("unknown booster {}", py_repr(id))))
    }

    pub fn max_level(&self, tier: u8, gem_type: u8) -> Result<i64, GemError> {
        Ok(self.upgrade_data(tier, gem_type, WATER)?.max_level)
    }

    pub fn level_attempt(
        &self,
        tier: u8,
        gem_type: u8,
        element: u8,
        level: i64,
    ) -> Result<Option<&LevelRow>, GemError> {
        Ok(self.upgrade_data(tier, gem_type, element)?.levels.iter().find(|lv| lv.level == level))
    }

    pub fn boost_levels(&self, tier: u8, gem_type: u8) -> Result<Vec<i64>, GemError> {
        Ok(self.upgrade_data(tier, gem_type, WATER)?.levels.iter().filter(|lv| lv.boost).map(|lv| lv.level).collect())
    }

    pub fn boosts_at(&self, tier: u8, gem_type: u8, level: i64) -> Result<i64, GemError> {
        Ok(self.boost_levels(tier, gem_type)?.iter().filter(|&&lv| lv <= level).count() as i64)
    }

    fn steps(&self, tier: u8, level: i64) -> Result<i64, GemError> {
        Ok(self.level_attempt(tier, LESSER, WATER, level)?.map_or(0, |lv| lv.stat_steps))
    }

    /// `get_increment_power_rank_*` (both types share the schedule).
    pub fn increment(&self, tier: u8, level: i64) -> Result<i64, GemError> {
        Ok(cat::pr_base(tier) * self.steps(tier, level)?)
    }

    /// The per-level increments for levels 1..=level, in order. Levels the
    /// file does not list add 0, so the walk stops at the last one it does.
    pub fn increments(&self, tier: u8, level: i64) -> Result<Vec<i64>, GemError> {
        let rows = &self.upgrade_data(tier, LESSER, WATER)?.levels;
        let last = rows.iter().map(|lv| lv.level).max().unwrap_or(1).min(level);
        (1..=last).map(|lv| self.increment(tier, lv)).collect()
    }

    pub fn increment_total(&self, tier: u8, level: i64) -> Result<i64, GemError> {
        Ok(self.increments(tier, level)?.iter().sum())
    }

    pub fn stat_roll(&self, tier: u8, gem_type: u8, element: u8, stat: u8) -> Result<&StatRow, GemError> {
        let key = cat::stat_key(stat);
        self.upgrade_data(tier, gem_type, element)?
            .stats
            .iter()
            .find(|row| key == Some((row.stat.as_str(), row.percent)))
            .ok_or_else(|| {
                GemError::Value(format!(
                    "no stat base for {} at {}",
                    cat::name(&cat::STATS, stat),
                    cat::name(&cat::TIERS, tier)
                ))
            })
    }

    /// `_base`: a stat's per-level step over the tier's Power Rank per step.
    pub fn stat_base(&self, tier: u8, gem_type: u8, element: u8, stat: u8) -> Result<f64, GemError> {
        Ok(self.stat_roll(tier, gem_type, element, stat)?.step / cat::pr_base(tier) as f64)
    }

    /// `_threshold`: the roll range in base units, rounded to 9 places.
    pub fn threshold(&self, tier: u8, gem_type: u8, element: u8, stat: u8) -> Result<[f64; 2], GemError> {
        let roll = self.stat_roll(tier, gem_type, element, stat)?;
        let base = self.stat_base(tier, gem_type, element, stat)?;
        if base == 0.0 {
            return Err(GemError::zero_div());
        }
        Ok([py_round(roll.min / base, 9), py_round(roll.max / base, 9)])
    }

    /// `get_augment_base`: the focus's percent.
    pub fn augment_base(&self, augment: u8) -> Result<f64, GemError> {
        let item = cat::focus_item(augment);
        self.focuses
            .iter()
            .find(|(i, _)| i == item)
            .map(|f| f.1)
            .ok_or_else(|| GemError::Unhandled(format!("no focus amount for {item}")))
    }

    /// `stat_weights`: roll weights by stat, in `_STAT` order. A cosmic gem
    /// without a pool of its own rolls like the water gem of its size and tier.
    pub fn stat_weights(&self, tier: u8, gem_type: u8, element: u8) -> Result<Vec<(u8, f64)>, GemError> {
        let mut pool = &self.upgrade_data(tier, gem_type, element)?.pool;
        if pool.is_empty() {
            pool = &self.upgrade_data(tier, gem_type, WATER)?.pool;
        }
        let mut by_key: Vec<((&str, bool), f64)> = Vec::new();
        for p in pool {
            let key = (p.stat.as_str(), p.percent);
            match by_key.iter_mut().find(|e| e.0 == key) {
                Some(e) => e.1 = p.weight,
                None => by_key.push((key, p.weight)),
            }
        }
        Ok(cat::STAT_KEYS
            .iter()
            .filter_map(|&(st, k, pct)| by_key.iter().find(|e| e.0 == (k, pct)).map(|e| (st, e.1)))
            .collect())
    }

    /// `attempt_odds`: (chance, double chance), booster applied, capped at 1.
    pub fn attempt_odds(lv: &LevelRow, boost: Option<&Booster>) -> (f64, f64) {
        let chance = lv.chance * boost.map_or(1.0, |b| b.chance_multiplier);
        let double = lv.double_chance * boost.map_or(1.0, |b| b.double_multiplier);
        (super::pynum::cap_one(chance), super::pynum::cap_one(double))
    }

    /// `karma_gain`: whole percent of both chances, rounded down.
    pub fn karma_gain(lv: &LevelRow, boost: Option<&Booster>) -> i64 {
        let (chance, double) = Self::attempt_odds(lv, boost);
        py_round((chance + double) * 100.0, 6).floor() as i64
    }

    /// `attempt_cost`: what one attempt spends, landed or not.
    pub fn attempt_cost(&self, element: u8, lv: &LevelRow, boost: Option<&Booster>) -> Vec<ItemCount> {
        let (dust, amber) = self.materials.get(cat::color(element)).cloned().unwrap_or_default();
        let mut rows: Vec<(String, i64)> = lv.cost.iter().map(|c| (c.item.clone(), c.count)).collect();
        if lv.dust != 0 && !dust.is_empty() {
            rows.push((dust, lv.dust));
        }
        if lv.amber != 0 && !amber.is_empty() {
            rows.push((amber, lv.amber));
        }
        if let Some(b) = boost {
            rows.push((b.item.clone(), 1));
        }
        rows.into_iter().map(|(item, count)| ItemCount { name: self.item_name(&item), item, count }).collect()
    }

    /// `level_plan`: every attempt above `from_level` and the expected spend.
    pub fn level_plan(
        &self,
        tier: u8,
        gem_type: u8,
        element: u8,
        booster: Option<&str>,
        from_level: i64,
    ) -> Result<LevelPlan, GemError> {
        let boost = match booster {
            Some(id) if !id.is_empty() => Some(self.booster(id)?),
            _ => None,
        };
        let mut rows = Vec::new();
        let mut expected: Vec<(String, f64)> = Vec::new();
        for lv in &self.upgrade_data(tier, gem_type, element)?.levels {
            if lv.level <= from_level {
                continue;
            }
            let (chance, double) = Self::attempt_odds(lv, boost.as_ref());
            let cost = self.attempt_cost(element, lv, boost.as_ref());
            let gain = Self::karma_gain(lv, boost.as_ref());
            let fails = (gain > 0).then(|| (cat::KARMA_MAX + gain - 1).div_euclid(gain));
            let (attempts, most) = if chance >= 1.0 {
                (1.0, Some(1))
            } else if let Some(f) = fails {
                ((1.0 - (1.0 - chance).powf(f as f64)) / chance, Some(f))
            } else {
                (if chance > 0.0 { 1.0 / chance } else { 0.0 }, None)
            };
            for c in &cost {
                let add = c.count as f64 * attempts;
                match expected.iter_mut().find(|e| e.0 == c.name) {
                    Some(e) => e.1 += add,
                    None => expected.push((c.name.clone(), add)),
                }
            }
            rows.push(PlanStep {
                level: lv.level,
                chance,
                double_chance: double,
                karma_per_fail: gain,
                expected_attempts: py_round(attempts, 2),
                max_attempts: most,
                cost,
            });
        }
        let expected_total = expected.into_iter().map(|(k, v)| (k, v.round_ties_even() as i64)).collect();
        Ok(LevelPlan { levels: rows, expected_total })
    }

    /// `build_gem_stats`.
    pub fn build_gem_stats(&self) -> Result<GemStats, GemError> {
        use cat::{CRITICAL_DAMAGE, CRITICAL_HIT, LIGHT, MAX_HEALTH, MAX_HEALTH_BONUS, PHYSICAL_DAMAGE};
        let stats = [PHYSICAL_DAMAGE, CRITICAL_DAMAGE, CRITICAL_HIT, MAX_HEALTH, MAX_HEALTH_BONUS, LIGHT];
        let mut out = [[[0.0; 2]; 6]; 2];
        for (t, gem_type) in [LESSER, cat::EMPOWERED].into_iter().enumerate() {
            let steps: i64 = (1..=self.max_level(MYSTIC, gem_type)?)
                .map(|lv| self.steps(MYSTIC, lv))
                .collect::<Result<Vec<_>, _>>()?
                .iter()
                .sum();
            for (i, &st) in stats.iter().enumerate() {
                let element = if st == LIGHT { COSMIC } else { WATER };
                let roll = self.stat_roll(MYSTIC, gem_type, element, st)?;
                out[t][i] = [roll.max + steps as f64 * roll.step, roll.max];
            }
        }
        Ok(GemStats { lesser: out[0], empowered: out[1] })
    }
}

/// Python `repr()` of a str.
pub fn py_repr(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') { '"' } else { '\'' };
    let mut out = String::with_capacity(s.len() + 2);
    out.push(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if is_unprintable(c) => {
                let n = c as u32;
                if n < 0x100 {
                    out.push_str(&format!("\\x{n:02x}"));
                } else if n <= 0xffff {
                    out.push_str(&format!("\\u{n:04x}"));
                } else {
                    out.push_str(&format!("\\U{n:08x}"));
                }
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// `not str.isprintable()` for the common cases: controls, non-space
/// separators, and the format and private-use ranges.
fn is_unprintable(c: char) -> bool {
    c.is_control()
        || (c.is_whitespace() && c != ' ')
        || matches!(c as u32,
            0xad | 0x600..=0x605 | 0x61c | 0x6dd | 0x70f | 0x180e | 0x200b..=0x200f | 0x202a..=0x202e
            | 0x2060..=0x2064 | 0x2066..=0x206f | 0xfeff | 0xfff9..=0xfffb | 0xe000..=0xf8ff | 0xf0000..=0x10ffff)
}

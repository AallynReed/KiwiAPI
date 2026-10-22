//! The Gem object of `app/trove/gems/model.py`: its computed fields and the
//! simulator actions, drawing from [`PyRandom`] in Python's exact order.

use super::GemError;
use super::catalog::{
    self as cat, COSMIC, CRITICAL_HIT, EMPOWERED, FIERCE, LESSER, LIGHT, MAGIC_DAMAGE, PHYSICAL_DAMAGE,
};
use super::py_round;
use super::pynum::{PyNum, min_one, py_fsum, py_sum, round_int};
use super::pyrandom::PyRandom;
use super::upgrades::{ItemCount, Upgrades};

#[derive(Clone, Debug, PartialEq)]
pub struct Augment {
    pub kind: u8,
    pub count: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Container {
    pub base: f64,
    pub augments: Vec<Augment>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Stat {
    pub kind: u8,
    pub containers: Vec<Container>,
    pub locked: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Gem {
    pub id: i64,
    pub tier: u8,
    pub gem_type: u8,
    pub element: u8,
    pub restriction: Option<u8>,
    pub ability: Option<u8>,
    pub level: i64,
    pub stats: Vec<Stat>,
    pub augmentation: Option<f64>,
    pub attempts: i64,
    pub karma: i64,
    pub spent: Vec<(String, i64)>,
}

/// The four default augments of a fresh container.
pub fn default_augments() -> Vec<Augment> {
    (1..=3).map(|kind| Augment { kind, count: 0 }).collect()
}

impl Container {
    /// `StatContainer(**aug)`: a random base unless the gem pins one.
    pub fn new(base: Option<f64>, rng: &mut PyRandom) -> Self {
        Container { base: base.unwrap_or_else(|| rng.random()), augments: default_augments() }
    }

    pub fn increase(&self, data: &Upgrades) -> Result<PyNum, GemError> {
        let mut terms = Vec::with_capacity(self.augments.len());
        for a in &self.augments {
            terms.push(PyNum::Float(data.augment_base(a.kind)? / 100.0 * a.count as f64));
        }
        Ok(py_sum(terms))
    }

    pub fn real_value(&self, data: &Upgrades) -> Result<f64, GemError> {
        Ok(self.base + self.increase(data)?.f64())
    }

    pub fn value(&self, data: &Upgrades) -> Result<PyNum, GemError> {
        Ok(min_one(self.real_value(data)?))
    }

    fn add_augment(&mut self, augment: u8) {
        for a in self.augments.iter_mut().filter(|a| a.kind == augment) {
            a.count += 1;
        }
    }
}

impl Stat {
    pub fn new(kind: u8) -> Self {
        Stat { kind, containers: Vec::new(), locked: false }
    }

    pub fn augmentation_progress(&self, data: &Upgrades) -> Result<PyNum, GemError> {
        if self.containers.is_empty() {
            return Ok(PyNum::Float(0.0));
        }
        let values = self.containers.iter().map(|c| c.real_value(data)).collect::<Result<Vec<_>, _>>()?;
        Ok(min_one(py_fsum(values) / self.containers.len() as f64))
    }

    fn add_augment(&mut self, augment: u8, data: &Upgrades) -> Result<bool, GemError> {
        if self.augmentation_progress(data)?.f64() == 1.0 {
            return Ok(false);
        }
        for c in &mut self.containers {
            if c.real_value(data)? >= 1.0 {
                continue;
            }
            c.add_augment(augment);
            return Ok(true);
        }
        Ok(false)
    }
}

/// What `level_up` reports besides the gem.
#[derive(Clone, Debug, PartialEq)]
pub struct LevelUp {
    pub outcome: &'static str,
    pub chance: f64,
    pub double_chance: f64,
    pub cost: Vec<ItemCount>,
    pub guaranteed: bool,
    pub karma_gained: i64,
}

/// The inputs of `Gem.create`.
#[derive(Clone, Debug, Default)]
pub struct CreateParams {
    pub tier: Option<i64>,
    pub gem_type: Option<i64>,
    pub element: Option<i64>,
    pub restriction: Option<i64>,
    pub augmentation: Option<f64>,
    pub level: i64,
    pub retired_stats: bool,
}

fn index_error(msg: &str) -> GemError {
    GemError::Index(msg.to_owned())
}

const EMPTY_CHOICE: &str = "Cannot choose from an empty sequence";

/// `_weighted_sample`: `k` distinct stats, each draw weighted by the roll weights.
fn weighted_sample(pool: &[u8], weights: &[(u8, f64)], k: usize, rng: &mut PyRandom) -> Result<Vec<u8>, GemError> {
    let mut left = pool.to_vec();
    let mut picked = Vec::new();
    for _ in 0..k.min(left.len()) {
        let mut w: Vec<f64> = left.iter().map(|st| weights.iter().find(|e| e.0 == *st).map_or(0.0, |e| e.1)).collect();
        if !w.iter().any(|&x| x != 0.0) {
            w = vec![1.0; left.len()];
        }
        let i = rng.weighted_choice(&w).map_err(|m| GemError::Value(m.into()))?;
        let st = left[i];
        picked.push(st);
        let first = left.iter().position(|&x| x == st).unwrap_or(i);
        left.remove(first);
    }
    Ok(picked)
}

impl Gem {
    /// `Gem.create(...)` without `procs`/`generation` (the API never passes them).
    pub fn create(data: &Upgrades, p: &CreateParams, id: i64, rng: &mut PyRandom) -> Result<Gem, GemError> {
        let pick = |rng: &mut PyRandom, n: usize| rng.choice(n).expect("non-empty enum") as u8 + 1;
        let tier = match p.tier {
            None | Some(0) => pick(rng, 4),
            Some(t) => cat::tier(t)?,
        };
        let gem_type = match p.gem_type {
            None | Some(0) => pick(rng, 2),
            Some(t) => cat::gem_type(t)?,
        };
        let element = match p.element {
            None | Some(0) => pick(rng, 4),
            Some(e) => cat::element(e)?,
        };
        let restriction = if gem_type == LESSER {
            Some(match p.restriction {
                None => pick(rng, 2),
                Some(r) => cat::restriction(r)?,
            })
        } else {
            None
        };
        let extra = data.boosts_at(tier, gem_type, p.level)?;
        let physical = match restriction {
            None => rng.choice(2) == Some(0),
            Some(r) => r == FIERCE,
        };
        let mut pool: Vec<u8> = if physical { cat::PHYSICAL_POOL.to_vec() } else { cat::MAGIC_POOL.to_vec() };
        let mut weights = data.stat_weights(tier, gem_type, element)?;
        if p.retired_stats {
            for st in cat::RETIRED {
                pool.push(st);
                let w = weights.iter().find(|e| e.0 == CRITICAL_HIT).map_or(0.0, |e| e.1);
                match weights.iter_mut().find(|e| e.0 == st) {
                    Some(e) => e.1 = w,
                    None => weights.push((st, w)),
                }
            }
        }
        let mut stats: Vec<Stat> = weighted_sample(&pool, &weights, 3, rng)?.into_iter().map(Stat::new).collect();
        if element == COSMIC {
            let i = rng.randint(0, 2) as usize;
            let stat = stats.get_mut(i).ok_or_else(|| index_error("list index out of range"))?;
            stat.kind = LIGHT;
            stat.locked = true;
        }
        for stat in &mut stats {
            stat.containers.push(Container::new(p.augmentation, rng));
        }
        for _ in 0..extra.max(0) {
            let i = rng.randint(0, 2) as usize;
            let c = Container::new(p.augmentation, rng);
            stats.get_mut(i).ok_or_else(|| index_error("list index out of range"))?.containers.push(c);
        }
        let level = p.level.min(data.max_level(tier, gem_type)?);
        let ability = if gem_type == EMPOWERED {
            let abilities = cat::element_abilities(element);
            Some(abilities[rng.choice(abilities.len()).expect("non-empty")])
        } else {
            None
        };
        Ok(Gem {
            id,
            tier,
            gem_type,
            element,
            restriction,
            ability,
            level,
            stats,
            augmentation: p.augmentation,
            attempts: 0,
            karma: 0,
            spent: Vec::new(),
        })
    }

    fn check_position(&self, position: i64) -> Result<usize, GemError> {
        if 0 <= position && (position as usize) < self.stats.len() {
            Ok(position as usize)
        } else {
            Err(index_error(&format!("stat position {position} out of range (0..{})", self.stats.len() as i64 - 1)))
        }
    }

    /// `augment_stat`; `augment` is the raw request int.
    pub fn augment_stat(&mut self, data: &Upgrades, position: i64, augment: i64) -> Result<bool, GemError> {
        let i = self.check_position(position)?;
        let augment = cat::augment(augment)?;
        self.stats[i].add_augment(augment, data)
    }

    pub fn spark_stat(&mut self, position: i64, rng: &mut PyRandom) -> Result<bool, GemError> {
        let i = self.check_position(position)?;
        if self.stats[i].locked {
            return Ok(false);
        }
        let in_use: Vec<u8> = self.stats.iter().map(|s| s.kind).collect();
        let pool: &[u8] = match self.restriction {
            Some(r) if r == FIERCE => &cat::PHYSICAL_POOL,
            Some(_) => &cat::MAGIC_POOL,
            None => cat::element_stats(self.element),
        };
        let mut unused: Vec<u8> = pool.iter().copied().filter(|s| !in_use.contains(s)).collect();
        if in_use.contains(&PHYSICAL_DAMAGE) {
            unused.retain(|&s| s != MAGIC_DAMAGE);
        }
        if in_use.contains(&MAGIC_DAMAGE) {
            unused.retain(|&s| s != PHYSICAL_DAMAGE);
        }
        let Some(pick) = rng.choice(unused.len()) else { return Ok(false) };
        self.stats[i].kind = unused[pick];
        Ok(true)
    }

    pub fn flare_stat(&mut self, position: i64, rng: &mut PyRandom) -> Result<bool, GemError> {
        let i = self.check_position(position)?;
        if self.stats[i].containers.len() <= 1 {
            return Ok(false);
        }
        let moved = self.stats[i].containers.pop().expect("more than one container");
        let others: Vec<usize> = (0..self.stats.len()).filter(|&j| j != i).collect();
        let pick = rng.choice(others.len()).ok_or_else(|| index_error(EMPTY_CHOICE))?;
        self.stats[others[pick]].containers.push(moved);
        Ok(true)
    }

    fn gain_level(&mut self, data: &Upgrades, rng: &mut PyRandom) -> Result<(), GemError> {
        self.level += 1;
        if data.boost_levels(self.tier, self.gem_type)?.contains(&self.level) {
            let i = rng.randint(0, 2) as usize;
            if i >= self.stats.len() {
                return Err(index_error("list index out of range"));
            }
            let c = Container::new(self.augmentation, rng);
            self.stats[i].containers.push(c);
        }
        Ok(())
    }

    /// `level_up`: one attempt at the game's odds.
    pub fn level_up(
        &mut self,
        data: &Upgrades,
        booster: Option<&str>,
        rng: &mut PyRandom,
    ) -> Result<LevelUp, GemError> {
        let Some(attempt) = data.level_attempt(self.tier, self.gem_type, self.element, self.level.saturating_add(1))?
        else {
            return Ok(LevelUp {
                outcome: "max_level",
                chance: 0.0,
                double_chance: 0.0,
                cost: Vec::new(),
                guaranteed: false,
                karma_gained: 0,
            });
        };
        let attempt = attempt.clone();
        let boost = match booster {
            Some(id) if !id.is_empty() => Some(data.booster(id)?),
            _ => None,
        };
        let (chance, double) = Upgrades::attempt_odds(&attempt, boost.as_ref());
        let cost = data.attempt_cost(self.element, &attempt, boost.as_ref());
        self.attempts = self.attempts.saturating_add(1);
        for row in &cost {
            match self.spent.iter_mut().find(|e| e.0 == row.name) {
                Some(e) => e.1 = e.1.saturating_add(row.count),
                None => self.spent.push((row.name.clone(), row.count)),
            }
        }
        let mut guaranteed = false;
        let mut karma_gained = 0;
        let outcome;
        if rng.random() < chance {
            let double_roll = rng.random() < double;
            let gained = if double_roll && self.level + 2 <= data.max_level(self.tier, self.gem_type)? { 2 } else { 1 };
            for _ in 0..gained {
                self.gain_level(data, rng)?;
            }
            outcome = if gained == 2 { "double" } else { "success" };
            self.karma = 0;
        } else {
            karma_gained = Upgrades::karma_gain(&attempt, boost.as_ref());
            if self.karma.saturating_add(karma_gained) >= cat::KARMA_MAX {
                self.gain_level(data, rng)?;
                outcome = "success";
                guaranteed = true;
                self.karma = 0;
            } else {
                outcome = "failed";
                self.karma += karma_gained;
            }
        }
        Ok(LevelUp { outcome, chance, double_chance: double, cost, guaranteed, karma_gained })
    }

    pub fn container_count(&self) -> usize {
        self.stats.iter().map(|s| s.containers.len()).sum()
    }

    pub fn set_level(&mut self, data: &Upgrades, level: i64, rng: &mut PyRandom) -> Result<bool, GemError> {
        if self.level == level {
            return Ok(false);
        }
        self.level = level.min(data.max_level(self.tier, self.gem_type)?);
        self.karma = 0;
        let target = 3 + data.boosts_at(self.tier, self.gem_type, self.level)?;
        let diff = target - self.container_count() as i64;
        if diff > 0 {
            for _ in 0..diff {
                let i = rng.choice(self.stats.len()).ok_or_else(|| index_error(EMPTY_CHOICE))?;
                let c = Container::new(self.augmentation, rng);
                self.stats[i].containers.push(c);
            }
        } else {
            for _ in 0..-diff {
                let candidates: Vec<usize> =
                    (0..self.stats.len()).filter(|&j| self.stats[j].containers.len() > 1).collect();
                let pick = rng.choice(candidates.len()).ok_or_else(|| index_error(EMPTY_CHOICE))?;
                self.stats[candidates[pick]].containers.pop();
            }
        }
        Ok(true)
    }

    pub fn ability_name(&self) -> Option<&'static str> {
        self.ability.map(|a| cat::name(&cat::ABILITIES, a))
    }

    pub fn gem_name(&self) -> Option<String> {
        if self.gem_type != LESSER {
            return self.ability_name().map(str::to_owned);
        }
        let tier = cat::name(&cat::TIERS, self.tier);
        Some(match self.restriction {
            None => format!("{tier} Gem"),
            Some(r) => format!("{} {tier} Gem", cat::name(&cat::RESTRICTIONS, r)),
        })
    }

    pub fn is_max_level(&self, data: &Upgrades) -> Result<bool, GemError> {
        Ok(self.level == data.max_level(self.tier, self.gem_type)?)
    }

    pub fn quality(&self, data: &Upgrades) -> Result<f64, GemError> {
        let mut values = Vec::new();
        for s in &self.stats {
            for c in &s.containers {
                values.push(c.value(data)?);
            }
        }
        if values.is_empty() {
            return Ok(0.0);
        }
        let n = values.len() as f64;
        Ok(py_round(py_sum(values).f64() / n * 100.0, 1) / 100.0)
    }

    pub fn power_rank(&self, data: &Upgrades) -> Result<i64, GemError> {
        let mut pr = if self.gem_type == EMPOWERED { 100.0 } else { 0.0 };
        let [lo, hi] = cat::pr_threshold(self.tier, self.gem_type);
        for stat in &self.stats {
            let progress = lo as f64 + (hi - lo) as f64 * stat.augmentation_progress(data)?.f64();
            pr += progress * stat.containers.len() as f64;
        }
        if !self.stats.is_empty() {
            let inc = data.increments(self.tier, self.level)?;
            for _ in &self.stats {
                for &i in &inc {
                    pr += i as f64;
                }
            }
        }
        round_int(pr).ok_or_else(|| GemError::Unhandled(format!("cannot round power rank {pr}")))
    }

    pub fn stat_values(&self, data: &Upgrades) -> Result<Vec<(&'static str, f64)>, GemError> {
        let pr_increments = data.increment_total(self.tier, self.level)? as f64;
        let mut out = Vec::with_capacity(self.stats.len());
        for stat in &self.stats {
            let base = data.stat_base(self.tier, self.gem_type, self.element, stat.kind)?;
            let [t0, t1] = data.threshold(self.tier, self.gem_type, self.element, stat.kind)?;
            let progress = t0 + (t1 - t0) * stat.augmentation_progress(data)?.f64();
            let value = base * progress * stat.containers.len() as f64 + base * pr_increments;
            out.push((cat::name(&cat::STATS, stat.kind), value));
        }
        Ok(out)
    }
}

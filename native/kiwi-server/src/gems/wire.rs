//! The request models of `app/trove/gems/schemas.py` (pydantic's lax rules
//! and error items, through [`crate::validation`]) and the response models as
//! FastAPI dumps them: stored fields, then computed ones, in model order.

use kiwi_core::gems::{
    GemError,
    builds::{BuildRow, HealthPlan},
    catalog::{self as cat, LESSER},
    evaluator::{Evaluation, FocusCost, SimpleEvaluation, StatInput, StatRange},
    model::{Augment, Container, Gem, LevelUp, Stat, default_augments},
    pyrandom::PyRandom,
    upgrades::{ItemCount, LevelPlan, Upgrades},
};
use serde_json::{Map, Value, json};

use crate::validation::{Coerce, Fail, Field, Input, IntC};

/// `int(datetime.now(UTC).timestamp() * 100)`, the default `Gem.id`.
pub fn now_id() -> i64 {
    let micros = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_micros() as i64);
    (micros as f64 / 1e6 * 100.0) as i64
}

/// pydantic's IntEnum check: a lax int that must be one of `1..=n`.
fn int_enum(input: &Input<'_>, n: usize) -> Result<u8, Fail> {
    match IntC::default().coerce(input) {
        Ok(v) if (1..=n as i64).contains(&v) => Ok(v as u8),
        Err(f) if f.kind == "int_parsing_size" => Err(f),
        _ => {
            let ids: Vec<String> = (1..=n).map(|i| i.to_string()).collect();
            let expected = match ids.split_last() {
                Some((last, [])) => last.clone(),
                Some((last, rest)) => format!("{} or {last}", rest.join(", ")),
                None => String::new(),
            };
            Err(Fail::new("enum", format!("Input should be {expected}")).with_ctx("expected", expected))
        }
    }
}

fn augment(f: Field<'_>) -> Option<Augment> {
    f.model(|m| {
        let kind = m.field("type").custom(|i| int_enum(i, 3)).required();
        let count = m.field("count").int().default(0);
        Some(Augment { kind, count })
    })
    .required()
}

fn container(f: Field<'_>, rng: &mut PyRandom) -> Option<Container> {
    f.model(|m| {
        let base = {
            let f = m.field("base");
            if f.is_missing() { rng.random() } else { f.float().required() }
        };
        let augments = {
            let f = m.field("augments");
            if f.is_missing() {
                Some(default_augments())
            } else {
                f.list(augment).required().into_iter().collect::<Option<Vec<_>>>()
            }
        };
        Some(Container { base, augments: augments? })
    })
    .required()
}

fn stat(f: Field<'_>, rng: &mut PyRandom) -> Option<Stat> {
    f.model(|m| {
        let kind = m.field("type").custom(|i| int_enum(i, 10)).required();
        let containers = m.field("containers").list(|f| container(f, rng)).default(Vec::new());
        let locked = m.field("locked").bool().default(false);
        Some(Stat { kind, containers: containers.into_iter().collect::<Option<Vec<_>>>()?, locked })
    })
    .required()
}

/// A posted `Gem` (computed fields in the input are ignored, as pydantic does).
pub fn gem(f: Field<'_>, rng: &mut PyRandom) -> Option<Gem> {
    f.model(|m| {
        let id = {
            let f = m.field("id");
            if f.is_missing() { now_id() } else { f.int().required() }
        };
        let tier = m.field("tier").custom(|i| int_enum(i, 4)).required();
        let gem_type = m.field("type").custom(|i| int_enum(i, 2)).required();
        let element = m.field("element").custom(|i| int_enum(i, 4)).required();
        let restriction = m.field("restriction").custom(|i| int_enum(i, 2)).optional();
        let ability = m.field("ability").custom(|i| int_enum(i, 12)).optional();
        let level = m.field("level").int().required();
        let stats = m.field("stats").list(|f| stat(f, rng)).required();
        let augmentation = m.field("augmentation").float().optional();
        let attempts = m.field("attempts").int().default(0);
        let karma = m.field("karma").int().default(0);
        let spent = m.field("spent").dict(|f| f.int().required()).default(Vec::new());
        Some(Gem {
            id,
            tier,
            gem_type,
            element,
            restriction,
            ability,
            level,
            stats: stats.into_iter().collect::<Option<Vec<_>>>()?,
            augmentation,
            attempts,
            karma,
            spent,
        })
    })
    .required()
}

fn float(x: f64) -> Value {
    Value::from(x)
}

fn item_counts(rows: &[ItemCount]) -> Value {
    Value::Array(rows.iter().map(|c| json!({"item": c.item, "name": c.name, "count": c.count})).collect())
}

/// The `Gem` model dumped: stored fields, then the computed ones.
pub fn gem_json(g: &Gem, data: &Upgrades) -> Result<Value, GemError> {
    let mut stats = Vec::with_capacity(g.stats.len());
    for s in &g.stats {
        let mut containers = Vec::with_capacity(s.containers.len());
        for c in &s.containers {
            let augments: Vec<Value> = c.augments.iter().map(|a| json!({"type": a.kind, "count": a.count})).collect();
            containers.push(json!({
                "base": float(c.base),
                "augments": augments,
                "increase": float(c.increase(data)?.f64()),
                "value": float(c.value(data)?.f64()),
                "real_value": float(c.real_value(data)?),
            }));
        }
        stats.push(json!({
            "type": s.kind,
            "containers": containers,
            "locked": s.locked,
            "augmentation_progress": float(s.augmentation_progress(data)?.f64()),
        }));
    }
    let spent: Map<String, Value> = g.spent.iter().map(|(k, v)| (k.clone(), Value::from(*v))).collect();
    let is_max_level = g.is_max_level(data)?;
    let quality = g.quality(data)?;
    let power_rank = g.power_rank(data)?;
    let stat_values: Vec<Value> = g
        .stat_values(data)?
        .into_iter()
        .map(|(name, v)| Value::Object(Map::from_iter([(name.to_owned(), float(v))])))
        .collect();
    Ok(json!({
        "id": g.id,
        "tier": g.tier,
        "type": g.gem_type,
        "element": g.element,
        "restriction": g.restriction,
        "ability": g.ability,
        "level": g.level,
        "stats": stats,
        "augmentation": g.augmentation.map(float),
        "attempts": g.attempts,
        "karma": g.karma,
        "spent": spent,
        "ability_name": g.ability_name(),
        "gem_name": g.gem_name(),
        "is_max_level": is_max_level,
        "quality": float(quality),
        "power_rank": power_rank,
        "stat_values": stat_values,
    }))
}

/// `GemActionResult`.
pub fn action_json(applied: bool, g: &Gem, data: &Upgrades) -> Result<Value, GemError> {
    Ok(json!({"applied": applied, "gem": gem_json(g, data)?}))
}

/// `LevelUpResult`.
pub fn level_up_json(applied: bool, out: &LevelUp, g: &Gem, data: &Upgrades) -> Result<Value, GemError> {
    Ok(json!({
        "applied": applied,
        "outcome": out.outcome,
        "chance": float(out.chance),
        "double_chance": float(out.double_chance),
        "cost": item_counts(&out.cost),
        "guaranteed": out.guaranteed,
        "karma_gained": out.karma_gained,
        "gem": gem_json(g, data)?,
    }))
}

/// `LevelPlan`.
pub fn level_plan_json(
    tier: i64,
    gem_type: i64,
    element: i64,
    booster: Option<&str>,
    from_level: i64,
    plan: &LevelPlan,
) -> Value {
    let levels: Vec<Value> = plan
        .levels
        .iter()
        .map(|s| {
            json!({
                "level": s.level,
                "chance": float(s.chance),
                "double_chance": float(s.double_chance),
                "karma_per_fail": s.karma_per_fail,
                "expected_attempts": float(s.expected_attempts),
                "max_attempts": s.max_attempts,
                "cost": item_counts(&s.cost),
            })
        })
        .collect();
    let totals: Vec<Value> = plan.expected_total.iter().map(|(n, c)| json!({"name": n, "count": c})).collect();
    json!({
        "tier": tier,
        "type": gem_type,
        "element": element,
        "booster": booster,
        "from_level": from_level,
        "levels": levels,
        "expected_total": totals,
    })
}

fn focus_json(f: &FocusCost) -> Value {
    let totals: Map<String, Value> = f.recipe_totals.iter().map(|(k, v)| ((*k).to_owned(), Value::from(*v))).collect();
    json!({
        "key": f.key,
        "label": f.label,
        "superior": f.counts[0],
        "precise": f.counts[1],
        "rough": f.counts[2],
        "total": f.total(),
        "recipe_totals": totals,
    })
}

fn focus_totals(all: &[FocusCost]) -> (Value, Value) {
    let map: Map<String, Value> = all.iter().map(|f| (f.key.to_owned(), focus_json(f))).collect();
    let headline = all.iter().find(|f| f.key == "optimized_precise_rough").map_or(Value::Null, focus_json);
    (Value::Object(map), headline)
}

/// `EvaluationResult`.
pub fn evaluation_json(e: &Evaluation) -> Value {
    let stats: Vec<Value> = e
        .stats
        .iter()
        .map(|s| {
            json!({
                "type": s.stat,
                "display_name": cat::name(&cat::STATS, s.stat),
                "entered_value": float(s.entered_value),
                "extra_containers": s.extra_containers,
                "containers": s.containers,
                "progress": float(s.progress),
                "quality_percent": float(s.quality_percent),
                "estimated_pr_contribution": float(s.estimated_pr_contribution),
                "is_within_range": s.is_within_range,
                "raw_progress": float(s.raw_progress),
                "threshold_progress": float(s.threshold_progress),
            })
        })
        .collect();
    let (totals, headline) = focus_totals(&e.focus_totals);
    json!({
        "tier": e.tier,
        "type": e.gem_type,
        "type_name": cat::name(&cat::TYPES, e.gem_type),
        "element": e.element,
        "element_name": cat::name(&cat::ELEMENTS, e.element),
        "restriction": e.restriction,
        "restriction_name": e.restriction.map_or("Any", |r| cat::name(&cat::RESTRICTIONS, r)),
        "level": e.level,
        "quality": float(e.quality),
        "quality_percent": float(e.quality_percent),
        "calculated_power_rank": e.calculated_power_rank,
        "has_issues": !e.issues.is_empty(),
        "issues": e.issues,
        "stats": stats,
        "focus_totals": totals,
        "headline_cost": headline,
        "available_extra_containers": e.available_extra_containers,
        "guessed_distribution": e.guessed_distribution,
    })
}

/// `SimpleEvaluationResult`.
pub fn simple_json(e: &SimpleEvaluation) -> Value {
    let (totals, headline) = focus_totals(&e.focus_totals);
    json!({
        "tier": e.tier,
        "tier_name": cat::name(&cat::TIERS, e.tier),
        "type": e.gem_type,
        "type_name": cat::name(&cat::TYPES, e.gem_type),
        "level": e.level,
        "power_rank": e.power_rank,
        "min_power_rank": e.min_power_rank,
        "max_power_rank": e.max_power_rank,
        "quality": float(e.quality),
        "quality_percent": float(e.quality_percent),
        "is_within_range": e.is_within_range,
        "distance": e.distance,
        "containers": e.containers,
        "boosts": e.containers - 3,
        "focus_totals": totals,
        "headline_cost": headline,
    })
}

/// `GemStatRange`.
pub fn stat_range_json(r: &StatRange) -> Value {
    json!({
        "stat_type": r.stat,
        "stat_display_name": cat::name(&cat::STATS, r.stat),
        "element": r.element,
        "containers": r.containers,
        "min_value": float(r.min_value),
        "max_value": float(r.max_value),
        "stat_base": float(r.stat_base),
        "thresholds": [float(r.thresholds[0]), float(r.thresholds[1])],
    })
}

/// `BuildResult`; `light: int` takes only whole values (pydantic's int check
/// on a float), so a fractional one is the ValidationError Python raises.
pub fn build_row_json(r: &BuildRow) -> Option<Value> {
    if !(r.light.is_finite() && r.light.fract() == 0.0) {
        return None;
    }
    Some(json!({
        "rank": r.rank,
        "layout": r.layout,
        "base_dmg": float(r.base_dmg),
        "crit_dmg": float(r.crit_dmg),
        "light": r.light as i64,
        "bonus_dmg": float(r.bonus_dmg),
        "crit_bonus": float(r.crit_bonus),
        "total_dmg": float(r.total_dmg),
        "class_bonus": r.class_bonus.map(float),
        "coefficient": float(r.coefficient),
    }))
}

/// `HealthOptimizerResponse` (result rows lead with the inherited totals).
pub fn health_json(plan: &HealthPlan) -> Value {
    const KEYS: [&str; 3] = ["critical_hit", "maximum_health", "maximum_health_per"];
    let counts =
        |c: [usize; 3]| -> Value { KEYS.iter().zip(c).map(|(k, n)| ((*k).to_owned(), Value::from(n))).collect() };
    let results: Vec<Value> = plan
        .results
        .iter()
        .map(|r| {
            json!({
                "critical_hit": float(r.totals[0]),
                "maximum_health": float(r.totals[1]),
                "maximum_health_per": float(r.totals[2]),
                "rank": r.rank,
                "layout": r.layout(),
                "empowered": counts(r.empowered),
                "lesser": counts(r.lesser),
                "health": float(r.health),
            })
        })
        .collect();
    let count = results.len();
    json!({
        "base": {
            "critical_hit": float(plan.base[0]),
            "maximum_health": float(plan.base[1]),
            "maximum_health_per": float(plan.base[2]),
        },
        "results": results,
        "count": count,
    })
}

/// A value of an `EmpoweredGem` field, validated like pydantic's lax mode;
/// `None` is the ValidationError (a 500 in Python).
fn as_str(v: Option<&Value>) -> Option<Value> {
    v.filter(|v| v.is_string()).cloned()
}

fn as_float(v: Option<&Value>) -> Option<Value> {
    match v? {
        Value::Number(n) => n.as_f64().map(Value::from),
        Value::Bool(b) => Some(Value::from(f64::from(u8::from(*b)))),
        _ => None,
    }
}

fn as_int(v: &Value) -> Option<Value> {
    match v {
        Value::Number(n) if n.is_i64() || n.is_u64() => Some(v.clone()),
        Value::Number(n) => n.as_f64().filter(|f| f.fract() == 0.0 && f.is_finite()).map(|f| Value::from(f as i64)),
        Value::Bool(b) => Some(Value::from(i64::from(*b))),
        _ => None,
    }
}

fn objects(v: Option<&Value>) -> Option<&Vec<Value>> {
    v?.as_array()
}

/// `EmpoweredGem` projected from one `gem_abilities.json` entry.
fn empowered_gem(g: &Value) -> Option<Value> {
    let tiers: Option<Vec<Value>> = objects(g.get("tiers"))?.iter().map(as_int).collect();
    let prefabs: Option<Vec<Value>> = objects(g.get("prefabs"))?.iter().map(|p| as_str(Some(p))).collect();
    let stats: Option<Vec<Value>> = objects(g.get("stats"))?
        .iter()
        .map(|s| {
            Some(json!({
                "name": as_str(s.get("name"))?,
                "stat": as_str(s.get("stat"))?,
                "op": as_str(s.get("op"))?,
                "value": as_float(s.get("value"))?,
                "amount": as_float(s.get("amount"))?,
                "prefab": as_str(s.get("prefab"))?,
            }))
        })
        .collect();
    let stages: Option<Vec<Value>> = objects(g.get("stages"))?
        .iter()
        .map(|s| {
            Some(json!({
                "name": as_str(s.get("name"))?,
                "prefab": as_str(s.get("prefab"))?,
                "base": as_float(s.get("base"))?,
                "multiplier": as_float(s.get("multiplier"))?,
            }))
        })
        .collect();
    Some(json!({
        "element": as_str(g.get("element"))?,
        "slug": as_str(g.get("slug"))?,
        "name": as_str(g.get("name"))?,
        "description": as_str(g.get("description"))?,
        "tiers": tiers?,
        "prefabs": prefabs?,
        "stats": stats?,
        "stages": stages?,
    }))
}

fn lookup(names: &[&str]) -> Vec<Value> {
    names.iter().enumerate().map(|(i, n)| json!({"id": i + 1, "name": n})).collect()
}

/// `GemLookups(**gem_lookups())`.
pub fn lookups_json(data: &Upgrades, abilities: &Value) -> Result<Value, GemError> {
    let bad = || GemError::Unhandled("gem_abilities.json does not fit EmpoweredGem".into());
    let mut tiers = Vec::new();
    for t in 1..=4u8 {
        tiers.push(json!({"id": t, "name": cat::name(&cat::TIERS, t), "max_level": data.max_level(t, LESSER)?}));
    }
    let stat_types: Vec<Value> = (1..=10u8)
        .map(|s| json!({"id": s, "name": cat::name(&cat::STATS, s), "retired": cat::RETIRED.contains(&s)}))
        .collect();
    let mut augment_types = Vec::new();
    for a in 1..=3u8 {
        let pct = data.augment_base(a)?;
        augment_types.push(json!({"id": a, "name": cat::name(&cat::AUGMENTS, a), "increase_percent": float(pct)}));
    }
    let by_element: Map<String, Value> =
        (1..=4u8).map(|e| (cat::name(&cat::ELEMENTS, e).to_owned(), json!(cat::element_abilities(e)))).collect();
    let boosters: Vec<Value> = data
        .boosters()
        .iter()
        .map(|b| {
            json!({
                "id": b.id,
                "name": b.name,
                "chance_multiplier": float(b.chance_multiplier),
                "double_multiplier": float(b.double_multiplier),
            })
        })
        .collect();
    let empowered =
        abilities.as_array().ok_or_else(bad)?.iter().map(empowered_gem).collect::<Option<Vec<_>>>().ok_or_else(bad)?;
    Ok(json!({
        "tiers": tiers,
        "types": lookup(&cat::TYPES),
        "elements": lookup(&cat::ELEMENTS),
        "restrictions": lookup(&cat::RESTRICTIONS),
        "stat_types": stat_types,
        "augment_types": augment_types,
        "abilities": lookup(&cat::ABILITIES),
        "abilities_by_element": by_element,
        "karma_max": cat::KARMA_MAX,
        "boosters": boosters,
        "empowered_gems": empowered,
    }))
}

pub fn option_entries(rows: impl Iterator<Item = (String, String)>) -> Vec<Value> {
    rows.map(|(key, label)| json!({"key": key, "label": label})).collect()
}

/// `BuildOptions(**build_options())`.
pub fn build_options_json(classes: &[String], allies: Vec<Value>, foods: Vec<Value>) -> Value {
    json!({
        "build_type": ["Light", "Farm", "Health"],
        "character": classes,
        "ally": allies,
        "food": foods,
        "critical_damage_count": {"min": 0, "max": 3, "default": 3},
        "flags": ["berserker_battler", "no_face", "subclass_active", "litany", "ally_buff", "bounty_hunt", "high_precision"],
        "notes": {
            "subclass": "Same options as character.",
            "ally": "Use 'boot_clown' for no ally. Ally stats are the level-30 values.",
            "ally_buff": "Blessing of the Lilypad, on by default: multiplies the ally's level-30 Light by 1.155 and its Physical/Magic/Critical Damage by 1.31. Ally stats only.",
            "food": "Use \"\" for none.",
            "high_precision": "Round every result field to 8 decimals instead of 1-2.",
            "light": "Farm builds only - target base light; 0 disables light targeting.",
            "star_chart": "Optional star-chart build code (SC:/v2: compact, or base64).",
            "bounty_hunt": "The Sundered Uplands boss buff. Ignored unless the star chart code unlocks it: 'Bounty Hunt Boon' gives +10% Physical/Magic Damage, and 'Bounty Hunt' upgrades that same buff to +15%. Check bounty_hunt.available on /parse-star-chart before offering the toggle.",
        },
    })
}

/// `EvalStatInput`.
pub fn eval_stat(f: Field<'_>) -> Option<StatInput> {
    f.model(|m| {
        let stat = m.field("type").int().required();
        let value = m.field("value").float().required();
        let extra_containers = m.field("extra_containers").int().default(0);
        Some(StatInput { stat, value, extra_containers })
    })
    .required()
}

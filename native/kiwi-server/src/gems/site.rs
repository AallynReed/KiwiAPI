//! The `/site/gems/*` proxies of `app/site/router.py`: the `/v1/gems` compute
//! behind the site gate, answered as the raw dicts (`jsonable_encoder` +
//! `JSONResponse`) instead of the `/v1` response models.

use axum::{
    Router,
    extract::State,
    http::{HeaderValue, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use kiwi_core::gems::{
    builds::{self, BuildRow},
    catalog as cat,
    evaluator::{self, Evaluation, FocusCost, focus_breakdown},
    pynum::{PyNum, py_sum},
};
use serde_json::{Map, Value, json};

use super::{
    BuildReq, EvaluateReq, HealthReq, Reply, SimpleReq, StatRangeQ, data, evaluator_error, run_build_options,
    run_lookups, run_stat_range, star_chart::Preview, store, wire,
};
use crate::{
    gamedata::Gamedata,
    pyjson::PyJson,
    site_live::flags::SiteGate,
    state::AppState,
    validation::{DepsThenBody, FromParams, Params, Query},
};

const HARD_CACHE: &str = "public, max-age=3600";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/site/gems/lookups", get(lookups))
        .route("/site/gems/evaluate", post(evaluate))
        .route("/site/gems/evaluate-simple", post(evaluate_simple))
        .route("/site/gems/stat-range", get(stat_range))
        .route("/site/gems/builds/options", get(build_options))
        .route("/site/gems/builds/calculate", post(calculate))
        .route("/site/gems/builds/health-optimizer", post(health))
        .route("/site/gems/parse-star-chart", get(parse_star_chart))
}

/// A `JSONResponse` of the reply (an unhandled error unwinds like `/v1`'s).
fn respond(r: Reply, cache: Option<&'static str>) -> Response {
    match r {
        Reply::Json(v) => {
            let mut res = PyJson(v).into_response();
            if let Some(c) = cache {
                res.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static(c));
            }
            res
        }
        Reply::Error(e) => e.into_response(),
        Reply::Unhandled(msg) => {
            tracing::error!("site gems: unhandled error: {msg}");
            std::panic::resume_unwind(Box::new(msg))
        }
    }
}

/// `gem_lookups()`: the `empowered_gems` file passed through as decoded.
fn site_lookups(s: &Gamedata) -> Reply {
    match run_lookups(s) {
        Reply::Json(mut v) => {
            v["empowered_gems"] = data::abilities(s).as_ref().clone();
            Reply::Json(v)
        }
        other => other,
    }
}

async fn lookups(_: SiteGate, State(state): State<AppState>) -> Response {
    respond(site_lookups(store(&state)), Some(HARD_CACHE))
}

async fn stat_range(_: SiteGate, State(state): State<AppState>, Query(q): Query<StatRangeQ>) -> Response {
    respond(run_stat_range(store(&state), &q), Some(HARD_CACHE))
}

async fn build_options(_: SiteGate, State(state): State<AppState>) -> Response {
    respond(run_build_options(store(&state)), Some(HARD_CACHE))
}

fn cost_json(f: &FocusCost, with_key: bool) -> Map<String, Value> {
    let mut m = Map::new();
    if with_key {
        m.insert("key".into(), f.key.into());
    }
    m.insert("label".into(), f.label.into());
    for (k, n) in ["superior", "precise", "rough"].iter().zip(f.counts) {
        m.insert((*k).into(), n.into());
    }
    m.insert("total".into(), f.total().into());
    m.insert(
        "recipe_totals".into(),
        Value::Object(f.recipe_totals.iter().map(|(k, n)| ((*k).to_owned(), Value::from(*n))).collect()),
    );
    m
}

/// `gem_focus_totals` and the headline (`optimized_precise_rough` + `strategy`).
fn totals_and_headline(all: &[FocusCost]) -> (Value, Value) {
    let totals: Map<String, Value> =
        all.iter().map(|f| (f.key.to_owned(), Value::Object(cost_json(f, true)))).collect();
    let headline = all.iter().find(|f| f.key == "optimized_precise_rough").map_or(Value::Null, |f| {
        let mut m = cost_json(f, true);
        m.insert("strategy".into(), "optimized_precise_rough".into());
        Value::Object(m)
    });
    (Value::Object(totals), headline)
}

/// `_build_focus_plan(display_progress, containers)` of one stat.
fn focus_plan(current_percent: f64, containers: i64) -> Value {
    let b = focus_breakdown(current_percent, containers);
    let per: Map<String, Value> = b
        .per_container
        .iter()
        .map(|(key, c)| {
            let total = c.iter().fold(0i64, |a, &n| a.saturating_add(n));
            ((*key).to_owned(), json!({"superior": c[0], "precise": c[1], "rough": c[2], "total": total}))
        })
        .collect();
    let totals: Map<String, Value> =
        b.totals.iter().map(|f| (f.key.to_owned(), Value::Object(cost_json(f, false)))).collect();
    json!({
        "current_percent": current_percent,
        "remaining_percent": b.remaining_percent,
        "per_container": per,
        "totals": totals,
    })
}

/// `{**out["result"], available_extra_containers, guessed_distribution}`.
pub fn evaluation_json(e: &Evaluation) -> Value {
    let stats: Vec<Value> = e
        .stats
        .iter()
        .map(|s| {
            json!({
                "type": s.stat,
                "display_name": cat::name(&cat::STATS, s.stat),
                "entered_value": s.entered_value,
                "extra_containers": s.extra_containers,
                "containers": s.containers,
                "progress": s.progress,
                "quality_percent": s.quality_percent,
                "estimated_pr_contribution": s.estimated_pr_contribution,
                "is_within_range": s.is_within_range,
                "raw_progress": s.raw_progress,
                "threshold_progress": s.threshold_progress,
                "focus_plan": focus_plan(s.quality_percent, s.containers),
            })
        })
        .collect();
    let (totals, headline) = totals_and_headline(&e.focus_totals);
    json!({
        "type": e.gem_type,
        "type_name": cat::name(&cat::TYPES, e.gem_type),
        "element": e.element,
        "element_name": cat::name(&cat::ELEMENTS, e.element),
        "restriction": e.restriction,
        "restriction_name": e.restriction.map_or("Any", |r| cat::name(&cat::RESTRICTIONS, r)),
        "quality": e.quality,
        "quality_percent": e.quality_percent,
        "calculated_power_rank": e.calculated_power_rank,
        "has_issues": !e.issues.is_empty(),
        "issues": e.issues,
        "focus_totals": totals,
        "headline_cost": headline,
        "stats": stats,
        "level": e.level,
        "tier": e.tier,
        "available_extra_containers": e.available_extra_containers,
        "guessed_distribution": e.guessed_distribution,
    })
}

fn run_evaluate(s: &Gamedata, req: EvaluateReq) -> Reply {
    let d = data::gems(s);
    match evaluator::evaluate(&d.up, req.tier, req.gem_type, req.level, &req.stats, req.auto_guess) {
        Ok(e) => Reply::Json(evaluation_json(&e)),
        Err(e) => evaluator_error(e),
    }
}

async fn evaluate(
    State(state): State<AppState>,
    DepsThenBody(_, req): DepsThenBody<SiteGate, EvaluateReq>,
) -> Response {
    respond(run_evaluate(store(&state), req), None)
}

fn run_evaluate_simple(s: &Gamedata, req: SimpleReq) -> Reply {
    let d = data::gems(s);
    match evaluator::evaluate_simple(&d.up, req.tier, req.gem_type, req.power_rank, req.level) {
        Ok(e) => {
            let mut v = wire::simple_json(&e);
            if let Some(h) = v.get_mut("headline_cost").and_then(Value::as_object_mut) {
                h.insert("strategy".into(), "optimized_precise_rough".into());
            }
            Reply::Json(v)
        }
        Err(e) => evaluator_error(e),
    }
}

async fn evaluate_simple(
    State(state): State<AppState>,
    DepsThenBody(_, req): DepsThenBody<SiteGate, SimpleReq>,
) -> Response {
    respond(run_evaluate_simple(store(&state), req), None)
}

// --- builds ---------------------------------------------------------------------

fn add(a: PyNum, b: PyNum) -> PyNum {
    match (a, b) {
        (PyNum::Int(x), PyNum::Int(y)) => PyNum::Int(x.saturating_add(y)),
        _ => PyNum::Float(a.f64() + b.f64()),
    }
}

fn truthy(v: Option<&Value>) -> bool {
    data::truthy(v)
}

/// Whether `fourth` and `fifth` are still Python ints when `calculate_builds`
/// formats its rows, and the raw `class_bonus` it echoes back.
fn row_types(s: &Gamedata, req: &BuildReq, chart: Option<&kiwi_core::gems::builds::Chart>) -> (bool, bool, Value) {
    let c = &req.config;
    let classes = s.load("classes.json");
    let class = classes.as_deref().and_then(Value::as_array).and_then(|rows| {
        rows.iter().rev().find(|r| r.get("name").and_then(Value::as_str) == Some(c.character.as_str()))
    });
    let magic = class.and_then(|k| k.get("damage_type")).and_then(Value::as_str) == Some("Magic");
    let damage_type = if c.build_type == "Health" {
        "Maximum Health"
    } else if magic {
        "Magic Damage"
    } else {
        "Physical Damage"
    };
    let class_bonus = class
        .and_then(|k| k.get("bonuses"))
        .and_then(Value::as_array)
        .and_then(|b| b.iter().find(|b| b.get("name").and_then(Value::as_str) == Some(damage_type)))
        .and_then(|b| b.get("value").cloned())
        .unwrap_or(Value::Null);
    if c.build_type == "Health" {
        return (true, true, class_bonus);
    }
    let bonus = s.load("builds/bonus_damage.json");
    let mut fourth = match bonus.as_deref().and_then(Value::as_object) {
        Some(m) if !m.is_empty() => py_sum(m.values().map(data::py_num)),
        _ => PyNum::Float(0.0),
    };
    let mut fifth = PyNum::Int(100);
    let foods = s.load("builds/food.json");
    if !c.food.is_empty()
        && let Some(food) = foods.as_deref().and_then(|f| f.get(&c.food))
    {
        for stat in food.get("stats").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default() {
            if stat.get("name").and_then(Value::as_str) == Some(damage_type) && truthy(stat.get("percentage")) {
                fourth = add(fourth, data::py_num(stat.get("value").unwrap_or(&Value::Null)));
            }
        }
    }
    let allies = s.load("builds/ally.json");
    if !c.ally.is_empty()
        && let Some(ally) = allies.as_deref().and_then(|a| a.get(&c.ally))
    {
        for stat in ally.get("stats").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default() {
            if !truthy(stat.get("percentage")) {
                continue;
            }
            let raw = data::py_num(stat.get("value").unwrap_or(&Value::Null));
            let value = if c.ally_buff { PyNum::Float(raw.f64()) } else { raw };
            match stat.get("name").and_then(Value::as_str) {
                Some(n) if n == damage_type => fourth = add(fourth, value),
                Some("Critical Damage") => fifth = add(fifth, value),
                _ => {}
            }
        }
    }
    if c.subclass_active && c.subclass == "Gunslinger" {
        fourth = add(fourth, PyNum::Float(5.5));
    }
    if let Some(chart) = chart {
        let has = |name: &str| chart.stats.iter().any(|s| s.0 == name || s.0 == format!("{name} Bonus"));
        if has(damage_type) || (c.bounty_hunt && chart.bounty.is_some()) {
            fourth = PyNum::Float(fourth.f64());
        }
        if has("Critical Damage") {
            fifth = PyNum::Float(fifth.f64());
        }
    }
    (matches!(fourth, PyNum::Int(_)), matches!(fifth, PyNum::Int(_)), class_bonus)
}

/// One `calculate_builds` row as the raw dict.
pub fn raw_row(r: &BuildRow, fourth_int: bool, fifth_int: bool, class_bonus: &Value) -> Value {
    let maybe_int = |v: f64, int: bool| if int { Value::from(v as i64) } else { Value::from(v) };
    json!({
        "rank": r.rank,
        "layout": r.layout,
        "base_dmg": r.base_dmg,
        "crit_dmg": r.crit_dmg,
        "light": r.light,
        "bonus_dmg": maybe_int(r.bonus_dmg, fourth_int),
        "crit_bonus": maybe_int(r.crit_bonus, fifth_int),
        "total_dmg": r.total_dmg,
        "class_bonus": class_bonus,
        "coefficient": r.coefficient,
    })
}

fn run_calculate(s: &Gamedata, req: BuildReq) -> Reply {
    let d = data::builds(s);
    let gem_stats = match &d.gem_stats {
        Ok(g) => g,
        Err(e) => return Reply::Unhandled(e.message().to_owned()),
    };
    let foods: Vec<(String, Vec<_>)> = d.foods.iter().map(|f| (f.key.clone(), f.stats.clone())).collect();
    let allies: Vec<(String, Vec<_>)> = d.allies.iter().map(|a| (a.key.clone(), a.stats.clone())).collect();
    let sums = |name: &str| d.sums.get(name).copied().unwrap_or(0.0);
    let input = builds::BuildData {
        classes: &d.classes,
        foods: &foods,
        allies: &allies,
        face_damage: d.face_damage,
        sums: &sums,
        gem_stats,
    };
    let chart = req.star_chart.as_deref().filter(|c| !c.is_empty()).map(|c| d.chart.parse(c));
    let rows = match builds::calculate(&input, &req.config, chart.as_ref()) {
        Ok(rows) => rows,
        Err(msg) => return Reply::bad_request(msg),
    };
    let (fourth_int, fifth_int, class_bonus) = row_types(s, &req, chart.as_ref());
    let results: Vec<Value> = rows.iter().map(|r| raw_row(r, fourth_int, fifth_int, &class_bonus)).collect();
    let count = results.len();
    Reply::Json(json!({"results": results, "count": count}))
}

async fn calculate(State(state): State<AppState>, DepsThenBody(_, req): DepsThenBody<SiteGate, BuildReq>) -> Response {
    let s = store(&state);
    let reply = tokio::task::spawn_blocking(move || run_calculate(s, req)).await;
    match reply {
        Ok(r) => respond(r, None),
        Err(e) => std::panic::resume_unwind(e.into_panic()),
    }
}

/// `optimize_health(...)` as the raw dict: result rows lead with their rank.
pub fn reorder_health(v: Value) -> Value {
    let Value::Object(mut m) = v else { return v };
    if let Some(Value::Array(rows)) = m.get_mut("results") {
        for row in rows.iter_mut() {
            if let Value::Object(r) = row {
                let order = [
                    "rank",
                    "layout",
                    "empowered",
                    "lesser",
                    "critical_hit",
                    "maximum_health",
                    "maximum_health_per",
                    "health",
                ];
                *row = Value::Object(order.iter().filter_map(|k| r.remove(*k).map(|v| ((*k).to_owned(), v))).collect());
            }
        }
    }
    Value::Object(m)
}

async fn health(State(state): State<AppState>, DepsThenBody(_, req): DepsThenBody<SiteGate, HealthReq>) -> Response {
    let reply = match super::run_health(store(&state), req) {
        Reply::Json(v) => Reply::Json(reorder_health(v)),
        other => other,
    };
    respond(reply, None)
}

// --- star chart preview -------------------------------------------------------------

pub struct ChartQuery {
    code: String,
}

impl FromParams for ChartQuery {
    fn from_params(p: &mut Params<'_>) -> Self {
        ChartQuery { code: p.query("code").str().max_len(8192).default(String::new()) }
    }
}

/// `parse_star_chart(code)` as the raw dict.
fn preview_json(p: &Preview) -> Value {
    let stats: Map<String, Value> =
        p.stats.iter().map(|(n, flat, pct)| (n.clone(), json!({"flat": flat, "pct": pct}))).collect();
    let bounty = match &p.bounty {
        Some((name, physical, magic)) => json!({"available": true, "name": name, "physical": physical, "magic": magic}),
        None => json!({"available": false, "name": null, "physical": 0.0, "magic": 0.0}),
    };
    json!({"stats": stats, "abilities": p.abilities, "paths_count": p.paths_count, "bounty_hunt": bounty})
}

async fn parse_star_chart(_: SiteGate, State(state): State<AppState>, Query(q): Query<ChartQuery>) -> Response {
    let d = data::builds(store(&state));
    let body = match &d.gem_stats {
        Ok(_) => preview_json(&d.chart.preview(&q.code)),
        Err(_) => json!({"stats": {}, "abilities": [], "paths_count": 0}),
    };
    PyJson(body).into_response()
}

#[cfg(test)]
#[path = "site_tests.rs"]
mod tests;

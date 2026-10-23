//! `/v1/gems/*` (`app/trove/router.py`, `app/trove/gems/`): the stateless gem
//! simulator, evaluator, build ranker and Health Optimizer, all behind
//! `require_scope("gems:read")`.
//!
//! POST bodies are read and JSON-parsed before the scope check, then
//! validated after it, which is FastAPI's order. Random draws use CPython's
//! generator ([`PyRandom`]), seeded from the OS, in Python's call order.

mod data;
pub mod site;
mod star_chart;
mod wire;

#[cfg(test)]
mod tests;

use std::cell::RefCell;

use axum::{
    Router,
    extract::{Request, State},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use kiwi_core::gems::{
    GemError,
    builds::{self, BuildConfig},
    evaluator::{self, StatInput},
    model::{CreateParams, Gem},
    pyrandom::PyRandom,
    upgrades::Upgrades,
};
use serde_json::Value;

use crate::{
    auth::{RequireScope, extract::require_scope, scopes::GEMS_READ},
    error::ApiError,
    gamedata::Gamedata,
    pyjson::PydanticJson,
    ratelimit::RateLimitInfo,
    state::AppState,
    validation::{Field, FromBody, FromParams, Params, Query, read_body, validate},
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/gems/lookups", get(lookups))
        .route("/v1/gems/generate", post(|s: State<AppState>, r: Request| gated(s, r, run_generate)))
        .route("/v1/gems/augment", post(|s: State<AppState>, r: Request| gated(s, r, run_augment)))
        .route("/v1/gems/spark", post(|s: State<AppState>, r: Request| gated(s, r, run_spark)))
        .route("/v1/gems/flare", post(|s: State<AppState>, r: Request| gated(s, r, run_flare)))
        .route("/v1/gems/level-up", post(|s: State<AppState>, r: Request| gated(s, r, run_level_up)))
        .route("/v1/gems/level-costs", get(level_costs))
        .route("/v1/gems/set-level", post(|s: State<AppState>, r: Request| gated(s, r, run_set_level)))
        .route("/v1/gems/evaluate", post(|s: State<AppState>, r: Request| gated(s, r, run_evaluate)))
        .route("/v1/gems/evaluate-simple", post(|s: State<AppState>, r: Request| gated(s, r, run_evaluate_simple)))
        .route("/v1/gems/stat-range", get(stat_range))
        .route("/v1/gems/builds/options", get(build_options))
        .route("/v1/gems/builds/calculate", post(|s: State<AppState>, r: Request| gated(s, r, run_builds)))
        .route("/v1/gems/builds/health-optimizer", post(|s: State<AppState>, r: Request| gated(s, r, run_health)))
}

/// What a route's body produces: the model, a raised `APIError`, or an
/// exception Python leaves unhandled.
#[derive(Debug)]
enum Reply {
    Json(Value),
    Error(ApiError),
    Unhandled(String),
}

impl Reply {
    fn bad_request(message: impl Into<String>) -> Self {
        Reply::Error(ApiError::bad_request(message))
    }

    fn json(r: Result<Value, GemError>) -> Self {
        r.map_or_else(|e| Reply::Unhandled(e.message().to_owned()), Reply::Json)
    }

    /// `except (<caught>) as e: raise _bad_request(prefix + str(e))`.
    fn caught(e: GemError, prefix: &str, caught: fn(&GemError) -> bool) -> Self {
        if caught(&e) {
            Reply::bad_request(format!("{prefix}{}", e.message()))
        } else {
            Reply::Unhandled(e.message().to_owned())
        }
    }

    /// The response, with the scope's `X-RateLimit-*` on a model. An
    /// unhandled error unwinds to the request-id layer, which answers the
    /// 500 the way Python's error boundary does (no headers from the layers
    /// in between).
    fn respond(self, rate_limit: RateLimitInfo) -> Response {
        match self {
            Reply::Json(v) => (rate_limit, PydanticJson(v)).into_response(),
            Reply::Error(e) => e.into_response(),
            Reply::Unhandled(msg) => {
                tracing::error!("gems: unhandled error: {msg}");
                std::panic::resume_unwind(Box::new(msg))
            }
        }
    }
}

fn is_value(e: &GemError) -> bool {
    matches!(e, GemError::Value(_) | GemError::Evaluator(_))
}

fn store(state: &AppState) -> &'static Gamedata {
    Gamedata::global(state.settings())
}

/// `evaluate_gem(...)["result"]` as JSON, for the `/gem` slash command.
pub fn evaluate_json(
    gd: &Gamedata,
    tier: i64,
    gem_type: i64,
    level: i64,
    stats: &[StatInput],
    auto_guess: bool,
) -> Result<Value, GemError> {
    let data = data::gems(gd);
    evaluator::evaluate(&data.up, tier, gem_type, level, stats, auto_guess).map(|e| wire::evaluation_json(&e))
}

thread_local! {
    static RNG: RefCell<Option<PyRandom>> = const { RefCell::new(None) };
}

/// The process's `random` instance (one per thread here), seeded like
/// CPython's from 2496 bytes of OS entropy.
fn with_rng<T>(f: impl FnOnce(&mut PyRandom) -> T) -> T {
    RNG.with(|cell| {
        let mut slot = cell.borrow_mut();
        let rng = slot.get_or_insert_with(|| {
            let mut bytes = [0u8; 624 * 4];
            if getrandom::fill(&mut bytes).is_err() {
                let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
                bytes[..16].copy_from_slice(&t.as_nanos().to_le_bytes());
            }
            let key: Vec<u32> = bytes.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
            PyRandom::from_key(&key)
        });
        f(rng)
    })
}

/// A POST route: body read (malformed JSON is a 422 before auth), the scope,
/// then validation and the route.
async fn gated<B: FromBody>(State(state): State<AppState>, req: Request, run: fn(&Gamedata, B) -> Reply) -> Response {
    let (parts, body) = req.into_parts();
    let body = match read_body(&parts.headers, body).await {
        Ok(b) => b,
        Err(e) => return e.into_response(),
    };
    let ctx = match require_scope(&state, &parts, GEMS_READ).await {
        Ok(ctx) => ctx,
        Err(r) => return r.into_response(),
    };
    match validate::<B>(&body) {
        Ok(req) => run(store(&state), req).respond(ctx.rate_limit),
        Err(e) => e.into_response(),
    }
}

fn body<T>(
    body: Field<'_>,
    f: impl FnOnce(&mut crate::validation::Model<'_>, &mut PyRandom) -> Option<T>,
) -> Option<T> {
    with_rng(|rng| body.model(|m| f(m, rng)).required())
}

struct GenerateReq {
    params: CreateParams,
}

impl FromBody for GenerateReq {
    fn from_body(b: Field<'_>) -> Option<Self> {
        body(b, |m, _| {
            let tier = m.field("tier").int().optional();
            let gem_type = m.field("type").int().optional();
            let element = m.field("element").int().optional();
            let restriction = m.field("restriction").int().optional();
            let level = m.field("level").int().default(1);
            let augmentation = m.field("augmentation").float().optional();
            let retired_stats = m.field("retired_stats").bool().default(false);
            Some(GenerateReq {
                params: CreateParams { tier, gem_type, element, restriction, augmentation, level, retired_stats },
            })
        })
    }
}

fn run_generate(s: &Gamedata, req: GenerateReq) -> Reply {
    let data = data::gems(s);
    let id = wire::now_id();
    match with_rng(|rng| Gem::create(&data.up, &req.params, id, rng)) {
        Ok(g) => Reply::json(wire::gem_json(&g, &data.up)),
        Err(e) => Reply::caught(e, "Invalid gem parameters: ", is_value),
    }
}

struct AugmentReq {
    gem: Gem,
    position: i64,
    augment: i64,
}

impl FromBody for AugmentReq {
    fn from_body(b: Field<'_>) -> Option<Self> {
        body(b, |m, rng| {
            let gem = wire::gem(m.field("gem"), rng);
            let position = m.field("stat_position").int().ge(0).required();
            let augment = m.field("augment_type").int().default(1);
            Some(AugmentReq { gem: gem?, position, augment })
        })
    }
}

fn action(data: &Upgrades, gem: &Gem, applied: Result<bool, GemError>, caught: fn(&GemError) -> bool) -> Reply {
    match applied {
        Ok(applied) => Reply::json(wire::action_json(applied, gem, data)),
        Err(e) => Reply::caught(e, "", caught),
    }
}

fn run_augment(s: &Gamedata, mut req: AugmentReq) -> Reply {
    let data = data::gems(s);
    let applied = req.gem.augment_stat(&data.up, req.position, req.augment);
    action(&data.up, &req.gem, applied, |e| matches!(e, GemError::Index(_)) || is_value(e))
}

struct PositionReq {
    gem: Gem,
    position: i64,
}

impl FromBody for PositionReq {
    fn from_body(b: Field<'_>) -> Option<Self> {
        body(b, |m, rng| {
            let gem = wire::gem(m.field("gem"), rng);
            let position = m.field("stat_position").int().ge(0).required();
            Some(PositionReq { gem: gem?, position })
        })
    }
}

fn is_index(e: &GemError) -> bool {
    matches!(e, GemError::Index(_))
}

fn run_spark(s: &Gamedata, mut req: PositionReq) -> Reply {
    let data = data::gems(s);
    let applied = with_rng(|rng| req.gem.spark_stat(req.position, rng));
    action(&data.up, &req.gem, applied, is_index)
}

fn run_flare(s: &Gamedata, mut req: PositionReq) -> Reply {
    let data = data::gems(s);
    let applied = with_rng(|rng| req.gem.flare_stat(req.position, rng));
    action(&data.up, &req.gem, applied, is_index)
}

struct LevelUpReq {
    gem: Gem,
    booster: Option<String>,
}

impl FromBody for LevelUpReq {
    fn from_body(b: Field<'_>) -> Option<Self> {
        body(b, |m, rng| {
            let gem = wire::gem(m.field("gem"), rng);
            let booster = m.field("booster").str().optional();
            Some(LevelUpReq { gem: gem?, booster })
        })
    }
}

fn run_level_up(s: &Gamedata, mut req: LevelUpReq) -> Reply {
    let data = data::gems(s);
    let level = req.gem.level;
    match with_rng(|rng| req.gem.level_up(&data.up, req.booster.as_deref(), rng)) {
        Ok(out) => Reply::json(wire::level_up_json(req.gem.level > level, &out, &req.gem, &data.up)),
        Err(e) => Reply::caught(e, "", is_value),
    }
}

struct SetLevelReq {
    gem: Gem,
    level: i64,
}

impl FromBody for SetLevelReq {
    fn from_body(b: Field<'_>) -> Option<Self> {
        body(b, |m, rng| {
            let gem = wire::gem(m.field("gem"), rng);
            let level = m.field("level").int().ge(1).required();
            Some(SetLevelReq { gem: gem?, level })
        })
    }
}

fn run_set_level(s: &Gamedata, mut req: SetLevelReq) -> Reply {
    let data = data::gems(s);
    let applied = with_rng(|rng| req.gem.set_level(&data.up, req.level, rng));
    action(&data.up, &req.gem, applied, |_| false)
}

struct EvaluateReq {
    tier: i64,
    gem_type: i64,
    level: i64,
    stats: Vec<StatInput>,
    auto_guess: bool,
}

impl FromBody for EvaluateReq {
    fn from_body(b: Field<'_>) -> Option<Self> {
        body(b, |m, _| {
            let tier = m.field("tier").int().default(4);
            let gem_type = m.field("type").int().default(1);
            let level = m.field("level").int().default(1);
            let stats = m.field("stats").list(wire::eval_stat).required();
            let auto_guess = m.field("auto_guess_procs").bool().default(false);
            Some(EvaluateReq { tier, gem_type, level, stats: stats.into_iter().collect::<Option<_>>()?, auto_guess })
        })
    }
}

fn evaluator_error(e: GemError) -> Reply {
    match e {
        GemError::Evaluator(m) => Reply::bad_request(m),
        e => Reply::caught(e, "Could not evaluate gem: ", |e| {
            matches!(e, GemError::Value(_) | GemError::ZeroDivision(_))
        }),
    }
}

fn run_evaluate(s: &Gamedata, req: EvaluateReq) -> Reply {
    let data = data::gems(s);
    match evaluator::evaluate(&data.up, req.tier, req.gem_type, req.level, &req.stats, req.auto_guess) {
        Ok(e) => Reply::Json(wire::evaluation_json(&e)),
        Err(e) => evaluator_error(e),
    }
}

struct SimpleReq {
    tier: i64,
    gem_type: i64,
    power_rank: i64,
    level: i64,
}

impl FromBody for SimpleReq {
    fn from_body(b: Field<'_>) -> Option<Self> {
        body(b, |m, _| {
            let tier = m.field("tier").int().default(4);
            let gem_type = m.field("type").int().default(1);
            let power_rank = m.field("power_rank").int().default(0);
            let level = m.field("level").int().default(1);
            Some(SimpleReq { tier, gem_type, power_rank, level })
        })
    }
}

fn run_evaluate_simple(s: &Gamedata, req: SimpleReq) -> Reply {
    let data = data::gems(s);
    match evaluator::evaluate_simple(&data.up, req.tier, req.gem_type, req.power_rank, req.level) {
        Ok(e) => Reply::Json(wire::simple_json(&e)),
        Err(e) => evaluator_error(e),
    }
}

struct BuildReq {
    config: BuildConfig,
    star_chart: Option<String>,
}

impl FromBody for BuildReq {
    fn from_body(b: Field<'_>) -> Option<Self> {
        body(b, |m, _| {
            let build_type = m.field("build_type").str().default("Light".into());
            let character = m.field("character").str().default("Bard".into());
            let subclass = m.field("subclass").str().default("Boomeranger".into());
            let food = m.field("food").str().default(String::new());
            let ally = m.field("ally").str().default("boot_clown".into());
            let ally_buff = m.field("ally_buff").bool().default(true);
            let berserker_battler = m.field("berserker_battler").bool().default(false);
            let critical_damage_count = m.field("critical_damage_count").int().ge(0).le(3).default(3);
            let no_face = m.field("no_face").bool().default(false);
            let light = m.field("light").int().default(0);
            let subclass_active = m.field("subclass_active").bool().default(false);
            let litany = m.field("litany").bool().default(false);
            let bounty_hunt = m.field("bounty_hunt").bool().default(false);
            let star_chart = m.field("star_chart").str().optional();
            let high_precision = m.field("high_precision").bool().default(false);
            Some(BuildReq {
                config: BuildConfig {
                    build_type,
                    character,
                    subclass,
                    food,
                    ally,
                    ally_buff,
                    berserker_battler,
                    critical_damage_count,
                    no_face,
                    light,
                    subclass_active,
                    litany,
                    bounty_hunt,
                    high_precision,
                },
                star_chart,
            })
        })
    }
}

fn run_builds(s: &Gamedata, req: BuildReq) -> Reply {
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
    match rows.iter().map(wire::build_row_json).collect::<Option<Vec<_>>>() {
        Some(results) => {
            let count = results.len();
            Reply::Json(serde_json::json!({"results": results, "count": count}))
        }
        None => Reply::Unhandled("BuildResult.light: a fractional value is not a valid integer".into()),
    }
}

struct HealthReq {
    character: String,
    star_chart: Option<String>,
    flags: [bool; 4],
    high_precision: bool,
}

const HEALTH_FLAGS: [&str; 4] = ["weapon_ch", "ring_ch", "hat_health", "face_health"];

impl FromBody for HealthReq {
    fn from_body(b: Field<'_>) -> Option<Self> {
        body(b, |m, _| {
            let character = m.field("character").str().default("Bard".into());
            let star_chart = m.field("star_chart").str().optional();
            let flags = HEALTH_FLAGS.map(|f| m.field(f).bool().default(false));
            let high_precision = m.field("high_precision").bool().default(false);
            Some(HealthReq { character, star_chart, flags, high_precision })
        })
    }
}

fn run_health(s: &Gamedata, req: HealthReq) -> Reply {
    let d = data::builds(s);
    let gem_stats = match &d.gem_stats {
        Ok(g) => g,
        Err(e) => return Reply::Unhandled(e.message().to_owned()),
    };
    let Some(class) = d.classes.iter().rev().find(|c| c.name == req.character) else {
        return Reply::bad_request(format!("Unknown class '{}'.", req.character));
    };
    let flag = |name: &str| match name {
        "high_precision" => req.high_precision,
        _ => HEALTH_FLAGS.iter().position(|f| *f == name).is_some_and(|i| req.flags[i]),
    };
    let chart = req.star_chart.as_deref().filter(|c| !c.is_empty()).map(|c| d.chart.parse(c));
    let plan = builds::optimize_health(class, &d.health, &flag, chart.as_ref(), gem_stats, req.high_precision);
    Reply::Json(wire::health_json(&plan))
}

struct LevelCostsQ {
    tier: i64,
    gem_type: i64,
    element: i64,
    booster: Option<String>,
    from_level: i64,
}

impl FromParams for LevelCostsQ {
    fn from_params(p: &mut Params<'_>) -> Self {
        LevelCostsQ {
            tier: p.query("tier").int().required(),
            gem_type: p.query("type").int().required(),
            element: p.query("element").int().default(1),
            booster: p.query("booster").str().optional(),
            from_level: p.query("from_level").int().ge(1).default(1),
        }
    }
}

fn run_level_costs(s: &Gamedata, q: &LevelCostsQ) -> Reply {
    let data = data::gems(s);
    let plan = kiwi_core::gems::catalog::tier(q.tier)
        .and_then(|t| Ok((t, kiwi_core::gems::catalog::gem_type(q.gem_type)?)))
        .and_then(|(t, g)| Ok((t, g, kiwi_core::gems::catalog::element(q.element)?)))
        .and_then(|(t, g, e)| data.up.level_plan(t, g, e, q.booster.as_deref(), q.from_level));
    match plan {
        Ok(plan) => {
            Reply::Json(wire::level_plan_json(q.tier, q.gem_type, q.element, q.booster.as_deref(), q.from_level, &plan))
        }
        Err(e) => Reply::caught(e, "Invalid level-cost parameters: ", is_value),
    }
}

struct StatRangeQ {
    tier: i64,
    gem_type: i64,
    stat_type: i64,
    level: i64,
    extra_containers: i64,
    element: Option<i64>,
}

impl FromParams for StatRangeQ {
    fn from_params(p: &mut Params<'_>) -> Self {
        StatRangeQ {
            tier: p.query("tier").int().required(),
            gem_type: p.query("type").int().required(),
            stat_type: p.query("stat_type").int().required(),
            level: p.query("level").int().ge(1).default(1),
            extra_containers: p.query("extra_containers").int().ge(0).default(0),
            element: p.query("element").int().optional(),
        }
    }
}

fn run_stat_range(s: &Gamedata, q: &StatRangeQ) -> Reply {
    let data = data::gems(s);
    match evaluator::stat_range(&data.up, q.tier, q.gem_type, q.stat_type, q.level, q.extra_containers, q.element) {
        Ok(r) => Reply::Json(wire::stat_range_json(&r)),
        Err(e) => Reply::caught(e, "Invalid stat-range parameters: ", is_value),
    }
}

fn run_lookups(s: &Gamedata) -> Reply {
    let data = data::gems(s);
    let abilities = data::abilities(s);
    Reply::json(wire::lookups_json(&data.up, &abilities))
}

fn run_build_options(s: &Gamedata) -> Reply {
    let d = data::builds(s);
    if let Err(e) = &d.gem_stats {
        return Reply::Unhandled(e.message().to_owned());
    }
    let foods = wire::option_entries(d.foods.iter().map(|f| (f.key.clone(), f.label.clone())));
    let allies = std::iter::once(("boot_clown".to_owned(), "No ally".to_owned()))
        .chain(d.allies.iter().filter(|a| a.key != "boot_clown").map(|a| (a.key.clone(), a.label.clone())));
    Reply::Json(wire::build_options_json(&d.class_names, wire::option_entries(allies), foods))
}

async fn lookups(State(state): State<AppState>, RequireScope(ctx): RequireScope<GEMS_READ>) -> Response {
    run_lookups(store(&state)).respond(ctx.rate_limit)
}

async fn build_options(State(state): State<AppState>, RequireScope(ctx): RequireScope<GEMS_READ>) -> Response {
    run_build_options(store(&state)).respond(ctx.rate_limit)
}

async fn level_costs(
    State(state): State<AppState>,
    RequireScope(ctx): RequireScope<GEMS_READ>,
    Query(q): Query<LevelCostsQ>,
) -> Response {
    run_level_costs(store(&state), &q).respond(ctx.rate_limit)
}

async fn stat_range(
    State(state): State<AppState>,
    RequireScope(ctx): RequireScope<GEMS_READ>,
    Query(q): Query<StatRangeQ>,
) -> Response {
    run_stat_range(store(&state), &q).respond(ctx.rate_limit)
}

//! `/v1/stats/*` (`app/trove/router.py` + `app/trove/stats.py`): the stat
//! tables, the classes, the Coefficient calculator and the player sigil.

mod classes;
#[cfg(test)]
mod tests;

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, LazyLock, Mutex},
};

use axum::{
    Router,
    body::Body,
    extract::{Request, State},
    http::{HeaderValue, StatusCode, Uri, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use base64::Engine;
use bytes::Bytes;
use kiwi_core::{imaging, stats as core};
use serde_json::{Value, json};

use crate::{
    auth::{Public, RequireScope, extract::public_scope, scopes::STATS_READ},
    error::{ApiError, ErrorCode},
    gamedata::{Gamedata, read_json},
    middleware::pyurl::scope_path,
    pyjson,
    state::AppState,
    validation::{BodyInput, Field, FromBody, FromParams, Input, Params, Query, Validator, read_body},
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/stats/power-rank", get(power_rank))
        .route("/v1/stats/magic-find", get(magic_find))
        .route("/v1/stats/light", get(light))
        .route("/v1/stats/classes", get(list_classes))
        .route("/v1/stats/classes/{tech_name}", get(get_class))
        .route("/v1/stats/coefficient", post(coefficient))
        .route("/v1/stats/sigil", get(sigil))
}

fn gamedata(state: &AppState) -> &'static Gamedata {
    Gamedata::global(state.settings())
}

/// A FastAPI response-model body that was rendered ahead of time.
fn json_body(body: impl Into<Body>) -> Response {
    ([(header::CONTENT_TYPE, HeaderValue::from_static("application/json"))], body.into()).into_response()
}

type Scoped = RequireScope<{ STATS_READ }>;

// --- stat tables -----------------------------------------------------------

/// Python `for x in v` where each `x` must then be a dict: only a list, or an
/// empty str/dict (whose items would be str and fail), gets through.
fn py_items(v: &Value) -> Option<&[Value]> {
    match v {
        Value::Array(a) => Some(a),
        Value::Object(m) if m.is_empty() => Some(&[]),
        Value::String(s) if s.is_empty() => Some(&[]),
        _ => None,
    }
}

fn get_or(m: &serde_json::Map<String, Value>, key: &str, default: Value) -> Value {
    m.get(key).cloned().unwrap_or(default)
}

/// `StatTable(**stat_table(stat))` rendered, or `None` where Python raises.
pub fn render_stat_table(stat: &str, label: &str, raw: &Value) -> Option<String> {
    let mut cleaned = Vec::new();
    for s in py_items(raw)? {
        let m = s.as_object()?;
        cleaned.push(json!({
            "name": m.get("name")?,
            "value": m.get("value")?,
            "type": get_or(m, "type", "switch".into()),
            "percentage": get_or(m, "percentage", false.into()),
            "step": get_or(m, "step", Value::Null),
            "permanent": get_or(m, "perm", Value::Null),
        }));
    }
    let mut v = Validator::default();
    let sources: Vec<Value> = cleaned
        .iter()
        .map(|row| {
            Field::new(&mut v, vec![], Input::Json(row))
                .model(|m| {
                    json!({
                        "name": m.field("name").str().required(),
                        "value": m.field("value").float().required(),
                        "type": m.field("type").str().required(),
                        "percentage": m.field("percentage").bool().required(),
                        "step": m.field("step").float().nullable(),
                        "permanent": m.field("permanent").bool().nullable(),
                    })
                })
                .required()
        })
        .collect();
    if !v.is_ok() {
        return None;
    }
    let count = sources.len();
    Some(pyjson::pydantic(&json!({"stat": stat, "label": label, "sources": sources, "count": count})))
}

/// `stat_table(stat)`: baseline only, the parsed file kept for the process
/// lifetime once read (`functools.cache`, which never caches a failure).
pub fn stat_table(gd: &Gamedata, stat: &str) -> Result<String, ApiError> {
    static RAW: LazyLock<Mutex<HashMap<PathBuf, Arc<Value>>>> = LazyLock::new(Mutex::default);
    let (file, label) = match stat {
        "power-rank" => ("power_rank.json", "Power Rank"),
        "magic-find" => ("magic_find.json", "Magic Find"),
        _ => ("light.json", "Light"),
    };
    let path = gd.baseline_dir().join("stats").join(file);
    let cached = RAW.lock().unwrap_or_else(|e| e.into_inner()).get(&path).cloned();
    let raw = match cached {
        Some(raw) => raw,
        None => {
            let raw = Arc::new(read_json(&path).ok_or_else(ApiError::internal)?);
            RAW.lock().unwrap_or_else(|e| e.into_inner()).insert(path, raw.clone());
            raw
        }
    };
    render_stat_table(stat, label, &raw).ok_or_else(ApiError::internal)
}

fn table(state: &AppState, scope: Scoped, stat: &str) -> Result<Response, ApiError> {
    let body = stat_table(gamedata(state), stat)?;
    Ok((scope.0.rate_limit, json_body(body)).into_response())
}

async fn power_rank(State(state): State<AppState>, scope: Scoped) -> Result<Response, ApiError> {
    table(&state, scope, "power-rank")
}

async fn magic_find(State(state): State<AppState>, scope: Scoped) -> Result<Response, ApiError> {
    table(&state, scope, "magic-find")
}

async fn light(State(state): State<AppState>, scope: Scoped) -> Result<Response, ApiError> {
    table(&state, scope, "light")
}

// --- classes ---------------------------------------------------------------

async fn list_classes(State(state): State<AppState>, scope: Scoped) -> Result<Response, ApiError> {
    let body = classes::list_body(gamedata(&state))?;
    Ok((scope.0.rate_limit, json_body(body)).into_response())
}

const CLASS_PREFIX: &str = "/v1/stats/classes/";

async fn get_class(State(state): State<AppState>, scope: Scoped, uri: Uri) -> Result<Response, ApiError> {
    // Starlette's path param is cut from the percent-decoded path (lossy UTF-8).
    let path = scope_path(&uri);
    let tech_name = path.strip_prefix(CLASS_PREFIX).unwrap_or_default();
    let body = classes::class_body(gamedata(&state), tech_name)?;
    Ok((scope.0.rate_limit, json_body(body)).into_response())
}

// --- coefficient -----------------------------------------------------------

#[derive(Debug, PartialEq)]
pub struct CoefficientReq {
    pub physical_damage: Option<f64>,
    pub magic_damage: Option<f64>,
    pub critical_damage: f64,
}

impl FromBody for CoefficientReq {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| {
            Some(CoefficientReq {
                physical_damage: m.field("physical_damage").float().ge(0).optional(),
                magic_damage: m.field("magic_damage").float().ge(0).optional(),
                critical_damage: m.field("critical_damage").float().ge(0).required(),
            })
        })
        .required()
    }
}

/// The `CoefficientRequest` body model, or its 422.
pub fn parse_coefficient(body: &BodyInput) -> Result<CoefficientReq, ApiError> {
    let mut v = Validator::default();
    let parsed = CoefficientReq::from_body(Field::new(&mut v, vec!["body".into()], body.input()));
    v.finish(parsed)?.ok_or_else(ApiError::internal)
}

/// `CoefficientResponse` for a validated request.
pub fn coefficient_body(req: &CoefficientReq) -> Result<String, ApiError> {
    let Some((value, which)) = core::coefficient(req.physical_damage, req.magic_damage, req.critical_damage) else {
        return Err(ApiError::bad_request(
            "Provide critical_damage and at least one of physical_damage / magic_damage.",
        ));
    };
    // `int()` of a large float is a big int, which `Value` can't hold.
    let digits = core::py_int_digits(value).ok_or_else(ApiError::internal)?;
    let rest = pyjson::pydantic(&json!({"damage_used": which, "formula": core::COEFFICIENT_FORMULA}));
    Ok(format!("{{\"coefficient\":{digits},{}", &rest[1..]))
}

/// The body is read (a JSON syntax error answers first), then the scope
/// dependency runs, then the body is validated: FastAPI's order.
async fn coefficient(State(state): State<AppState>, req: Request) -> Response {
    let (parts, body) = req.into_parts();
    let body = match read_body(&parts.headers, body).await {
        Ok(b) => b,
        Err(e) => return e.into_response(),
    };
    let ctx = match public_scope(&state, &parts, STATS_READ, 1, None).await {
        Ok(ctx) => ctx,
        Err(r) => return r.into_response(),
    };
    match parse_coefficient(&body).and_then(|req| coefficient_body(&req)) {
        Ok(body) => (ctx.rate_limit, json_body(body)).into_response(),
        Err(e) => e.into_response(),
    }
}

// --- sigil -----------------------------------------------------------------

pub struct SigilQuery {
    pub power_rank: i64,
    pub mastery: i64,
}

impl FromParams for SigilQuery {
    fn from_params(p: &mut Params<'_>) -> Self {
        SigilQuery {
            power_rank: p.query("power_rank").int().ge(0).le(10_000_000).required(),
            mastery: p.query("mastery").int().ge(0).le(100_000).required(),
        }
    }
}

/// Python truthiness of a parsed JSON value.
fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(m) => !m.is_empty(),
    }
}

/// `_tier(data[key], value)`; `None` where Python raises.
fn tier(data: &Value, key: &str, value: i64) -> Option<usize> {
    let thresholds = data
        .as_object()?
        .get(key)?
        .as_array()?
        .iter()
        .map(|t| match t.as_object()?.get("from")? {
            Value::Bool(b) => Some(core::Threshold::Int(*b as i64)),
            Value::Number(n) => Some(match n.as_i64() {
                Some(i) => core::Threshold::Int(i),
                None => core::Threshold::Float(n.as_f64()?),
            }),
            _ => None,
        })
        .collect::<Option<Vec<_>>>()?;
    core::tier(&thresholds, value)
}

/// `base64.b64decode(str)`: ASCII only, non-alphabet characters dropped,
/// leftover bits ignored.
fn b64decode(s: &str) -> Option<Vec<u8>> {
    use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};
    const ENGINE: GeneralPurpose = GeneralPurpose::new(
        &base64::alphabet::STANDARD,
        GeneralPurposeConfig::new()
            .with_decode_allow_trailing_bits(true)
            .with_decode_padding_mode(DecodePaddingMode::RequireCanonical),
    );
    if !s.is_ascii() {
        return None;
    }
    let clean: String = s.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '=')).collect();
    ENGINE.decode(clean).ok()
}

/// `_sigil(shield, wings)`: the mastery wings under the Power Rank shield.
fn render_sigil(data: Option<&Value>, shield: usize, wings: usize) -> Option<Vec<u8>> {
    let layer = |key: &str, i: usize| {
        let b64 = data?.as_object()?.get(key)?.as_array()?.get(i)?.as_object()?.get("image")?.as_str()?;
        imaging::decode_png(&b64decode(b64)?).ok()
    };
    let out = imaging::alpha_composite(&layer("mastery", wings)?, &layer("power_rank", shield)?).ok()?;
    imaging::encode_png(&out).ok()
}

/// `sigil_png(power_rank, mastery)` as the route answers it.
pub fn sigil_png(gd: &Gamedata, power_rank: i64, mastery: i64) -> Result<Bytes, ApiError> {
    let data = gd.load("insignia.json");
    if !data.as_deref().is_some_and(truthy) {
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            ErrorCode::ServiceUnavailable,
            "Sigil art is not available yet",
        ));
    }
    let data = data.as_deref().unwrap_or(&Value::Null);
    let shield = tier(data, "power_rank", power_rank).ok_or_else(ApiError::internal)?;
    let wings = tier(data, "mastery", mastery).ok_or_else(ApiError::internal)?;
    let key = format!("stats.sigil:{shield}:{wings}");
    let png = gd.cached(&key, &["insignia.json"], || {
        render_sigil(gd.load("insignia.json").as_deref(), shield, wings).map(Bytes::from)
    });
    png.as_ref().clone().ok_or_else(ApiError::internal)
}

async fn sigil(
    State(state): State<AppState>,
    _ctx: Public<{ STATS_READ }>,
    Query(q): Query<SigilQuery>,
) -> Result<Response, ApiError> {
    let png = sigil_png(gamedata(&state), q.power_rank, q.mastery)?;
    let mut res = Response::new(Body::from(png));
    let h = res.headers_mut();
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=86400"));
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("image/png"));
    Ok(res)
}

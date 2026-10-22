//! `/v1/misc` leaf routes from `app/trove/router.py`: modding software, the
//! timezone list, clocks and converter, the market interest list and feedback.

pub mod feedback;
pub mod time;

#[cfg(test)]
mod tests;

use std::{
    collections::BTreeSet,
    path::Path,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use axum::{
    Router,
    extract::{Request, State},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use futures::TryStreamExt;
use mongodb::bson::{Bson, Document};
use serde_json::{Map, Value, json};

use crate::{
    auth::{AccessContext, Public, extract::public_scope, scopes::MISC_READ},
    error::ApiError,
    pyjson::PydanticJson,
    state::AppState,
    validation::{Field, Validator, read_body},
};

type Misc = Public<MISC_READ>;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/misc/software", get(software))
        .route("/v1/misc/timezones", get(timezones))
        .route("/v1/misc/time/now", get(time_now))
        .route("/v1/misc/time/convert", post(convert))
        .route("/v1/misc/interest-items", get(interest_items))
        .route("/v1/misc/feedback", post(feedback::post_feedback))
}

const GAMEDATA: &str = "app/trove/gamedata";

/// `modding_software()` validated through `ModdingSoftware`; `None` where the
/// file is missing or does not fit the model (a 500 in Python).
pub fn modding_software(path: &Path) -> Option<Value> {
    let raw: Value = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    let mut categories = Vec::new();
    for (key, cat) in raw.as_object()? {
        let cat = cat.as_object()?;
        let description = match cat.get("description") {
            None => "",
            Some(v) => v.as_str()?,
        };
        let mut software = Vec::new();
        for tool in cat.get("software").map_or(Some(&Vec::new()), Value::as_array)? {
            let tool = tool.as_object()?;
            let s = |k: &str| tool.get(k).and_then(Value::as_str).map(str::to_owned);
            software.push(json!({
                "name": s("name")?,
                "free": tool.get("free").and_then(Value::as_bool)?,
                "url": s("url")?,
                "description": s("description")?,
            }));
        }
        categories.push(json!({"key": key, "description": description, "software": software}));
    }
    let count = categories.len();
    Some(json!({"categories": categories, "count": count}))
}

async fn software(Public(ctx): Misc) -> Response {
    static CACHE: OnceLock<Value> = OnceLock::new();
    let body = match CACHE.get() {
        Some(v) => v.clone(),
        None => match modding_software(&Path::new(GAMEDATA).join("modding_software.json")) {
            Some(v) => CACHE.get_or_init(|| v).clone(),
            None => return ApiError::internal().into_response(),
        },
    };
    (ctx.rate_limit, PydanticJson(body)).into_response()
}

async fn timezones(Public(ctx): Misc) -> Response {
    (ctx.rate_limit, PydanticJson(time::timezones())).into_response()
}

async fn time_now(Public(ctx): Misc) -> Response {
    let unix = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    match time::time_now(time::source(), unix) {
        Ok(body) => (ctx.rate_limit, PydanticJson(body)).into_response(),
        Err(_) => ApiError::internal().into_response(),
    }
}

/// `TimeConvertRequest`.
#[derive(Default)]
pub struct ConvertBody {
    pub datetime: Option<String>,
    pub timezone: String,
    pub unix: Option<i64>,
}

/// The JSON body error first, then the `public_scope` dependency, then body
/// validation: FastAPI's order.
async fn convert(State(state): State<AppState>, req: Request) -> Response {
    let (parts, body) = req.into_parts();
    let body = match read_body(&parts.headers, body).await {
        Ok(b) => b,
        Err(e) => return e.into_response(),
    };
    let ctx: AccessContext = match public_scope(&state, &parts, MISC_READ, 1, None).await {
        Ok(c) => c,
        Err(rej) => return rej.into_response(),
    };
    let mut v = Validator::default();
    let parsed = Field::new(&mut v, vec!["body".into()], body.input())
        .model(|m| ConvertBody {
            datetime: m.field("datetime").str().optional(),
            timezone: m.field("timezone").str().default("UTC".into()),
            unix: m.field("unix").int().optional(),
        })
        .required();
    let req = match v.finish(parsed) {
        Ok(r) => r,
        Err(e) => return e.into_response(),
    };
    match time::convert_time(time::source(), req.datetime.as_deref(), &req.timezone, req.unix) {
        Ok(out) => (ctx.rate_limit, PydanticJson(out)).into_response(),
        Err(time::TimeError::Bad(msg)) => ApiError::bad_request(msg).into_response(),
        Err(time::TimeError::Internal) => ApiError::internal().into_response(),
    }
}

/// `market_service._interest_items_set`: the allow-list (30s cache), or the
/// bundled seed list while the collection is empty.
async fn interest_set(state: &AppState) -> Result<BTreeSet<String>, ApiError> {
    static CACHE: Mutex<Option<(BTreeSet<String>, Instant)>> = Mutex::new(None);
    if let Some((items, at)) = CACHE.lock().unwrap_or_else(|e| e.into_inner()).as_ref()
        && at.elapsed() < Duration::from_secs(30)
    {
        return Ok(items.clone());
    }
    let internal = |err: &dyn std::fmt::Display| {
        tracing::error!("interest items: {err}");
        ApiError::internal()
    };
    let docs: Vec<Document> = state
        .mongo()
        .collection::<Document>("market_interest_items")
        .find(Document::new())
        .await
        .map_err(|e| internal(&e))?
        .try_collect()
        .await
        .map_err(|e| internal(&e))?;
    let items: BTreeSet<String> = if docs.is_empty() {
        let raw = std::fs::read_to_string(Path::new(GAMEDATA).join("market_items.json")).map_err(|e| internal(&e))?;
        let list: Vec<String> = serde_json::from_str(&raw).map_err(|e| internal(&e))?;
        list.into_iter().collect()
    } else {
        let mut out = BTreeSet::new();
        for d in &docs {
            match d.get("name") {
                Some(Bson::String(s)) => {
                    out.insert(s.clone());
                }
                _ => return Err(internal(&"document without a string name")),
            }
        }
        out
    };
    *CACHE.lock().unwrap_or_else(|e| e.into_inner()) = Some((items.clone(), Instant::now()));
    Ok(items)
}

async fn interest_items(State(state): State<AppState>, Public(ctx): Misc) -> Response {
    match interest_set(&state).await {
        Ok(items) => {
            let count = items.len();
            let items: Vec<Value> = items.into_iter().map(Value::String).collect();
            let mut body = Map::new();
            body.insert("items".into(), Value::Array(items));
            body.insert("count".into(), Value::from(count));
            (ctx.rate_limit, PydanticJson(Value::Object(body))).into_response()
        }
        Err(e) => e.into_response(),
    }
}

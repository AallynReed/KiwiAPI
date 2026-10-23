//! The `/site` live and reference reads of `app/site/router.py` that are not
//! another area's, the crawler files, the site flag map and site search.
//! Every route sits behind the router's `_resolve_feature_flags` ([`flags::SiteGate`]).

pub mod clubs;
pub mod crawler;
pub mod flags;
pub mod reads;
pub mod rotations;
pub mod search;
pub mod search_index;

#[cfg(test)]
mod store_tests;
#[cfg(test)]
pub(crate) mod tests;

use axum::{
    Router,
    http::{HeaderValue, header},
    response::{IntoResponse, Response},
    routing::get,
};
use mongodb::bson::Bson;
use serde_json::{Map, Value};

use crate::{dt, pyjson::PyJson, state::AppState};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/robots.txt", get(crawler::robots))
        .route("/BingSiteAuth.xml", get(crawler::bing))
        .route("/sitemap.xml", get(crawler::sitemap))
        .route("/site/feature-flags", get(flags::feature_flags))
        .route("/site/search", get(search::route))
        .route("/site/server-time", get(reads::server_time))
        .route("/site/rotations", get(rotations::route))
        .route("/site/feeds/news", get(reads::news))
        .route("/site/feeds/videos", get(reads::videos))
        .route("/site/calendar/events", get(reads::calendar_events))
        .route("/site/calendar/yearly", get(rotations::yearly))
        .route("/site/stats/classes", get(reads::classes))
        .route("/site/stats/sigil", get(reads::sigil))
        .route("/site/giveaways", get(reads::giveaways))
        .route("/site/clubs", get(clubs::route))
        .route("/site/screenshots.json", get(reads::screenshots))
        .merge(crate::gems::site::router())
        .merge(crate::routes::btt::site::router())
}

/// A Starlette `JSONResponse` with its `Cache-Control`.
pub fn cached(body: Value, cache_control: &'static str) -> Response {
    let mut res = PyJson(body).into_response();
    res.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static(cache_control));
    res
}

/// `jsonable_encoder` of a value Mongo handed back (tz-aware datetimes in
/// `isoformat()`, ObjectIds as hex).
pub fn jsonable(b: &Bson) -> Value {
    match b {
        Bson::Null | Bson::Undefined => Value::Null,
        Bson::Boolean(v) => Value::Bool(*v),
        Bson::Int32(i) => Value::from(*i),
        Bson::Int64(i) => Value::from(*i),
        Bson::Double(f) => serde_json::Number::from_f64(*f).map_or(Value::Null, Value::Number),
        Bson::String(s) | Bson::Symbol(s) | Bson::JavaScriptCode(s) => Value::String(s.clone()),
        Bson::DateTime(d) => Value::String(dt::isoformat(&dt::from_bson(*d))),
        Bson::ObjectId(o) => Value::String(o.to_hex()),
        Bson::Array(items) => Value::Array(items.iter().map(jsonable).collect()),
        Bson::Document(d) => {
            Value::Object(d.iter().map(|(k, v)| (k.clone(), jsonable(v))).collect::<Map<String, Value>>())
        }
        Bson::Decimal128(d) => Value::String(d.to_string()),
        other => Value::String(other.to_string()),
    }
}

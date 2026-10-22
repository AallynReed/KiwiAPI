//! The feeds routes of `app/trove/router.py` (`feeds_router` and
//! `/v1/misc/news-history`). Every one but the image proxy is a response
//! model, so only those carry the scope's `X-RateLimit-*` headers.

use std::collections::HashMap;

use axum::{
    Router,
    extract::{FromRequestParts, State},
    http::{HeaderValue, StatusCode, header, request::Parts},
    response::{IntoResponse, Response},
    routing::get,
};
use mongodb::bson::{Document, doc};
use serde_json::{Value, json};

use super::{
    image::{self, ImageError},
    pyval,
    store::{self, News},
};
use crate::{
    auth::{AccessContext, Public, Rejection, RequireScope, extract::public_scope, scopes},
    error::{ApiError, ErrorCode},
    pyjson::PydanticJson,
    state::AppState,
    validation::{FromParams, Params, Query},
};

type Feeds = Public<{ scopes::FEEDS_READ }>;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/feeds/news", get(news))
        .route("/v1/feeds/twitch", get(twitch))
        .route("/v1/feeds/youtube", get(youtube))
        .route("/v1/feeds/bilibili", get(bilibili))
        .route("/v1/feeds/bilibili/image", get(bilibili_image))
        .route("/v1/feeds/events", get(events_ongoing))
        .route("/v1/feeds/events/categories", get(event_categories))
        .route("/v1/feeds/events/upcoming", get(events_upcoming))
        .route("/v1/feeds/events/history", get(events_history))
        .route("/v1/misc/news-history", get(news_history))
}

struct NewsQuery {
    limit: i64,
}

impl FromParams for NewsQuery {
    fn from_params(p: &mut Params<'_>) -> Self {
        NewsQuery { limit: p.query("limit").int().ge(1).le(50).default(20) }
    }
}

struct HistoryQuery {
    limit: i64,
    offset: i64,
}

impl FromParams for HistoryQuery {
    fn from_params(p: &mut Params<'_>) -> Self {
        HistoryQuery {
            limit: p.query("limit").int().ge(1).le(200).default(50),
            offset: p.query("offset").int().ge(0).default(0),
        }
    }
}

struct EventsQuery {
    category: Option<String>,
    limit: i64,
}

impl FromParams for EventsQuery {
    fn from_params(p: &mut Params<'_>) -> Self {
        EventsQuery {
            category: p.query("category").str().optional(),
            limit: p.query("limit").int().ge(1).le(500).default(100),
        }
    }
}

struct ImageQuery {
    url: String,
}

impl FromParams for ImageQuery {
    fn from_params(p: &mut Params<'_>) -> Self {
        ImageQuery { url: p.query("url").str().required() }
    }
}

/// `latest_news(limit)`: over-fetch, keep the newest copy per title and merge
/// the later copies' categories into it.
pub fn dedupe_news(docs: Vec<News>, limit: usize) -> Vec<News> {
    let mut out: Vec<News> = Vec::new();
    let mut seen: HashMap<String, usize> = HashMap::new();
    for d in docs {
        let title = kiwi_core::feeds::py::strip(&d.title).to_lowercase();
        let key = if title.is_empty() { d.url.clone() } else { title };
        if let Some(&i) = seen.get(&key) {
            for c in d.categories {
                if !out[i].categories.contains(&c) {
                    out[i].categories.push(c);
                }
            }
            continue;
        }
        seen.insert(key, out.len());
        out.push(d);
    }
    out.truncate(limit);
    out
}

pub fn news_list(items: &[News]) -> Value {
    json!({ "items": items.iter().map(News::item).collect::<Vec<_>>(), "count": items.len() })
}

async fn news(State(state): State<AppState>, Public(ctx): Feeds, Query(q): Query<NewsQuery>) -> Response {
    let docs = store::news_page(state.mongo(), None, (q.limit * 3).max(q.limit)).await;
    let items = dedupe_news(docs, q.limit as usize);
    (ctx.rate_limit, PydanticJson(news_list(&items))).into_response()
}

async fn news_history(
    State(state): State<AppState>,
    RequireScope(ctx): RequireScope<{ scopes::MISC_READ }>,
    Query(q): Query<HistoryQuery>,
) -> Response {
    let total = store::news_count(state.mongo()).await;
    let docs = store::news_page(state.mongo(), Some(q.offset as u64), q.limit).await;
    let body = json!({
        "items": docs.iter().map(News::item).collect::<Vec<_>>(),
        "count": docs.len(),
        "total": total,
    });
    (ctx.rate_limit, PydanticJson(body)).into_response()
}

/// `TwitchStreams` / `Videos` from a `feed_cache` document.
pub fn feed_body(feed: Option<(Vec<Document>, chrono::DateTime<chrono::Utc>)>, item: fn(&Document) -> Value) -> Value {
    let (items, fetched_at) = match feed {
        Some((items, at)) => (items.iter().map(item).collect::<Vec<_>>(), Value::String(crate::dt::pydantic(&at))),
        None => (Vec::new(), Value::Null),
    };
    json!({ "items": items, "count": items.len(), "fetched_at": fetched_at })
}

async fn twitch(State(state): State<AppState>, Public(ctx): Feeds) -> Response {
    let body = feed_body(store::feed(state.mongo(), "twitch").await, store::twitch_stream);
    (ctx.rate_limit, PydanticJson(body)).into_response()
}

async fn youtube(State(state): State<AppState>, Public(ctx): Feeds) -> Response {
    let body = feed_body(store::feed(state.mongo(), "youtube").await, store::video);
    (ctx.rate_limit, PydanticJson(body)).into_response()
}

async fn bilibili(State(state): State<AppState>, Public(ctx): Feeds) -> Response {
    let body = feed_body(store::feed(state.mongo(), "bilibili").await, store::video);
    (ctx.rate_limit, PydanticJson(body)).into_response()
}

/// `public_scope("feeds:read", rate_multiplier=settings.bilibili_image_rate_limit_multiplier)`.
struct FeedImageScope;

impl FromRequestParts<AppState> for FeedImageScope {
    type Rejection = Rejection;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Rejection> {
        let multiplier = state.settings().bilibili_image_rate_limit_multiplier;
        let _: AccessContext = public_scope(state, parts, scopes::FEEDS_READ, multiplier, None).await?;
        Ok(FeedImageScope)
    }
}

async fn bilibili_image(_: FeedImageScope, Query(q): Query<ImageQuery>) -> Response {
    match image::fetch(&q.url).await {
        Ok((content, content_type)) => {
            if content_type.chars().any(|c| c as u32 > 0xff) {
                store::unhandled("content-type is not latin-1");
            }
            let latin1: Vec<u8> = content_type.chars().map(|c| c as u32 as u8).collect();
            let ctype = HeaderValue::from_bytes(&latin1).unwrap_or_else(|e| store::unhandled(e));
            let mut resp = content.into_response();
            resp.headers_mut().insert(header::CONTENT_TYPE, ctype);
            resp.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=86400"));
            resp
        }
        Err(ImageError::BadRequest(msg)) => ApiError::bad_request(msg).into_response(),
        Err(ImageError::Upstream(msg)) => ApiError::new(
            StatusCode::BAD_GATEWAY,
            ErrorCode::InternalError,
            format!("upstream thumbnail fetch failed: {msg}"),
        )
        .into_response(),
        Err(ImageError::InvalidUrl(msg)) => store::unhandled(format!("httpx.InvalidURL: {msg}")),
    }
}

fn events_filter(mut filter: Document, category: Option<String>) -> Document {
    if let Some(c) = category.filter(|c| !c.is_empty()) {
        filter.insert("category", c);
    }
    filter
}

fn event_list(events: &[store::Event], now: i64) -> Value {
    json!({ "items": events.iter().map(|e| e.item(now)).collect::<Vec<_>>(), "count": events.len() })
}

fn now_bson(now: i64) -> mongodb::bson::Bson {
    pyval::int_bson(now.into()).unwrap_or(mongodb::bson::Bson::Int64(now))
}

async fn events_ongoing(State(state): State<AppState>, Public(ctx): Feeds, Query(q): Query<EventsQuery>) -> Response {
    let now = chrono::Utc::now().timestamp();
    let filter =
        events_filter(doc! { "starts_at": { "$lte": now_bson(now) }, "ends_at": { "$gt": now_bson(now) } }, q.category);
    let events = store::events(state.mongo(), filter, doc! { "ends_at": 1 }, q.limit).await;
    (ctx.rate_limit, PydanticJson(event_list(&events, now))).into_response()
}

async fn events_upcoming(State(state): State<AppState>, Public(ctx): Feeds, Query(q): Query<EventsQuery>) -> Response {
    let now = chrono::Utc::now().timestamp();
    let filter = events_filter(doc! { "starts_at": { "$gt": now_bson(now) } }, q.category);
    let events = store::events(state.mongo(), filter, doc! { "starts_at": 1 }, q.limit).await;
    (ctx.rate_limit, PydanticJson(event_list(&events, now))).into_response()
}

async fn events_history(State(state): State<AppState>, Public(ctx): Feeds, Query(q): Query<EventsQuery>) -> Response {
    let now = chrono::Utc::now().timestamp();
    let filter = events_filter(doc! { "ends_at": { "$lte": now_bson(now) } }, q.category);
    let events = store::events(state.mongo(), filter, doc! { "ends_at": -1 }, q.limit).await;
    (ctx.rate_limit, PydanticJson(event_list(&events, now))).into_response()
}

async fn event_categories(State(state): State<AppState>, Public(ctx): Feeds) -> Response {
    let categories = store::event_categories(state.mongo()).await;
    let count = categories.len();
    (ctx.rate_limit, PydanticJson(json!({ "categories": categories, "count": count }))).into_response()
}

//! The read routes: `/v1/market/*` (pydantic models, `market:read`) and the
//! tokenless `/site/market/*` proxies and analytics (raw dicts), plus
//! `POST /v1/market/reset`.

use axum::{
    extract::State,
    http::{HeaderMap, HeaderName, HeaderValue, header},
    response::{IntoResponse, Response},
};
use kiwi_core::market::{
    self as core, LIQUIDITY_SOLD_MARGIN_SECONDS as SOLD_MARGIN, LISTING_LIFETIME_SECONDS as LIFE,
    LISTING_STALE_SECONDS as STALE, signals,
};
use mongodb::bson::{Document, doc};
use serde_json::{Map, Value, json};

use super::{
    Segment, SiteFeature,
    store::{self, Direction, ListingFilter, Live, Store, internal},
};
use crate::{
    auth::{MasterIngest, RequireScope, pyutil::bson_int, scopes::MARKET_READ},
    dt,
    error::{ApiError, ApiResult},
    features::{Market, RequireFeature},
    pyjson::{PyJson, PydanticJson},
    ratelimit::{RateLimitInfo, check_rate_limit, get_rate_limit},
    state::AppState,
    validation::{FromParams, Params, Query},
};

pub const ICON_BRANCH: &str = "live-us";

fn cache(max_age: u32) -> [(HeaderName, HeaderValue); 1] {
    let v = HeaderValue::from_str(&format!("public, max-age={max_age}")).expect("ascii header");
    [(header::CACHE_CONTROL, v)]
}

/// A `/site` JSONResponse with its cache lifetime.
pub(super) fn site(body: ApiResult<Value>, max_age: u32) -> Response {
    match body {
        Ok(v) => (cache(max_age), PyJson(v)).into_response(),
        Err(e) => e.into_response(),
    }
}

// --- listings ---------------------------------------------------------------------

/// The listings query (`le` is 1000 on `/v1`, 500 on `/site`).
#[derive(Clone, Debug, PartialEq)]
pub struct ListingsQuery<const MAX: i64> {
    pub name: Option<String>,
    pub price_min: Option<f64>,
    pub price_max: Option<f64>,
    pub last_seen_after: Option<i64>,
    pub hide_expired: bool,
    pub sort: String,
    pub limit: i64,
    pub offset: i64,
}

impl<const MAX: i64> FromParams for ListingsQuery<MAX> {
    fn from_params(p: &mut Params<'_>) -> Self {
        ListingsQuery {
            name: p.query("name").str().optional(),
            price_min: p.query("price_min").float().ge(0).optional(),
            price_max: p.query("price_max").float().ge(0).optional(),
            last_seen_after: p.query("last_seen_after").int().ge(0).optional(),
            hide_expired: p.query("hide_expired").bool().default(true),
            sort: p.query("sort").str().default("-last_seen".into()),
            limit: p.query("limit").int().ge(1).le(MAX).default(100),
            offset: p.query("offset").int().ge(0).default(0),
        }
    }
}

impl<const MAX: i64> ListingsQuery<MAX> {
    /// `price_min > price_max` is a 400.
    pub fn check_range(&self) -> ApiResult<()> {
        match (self.price_min, self.price_max) {
            (Some(lo), Some(hi)) if lo > hi => Err(ApiError::bad_request("price_min cannot be greater than price_max")),
            _ => Ok(()),
        }
    }
}

/// `service.list_listings` as a `{items, count, total}` page.
pub async fn listings_core<S: Store, const MAX: i64>(s: &S, q: &ListingsQuery<MAX>) -> ApiResult<Value> {
    let mut f = ListingFilter {
        name: q.name.clone(),
        price_min: q.price_min,
        price_max: q.price_max,
        last_seen_floor: q.last_seen_after,
        created_at_floor: None,
        sort: q.sort.clone(),
        limit: q.limit,
        offset: q.offset,
    };
    if q.hide_expired {
        let stale = s.now() - STALE;
        f.last_seen_floor = Some(f.last_seen_floor.map_or(stale, |v| v.max(stale)));
        f.created_at_floor = Some(s.now() - LIFE);
    }
    let (rows, total) = store::list_listings(s, &f).await?;
    let items = rows.iter().map(|r| store::listing_dict(r, s.now())).collect::<ApiResult<Vec<_>>>()?;
    let count = items.len();
    Ok(json!({"items": items, "count": count, "total": total}))
}

/// The rate-limit headers renamed `X-RateLimit-Archive-*`.
pub fn archive_headers(info: RateLimitInfo) -> HeaderMap {
    let mut h = HeaderMap::new();
    for (name, value) in info.headers() {
        let renamed = name.as_str().replace("x-ratelimit-", "x-ratelimit-archive-");
        if let Ok(n) = HeaderName::from_bytes(renamed.as_bytes()) {
            h.insert(n, value);
        }
    }
    h
}

/// `_enforce_market_archive_limit` for an archive read (`hide_expired=false`).
async fn archive_limit(state: &AppState, token: &mongodb::bson::oid::ObjectId) -> Result<HeaderMap, Response> {
    let (max, window) = get_rate_limit(state, "market_archive_rate_limit").await;
    let info = check_rate_limit(state, &format!("mkt_archive:{token}"), max, window)
        .await
        .map_err(IntoResponse::into_response)?;
    Ok(archive_headers(info))
}

pub async fn v1_listings(
    State(state): State<AppState>,
    _: RequireFeature<Market>,
    RequireScope(ctx): RequireScope<MARKET_READ>,
    Query(q): Query<ListingsQuery<1000>>,
) -> Response {
    if let Err(e) = q.check_range() {
        return e.into_response();
    }
    let archive = if q.hide_expired {
        HeaderMap::new()
    } else {
        match archive_limit(&state, &ctx.token.id).await {
            Ok(h) => h,
            Err(r) => return r,
        }
    };
    match listings_core(&Live(&state), &q).await {
        Ok(v) => (ctx.rate_limit, archive, PydanticJson(v)).into_response(),
        Err(e) => e.into_response(),
    }
}

pub async fn site_listings(
    State(state): State<AppState>,
    _: SiteFeature<Market>,
    Query(q): Query<ListingsQuery<500>>,
) -> Response {
    let body = match q.check_range() {
        Ok(()) => listings_core(&Live(&state), &q).await,
        Err(e) => Err(e),
    };
    site(body, 30)
}

// --- items + summary -----------------------------------------------------------------

pub async fn v1_items(
    State(state): State<AppState>,
    _: RequireFeature<Market>,
    RequireScope(ctx): RequireScope<MARKET_READ>,
) -> Response {
    match store::distinct_items(&Live(&state)).await {
        Ok(items) => (ctx.rate_limit, PydanticJson(json!({"items": items, "count": items.len()}))).into_response(),
        Err(e) => e.into_response(),
    }
}

/// `service.item_summary`, or the 404 both surfaces raise.
pub async fn summary_core<S: Store>(s: &S, name: &str) -> ApiResult<Value> {
    store::item_summary(s, name, s.now() - STALE, s.now() - LIFE)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("No active market listings for '{name}'")))
}

pub async fn v1_summary(
    State(state): State<AppState>,
    _: RequireFeature<Market>,
    RequireScope(ctx): RequireScope<MARKET_READ>,
    Segment(name): Segment<0>,
) -> Response {
    match summary_core(&Live(&state), &name).await {
        Ok(v) => (ctx.rate_limit, PydanticJson(v)).into_response(),
        Err(e) => e.into_response(),
    }
}

pub async fn site_summary(
    State(state): State<AppState>,
    _: SiteFeature<Market>,
    Segment(name): Segment<1>,
) -> Response {
    site(summary_core(&Live(&state), &name).await, 30)
}

/// `/site/market/items`: traded names, the sidebar categories and the
/// names no longer on the scan list.
pub async fn items_core<S: Store>(s: &S) -> ApiResult<Value> {
    let items = store::distinct_items(s).await?;
    let docs = s.categories().await?;
    let keys: Vec<(i64, &str)> = docs.iter().map(|d| (d.order, d.name.as_str())).collect();
    let categories: Vec<Value> = core::category_order(&keys)
        .into_iter()
        .map(|i| json!({"name": docs[i].name, "items": core::sorted_lower(&docs[i].items)}))
        .collect();
    let interest = s.interest().await?;
    let untracked: Vec<&String> = items.iter().filter(|n| !interest.contains(*n)).collect();
    Ok(json!({"items": items, "count": items.len(), "categories": categories, "untracked": untracked}))
}

pub async fn site_items(State(state): State<AppState>, _: SiteFeature<Market>) -> Response {
    site(items_core(&Live(&state)).await, 60)
}

/// `/site/market/item-images`: any failure is an empty map.
pub async fn item_images_core<S: Store>(s: &S) -> Value {
    let mut images = Map::new();
    let resolved = async {
        let names = store::distinct_items(s).await?;
        let resolved = if names.is_empty() {
            Default::default()
        } else {
            store::blueprints_for_names(s, ICON_BRANCH, &names).await?
        };
        ApiResult::Ok((names, resolved))
    };
    match resolved.await {
        Ok((names, resolved)) => {
            for name in names {
                if let Some(bp) = resolved.get(&name.to_lowercase()).filter(|b| !b.is_empty()) {
                    images.insert(name, Value::String(bp.clone()));
                }
            }
        }
        Err(e) => tracing::error!("market: item-image resolve failed: {e}"),
    }
    let count = images.len();
    json!({"images": images, "branch": ICON_BRANCH, "count": count})
}

pub async fn site_item_images(State(state): State<AppState>, _: SiteFeature<Market>) -> Response {
    site(Ok(item_images_core(&Live(&state)).await), 300)
}

// --- price history ------------------------------------------------------------------

pub struct HistoryQuery {
    pub days: i64,
    pub include_expired: bool,
    pub keep_outliers: bool,
}

impl FromParams for HistoryQuery {
    fn from_params(p: &mut Params<'_>) -> Self {
        HistoryQuery {
            days: p.query("days").int().ge(1).le(30).default(7),
            include_expired: p.query("include_expired").bool().default(false),
            keep_outliers: p.query("keep_outliers").bool().default(false),
        }
    }
}

/// `service.price_history`.
pub async fn history_core<S: Store>(s: &S, name: &str, q: &HistoryQuery) -> ApiResult<Value> {
    let days = q.days.clamp(1, 30);
    let limit: i64 = 5000;
    let now = s.now();
    let window_start = now - days * 86_400;
    let mut created_at_floor = window_start;
    let mut last_seen_floor = None;
    if !q.include_expired {
        last_seen_floor = Some(now - STALE);
        created_at_floor = created_at_floor.max(now - LIFE);
    }
    let rows = store::price_history_rows(s, name, created_at_floor, last_seen_floor, limit).await?;
    let mut points: Vec<Value> = rows.iter().map(store::history_point).collect();
    let truncated = points.len() as i64 >= limit;
    let raw_count = points.len();
    let (mut excluded, mut lo, mut hi) = (0usize, Value::Null, Value::Null);
    if !q.keep_outliers {
        let prices: Vec<f64> = points.iter().map(|p| p["price_each"].as_f64().unwrap_or(0.0)).collect();
        if let Some(dropped) = core::price_outliers(&prices) {
            let (mut kept, mut out) = (Vec::new(), Vec::new());
            for (p, d) in points.into_iter().zip(dropped) {
                if d { out.push(p) } else { kept.push(p) }
            }
            points = kept;
            excluded = out.len();
            let prices = out.iter().filter_map(|p| p["price_each"].as_f64());
            lo = prices.clone().reduce(f64::min).map_or(Value::Null, Value::from);
            hi = prices.reduce(f64::max).map_or(Value::Null, Value::from);
        }
    }
    let count = points.len();
    Ok(json!({
        "name": name,
        "days": days,
        "include_expired": q.include_expired,
        "keep_outliers": q.keep_outliers,
        "window_start": window_start,
        "window_end": now,
        "points": points,
        "count": count,
        "raw_count": raw_count,
        "outliers_excluded": excluded,
        "outliers_min_price": lo,
        "outliers_max_price": hi,
        "outliers_threshold_z": core::PRICE_OUTLIER_Z,
        "truncated": truncated,
    }))
}

pub async fn site_history(
    State(state): State<AppState>,
    _: SiteFeature<Market>,
    Segment(name): Segment<2>,
    Query(q): Query<HistoryQuery>,
) -> Response {
    site(history_core(&Live(&state), &name, &q).await, 30)
}

// --- analytics ---------------------------------------------------------------------

pub struct TimelineQuery {
    pub name: String,
    pub days: i64,
    pub bucket_hours: i64,
}

impl FromParams for TimelineQuery {
    fn from_params(p: &mut Params<'_>) -> Self {
        TimelineQuery {
            name: p.query("name").str().min_len(1).required(),
            days: p.query("days").int().ge(1).le(90).default(14),
            bucket_hours: p.query("bucket_hours").int().ge(1).le(168).default(24),
        }
    }
}

fn bands_json(start: i64, end: i64) -> Vec<Value> {
    core::event_bands(start, end)
        .into_iter()
        .map(|b| json!({"name": b.name, "kind": b.kind, "starts_at": b.starts_at, "ends_at": b.ends_at}))
        .collect()
}

/// `service.analytics_timeline`.
pub async fn timeline_core<S: Store>(s: &S, q: &TimelineQuery) -> ApiResult<Value> {
    let now = s.now();
    let floor = now - q.days * 86400;
    let points = if s.pg_enabled() {
        store::price_volume_timeline(s, &q.name, floor, q.bucket_hours * 3600).await?
    } else {
        vec![]
    };
    Ok(json!({
        "name": q.name,
        "days": q.days,
        "bucket_hours": q.bucket_hours,
        "points": points,
        "events": bands_json(floor, now),
        "now": now,
    }))
}

pub async fn site_timeline(
    State(state): State<AppState>,
    _: SiteFeature<Market>,
    Query(q): Query<TimelineQuery>,
) -> Response {
    site(timeline_core(&Live(&state), &q).await, 120)
}

pub struct DealsQuery {
    pub days: i64,
    pub min_discount: f64,
    pub limit: i64,
}

impl FromParams for DealsQuery {
    fn from_params(p: &mut Params<'_>) -> Self {
        DealsQuery {
            days: p.query("days").int().ge(1).le(30).default(7),
            min_discount: p.query("min_discount").float().ge(0.05).le(0.95).default(0.25),
            limit: p.query("limit").int().ge(1).le(200).default(50),
        }
    }
}

/// The `/site/market/analytics/deals` payload.
pub async fn deals_core<S: Store>(s: &S, q: &DealsQuery) -> ApiResult<Value> {
    let deals = if s.pg_enabled() {
        let now = s.now();
        let created_floor = (now - q.days * 86400).max(now - LIFE);
        store::underpriced_deals(s, now - STALE, created_floor, q.min_discount, 5, q.limit).await?
    } else {
        vec![]
    };
    Ok(json!({"items": deals, "count": deals.len(), "min_discount": q.min_discount, "days": q.days}))
}

pub async fn site_deals(
    State(state): State<AppState>,
    _: SiteFeature<Market>,
    Query(q): Query<DealsQuery>,
) -> Response {
    site(deals_core(&Live(&state), &q).await, 60)
}

pub struct WindowQuery<const DAYS: i64, const LIMIT: i64> {
    pub days: i64,
    pub limit: i64,
}

impl<const DAYS: i64, const LIMIT: i64> FromParams for WindowQuery<DAYS, LIMIT> {
    fn from_params(p: &mut Params<'_>) -> Self {
        WindowQuery {
            days: p.query("days").int().ge(1).le(30).default(DAYS),
            limit: p.query("limit").int().ge(1).le(100).default(LIMIT),
        }
    }
}

/// `service.analytics_movers`.
pub async fn movers_core<S: Store>(s: &S, days: i64, limit: i64) -> ApiResult<Value> {
    if !s.pg_enabled() {
        return Ok(json!({"risers": [], "fallers": [], "days": days}));
    }
    let now = s.now();
    let per_side = (limit / 2).max(1);
    let (recent, prior) = (now - days * 86400, now - 2 * days * 86400);
    let risers = store::market_movers(s, recent, prior, now, 5, per_side, Direction::Up).await?;
    let fallers = store::market_movers(s, recent, prior, now, 5, per_side, Direction::Down).await?;
    Ok(json!({"risers": risers, "fallers": fallers, "days": days, "now": now}))
}

pub async fn site_movers(
    State(state): State<AppState>,
    _: SiteFeature<Market>,
    Query(q): Query<WindowQuery<7, 40>>,
) -> Response {
    site(movers_core(&Live(&state), q.days, q.limit).await, 120)
}

pub struct DaysQuery<const MIN: i64, const MAX: i64, const DEFAULT: i64> {
    pub days: i64,
}

impl<const MIN: i64, const MAX: i64, const DEFAULT: i64> FromParams for DaysQuery<MIN, MAX, DEFAULT> {
    fn from_params(p: &mut Params<'_>) -> Self {
        DaysQuery { days: p.query("days").int().ge(MIN).le(MAX).default(DEFAULT) }
    }
}

/// `service.analytics_overview`.
pub async fn overview_core<S: Store>(s: &S, days: i64) -> ApiResult<Value> {
    let now = s.now();
    let mut base = json!({
        "active_listings": 0, "active_items": 0, "total_value": 0, "total_units": 0,
        "top_mover": null, "top_traded": null, "days": days, "now": now,
    });
    if !s.pg_enabled() {
        return Ok(base);
    }
    let snap = store::market_overview(s, now - STALE, now - LIFE).await?;
    let movers = store::market_movers(s, now - days * 86400, now - 2 * days * 86400, now, 5, 1, Direction::Abs).await?;
    let vol = store::volume_leaders(s, now - days * 86400, 1).await?;
    let m = base.as_object_mut().expect("object");
    m.extend(snap);
    m.insert("top_mover".into(), movers.into_iter().next().unwrap_or(Value::Null));
    m.insert("top_traded".into(), vol.into_iter().next().unwrap_or(Value::Null));
    Ok(base)
}

pub async fn site_overview(
    State(state): State<AppState>,
    _: SiteFeature<Market>,
    Query(q): Query<DaysQuery<1, 30, 7>>,
) -> Response {
    site(overview_core(&Live(&state), q.days).await, 120)
}

/// `signals.scan`.
pub async fn signals_core<S: Store>(s: &S, days: i64) -> ApiResult<Value> {
    if !s.pg_enabled() {
        return Ok(json!({"days": days, "generated_for": null, "market": null, "signals": [], "scanned_items": 0}));
    }
    let floor = s.now().div_euclid(signals::DAY) * signals::DAY - days * signals::DAY;
    let rows = store::daily_series_all(s, floor, signals::DAY).await?;
    let scan = signals::scan(&rows).map_err(|_| internal("signals: missing price baseline"))?;
    let m = &scan.market;
    let market = json!({
        "items": m.items, "median_move": m.median_move, "share_up": m.share_up, "share_down": m.share_down,
        "verdict": m.verdict, "reading": m.reading,
    });
    let sigs: Vec<Value> = scan
        .signals
        .iter()
        .map(|g| {
            json!({
                "name": g.name, "pattern": g.pattern, "reading": g.reading, "severity": g.severity, "day": g.day,
                "price": g.price, "baseline": g.baseline, "change": g.change, "excess": g.excess,
                "listings": g.listings, "stack_med": g.stack_med, "stack_max": g.stack_max,
                "price_z": g.price_z, "supply_z": g.supply_z, "stack_z": g.stack_z,
            })
        })
        .collect();
    Ok(json!({
        "days": days, "generated_for": scan.generated_for, "market": market, "signals": sigs,
        "scanned_items": scan.scanned_items,
    }))
}

pub async fn site_signals(
    State(state): State<AppState>,
    _: SiteFeature<Market>,
    Query(q): Query<DaysQuery<10, 60, 21>>,
) -> Response {
    site(signals_core(&Live(&state), q.days).await, 600)
}

/// `service.analytics_liquidity`.
pub async fn liquidity_core<S: Store>(s: &S, days: i64, limit: i64) -> ApiResult<Value> {
    let now = s.now();
    if !s.pg_enabled() {
        return Ok(json!({"items": [], "days": days, "now": now}));
    }
    let items = store::market_liquidity(s, now - STALE, now - days * 86400, LIFE - SOLD_MARGIN, 5, limit).await?;
    Ok(json!({"items": items, "days": days, "now": now, "ttl_seconds": LIFE}))
}

pub async fn site_liquidity(
    State(state): State<AppState>,
    _: SiteFeature<Market>,
    Query(q): Query<WindowQuery<14, 40>>,
) -> Response {
    site(liquidity_core(&Live(&state), q.days, q.limit).await, 120)
}

/// `service.analytics_volume`.
pub async fn volume_core<S: Store>(s: &S, days: i64, limit: i64) -> ApiResult<Value> {
    let now = s.now();
    if !s.pg_enabled() {
        return Ok(json!({"items": [], "days": days, "now": now}));
    }
    let items = store::volume_leaders(s, now - days * 86400, limit).await?;
    Ok(json!({"items": items, "days": days, "now": now}))
}

pub async fn site_volume(
    State(state): State<AppState>,
    _: SiteFeature<Market>,
    Query(q): Query<WindowQuery<14, 40>>,
) -> Response {
    site(volume_core(&Live(&state), q.days, q.limit).await, 120)
}

// --- reset -------------------------------------------------------------------------

/// `ingest_log.record` (never fails the request).
pub async fn record_ingest(state: &AppState, endpoint: &str, ingest: &crate::auth::IngestAuth, summary: Document) {
    let row = doc! {
        "_id": mongodb::bson::oid::ObjectId::new(),
        "endpoint": endpoint,
        "timestamp": dt::to_bson(dt::now()),
        "user_id": ingest.user.id,
        "user_email": &ingest.user.email,
        "success": true,
        "summary": summary,
        "error": null,
        "auth_via": if ingest.token.is_some() { "token" } else { "session" },
        "token_name": ingest.token.as_ref().map(|t| t.name.clone()),
    };
    if let Err(e) = state.mongo().collection::<Document>("ingest_log").insert_one(row).await {
        tracing::error!("ingest_log.record: failed to write entry for {endpoint}: {e}");
    }
}

pub async fn v1_reset(
    State(state): State<AppState>,
    _: RequireFeature<Market>,
    MasterIngest(auth): MasterIngest,
) -> Response {
    let removed = match store::reset(&Live(&state)).await {
        Ok(n) => n,
        Err(e) => return e.into_response(),
    };
    tracing::warn!("market: listings wiped by admin ({removed} rows removed)");
    record_ingest(&state, "/v1/market/reset", &auth, doc! {"removed": bson_int(removed)}).await;
    let body = PydanticJson(json!({"reset": true, "removed": removed}));
    match auth.rate_limit {
        Some(info) => (info, body).into_response(),
        None => body.into_response(),
    }
}

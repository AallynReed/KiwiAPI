//! `app/trove/market/pg_store.py` and the codex name lookup, over a [`Store`]:
//! the same SQL text, arguments and row handling, returning the dicts Python
//! builds. These are what the leaderboards warmer can call.

use std::collections::{BTreeSet, HashMap};

use chrono::{DateTime, Utc};
use futures::TryStreamExt;
use kiwi_core::{gems::py_round, market::signals::DayRow};
use mongodb::bson::{Bson, Document, doc, oid::ObjectId};
use serde_json::{Map, Value, json};

use super::pg::{Arg, PgValue, Row};
use crate::{
    dt,
    error::{ApiError, ApiResult},
    state::AppState,
};

pub const CATEGORIES: &str = "market_item_categories";
pub const INTEREST: &str = "market_interest_items";

pub(crate) fn internal(err: impl std::fmt::Display) -> ApiError {
    tracing::error!("market: {err}");
    ApiError::internal()
}

/// What the market code reads: Postgres, Mongo and the clock.
#[allow(async_fn_in_trait)]
pub trait Store: Sync {
    /// `service._now()`: whole Unix seconds.
    fn now(&self) -> i64;
    /// The clock in microseconds (`server_time()` callers).
    fn now_us(&self) -> i64;
    /// `settings.postgres_enabled`.
    fn pg_enabled(&self) -> bool;
    /// One asyncpg call (`fetch` / `fetchrow` / `fetchval` / `execute`).
    async fn run(&self, method: &'static str, sql: &str, args: Vec<Arg>) -> ApiResult<Vec<Row>>;
    /// `_interest_items_set()`.
    async fn interest(&self) -> ApiResult<BTreeSet<String>>;
    /// Every `market_item_categories` document in store order.
    async fn categories(&self) -> ApiResult<Vec<Category>>;

    async fn fetch(&self, sql: &str, args: Vec<Arg>) -> ApiResult<Vec<Row>> {
        self.run("fetch", sql, args).await
    }

    async fn fetchrow(&self, sql: &str, args: Vec<Arg>) -> ApiResult<Option<Row>> {
        Ok(self.run("fetchrow", sql, args).await?.into_iter().next())
    }

    async fn fetchval(&self, sql: &str, args: Vec<Arg>) -> ApiResult<PgValue> {
        Ok(self.run("fetchval", sql, args).await?.first().map_or(PgValue::Null, |r| r.first().clone()))
    }
}

/// The production stores.
pub struct Live<'a>(pub &'a AppState);

impl Store for Live<'_> {
    fn now(&self) -> i64 {
        Utc::now().timestamp()
    }

    fn now_us(&self) -> i64 {
        Utc::now().timestamp_micros()
    }

    fn pg_enabled(&self) -> bool {
        self.0.inner.db.pg.is_some()
    }

    async fn run(&self, method: &'static str, sql: &str, args: Vec<Arg>) -> ApiResult<Vec<Row>> {
        let pool = self.0.inner.db.pg.as_ref().ok_or_else(|| internal("Postgres pool not initialized"))?;
        super::pg::run(pool, method, sql, &args).await.map_err(|e| internal(format!("{e:#}")))
    }

    async fn interest(&self) -> ApiResult<BTreeSet<String>> {
        crate::routes::misc::interest_set(self.0).await
    }

    async fn categories(&self) -> ApiResult<Vec<Category>> {
        let docs: Vec<Document> = self
            .0
            .mongo()
            .collection::<Document>(CATEGORIES)
            .find(doc! {})
            .await
            .map_err(internal)?
            .try_collect()
            .await
            .map_err(internal)?;
        docs.iter().map(Category::from_doc).collect()
    }
}

fn bad_row(what: &str) -> ApiError {
    internal(format!("unexpected value in {what}"))
}

fn int(row: &Row, col: &str) -> ApiResult<i64> {
    row.get(col).int().ok_or_else(|| bad_row(col))
}

fn int0(row: &Row, col: &str) -> ApiResult<i64> {
    row.get(col).int_or_zero().ok_or_else(|| bad_row(col))
}

fn float(row: &Row, col: &str) -> ApiResult<f64> {
    row.get(col).float().ok_or_else(|| bad_row(col))
}

fn text(row: &Row, col: &str) -> ApiResult<String> {
    row.get(col).text().map(str::to_owned).ok_or_else(|| bad_row(col))
}

/// A column as the JSON value of what asyncpg returned.
fn raw(row: &Row, col: &str) -> Value {
    match row.get(col) {
        PgValue::Null => Value::Null,
        PgValue::Bool(b) => Value::Bool(*b),
        PgValue::Int(i) => Value::from(*i),
        PgValue::Float(f) => Value::from(*f),
        PgValue::Text(s) | PgValue::Numeric(s) => Value::String(s.clone()),
        PgValue::Uuid(u) => Value::String(kiwi_core::market::parser::uuid_str(*u)),
    }
}

fn uuid(row: &Row, col: &str) -> ApiResult<String> {
    match row.get(col) {
        PgValue::Uuid(u) => Ok(kiwi_core::market::parser::uuid_str(*u)),
        _ => Err(bad_row(col)),
    }
}

/// Filters of `list_listings`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ListingFilter {
    pub name: Option<String>,
    pub price_min: Option<f64>,
    pub price_max: Option<f64>,
    pub last_seen_floor: Option<i64>,
    pub created_at_floor: Option<i64>,
    pub sort: String,
    pub limit: i64,
    pub offset: i64,
}

/// `pg_store.list_listings`: raw rows and the total.
pub async fn list_listings<S: Store>(s: &S, f: &ListingFilter) -> ApiResult<(Vec<Row>, i64)> {
    let mut conds: Vec<String> = Vec::new();
    let mut args: Vec<Arg> = Vec::new();
    let mut add = |expr: &str, v: Arg| {
        args.push(v);
        conds.push(format!("{expr}${}", args.len()));
    };
    if let Some(n) = &f.name {
        add("name = ", Arg::Text(n.clone()));
    }
    if let Some(v) = f.price_min {
        add("price_each >= ", Arg::Float(v));
    }
    if let Some(v) = f.price_max {
        add("price_each <= ", Arg::Float(v));
    }
    if let Some(v) = f.last_seen_floor {
        add("last_seen >= ", Arg::Int(v));
    }
    if let Some(v) = f.created_at_floor {
        add("created_at >= ", Arg::Int(v));
    }
    let wh = if conds.is_empty() { String::new() } else { format!(" WHERE {}", conds.join(" AND ")) };
    let total = s.fetchval(&format!("SELECT count(*) FROM market_listing{wh}"), args.clone()).await?;
    let n = args.len();
    let sql = format!(
        "SELECT id, name, type, stack, price, price_each, last_seen, created_at FROM market_listing{wh} ORDER BY {} \
         LIMIT ${} OFFSET ${}",
        kiwi_core::market::order_by(&f.sort),
        n + 1,
        n + 2
    );
    let mut all = args;
    all.extend([Arg::Int(f.limit), Arg::Int(f.offset)]);
    let rows = s.fetch(&sql, all).await?;
    Ok((rows, total.int_or_zero().ok_or_else(|| bad_row("count"))?))
}

/// `service._to_dict`.
pub fn listing_dict(row: &Row, now: i64) -> ApiResult<Value> {
    use kiwi_core::market::{LISTING_LIFETIME_SECONDS as LIFE, LISTING_STALE_SECONDS as STALE};
    let created_at = int(row, "created_at")?;
    let last_seen = int(row, "last_seen")?;
    Ok(json!({
        "id": uuid(row, "id")?,
        "name": raw(row, "name"),
        "type": raw(row, "type"),
        "stack": raw(row, "stack"),
        "price": raw(row, "price"),
        "price_each": raw(row, "price_each"),
        "last_seen": last_seen,
        "created_at": created_at,
        "expires_at": created_at + LIFE,
        "expired": now - created_at > LIFE || now - last_seen > STALE,
    }))
}

/// `pg_store.distinct_items`.
pub async fn distinct_items<S: Store>(s: &S) -> ApiResult<Vec<String>> {
    let rows = s.fetch("SELECT DISTINCT name FROM market_listing ORDER BY name", Vec::new()).await?;
    rows.iter().map(|r| text(r, "name")).collect()
}

/// `pg_store.item_summary`.
pub async fn item_summary<S: Store>(s: &S, name: &str, ls_floor: i64, ca_floor: i64) -> ApiResult<Option<Value>> {
    let sql = concat!(
        "SELECT count(*) AS count, sum(price) AS total_price, sum(stack) AS total_stack, ",
        "min(price_each) AS min_each, max(price_each) AS max_each, avg(price_each) AS avg_each, ",
        "percentile_cont(0.5) WITHIN GROUP (ORDER BY price_each) AS median_each ",
        "FROM market_listing WHERE name = $1 AND last_seen >= $2 AND created_at >= $3",
    );
    let args = vec![Arg::Text(name.to_owned()), Arg::Int(ls_floor), Arg::Int(ca_floor)];
    let Some(r) = s.fetchrow(sql, args).await? else { return Ok(None) };
    if r.get("count").int().unwrap_or(0) == 0 {
        return Ok(None);
    }
    let avg = match r.get("avg_each") {
        PgValue::Null => 0.0,
        v => py_round(v.float().ok_or_else(|| bad_row("avg_each"))?, 3),
    };
    let median = match r.get("median_each") {
        PgValue::Null => 0.0,
        v => v.float().ok_or_else(|| bad_row("median_each"))?,
    };
    Ok(Some(json!({
        "name": name,
        "count": int(&r, "count")?,
        "total_price": int0(&r, "total_price")?,
        "total_stack": int0(&r, "total_stack")?,
        "min_each": float(&r, "min_each")?,
        "max_each": float(&r, "max_each")?,
        "avg_each": avg,
        "median_each": median,
    })))
}

/// `pg_store.medians_for_names`: name -> (median_each, count).
pub async fn medians_for_names<S: Store>(
    s: &S,
    names: &[String],
    ls_floor: i64,
    ca_floor: i64,
) -> ApiResult<kiwi_core::market::tomes::Medians> {
    if names.is_empty() {
        return Ok(HashMap::new());
    }
    let sql = concat!(
        "SELECT name, count(*) AS count, ",
        "percentile_cont(0.5) WITHIN GROUP (ORDER BY price_each) AS median_each ",
        "FROM market_listing ",
        "WHERE name = ANY($1) AND last_seen >= $2 AND created_at >= $3 ",
        "GROUP BY name",
    );
    let rows = s.fetch(sql, vec![Arg::TextList(names.to_vec()), Arg::Int(ls_floor), Arg::Int(ca_floor)]).await?;
    let mut out = HashMap::new();
    for r in rows.iter().filter(|r| !r.get("median_each").is_null()) {
        out.insert(text(r, "name")?, (float(r, "median_each")?, int(r, "count")?));
    }
    Ok(out)
}

/// `service.medians_for_names`: empty without Postgres.
pub async fn active_medians<S: Store>(s: &S, names: &[String]) -> ApiResult<kiwi_core::market::tomes::Medians> {
    use kiwi_core::market::{LISTING_LIFETIME_SECONDS as LIFE, LISTING_STALE_SECONDS as STALE};
    if !s.pg_enabled() || names.is_empty() {
        return Ok(HashMap::new());
    }
    medians_for_names(s, names, s.now() - STALE, s.now() - LIFE).await
}

/// `pg_store.price_history_rows`.
pub async fn price_history_rows<S: Store>(
    s: &S,
    name: &str,
    created_at_floor: i64,
    last_seen_floor: Option<i64>,
    limit: i64,
) -> ApiResult<Vec<Row>> {
    let mut conds = vec!["name = $1".to_owned(), "created_at >= $2".to_owned()];
    let mut args = vec![Arg::Text(name.to_owned()), Arg::Int(created_at_floor)];
    if let Some(v) = last_seen_floor {
        args.push(Arg::Int(v));
        conds.push(format!("last_seen >= ${}", args.len()));
    }
    args.push(Arg::Int(limit));
    let sql = format!(
        "SELECT created_at, price_each, last_seen, stack, price FROM market_listing WHERE {} ORDER BY created_at ASC \
         LIMIT ${}",
        conds.join(" AND "),
        args.len()
    );
    s.fetch(&sql, args).await
}

/// The point dict `price_history` builds from a row.
pub fn history_point(r: &Row) -> Value {
    json!({
        "created_at": raw(r, "created_at"),
        "price_each": raw(r, "price_each"),
        "last_seen": raw(r, "last_seen"),
        "stack": raw(r, "stack"),
        "price": raw(r, "price"),
    })
}

/// `pg_store.price_volume_timeline`.
pub async fn price_volume_timeline<S: Store>(
    s: &S,
    name: &str,
    created_at_floor: i64,
    bucket_seconds: i64,
) -> ApiResult<Vec<Value>> {
    let sql = concat!(
        "SELECT (created_at / $3) * $3 AS bucket, count(*) AS listings, ",
        "sum(stack) AS stack, ",
        "percentile_cont(0.5) WITHIN GROUP (ORDER BY price_each) AS p50, ",
        "percentile_cont(0.25) WITHIN GROUP (ORDER BY price_each) AS p25, ",
        "percentile_cont(0.75) WITHIN GROUP (ORDER BY price_each) AS p75 ",
        "FROM market_listing WHERE name = $1 AND created_at >= $2 ",
        "GROUP BY bucket ORDER BY bucket",
    );
    let args = vec![Arg::Text(name.to_owned()), Arg::Int(created_at_floor), Arg::Int(bucket_seconds)];
    let rows = s.fetch(sql, args).await?;
    rows.iter()
        .map(|r| {
            Ok(json!({
                "bucket": int(r, "bucket")?,
                "listings": int(r, "listings")?,
                "stack": int0(r, "stack")?,
                "p50": float(r, "p50")?,
                "p25": float(r, "p25")?,
                "p75": float(r, "p75")?,
            }))
        })
        .collect()
}

/// `pg_store.daily_series_all`.
pub async fn daily_series_all<S: Store>(s: &S, created_at_floor: i64, bucket_seconds: i64) -> ApiResult<Vec<DayRow>> {
    let sql = concat!(
        "SELECT name, (created_at / $2) * $2 AS bucket, ",
        "count(*) AS listings, sum(stack) AS stack, ",
        "percentile_cont(0.5) WITHIN GROUP (ORDER BY price_each) AS p50, ",
        "percentile_cont(0.5) WITHIN GROUP (ORDER BY stack) AS stack_med, ",
        "max(stack) AS stack_max ",
        "FROM market_listing WHERE created_at >= $1 ",
        "GROUP BY name, bucket ORDER BY name, bucket",
    );
    let rows = s.fetch(sql, vec![Arg::Int(created_at_floor), Arg::Int(bucket_seconds)]).await?;
    rows.iter()
        .map(|r| {
            Ok(DayRow {
                name: text(r, "name")?,
                bucket: int(r, "bucket")?,
                listings: int(r, "listings")?,
                stack: int0(r, "stack")?,
                p50: float(r, "p50")?,
                stack_med: r.get("stack_med").float_or_zero().ok_or_else(|| bad_row("stack_med"))?,
                stack_max: int0(r, "stack_max")?,
            })
        })
        .collect()
}

/// `pg_store.underpriced_deals`.
pub async fn underpriced_deals<S: Store>(
    s: &S,
    last_seen_floor: i64,
    created_at_floor: i64,
    min_discount: f64,
    min_samples: i64,
    limit: i64,
) -> ApiResult<Vec<Value>> {
    let sql = concat!(
        "WITH med AS (",
        "  SELECT name, count(*) AS n, ",
        "  percentile_cont(0.5) WITHIN GROUP (ORDER BY price_each) AS median_each ",
        "  FROM market_listing WHERE last_seen >= $1 AND created_at >= $2 ",
        "  GROUP BY name HAVING count(*) >= $3) ",
        "SELECT l.id, l.name, l.stack, l.price, l.price_each, l.created_at, ",
        "  l.last_seen, m.median_each, m.n AS sample_size, ",
        "  (1 - l.price_each / m.median_each) AS discount ",
        "FROM market_listing l JOIN med m USING (name) ",
        "WHERE l.last_seen >= $1 AND l.created_at >= $2 ",
        "  AND m.median_each > 0 ",
        "  AND l.price_each <= m.median_each * (1 - $4) ",
        "ORDER BY discount DESC LIMIT $5",
    );
    let args = vec![
        Arg::Int(last_seen_floor),
        Arg::Int(created_at_floor),
        Arg::Int(min_samples),
        Arg::Float(min_discount),
        Arg::Int(limit),
    ];
    let rows = s.fetch(sql, args).await?;
    rows.iter()
        .map(|r| {
            Ok(json!({
                "id": uuid(r, "id")?,
                "name": raw(r, "name"),
                "stack": int(r, "stack")?,
                "price": int(r, "price")?,
                "price_each": float(r, "price_each")?,
                "median_each": float(r, "median_each")?,
                "sample_size": int(r, "sample_size")?,
                "discount": py_round(float(r, "discount")?, 4),
                "created_at": int(r, "created_at")?,
                "last_seen": int(r, "last_seen")?,
            }))
        })
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Abs,
    Up,
    Down,
}

/// `pg_store.market_movers`.
pub async fn market_movers<S: Store>(
    s: &S,
    recent_start: i64,
    prior_start: i64,
    now: i64,
    min_samples: i64,
    limit: i64,
    direction: Direction,
) -> ApiResult<Vec<Value>> {
    let (extra, order) = match direction {
        Direction::Abs => ("", "abs((r.med - p.med) / p.med) DESC"),
        Direction::Up => ("AND r.med > p.med", "(r.med - p.med) / p.med DESC"),
        Direction::Down => ("AND r.med < p.med", "(r.med - p.med) / p.med ASC"),
    };
    let sql = format!(
        concat!(
            "WITH recent AS (",
            "  SELECT name, count(*) AS n, ",
            "  percentile_cont(0.5) WITHIN GROUP (ORDER BY price_each) AS med ",
            "  FROM market_listing WHERE created_at >= $1 AND created_at < $3 ",
            "  GROUP BY name HAVING count(*) >= $4), ",
            "prior AS (",
            "  SELECT name, count(*) AS n, ",
            "  percentile_cont(0.5) WITHIN GROUP (ORDER BY price_each) AS med ",
            "  FROM market_listing WHERE created_at >= $2 AND created_at < $1 ",
            "  GROUP BY name HAVING count(*) >= $4) ",
            "SELECT r.name, r.med AS recent_med, p.med AS prior_med, r.n AS recent_n, ",
            "  (r.med - p.med) / p.med AS change ",
            "FROM recent r JOIN prior p USING (name) WHERE p.med > 0 {} ",
            "ORDER BY {} LIMIT $5",
        ),
        extra, order
    );
    let args =
        vec![Arg::Int(recent_start), Arg::Int(prior_start), Arg::Int(now), Arg::Int(min_samples), Arg::Int(limit)];
    let rows = s.fetch(&sql, args).await?;
    rows.iter()
        .map(|r| {
            Ok(json!({
                "name": raw(r, "name"),
                "recent_med": float(r, "recent_med")?,
                "prior_med": float(r, "prior_med")?,
                "recent_n": int(r, "recent_n")?,
                "change": py_round(float(r, "change")?, 4),
            }))
        })
        .collect()
}

/// `pg_store.market_overview`.
pub async fn market_overview<S: Store>(s: &S, stale_floor: i64, lifetime_floor: i64) -> ApiResult<Map<String, Value>> {
    let sql = concat!(
        "SELECT count(*) AS listings, count(DISTINCT name) AS items, ",
        "COALESCE(sum(price), 0) AS total_value, COALESCE(sum(stack), 0) AS total_units ",
        "FROM market_listing WHERE last_seen >= $1 AND created_at >= $2",
    );
    let r = s.fetchrow(sql, vec![Arg::Int(stale_floor), Arg::Int(lifetime_floor)]).await?;
    // `r["listings"]` on a missing row is a TypeError in Python.
    let r = r.ok_or_else(|| bad_row("overview"))?;
    let mut out = Map::new();
    out.insert("active_listings".into(), int0(&r, "listings")?.into());
    out.insert("active_items".into(), int0(&r, "items")?.into());
    out.insert("total_value".into(), int0(&r, "total_value")?.into());
    out.insert("total_units".into(), int0(&r, "total_units")?.into());
    Ok(out)
}

/// `pg_store.volume_leaders`.
pub async fn volume_leaders<S: Store>(s: &S, window_start: i64, limit: i64) -> ApiResult<Vec<Value>> {
    let sql = concat!(
        "SELECT name, count(*) AS listings, ",
        "COALESCE(sum(stack), 0) AS units, COALESCE(sum(price), 0) AS total_value, ",
        "percentile_cont(0.5) WITHIN GROUP (ORDER BY price_each) AS median_each ",
        "FROM market_listing WHERE created_at >= $1 ",
        "GROUP BY name ORDER BY listings DESC, units DESC LIMIT $2",
    );
    let rows = s.fetch(sql, vec![Arg::Int(window_start), Arg::Int(limit)]).await?;
    rows.iter()
        .map(|r| {
            let median = match r.get("median_each") {
                PgValue::Null => 0.0,
                v => v.float().ok_or_else(|| bad_row("median_each"))?,
            };
            Ok(json!({
                "name": raw(r, "name"),
                "listings": int(r, "listings")?,
                "units": int0(r, "units")?,
                "total_value": int0(r, "total_value")?,
                "median_each": median,
            }))
        })
        .collect()
}

/// `pg_store.market_liquidity`.
pub async fn market_liquidity<S: Store>(
    s: &S,
    concluded_before: i64,
    window_start: i64,
    expire_lifespan: i64,
    min_samples: i64,
    limit: i64,
) -> ApiResult<Vec<Value>> {
    let sql = concat!(
        "WITH concluded AS (",
        "  SELECT name, (last_seen - created_at) AS lifespan ",
        "  FROM market_listing WHERE last_seen < $1 AND last_seen >= $2) ",
        "SELECT name, count(*) AS concluded, ",
        "  count(*) FILTER (WHERE lifespan < $3) AS sold, ",
        "  count(*) FILTER (WHERE lifespan >= $3) AS expired, ",
        "  percentile_cont(0.5) WITHIN GROUP (ORDER BY lifespan) ",
        "    FILTER (WHERE lifespan < $3) AS median_tts ",
        "FROM concluded GROUP BY name HAVING count(*) >= $4 ",
        "ORDER BY (count(*) FILTER (WHERE lifespan < $3))::float / count(*) DESC, ",
        "         count(*) DESC LIMIT $5",
    );
    let args = vec![
        Arg::Int(concluded_before),
        Arg::Int(window_start),
        Arg::Int(expire_lifespan),
        Arg::Int(min_samples),
        Arg::Int(limit),
    ];
    let rows = s.fetch(sql, args).await?;
    rows.iter()
        .map(|r| {
            let concluded = int(r, "concluded")?;
            let sold = int(r, "sold")?;
            let tts = match r.get("median_tts") {
                PgValue::Null => Value::Null,
                v => v.int().ok_or_else(|| bad_row("median_tts"))?.into(),
            };
            Ok(json!({
                "name": raw(r, "name"),
                "concluded": concluded,
                "sold": sold,
                "expired": int(r, "expired")?,
                "sell_through": if concluded != 0 { py_round(sold as f64 / concluded as f64, 4) } else { 0.0 },
                "median_time_to_sell": tts,
            }))
        })
        .collect()
}

/// `pg_store.reset`: the row count before the wipe.
pub async fn reset<S: Store>(s: &S) -> ApiResult<i64> {
    let n = s.fetchval("SELECT count(*) FROM market_listing", Vec::new()).await?;
    s.run("execute", "TRUNCATE market_listing", Vec::new()).await?;
    n.int_or_zero().ok_or_else(|| bad_row("count"))
}

/// `codexes_read.blueprints_for_names`: lower(name) -> blueprint, for names
/// with one unambiguous best-ranked blueprint. Empty without Postgres.
pub async fn blueprints_for_names<S: Store>(
    s: &S,
    branch: &str,
    names: &[String],
) -> ApiResult<HashMap<String, String>> {
    if !s.pg_enabled() || names.is_empty() {
        return Ok(HashMap::new());
    }
    let lowered: BTreeSet<String> = names.iter().map(|n| n.to_lowercase()).collect();
    let sql = concat!(
        "SELECT lower(name) AS lname, blueprint, path FROM codex_entry ",
        "WHERE branch = $1 AND blueprint IS NOT NULL AND blueprint <> '' ",
        "AND lower(name) = ANY($2::text[])",
    );
    let rows = s.fetch(sql, vec![Arg::Text(branch.to_owned()), Arg::TextList(lowered.into_iter().collect())]).await?;
    let mut cands = Vec::with_capacity(rows.len());
    for r in &rows {
        cands.push((text(r, "lname")?, text(r, "blueprint")?, r.get("path").text().map(str::to_owned)));
    }
    Ok(kiwi_core::market::pick_blueprints(cands.iter().map(|(l, b, p)| (l.as_str(), b.as_str(), p.as_deref()))))
}

// --- Mongo documents -----------------------------------------------------------

/// A `MarketItemCategory` as Beanie loads it (a document it can't validate is a 500).
#[derive(Clone, Debug, PartialEq)]
pub struct Category {
    pub id: ObjectId,
    pub name: String,
    pub order: i64,
    pub items: Vec<String>,
    pub created_by: Option<ObjectId>,
    pub created_at: DateTime<Utc>,
}

fn object_id(v: Option<&Bson>, what: &str) -> ApiResult<Option<ObjectId>> {
    match v {
        None | Some(Bson::Null) => Ok(None),
        Some(Bson::ObjectId(o)) => Ok(Some(*o)),
        Some(Bson::String(s)) => ObjectId::parse_str(s).map(Some).map_err(internal),
        Some(other) => Err(internal(format!("{what} {other:?}"))),
    }
}

fn datetime(v: Option<&Bson>, what: &str) -> ApiResult<DateTime<Utc>> {
    match v {
        None => Ok(dt::now()),
        Some(Bson::DateTime(b)) => Ok(dt::from_bson(*b)),
        Some(other) => Err(internal(format!("{what} {other:?}"))),
    }
}

/// pydantic's lax `int` of a stored number.
fn bson_int(v: &Bson) -> Option<i64> {
    match v {
        Bson::Int32(i) => Some((*i).into()),
        Bson::Int64(i) => Some(*i),
        Bson::Double(f) if f.fract() == 0.0 && f.is_finite() => Some(*f as i64),
        Bson::Boolean(b) => Some(*b as i64),
        _ => None,
    }
}

impl Category {
    pub fn from_doc(d: &Document) -> ApiResult<Self> {
        let id = match d.get("_id") {
            Some(Bson::ObjectId(o)) => *o,
            other => return Err(internal(format!("category _id {other:?}"))),
        };
        let name = match d.get("name") {
            Some(Bson::String(s)) => s.clone(),
            other => return Err(internal(format!("category name {other:?}"))),
        };
        let order = match d.get("order") {
            None => 0,
            Some(v) => bson_int(v).ok_or_else(|| internal(format!("category order {v:?}")))?,
        };
        let items = match d.get("items") {
            None => Vec::new(),
            Some(Bson::Array(a)) => a
                .iter()
                .map(|v| match v {
                    Bson::String(s) => Ok(s.clone()),
                    other => Err(internal(format!("category item {other:?}"))),
                })
                .collect::<ApiResult<_>>()?,
            Some(other) => return Err(internal(format!("category items {other:?}"))),
        };
        Ok(Category {
            id,
            name,
            order,
            items,
            created_by: object_id(d.get("created_by"), "category created_by")?,
            created_at: datetime(d.get("created_at"), "category created_at")?,
        })
    }

    /// The full document Beanie writes (`_id` first, model order).
    pub fn to_doc(&self) -> Document {
        doc! {
            "_id": self.id,
            "name": &self.name,
            "order": crate::auth::pyutil::bson_int(self.order),
            "items": self.items.clone(),
            "created_by": self.created_by,
            "created_at": dt::to_bson(self.created_at),
        }
    }
}

/// A `MarketInterestItem`.
#[derive(Clone, Debug, PartialEq)]
pub struct InterestItem {
    pub name: String,
    pub added_by: Option<ObjectId>,
    pub added_at: DateTime<Utc>,
}

impl InterestItem {
    pub fn from_doc(d: &Document) -> ApiResult<Self> {
        let name = match d.get("name") {
            Some(Bson::String(s)) => s.clone(),
            other => return Err(internal(format!("interest name {other:?}"))),
        };
        Ok(InterestItem {
            name,
            added_by: object_id(d.get("added_by"), "interest added_by")?,
            added_at: datetime(d.get("added_at"), "interest added_at")?,
        })
    }

    pub fn to_doc(&self, id: ObjectId) -> Document {
        doc! { "_id": id, "name": &self.name, "added_by": self.added_by, "added_at": dt::to_bson(self.added_at) }
    }
}

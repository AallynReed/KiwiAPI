//! `POST /v1/market/insert` (`app/trove/router.py`, `service.insert_dump`,
//! `pg_store.upsert_listings`): the scraper's GrainusMod.cfg dump, upserted
//! into `market_listing`, checked against the DM watchlists, logged, and
//! announced as a `market` event.

use axum::{
    extract::{FromRequestParts, Request, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use bytes::BytesMut;
use futures::future::try_join_all;
use kiwi_core::{
    form::{self, FormError, FormValue},
    market::parser::{ParsedListing, parse_dump},
};
use mongodb::bson::{Bson, Document, doc};
use serde_json::{Value, json};
use tokio_postgres::types::{IsNull, ToSql, Type, to_sql_checked};

use crate::{
    auth::{IngestAuth, MasterIngest, pyutil::bson_int},
    error::ApiError,
    features::{self, Feature},
    lb_ingest::signals,
    pyjson::PydanticJson,
    state::AppState,
    validation::{Fail, Params, ValidationError},
};

pub const PATH: &str = "/v1/market/insert";
pub const EMPTY_UPLOAD: &str = "Empty upload - POST the raw cfg text as a multipart 'file' field.";
const CLOCK_SKEW_GRACE_SECONDS: i64 = 300;
const UPSERT_SQL: &str = "INSERT INTO market_listing (id, name, type, stack, price, price_each, created_at, last_seen) \
                          VALUES ($1, $2, $3, $4, $5, $6, $7, $8) \
                          ON CONFLICT (id) DO UPDATE SET last_seen = EXCLUDED.last_seen";

/// `file: UploadFile = File(...)` and `timestamp: int | None = Query(None)`,
/// validated once every dependency has run (query errors first).
pub fn validate_upload(
    query: Option<&str>,
    headers: &HeaderMap,
    items: Vec<(String, FormValue)>,
    saturate: bool,
) -> Result<(Vec<u8>, Option<i64>), ValidationError> {
    let mut p = Params::new(Vec::new(), query, headers);
    let ts = p.query("timestamp").int();
    let timestamp = if saturate { ts.saturating().optional() } else { ts.optional() };
    let mut v = p.v;
    let loc = || vec!["body".into(), "file".into()];
    let file = match items.into_iter().rev().find(|(k, _)| k == "file").map(|(_, v)| v) {
        Some(FormValue::File(f)) => f.data,
        Some(FormValue::Text(t)) if !t.is_empty() => {
            v.push(loc(), Value::String(t), Fail::value_error("Expected UploadFile, received: <class 'str'>"));
            Vec::new()
        }
        _ => {
            v.push(loc(), Value::Null, Fail::missing());
            Vec::new()
        }
    };
    v.finish((file, timestamp))
}

/// The shared front half of the two cfg-dump inserts: the form (a parse error
/// is a 400 before anything runs), the router's feature gate, the master
/// gate, then parameter validation and the empty-upload 400.
pub async fn read_upload<F: Feature>(
    state: &AppState,
    req: Request,
    saturate: bool,
) -> Result<(IngestAuth, Vec<u8>, Option<i64>), Response> {
    let (mut parts, body) = req.into_parts();
    let parse_error = |msg: &str| ApiError::http(StatusCode::BAD_REQUEST, Some(msg)).into_response();
    let Ok(bytes) = axum::body::to_bytes(body, usize::MAX).await else {
        return Err(parse_error("There was an error parsing the body"));
    };
    let items = match form::parse_form(parts.headers.get(header::CONTENT_TYPE).map(|v| v.as_bytes()), &bytes) {
        Ok(items) => items,
        Err(FormError::Http(msg)) => return Err(parse_error(&msg)),
    };
    drop(bytes);
    features::check::<F>(state).await.map_err(IntoResponse::into_response)?;
    let MasterIngest(auth) =
        MasterIngest::from_request_parts(&mut parts, state).await.map_err(IntoResponse::into_response)?;
    let (file, timestamp) =
        validate_upload(parts.uri.query(), &parts.headers, items, saturate).map_err(IntoResponse::into_response)?;
    if file.is_empty() {
        return Err(ApiError::bad_request(EMPTY_UPLOAD).into_response());
    }
    Ok((auth, file, timestamp))
}

/// `raw.decode("utf-8")`, falling back to `errors="replace"`.
pub fn decode(raw: &[u8]) -> String {
    String::from_utf8_lossy(raw).into_owned()
}

/// A UUID bound to a `uuid` parameter.
#[derive(Debug)]
struct PgUuid(u128);

impl ToSql for PgUuid {
    fn to_sql(&self, _: &Type, out: &mut BytesMut) -> Result<IsNull, Box<dyn std::error::Error + Sync + Send>> {
        out.extend_from_slice(&self.0.to_be_bytes());
        Ok(IsNull::No)
    }

    fn accepts(ty: &Type) -> bool {
        *ty == Type::UUID
    }

    to_sql_checked!();
}

/// `insert_dump`'s summary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Summary {
    pub parsed: usize,
    pub imported: usize,
    pub ignored: usize,
    pub last_seen: Option<i64>,
}

/// `last_seen`: the override when positive, else now; never past now + 300 s.
pub fn last_seen(timestamp: Option<i64>, now: i64) -> i64 {
    let ls = timestamp.filter(|t| *t > 0).unwrap_or(now);
    if ls > now + CLOCK_SKEW_GRACE_SECONDS {
        tracing::warn!(
            "market: client last_seen {ls} is {}s in the future - clamping to {now}. Check the scraper host's clock.",
            ls - now
        );
        return now;
    }
    ls
}

/// The kept rows and how many were not on the interest list.
pub fn select_rows<'a>(
    listings: &'a [ParsedListing],
    interest: &std::collections::BTreeSet<String>,
) -> (Vec<&'a ParsedListing>, usize) {
    let (rows, ignored): (Vec<_>, Vec<_>) = listings.iter().partition(|l| interest.contains(&l.name));
    (rows, ignored.len())
}

/// `{item: cheapest price_each}` in first-seen order, for `check_market`.
pub fn cheapest(rows: &[&ParsedListing]) -> Vec<(String, f64)> {
    let mut out: Vec<(String, f64)> = Vec::new();
    for r in rows {
        match out.iter_mut().find(|(n, _)| *n == r.name) {
            Some((_, pe)) if r.price_each < *pe => *pe = r.price_each,
            Some(_) => {}
            None => out.push((r.name.clone(), r.price_each)),
        }
    }
    out
}

/// `pg_store.upsert_listings`: one atomic `executemany`.
async fn upsert(state: &AppState, rows: &[&ParsedListing], last_seen: i64) -> anyhow::Result<usize> {
    if rows.is_empty() {
        return Ok(0);
    }
    let pool = state.inner.db.pg.as_ref().ok_or_else(|| anyhow::anyhow!("Postgres pool not initialized"))?;
    let mut client = pool.get().await?;
    let tx = client.transaction().await?;
    let stmt = tx.prepare(UPSERT_SQL).await?;
    let params: Vec<_> = rows
        .iter()
        .map(|r| {
            let stack = i32::try_from(r.stack).unwrap_or(i32::MAX);
            (PgUuid(r.id), r.name.as_str(), r.kind.as_deref(), stack, r.price, r.price_each, r.created_at)
        })
        .collect();
    type Param<'a> = &'a (dyn ToSql + Sync);
    let bound: Vec<[Param<'_>; 8]> = params
        .iter()
        .map(|(id, name, kind, stack, price, pe, created)| {
            [id as Param<'_>, name, kind, stack, price, pe, created, &last_seen]
        })
        .collect();
    try_join_all(bound.iter().map(|p| tx.execute(&stmt, p))).await?;
    tx.commit().await?;
    Ok(rows.len())
}

/// `service.insert_dump(text, timestamp=...)`.
pub async fn insert_dump(state: &AppState, text: &str, timestamp: Option<i64>) -> Result<Summary, ApiError> {
    let listings = parse_dump(text);
    if listings.is_empty() {
        tracing::warn!("market: parsed 0 listings from {}-char dump", text.chars().count());
        return Ok(Summary { parsed: 0, imported: 0, ignored: 0, last_seen: None });
    }
    let last_seen = last_seen(timestamp, chrono::Utc::now().timestamp());
    let interest = crate::routes::misc::interest_set(state).await?;
    let (rows, ignored) = select_rows(&listings, &interest);
    let imported = upsert(state, &rows, last_seen).await.map_err(|e| super::store::internal(format!("{e:#}")))?;
    let prices = cheapest(&rows);
    if !prices.is_empty()
        && let Err(e) = crate::dm_subs::check_market(state, &prices).await
    {
        tracing::warn!("market: DM watchlist check failed: {e:#}");
    }
    tracing::info!(
        "market: ingested last_seen={last_seen} parsed={} imported={imported} ignored={ignored}",
        listings.len()
    );
    Ok(Summary { parsed: listings.len(), imported, ignored, last_seen: Some(last_seen) })
}

fn opt_int(v: Option<i64>) -> Bson {
    v.map_or(Bson::Null, bson_int)
}

pub fn log_summary(s: &Summary, nbytes: usize) -> Document {
    doc! {
        "parsed": bson_int(s.parsed as i64),
        "imported": bson_int(s.imported as i64),
        "ignored_not_in_list": bson_int(s.ignored as i64),
        "last_seen": opt_int(s.last_seen),
        "bytes": bson_int(nbytes as i64),
    }
}

/// `MarketInsertResponse`.
pub fn response_body(s: &Summary) -> Value {
    json!({"parsed": s.parsed, "imported": s.imported, "ignored_not_in_list": s.ignored, "last_seen": s.last_seen})
}

pub async fn insert(State(state): State<AppState>, req: Request) -> Response {
    let (auth, raw, timestamp) = match read_upload::<features::Market>(&state, req, true).await {
        Ok(x) => x,
        Err(r) => return r,
    };
    let text = decode(&raw);
    let summary = match insert_dump(&state, &text, timestamp).await {
        Ok(s) => s,
        Err(e) => return e.into_response(),
    };
    signals::record(&state, PATH, &auth.user, auth.token.as_ref(), log_summary(&summary, raw.len()), None).await;
    if let Some(ls) = summary.last_seen.filter(|ls| *ls != 0) {
        let data = json!({"last_seen": ls, "imported": summary.imported, "parsed": summary.parsed});
        crate::events::publish(&state, "market", &ls.to_string(), data).await;
    }
    (auth.rate_limit, PydanticJson(response_body(&summary))).into_response()
}

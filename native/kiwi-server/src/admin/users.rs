//! `/admin/users*`, `/admin/tokens/*`, `/admin/events` and `/admin/activity`:
//! dev-portal accounts, their API tokens and the usage they generate.

use std::collections::HashMap;

use axum::extract::State;
use chrono::Duration;
use futures::TryStreamExt;
use mongodb::bson::{Bson, DateTime, Document, doc, oid::ObjectId};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{
    Args, Res, coll,
    docs::{self, find_all, find_one, pyd, pyd_opt},
    internal,
};
use crate::{
    auth::{
        Superuser,
        models::{API_TOKENS, ApiToken, SESSIONS, USERS, User},
        scopes,
    },
    dt,
    error::ApiError,
    pyfloat,
    pyjson::PydanticJson,
    state::AppState,
    usage::USAGE_COLLECTION,
    validation::{FromParams, Params},
};

// --- parameters --------------------------------------------------------------------

/// `days: int = Query(default=D, ge=1, le=MAX)`.
pub struct Days<const D: i64, const MAX: i64>(pub i64);

impl<const D: i64, const MAX: i64> FromParams for Days<D, MAX> {
    fn from_params(p: &mut Params<'_>) -> Self {
        Days(p.query("days").int().ge(1).le(MAX).default(D))
    }
}

fn oid_path(p: &mut Params<'_>, name: &str) -> ObjectId {
    p.path(name).object_id().required()
}

pub struct UserPath(pub ObjectId);

impl FromParams for UserPath {
    fn from_params(p: &mut Params<'_>) -> Self {
        UserPath(oid_path(p, "user_id"))
    }
}

pub struct TokenPath(pub ObjectId);

impl FromParams for TokenPath {
    fn from_params(p: &mut Params<'_>) -> Self {
        TokenPath(oid_path(p, "token_id"))
    }
}

/// `user_id` path plus `days: int = Query(default=7, ge=1, le=365)`.
pub struct UserDays {
    pub user_id: ObjectId,
    pub days: i64,
}

impl FromParams for UserDays {
    fn from_params(p: &mut Params<'_>) -> Self {
        let user_id = oid_path(p, "user_id");
        UserDays { user_id, days: p.query("days").int().ge(1).le(365).default(7) }
    }
}

/// `list_events`' query.
pub struct EventsQuery {
    pub user_id: Option<ObjectId>,
    pub status_code: Option<i64>,
    pub cursor: Option<String>,
    pub limit: i64,
}

impl FromParams for EventsQuery {
    fn from_params(p: &mut Params<'_>) -> Self {
        EventsQuery {
            user_id: p.query("user_id").object_id().optional(),
            status_code: p.query("status_code").int().ge(100).le(599).optional(),
            cursor: p.query("cursor").str().optional(),
            limit: p.query("limit").int().ge(1).le(200).default(50),
        }
    }
}

/// `activity_overview`'s query.
pub struct OverviewQuery {
    pub days: i64,
    pub top: i64,
}

impl FromParams for OverviewQuery {
    fn from_params(p: &mut Params<'_>) -> Self {
        OverviewQuery {
            days: p.query("days").int().ge(1).le(365).default(7),
            top: p.query("top").int().ge(1).le(100).default(10),
        }
    }
}

// --- shared --------------------------------------------------------------------------

/// `str(x)` of a grouped `_id`.
pub fn id_key(b: Option<&Bson>) -> String {
    match b {
        Some(Bson::ObjectId(o)) => o.to_hex(),
        Some(Bson::String(s)) => s.clone(),
        None | Some(Bson::Null) => "None".into(),
        Some(other) => other.to_string(),
    }
}

/// An aggregated number read into a pydantic `int` field.
pub fn int_field(d: &Document, key: &str) -> Result<i64, ApiError> {
    match d.get(key) {
        Some(Bson::Int32(n)) => Ok(i64::from(*n)),
        Some(Bson::Int64(n)) => Ok(*n),
        Some(Bson::Double(f)) if f.fract() == 0.0 && f.is_finite() => Ok(*f as i64),
        other => Err(internal(format!("{key}: {other:?}"))),
    }
}

/// `round(x or 0, 2)` of an `$avg`, as the float field it fills.
pub fn avg_ms(d: &Document, key: &str) -> Value {
    let x = match d.get(key) {
        Some(Bson::Double(f)) => *f,
        Some(Bson::Int32(n)) => f64::from(*n),
        Some(Bson::Int64(n)) => *n as f64,
        _ => 0.0,
    };
    json!(pyfloat::round(x, 2))
}

fn date_or_null(b: Option<&Bson>) -> Result<Value, ApiError> {
    match b {
        None | Some(Bson::Null) => Ok(Value::Null),
        Some(Bson::DateTime(d)) => Ok(pyd(d).into()),
        Some(other) => Err(internal(format!("datetime field: {other}"))),
    }
}

/// `ERROR_COND`.
pub fn error_cond() -> Document {
    doc! {"$cond": [{"$gte": ["$status_code", 400]}, 1, 0]}
}

/// `RATE_LIMITED_COND`.
pub fn rate_limited_cond() -> Document {
    doc! {"$cond": [{"$eq": ["$status_code", 429]}, 1, 0]}
}

pub async fn aggregate(state: &AppState, name: &str, pipeline: Vec<Document>) -> Result<Vec<Document>, ApiError> {
    coll(state, name).aggregate(pipeline).await.map_err(internal)?.try_collect().await.map_err(internal)
}

fn since(days: i64) -> DateTime {
    dt::to_bson(dt::now() - Duration::days(days))
}

/// `AdminUserSummary`.
pub fn user_summary(u: &User, token_count: i64, total_requests: i64, last_used_at: Value, rate_limited: i64) -> Value {
    json!({
        "id": u.id.to_hex(),
        "email": docs::normalize_email(&u.email),
        "display_name": u.display_name,
        "is_active": u.is_active,
        "is_verified": u.is_verified,
        "is_superuser": u.is_superuser,
        "created_at": pyd(&u.created_at),
        "last_login_at": pyd_opt(&u.last_login_at),
        "token_count": token_count,
        "total_requests": total_requests,
        "last_used_at": last_used_at,
        "rate_limited": rate_limited,
    })
}

/// `_token_view` (`AdminTokenView`).
pub fn token_view(t: &ApiToken) -> Value {
    json!({
        "id": t.id.to_hex(),
        "user_id": t.user_id.to_hex(),
        "name": t.name,
        "prefix": t.prefix,
        "scopes": t.scopes,
        "scope_names": scopes::decode(t.scopes),
        "allowed_ip_count": t.allowed_ip_hashes.len(),
        "revoked": t.revoked,
        "revoked_at": pyd_opt(&t.revoked_at),
        "revoke_reason": t.revoke_reason,
        "created_at": pyd(&t.created_at),
        "last_used_at": pyd_opt(&t.last_used_at),
        "rotated_at": pyd_opt(&t.rotated_at),
        "expires_at": pyd_opt(&t.expires_at),
        "request_count": t.request_count,
    })
}

async fn require_user(state: &AppState, id: ObjectId) -> Result<User, ApiError> {
    find_one(&coll(state, USERS), doc! {"_id": id}).await?.ok_or_else(|| ApiError::not_found("User not found"))
}

async fn user_tokens(state: &AppState, id: ObjectId) -> Result<Vec<ApiToken>, ApiError> {
    find_all(&coll(state, API_TOKENS), doc! {"user_id": id}, None).await
}

/// `ApiToken.find(user_id, revoked == False)`: count, then revoke them all.
async fn revoke_active(state: &AppState, id: ObjectId, reason: &str) -> Result<u64, ApiError> {
    let c = coll(state, API_TOKENS);
    let filter = doc! {"$and": [{"user_id": id}, {"revoked": false}]};
    let count = c.count_documents(filter.clone()).await.map_err(internal)?;
    if count > 0 {
        let set = doc! {"$set": {"revoked": true, "revoked_at": docs::now(), "revoke_reason": reason}};
        c.update_many(filter, set).await.map_err(internal)?;
    }
    Ok(count)
}

// --- users -------------------------------------------------------------------------

/// `list_users` from its three reads.
pub fn user_summaries(users: &[User], token_rows: &[Document], rl_rows: &[Document]) -> Result<Value, ApiError> {
    let stats: HashMap<String, &Document> = token_rows.iter().map(|r| (id_key(r.get("_id")), r)).collect();
    let mut rl: HashMap<String, i64> = HashMap::new();
    for r in rl_rows {
        rl.insert(id_key(r.get("_id")), int_field(r, "rate_limited")?);
    }
    let mut out = Vec::with_capacity(users.len());
    for u in users {
        let key = u.id.to_hex();
        let (count, total, last) = match stats.get(&key) {
            Some(s) => {
                (int_field(s, "token_count")?, int_field(s, "total_requests")?, date_or_null(s.get("last_used_at"))?)
            }
            None => (0, 0, Value::Null),
        };
        out.push(user_summary(u, count, total, last, rl.get(&key).copied().unwrap_or(0)));
    }
    Ok(Value::Array(out))
}

pub async fn list_users(State(state): State<AppState>, _: Superuser, Args(Days(days)): Args<Days<30, 365>>) -> Res {
    let tokens = aggregate(
        &state,
        API_TOKENS,
        vec![doc! {"$group": {
            "_id": "$user_id",
            "token_count": {"$sum": 1},
            "total_requests": {"$sum": "$request_count"},
            "last_used_at": {"$max": "$last_used_at"},
        }}],
    )
    .await?;
    let rl = aggregate(
        &state,
        USAGE_COLLECTION,
        vec![
            doc! {"$match": {"created_at": {"$gte": since(days)}, "status_code": 429}},
            doc! {"$group": {"_id": "$user_id", "rate_limited": {"$sum": 1}}},
        ],
    )
    .await?;
    let users: Vec<User> = find_all(&coll(&state, USERS), doc! {}, None).await?;
    Ok(PydanticJson(user_summaries(&users, &tokens, &rl)?))
}

pub async fn list_user_tokens(State(state): State<AppState>, _: Superuser, Args(UserPath(id)): Args<UserPath>) -> Res {
    require_user(&state, id).await?;
    let tokens = user_tokens(&state, id).await?;
    Ok(PydanticJson(Value::Array(tokens.iter().map(token_view).collect())))
}

pub async fn revoke_user_tokens(
    State(state): State<AppState>,
    _: Superuser,
    Args(UserPath(id)): Args<UserPath>,
) -> Res {
    require_user(&state, id).await?;
    let n = revoke_active(&state, id, "Revoked by administrator").await?;
    Ok(PydanticJson(json!({"revoked": n})))
}

/// `revoke_all_sessions(user)` after the caller's edits: one `save()` with the
/// bumped `token_version`, then every live session revoked.
async fn save_revoking_sessions(state: &AppState, u: &mut User) -> Result<(), ApiError> {
    u.token_version += 1;
    u.updated_at = docs::now();
    docs::save(&coll(state, USERS), u.id, docs::user_doc(u)).await?;
    coll(state, SESSIONS)
        .update_many(doc! {"$and": [{"user_id": u.id}, {"revoked": false}]}, doc! {"$set": {"revoked": true}})
        .await
        .map_err(internal)?;
    Ok(())
}

pub async fn terminate_user(
    State(state): State<AppState>,
    Superuser(admin): Superuser,
    Args(UserPath(id)): Args<UserPath>,
) -> Res {
    let mut user = require_user(&state, id).await?;
    if user.is_superuser || user.id == admin.id {
        return Err(ApiError::bad_request("Admin accounts can't be terminated"));
    }
    user.is_active = false;
    save_revoking_sessions(&state, &mut user).await?;
    let n = revoke_active(&state, id, "Access terminated by administrator").await?;
    Ok(PydanticJson(json!({"id": user.id.to_hex(), "is_active": false, "revoked": n})))
}

pub async fn reactivate_user(State(state): State<AppState>, _: Superuser, Args(UserPath(id)): Args<UserPath>) -> Res {
    let mut user = require_user(&state, id).await?;
    user.is_active = true;
    user.updated_at = docs::now();
    docs::save(&coll(&state, USERS), user.id, docs::user_doc(&user)).await?;
    Ok(PydanticJson(json!({"id": user.id.to_hex(), "is_active": true})))
}

/// `aggregate_activity`'s three pipelines for one match.
pub fn activity_pipelines(base: Document, since: DateTime) -> [Vec<Document>; 3] {
    let mut m = base;
    m.insert("created_at", doc! {"$gte": since});
    let totals = vec![
        doc! {"$match": m.clone()},
        doc! {"$group": {
            "_id": Bson::Null,
            "count": {"$sum": 1},
            "error_count": {"$sum": error_cond()},
            "rate_limited": {"$sum": rate_limited_cond()},
            "avg_duration_ms": {"$avg": "$duration_ms"},
        }},
    ];
    let by_day = vec![
        doc! {"$match": m.clone()},
        doc! {"$group": {
            "_id": {"$dateToString": {"format": "%Y-%m-%d", "date": "$created_at", "timezone": "UTC"}},
            "count": {"$sum": 1},
            "error_count": {"$sum": error_cond()},
            "avg_duration_ms": {"$avg": "$duration_ms"},
        }},
        doc! {"$sort": {"_id": 1}},
    ];
    let by_endpoint = vec![
        doc! {"$match": m},
        doc! {"$group": {
            "_id": {"route": "$route", "method": "$method"},
            "count": {"$sum": 1},
            "error_count": {"$sum": error_cond()},
            "avg_duration_ms": {"$avg": "$duration_ms"},
        }},
        doc! {"$sort": {"count": -1}},
    ];
    [totals, by_day, by_endpoint]
}

fn str_of(b: Option<&Bson>, what: &str) -> Result<String, ApiError> {
    match b {
        Some(Bson::String(s)) => Ok(s.clone()),
        other => Err(internal(format!("{what}: {other:?}"))),
    }
}

/// The `{count, error_count, rate_limited, avg}` head shared by both summaries.
fn totals_head(totals: &[Document]) -> Result<(i64, i64, i64, Value), ApiError> {
    match totals.first() {
        None => Ok((0, 0, 0, json!(0.0))),
        Some(t) => {
            let rl = if t.contains_key("rate_limited") { int_field(t, "rate_limited")? } else { 0 };
            Ok((int_field(t, "count")?, int_field(t, "error_count")?, rl, avg_ms(t, "avg_duration_ms")))
        }
    }
}

/// `ActivitySummary` from `aggregate_activity`'s rows.
pub fn activity_summary(days: i64, rows: &[Vec<Document>; 3]) -> Result<Value, ApiError> {
    let (count, errors, rl, avg) = totals_head(&rows[0])?;
    let mut by_day = Vec::new();
    for r in &rows[1] {
        by_day.push(json!({
            "date": str_of(r.get("_id"), "date")?,
            "count": int_field(r, "count")?,
            "error_count": int_field(r, "error_count")?,
            "avg_duration_ms": avg_ms(r, "avg_duration_ms"),
        }));
    }
    let mut by_endpoint = Vec::new();
    for r in &rows[2] {
        let id = r.get_document("_id").map_err(internal)?;
        by_endpoint.push(json!({
            "route": str_of(id.get("route"), "route")?,
            "method": str_of(id.get("method"), "method")?,
            "count": int_field(r, "count")?,
            "error_count": int_field(r, "error_count")?,
            "avg_duration_ms": avg_ms(r, "avg_duration_ms"),
        }));
    }
    Ok(json!({
        "window_days": days,
        "total_requests": count,
        "error_count": errors,
        "rate_limited": rl,
        "avg_duration_ms": avg,
        "by_day": by_day,
        "by_endpoint": by_endpoint,
    }))
}

pub async fn aggregate_activity(state: &AppState, base: Document, days: i64) -> Result<Value, ApiError> {
    let [a, b, c] = activity_pipelines(base, since(days));
    let rows = [
        aggregate(state, USAGE_COLLECTION, a).await?,
        aggregate(state, USAGE_COLLECTION, b).await?,
        aggregate(state, USAGE_COLLECTION, c).await?,
    ];
    activity_summary(days, &rows)
}

pub async fn user_activity(State(state): State<AppState>, _: Superuser, Args(q): Args<UserDays>) -> Res {
    require_user(&state, q.user_id).await?;
    Ok(PydanticJson(aggregate_activity(&state, doc! {"user_id": q.user_id}, q.days).await?))
}

/// `get_user`: lifetime token stats, never `rate_limited`.
pub fn single_summary(u: &User, tokens: &[ApiToken]) -> Value {
    let total: i64 = tokens.iter().map(|t| t.request_count).sum();
    let last = tokens.iter().filter_map(|t| t.last_used_at).max();
    user_summary(u, tokens.len() as i64, total, pyd_opt(&last), 0)
}

pub async fn get_user(State(state): State<AppState>, _: Superuser, Args(UserPath(id)): Args<UserPath>) -> Res {
    let user = require_user(&state, id).await?;
    let tokens = user_tokens(&state, id).await?;
    Ok(PydanticJson(single_summary(&user, &tokens)))
}

// --- tokens --------------------------------------------------------------------------

pub async fn revoke_token(State(state): State<AppState>, _: Superuser, Args(TokenPath(id)): Args<TokenPath>) -> Res {
    let c = coll(&state, API_TOKENS);
    let Some(mut token) = find_one::<ApiToken>(&c, doc! {"_id": id}).await? else {
        return Err(ApiError::not_found("Token not found"));
    };
    if !token.revoked {
        token.revoked = true;
        token.revoked_at = Some(docs::now());
        token.revoke_reason = Some("Revoked by administrator".into());
        docs::save(&c, token.id, docs::token_doc(&token)).await?;
    }
    Ok(PydanticJson(token_view(&token)))
}

// --- usage feed ----------------------------------------------------------------------

fn now_default() -> DateTime {
    docs::now()
}

/// `UsageEvent` as Beanie loads it.
#[derive(Debug, Clone, Deserialize)]
pub struct UsageRow {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub user_id: ObjectId,
    pub token_id: ObjectId,
    pub method: String,
    pub route: String,
    pub path: String,
    pub status_code: i64,
    pub duration_ms: f64,
    #[serde(default = "now_default")]
    pub created_at: DateTime,
}

/// `AdminUsageEvent`.
pub fn event_view(e: &UsageRow) -> Value {
    json!({
        "id": e.id.to_hex(),
        "user_id": e.user_id.to_hex(),
        "token_id": e.token_id.to_hex(),
        "method": e.method,
        "route": e.route,
        "path": e.path,
        "status_code": e.status_code,
        "duration_ms": e.duration_ms,
        "created_at": pyd(&e.created_at),
    })
}

/// `paginate_newest_first`'s filter (a bad cursor is a 400).
pub fn events_filter(q: &EventsQuery) -> Result<Document, ApiError> {
    let mut f = Document::new();
    if let Some(u) = q.user_id {
        f.insert("user_id", u);
    }
    if let Some(s) = q.status_code {
        f.insert("status_code", crate::auth::pyutil::bson_int(s));
    }
    if let Some(c) = &q.cursor {
        let after = ObjectId::parse_str(c).map_err(|_| ApiError::bad_request("Invalid cursor"))?;
        f.insert("_id", doc! {"$lt": after});
    }
    Ok(f)
}

/// `Page[AdminUsageEvent]` from up to `limit + 1` rows.
pub fn events_page(mut rows: Vec<UsageRow>, limit: usize) -> Value {
    let has_more = rows.len() > limit;
    rows.truncate(limit);
    let next = if has_more { rows.last().map(|e| e.id.to_hex()) } else { None };
    json!({"items": rows.iter().map(event_view).collect::<Vec<_>>(), "next_cursor": next, "has_more": has_more})
}

pub async fn list_events(State(state): State<AppState>, _: Superuser, Args(q): Args<EventsQuery>) -> Res {
    let filter = events_filter(&q)?;
    let docs: Vec<Document> = coll(&state, USAGE_COLLECTION)
        .find(filter)
        .sort(doc! {"_id": -1})
        .limit(q.limit + 1)
        .await
        .map_err(internal)?
        .try_collect()
        .await
        .map_err(internal)?;
    let rows = docs.into_iter().map(docs::load).collect::<Result<Vec<UsageRow>, _>>()?;
    Ok(PydanticJson(events_page(rows, q.limit as usize)))
}

// --- global activity -------------------------------------------------------------------

pub fn overview_pipelines(since: DateTime, top: i64) -> [Vec<Document>; 2] {
    let m = doc! {"created_at": {"$gte": since}};
    [
        vec![
            doc! {"$match": m.clone()},
            doc! {"$group": {
                "_id": Bson::Null,
                "count": {"$sum": 1},
                "error_count": {"$sum": error_cond()},
                "rate_limited": {"$sum": rate_limited_cond()},
                "avg_duration_ms": {"$avg": "$duration_ms"},
            }},
        ],
        vec![
            doc! {"$match": m},
            doc! {"$group": {
                "_id": "$user_id",
                "count": {"$sum": 1},
                "error_count": {"$sum": error_cond()},
                "rate_limited": {"$sum": rate_limited_cond()},
            }},
            doc! {"$sort": {"count": -1}},
            doc! {"$limit": crate::auth::pyutil::bson_int(top)},
        ],
    ]
}

/// `ActivityOverview`; `emails` maps a user id to its stored address.
pub fn overview(
    days: i64,
    totals: &[Document],
    top: &[Document],
    emails: &HashMap<ObjectId, String>,
) -> Result<Value, ApiError> {
    let (count, errors, rl, avg) = totals_head(totals)?;
    let mut users = Vec::new();
    for r in top {
        let email = match r.get("_id") {
            Some(Bson::ObjectId(o)) => emails.get(o).map(|e| docs::normalize_email(e)),
            _ => None,
        };
        let rl = if r.contains_key("rate_limited") { int_field(r, "rate_limited")? } else { 0 };
        users.push(json!({
            "user_id": id_key(r.get("_id")),
            "email": email,
            "count": int_field(r, "count")?,
            "error_count": int_field(r, "error_count")?,
            "rate_limited": rl,
        }));
    }
    Ok(json!({
        "window_days": days,
        "total_requests": count,
        "error_count": errors,
        "rate_limited": rl,
        "avg_duration_ms": avg,
        "top_users": users,
    }))
}

pub async fn activity_overview(State(state): State<AppState>, _: Superuser, Args(q): Args<OverviewQuery>) -> Res {
    let [a, b] = overview_pipelines(since(q.days), q.top);
    let totals = aggregate(&state, USAGE_COLLECTION, a).await?;
    let top = aggregate(&state, USAGE_COLLECTION, b).await?;
    let ids: Vec<Bson> = top.iter().map(|r| r.get("_id").cloned().unwrap_or(Bson::Null)).collect();
    let mut emails = HashMap::new();
    if !ids.is_empty() {
        let users: Vec<User> = find_all(&coll(&state, USERS), doc! {"_id": {"$in": ids}}, None).await?;
        emails = users.into_iter().map(|u| (u.id, u.email)).collect();
    }
    Ok(PydanticJson(overview(q.days, &totals, &top, &emails)?))
}

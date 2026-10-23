//! The read routes: `/v1/codexes/*` (pydantic models, `codexes:read`, 5x
//! budget, behind the Codexes gate) and the tokenless `/site/codexes/*`
//! proxies (raw dicts, `json.dumps`, per-route Cache-Control).

use axum::{
    extract::{FromRequestParts, State},
    http::{HeaderValue, StatusCode, header, request::Parts},
    response::{IntoResponse, Response},
};
use deadpool_postgres::Pool;
use kiwi_core::codex::links::ALL_RELATIONS;
use serde_json::{Map, Value, json};

use super::{
    BRANCHES, CodexScope, DEFAULT_BRANCH, branch_message, crafting, internal, is_type,
    pg::{Cell, Row},
    segment,
    store::{self, Filter},
    type_message,
};
use crate::{
    dt,
    error::{ApiError, ApiResult},
    features::{Codexes, RequireFeature},
    market::SiteFeature,
    pyjson::{PyJson, PydanticJson},
    state::AppState,
    validation::{FromParams, Params, Query},
};

fn pool(state: &AppState) -> Option<&Pool> {
    state.inner.db.pg.as_ref()
}

fn check_branch(branch: &str) -> ApiResult<()> {
    if BRANCHES.contains(&branch) { Ok(()) } else { Err(ApiError::not_found(branch_message(branch))) }
}

fn check_type(t: &str) -> ApiResult<()> {
    if is_type(t) { Ok(()) } else { Err(ApiError::not_found(type_message(t))) }
}

fn site_branch(branch: &str) -> ApiResult<()> {
    if BRANCHES.contains(&branch) {
        Ok(())
    } else {
        Err(ApiError::http(StatusCode::NOT_FOUND, Some(&branch_message(branch))))
    }
}

fn site_type(t: &str) -> ApiResult<()> {
    if is_type(t) { Ok(()) } else { Err(ApiError::http(StatusCode::NOT_FOUND, Some(&type_message(t)))) }
}

fn opt(v: &Option<String>) -> Value {
    v.as_ref().map_or(Value::Null, |s| json!(s))
}

fn cell_float(c: &Cell) -> Value {
    match c {
        Cell::Float(f) => serde_json::Number::from_f64(*f).map_or(Value::Null, Value::Number),
        Cell::Int(i) => json!(*i as f64),
        _ => Value::Null,
    }
}

fn cell_or(c: &Cell, default: Value) -> Value {
    if c.is_null() { default } else { c.json() }
}

fn truthy_text(c: &Cell) -> String {
    c.str().unwrap_or_default().to_owned()
}

// --- shared reads (read.py: empty without Postgres) ------------------------------------

async fn query_entries(
    state: &AppState,
    branch: &str,
    f: &Filter,
    sort: &str,
    limit: i64,
    offset: i64,
) -> ApiResult<(Vec<Row>, i64)> {
    let Some(p) = pool(state) else { return Ok((Vec::new(), 0)) };
    store::query_entries(p, branch, f, sort, limit, offset).await.map_err(internal)
}

macro_rules! read_or_empty {
    ($state:expr, $call:expr) => {
        match pool($state) {
            None => Ok(Default::default()),
            Some(p) => $call(p).await.map_err(internal),
        }
    };
}

/// `_codex_out`: the pydantic `CodexEntryOut`.
pub(crate) fn entry_out(r: &Row) -> Value {
    let indexed = match r.get("indexed_at") {
        Cell::Ts(t) => json!(dt::pydantic(t)),
        other => other.json(),
    };
    json!({
        "type": truthy_text(r.get("codex_type")),
        "path": truthy_text(r.get("path")),
        "name": truthy_text(r.get("name")),
        "category": truthy_text(r.get("category")),
        "description": truthy_text(r.get("description")),
        "tradable": r.get("tradable").json(),
        "mastery": r.get("mastery").json(),
        "mastery_geode": r.get("mastery_geode").json(),
        "power_rank": r.get("power_rank").json(),
        "blueprint": r.get("blueprint").json(),
        "data": cell_or(r.get("data"), json!({})),
        "indexed_at": indexed,
        "stat_value": cell_float(r.get("stat_value")),
        "stat_percent": r.get("stat_percent").json(),
    })
}

/// `_codex_row`: the record as a dict, `indexed_at` in `isoformat()`.
pub(crate) fn site_row(r: &Row) -> Value {
    Value::Object(r.to_map())
}

fn cache(max_age: u32) -> [(header::HeaderName, HeaderValue); 1] {
    [(header::CACHE_CONTROL, HeaderValue::from_str(&format!("public, max-age={max_age}")).expect("ascii"))]
}

fn site(body: ApiResult<Value>, max_age: u32) -> Response {
    match body {
        Ok(v) => (cache(max_age), PyJson(v)).into_response(),
        Err(e) => e.into_response(),
    }
}

fn v1(ctx: CodexScope, body: ApiResult<Value>) -> Response {
    match body {
        Ok(v) => (ctx.0.rate_limit, PydanticJson(v)).into_response(),
        Err(e) => e.into_response(),
    }
}

// --- query models -------------------------------------------------------------------

pub struct BranchQ {
    pub branch: String,
}

impl FromParams for BranchQ {
    fn from_params(p: &mut Params<'_>) -> Self {
        BranchQ { branch: p.query("branch").str().default(DEFAULT_BRANCH.into()) }
    }
}

/// The filter/sort/page query `/search` and `/{codex_type}` share.
pub struct SearchQ<const SITE: bool> {
    pub branch: String,
    pub q: Option<String>,
    pub codex_type: Option<String>,
    pub category: Option<String>,
    pub tradable: Option<bool>,
    pub stat: Option<String>,
    pub ability: Option<String>,
    pub sort: String,
    pub limit: i64,
    pub offset: i64,
}

fn search_q<const SITE: bool>(p: &mut Params<'_>, q_name: &str, with_type: bool) -> SearchQ<SITE> {
    let branch = p.query("branch").str().default(DEFAULT_BRANCH.into());
    let q = p.query(q_name).str().optional();
    let codex_type = if with_type { p.query("type").str().optional() } else { None };
    SearchQ {
        branch,
        q,
        codex_type,
        category: p.query("category").str().optional(),
        tradable: p.query("tradable").bool().optional(),
        stat: p.query("stat").str().optional(),
        ability: p.query("ability").str().optional(),
        sort: p.query("sort").str().default("name".into()),
        limit: p.query("limit").int().ge(1).le(200).default(if SITE { 60 } else { 50 }),
        offset: p.query("offset").int().ge(0).default(0),
    }
}

/// `/search` (v1 and site): `q` plus an optional `type`.
pub struct CrossQ<const SITE: bool>(pub SearchQ<SITE>);

impl<const SITE: bool> FromParams for CrossQ<SITE> {
    fn from_params(p: &mut Params<'_>) -> Self {
        CrossQ(search_q(p, "q", true))
    }
}

/// `/{codex_type}`: `search`, the type comes from the path.
pub struct TypeQ(pub SearchQ<false>);

impl FromParams for TypeQ {
    fn from_params(p: &mut Params<'_>) -> Self {
        TypeQ(search_q(p, "search", false))
    }
}

/// `_stat_key`: a bare name gets `$Stat_`.
fn stat_key(stat: &Option<String>) -> Option<String> {
    let s = stat.as_deref().filter(|s| !s.is_empty())?;
    Some(if s.starts_with('$') { s.to_owned() } else { format!("$Stat_{s}") })
}

fn check_sort_v1(sort: &str) -> ApiResult<()> {
    if store::is_sort(sort) {
        Ok(())
    } else {
        Err(ApiError::bad_request(format!("Invalid sort '{sort}' (allowed: {})", store::sort_keys())))
    }
}

// --- /v1/codexes ----------------------------------------------------------------------

pub async fn v1_types(
    State(state): State<AppState>,
    _: RequireFeature<Codexes>,
    ctx: CodexScope,
    Query(q): Query<BranchQ>,
) -> Response {
    let body = async {
        check_branch(&q.branch)?;
        let rows: Vec<(String, i64)> = read_or_empty!(&state, |p| store::type_counts(p, &q.branch))?;
        let items: Vec<Value> = rows.iter().map(|(t, c)| json!({"type": t, "count": c})).collect();
        Ok(json!({"branch": q.branch, "items": items, "count": rows.len()}))
    };
    v1(ctx, body.await)
}

async fn search_body(
    state: &AppState,
    q: &SearchQ<false>,
    codex_type: Option<String>,
    query_key: Option<&str>,
) -> ApiResult<Value> {
    check_branch(&q.branch)?;
    if let Some(t) = &codex_type {
        check_type(t)?;
    }
    check_sort_v1(&q.sort)?;
    let f = Filter {
        codex_type: codex_type.clone(),
        search: q.q.clone(),
        category: q.category.clone(),
        tradable: q.tradable,
        stat: stat_key(&q.stat),
        ability: q.ability.clone(),
    };
    let (rows, total) = query_entries(state, &q.branch, &f, &q.sort, q.limit, q.offset).await?;
    let items: Vec<Value> = rows.iter().map(entry_out).collect();
    let mut out = Map::new();
    out.insert("branch".into(), json!(q.branch));
    out.insert("type".into(), opt(&codex_type));
    if let Some(k) = query_key {
        out.insert(k.into(), opt(&q.q));
    }
    out.insert("count".into(), json!(items.len()));
    out.insert("items".into(), Value::Array(items));
    out.insert("total".into(), json!(total));
    let mut ordered = Map::new();
    for k in ["branch", "type", "query", "items", "count", "total"] {
        if let Some(v) = out.remove(k) {
            ordered.insert(k.into(), v);
        }
    }
    Ok(Value::Object(ordered))
}

pub async fn v1_search(
    State(state): State<AppState>,
    _: RequireFeature<Codexes>,
    ctx: CodexScope,
    Query(CrossQ(q)): Query<CrossQ<false>>,
) -> Response {
    let q = SearchQ::<false> { ..q };
    let t = q.codex_type.clone();
    v1(ctx, search_body(&state, &q, t, Some("query")).await)
}

pub struct LinksQ {
    branch: String,
    path: String,
    direction: String,
    rel: Option<String>,
    limit: i64,
}

impl FromParams for LinksQ {
    fn from_params(p: &mut Params<'_>) -> Self {
        LinksQ {
            branch: p.query("branch").str().default(DEFAULT_BRANCH.into()),
            path: p.query("path").str().required(),
            direction: p.query("direction").str().default("out".into()),
            rel: p.query("rel").str().optional(),
            limit: p.query("limit").int().ge(1).le(500).default(200),
        }
    }
}

pub async fn v1_links(
    State(state): State<AppState>,
    _: RequireFeature<Codexes>,
    ctx: CodexScope,
    Query(q): Query<LinksQ>,
) -> Response {
    let body = async {
        check_branch(&q.branch)?;
        // `ErrorCode.invalid_request` does not exist, so Python raises AttributeError here.
        if q.direction != "out" && q.direction != "in" {
            return Err(ApiError::internal());
        }
        if q.rel.as_ref().is_some_and(|r| !ALL_RELATIONS.contains(&r.as_str())) {
            return Err(ApiError::internal());
        }
        let outward = q.direction != "in";
        let rows: Vec<Row> =
            read_or_empty!(&state, |p| store::links_for(p, &q.branch, &q.path, outward, q.rel.as_deref(), q.limit))?;
        let items: Vec<Value> = rows
            .iter()
            .map(|r| {
                json!({
                    "rel": truthy_text(r.get("rel")),
                    "path": truthy_text(r.get("path")),
                    "type": r.get("codex_type").json(),
                    "name": truthy_text(r.get("name")),
                    "category": truthy_text(r.get("category")),
                    "blueprint": r.get("blueprint").json(),
                    "qty": cell_float(r.get("qty")),
                    "data": data_or_empty(r.get("data")),
                })
            })
            .collect();
        Ok(json!({"branch": q.branch, "path": q.path, "direction": q.direction, "count": items.len(), "items": items}))
            .map(reorder(&["branch", "path", "direction", "items", "count"]))
    };
    v1(ctx, body.await)
}

fn data_or_empty(c: &Cell) -> Value {
    match c {
        Cell::Json(v) if crate::features::py_truthy(v) => v.clone(),
        _ => json!({}),
    }
}

fn reorder(keys: &'static [&'static str]) -> impl Fn(Value) -> Value {
    move |v| {
        let Value::Object(mut m) = v else { return v };
        let mut out = Map::new();
        for k in keys {
            if let Some(x) = m.remove(*k) {
                out.insert((*k).into(), x);
            }
        }
        out.extend(m);
        Value::Object(out)
    }
}

pub struct StatsQ {
    branch: String,
    stat: String,
    codex_type: Option<String>,
    limit: i64,
    offset: i64,
}

impl FromParams for StatsQ {
    fn from_params(p: &mut Params<'_>) -> Self {
        StatsQ {
            branch: p.query("branch").str().default(DEFAULT_BRANCH.into()),
            stat: p.query("stat").str().required(),
            codex_type: p.query("type").str().optional(),
            limit: p.query("limit").int().ge(1).le(200).default(50),
            offset: p.query("offset").int().ge(0).default(0),
        }
    }
}

pub async fn v1_stats(
    State(state): State<AppState>,
    _: RequireFeature<Codexes>,
    ctx: CodexScope,
    Query(q): Query<StatsQ>,
) -> Response {
    let body = async {
        check_branch(&q.branch)?;
        if let Some(t) = &q.codex_type {
            check_type(t)?;
        }
        let key = if q.stat.starts_with('$') { q.stat.clone() } else { format!("$Stat_{}", q.stat) };
        let (rows, total): (Vec<Row>, i64) = read_or_empty!(&state, |p| store::entries_with_stat(
            p,
            &q.branch,
            &key,
            q.codex_type.as_deref(),
            q.limit,
            q.offset
        ))?;
        let items: Vec<Value> = rows
            .iter()
            .map(|r| {
                json!({
                    "path": truthy_text(r.get("path")),
                    "type": truthy_text(r.get("codex_type")),
                    "name": truthy_text(r.get("name")),
                    "category": truthy_text(r.get("category")),
                    "blueprint": r.get("blueprint").json(),
                    "stat": truthy_text(r.get("stat_key")),
                    "stat_name": truthy_text(r.get("stat_name")),
                    "value": cell_float(r.get("value")),
                    "is_percent": matches!(r.get("is_percent"), Cell::Bool(true)),
                    "slot": r.get("slot_key").json(),
                })
            })
            .collect();
        Ok(json!({"branch": q.branch, "stat": key, "count": items.len(), "items": items, "total": total}))
            .map(reorder(&["branch", "stat", "items", "count", "total"]))
    };
    v1(ctx, body.await)
}

pub struct RequirementsQ {
    branch: String,
    collection: String,
}

impl FromParams for RequirementsQ {
    fn from_params(p: &mut Params<'_>) -> Self {
        RequirementsQ {
            branch: p.query("branch").str().default(DEFAULT_BRANCH.into()),
            collection: p.query("collection").str().required(),
        }
    }
}

fn requirement_out(r: &Row) -> Value {
    json!({
        "rank": r.get("rank").json(),
        "rank_name": truthy_text(r.get("rank_name")),
        "collection": truthy_text(r.get("collection")),
        "badge_id": truthy_text(r.get("badge_id")),
        "completion_kind": truthy_text(r.get("completion_kind")),
        "requirement_key": truthy_text(r.get("requirement_key")),
        "label": truthy_text(r.get("label")),
        "amount": r.get("amount").json(),
        "difficulty": r.get("difficulty").json(),
        "status": truthy_text(r.get("status")),
        "context": cell_or(r.get("context"), json!({})),
    })
}

pub async fn v1_requirements(
    State(state): State<AppState>,
    _: RequireFeature<Codexes>,
    ctx: CodexScope,
    Query(q): Query<RequirementsQ>,
) -> Response {
    let body = async {
        check_branch(&q.branch)?;
        let rows: Vec<Row> = read_or_empty!(&state, |p| store::requirements_for(p, &q.branch, &q.collection))?;
        let items: Vec<Value> = rows.iter().map(requirement_out).collect();
        Ok(json!({"branch": q.branch, "collection": q.collection, "count": items.len(), "items": items}))
            .map(reorder(&["branch", "collection", "items", "count"]))
    };
    v1(ctx, body.await)
}

fn system_info(r: &Row) -> Value {
    json!({"system_kind": truthy_text(r.get("system_kind")), "system_key": truthy_text(r.get("system_key")), "nodes": r.get("nodes").json()})
}

pub async fn v1_upgrades(
    State(state): State<AppState>,
    _: RequireFeature<Codexes>,
    ctx: CodexScope,
    Query(q): Query<BranchQ>,
) -> Response {
    let body = async {
        check_branch(&q.branch)?;
        let rows: Vec<Row> = read_or_empty!(&state, |p| store::upgrade_systems(p, &q.branch))?;
        let items: Vec<Value> = rows.iter().map(system_info).collect();
        Ok(json!({"branch": q.branch, "count": items.len(), "items": items}))
            .map(reorder(&["branch", "items", "count"]))
    };
    v1(ctx, body.await)
}

/// The decoded `{system_key}`.
pub struct SystemKey(pub String);

impl<S: Send + Sync> FromRequestParts<S> for SystemKey {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        Ok(SystemKey(segment(parts, 4)))
    }
}

/// The decoded `{codex_type}`.
pub struct TypeSeg(pub String);

impl<S: Send + Sync> FromRequestParts<S> for TypeSeg {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        Ok(TypeSeg(segment(parts, 3)))
    }
}

pub async fn v1_upgrade_system(
    State(state): State<AppState>,
    _: RequireFeature<Codexes>,
    SystemKey(key): SystemKey,
    ctx: CodexScope,
    Query(q): Query<BranchQ>,
) -> Response {
    let body = async {
        check_branch(&q.branch)?;
        let rows: Vec<Row> = read_or_empty!(&state, |p| store::upgrade_system(p, &q.branch, &key))?;
        if rows.is_empty() {
            return Err(ApiError::not_found(format!("No upgrade system '{key}'")));
        }
        let items: Vec<Value> = rows
            .iter()
            .map(|r| {
                json!({
                    "system_kind": truthy_text(r.get("system_kind")),
                    "system_key": truthy_text(r.get("system_key")),
                    "node_key": truthy_text(r.get("node_key")),
                    "rank": r.get("rank").json(),
                    "costs": cell_or(r.get("costs"), json!([])),
                    "requires": cell_or(r.get("requires"), json!([])),
                    "effects": {},
                })
            })
            .collect();
        Ok(json!({"branch": q.branch, "system_key": key, "count": items.len(), "items": items})).map(reorder(&[
            "branch",
            "system_key",
            "items",
            "count",
        ]))
    };
    v1(ctx, body.await)
}

pub async fn v1_entries(
    State(state): State<AppState>,
    _: RequireFeature<Codexes>,
    TypeSeg(t): TypeSeg,
    ctx: CodexScope,
    Query(TypeQ(q)): Query<TypeQ>,
) -> Response {
    v1(ctx, search_body(&state, &q, Some(t), None).await)
}

async fn v1_type_list(state: &AppState, branch: &str, t: &str, kind: &str) -> ApiResult<Value> {
    check_branch(branch)?;
    check_type(t)?;
    let items: Vec<Value> = match kind {
        "categories" => {
            let rows: Vec<(String, i64)> = read_or_empty!(state, |p| store::list_categories(p, branch, Some(t)))?;
            rows.iter().map(|(c, n)| json!({"category": c, "count": n})).collect()
        }
        "stat-keys" => {
            let rows: Vec<Row> = read_or_empty!(state, |p| store::stat_keys(p, branch, Some(t)))?;
            rows.iter()
                .map(|r| json!({"stat": truthy_text(r.get("stat")), "stat_name": truthy_text(r.get("stat_name")), "count": r.get("count").json()}))
                .collect()
        }
        _ => {
            let rows: Vec<Row> = read_or_empty!(state, |p| store::ability_refs(p, branch, Some(t)))?;
            rows.iter()
                .map(|r| {
                    json!({
                        "ref": truthy_text(r.get("ref")),
                        "name": truthy_text(r.get("name")),
                        "description": truthy_text(r.get("description")),
                        "count": r.get("count").json(),
                    })
                })
                .collect()
        }
    };
    Ok(json!({"branch": branch, "type": t, "count": items.len(), "items": items}))
        .map(reorder(&["branch", "type", "items", "count"]))
}

pub async fn v1_categories(
    State(state): State<AppState>,
    _: RequireFeature<Codexes>,
    TypeSeg(t): TypeSeg,
    ctx: CodexScope,
    Query(q): Query<BranchQ>,
) -> Response {
    v1(ctx, v1_type_list(&state, &q.branch, &t, "categories").await)
}

pub async fn v1_stat_keys(
    State(state): State<AppState>,
    _: RequireFeature<Codexes>,
    TypeSeg(t): TypeSeg,
    ctx: CodexScope,
    Query(q): Query<BranchQ>,
) -> Response {
    v1(ctx, v1_type_list(&state, &q.branch, &t, "stat-keys").await)
}

pub async fn v1_abilities(
    State(state): State<AppState>,
    _: RequireFeature<Codexes>,
    TypeSeg(t): TypeSeg,
    ctx: CodexScope,
    Query(q): Query<BranchQ>,
) -> Response {
    v1(ctx, v1_type_list(&state, &q.branch, &t, "abilities").await)
}

pub struct EntryQ {
    path: String,
    branch: String,
}

impl FromParams for EntryQ {
    fn from_params(p: &mut Params<'_>) -> Self {
        EntryQ {
            path: p.query("path").str().required(),
            branch: p.query("branch").str().default(DEFAULT_BRANCH.into()),
        }
    }
}

pub async fn v1_entry(
    State(state): State<AppState>,
    _: RequireFeature<Codexes>,
    TypeSeg(t): TypeSeg,
    ctx: CodexScope,
    Query(q): Query<EntryQ>,
) -> Response {
    let body = async {
        check_branch(&q.branch)?;
        check_type(&t)?;
        let row: Option<Row> = read_or_empty!(&state, |p| store::get_entry(p, &q.branch, &t, &q.path))?;
        let row = row.ok_or_else(|| ApiError::not_found(format!("No {t} entry '{}'", q.path)))?;
        Ok(entry_out(&row))
    };
    v1(ctx, body.await)
}

// --- /site/codexes ------------------------------------------------------------------

pub struct SiteTypeQ {
    branch: String,
    codex_type: Option<String>,
}

impl FromParams for SiteTypeQ {
    fn from_params(p: &mut Params<'_>) -> Self {
        SiteTypeQ {
            branch: p.query("branch").str().default(DEFAULT_BRANCH.into()),
            codex_type: p.query("type").str().optional(),
        }
    }
}

pub async fn site_types(State(state): State<AppState>, _: SiteFeature<Codexes>, Query(q): Query<BranchQ>) -> Response {
    let body = async {
        site_branch(&q.branch)?;
        let rows: Vec<(String, i64)> = read_or_empty!(&state, |p| store::type_counts(p, &q.branch))?;
        let items: Vec<Value> = rows.iter().map(|(t, c)| json!({"type": t, "count": c})).collect();
        Ok(json!({"branch": q.branch, "count": items.len(), "items": items}))
            .map(reorder(&["branch", "items", "count"]))
    };
    site(body.await, 60)
}

async fn site_type_list(state: &AppState, q: &SiteTypeQ, kind: &str) -> ApiResult<Value> {
    site_branch(&q.branch)?;
    if let Some(t) = &q.codex_type {
        site_type(t)?;
    }
    let t = q.codex_type.as_deref();
    let items: Vec<Value> = match kind {
        "categories" => {
            let rows: Vec<(String, i64)> = read_or_empty!(state, |p| store::list_categories(p, &q.branch, t))?;
            rows.iter().map(|(c, n)| json!({"category": c, "count": n})).collect()
        }
        "stat-keys" => {
            let rows: Vec<Row> = read_or_empty!(state, |p| store::stat_keys(p, &q.branch, t))?;
            rows.iter().map(site_row).collect()
        }
        _ => {
            let rows: Vec<Row> = read_or_empty!(state, |p| store::ability_refs(p, &q.branch, t))?;
            rows.iter().map(site_row).collect()
        }
    };
    Ok(json!({"branch": q.branch, "type": opt(&q.codex_type), "count": items.len(), "items": items}))
        .map(reorder(&["branch", "type", "items", "count"]))
}

pub async fn site_categories(
    State(state): State<AppState>,
    _: SiteFeature<Codexes>,
    Query(q): Query<SiteTypeQ>,
) -> Response {
    site(site_type_list(&state, &q, "categories").await, 60)
}

pub async fn site_stat_keys(
    State(state): State<AppState>,
    _: SiteFeature<Codexes>,
    Query(q): Query<SiteTypeQ>,
) -> Response {
    site(site_type_list(&state, &q, "stat-keys").await, 60)
}

pub async fn site_abilities(
    State(state): State<AppState>,
    _: SiteFeature<Codexes>,
    Query(q): Query<SiteTypeQ>,
) -> Response {
    site(site_type_list(&state, &q, "abilities").await, 60)
}

pub async fn site_search(
    State(state): State<AppState>,
    _: SiteFeature<Codexes>,
    Query(CrossQ(q)): Query<CrossQ<true>>,
) -> Response {
    let body = async {
        site_branch(&q.branch)?;
        if let Some(t) = &q.codex_type {
            site_type(t)?;
        }
        if !store::is_sort(&q.sort) {
            return Err(ApiError::http(StatusCode::BAD_REQUEST, Some(&format!("Invalid sort '{}'", q.sort))));
        }
        let f = Filter {
            codex_type: q.codex_type.clone(),
            search: q.q.clone(),
            category: q.category.clone(),
            tradable: q.tradable,
            stat: q.stat.clone(),
            ability: q.ability.clone(),
        };
        let (rows, total) = query_entries(&state, &q.branch, &f, &q.sort, q.limit, q.offset).await?;
        let items: Vec<Value> = rows.iter().map(site_row).collect();
        Ok(json!({"branch": q.branch, "type": opt(&q.codex_type), "query": opt(&q.q), "count": items.len(), "items": items, "total": total}))
            .map(reorder(&["branch", "type", "query", "items", "count", "total"]))
    };
    site(body.await, 30)
}

pub struct SiteEntryQ {
    codex_type: String,
    path: String,
    branch: String,
}

impl FromParams for SiteEntryQ {
    fn from_params(p: &mut Params<'_>) -> Self {
        SiteEntryQ {
            codex_type: p.query("type").str().required(),
            path: p.query("path").str().required(),
            branch: p.query("branch").str().default(DEFAULT_BRANCH.into()),
        }
    }
}

pub async fn site_entry(
    State(state): State<AppState>,
    _: SiteFeature<Codexes>,
    Query(q): Query<SiteEntryQ>,
) -> Response {
    let body = async {
        site_branch(&q.branch)?;
        site_type(&q.codex_type)?;
        let row: Option<Row> = read_or_empty!(&state, |p| store::get_entry(p, &q.branch, &q.codex_type, &q.path))?;
        match row {
            Some(r) => Ok(site_row(&r)),
            None => {
                Err(ApiError::http(StatusCode::NOT_FOUND, Some(&format!("No {} entry '{}'", q.codex_type, q.path))))
            }
        }
    };
    site(body.await, 60)
}

pub struct PathQ {
    path: String,
    branch: String,
}

impl FromParams for PathQ {
    fn from_params(p: &mut Params<'_>) -> Self {
        PathQ { path: p.query("path").str().required(), branch: p.query("branch").str().default(DEFAULT_BRANCH.into()) }
    }
}

fn related_rows(rows: &[Row], rel: &str) -> Vec<Value> {
    rows.iter()
        .filter(|r| r.get("rel").str() == Some(rel))
        .map(|r| {
            json!({
                "path": truthy_text(r.get("path")),
                "type": r.get("codex_type").json(),
                "name": truthy_text(r.get("name")),
                "qty": r.get("qty").json(),
                "blueprint": r.get("blueprint").json(),
                "data": data_or_empty(r.get("data")),
            })
        })
        .collect()
}

pub async fn site_related(State(state): State<AppState>, _: SiteFeature<Codexes>, Query(q): Query<PathQ>) -> Response {
    let body = async {
        site_branch(&q.branch)?;
        let mut out = Map::new();
        out.insert("path".into(), json!(q.path));
        let outgoing: Vec<Row> = read_or_empty!(&state, |p| store::links_for(p, &q.branch, &q.path, true, None, 300))?;
        let incoming: Vec<Row> = read_or_empty!(&state, |p| store::links_for(p, &q.branch, &q.path, false, None, 300))?;
        for rel in ["crafts", "ingredient", "craftable_at", "unlocks", "member_of"] {
            let found = related_rows(&outgoing, rel);
            if !found.is_empty() {
                out.insert(rel.into(), Value::Array(found));
            }
        }
        for (key, rel) in [
            ("made_by", "crafts"),
            ("used_in", "ingredient"),
            ("unlocked_by", "unlocks"),
            ("upgrade_cost_of", "upgrade_cost"),
        ] {
            let found = related_rows(&incoming, rel);
            if !found.is_empty() {
                out.insert(key.into(), Value::Array(found));
            }
        }
        let rel_path = match q.path.strip_prefix("prefabs/") {
            Some(r) => r.strip_suffix(".binfab").unwrap_or(r).to_owned(),
            None => q.path.clone(),
        };
        let reqs: Vec<Row> = read_or_empty!(&state, |p| store::requirements_for(p, &q.branch, &rel_path))?;
        if !reqs.is_empty() {
            out.insert("requirements".into(), Value::Array(reqs.iter().map(site_row).collect()));
        }
        Ok(Value::Object(out))
    };
    site(body.await, 60)
}

pub struct UpgradesQ {
    system: Option<String>,
    branch: String,
}

impl FromParams for UpgradesQ {
    fn from_params(p: &mut Params<'_>) -> Self {
        UpgradesQ {
            system: p.query("system").str().optional(),
            branch: p.query("branch").str().default(DEFAULT_BRANCH.into()),
        }
    }
}

pub async fn site_upgrades(
    State(state): State<AppState>,
    _: SiteFeature<Codexes>,
    Query(q): Query<UpgradesQ>,
) -> Response {
    let body = async {
        site_branch(&q.branch)?;
        if let Some(system) = q.system.as_deref().filter(|s| !s.is_empty()) {
            let nodes: Vec<Row> = read_or_empty!(&state, |p| store::upgrade_system(p, &q.branch, system))?;
            if nodes.is_empty() {
                return Err(ApiError::http(StatusCode::NOT_FOUND, Some(&format!("No upgrade system '{system}'"))));
            }
            return Ok(json!({"system_key": system, "items": nodes.iter().map(site_row).collect::<Vec<_>>()}));
        }
        let systems: Vec<Row> = read_or_empty!(&state, |p| store::upgrade_systems(p, &q.branch))?;
        Ok(json!({"items": systems.iter().map(site_row).collect::<Vec<_>>()}))
    };
    site(body.await, 300)
}

pub async fn site_crafting(State(state): State<AppState>, _: SiteFeature<Codexes>, Query(q): Query<PathQ>) -> Response {
    let body = async {
        site_branch(&q.branch)?;
        match crafting::build_tree(&state, &q.branch, &q.path).await? {
            Some(tree) => Ok(tree),
            None => Err(ApiError::http(
                StatusCode::NOT_FOUND,
                Some(&format!("No recipe '{}' on branch '{}'", q.path, q.branch)),
            )),
        }
    };
    site(body.await, 30)
}

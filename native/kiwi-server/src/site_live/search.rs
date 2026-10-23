//! `GET /site/search` (`app/site/search.py`): pages, codex entries, players
//! and mods for one query, grouped by subject.

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::{Map, Value, json};

use super::{
    cached,
    flags::{Flags, SiteGate, with_calc_flags},
    search_index::{self as index, SUBJECT_MODPACKS, SUBJECT_MODS, SUBJECT_ORDER, SUBJECT_PAGES, SUBJECT_PLAYERS},
};
use crate::{
    codexes::store::{Filter, query_entries},
    error::ApiError,
    middleware::pyurl::quote,
    state::AppState,
    validation::{FromParams, Params, Query},
};

pub const MIN_QUERY: usize = 2;
pub const PREVIEW_PER_SUBJECT: i64 = 4;
pub const PAGE_SIZE: i64 = 30;
pub const CODEX_BRANCHES: [&str; 2] = ["live-us", "pts"];

pub struct SearchQuery {
    pub q: String,
    pub subject: Option<String>,
    pub limit: i64,
    pub offset: i64,
    pub branch: String,
}

impl FromParams for SearchQuery {
    fn from_params(p: &mut Params<'_>) -> Self {
        SearchQuery {
            q: p.query("q").str().default(String::new()),
            subject: p.query("subject").str().optional(),
            limit: p.query("limit").int().ge(1).le(100).default(PAGE_SIZE),
            offset: p.query("offset").int().ge(0).default(0),
            branch: p.query("branch").str().default("live-us".into()),
        }
    }
}

/// `_SUBJECT_FLAG`.
fn subject_flag(subject: &str) -> Option<&'static str> {
    match subject {
        SUBJECT_PAGES => None,
        SUBJECT_PLAYERS => Some("leaderboards_enabled"),
        SUBJECT_MODS | SUBJECT_MODPACKS => Some("mods_hub_enabled"),
        _ => Some("codexes_enabled"),
    }
}

/// `available_subjects(flags)`: an unknown flag counts as off.
pub fn available_subjects(flags: &Flags) -> Vec<&'static str> {
    SUBJECT_ORDER.iter().copied().filter(|s| subject_flag(s).is_none_or(|f| flags.get_or(f, false))).collect()
}

fn enabled(flags: &Flags) -> impl Fn(&str) -> Option<bool> + '_ {
    move |k| flags.0.iter().find(|(name, _)| name == k).map(|(_, v)| *v)
}

/// `_pages`.
pub fn pages(query: &str, flags: &Flags, limit: i64, offset: i64) -> (Vec<Value>, i64) {
    let rows: Vec<Value> = index::search_destinations(query, &enabled(flags), 200).iter().map(|d| d.row()).collect();
    let total = rows.len() as i64;
    (window(rows, offset, limit), total)
}

fn window<T>(rows: Vec<T>, offset: i64, limit: i64) -> Vec<T> {
    rows.into_iter().skip(offset.max(0) as usize).take(limit.max(0) as usize).collect()
}

fn truthy_str(v: Option<&Value>) -> Option<String> {
    v.and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_owned)
}

/// One codex row as `_codex` reshapes it.
pub fn codex_row(r: &Map<String, Value>) -> Value {
    let name = truthy_str(r.get("name")).unwrap_or_default();
    let kind = truthy_str(r.get("codex_type")).unwrap_or_default();
    let image =
        truthy_str(r.get("blueprint")).map(|bp| format!("/site/codexes/render?dim=64&blueprint={}", quote(&bp, "")));
    json!({
        "name": name,
        "path": format!("/codexes#type={}&q={}", quote(&kind, ""), quote(&name, "")),
        "detail": truthy_str(r.get("category")).unwrap_or_default(),
        "kind": kind,
        "image": image,
    })
}

async fn codex(
    state: &AppState,
    query: &str,
    subject: &str,
    branch: &str,
    limit: i64,
    offset: i64,
) -> anyhow::Result<(Vec<Value>, i64)> {
    let types = index::codex_types(subject);
    let Some(pool) = state.inner.db.pg.as_ref() else { return Ok((Vec::new(), 0)) };
    let f = Filter {
        codex_type: (types.len() == 1).then(|| types[0].to_owned()),
        search: Some(query.to_owned()),
        ..Filter::default()
    };
    let (rows, total) = query_entries(pool, branch, &f, "name", limit, offset).await?;
    let rows: Vec<Map<String, Value>> = rows.iter().map(|r| r.to_map()).collect();
    let rows = rows
        .iter()
        .filter(|r| types.len() <= 1 || r.get("codex_type").and_then(Value::as_str).is_some_and(|t| types.contains(&t)))
        .map(codex_row)
        .collect();
    Ok((rows, total))
}

/// `pg_store.search_players(query, limit=limit)`: prefix matches first.
pub async fn search_players(state: &AppState, query: &str, limit: i64) -> anyhow::Result<(Vec<String>, i64)> {
    let term = kiwi_core::pystr::strip(query);
    if term.chars().count() < 2 {
        return Ok((Vec::new(), 0));
    }
    let like = term.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_").to_lowercase();
    let pool = state.inner.db.pg.as_ref().ok_or_else(|| anyhow::anyhow!("POSTGRES_DSN not configured"))?;
    let client = pool.get().await?;
    const IS_A_PLAYER: &str =
        "EXISTS (SELECT 1 FROM player_board_agg a WHERE a.player_id = p.id AND NOT (a.board_uuid = ANY($4::int[]))) ";
    let sql = format!(
        "SELECT name FROM (  (SELECT name, name_lower, 0 AS tier FROM player p     WHERE name_lower LIKE $1 || '%' ESCAPE '\\' AND {IS_A_PLAYER}    ORDER BY name_lower LIMIT $3)  UNION ALL   (SELECT name, name_lower, 1 AS tier FROM player p     WHERE name_lower LIKE '%' || $1 || '%' ESCAPE '\\'       AND name_lower NOT LIKE $1 || '%' ESCAPE '\\' AND {IS_A_PLAYER}    LIMIT $3)) s ORDER BY tier, length(name_lower), name_lower LIMIT $2"
    );
    let boards: Vec<i32> = kiwi_core::activity_calc::NON_PLAYER_BOARDS.to_vec();
    let wide = (limit * 4).max(100);
    let stmt = client.prepare_cached(&sql).await?;
    let rows = client.query(&stmt, &[&like, &limit, &wide, &boards]).await?;
    let names = rows.iter().map(|r| r.try_get::<_, String>(0)).collect::<Result<Vec<_>, _>>()?;
    let total = names.len() as i64;
    Ok((names, total))
}

pub fn player_row(name: &str) -> Value {
    json!({"name": name, "path": format!("/player/{}", quote(name, "")), "detail": "", "kind": "player"})
}

/// One hub card as `_mods` reshapes it; `None` without a handle and slug.
pub fn mod_row(r: &Value, modpacks: bool) -> Option<Value> {
    let handle = truthy_str(r.get("handle"))?;
    let slug = truthy_str(r.get("slug"))?;
    let (root, kind) = if modpacks { ("/modpacks", "modpack") } else { ("/mods", "mod") };
    let sha = truthy_str(r.get("banner_sha")).or_else(|| truthy_str(r.get("preview_sha")));
    Some(json!({
        "name": truthy_str(r.get("title")).unwrap_or_else(|| slug.clone()),
        "path": format!("{root}/{handle}/{slug}"),
        "detail": truthy_str(r.get("owner_username")).unwrap_or_else(|| handle.clone()),
        "kind": kind,
        "image": sha.map(|s| format!("/site/mods/image/{}?w=200", quote(&s, ""))),
    }))
}

async fn mods(
    state: &AppState,
    query: &str,
    limit: i64,
    offset: i64,
    modpacks: bool,
) -> anyhow::Result<(Vec<Value>, i64)> {
    let (cards, total): (Vec<Value>, u64) = if modpacks {
        let q = crate::modpacks::body::ListQuery {
            q: Some(query.to_owned()),
            tag: None,
            author: None,
            sort: "recent".into(),
            limit,
            offset,
        };
        let (packs, total) = crate::modpacks::service::browse(state, &q).await.map_err(|e| anyhow::anyhow!("{e}"))?;
        (packs.iter().map(|p| Value::Object(crate::modpacks::service::pack_card(p))).collect(), total)
    } else {
        let a = crate::mods_catalog::service::ListArgs {
            q: Some(query.to_owned()),
            tag: None,
            author: None,
            sort: "recent".into(),
            limit,
            offset,
        };
        crate::mods_catalog::service::list_public(state, &a).await.map_err(|e| anyhow::anyhow!("{e}"))?
    };
    Ok((cards.iter().filter_map(|c| mod_row(c, modpacks)).collect(), total as i64))
}

async fn run(
    state: &AppState,
    subject: &str,
    query: &str,
    flags: &Flags,
    branch: &str,
    limit: i64,
    offset: i64,
) -> (Vec<Value>, i64) {
    let out = match subject {
        SUBJECT_PAGES => Ok(pages(query, flags, limit, offset)),
        SUBJECT_PLAYERS => search_players(state, query, limit + offset)
            .await
            .map(|(names, total)| (window(names, offset, limit).iter().map(|n| player_row(n)).collect(), total)),
        SUBJECT_MODS | SUBJECT_MODPACKS => mods(state, query, limit, offset, subject == SUBJECT_MODPACKS).await,
        _ => codex(state, query, subject, branch, limit, offset).await,
    };
    out.unwrap_or_else(|e| {
        tracing::warn!("site search: subject {subject:?} failed for {query:?}: {e:#}");
        (Vec::new(), 0)
    })
}

fn tagged(rows: Vec<Value>, subject: &str) -> impl Iterator<Item = Value> + '_ {
    rows.into_iter().map(move |r| {
        let mut m = match r {
            Value::Object(m) => m,
            _ => Map::new(),
        };
        m.insert("subject".into(), subject.into());
        Value::Object(m)
    })
}

/// Assembles the response from each subject's `(rows, total)`; `paged` is the
/// selected subject's own page.
pub fn assemble(
    query: &str,
    subjects: &[&'static str],
    subject: Option<&str>,
    preview: Vec<(Vec<Value>, i64)>,
    paged: Option<(Vec<Value>, i64)>,
) -> Value {
    let mut counts: Vec<i64> = preview.iter().map(|(_, t)| *t).collect();
    let label = |s: &str, n: i64| json!({"key": s, "label": index::subject_label(s), "count": n});
    if let (Some(subject), Some((rows, total))) = (subject, paged) {
        if let Some(i) = subjects.iter().position(|s| *s == subject) {
            counts[i] = counts[i].max(total);
        }
        let items: Vec<Value> = tagged(rows, subject).collect();
        let listed: Vec<Value> = subjects.iter().zip(&counts).map(|(s, n)| label(s, *n)).collect();
        let count = items.len();
        return json!({"query": query, "subjects": listed, "subject": subject, "items": items, "count": count, "total": total});
    }
    let mut items = Vec::new();
    for (s, (rows, _)) in subjects.iter().zip(preview) {
        items.extend(tagged(rows, s));
    }
    let listed: Vec<Value> =
        subjects.iter().zip(&counts).filter(|(_, n)| **n != 0).map(|(s, n)| label(s, *n)).collect();
    let total: i64 = counts.iter().sum();
    let count = items.len();
    json!({"query": query, "subjects": listed, "subject": null, "items": items, "count": count, "total": total})
}

pub fn empty(query: &str, subject: Option<&str>) -> Value {
    json!({"query": query, "subjects": [], "subject": subject, "items": [], "count": 0, "total": 0})
}

/// `search(query, flags, branch=..., subject=..., limit=..., offset=...)`.
pub async fn search(state: &AppState, q: &SearchQuery, flags: &Flags) -> Value {
    let query = kiwi_core::pystr::strip(&q.q).to_owned();
    let subjects = available_subjects(flags);
    let subject = q.subject.as_deref();
    if query.chars().count() < MIN_QUERY || subjects.is_empty() || subject.is_some_and(|s| !subjects.contains(&s)) {
        return empty(&query, subject);
    }
    let preview = futures::future::join_all(
        subjects.iter().map(|s| run(state, s, &query, flags, &q.branch, PREVIEW_PER_SUBJECT, 0)),
    )
    .await;
    let paged = match subject {
        Some(s) => Some(run(state, s, &query, flags, &q.branch, q.limit, q.offset).await),
        None => None,
    };
    assemble(&query, &subjects, subject, preview, paged)
}

pub async fn route(
    SiteGate(flags): SiteGate,
    State(state): State<AppState>,
    Query(q): Query<SearchQuery>,
) -> Result<Response, ApiError> {
    if !CODEX_BRANCHES.contains(&q.branch.as_str()) {
        return Err(ApiError::http(
            StatusCode::NOT_FOUND,
            Some(&format!("Unknown branch '{}' (known: {})", q.branch, CODEX_BRANCHES.join(", "))),
        ));
    }
    let flags = with_calc_flags(&state, flags, "leaderboard_renames_enabled").await?;
    Ok(cached(search(&state, &q, &flags).await, "public, max-age=15").into_response())
}

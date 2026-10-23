//! `app/trove/codexes/pg_store.py`: the same SQL text and arguments, rows back
//! as asyncpg records.

use std::collections::HashMap;

use anyhow::Context;
use chrono::{DateTime, Utc};
use deadpool_postgres::Pool;
use kiwi_core::codex::{badges, extract::Parsed, links::Link, models::StatRow, upgrades::Tree};
use serde_json::{Map, Value};

use super::pg::{self, Cell, P, Row};

pub const DEFAULT_SORT: &str = "name";

/// `SORTS`: public key -> ORDER BY, in Python's dict order.
pub const SORTS: [(&str, &str); 14] = [
    ("name", "name ASC"),
    ("-name", "name DESC"),
    ("category", "category ASC, name ASC"),
    ("-category", "category DESC, name ASC"),
    ("mastery", "mastery ASC NULLS LAST, name ASC"),
    ("-mastery", "mastery DESC NULLS LAST, name ASC"),
    ("mastery_geode", "mastery_geode ASC NULLS LAST, name ASC"),
    ("-mastery_geode", "mastery_geode DESC NULLS LAST, name ASC"),
    ("power_rank", "power_rank ASC NULLS LAST, name ASC"),
    ("-power_rank", "power_rank DESC NULLS LAST, name ASC"),
    ("indexed_at", "indexed_at ASC, name ASC"),
    ("-indexed_at", "indexed_at DESC, name ASC"),
    ("stat_value", "stat_value ASC NULLS LAST, name ASC"),
    ("-stat_value", "stat_value DESC NULLS LAST, name ASC"),
];

pub fn is_sort(sort: &str) -> bool {
    SORTS.iter().any(|(k, _)| *k == sort)
}

pub fn sort_keys() -> String {
    SORTS.iter().map(|(k, _)| *k).collect::<Vec<_>>().join(", ")
}

fn order_by(sort: &str) -> &'static str {
    SORTS.iter().find(|(k, _)| *k == sort).map_or("name ASC", |(_, v)| v)
}

const SELECT_COLS: [&str; 12] = [
    "codex_type",
    "path",
    "name",
    "category",
    "description",
    "tradable",
    "mastery",
    "mastery_geode",
    "power_rank",
    "blueprint",
    "data",
    "indexed_at",
];

const COLUMNS: [&str; 16] = [
    "branch",
    "path",
    "codex_type",
    "content_sha256",
    "name",
    "category",
    "description",
    "tradable",
    "mastery",
    "mastery_geode",
    "power_rank",
    "name_key",
    "desc_key",
    "blueprint",
    "data",
    "indexed_at",
];
const STAT_COLUMNS: [&str; 13] = [
    "branch",
    "path",
    "ord",
    "stat_key",
    "stat_id",
    "stat_name",
    "operation",
    "amount",
    "value",
    "is_percent",
    "slot_key",
    "label",
    "level",
];
const ABILITY_COLUMNS: [&str; 8] = ["branch", "path", "ord", "ref", "hidden", "loc_key", "name", "description"];
const LINK_COLUMNS: [&str; 7] = ["branch", "src_path", "rel", "dst_path", "ord", "qty", "data"];
const REQUIREMENT_COLUMNS: [&str; 12] = [
    "branch",
    "collection",
    "rank",
    "rank_name",
    "badge_id",
    "completion_kind",
    "requirement_key",
    "label",
    "amount",
    "difficulty",
    "status",
    "context",
];
const UPGRADE_COLUMNS: [&str; 9] =
    ["branch", "system_kind", "system_key", "node_key", "rank", "source_path", "costs", "requires", "effects"];

fn placeholders(n: usize) -> String {
    (1..=n).map(|i| format!("${i}")).collect::<Vec<_>>().join(", ")
}

fn upsert_sql() -> String {
    let sets: Vec<String> =
        COLUMNS.iter().filter(|c| !matches!(**c, "branch" | "path")).map(|c| format!("{c} = EXCLUDED.{c}")).collect();
    format!(
        "INSERT INTO codex_entry ({}) VALUES ({}) ON CONFLICT (branch, path) DO UPDATE SET {}",
        COLUMNS.join(", "),
        placeholders(COLUMNS.len()),
        sets.join(", ")
    )
}

fn insert_sql(table: &str, columns: &[&str]) -> String {
    format!(
        "INSERT INTO {table} ({}) VALUES ({}) ON CONFLICT DO NOTHING",
        columns.join(", "),
        placeholders(columns.len())
    )
}

fn escape_like(term: &str) -> String {
    term.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")
}

/// `build_filter` inputs.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Filter {
    pub codex_type: Option<String>,
    pub search: Option<String>,
    pub category: Option<String>,
    pub tradable: Option<bool>,
    pub stat: Option<String>,
    pub ability: Option<String>,
}

fn nonempty(s: &Option<String>) -> Option<&str> {
    s.as_deref().filter(|v| !v.is_empty())
}

/// `build_filter`: the WHERE body and its arguments.
pub fn build_filter(branch: &str, f: &Filter) -> (String, Vec<P>) {
    let mut conds = vec!["branch = $1".to_owned()];
    let mut args = vec![P::text(branch)];
    let mut add = |expr: &str, v: P, conds: &mut Vec<String>| {
        args.push(v);
        conds.push(expr.replace("{n}", &args.len().to_string()));
    };
    if let Some(t) = nonempty(&f.codex_type) {
        add("codex_type = ${n}", P::text(t), &mut conds);
    }
    if let Some(c) = nonempty(&f.category) {
        add("category = ${n}", P::text(c), &mut conds);
    }
    if let Some(t) = f.tradable {
        add("tradable = ${n}", P::Bool(t), &mut conds);
    }
    if let Some(s) = nonempty(&f.search) {
        add(
            "(name ILIKE ${n} ESCAPE '\\' OR description ILIKE ${n} ESCAPE '\\')",
            P::Text(format!("%{}%", escape_like(s))),
            &mut conds,
        );
    }
    if let Some(s) = nonempty(&f.stat) {
        add(
            "EXISTS (SELECT 1 FROM codex_stat s WHERE s.branch = codex_entry.branch AND s.path = codex_entry.path AND s.stat_key = ${n})",
            P::text(s),
            &mut conds,
        );
    }
    if let Some(a) = nonempty(&f.ability) {
        add(
            "EXISTS (SELECT 1 FROM codex_ability a WHERE a.branch = codex_entry.branch AND a.path = codex_entry.path AND a.ref = ${n} AND NOT a.hidden)",
            P::text(a),
            &mut conds,
        );
    }
    (conds.join(" AND "), args)
}

async fn client(pool: &Pool) -> anyhow::Result<deadpool_postgres::Object> {
    pool.get().await.context("postgres pool")
}

// --- reads -----------------------------------------------------------------------

/// `query_entries`: a page of rows plus the total.
pub async fn query_entries(
    pool: &Pool,
    branch: &str,
    f: &Filter,
    sort: &str,
    limit: i64,
    offset: i64,
) -> anyhow::Result<(Vec<Row>, i64)> {
    let (wh, mut args) = build_filter(branch, f);
    let count_args = args.clone();
    let mut cols: Vec<String> = SELECT_COLS.iter().map(|c| (*c).to_owned()).collect();
    let mut source = "codex_entry".to_owned();
    let mut sort = sort;
    if let Some(stat) = nonempty(&f.stat) {
        args.push(P::text(stat));
        source = format!(
            "codex_entry LEFT JOIN LATERAL (  SELECT s.value, s.is_percent FROM codex_stat s  WHERE s.branch = codex_entry.branch AND s.path = codex_entry.path    AND s.stat_key = ${}  ORDER BY s.value DESC NULLS LAST LIMIT 1) sv ON TRUE",
            args.len()
        );
        cols.push("sv.value AS stat_value".into());
        cols.push("sv.is_percent AS stat_percent".into());
    } else if sort == "stat_value" || sort == "-stat_value" {
        sort = DEFAULT_SORT;
    }
    let c = client(pool).await?;
    let total = pg::fetchval(&**c, &format!("SELECT count(*) FROM codex_entry WHERE {wh}"), &count_args).await?;
    let n = args.len();
    args.push(P::Int(limit));
    args.push(P::Int(offset));
    let sql = format!(
        "SELECT {} FROM {source} WHERE {wh} ORDER BY {} LIMIT ${} OFFSET ${}",
        cols.join(", "),
        order_by(sort),
        n + 1,
        n + 2
    );
    let rows = pg::fetch(&**c, &sql, &args).await?;
    Ok((rows, total.int().unwrap_or(0)))
}

fn type_scope(branch: &str, codex_type: Option<&str>, base: &[&str], col: &str) -> (String, Vec<P>) {
    let mut conds: Vec<String> = base.iter().map(|s| (*s).to_owned()).collect();
    let mut args = vec![P::text(branch)];
    if let Some(t) = codex_type.filter(|t| !t.is_empty()) {
        args.push(P::text(t));
        conds.push(format!("{col} = ${}", args.len()));
    }
    (conds.join(" AND "), args)
}

/// `stat_keys`.
pub async fn stat_keys(pool: &Pool, branch: &str, codex_type: Option<&str>) -> anyhow::Result<Vec<Row>> {
    let (wh, args) = type_scope(branch, codex_type, &["s.branch = $1"], "e.codex_type");
    let sql = format!(
        "SELECT s.stat_key AS stat, max(s.stat_name) AS stat_name,        count(DISTINCT s.path) AS count FROM codex_stat s JOIN codex_entry e ON e.branch = s.branch AND e.path = s.path WHERE {wh} GROUP BY s.stat_key ORDER BY lower(coalesce(nullif(max(s.stat_name), ''), s.stat_key)), s.stat_key"
    );
    pg::fetch(&**client(pool).await?, &sql, &args).await
}

/// `ability_refs`.
pub async fn ability_refs(pool: &Pool, branch: &str, codex_type: Option<&str>) -> anyhow::Result<Vec<Row>> {
    let (wh, args) = type_scope(branch, codex_type, &["a.branch = $1", "NOT a.hidden"], "e.codex_type");
    let sql = format!(
        "SELECT a.ref, max(a.name) AS name, max(a.description) AS description,        count(DISTINCT a.path) AS count FROM codex_ability a JOIN codex_entry e ON e.branch = a.branch AND e.path = a.path WHERE {wh} GROUP BY a.ref ORDER BY count DESC, a.ref"
    );
    pg::fetch(&**client(pool).await?, &sql, &args).await
}

/// `all_recipes`.
pub async fn all_recipes(pool: &Pool, branch: &str) -> anyhow::Result<Vec<Row>> {
    pg::fetch(
        &**client(pool).await?,
        "SELECT path, name, category, data FROM codex_entry WHERE branch = $1 AND codex_type = 'recipe'",
        &[P::text(branch)],
    )
    .await
}

/// `type_counts`: `(type, count)`.
pub async fn type_counts(pool: &Pool, branch: &str) -> anyhow::Result<Vec<(String, i64)>> {
    let rows = pg::fetch(
        &**client(pool).await?,
        "SELECT codex_type AS type, count(*) AS count FROM codex_entry WHERE branch = $1 GROUP BY codex_type ORDER BY codex_type",
        &[P::text(branch)],
    )
    .await?;
    Ok(rows.iter().map(|r| (r.text("type"), r.get("count").int().unwrap_or(0))).collect())
}

/// `list_categories`: `(category, count)`.
pub async fn list_categories(
    pool: &Pool,
    branch: &str,
    codex_type: Option<&str>,
) -> anyhow::Result<Vec<(String, i64)>> {
    let (wh, args) = type_scope(branch, codex_type, &["branch = $1", "category <> ''"], "codex_type");
    let sql =
        format!("SELECT category, count(*) AS count FROM codex_entry WHERE {wh} GROUP BY category ORDER BY category");
    let rows = pg::fetch(&**client(pool).await?, &sql, &args).await?;
    Ok(rows.iter().map(|r| (r.text("category"), r.get("count").int().unwrap_or(0))).collect())
}

/// `get_entry`.
pub async fn get_entry(pool: &Pool, branch: &str, codex_type: &str, path: &str) -> anyhow::Result<Option<Row>> {
    let sql = format!(
        "SELECT {} FROM codex_entry WHERE branch = $1 AND codex_type = $2 AND path = $3",
        SELECT_COLS.join(", ")
    );
    pg::fetchrow(&**client(pool).await?, &sql, &[P::text(branch), P::text(codex_type), P::text(path)]).await
}

/// `links_for`.
pub async fn links_for(
    pool: &Pool,
    branch: &str,
    path: &str,
    outward: bool,
    relation: Option<&str>,
    limit: i64,
) -> anyhow::Result<Vec<Row>> {
    let (near, far) = if outward { ("src_path", "dst_path") } else { ("dst_path", "src_path") };
    let mut conds = vec!["l.branch = $1".to_owned(), format!("l.{near} = $2")];
    let mut args = vec![P::text(branch), P::text(path)];
    if let Some(r) = relation.filter(|r| !r.is_empty()) {
        args.push(P::text(r));
        conds.push(format!("l.rel = ${}", args.len()));
    }
    args.push(P::Int(limit));
    let sql = format!(
        "SELECT l.rel, l.{far} AS path, l.ord, l.qty, l.data,        e.codex_type, e.name, e.category, e.blueprint FROM codex_link l LEFT JOIN codex_entry e ON e.branch = l.branch AND e.path = l.{far} WHERE {} ORDER BY l.rel, l.ord LIMIT ${}",
        conds.join(" AND "),
        args.len()
    );
    pg::fetch(&**client(pool).await?, &sql, &args).await
}

/// `entries_with_stat`: best row per entry, strongest first, sliced in Rust.
pub async fn entries_with_stat(
    pool: &Pool,
    branch: &str,
    stat_key: &str,
    codex_type: Option<&str>,
    limit: i64,
    offset: i64,
) -> anyhow::Result<(Vec<Row>, i64)> {
    let mut conds = vec!["s.branch = $1".to_owned(), "s.stat_key = $2".to_owned()];
    let mut args = vec![P::text(branch), P::text(stat_key)];
    if let Some(t) = codex_type.filter(|t| !t.is_empty()) {
        args.push(P::text(t));
        conds.push(format!("e.codex_type = ${}", args.len()));
    }
    let wh = conds.join(" AND ");
    let c = client(pool).await?;
    let total = pg::fetchval(
        &**c,
        &format!("SELECT count(DISTINCT s.path) FROM codex_stat s JOIN codex_entry e ON e.branch = s.branch AND e.path = s.path WHERE {wh}"),
        &args,
    )
    .await?;
    let mut rows = pg::fetch(
        &**c,
        &format!(
            "SELECT DISTINCT ON (s.path) s.path, s.stat_key, s.stat_name, s.value,        s.is_percent, s.slot_key, e.codex_type, e.name, e.category, e.blueprint FROM codex_stat s JOIN codex_entry e ON e.branch = s.branch AND e.path = s.path WHERE {wh} ORDER BY s.path, s.value DESC NULLS LAST"
        ),
        &args,
    )
    .await?;
    rows.sort_by(|a, b| {
        let key = |r: &Row| match r.get("value") {
            Cell::Float(v) => (false, if *v == 0.0 { 0.0 } else { -*v }),
            _ => (true, 0.0),
        };
        let (ka, kb) = (key(a), key(b));
        ka.0.cmp(&kb.0).then(ka.1.total_cmp(&kb.1)).then_with(|| a.text("name").cmp(&b.text("name")))
    });
    let start = usize::try_from(offset).unwrap_or(usize::MAX).min(rows.len());
    let end = start.saturating_add(usize::try_from(limit).unwrap_or(0)).min(rows.len());
    Ok((rows[start..end].to_vec(), total.int().unwrap_or(0)))
}

/// `requirements_for`: the whole ladder of the badge `collection` belongs to.
pub async fn requirements_for(pool: &Pool, branch: &str, collection: &str) -> anyhow::Result<Vec<Row>> {
    pg::fetch(
        &**client(pool).await?,
        "SELECT rank, rank_name, badge_id, collection, completion_kind,        requirement_key, label, amount, difficulty, status, context FROM codex_requirement WHERE branch = $1 AND badge_id = (    SELECT badge_id FROM codex_requirement     WHERE branch = $1 AND collection = $2 LIMIT 1) ORDER BY rank",
        &[P::text(branch), P::text(collection)],
    )
    .await
}

/// `upgrade_system` (no `effects` column, as in Python).
pub async fn upgrade_system(pool: &Pool, branch: &str, system_key: &str) -> anyhow::Result<Vec<Row>> {
    pg::fetch(
        &**client(pool).await?,
        "SELECT system_kind, system_key, node_key, rank, source_path, costs, requires FROM codex_upgrade WHERE branch = $1 AND system_key = $2 ORDER BY rank NULLS LAST, node_key",
        &[P::text(branch), P::text(system_key)],
    )
    .await
}

/// `upgrade_systems`.
pub async fn upgrade_systems(pool: &Pool, branch: &str) -> anyhow::Result<Vec<Row>> {
    pg::fetch(
        &**client(pool).await?,
        "SELECT system_kind, system_key, count(*) AS nodes FROM codex_upgrade WHERE branch = $1 GROUP BY system_kind, system_key ORDER BY system_kind, system_key",
        &[P::text(branch)],
    )
    .await
}

/// `meta_signature`: `(parser_version, updated_at)`, `(0, None)` without a row.
pub async fn meta_signature(pool: &Pool, branch: &str) -> anyhow::Result<(i64, Option<DateTime<Utc>>)> {
    let row = pg::fetchrow(
        &**client(pool).await?,
        "SELECT parser_version, updated_at FROM codex_meta WHERE branch = $1",
        &[P::text(branch)],
    )
    .await?;
    Ok(match row {
        Some(r) => (
            r.get("parser_version").int().unwrap_or(0),
            match r.get("updated_at") {
                Cell::Ts(t) => Some(*t),
                _ => None,
            },
        ),
        None => (0, None),
    })
}

async fn int_val(pool: &Pool, sql: &str, branch: &str) -> anyhow::Result<i64> {
    Ok(pg::fetchval(&**client(pool).await?, sql, &[P::text(branch)]).await?.int().unwrap_or(0))
}

pub async fn has_any(pool: &Pool, branch: &str) -> anyhow::Result<bool> {
    let v = pg::fetchval(
        &**client(pool).await?,
        "SELECT EXISTS (SELECT 1 FROM codex_entry WHERE branch = $1)",
        &[P::text(branch)],
    )
    .await?;
    Ok(matches!(v, Cell::Bool(true)))
}

pub async fn branch_count(pool: &Pool, branch: &str) -> anyhow::Result<i64> {
    int_val(pool, "SELECT count(*) FROM codex_entry WHERE branch = $1", branch).await
}

pub async fn get_parser_version(pool: &Pool, branch: &str) -> anyhow::Result<i64> {
    int_val(pool, "SELECT parser_version FROM codex_meta WHERE branch = $1", branch).await
}

pub async fn get_rig_version(pool: &Pool, branch: &str) -> anyhow::Result<i64> {
    int_val(pool, "SELECT rig_version FROM codex_meta WHERE branch = $1", branch).await
}

pub async fn rig_binding_count(pool: &Pool, branch: &str) -> anyhow::Result<i64> {
    int_val(pool, "SELECT count(*) FROM rig_binding WHERE branch = $1", branch).await
}

/// `load_rig_bindings`: `(prefab, blueprint, skeleton, ap_key, mesh_scale, head_scale)`.
pub async fn load_rig_bindings(
    pool: &Pool,
    branch: &str,
) -> anyhow::Result<Vec<(String, String, String, String, f64, f64)>> {
    let rows = pg::fetch(
        &**client(pool).await?,
        "SELECT prefab, blueprint, skeleton, ap_key, mesh_scale, head_scale FROM rig_binding WHERE branch = $1 ORDER BY prefab, blueprint",
        &[P::text(branch)],
    )
    .await?;
    let f = |c: &Cell| match c {
        Cell::Float(v) => *v,
        _ => 1.0,
    };
    Ok(rows
        .iter()
        .map(|r| {
            (
                r.text("prefab"),
                r.text("blueprint"),
                r.text("skeleton"),
                r.text("ap_key"),
                f(r.get("mesh_scale")),
                f(r.get("head_scale")),
            )
        })
        .collect())
}

/// `child_counts`.
pub async fn child_counts(pool: &Pool, branch: &str) -> anyhow::Result<Map<String, Value>> {
    let row = pg::fetchrow(
        &**client(pool).await?,
        "SELECT (SELECT count(*) FROM codex_entry WHERE branch = $1) AS entries,        (SELECT count(*) FROM codex_stat WHERE branch = $1) AS stats,        (SELECT count(*) FROM codex_ability WHERE branch = $1) AS abilities,        (SELECT count(*) FROM codex_link WHERE branch = $1) AS links,        (SELECT count(*) FROM codex_requirement WHERE branch = $1) AS requirements,        (SELECT count(*) FROM codex_upgrade WHERE branch = $1) AS upgrades",
        &[P::text(branch)],
    )
    .await?;
    Ok(row.map_or_else(Map::new, |r| r.0.iter().map(|(k, v)| (k.clone(), Value::from(v.int().unwrap_or(0)))).collect()))
}

// --- writes (indexer) --------------------------------------------------------------

fn int128(v: Option<i128>) -> P {
    v.map_or(P::Null, |i| P::Int(i64::try_from(i).unwrap_or(i64::MAX)))
}

/// `to_row`.
pub fn entry_args(branch: &str, e: &Parsed, sha: &str, now: DateTime<Utc>) -> Vec<P> {
    vec![
        P::text(branch),
        P::text(&e.path),
        P::text(&e.codex_type),
        P::text(sha),
        P::text(&e.name),
        P::text(&e.category),
        P::text(&e.description),
        P::opt_bool(e.tradable),
        int128(e.mastery),
        int128(e.mastery_geode),
        int128(e.power_rank),
        P::opt_text(e.name_key.clone()),
        P::opt_text(e.desc_key.clone()),
        P::opt_text(e.blueprint.clone()),
        P::json(&Value::Object(e.data.clone())),
        P::Ts(now),
    ]
}

pub fn stat_args(branch: &str, s: &StatRow) -> Vec<P> {
    vec![
        P::text(branch),
        P::text(&s.path),
        P::Int(s.ord),
        P::text(&s.stat_key),
        P::opt_int(s.stat_id),
        P::text(&s.stat_name),
        P::text(&s.operation),
        P::opt_float(s.amount),
        P::opt_float(s.value),
        P::Bool(s.is_percent),
        P::opt_text(s.slot_key.clone()),
        P::text(&s.label),
        P::Int(s.level),
    ]
}

pub fn ability_args(branch: &str, a: &kiwi_core::codex::models::AbilityRow) -> Vec<P> {
    vec![
        P::text(branch),
        P::text(&a.path),
        P::Int(a.ord),
        P::text(&a.r#ref),
        P::Bool(a.hidden),
        P::opt_text(a.loc_key.clone()),
        P::text(&a.name),
        P::text(&a.description),
    ]
}

pub fn link_args(branch: &str, l: &Link) -> Vec<P> {
    vec![
        P::text(branch),
        P::text(&l.src_path),
        P::text(l.rel),
        P::text(&l.dst_path),
        P::Int(l.ord),
        P::opt_float(l.qty),
        P::json(&Value::Object(l.data.clone())),
    ]
}

pub fn requirement_args(branch: &str, r: &badges::Row) -> Vec<P> {
    vec![
        P::text(branch),
        P::text(&r.collection),
        P::Int(i64::from(r.rank)),
        P::text(&r.rank_name),
        P::text(&r.badge_id),
        P::text(&r.completion_kind),
        P::text(&r.requirement_key),
        P::text(&r.label),
        P::opt_int(badges::stored_amount(&r.amount)),
        P::Int(i64::try_from(r.difficulty).unwrap_or(i64::MAX)),
        P::text(&r.status),
        P::json(&Value::Object(badges::context_map(&r.context))),
    ]
}

/// `upgrade_rows` for one tree.
pub fn upgrade_args(branch: &str, tree: &Tree, source_path: &str, effects: &HashMap<String, Value>) -> Vec<Vec<P>> {
    tree.nodes
        .iter()
        .map(|n| {
            vec![
                P::text(branch),
                P::text(tree.system_kind),
                P::text(&tree.system_key),
                P::text(&n.node_key),
                P::opt_int(n.rank),
                P::text(source_path),
                P::json(&Value::Array(n.costs.clone())),
                P::json(&serde_json::json!(n.requires)),
                P::json(effects.get(&n.node_key).unwrap_or(&Value::Object(Map::new()))),
            ]
        })
        .collect()
}

/// One flush: entries, then their child rows replaced by path, as Python does.
pub async fn flush(
    pool: &Pool,
    branch: &str,
    entries: &[Vec<P>],
    paths: &[String],
    children: &Children,
) -> anyhow::Result<()> {
    if entries.is_empty() {
        return Ok(());
    }
    let mut c = client(pool).await?;
    {
        let tx = c.transaction().await?;
        pg::executemany(&*tx, &upsert_sql(), entries).await?;
        tx.commit().await?;
    }
    if paths.is_empty() {
        return Ok(());
    }
    let tx = c.transaction().await?;
    let scope = [P::text(branch), P::TextList(paths.to_vec())];
    pg::execute(&*tx, "DELETE FROM codex_stat WHERE branch = $1 AND path = ANY($2::text[])", &scope).await?;
    pg::execute(&*tx, "DELETE FROM codex_ability WHERE branch = $1 AND path = ANY($2::text[])", &scope).await?;
    pg::execute(&*tx, "DELETE FROM codex_link WHERE branch = $1 AND src_path = ANY($2::text[])", &scope).await?;
    pg::executemany(&*tx, &insert_sql("codex_stat", &STAT_COLUMNS), &children.stats).await?;
    pg::executemany(&*tx, &insert_sql("codex_ability", &ABILITY_COLUMNS), &children.abilities).await?;
    pg::executemany(&*tx, &insert_sql("codex_link", &LINK_COLUMNS), &children.links).await?;
    tx.commit().await?;
    Ok(())
}

/// The child rows of one flush.
#[derive(Default)]
pub struct Children {
    pub stats: Vec<Vec<P>>,
    pub abilities: Vec<Vec<P>>,
    pub links: Vec<Vec<P>>,
}

/// `replace_links_for`.
pub async fn replace_links_for(pool: &Pool, branch: &str, relation: &str, rows: &[Vec<P>]) -> anyhow::Result<i64> {
    let mut c = client(pool).await?;
    let tx = c.transaction().await?;
    pg::execute(&*tx, "DELETE FROM codex_link WHERE branch = $1 AND rel = $2", &[P::text(branch), P::text(relation)])
        .await?;
    pg::executemany(&*tx, &insert_sql("codex_link", &LINK_COLUMNS), rows).await?;
    tx.commit().await?;
    Ok(rows.len() as i64)
}

async fn replace_branch(
    pool: &Pool,
    table: &str,
    columns: &[&str],
    branch: &str,
    rows: &[Vec<P>],
) -> anyhow::Result<i64> {
    let mut c = client(pool).await?;
    let tx = c.transaction().await?;
    pg::execute(&*tx, &format!("DELETE FROM {table} WHERE branch = $1"), &[P::text(branch)]).await?;
    pg::executemany(&*tx, &insert_sql(table, columns), rows).await?;
    tx.commit().await?;
    Ok(rows.len() as i64)
}

pub async fn replace_requirements(pool: &Pool, branch: &str, rows: &[Vec<P>]) -> anyhow::Result<i64> {
    replace_branch(pool, "codex_requirement", &REQUIREMENT_COLUMNS, branch, rows).await
}

pub async fn replace_upgrades(pool: &Pool, branch: &str, rows: &[Vec<P>]) -> anyhow::Result<i64> {
    replace_branch(pool, "codex_upgrade", &UPGRADE_COLUMNS, branch, rows).await
}

/// `delete_entries`: the entries and every child row they own.
pub async fn delete_entries(pool: &Pool, branch: &str, paths: &[String]) -> anyhow::Result<i64> {
    if paths.is_empty() {
        return Ok(0);
    }
    let mut c = client(pool).await?;
    let tx = c.transaction().await?;
    let scope = [P::text(branch), P::TextList(paths.to_vec())];
    pg::execute(&*tx, "DELETE FROM codex_entry WHERE branch = $1 AND path = ANY($2::text[])", &scope).await?;
    pg::execute(&*tx, "DELETE FROM codex_stat WHERE branch = $1 AND path = ANY($2::text[])", &scope).await?;
    pg::execute(&*tx, "DELETE FROM codex_ability WHERE branch = $1 AND path = ANY($2::text[])", &scope).await?;
    pg::execute(&*tx, "DELETE FROM codex_link WHERE branch = $1 AND src_path = ANY($2::text[])", &scope).await?;
    tx.commit().await?;
    Ok(paths.len() as i64)
}

/// `existing_shas`: path -> content sha.
pub async fn existing_shas(pool: &Pool, branch: &str) -> anyhow::Result<HashMap<String, String>> {
    let rows = pg::fetch(
        &**client(pool).await?,
        "SELECT path, content_sha256 FROM codex_entry WHERE branch = $1",
        &[P::text(branch)],
    )
    .await?;
    Ok(rows.iter().map(|r| (r.text("path"), r.text("content_sha256"))).collect())
}

pub async fn set_parser_version(pool: &Pool, branch: &str, version: i64) -> anyhow::Result<()> {
    pg::execute(
        &**client(pool).await?,
        "INSERT INTO codex_meta (branch, parser_version, updated_at) VALUES ($1, $2, now()) ON CONFLICT (branch) DO UPDATE SET parser_version = EXCLUDED.parser_version, updated_at = now()",
        &[P::text(branch), P::Int(version)],
    )
    .await
}

pub async fn set_rig_version(pool: &Pool, branch: &str, version: i64) -> anyhow::Result<()> {
    pg::execute(
        &**client(pool).await?,
        "INSERT INTO codex_meta (branch, rig_version, updated_at) VALUES ($1, $2, now()) ON CONFLICT (branch) DO UPDATE SET rig_version = EXCLUDED.rig_version, updated_at = now()",
        &[P::text(branch), P::Int(version)],
    )
    .await
}

pub async fn touch_meta(pool: &Pool, branch: &str) -> anyhow::Result<()> {
    pg::execute(
        &**client(pool).await?,
        "UPDATE codex_meta SET updated_at = now() WHERE branch = $1",
        &[P::text(branch)],
    )
    .await
}

const RIG_INSERT: &str = "INSERT INTO rig_binding (branch, prefab, blueprint, skeleton, ap_key, mesh_scale, head_scale) VALUES ($1, $2, $3, $4, $5, $6, $7) ON CONFLICT (branch, prefab, blueprint) DO UPDATE SET skeleton = EXCLUDED.skeleton, ap_key = EXCLUDED.ap_key, mesh_scale = EXCLUDED.mesh_scale, head_scale = EXCLUDED.head_scale";

/// `replace_rig_bindings`.
pub async fn replace_rig_bindings(pool: &Pool, branch: &str, rows: &[Vec<P>]) -> anyhow::Result<i64> {
    let mut c = client(pool).await?;
    let tx = c.transaction().await?;
    pg::execute(&*tx, "DELETE FROM rig_binding WHERE branch = $1", &[P::text(branch)]).await?;
    pg::executemany(&*tx, RIG_INSERT, rows).await?;
    tx.commit().await?;
    Ok(rows.len() as i64)
}

/// `replace_prefab_rig_bindings`.
pub async fn replace_prefab_rig_bindings(
    pool: &Pool,
    branch: &str,
    prefabs: &[String],
    rows: &[Vec<P>],
) -> anyhow::Result<i64> {
    if prefabs.is_empty() {
        return Ok(0);
    }
    let mut c = client(pool).await?;
    let tx = c.transaction().await?;
    pg::execute(
        &*tx,
        "DELETE FROM rig_binding WHERE branch = $1 AND prefab = ANY($2::text[])",
        &[P::text(branch), P::TextList(prefabs.to_vec())],
    )
    .await?;
    pg::executemany(&*tx, RIG_INSERT, rows).await?;
    tx.commit().await?;
    Ok(rows.len() as i64)
}

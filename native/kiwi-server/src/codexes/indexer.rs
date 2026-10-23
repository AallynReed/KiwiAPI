//! `app/trove/codexes/indexer.py`: the archive -> Postgres codex build (full,
//! forced and per-version delta), the shared tables and the rig map.

use std::{
    collections::{HashMap, HashSet},
    sync::{LazyLock, Mutex},
};

use chrono::{DateTime, Utc};
use deadpool_postgres::Pool;
use kiwi_core::codex::{
    PyErr, badges, blocks, catalogue,
    extract::{Blobs, Maps, Parsed, blueprint_stems},
    geode, links,
    localize::{self, Loc},
    loot, mastery, powerrank, providers, py,
    types::{LOCALE_ROOT, PREFABS_ROOT, classify},
    unlocks, upgrades,
    wire::{self, extract_localization_map},
};
use serde_json::{Map, Value, json};

use super::{
    archive::{Archive, Change},
    pg::P,
    store::{self, Children},
};
use crate::dt;

/// `CODEX_PARSER_VERSION`: kept at Python's value so both write the same meta.
pub const CODEX_PARSER_VERSION: i64 = 27;
pub const RIG_PARSER_VERSION: i64 = 4;

const FLUSH_AT: usize = 1000;
const MOUNT_TABLE: &str = "prefabs/collections/collection_mount.binfab";
const MULTIPLIERS: &str = "prefabs/meta/multipliers.binfab";
const GEODE_MULTIPLIERS: &str = "prefabs/meta/geode_multipliers.binfab";
const GEODE_TABLE: &str = "prefabs/collections/collection_geodecompanion.binfab";
const RECIPE_TABLE: &str = "prefabs/collections/collection_recipe.binfab";
const BLOCKS_TABLE: &str = "prefabs/blocks/blocks.binfab";
const POWERRANK_TABLE: &str = "prefabs/meta/powerrank.binfab";
const BADGES_TABLE: &str = "prefabs/meta/badges.binfab";
const UNLOCKS_TABLE: &str = "prefabs/collections/unlocks.binfab";

fn nonempty(b: Option<Vec<u8>>) -> Option<Vec<u8>> {
    b.filter(|v| !v.is_empty())
}

fn py_err(e: PyErr) -> anyhow::Error {
    anyhow::anyhow!(e.0)
}

/// `load_locale_map`.
pub async fn load_locale_map<A: Archive>(a: &A) -> anyhow::Result<Loc> {
    let rows = a.prefix(LOCALE_ROOT).await?;
    let blobs = a.blobs();
    let loc = tokio::task::spawn_blocking(move || {
        let mut loc = Loc::new();
        for (_, sha) in rows {
            if let Some(content) = nonempty(sha.and_then(|s| blobs.get(&s))) {
                extract_localization_map(&content, &mut loc);
            }
        }
        loc
    })
    .await?;
    Ok(loc)
}

async fn upgrade_trees<A: Archive>(a: &A) -> anyhow::Result<HashMap<String, Vec<u8>>> {
    let mut trees = HashMap::new();
    for (path, sha) in a.prefix(upgrades::UPGRADES_ROOT).await? {
        if !path.ends_with(".binfab") {
            continue;
        }
        let Some(sha) = sha else { continue };
        if let Some(content) = nonempty(a.blob(&sha).await?) {
            let last = py::last_part(&path, "/");
            let stem = last.strip_suffix(".binfab").unwrap_or(last);
            trees.insert(format!("{stem}_upgrade_tree"), content);
        }
    }
    Ok(trees)
}

async fn recipe_providers<A: Archive>(a: &A, known: &HashSet<String>) -> anyhow::Result<HashMap<String, Vec<Value>>> {
    if known.is_empty() {
        return Ok(HashMap::new());
    }
    let mut prefabs = Vec::new();
    for (path, sha) in a.regex(providers::PROVIDER_PATH_RE, true).await? {
        if !(path.starts_with(PREFABS_ROOT) && path.ends_with(".binfab")) {
            continue;
        }
        let Some(sha) = sha else { continue };
        if let Some(content) = nonempty(a.blob(&sha).await?) {
            prefabs.push((path, content));
        }
    }
    let known = known.clone();
    tokio::task::spawn_blocking(move || providers::build_provider_map(&prefabs, &known)).await?.map_err(py_err)
}

async fn style_catalogues<A: Archive>(a: &A) -> anyhow::Result<HashMap<String, loot::StyleRow>> {
    let mut catalogues = Vec::new();
    for (path, sha) in a.prefix(loot::LOOT_ROOT).await? {
        if !loot::is_style_catalogue(&path) {
            continue;
        }
        let Some(sha) = sha else { continue };
        if let Some(content) = nonempty(a.blob(&sha).await?) {
            catalogues.retain(|(p, _): &(String, Vec<u8>)| *p != path);
            catalogues.push((path, content));
        }
    }
    if catalogues.is_empty() {
        return Ok(HashMap::new());
    }
    tokio::task::spawn_blocking(move || loot::style_index(&catalogues)).await?.map_err(py_err)
}

async fn prefab_shas<A: Archive>(a: &A) -> anyhow::Result<HashMap<String, String>> {
    Ok(a.prefix(PREFABS_ROOT).await?.into_iter().filter_map(|(p, s)| s.map(|s| (p, s))).collect())
}

fn walk_blueprints(dir: &std::path::Path, base: &std::path::Path, out: &mut HashSet<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let path = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        if name.ends_with(".blueprint")
            && let Ok(rel) = path.strip_prefix(base)
        {
            let rel: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
            out.insert(py::lower(&rel.join("/")));
        }
        if e.file_type().is_ok_and(|t| t.is_dir()) {
            walk_blueprints(&path, base, out);
        }
    }
}

/// `_load_valid_blueprints` with the name -> sha map.
async fn valid_blueprints<A: Archive>(
    a: &A,
    local_game_dir: &str,
) -> anyhow::Result<(HashSet<String>, HashMap<String, String>)> {
    let mut valid = HashSet::new();
    let mut shas = HashMap::new();
    for (path, sha) in a.regex("^blueprints/", false).await? {
        let rel = py::lower(path.strip_prefix("blueprints/").unwrap_or(&path));
        if let Some(s) = sha.filter(|s| !s.is_empty()) {
            shas.insert(rel.clone(), s);
        }
        valid.insert(rel);
    }
    if !local_game_dir.is_empty() {
        let dir = std::path::Path::new(local_game_dir).join("blueprints");
        if dir.is_dir() {
            walk_blueprints(&dir, &dir, &mut valid);
        }
    }
    Ok((valid, shas))
}

/// `_load_maps`.
pub async fn load_maps<A: Archive>(a: &A, with_resolver: bool, local_game_dir: &str) -> anyhow::Result<Maps<A::Blobs>> {
    let mount_table = nonempty(a.load_file(MOUNT_TABLE).await?);
    let multipliers = nonempty(a.load_file(MULTIPLIERS).await?);
    let geode_mult = nonempty(a.load_file(GEODE_MULTIPLIERS).await?);
    let geode_table = nonempty(a.load_file(GEODE_TABLE).await?);
    let geode_members = geode_table.as_deref().map(geode::geode_companion_members).unwrap_or_default();
    let recipe_table = if with_resolver { nonempty(a.load_file(RECIPE_TABLE).await?) } else { None };
    let prefab_shas = if with_resolver { prefab_shas(a).await? } else { HashMap::new() };
    let root = "prefabs/recipes/";
    let known: HashSet<String> = prefab_shas
        .keys()
        .filter(|p| p.starts_with(root) && p.ends_with(".binfab"))
        .map(|p| py::lower(p[root.len()..].strip_suffix(".binfab").unwrap_or(&p[root.len()..])))
        .collect();
    let blocks_table = nonempty(a.load_file(BLOCKS_TABLE).await?);
    let pr_table = nonempty(a.load_file(POWERRANK_TABLE).await?);
    let style_rows = style_catalogues(a).await?;
    let (valid, bp_shas) = valid_blueprints(a, local_game_dir).await?;
    let loc = load_locale_map(a).await?;
    let upgrade_trees = if geode_members.is_empty() { HashMap::new() } else { upgrade_trees(a).await? };
    let recipe_catalogue =
        recipe_table.as_deref().map(|t| catalogue::parse_recipe_catalogue(t, &known)).unwrap_or_default();
    let recipe_providers = recipe_providers(a, &known).await?;
    let mut maps = Maps::<A::Blobs>::default();
    maps.loc = loc;
    maps.mount_categories =
        mount_table.as_deref().map(wire::collection_category_map).unwrap_or_default().into_iter().collect();
    maps.multipliers = multipliers.as_deref().map(mastery::parse_multipliers).unwrap_or_default();
    maps.geode_multipliers = geode_mult.as_deref().map(mastery::parse_geode_multipliers).unwrap_or_default();
    maps.geode_members = geode_members;
    maps.upgrade_trees = upgrade_trees;
    maps.recipe_catalogue = recipe_catalogue;
    maps.recipe_providers = recipe_providers;
    maps.store = Some(a.blobs());
    maps.prefab_shas = prefab_shas;
    maps.blueprint_stems = blueprint_stems(&valid);
    maps.valid_blueprints = valid;
    maps.blueprint_shas = bp_shas;
    maps.block_models = blocks_table.as_deref().map(blocks::parse_block_models).unwrap_or_default();
    maps.power_rank_table = pr_table.as_deref().map(powerrank::parse_power_rank_table).unwrap_or_default();
    maps.style_rows = style_rows;
    tracing::info!(
        "codexes: {} mount categories, {} mastery rows, {} geode rows, {} geode members, {} upgrade trees, {} prefab refs, {} blueprints, {} recipe catalogue, {} recipe providers, {} block models",
        maps.mount_categories.len(),
        maps.multipliers.len(),
        maps.geode_multipliers.len(),
        maps.geode_members.len(),
        maps.upgrade_trees.len(),
        maps.prefab_shas.len(),
        maps.valid_blueprints.len(),
        maps.recipe_catalogue.len(),
        maps.recipe_providers.len(),
        maps.block_models.len()
    );
    Ok(maps)
}

/// One prefab to (re)parse.
#[derive(Clone)]
pub struct Job {
    pub path: String,
    pub sha: String,
    pub ctype: &'static str,
}

/// `_parse_entry` over a run of prefabs, stopping at the first raise; `None`
/// is a missing blob.
pub fn parse_jobs<S: Blobs>(maps: &Maps<S>, jobs: &[Job]) -> Vec<Result<Option<Parsed>, PyErr>> {
    let mut out = Vec::with_capacity(jobs.len());
    for j in jobs {
        let content = maps.store.as_ref().and_then(|s| s.get(&j.sha));
        let r = match content {
            None => Ok(None),
            Some(c) => maps.parse_entry(&j.path, j.ctype, &c).map(Some),
        };
        let failed = r.is_err();
        out.push(r);
        if failed {
            break;
        }
    }
    out
}

/// Accumulates parsed prefabs and flushes them 1000 at a time.
struct Batch<'a> {
    pool: &'a Pool,
    branch: &'a str,
    now: DateTime<Utc>,
    entries: Vec<Vec<P>>,
    paths: Vec<String>,
    children: Children,
}

impl<'a> Batch<'a> {
    fn new(pool: &'a Pool, branch: &'a str, now: DateTime<Utc>) -> Self {
        Batch { pool, branch, now, entries: Vec::new(), paths: Vec::new(), children: Children::default() }
    }

    async fn add(&mut self, p: &Parsed, sha: &str) -> anyhow::Result<()> {
        self.entries.push(store::entry_args(self.branch, p, sha, self.now));
        self.paths.push(p.path.clone());
        self.children.stats.extend(p.stats.iter().map(|s| store::stat_args(self.branch, s)));
        self.children.abilities.extend(p.abilities.iter().map(|s| store::ability_args(self.branch, s)));
        self.children.links.extend(p.links.iter().map(|l| store::link_args(self.branch, l)));
        if self.entries.len() >= FLUSH_AT {
            self.flush().await?;
        }
        Ok(())
    }

    async fn flush(&mut self) -> anyhow::Result<()> {
        store::flush(self.pool, self.branch, &self.entries, &self.paths, &self.children).await?;
        self.entries.clear();
        self.paths.clear();
        self.children = Children::default();
        Ok(())
    }
}

/// Parses `jobs` off the event loop and feeds `batch`; returns the maps back.
async fn run_jobs<S: Blobs + Send + 'static>(
    mut maps: Maps<S>,
    jobs: Vec<Job>,
    batch: &mut Batch<'_>,
    counts: &mut Counts,
) -> anyhow::Result<Maps<S>> {
    for chunk in jobs.chunks(256) {
        let chunk = chunk.to_vec();
        let (m, results, chunk) = tokio::task::spawn_blocking(move || {
            let r = parse_jobs(&maps, &chunk);
            (maps, r, chunk)
        })
        .await?;
        maps = m;
        for (job, r) in chunk.iter().zip(results) {
            match r.map_err(py_err)? {
                None => counts.add("missing_blob", 1),
                Some(p) => {
                    batch.add(&p, &job.sha).await?;
                    counts.add("indexed", 1);
                }
            }
        }
    }
    Ok(maps)
}

/// Python's counts dict: insertion-ordered ints.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Counts(pub Vec<(String, i64)>);

impl Counts {
    fn with(keys: &[&str]) -> Self {
        Counts(keys.iter().map(|k| ((*k).to_owned(), 0)).collect())
    }

    fn add(&mut self, key: &str, n: i64) {
        match self.0.iter_mut().find(|(k, _)| k == key) {
            Some(slot) => slot.1 += n,
            None => self.0.push((key.to_owned(), n)),
        }
    }

    fn set(&mut self, key: &str, n: i64) {
        match self.0.iter_mut().find(|(k, _)| k == key) {
            Some(slot) => slot.1 = n,
            None => self.0.push((key.to_owned(), n)),
        }
    }

    pub fn to_value(&self) -> Value {
        Value::Object(self.0.iter().map(|(k, v)| (k.clone(), json!(v))).collect())
    }
}

/// The shared-table rows of one branch, before they are written.
#[derive(Default)]
pub struct Shared {
    pub requirements: Option<Vec<badges::Row>>,
    pub upgrades: Vec<Vec<P>>,
    pub cost_edges: Vec<links::Link>,
    pub unlock_edges: Option<Vec<links::Link>>,
    pub member_edges: Vec<links::Link>,
}

/// `_resolve_effects`.
fn resolve_effects(effects: HashMap<String, (String, String, Vec<String>)>, loc: &Loc) -> HashMap<String, Value> {
    effects
        .into_iter()
        .map(|(node, (name_key, desc_key, abilities))| {
            let resolve = |k: &str| if k.is_empty() { String::new() } else { localize::resolve_text(loc, Some(k)) };
            let mut m = Map::new();
            m.insert("name_key".into(), json!(name_key));
            m.insert("name".into(), json!(resolve(&name_key)));
            m.insert("desc_key".into(), json!(desc_key));
            m.insert("description".into(), json!(resolve(&desc_key)));
            m.insert("abilities".into(), json!(abilities));
            (node, Value::Object(m))
        })
        .collect()
}

/// Every shared table, read the way `_index_shared_tables` reads them.
pub async fn shared_tables<A: Archive>(a: &A, branch: &str, loc: &Loc) -> anyhow::Result<Shared> {
    let mut out = Shared::default();
    if let Some(data) = nonempty(a.load_file(BADGES_TABLE).await?) {
        let names = match nonempty(a.load_file(badges::EXE_PATH).await?) {
            Some(exe) => tokio::task::spawn_blocking(move || badges::parse_metric_names(&exe)).await?,
            None => {
                tracing::warn!("codexes[{branch}]: {} absent - badge metric names unresolved", badges::EXE_PATH);
                Vec::new()
            }
        };
        let (rows, errors) = tokio::task::spawn_blocking(move || badges::parse_badges(&data, &names)).await?;
        if !errors.is_empty() {
            tracing::warn!(
                "codexes[{branch}]: badge decode reported {} issue(s): {:?}",
                errors.len(),
                &errors[..errors.len().min(3)]
            );
        }
        out.requirements = Some(badges::requirement_rows(&rows).into_iter().cloned().collect());
    }

    let docs = a.prefix(upgrades::TREES_ROOT).await?;
    let effect_shas: HashMap<String, Option<String>> = a.prefix(upgrades::UPGRADES_ROOT).await?.into_iter().collect();
    for (path, sha) in docs {
        if !path.ends_with(".binfab") {
            continue;
        }
        let Some(content) = nonempty(match sha {
            Some(s) => a.blob(&s).await?,
            None => None,
        }) else {
            continue;
        };
        let tree = upgrades::parse_upgrade_costs(&content, &path);
        if tree.nodes.is_empty() {
            continue;
        }
        let mut effects = HashMap::new();
        if let Some(Some(sha)) = effect_shas.get(&format!("{}{}.binfab", upgrades::UPGRADES_ROOT, tree.system_key))
            && let Some(raw) = nonempty(a.blob(sha).await?)
        {
            let keys: Vec<String> = tree.nodes.iter().map(|n| n.node_key.clone()).collect();
            effects = resolve_effects(upgrades::parse_upgrade_effects(&raw, &keys), loc);
        }
        out.upgrades.extend(store::upgrade_args(branch, &tree, &path, &effects));
        out.cost_edges.extend(links::upgrade_links(&tree, &path));
    }

    if let Some(data) = nonempty(a.load_file(UNLOCKS_TABLE).await?) {
        out.unlock_edges = Some(links::unlock_links(&unlocks::parse_unlocks(&data)));
    }

    for (path, sha) in a.prefix("prefabs/collections/collection_").await? {
        if !path.ends_with(".binfab") {
            continue;
        }
        let Some(content) = nonempty(match sha {
            Some(s) => a.blob(&s).await?,
            None => None,
        }) else {
            continue;
        };
        out.member_edges.extend(links::membership_links(&wire::collection_category_map(&content), &path));
    }
    Ok(out)
}

/// `_index_shared_tables`: write them; Python's counts.
async fn index_shared_tables<A: Archive>(pool: &Pool, a: &A, branch: &str, loc: &Loc) -> anyhow::Result<Counts> {
    let shared = shared_tables(a, branch, loc).await?;
    let requirements = match &shared.requirements {
        Some(rows) => {
            let args: Vec<Vec<P>> = rows.iter().map(|r| store::requirement_args(branch, r)).collect();
            store::replace_requirements(pool, branch, &args).await?
        }
        None => 0,
    };
    let nodes = store::replace_upgrades(pool, branch, &shared.upgrades).await?;
    let link_rows = |l: &[links::Link]| l.iter().map(|x| store::link_args(branch, x)).collect::<Vec<_>>();
    let cost = store::replace_links_for(pool, branch, links::UPGRADE_COST, &link_rows(&shared.cost_edges)).await?;
    let unlock = match &shared.unlock_edges {
        Some(e) => store::replace_links_for(pool, branch, links::UNLOCKS, &link_rows(e)).await?,
        None => 0,
    };
    let member = store::replace_links_for(pool, branch, links::MEMBER_OF, &link_rows(&shared.member_edges)).await?;
    tracing::info!(
        "codexes[{branch}]: shared tables - {requirements} badge requirements, {nodes} upgrade nodes, {cost} cost edges, {unlock} unlock edges, {member} membership edges"
    );
    Ok(Counts(vec![
        ("requirements".into(), requirements),
        ("upgrade_nodes".into(), nodes),
        ("link_edges".into(), cost + unlock + member),
    ]))
}

/// `reindex`: full build, skipping unchanged prefabs unless `force`.
pub async fn reindex<A: Archive>(
    pool: &Pool,
    a: &A,
    branch: &str,
    force: bool,
    local_game_dir: &str,
) -> anyhow::Result<Counts> {
    let maps = load_maps(a, true, local_game_dir).await?;
    let existing = store::existing_shas(pool, branch).await?;
    let docs = a.prefix(PREFABS_ROOT).await?;
    let mut counts = Counts::with(&["indexed", "unchanged", "missing_blob", "removed"]);
    let mut seen = HashSet::new();
    let now = dt::now();
    let mut jobs = Vec::new();
    for (path, sha) in docs {
        let Some(ctype) = classify(&path) else { continue };
        seen.insert(path.clone());
        let sha = sha.unwrap_or_default();
        if !force && existing.get(&path) == Some(&sha) {
            counts.add("unchanged", 1);
            continue;
        }
        jobs.push(Job { path, sha, ctype });
    }
    let mut batch = Batch::new(pool, branch, now);
    let maps = run_jobs(maps, jobs, &mut batch, &mut counts).await?;
    batch.flush().await?;

    let stale: Vec<String> = existing.keys().filter(|p| !seen.contains(*p)).cloned().collect();
    if !stale.is_empty() {
        store::delete_entries(pool, branch, &stale).await?;
        counts.set("removed", stale.len() as i64);
    }
    for (k, v) in index_shared_tables(pool, a, branch, &maps.loc).await?.0 {
        counts.set(&k, v);
    }
    store::set_parser_version(pool, branch, CODEX_PARSER_VERSION).await?;
    tracing::info!("codexes[{branch}]: {} {:?}", if force { "forced rebuild" } else { "full" }, counts.0);
    Ok(counts)
}

fn shared_sources_touched(changes: &[Change]) -> bool {
    changes.iter().any(|c| {
        [BADGES_TABLE, UNLOCKS_TABLE, badges::EXE_PATH].contains(&c.path.as_str())
            || c.path.starts_with(upgrades::TREES_ROOT)
    })
}

/// `reindex_changes`: just one version's delta.
pub async fn reindex_changes<A: Archive>(
    pool: &Pool,
    a: &A,
    branch: &str,
    ordinal: i64,
    local_game_dir: &str,
) -> anyhow::Result<Counts> {
    let rows = a.changes(ordinal).await?;
    let touched: Vec<&Change> = rows.iter().filter(|r| classify(&r.path).is_some()).collect();
    let mut counts = Counts::with(&["indexed", "removed", "missing_blob"]);
    let shared = shared_sources_touched(&rows);
    if touched.is_empty() && !shared {
        return Ok(counts);
    }
    if touched.is_empty() {
        let maps = load_maps(a, false, local_game_dir).await?;
        for (k, v) in index_shared_tables(pool, a, branch, &maps.loc).await?.0 {
            counts.set(&k, v);
        }
        store::touch_meta(pool, branch).await?;
        return Ok(counts);
    }
    let parse: Vec<&&Change> =
        touched.iter().filter(|r| r.kind != "removed" && r.sha.as_deref().is_some_and(|s| !s.is_empty())).collect();
    let needs_resolver = parse.iter().any(|r| classify(&r.path) == Some("recipe"));
    let maps = if parse.is_empty() { Maps::default() } else { load_maps(a, needs_resolver, local_game_dir).await? };
    let now = dt::now();
    let mut removed = Vec::new();
    let mut jobs = Vec::new();
    for r in &touched {
        if r.kind == "removed" {
            removed.push(r.path.clone());
            counts.add("removed", 1);
            continue;
        }
        match r.sha.as_deref().filter(|s| !s.is_empty()) {
            None => counts.add("missing_blob", 1),
            Some(sha) => {
                jobs.push(Job { path: r.path.clone(), sha: sha.to_owned(), ctype: classify(&r.path).expect("touched") })
            }
        }
    }
    let mut batch = Batch::new(pool, branch, now);
    let maps = run_jobs(maps, jobs, &mut batch, &mut counts).await?;
    batch.flush().await?;
    store::delete_entries(pool, branch, &removed).await?;
    if shared {
        for (k, v) in index_shared_tables(pool, a, branch, &maps.loc).await?.0 {
            counts.set(&k, v);
        }
    }
    store::touch_meta(pool, branch).await?;
    tracing::info!("codexes[{branch}]: delta ordinal={ordinal} {:?}", counts.0);
    Ok(counts)
}

/// `_index_decision`.
pub fn index_decision(has_any: bool, stored: i64, current: i64, changed: bool, ordinal: Option<i64>) -> &'static str {
    if !has_any {
        "full"
    } else if stored < current {
        "rebuild"
    } else if changed && ordinal.is_some() {
        "delta"
    } else {
        "noop"
    }
}

/// `_rig_rows`: `(prefab, blueprint, skeleton, ap_key, mesh_scale, head_scale)`.
pub fn rig_rows<S: Blobs>(
    blobs: &S,
    candidates: &[(String, String)],
) -> Vec<(String, String, String, String, f64, f64)> {
    let mut rows = Vec::new();
    for (path, sha) in candidates {
        let Some(content) = blobs.get(sha) else { continue };
        if content.is_empty() || !py::contains(&content, b".skeleton.gr2") {
            continue;
        }
        let Some(rig) = wire::extract_rig_refs(&content) else { continue };
        for (bp, ap) in &rig.parts {
            let scale = rig.scales.get(bp).copied().unwrap_or(1.0);
            rows.push((path.clone(), bp.clone(), rig.skeleton.clone(), ap.clone(), scale, rig.head_scale));
        }
    }
    rows
}

fn rig_args(branch: &str, rows: Vec<(String, String, String, String, f64, f64)>) -> Vec<Vec<P>> {
    rows.into_iter()
        .map(|(p, bp, sk, ap, ms, hs)| {
            vec![P::text(branch), P::Text(p), P::Text(bp), P::Text(sk), P::Text(ap), P::Float(ms), P::Float(hs)]
        })
        .collect()
}

/// `reindex_rigs`: every prefab.
pub async fn reindex_rigs<A: Archive>(pool: &Pool, a: &A, branch: &str) -> anyhow::Result<i64> {
    let candidates: Vec<(String, String)> =
        a.prefix(PREFABS_ROOT).await?.into_iter().map(|(p, s)| (p, s.unwrap_or_default())).collect();
    let n_candidates = candidates.len();
    let blobs = a.blobs();
    let rows = tokio::task::spawn_blocking(move || rig_rows(&blobs, &candidates)).await?;
    let n = store::replace_rig_bindings(pool, branch, &rig_args(branch, rows)).await?;
    store::set_rig_version(pool, branch, RIG_PARSER_VERSION).await?;
    store::touch_meta(pool, branch).await?;
    tracing::info!("codexes[{branch}]: rig map rebuilt - {n} bindings from {n_candidates} prefabs");
    Ok(n)
}

/// `reindex_rigs_changes`: just the prefabs one version touched.
pub async fn reindex_rigs_changes<A: Archive>(pool: &Pool, a: &A, branch: &str, ordinal: i64) -> anyhow::Result<i64> {
    let rows = a.changes(ordinal).await?;
    let touched: Vec<&Change> =
        rows.iter().filter(|r| r.path.starts_with(PREFABS_ROOT) && r.path.ends_with(".binfab")).collect();
    let prefabs: Vec<String> = touched.iter().map(|r| r.path.clone()).collect();
    let changed: Vec<(String, String)> = touched
        .iter()
        .filter(|r| r.kind != "removed")
        .filter_map(|r| r.sha.clone().filter(|s| !s.is_empty()).map(|s| (r.path.clone(), s)))
        .collect();
    if prefabs.is_empty() {
        return Ok(0);
    }
    let n_changed = changed.len();
    let blobs = a.blobs();
    let new_rows = tokio::task::spawn_blocking(move || rig_rows(&blobs, &changed)).await?;
    let n = store::replace_prefab_rig_bindings(pool, branch, &prefabs, &rig_args(branch, new_rows)).await?;
    store::touch_meta(pool, branch).await?;
    tracing::info!(
        "codexes[{branch}]: rig delta ordinal={ordinal} - {n} bindings from {n_changed} re-parsed prefabs ({} re-stated)",
        prefabs.len()
    );
    Ok(n)
}

/// `ensure_indexed`: the archiver's post-sync hook (`changed` / `ordinal`
/// from its sync summary). `pool` is `None` when Postgres is off.
pub async fn ensure_indexed<A: Archive>(
    pool: Option<&Pool>,
    a: &A,
    branch: &str,
    changed: bool,
    ordinal: Option<i64>,
    local_game_dir: &str,
) -> anyhow::Result<Counts> {
    let zero = Counts::with(&["indexed", "removed", "missing_blob"]);
    let Some(pool) = pool else {
        tracing::warn!("codexes[{branch}]: Postgres disabled - skipping index");
        return Ok(zero);
    };
    let has_any = store::has_any(pool, branch).await?;
    let stored = store::get_parser_version(pool, branch).await?;
    let decision = index_decision(has_any, stored, CODEX_PARSER_VERSION, changed, ordinal);
    let counts = match decision {
        "full" => reindex(pool, a, branch, false, local_game_dir).await?,
        "rebuild" => {
            tracing::info!("codexes[{branch}]: parser advanced to v{CODEX_PARSER_VERSION} - rebuilding");
            reindex(pool, a, branch, true, local_game_dir).await?
        }
        "delta" => reindex_changes(pool, a, branch, ordinal.unwrap_or_default(), local_game_dir).await?,
        _ => zero,
    };
    let rigs = async {
        let stale = store::get_rig_version(pool, branch).await? < RIG_PARSER_VERSION
            || store::rig_binding_count(pool, branch).await? == 0;
        if matches!(decision, "full" | "rebuild") || stale {
            reindex_rigs(pool, a, branch).await?;
        } else if decision == "delta" {
            reindex_rigs_changes(pool, a, branch, ordinal.unwrap_or_default()).await?;
        }
        anyhow::Ok(())
    };
    if let Err(e) = rigs.await {
        tracing::warn!("codexes[{branch}]: rig reindex failed: {e:#}");
    }
    Ok(counts)
}

// --- manual rebuild status (`_REBUILD_STATUS`) ---------------------------------------

static REBUILD_STATUS: LazyLock<Mutex<HashMap<String, Value>>> = LazyLock::new(Mutex::default);

fn idle() -> Value {
    json!({"running": false, "started_at": null, "finished_at": null, "counts": null, "error": null})
}

/// `get_rebuild_status`.
pub fn rebuild_status(branch: &str) -> Value {
    REBUILD_STATUS.lock().expect("status lock").get(branch).cloned().unwrap_or_else(idle)
}

fn set_status(branch: &str, v: Value) {
    REBUILD_STATUS.lock().expect("status lock").insert(branch.to_owned(), v);
}

/// Marks a rebuild as running (what `rebuild()` does before its first await).
pub fn mark_running(branch: &str) -> String {
    let started = dt::isoformat(&dt::now());
    set_status(
        branch,
        json!({"running": true, "started_at": started, "finished_at": null, "counts": null, "error": null}),
    );
    started
}

/// `rebuild`: forced re-parse plus the rig map, status kept for the admin poll.
pub async fn rebuild<A: Archive>(pool: &Pool, a: &A, branch: &str, started: String, local_game_dir: &str) {
    let result = async {
        let counts = reindex(pool, a, branch, true, local_game_dir).await?;
        reindex_rigs(pool, a, branch).await?;
        anyhow::Ok(counts)
    }
    .await;
    let finished = dt::isoformat(&dt::now());
    match result {
        Ok(counts) => set_status(
            branch,
            json!({"running": false, "started_at": started, "finished_at": finished, "counts": counts.to_value(), "error": null}),
        ),
        Err(e) => {
            tracing::error!("codexes[{branch}]: manual rebuild failed: {e:#}");
            set_status(
                branch,
                json!({"running": false, "started_at": started, "finished_at": finished, "counts": null, "error": e.to_string()}),
            );
        }
    }
}

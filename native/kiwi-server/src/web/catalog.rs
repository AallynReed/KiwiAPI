//! The catalog pages of `app/web/pages.py` with their `app/site/ssr.py` first
//! paint: /market, /store, /codexes, /mods (profiles and projects with their
//! Open Graph tags and JSON-LD) and /modpacks. The reference pages live in
//! `reference.rs`; [`router`] carries both.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Instant;

use axum::{
    Router,
    extract::State,
    http::{HeaderMap, Method, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::get,
};
use indexmap::IndexMap;
use minijinja::Value as Tpl;
use regex::Regex;
use serde_json::{Map, Value, json};

use super::{Gate, Web, flags::Flags, internal::InternalApi, pages::render, reference};
use crate::error::{ApiError, http_exception};
use crate::middleware::pyurl::{quote, scope_path, url_path};
use kiwi_core::pystr;

pub fn router() -> Router<Arc<Web>> {
    Router::new()
        .route("/commands", get(reference::commands))
        .route("/classes", get(reference::classes))
        .route("/abilities", get(reference::abilities))
        .route("/allies", get(reference::allies))
        .route("/market", get(market))
        .route("/store", get(store))
        .route("/codexes", get(codexes))
        .route("/mods", get(mods))
        .route("/mods/{handle}", get(mod_profile))
        .route("/mods/{handle}/{slug}", get(mod_project))
        .route("/modpacks", get(modpacks))
        .route("/modpacks/{handle}/{slug}", get(modpack_project))
}

// --- Python semantics over JSON ---------------------------------------------

/// Where the Python view raises: the page is the 500 envelope.
#[derive(Debug, Clone, Copy)]
pub struct Broken;

pub type Py<T> = Result<T, Broken>;

pub static NONE: Value = Value::Null;
pub static EMPTY: Value = Value::String(String::new());
static ZERO: LazyLock<Value> = LazyLock::new(|| Value::from(0));

pub fn truthy(v: &Value) -> bool {
    super::flags::truthy(v)
}

/// `a or b`.
pub fn or<'a>(a: Option<&'a Value>, b: &'a Value) -> &'a Value {
    a.filter(|v| truthy(v)).unwrap_or(b)
}

pub fn py_str(v: &Value) -> String {
    crate::feeds::pyval::py_str(v)
}

/// `for x in (v or [])`: list items, dict keys, str characters.
pub fn py_iter(v: &Value) -> Py<Cow<'_, [Value]>> {
    Ok(match v {
        Value::Array(a) => Cow::Borrowed(a.as_slice()),
        Value::Object(m) => Cow::Owned(m.keys().map(|k| Value::String(k.clone())).collect()),
        Value::String(s) => Cow::Owned(s.chars().map(|c| Value::String(c.to_string())).collect()),
        v if !truthy(v) => Cow::Borrowed(&[]),
        _ => return Err(Broken),
    })
}

/// A value that must be a str for the method Python calls on it.
pub fn as_str(v: &Value) -> Py<&str> {
    v.as_str().ok_or(Broken)
}

/// `_dict(data)`.
fn dict(data: Option<&Value>) -> Option<&Map<String, Value>> {
    data.and_then(Value::as_object)
}

/// `_items(data, key)`: the dict rows of `data[key]` when that is a list.
pub fn items<'a>(data: Option<&'a Value>, key: &str) -> Vec<&'a Map<String, Value>> {
    dict(data).map(|d| rows(d, key)).unwrap_or_default()
}

fn rows<'a>(d: &'a Map<String, Value>, key: &str) -> Vec<&'a Map<String, Value>> {
    match d.get(key) {
        Some(Value::Array(rows)) => rows.iter().filter_map(Value::as_object).collect(),
        _ => Vec::new(),
    }
}

/// `f"{n:,}"` over the digits of an integer.
pub fn group(digits: &str) -> String {
    let (sign, body) = digits.strip_prefix('-').map_or(("", digits), |b| ("-", b));
    let mut out = String::with_capacity(body.len() + body.len() / 3 + 1);
    out.push_str(sign);
    for (i, c) in body.chars().enumerate() {
        if i > 0 && (body.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// `f"{x:,.{prec}f}"`.
pub fn grouped_fixed(x: f64, prec: usize) -> String {
    let s = format!("{x:.prec$}");
    match s.split_once('.') {
        Some((int, frac)) => format!("{}.{frac}", group(int)),
        None => group(&s),
    }
}

/// `f"{int(v):,}" if v == int(v) else f"{v:,.2f}".rstrip("0").rstrip(".")`.
pub fn float_num(f: f64) -> Py<String> {
    if !f.is_finite() {
        return Err(Broken);
    }
    if f == f.trunc() {
        return Ok(if f == 0.0 { "0".into() } else { group(&format!("{f:.0}")) });
    }
    Ok(grouped_fixed(f, 2).trim_end_matches('0').trim_end_matches('.').to_owned())
}

/// `ssr.num`: `12345` -> `12,345`; anything non-numeric -> an em dash.
pub fn num(v: &Value) -> Py<String> {
    match v {
        Value::Number(n) if n.is_i64() || n.is_u64() => Ok(group(&n.to_string())),
        Value::Number(n) => float_num(n.as_f64().unwrap_or(f64::NAN)),
        _ => Ok("—".into()),
    }
}

/// `ssr._unix`: seconds from a number or an ISO-8601 string.
fn unix(v: Option<&Value>) -> Option<i64> {
    match v? {
        Value::Number(n) => match n.as_i64() {
            Some(i) => Some(i),
            None => n.as_f64().filter(|f| f.is_finite()).map(|f| f.trunc() as i64),
        },
        Value::String(s) if !s.is_empty() => {
            let d = kiwi_core::pydt::fromisoformat(&s.replace('Z', "+00:00")).ok()?;
            Some(kiwi_core::pydt::total_seconds_int(d.wall_us() - i128::from(d.offset_us.unwrap_or(0))))
        }
        _ => None,
    }
}

const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// `ssr.date`: `12 Mar 2026`.
pub fn date(v: Option<&Value>) -> String {
    unix(v).map_or_else(String::new, |u| {
        let w = kiwi_core::pydt::Wall::from_secs(u);
        format!("{:02} {} {}", w.day, MONTHS[(w.month - 1) as usize], w.year)
    })
}

/// `ssr.datetime_attr`: ISO-8601 UTC for a `<time datetime>`.
pub fn datetime_attr(v: Option<&Value>) -> String {
    unix(v).map_or_else(String::new, |u| {
        let w = kiwi_core::pydt::Wall::from_secs(u);
        format!("{}-{:02}-{:02}T{:02}:{:02}:{:02}Z", w.year, w.month, w.day, w.hour, w.minute, w.second)
    })
}

static TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<[^>]+>").expect("regex"));
static LINK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"!?\[([^\]]*)\]\([^)]*\)").expect("regex"));
static MARK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[#*`_>~|]").expect("regex"));

/// The shared markdown/HTML -> one line cleanup of `ssr.text` and `_plain_excerpt`.
fn plain(md: Option<&Value>) -> Py<String> {
    let src = match md {
        Some(v) if truthy(v) => as_str(v)?,
        _ => "",
    };
    let t = TAG.replace_all(src, " ");
    let t = LINK.replace_all(&t, "${1}");
    let t = MARK.replace_all(&t, "");
    let mut out = String::with_capacity(t.len());
    let mut space = false;
    for c in t.chars() {
        if pystr::is_space(c) {
            space = true;
        } else {
            if space {
                out.push(' ');
                space = false;
            }
            out.push(c);
        }
    }
    if space {
        out.push(' ');
    }
    Ok(pystr::strip(&out).to_owned())
}

/// `ssr.text(md, limit)`: a plain excerpt, `…` when cut.
pub fn text(md: Option<&Value>, limit: usize) -> Py<String> {
    let t = plain(md)?;
    if t.chars().count() > limit {
        return Ok(format!("{}…", pystr::prefix(&t, limit).trim_end_matches(pystr::is_space)));
    }
    Ok(t)
}

/// `pages._plain_excerpt(md)`.
fn excerpt(md: Option<&Value>) -> Py<String> {
    Ok(pystr::prefix(&plain(md)?, 280).to_owned())
}

/// `[t for t in (v or []) if isinstance(t, str)][:n]`.
fn str_list(v: Option<&Value>, n: usize) -> Py<Vec<Value>> {
    Ok(py_iter(or(v, &NONE))?.iter().filter(|t| t.is_string()).take(n).cloned().collect())
}

// --- the first-paint fetch memo -----------------------------------------------

type Entry = (Instant, Option<Arc<Value>>);

const MAX_ENTRIES: usize = 512;

/// `ssr.cached`: keyed by path and sorted params, single-flight per key, a
/// failure kept for the TTL unless an older payload can stand in for it.
#[derive(Default)]
pub struct SsrCache {
    entries: Mutex<IndexMap<String, Entry>>,
    locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

impl SsrCache {
    fn fresh(&self, key: &str, ttl: f64) -> (Option<Entry>, bool) {
        let hit = self.entries.lock().unwrap_or_else(|e| e.into_inner()).get(key).cloned();
        let fresh = hit.as_ref().is_some_and(|(at, _)| at.elapsed().as_secs_f64() <= ttl);
        (hit, fresh)
    }

    pub async fn get(&self, api: &InternalApi, path: &str, params: &[(&str, String)], ttl: f64) -> Option<Arc<Value>> {
        let mut key = path.to_owned();
        if !params.is_empty() {
            let mut sorted: Vec<&(&str, String)> = params.iter().collect();
            sorted.sort_by(|a, b| a.0.cmp(b.0));
            key.push('?');
            key.push_str(&sorted.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("&"));
        }
        if let (Some((_, v)), true) = self.fresh(&key, ttl) {
            return v;
        }
        let lock = self.locks.lock().unwrap_or_else(|e| e.into_inner()).entry(key.clone()).or_default().clone();
        let _guard = lock.lock().await;
        let (hit, fresh) = self.fresh(&key, ttl);
        if let (Some((_, v)), true) = (&hit, fresh) {
            return v.clone();
        }
        let data = api.get(path, params, 3.0, &[]).await.filter(|v| !v.is_null()).map(Arc::new);
        if data.is_none()
            && let Some((_, v)) = hit
        {
            return v;
        }
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        if entries.len() >= MAX_ENTRIES {
            let stale: Vec<String> = entries.keys().take(MAX_ENTRIES / 4).cloned().collect();
            let mut locks = self.locks.lock().unwrap_or_else(|e| e.into_inner());
            for k in stale {
                entries.shift_remove(&k);
                locks.remove(&k);
            }
        }
        entries.insert(key, (Instant::now(), data.clone()));
        data
    }
}

fn params(pairs: &[(&'static str, &str)]) -> Vec<(&'static str, String)> {
    pairs.iter().map(|(k, v)| (*k, (*v).to_owned())).collect()
}

fn total(data: Option<&Value>) -> Value {
    dict(data).and_then(|d| d.get("total")).cloned().unwrap_or(Value::Null)
}

// --- view models ----------------------------------------------------------------

fn mod_url(m: &Map<String, Value>) -> String {
    format!("/mods/{}/{}", py_str(&m["handle"]), py_str(&m["slug"]))
}

fn listed(m: &Map<String, Value>) -> bool {
    m.get("handle").is_some_and(truthy) && m.get("slug").is_some_and(truthy)
}

fn author(m: &Map<String, Value>) -> Value {
    or(m.get("author"), or(m.get("owner_username"), &EMPTY)).clone()
}

/// `ssr._mod_card`.
fn mod_card(m: &Map<String, Value>) -> Py<Value> {
    Ok(json!({
        "title": or(m.get("title"), &m["slug"]),
        "url": mod_url(m),
        "summary": text(m.get("summary"), 160)?,
        "author": author(m),
        "avatar": or(m.get("owner_avatar_url"), &EMPTY),
        "tags": str_list(m.get("tags"), 4)?,
        "downloads": num(or(m.get("download_count"), &ZERO))?,
        "stars": num(or(m.get("star_count"), &ZERO))?,
    }))
}

pub async fn mods_view(web: &Web) -> Py<Value> {
    let data =
        web.ssr.get(&web.api, "/site/mods/projects", &params(&[("sort", "recent"), ("limit", "24")]), 60.0).await;
    let data = data.as_deref();
    let cards = items(data, "items").into_iter().filter(|m| listed(m)).map(mod_card).collect::<Py<Vec<_>>>()?;
    Ok(json!({"mods": cards, "total": total(data)}))
}

pub async fn modpacks_view(web: &Web) -> Py<Value> {
    let data = web.ssr.get(&web.api, "/site/modpacks/projects", &params(&[("limit", "24")]), 60.0).await;
    let data = data.as_deref();
    let mut packs = Vec::new();
    for p in items(data, "items").into_iter().filter(|p| listed(p)) {
        packs.push(json!({
            "title": or(p.get("title"), &p["slug"]),
            "url": format!("/modpacks/{}/{}", py_str(&p["handle"]), py_str(&p["slug"])),
            "summary": text(p.get("summary"), 160)?,
            "author": or(p.get("owner_username"), &EMPTY),
            "downloads": num(or(p.get("download_count"), &ZERO))?,
        }));
    }
    Ok(json!({"modpacks": packs, "total": total(data)}))
}

pub async fn codexes_view(web: &Web) -> Py<Value> {
    let search = params(&[("sort", "name"), ("limit", "24")]);
    let (types, entries) = tokio::join!(
        web.ssr.get(&web.api, "/site/codexes/types", &[], 60.0),
        web.ssr.get(&web.api, "/site/codexes/search", &search, 60.0),
    );
    let mut tabs = Vec::new();
    for t in items(types.as_deref(), "items").into_iter().filter(|t| t.get("type").is_some_and(truthy)) {
        tabs.push(json!({"type": or(t.get("type"), &EMPTY), "count": num(or(t.get("count"), &ZERO))?}));
    }
    let rows: Vec<Value> = items(entries.as_deref(), "items")
        .into_iter()
        .filter(|e| e.get("name").is_some_and(truthy))
        .map(|e| {
            json!({
                "name": or(e.get("name"), &EMPTY),
                "type": or(e.get("type"), &EMPTY),
                "category": or(e.get("category"), &EMPTY),
            })
        })
        .collect();
    Ok(json!({"types": tabs, "entries": rows, "total": total(entries.as_deref())}))
}

pub async fn market_view(web: &Web) -> Py<Value> {
    let recent = params(&[("limit", "24"), ("sort", "-last_seen")]);
    let (names, listings) = tokio::join!(
        web.ssr.get(&web.api, "/site/market/items", &[], 60.0),
        web.ssr.get(&web.api, "/site/market/listings", &recent, 60.0),
    );
    let raw = dict(names.as_deref()).and_then(|d| d.get("items"));
    let names: Vec<Value> = py_iter(or(raw, &NONE))?.iter().filter(|n| n.is_string()).cloned().collect();
    let mut rows = Vec::new();
    for r in items(listings.as_deref(), "items").into_iter().filter(|r| r.get("name").is_some_and(truthy)) {
        rows.push(json!({
            "name": or(r.get("name"), &EMPTY),
            "each": num(r.get("price_each").unwrap_or(&NONE))?,
            "stack": num(r.get("stack").unwrap_or(&NONE))?,
            "total": num(r.get("price").unwrap_or(&NONE))?,
            "date": date(r.get("last_seen")),
        }));
    }
    let count = if names.is_empty() { Value::Null } else { Value::from(group(&names.len().to_string())) };
    Ok(json!({"item_names": &names[..names.len().min(200)], "item_count": count, "listings": rows}))
}

/// `re.sub(r"^\$?(StoreCategory_)?", "", raw)`.
fn store_label(raw: &str) -> &str {
    let s = raw.strip_prefix('$').unwrap_or(raw);
    s.strip_prefix("StoreCategory_").unwrap_or(s)
}

pub async fn store_view(web: &Web) -> Py<Value> {
    let live = params(&[("limit", "24"), ("active", "true")]);
    let (cats, products) = tokio::join!(
        web.ssr.get(&web.api, "/site/store/categories", &[], 60.0),
        web.ssr.get(&web.api, "/site/store/products", &live, 60.0),
    );
    let mut categories = Vec::new();
    for c in items(cats.as_deref(), "items") {
        let label = or(c.get("label"), or(c.get("name"), &EMPTY));
        if truthy(label) {
            categories.push(json!({"label": store_label(as_str(label)?)}));
        }
    }
    let rows: Vec<Value> = items(products.as_deref(), "items")
        .into_iter()
        .filter(|p| truthy(or(p.get("name"), or(p.get("code"), &NONE))))
        .map(|p| {
            json!({
                "name": or(p.get("name"), or(p.get("code"), &EMPTY)),
                "price": or(p.get("price_string"), &EMPTY),
                "kind": or(p.get("kind"), &EMPTY),
            })
        })
        .collect();
    Ok(json!({"categories": categories, "products": rows, "total": total(products.as_deref())}))
}

/// `ssr._release_file`.
fn release_file(r: &Map<String, Value>) -> Py<Value> {
    let name = or(r.get("tmod_filename"), &EMPTY);
    let size = match r.get("tmod_size") {
        Some(Value::Bool(b)) => f64::from(u8::from(*b)),
        Some(Value::Number(n)) if n.is_i64() || n.is_u64() => n.as_f64().unwrap_or(0.0),
        _ => return Ok(name.clone()),
    };
    if !truthy(name) {
        return Ok(name.clone());
    }
    let name = py_str(name);
    Ok(Value::from(if size >= 1_048_576.0 {
        format!("{name} · {:.1} MB", size / 1_048_576.0)
    } else {
        format!("{name} · {} KB", ((size / 1024.0).round_ties_even() as i64).max(1))
    }))
}

/// `ssr.mod_project_view(project)`.
pub fn mod_project_view(api_base: &str, p: &Map<String, Value>) -> Py<Value> {
    if p.is_empty() {
        return Ok(json!({}));
    }
    let releases: Vec<&Map<String, Value>> = match or(p.get("releases"), &NONE) {
        Value::Array(a) => a
            .iter()
            .filter_map(Value::as_object)
            .filter(|r| r.get("status").is_none_or(|s| s.as_str() == Some("published")))
            .collect(),
        other => {
            py_iter(other)?;
            Vec::new()
        }
    };
    let latest = releases.first().copied();
    let banner = match p.get("banner_sha") {
        Some(Value::String(b)) => format!("{api_base}/site/mods/image/{}?w=1416", quote(b, "/")),
        _ => String::new(),
    };
    let avatar = match p.get("owner_avatar_url") {
        Some(Value::String(a)) if a.starts_with("https://") => a.clone(),
        _ => String::new(),
    };
    let download = match latest.and_then(|l| l.get("id")).filter(|v| truthy(v)) {
        Some(id) => format!("{api_base}/site/mods/releases/{}/download", quote(&py_str(id), "/")),
        None => String::new(),
    };
    let mut rows = Vec::new();
    for r in releases.iter().take(10) {
        rows.push(json!({
            "title": or(r.get("title"), or(r.get("tag"), &EMPTY)),
            "date": date(r.get("published_at")),
            "datetime": datetime_attr(r.get("published_at")),
            "changelog": text(r.get("changelog"), 400)?,
        }));
    }
    Ok(json!({
        "title": or(p.get("title"), or(p.get("slug"), &EMPTY)),
        "banner_url": banner,
        "source_visible": p.get("source_visible").is_some_and(truthy),
        "stray": p.get("is_stray").is_some_and(truthy),
        "avatar": avatar,
        "download_url": download,
        "release_tag": or(latest.and_then(|l| l.get("tag")), &EMPTY),
        "summary": text(p.get("summary"), 300)?,
        "description": text(p.get("description"), 1200)?,
        "author": author(p),
        "owner": or(p.get("owner_username"), &EMPTY),
        "tags": str_list(p.get("tags"), 8)?,
        "downloads": num(or(p.get("download_count"), &ZERO))?,
        "stars": num(or(p.get("star_count"), &ZERO))?,
        "updated": date(p.get("updated_at")),
        "datetime": datetime_attr(p.get("updated_at")),
        "releases": rows,
        "file": match latest {
            Some(r) => release_file(r)?,
            None => EMPTY.clone(),
        },
    }))
}

fn number(v: &Value) -> Py<f64> {
    match v {
        Value::Bool(b) => Ok(f64::from(u8::from(*b))),
        Value::Number(n) => Ok(n.as_f64().unwrap_or(0.0)),
        _ => Err(Broken),
    }
}

/// `ssr.mod_related_view(profile, slug, title)`.
pub fn mod_related_view(profile: Option<&Value>, slug: &str, title: &Value) -> Py<Value> {
    let line = match title {
        Value::String(t) => t.split_once(" - ").map(|(head, _)| head.to_owned()),
        Value::Array(_) | Value::Object(_) => None,
        _ => return Err(Broken),
    };
    let mods: Vec<&Map<String, Value>> =
        items(profile, "mods").into_iter().filter(|m| listed(m) && m["slug"].as_str() != Some(slug)).collect();
    let mut keyed = Vec::with_capacity(mods.len());
    for m in mods {
        let same_line = match line.as_deref().filter(|l| !l.is_empty()) {
            Some(l) => as_str(or(m.get("title"), &EMPTY))?.starts_with(&format!("{l} - ")),
            None => false,
        };
        keyed.push(((!same_line, -number(or(m.get("download_count"), &ZERO))?), m));
    }
    keyed.sort_by(|a, b| a.0.0.cmp(&b.0.0).then(a.0.1.partial_cmp(&b.0.1).unwrap_or(std::cmp::Ordering::Equal)));
    let rows: Vec<Value> = keyed
        .iter()
        .take(8)
        .map(|(_, m)| json!({"title": or(m.get("title"), &m["slug"]), "url": mod_url(m)}))
        .collect();
    Ok(json!({"mods": rows}))
}

/// `ssr.mod_profile_view(profile)`.
pub fn mod_profile_view(d: &Map<String, Value>) -> Py<Value> {
    if d.is_empty() {
        return Ok(json!({}));
    }
    let mut cards = rows(d, "mods").into_iter().filter(|m| listed(m)).map(mod_card).collect::<Py<Vec<_>>>()?;
    cards.truncate(200);
    Ok(json!({
        "name": or(d.get("display_name"), or(d.get("username"), &EMPTY)),
        "tagline": text(d.get("tagline"), 200)?,
        "readme": text(d.get("readme"), 900)?,
        "mods": cards,
    }))
}

/// `ssr.modpack_project_view(pack)`.
pub fn modpack_project_view(p: &Map<String, Value>) -> Py<Value> {
    if p.is_empty() {
        return Ok(json!({}));
    }
    let variants = rows(p, "variants");
    let mut seen: Vec<String> = Vec::new();
    let mut mods = Vec::new();
    for variant in &variants {
        for m in rows(variant, "mods") {
            let title = or(m.get("title"), m.get("slug").unwrap_or(&NONE));
            let url = if listed(m) { mod_url(m) } else { String::new() };
            if !truthy(title) || seen.contains(&url) {
                continue;
            }
            seen.push(url.clone());
            mods.push(json!({"title": title, "url": url, "author": or(m.get("author"), &EMPTY)}));
        }
    }
    let labels: Vec<Value> =
        variants.iter().take(20).map(|v| or(v.get("label"), or(v.get("name"), &EMPTY)).clone()).collect();
    mods.truncate(80);
    Ok(json!({
        "title": or(p.get("title"), or(p.get("slug"), &EMPTY)),
        "summary": text(p.get("summary"), 300)?,
        "description": text(p.get("description"), 1200)?,
        "owner": or(p.get("owner_username"), &EMPTY),
        "downloads": num(or(p.get("download_count"), &ZERO))?,
        "updated": date(p.get("updated_at")),
        "variants": labels,
        "mods": mods,
    }))
}

// --- routes ---------------------------------------------------------------------

fn tpl(v: &Value) -> Tpl {
    Tpl::from_serialize(v)
}

/// The page, or the 500 a raising view model gives.
pub(super) fn page(
    web: &Web,
    flags: &Flags,
    uri: &Uri,
    headers: &HeaderMap,
    template: &str,
    ctx: Py<Vec<(&str, Tpl)>>,
) -> Response {
    match ctx {
        Ok(ctx) => render(web, flags, uri, headers, template, ctx),
        Err(Broken) => ApiError::internal().into_response(),
    }
}

fn ssr_page(web: &Web, flags: &Flags, uri: &Uri, headers: &HeaderMap, template: &str, view: Py<Value>) -> Response {
    page(web, flags, uri, headers, template, view.map(|v| vec![("ssr", tpl(&v))]))
}

async fn market(Gate(flags): Gate, State(web): State<Arc<Web>>, uri: Uri, headers: HeaderMap) -> Response {
    let view = market_view(&web).await;
    ssr_page(&web, &flags, &uri, &headers, "market.html", view)
}

async fn store(Gate(flags): Gate, State(web): State<Arc<Web>>, uri: Uri, headers: HeaderMap) -> Response {
    let view = store_view(&web).await;
    ssr_page(&web, &flags, &uri, &headers, "store.html", view)
}

async fn codexes(Gate(flags): Gate, State(web): State<Arc<Web>>, uri: Uri, headers: HeaderMap) -> Response {
    let view = codexes_view(&web).await;
    ssr_page(&web, &flags, &uri, &headers, "codexes.html", view)
}

async fn mods(Gate(flags): Gate, State(web): State<Arc<Web>>, uri: Uri, headers: HeaderMap) -> Response {
    let view = mods_view(&web).await;
    ssr_page(&web, &flags, &uri, &headers, "mods.html", view)
}

async fn modpacks(Gate(flags): Gate, State(web): State<Arc<Web>>, uri: Uri, headers: HeaderMap) -> Response {
    let view = modpacks_view(&web).await;
    ssr_page(&web, &flags, &uri, &headers, "modpacks.html", view)
}

/// The two path params of `/{prefix}/{handle}/{slug}`, cut from the decoded path.
fn pair(uri: &Uri, prefix: &str) -> (String, String) {
    let path = scope_path(uri);
    let rest = path.strip_prefix(prefix).unwrap_or("");
    let (a, b) = rest.split_once('/').unwrap_or((rest, ""));
    (a.to_owned(), b.to_owned())
}

/// `_ld(data)`: JSON-LD with `<` escaped.
fn ld(graph: Vec<Value>) -> String {
    let doc = json!({"@context": "https://schema.org", "@graph": graph});
    crate::pyjson::dumps_spaced_unicode(&doc).replace('<', "\\u003c")
}

fn crumbs(trail: &[(&Value, &str)]) -> Value {
    let rows: Vec<Value> = trail
        .iter()
        .enumerate()
        .map(|(i, (name, url))| json!({"@type": "ListItem", "position": i + 1, "name": name, "item": url}))
        .collect();
    json!({"@type": "BreadcrumbList", "itemListElement": rows})
}

/// `previews[0] if previews else None` over `project.get("preview_shas") or []`.
fn first_preview(v: Option<&Value>) -> Py<Value> {
    Ok(match or(v, &NONE) {
        Value::Array(a) => a.first().cloned().unwrap_or(Value::Null),
        Value::String(s) => s.chars().next().map_or(Value::Null, |c| Value::String(c.to_string())),
        v if !truthy(v) => Value::Null,
        _ => return Err(Broken),
    })
}

/// The Open Graph defaults of a project page.
struct Og {
    title: String,
    page_title: String,
    desc: String,
    image: String,
    image_alt: Value,
    author: Value,
    card: &'static str,
}

/// The shared head of `mods_project_page` / `modpack_project_page` over a fetched project.
fn og_for(
    web: &Web,
    p: &Map<String, Value>,
    slug: &str,
    kind: &str,
    fallback_desc: &str,
) -> Py<(Og, Value, String, Value)> {
    let slug_v = Value::from(slug);
    let title = or(p.get("title"), &slug_v).clone();
    let owner = or(p.get("owner_username"), &EMPTY).clone();
    let summary = match or(p.get("summary"), &EMPTY) {
        Value::String(s) => pystr::strip(s).to_owned(),
        _ => return Err(Broken),
    };
    let desc = if !summary.is_empty() {
        summary
    } else {
        let ex = excerpt(p.get("description"))?;
        if !ex.is_empty() {
            ex
        } else if truthy(&owner) {
            format!("A Trove {kind} by {}.", py_str(&owner))
        } else {
            fallback_desc.to_owned()
        }
    };
    let img = or(p.get("banner_sha"), &NONE).clone();
    let img = if truthy(&img) { img } else { first_preview(p.get("preview_shas"))? };
    let image = if truthy(&img) {
        format!("{}/site/mods/image/{}", web.api_base(), py_str(&img))
    } else {
        format!("{}/static/assets/favicon.png", web.app_base())
    };
    let t = py_str(&title);
    let og = Og {
        title: format!("{t} · Trove {kind}"),
        page_title: format!("{t} · Trove {kind} · Better Trove Tools"),
        desc: pystr::prefix(&desc, 300).to_owned(),
        image,
        image_alt: title.clone(),
        author: owner.clone(),
        card: if truthy(&img) { "summary_large_image" } else { "summary" },
    };
    Ok((og, title, desc, owner))
}

fn og_ctx(og: Og) -> Vec<(&'static str, Tpl)> {
    vec![
        ("page_title", Tpl::from(og.page_title)),
        ("og_title", Tpl::from(og.title)),
        ("og_desc", Tpl::from(og.desc)),
        ("og_image", Tpl::from(og.image)),
        ("og_image_alt", tpl(&og.image_alt)),
        ("og_author", tpl(&og.author)),
        ("twitter_card", Tpl::from(og.card)),
    ]
}

fn defaults(web: &Web, handle: &str, slug: &str, page_url: &str, kind: &str, desc: &str) -> Vec<(&'static str, Tpl)> {
    vec![
        ("slug", Tpl::from(slug)),
        ("handle", Tpl::from(handle)),
        ("og_page_url", Tpl::from(page_url)),
        ("page_title", Tpl::from(format!("{slug} · Trove {kind} · Better Trove Tools"))),
        ("og_title", Tpl::from(format!("{slug} · Trove {kind}"))),
        ("og_desc", Tpl::from(desc)),
        ("og_image", Tpl::from(format!("{}/static/assets/favicon.png", web.app_base()))),
        ("og_image_alt", Tpl::from("Better Trove Tools")),
        ("og_author", Tpl::from("")),
        ("twitter_card", Tpl::from("summary")),
    ]
}

fn update(ctx: &mut Vec<(&'static str, Tpl)>, more: Vec<(&'static str, Tpl)>) {
    for (k, v) in more {
        match ctx.iter_mut().find(|(key, _)| *key == k) {
            Some(slot) => slot.1 = v,
            None => ctx.push((k, v)),
        }
    }
}

const MOD_DESC: &str = "A Trove mod shared on the Better Trove Tools Mods Hub.";

async fn mod_project(Gate(flags): Gate, State(web): State<Arc<Web>>, uri: Uri, headers: HeaderMap) -> Response {
    let (handle, slug) = pair(&uri, "/mods/");
    let ctx = mod_project_ctx(&web, &handle, &slug).await;
    page(&web, &flags, &uri, &headers, "mods_project.html", ctx)
}

async fn mod_project_ctx(web: &Web, handle: &str, slug: &str) -> Py<Vec<(&'static str, Tpl)>> {
    let app = web.app_base();
    let page_url = format!("{app}/mods/{handle}/{slug}");
    let mut ctx = defaults(web, handle, slug, &page_url, "mod", MOD_DESC);
    let path = format!("/site/mods/projects/{}/{}", quote(handle, "/"), quote(slug, "/"));
    let Some(Value::Object(project)) = web.api.get(&path, &[], 8.0, &[]).await else { return Ok(ctx) };
    let (og, title, desc, owner) = og_for(web, &project, slug, "mod", MOD_DESC)?;
    let image = og.image.clone();
    let has_image = og.card == "summary_large_image";
    update(&mut ctx, og_ctx(og));
    ctx.push(("ssr", tpl(&mod_project_view(web.api_base(), &project)?)));
    let profile = web.ssr.get(&web.api, &format!("/site/mods/profile/{}", quote(handle, "/")), &[], 600.0).await;
    ctx.push(("related", tpl(&mod_related_view(profile.as_deref(), slug, &title)?)));
    let handle_v = Value::from(handle);
    let author_name = or(Some(&owner), &handle_v);
    let mut app_ld = json!({
        "@type": "SoftwareApplication", "name": title, "url": page_url,
        "description": pystr::prefix(&desc, 300), "applicationCategory": "GameApplication",
        "applicationSubCategory": "Trove mod", "operatingSystem": "Windows",
        "isAccessibleForFree": true,
        "offers": {"@type": "Offer", "price": 0, "priceCurrency": "USD"},
        "author": {"@type": "Person", "name": author_name, "url": format!("{app}/mods/{handle}")},
        "isPartOf": {"@id": format!("{app}/#website")},
    });
    let m = app_ld.as_object_mut().ok_or(Broken)?;
    if has_image {
        m.insert("image".into(), Value::from(image));
    }
    if let Some(v) = project.get("created_at").filter(|v| truthy(v)) {
        m.insert("datePublished".into(), v.clone());
    }
    let modified = or(project.get("last_release_at"), project.get("updated_at").unwrap_or(&NONE));
    if truthy(modified) {
        m.insert("dateModified".into(), modified.clone());
    }
    if let Some(dl) = project.get("download_count")
        && (dl.is_boolean() || dl.is_i64() || dl.is_u64())
    {
        m.insert(
            "interactionStatistic".into(),
            json!({"@type": "InteractionCounter", "interactionType": "https://schema.org/DownloadAction",
                   "userInteractionCount": dl}),
        );
    }
    let hub = format!("{app}/mods");
    let profile_url = format!("{app}/mods/{handle}");
    let trail = crumbs(&[(&Value::from("Mods Hub"), &hub), (author_name, &profile_url), (&title, &page_url)]);
    ctx.push(("ld", Tpl::from(ld(vec![app_ld, trail]))));
    Ok(ctx)
}

async fn mod_profile(
    Gate(flags): Gate,
    State(web): State<Arc<Web>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    let path = scope_path(&uri);
    let handle = path.strip_prefix("/mods/").unwrap_or("").to_owned();
    let data = web.api.get(&format!("/site/mods/profile/{}", quote(&handle, "/")), &[], 8.0, &[]).await;
    let Some(Value::Object(data)) = data else {
        return http_exception(StatusCode::NOT_FOUND, Some("No such modder."), &method, &url_path(&uri), &headers);
    };
    let ctx = mod_profile_ctx(&web, &handle, &data);
    page(&web, &flags, &uri, &headers, "mods_profile.html", ctx)
}

fn mod_profile_ctx(web: &Web, handle: &str, data: &Map<String, Value>) -> Py<Vec<(&'static str, Tpl)>> {
    let app = web.app_base();
    let page_url = format!("{app}/mods/{handle}");
    let handle_v = Value::from(handle);
    let name = or(data.get("display_name"), &handle_v).clone();
    let tagline = match or(data.get("tagline"), &EMPTY) {
        Value::String(s) => pystr::strip(s).to_owned(),
        _ => return Err(Broken),
    };
    let desc = if !tagline.is_empty() {
        tagline
    } else {
        let ex = excerpt(data.get("readme"))?;
        if ex.is_empty() { format!("{}'s mods on the Better Trove Tools Mods Hub.", py_str(&name)) } else { ex }
    };
    let img = or(data.get("banner_url"), data.get("avatar_url").unwrap_or(&NONE));
    let short = pystr::prefix(&desc, 300).to_owned();
    let n = py_str(&name);
    let mut ctx = vec![
        ("handle", Tpl::from(handle)),
        ("og_page_url", Tpl::from(page_url.clone())),
        ("page_title", Tpl::from(format!("{n} · Trove modder · Better Trove Tools"))),
        ("og_title", Tpl::from(format!("{n} · Trove modder"))),
        ("og_desc", Tpl::from(short.clone())),
        (
            "og_image",
            if truthy(img) { tpl(img) } else { Tpl::from(format!("{}/static/assets/favicon.png", web.app_base())) },
        ),
        ("og_image_alt", tpl(&name)),
        ("og_author", tpl(&name)),
        (
            "twitter_card",
            Tpl::from(if data.get("banner_url").is_some_and(truthy) { "summary_large_image" } else { "summary" }),
        ),
        ("ssr", tpl(&mod_profile_view(data)?)),
    ];
    let mut person =
        json!({"@type": "Person", "name": name, "alternateName": handle, "url": page_url, "description": short});
    let pm = person.as_object_mut().ok_or(Broken)?;
    if let Some(a) = data.get("avatar_url").filter(|v| truthy(v)) {
        pm.insert("image".into(), a.clone());
    }
    if let Some(w) = data.get("website_url").filter(|v| truthy(v)) {
        pm.insert("sameAs".into(), json!([w]));
    }
    let mut profile = json!({"@type": "ProfilePage", "url": page_url, "mainEntity": person});
    if let Some(j) = data.get("joined_at").filter(|v| truthy(v)) {
        profile.as_object_mut().ok_or(Broken)?.insert("dateCreated".into(), j.clone());
    }
    let hub = format!("{app}/mods");
    let trail = crumbs(&[(&Value::from("Mods Hub"), &hub), (&name, &page_url)]);
    ctx.push(("ld", Tpl::from(ld(vec![profile, trail]))));
    Ok(ctx)
}

const PACK_DESC: &str = "A Trove modpack shared on Better Trove Tools.";

async fn modpack_project(Gate(flags): Gate, State(web): State<Arc<Web>>, uri: Uri, headers: HeaderMap) -> Response {
    let (handle, slug) = pair(&uri, "/modpacks/");
    let ctx = modpack_project_ctx(&web, &handle, &slug).await;
    page(&web, &flags, &uri, &headers, "modpacks_project.html", ctx)
}

async fn modpack_project_ctx(web: &Web, handle: &str, slug: &str) -> Py<Vec<(&'static str, Tpl)>> {
    let page_url = format!("{}/modpacks/{handle}/{slug}", web.app_base());
    let mut ctx = defaults(web, handle, slug, &page_url, "modpack", PACK_DESC);
    let path = format!("/site/modpacks/projects/{}/{}", quote(handle, "/"), quote(slug, "/"));
    let Some(Value::Object(pack)) = web.api.get(&path, &[], 8.0, &[]).await else { return Ok(ctx) };
    let (og, ..) = og_for(web, &pack, slug, "modpack", PACK_DESC)?;
    update(&mut ctx, og_ctx(og));
    ctx.push(("ssr", tpl(&modpack_project_view(&pack)?)));
    Ok(ctx)
}

#[cfg(test)]
mod unit {
    use super::*;

    #[test]
    fn num_matches_python() {
        for (v, want) in [
            (json!(1234567), "1,234,567"),
            (json!(-1234), "-1,234"),
            (json!(1234.5), "1,234.5"),
            (json!(0.125), "0.12"),
            (json!(9.999), "10"),
            (json!(-0.001), "-0"),
            (json!(-0.0), "0"),
            (json!(1e16), "10,000,000,000,000,000"),
            (json!(2.0), "2"),
            (json!(true), "—"),
            (json!("12"), "—"),
            (json!(null), "—"),
        ] {
            assert_eq!(num(&v).unwrap(), want, "{v}");
        }
    }

    #[test]
    fn text_cuts_like_python() {
        let v = json!("  a  <b>x</b> [l](u) **c**  ");
        assert_eq!(text(Some(&v), 220).unwrap(), "a x l c");
        let long = json!("word ".repeat(10));
        assert_eq!(text(Some(&long), 12).unwrap(), "word word wo…");
        assert_eq!(text(Some(&json!("ab cd")), 3).unwrap(), "ab…");
        assert!(text(Some(&json!(5)), 3).is_err());
        assert_eq!(text(Some(&json!(0)), 3).unwrap(), "");
    }

    #[test]
    fn dates_like_python() {
        assert_eq!(date(Some(&json!(1760000000))), "09 Oct 2025");
        assert_eq!(datetime_attr(Some(&json!("2026-10-01T10:00:00.5+02:00"))), "2026-10-01T08:00:00Z");
        assert_eq!(date(Some(&json!("2026-10-01"))), "01 Oct 2026");
        assert_eq!(date(Some(&json!("nope"))), "");
        assert_eq!(date(Some(&json!(true))), "");
    }
}

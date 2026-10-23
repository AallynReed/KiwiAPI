//! `app/site/ssr.py`: the fetch memo (`cached`), the plain-text formatters
//! and the first-paint view models of the live pages, built from the same
//! `/site/*` JSON the api serves to `internal_get` (timeout 3s).

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use indexmap::IndexMap;
use kiwi_core::pystr;
use regex::Regex;
use serde_json::{Map, Value as Json, json};

use crate::web::flags::{Flags, truthy};
use crate::web::internal::InternalApi;

pub const ROWS: usize = 24;
const MAX_ENTRIES: usize = 512;

type Data = Option<Arc<Json>>;

struct Entry {
    at: Instant,
    data: Data,
}

#[derive(Default)]
struct Memo {
    cache: IndexMap<String, Entry>,
    locks: HashMap<String, Arc<tokio::sync::Mutex<()>>>,
}

static MEMO: LazyLock<Mutex<Memo>> = LazyLock::new(Mutex::default);

/// A query parameter: its `str()` for the memo key and its httpx form.
#[derive(Clone, Copy)]
pub enum P<'a> {
    S(&'a str),
    I(i64),
    J(&'a Json),
}

impl P<'_> {
    fn memo(&self) -> String {
        match self {
            P::S(s) => (*s).to_owned(),
            P::I(i) => i.to_string(),
            P::J(j) => pstr(j),
        }
    }

    fn query(&self) -> String {
        match self {
            P::J(Json::Bool(b)) => if *b { "true" } else { "false" }.to_owned(),
            P::J(Json::Null) => String::new(),
            other => other.memo(),
        }
    }
}

fn memo_key(path: &str, params: &[(&str, P)]) -> String {
    if params.is_empty() {
        return path.to_owned();
    }
    let mut sorted: Vec<&(&str, P)> = params.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(b.0));
    let q: Vec<String> = sorted.iter().map(|(k, v)| format!("{k}={}", v.memo())).collect();
    format!("{path}?{}", q.join("&"))
}

fn memo() -> std::sync::MutexGuard<'static, Memo> {
    MEMO.lock().unwrap_or_else(|e| e.into_inner())
}

/// `cached(fetch, path, params, ttl)`: single-flight per key, a failed fetch
/// keeps the last payload, oldest quarter evicted at 512 entries. The key
/// carries the api base so separate apps (tests) never share entries.
pub async fn cached(api: &InternalApi, path: &str, params: &[(&str, P<'_>)], ttl: f64) -> Data {
    let key = api.url(&memo_key(path, params));
    let ttl = Duration::from_secs_f64(ttl);
    let lock = {
        let mut m = memo();
        if let Some(e) = m.cache.get(&key)
            && e.at.elapsed() <= ttl
        {
            return e.data.clone();
        }
        m.locks.entry(key.clone()).or_default().clone()
    };
    let _flight = lock.lock().await;
    let hit = {
        let m = memo();
        match m.cache.get(&key) {
            Some(e) if e.at.elapsed() <= ttl => return e.data.clone(),
            Some(e) => Some(e.data.clone()),
            None => None,
        }
    };
    let query: Vec<(&str, String)> = params.iter().map(|(k, v)| (*k, v.query())).collect();
    let data = api.get(path, &query, 3.0, &[]).await.map(Arc::new);
    if data.is_none()
        && let Some(last) = hit
    {
        return last;
    }
    let mut m = memo();
    if m.cache.len() >= MAX_ENTRIES {
        let stale: Vec<String> = m.cache.drain(..MAX_ENTRIES / 4).map(|(k, _)| k).collect();
        for k in stale {
            m.locks.remove(&k);
        }
    }
    m.cache.insert(key, Entry { at: Instant::now(), data: data.clone() });
    data
}

// --- Python value semantics over JSON ----------------------------------------

static EMPTY: LazyLock<Map<String, Json>> = LazyLock::new(Map::new);

/// `str(v)`.
pub fn pstr(v: &Json) -> String {
    match v {
        Json::String(s) => s.clone(),
        other => crate::web::templates::py_str(&minijinja::Value::from_serialize(other)),
    }
}

fn t(v: Option<&Json>) -> bool {
    v.is_some_and(truthy)
}

/// `v or default`.
fn or(v: Option<&Json>, default: impl Into<Json>) -> Json {
    match v {
        Some(x) if truthy(x) => x.clone(),
        _ => default.into(),
    }
}

/// `a or b or default`.
fn or2(a: Option<&Json>, b: Option<&Json>, default: impl Into<Json>) -> Json {
    match a {
        Some(x) if truthy(x) => x.clone(),
        _ => or(b, default),
    }
}

fn g(m: &Map<String, Json>, k: &str) -> Json {
    m.get(k).cloned().unwrap_or(Json::Null)
}

/// `_dict(data)`.
fn dict(v: Option<&Json>) -> &Map<String, Json> {
    v.and_then(Json::as_object).unwrap_or(&EMPTY)
}

/// `_items(data, key)`.
fn items<'a>(v: Option<&'a Json>, key: &str) -> Vec<&'a Map<String, Json>> {
    match v.and_then(|d| d.get(key)) {
        Some(Json::Array(rows)) => rows.iter().filter_map(Json::as_object).collect(),
        _ => Vec::new(),
    }
}

/// Iterating `v or []`: list items, dict keys, string characters.
fn iter_py(v: Option<&Json>) -> Vec<Json> {
    match v {
        Some(Json::Array(a)) => a.clone(),
        Some(Json::Object(o)) => o.keys().map(|k| Json::String(k.clone())).collect(),
        Some(Json::String(s)) => s.chars().map(|c| Json::String(c.to_string())).collect(),
        _ => Vec::new(),
    }
}

fn objects(v: Option<&Json>) -> Vec<Map<String, Json>> {
    iter_py(v)
        .into_iter()
        .filter_map(|x| match x {
            Json::Object(o) => Some(o),
            _ => None,
        })
        .collect()
}

/// `isinstance(v, int)` (bools included).
fn is_int(v: Option<&Json>) -> bool {
    match v {
        Some(Json::Bool(_)) => true,
        Some(Json::Number(n)) => n.is_i64() || n.is_u64(),
        _ => false,
    }
}

/// `isinstance(v, (int, float))`.
fn is_num(v: Option<&Json>) -> bool {
    matches!(v, Some(Json::Bool(_) | Json::Number(_)))
}

fn rstrip(s: &str) -> &str {
    s.trim_end_matches(pystr::is_space)
}

/// `str.title()`.
fn title(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_cased = false;
    for c in s.chars() {
        if prev_cased {
            out.extend(c.to_lowercase());
        } else {
            out.extend(c.to_uppercase());
        }
        prev_cased = c.is_lowercase() || c.is_uppercase();
    }
    out
}

fn group_digits(int_part: &str) -> String {
    let (sign, digits) = match int_part.strip_prefix('-') {
        Some(d) => ("-", d),
        None => ("", int_part),
    };
    let mut out = String::with_capacity(digits.len() + digits.len() / 3 + 1);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    format!("{sign}{out}")
}

/// `num(v)`: `12,345`, non-integral floats to two trimmed decimals, `—` for
/// anything that isn't a number.
pub fn num(v: Option<&Json>) -> String {
    let Some(Json::Number(n)) = v else { return "—".into() };
    if let Some(i) = n.as_i64() {
        return group_digits(&i.to_string());
    }
    if let Some(u) = n.as_u64() {
        return group_digits(&u.to_string());
    }
    let f = n.as_f64().unwrap_or(f64::NAN);
    if !f.is_finite() {
        return "—".into();
    }
    if f == f.trunc() {
        let whole = if f.trunc() == 0.0 { 0.0 } else { f.trunc() };
        return group_digits(&format!("{whole:.0}"));
    }
    let fixed = format!("{f:.2}");
    let (int_part, frac) = fixed.split_once('.').unwrap_or((&fixed, ""));
    let s = format!("{}.{frac}", group_digits(int_part));
    s.trim_end_matches('0').trim_end_matches('.').to_owned()
}

/// `_unix(v)`.
fn unix(v: Option<&Json>) -> Option<i64> {
    match v? {
        Json::Number(n) => {
            if let Some(i) = n.as_i64() {
                return Some(i);
            }
            let f = n.as_f64()?;
            (f.is_finite() && f.abs() < 9.2e18).then(|| f.trunc() as i64)
        }
        Json::String(s) if !s.is_empty() => {
            let dt = kiwi_core::pydt::fromisoformat(&s.replace('Z', "+00:00")).ok()?;
            let us = dt.wall_us() - i128::from(dt.offset_us.unwrap_or(0));
            Some(kiwi_core::pydt::total_seconds_int(us))
        }
        _ => None,
    }
}

const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

fn gmtime(u: i64) -> (i64, i64, i64, i64, i64, i64) {
    let (days, secs) = (u.div_euclid(86_400), u.rem_euclid(86_400));
    let (y, m, d) = kiwi_core::pydt::civil_from_days(days);
    (y, m, d, secs / 3600, secs % 3600 / 60, secs % 60)
}

/// `date(v)`: `12 Mar 2026` (UTC).
pub fn date(v: Option<&Json>) -> String {
    unix(v).map_or_else(String::new, |u| {
        let (y, m, d, ..) = gmtime(u);
        format!("{d:02} {} {y}", MONTHS[(m - 1) as usize])
    })
}

/// `datetime_attr(v)`: ISO-8601 UTC for `<time datetime>`.
pub fn datetime_attr(v: Option<&Json>) -> String {
    unix(v).map_or_else(String::new, |u| {
        let (y, m, d, hh, mm, ss) = gmtime(u);
        format!("{y}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z")
    })
}

static TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<[^>]+>").expect("static regex"));
static LINK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"!?\[([^\]]*)\]\([^)]*\)").expect("static regex"));

/// `text(md, limit)`: markdown/HTML to a one-line plain excerpt.
pub fn text(md: Option<&Json>, limit: usize) -> String {
    let src = match md {
        Some(Json::String(s)) => s.as_str(),
        _ => "",
    };
    let t = TAG.replace_all(src, " ");
    let t = LINK.replace_all(&t, "${1}");
    let mut out = String::with_capacity(t.len());
    let mut in_space = false;
    for c in t.chars().filter(|c| !"#*`_>~|".contains(*c)) {
        if pystr::is_space(c) {
            if !in_space {
                out.push(' ');
            }
            in_space = true;
        } else {
            out.push(c);
            in_space = false;
        }
    }
    let t = pystr::strip(&out);
    if t.chars().count() > limit { format!("{}…", rstrip(pystr::prefix(t, limit))) } else { t.to_owned() }
}

// --- view models ---------------------------------------------------------------

fn buff(b: Option<&Json>, kicker: &str) -> Option<Json> {
    let b = b?.as_object()?;
    let name = match b.get("name") {
        Some(n @ Json::String(s)) if !pystr::strip(s).is_empty() => n,
        _ => return None,
    };
    let name = pstr(name);
    let shown = format!("{}{name}", format!("{} ", pstr(&or(b.get("emoji"), ""))).trim_start_matches(pystr::is_space));
    let strings = |v: Json| -> Vec<Json> { iter_py(Some(&v)).into_iter().filter(Json::is_string).take(4).collect() };
    Some(json!({
        "kicker": kicker,
        "name": shown,
        "title": name,
        "list": strings(or2(b.get("normal_buffs"), b.get("buffs"), Json::Null)),
        "patron": strings(or(b.get("premium_buffs"), Json::Null)),
    }))
}

fn buffs(rot: &Map<String, Json>) -> Vec<Json> {
    [buff(rot.get("daily_buff"), "Today's bonus"), buff(rot.get("weekly_buff"), "This week's bonus")]
        .into_iter()
        .flatten()
        .collect()
}

fn days(rotation: Option<&Json>) -> Vec<Json> {
    objects(rotation)
        .iter()
        .map(|d| {
            json!({
                "weekday": pystr::prefix(&pstr(&or(d.get("weekday"), "")), 3),
                "name": pstr(&or(d.get("name"), "")),
                "current": t(d.get("is_current")),
            })
        })
        .collect()
}

fn biomes(bs: Option<&Json>) -> Vec<Json> {
    objects(bs)
        .iter()
        .map(|b| json!({"name": pstr(&or(b.get("name"), "")), "icon": pstr(&or(b.get("icon"), ""))}))
        .collect()
}

fn island(rot: &Map<String, Json>) -> Json {
    let merchants: Vec<Json> = objects(rot.get("merchants"))
        .iter()
        .map(|m| {
            let schedule: Vec<Json> = match m.get("schedule") {
                Some(Json::Array(a)) => a
                    .iter()
                    .take(4)
                    .filter_map(Json::as_object)
                    .map(|s| json!({"starts_at": g(s, "starts_at"), "ends_at": g(s, "ends_at"), "biomes": biomes(s.get("biomes"))}))
                    .collect(),
                _ => Vec::new(),
            };
            json!({
                "id": g(m, "id"),
                "name": g(m, "name"),
                "active": t(m.get("active")),
                "state": g(m, "state"),
                "starts_at": g(m, "starts_at"),
                "ends_at": g(m, "ends_at"),
                "biomes": biomes(m.get("biomes")),
                "window": m.get("window").filter(|w| w.is_object()).cloned().unwrap_or(Json::Null),
                "schedule": schedule,
            })
        })
        .collect();
    let st = dict(rot.get("server_time"));
    json!({
        "now": g(st, "now_unix"),
        "daily_reset_at": g(st, "daily_reset_at"),
        "weekly_reset_at": g(st, "weekly_reset_at"),
        "merchants": merchants,
    })
}

fn chaos(c: Option<&Json>) -> Json {
    let Some(c) = c.and_then(Json::as_object) else { return Json::Null };
    let ends = c.get("ends_at");
    if !t(ends) {
        return Json::Null;
    }
    let name = match c.get("item").and_then(Json::as_object) {
        Some(item) => or(item.get("name"), "Featured item"),
        None => json!("Featured item"),
    };
    let unix = if is_int(ends) { ends.cloned().unwrap_or(Json::Null) } else { Json::Null };
    json!({"name": name, "ends_at": datetime_attr(ends), "ends_unix": unix})
}

fn merchant(m: &Map<String, Json>) -> Json {
    let active = t(m.get("active"));
    let biomes: Vec<Json> = iter_py(m.get("biomes"))
        .into_iter()
        .map(|b| match b {
            Json::String(_) => b,
            Json::Object(o) => or(o.get("name"), ""),
            _ => json!(""),
        })
        .take(3)
        .collect();
    json!({
        "name": or(m.get("name"), ""),
        "active": active,
        "badge": if active { or(m.get("state"), "Here") } else { json!("Away") },
        "biomes": biomes,
    })
}

fn merchants(rot: &Map<String, Json>) -> Vec<Json> {
    objects(rot.get("merchants")).iter().map(merchant).collect()
}

fn news(n: &Map<String, Json>) -> Json {
    json!({
        "title": or(n.get("title"), ""),
        "url": or(n.get("url"), ""),
        "category": or(n.get("category"), ""),
        "image": or(n.get("image"), ""),
        "date": date(n.get("published_at")),
        "datetime": datetime_attr(n.get("published_at")),
    })
}

/// `"Shop Offers" in (n.get("categories") or [])`.
fn shop_offer(n: &Map<String, Json>) -> bool {
    const SHOP: &str = "Shop Offers";
    match n.get("categories") {
        Some(Json::Array(a)) => a.iter().any(|c| c.as_str() == Some(SHOP)),
        Some(Json::String(s)) => s.contains(SHOP),
        Some(Json::Object(o)) => o.contains_key(SHOP),
        _ => false,
    }
}

fn video(v: &Map<String, Json>) -> Json {
    let thumb = match or2(v.get("thumbnail_url"), v.get("thumbnail"), "") {
        Json::String(s) => json!(s.replace("{width}", "440").replace("{height}", "248")),
        other => other,
    };
    json!({
        "title": or(v.get("title"), ""),
        "url": or(v.get("url"), ""),
        "channel": or(v.get("channel"), ""),
        "thumb": thumb,
        "date": date(v.get("published_at")),
    })
}

fn records(data: Option<&Json>) -> Vec<Json> {
    let d = dict(data);
    let mut out = Vec::new();
    for (key, kicker, level) in [
        ("trove_mastery", "Trove Mastery", true),
        ("geode_mastery", "Geode Mastery", true),
        ("power_rank", "Power Rank", false),
    ] {
        let Some(r) = d.get(key).and_then(Json::as_object) else { continue };
        let points = r.get("points");
        let has_points = points.is_some_and(|p| !p.is_null());
        out.push(json!({
            "key": key,
            "raw": if level { g(r, "level") } else { g(r, "value") },
            "points": if level && has_points { num(points) } else { String::new() },
            "kicker": kicker,
            "value": if level { format!("Level {}", num(r.get("level"))) } else { num(r.get("value")) },
            "meta": if level { format!("{} pts", num(points)) } else { "Highest across all classes".to_owned() },
            "holder": or(r.get("player_name"), ""),
        }));
    }
    out
}

fn mod_url(m: &Map<String, Json>) -> String {
    format!("/mods/{}/{}", pstr(&g(m, "handle")), pstr(&g(m, "slug")))
}

fn mod_rail(rows: &[&Map<String, Json>]) -> Vec<Json> {
    let mut per_author: HashMap<String, u32> = HashMap::new();
    let mut picked = Vec::new();
    for m in rows {
        if picked.len() >= 8 {
            break;
        }
        if !(t(m.get("handle")) && t(m.get("slug"))) {
            continue;
        }
        let key = pstr(&or2(m.get("author"), m.get("owner_username"), g(m, "handle"))).to_lowercase();
        let n = per_author.entry(key).or_insert(0);
        if *n >= 2 {
            continue;
        }
        *n += 1;
        picked.push(json!({
            "title": or(m.get("title"), g(m, "slug")),
            "author": or2(m.get("author"), m.get("owner_username"), ""),
            "url": mod_url(m),
        }));
    }
    picked
}

pub async fn home_view(api: &InternalApi, flags: &Flags) -> Json {
    let (want_lb, want_mods) = (flags.on("leaderboards_enabled"), flags.on("mods_hub_enabled"));
    let (rotations, news_data, videos, records_data, mods) = tokio::join!(
        cached(api, "/site/rotations", &[], 30.0),
        cached(api, "/site/feeds/news", &[("limit", P::I(16))], 300.0),
        cached(api, "/site/feeds/videos", &[("platform", P::S("youtube"))], 300.0),
        async { if want_lb { cached(api, "/site/leaderboards/records", &[], 900.0).await } else { None } },
        async {
            if want_mods {
                cached(api, "/site/mods/projects", &[("sort", P::S("recent")), ("limit", P::I(40))], 60.0).await
            } else {
                None
            }
        },
    );
    let rot = dict(rotations.as_deref());
    let news: Vec<Json> =
        items(news_data.as_deref(), "items").into_iter().filter(|n| !shop_offer(n)).map(news).take(12).collect();
    json!({
        "buffs": buffs(rot),
        "chaos": chaos(rot.get("chaos")),
        "merchants": merchants(rot),
        "days": days(rot.get("daily_rotation")),
        "island": island(rot),
        "news": news,
        "videos": items(videos.as_deref(), "items").into_iter().map(video).take(12).collect::<Vec<_>>(),
        "records": records(records_data.as_deref()),
        "mods": mod_rail(&items(mods.as_deref(), "items")),
    })
}

pub async fn updates_view(api: &InternalApi) -> Json {
    let (branches, versions) = tokio::join!(
        cached(api, "/site/updates/branches", &[], 60.0),
        cached(api, "/site/updates/live-us/versions", &[("limit", P::I(ROWS as i64))], 60.0),
    );
    let branches: Vec<Json> = items(branches.as_deref(), "items")
        .into_iter()
        .filter(|b| t(b.get("branch")))
        .map(|b| json!({"branch": or(b.get("branch"), ""), "current": or(b.get("current_version"), "")}))
        .collect();
    let zero = json!(0);
    let count = |v: &Map<String, Json>, k: &str| num(Some(&or(v.get(k), zero.clone())));
    let versions: Vec<Json> = items(versions.as_deref(), "items")
        .into_iter()
        .filter(|v| t(v.get("version_tag")))
        .map(|v| {
            json!({
                "tag": or(v.get("version_tag"), ""),
                "date": date(v.get("captured_at")),
                "datetime": datetime_attr(v.get("captured_at")),
                "added": count(v, "files_added"),
                "modified": count(v, "files_modified"),
                "removed": count(v, "files_removed"),
            })
        })
        .collect();
    json!({"branches": branches, "versions": versions})
}

pub async fn calendar_view(api: &InternalApi) -> Json {
    let (rotations, events) =
        tokio::join!(cached(api, "/site/rotations", &[], 30.0), cached(api, "/site/calendar/events", &[], 120.0),);
    let rot = dict(rotations.as_deref());
    let ev = |e: &Map<String, Json>| {
        json!({
            "name": or(e.get("name"), ""),
            "url": or(e.get("url"), ""),
            "category": or(e.get("category"), ""),
            "starts": date(e.get("starts_at")),
            "ends": date(e.get("ends_at")),
        })
    };
    let list = |key: &str| -> Vec<Json> { items(events.as_deref(), key).into_iter().map(ev).take(20).collect() };
    json!({
        "buffs": buffs(rot),
        "merchants": merchants(rot),
        "ongoing": list("ongoing"),
        "upcoming": list("upcoming"),
    })
}

pub async fn streams_view(api: &InternalApi) -> Json {
    let (youtube, twitch, news_data) = tokio::join!(
        cached(api, "/site/feeds/videos", &[("platform", P::S("youtube"))], 300.0),
        cached(api, "/site/feeds/videos", &[("platform", P::S("twitch"))], 300.0),
        cached(api, "/site/feeds/news", &[("limit", P::I(16))], 300.0),
    );
    let videos = |d: &Data| -> Vec<Json> { items(d.as_deref(), "items").into_iter().map(video).take(12).collect() };
    json!({
        "videos": videos(&youtube),
        "streams": videos(&twitch),
        "news": items(news_data.as_deref(), "items").into_iter().map(news).take(12).collect::<Vec<_>>(),
    })
}

pub async fn releases_view(api: &InternalApi) -> Json {
    let data = cached(api, "/site/btt/releases", &[("limit", P::I(ROWS as i64))], 180.0).await;
    let releases: Vec<Json> = items(data.as_deref(), "items")
        .into_iter()
        .filter(|r| t(r.get("tag_name")))
        .map(|r| {
            json!({
                "tag": or(r.get("tag_name"), ""),
                "name": or2(r.get("name"), r.get("tag_name"), ""),
                "channel": or(r.get("channel"), ""),
                "url": or(r.get("html_url"), ""),
                "date": date(r.get("published_at")),
                "datetime": datetime_attr(r.get("published_at")),
                "notes": text(r.get("body"), 240),
            })
        })
        .collect();
    json!({ "releases": releases })
}

pub async fn status_view(api: &InternalApi) -> Json {
    let data = cached(api, "/site/trove-status", &[], 30.0).await;
    let d = dict(data.as_deref());
    let overall = or(d.get("overall"), "unknown");
    let envs = dict(d.get("environments"));
    let headline = match overall.as_str() {
        Some("online") => "All Trove servers operational",
        Some("down") => "Trove is down",
        _ => "Checking Trove status…",
    };
    let environments: Vec<Json> = [("eu", "Live EU"), ("us", "Live US"), ("pts", "PTS")]
        .into_iter()
        .filter_map(|(key, label)| {
            let env = envs.get(key)?.as_object()?;
            Some(json!({"label": label, "state": title(&pstr(&or(env.get("status"), "unknown")))}))
        })
        .collect();
    json!({
        "headline": headline,
        "overall": overall,
        "environments": environments,
        "checked": date(d.get("checked_at")),
    })
}

pub async fn activity_view(api: &InternalApi) -> Json {
    let data = cached(api, "/site/leaderboards/activity", &[], 120.0).await;
    let d = dict(data.as_deref());
    let rollups: Vec<Json> = [("in the last 7 days", d.get("estimate_7d"))]
        .into_iter()
        .filter(|(_, v)| is_num(*v))
        .map(|(label, v)| json!({"label": label, "value": num(v)}))
        .collect();
    let day = d.get("estimate_24h");
    json!({
        "estimate": if is_num(day) { num(day) } else { String::new() },
        "rollups": rollups,
        "as_of": date(d.get("window_end")),
    })
}

fn as_f64(v: &Json) -> f64 {
    match v {
        Json::Bool(b) => f64::from(u8::from(*b)),
        Json::Number(n) => n.as_f64().unwrap_or(0.0),
        _ => 0.0,
    }
}

/// `f"{share * 100:.1f}%"`.
fn share(v: Option<&Json>) -> String {
    let pct = match v {
        Some(Json::Bool(b)) => f64::from(u8::from(*b)) * 100.0,
        Some(Json::Number(n)) => match n.as_i64() {
            Some(i) => (i128::from(i) * 100) as f64,
            None => n.as_f64().unwrap_or(0.0) * 100.0,
        },
        _ => return String::new(),
    };
    format!("{pct:.1}%")
}

pub async fn class_activity_view(api: &InternalApi) -> Json {
    let data = cached(api, "/site/leaderboards/class-activity/current", &[], 120.0).await;
    let mut rows: Vec<&Map<String, Json>> = items(data.as_deref(), "classes")
        .into_iter()
        .filter(|c| t(c.get("name")) && is_num(c.get("active_players")))
        .collect();
    rows.sort_by(|a, b| {
        as_f64(&b["active_players"]).partial_cmp(&as_f64(&a["active_players"])).unwrap_or(std::cmp::Ordering::Equal)
    });
    let classes: Vec<Json> = rows
        .iter()
        .take(40)
        .map(|c| {
            json!({
                "name": g(c, "name"),
                "players": num(c.get("active_players")),
                "share": if is_num(c.get("share")) { share(c.get("share")) } else { String::new() },
            })
        })
        .collect();
    let total = dict(data.as_deref()).get("total_active").filter(|v| truthy(v));
    json!({"classes": classes, "total": total.map_or_else(String::new, |v| num(Some(v)))})
}

pub async fn giveaways_view(api: &InternalApi) -> Json {
    let data = cached(api, "/site/giveaways", &[], 30.0).await;
    let giveaways: Vec<Json> = items(data.as_deref(), "items")
        .into_iter()
        .map(|gw| {
            let entries = gw.get("entry_count").filter(|v| !v.is_null());
            json!({
                "prize": or(gw.get("prize_name"), "Giveaway"),
                "title": or(gw.get("title"), ""),
                "description": text(gw.get("description"), 200),
                "status": or(gw.get("status"), ""),
                "entries": entries.map_or_else(String::new, |v| num(Some(v))),
                "ends": date(gw.get("ends_at")),
                "winner": or(gw.get("winner_username"), ""),
            })
        })
        .take(12)
        .collect();
    json!({ "giveaways": giveaways })
}

const ZAKROS_LINES: [&str; 3] = ["Zakros UI", "Zakros FX", "Zakros BP"];

pub async fn mod_stats_view(api: &InternalApi, app_base: &str) -> Json {
    let data = cached(api, "/site/mod-stats", &[], 600.0).await;
    let mut lines: IndexMap<String, Vec<(String, String)>> = IndexMap::new();
    for m in items(data.as_deref(), "mods") {
        let Some(Json::String(title)) = m.get("title").filter(|v| truthy(v)) else { continue };
        if !(t(m.get("handle")) && t(m.get("slug"))) {
            continue;
        }
        let line = title.split_once(" - ").map_or("Zakros", |(head, _)| head);
        lines.entry(line.to_owned()).or_default().push((title.clone(), mod_url(m)));
    }
    if lines.is_empty() {
        return json!({});
    }
    let rank = |name: &str| ZAKROS_LINES.iter().position(|l| *l == name).unwrap_or(99);
    let mut sorted: Vec<(String, Vec<(String, String)>)> = lines.into_iter().collect();
    sorted.sort_by(|a, b| (rank(&a.0), &a.0).cmp(&(rank(&b.0), &b.0)));
    let mut groups = Vec::new();
    let mut flat = Vec::new();
    for (line, mut mods) in sorted {
        mods.sort_by_cached_key(|(title, _)| title.to_lowercase());
        let list: Vec<Json> = mods.iter().map(|(title, url)| json!({"title": title, "url": url})).collect();
        flat.extend(mods);
        groups.push(json!({"line": line, "mods": list}));
    }
    let elements: Vec<Json> = flat
        .iter()
        .enumerate()
        .map(|(i, (title, url))| {
            json!({"@type": "ListItem", "position": i + 1, "name": title, "url": format!("{app_base}{url}")})
        })
        .collect();
    let ld = json!({
        "@context": "https://schema.org",
        "@type": "CollectionPage",
        "name": "Zakros UI, FX and BP mods for Trove",
        "url": format!("{app_base}/zakros-ui-stats"),
        "about": {"@type": "Thing", "name": "Zakros UI"},
        "mainEntity": {"@type": "ItemList", "numberOfItems": flat.len(), "itemListElement": elements},
    });
    json!({"groups": groups, "ld": crate::pyjson::dumps_spaced_unicode(&ld).replace('<', "\\u003c")})
}

pub async fn leaderboards_view(api: &InternalApi) -> Json {
    let stamps = cached(api, "/site/leaderboards/timestamps", &[("limit", P::I(1))], 120.0).await;
    let anchors = match stamps.as_deref() {
        Some(Json::Object(o)) => o.get("items"),
        _ => None,
    };
    let Some(anchor) = iter_py(anchors).into_iter().find(|a| is_int(Some(a))) else { return json!({}) };
    let boards = cached(api, "/site/leaderboards/boards", &[("created_at", P::J(&anchor))], 120.0).await;
    let rows = items(boards.as_deref(), "items");
    let Some(first) = rows.first() else { return json!({}) };
    let uuid = first.get("uuid").filter(|v| truthy(v));
    let entries = match uuid {
        Some(uuid) => {
            let path = format!("/site/leaderboards/{}/entries", pstr(uuid));
            cached(api, &path, &[("created_at", P::J(&anchor)), ("limit", P::I(25))], 120.0).await
        }
        None => None,
    };
    let boards: Vec<Json> = rows
        .iter()
        .filter(|b| t(b.get("name")))
        .take(100)
        .map(|b| {
            let n = b.get("entries").filter(|v| !v.is_null());
            json!({"name": or(b.get("name"), ""), "entries": n.map_or_else(String::new, |v| num(Some(v)))})
        })
        .collect();
    let entries: Vec<Json> = items(entries.as_deref(), "items")
        .into_iter()
        .filter(|e| t(e.get("player_name")))
        .map(|e| {
            let player = g(e, "player_name");
            json!({
                "rank": num(e.get("rank")),
                "player": player,
                "url": format!("/player/{}", crate::middleware::pyurl::quote(&pstr(&player), "/")),
                "score": num(e.get("score")),
            })
        })
        .collect();
    json!({
        "captured": date(Some(&anchor)),
        "datetime": datetime_attr(Some(&anchor)),
        "boards": boards,
        "board_name": or(first.get("name"), ""),
        "entries": entries,
    })
}

pub async fn player_view(api: &InternalApi, name: &str) -> Json {
    let path = format!("/site/leaderboards/players/{}/profile", crate::middleware::pyurl::quote(name, "/"));
    let data = cached(api, &path, &[], 120.0).await;
    let d = dict(data.as_deref());
    let summary = dict(d.get("summary"));
    let chip = |label: &str, key: &str, value: &dyn Fn(Option<&Json>) -> String| {
        t(summary.get(key)).then(|| json!({"label": label, "value": value(summary.get(key))}))
    };
    let chips: Vec<Json> = [
        chip("Best rank", "best_rank", &|v| format!("#{}", num(v))),
        chip("Leaderboards", "boards_appeared", &num),
        chip("Top 10s", "top10_count", &num),
        chip("Last seen", "latest_anchor", &date),
    ]
    .into_iter()
    .flatten()
    .collect();
    let rank = |v: Option<&Json>| if t(v) { format!("#{}", num(v)) } else { String::new() };
    let boards: Vec<Json> = items(data.as_deref(), "boards")
        .into_iter()
        .filter(|b| t(b.get("board_name")))
        .take(60)
        .map(|b| {
            let score = b.get("latest_score").filter(|v| !v.is_null());
            json!({
                "board": g(b, "board_name"),
                "category": or(b.get("category"), ""),
                "rank": rank(b.get("latest_rank")),
                "best": rank(b.get("best_rank")),
                "score": score.map_or_else(String::new, |v| num(Some(v))),
            })
        })
        .collect();
    json!({
        "canonical": or(d.get("player_name"), name),
        "verified": t(d.get("verified")),
        "summary": chips,
        "boards": boards,
    })
}

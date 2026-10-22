//! `app/trove/chaos.py`: the weekly chest response, the codex blueprint memo
//! behind its icon, and the Trovesaurus relay loop (`chaos`).

use std::{
    collections::HashMap,
    sync::{LazyLock, Mutex},
    time::Duration,
};

use chrono::{DateTime, Utc};
use kiwi_core::rotations as rot;
use serde_json::{Map, Value, json};

use super::{
    gamedata::{Invalid, Out},
    http, store,
};
use crate::{
    auth::pyutil::{int_of, strip},
    dt,
    features::py_truthy,
    jobs,
    middleware::pyurl::quote,
    pyjson,
    state::AppState,
};

pub const FEED: &str = "chaos_chest";
const CODEX_BRANCH: &str = "live-us";
const IMAGE_DIM: u32 = 256;

/// The chest before the item's blueprint is filled in.
#[derive(Clone, Debug, PartialEq)]
pub struct Chest {
    pub active: bool,
    pub starts_at: i64,
    pub ends_at: i64,
    pub seconds_remaining: i64,
    pub item: Option<Item>,
    pub fetched_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub name: String,
    pub identifier: Option<String>,
    pub blueprint: Option<String>,
}

/// The bot capture for the current week, in the relay's item shape.
pub fn capture_item(name: &str, week: (i64, i64)) -> Map<String, Value> {
    let v = json!({"name": name, "identifier": null, "blueprint": null, "start": week.0, "end": week.1});
    v.as_object().cloned().unwrap_or_default()
}

fn opt_int(v: Option<&Value>) -> Out<Option<i64>> {
    match v {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(n)) => n.as_i64().map(Some).ok_or(Invalid),
        Some(_) => Err(Invalid),
    }
}

fn opt_str(v: Option<&Value>) -> Out<Option<String>> {
    match v {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(Invalid),
    }
}

/// `chaos.build_response`: the computed week, overlaid with a cached item that
/// has not ended yet.
pub fn build(cached: Option<&Map<String, Value>>, fetched_at: Option<DateTime<Utc>>, now: i64) -> Out<Chest> {
    let now_ts = rot::unix(now);
    let (mut starts_at, mut ends_at) = rot::chaos_chest_window(now);
    let mut item = None;
    if let Some(c) = cached.filter(|c| c.get("name").is_some_and(py_truthy)) {
        let end = opt_int(c.get("end"))?;
        if end.is_none_or(|e| e > now_ts) {
            let name = match &c["name"] {
                Value::String(s) => s.clone(),
                _ => return Err(Invalid),
            };
            item =
                Some(Item { name, identifier: opt_str(c.get("identifier"))?, blueprint: opt_str(c.get("blueprint"))? });
            if let Some(start) = opt_int(c.get("start"))?.filter(|s| *s != 0) {
                starts_at = start;
            }
            if let Some(end) = end.filter(|e| *e != 0) {
                ends_at = end;
            }
        }
    }
    let fetched_at = if item.is_some() { fetched_at } else { None };
    Ok(Chest {
        active: starts_at <= now_ts && now_ts < ends_at,
        starts_at,
        ends_at,
        seconds_remaining: (ends_at - now_ts).max(0),
        item,
        fetched_at,
    })
}

/// `item_image_url`.
pub fn image_url(api_url: &str, blueprint: Option<&str>) -> Option<String> {
    let bp = blueprint.filter(|b| !b.is_empty())?;
    Some(format!("{}/v1/codexes/render?blueprint={}&dim={IMAGE_DIM}", api_url.trim_end_matches('/'), quote(bp, "")))
}

/// The served body once the blueprint is known (`resolved` is what
/// `resolve_item_blueprint` gave when the item carried none).
pub fn body(chest: &Chest, resolved: Option<String>, api_url: &str) -> Value {
    let item = chest.item.as_ref().map(|it| {
        let blueprint = match &it.blueprint {
            Some(b) if !b.is_empty() => Some(b.clone()),
            _ => resolved.clone(),
        };
        let image_url = image_url(api_url, blueprint.as_deref());
        json!({"name": it.name, "identifier": it.identifier, "blueprint": blueprint, "image_url": image_url})
    });
    json!({
        "active": chest.active,
        "starts_at": chest.starts_at,
        "ends_at": chest.ends_at,
        "seconds_remaining": chest.seconds_remaining,
        "item": item,
        "fetched_at": chest.fetched_at.as_ref().map(dt::pydantic),
    })
}

pub fn needs_blueprint(chest: &Chest) -> Option<&str> {
    let it = chest.item.as_ref()?;
    it.blueprint.as_deref().is_none_or(str::is_empty).then_some(it.name.as_str())
}

// --- blueprint memo ----------------------------------------------------------------

/// Per process and never invalidated; misses are cached too.
static BP_CACHE: LazyLock<Mutex<HashMap<String, Option<String>>>> = LazyLock::new(Default::default);

/// `_item_role_rank`: lower is more representative of the tradable item.
pub fn item_role_rank(path: &str) -> u8 {
    let p = path.to_lowercase();
    let is_item = p.starts_with("prefabs/item/");
    if is_item && !p.contains("/mount/") {
        return if p.contains("_debug") || p.contains("_notrade") { 1 } else { 0 };
    }
    if is_item {
        2
    } else if p.starts_with("prefabs/collections/") {
        3
    } else {
        4
    }
}

/// `pg_store.blueprints_for_names` for one lower-cased name: the best-ranked
/// role's blueprint when that role is unanimous.
pub fn pick_blueprint(rows: &[(u8, String)]) -> Option<String> {
    let best = rows.iter().map(|(r, _)| *r).min()?;
    let mut top = rows.iter().filter(|(r, _)| *r == best).map(|(_, b)| b);
    let first = top.next()?;
    top.all(|b| b == first).then(|| first.clone())
}

async fn lookup_blueprint(state: &AppState, key: &str) -> anyhow::Result<Option<String>> {
    if !state.settings().postgres_enabled() {
        return Ok(None);
    }
    let pool = state.inner.db.pg.as_ref().ok_or_else(|| anyhow::anyhow!("postgres not connected"))?;
    let con = pool.get().await?;
    let rows = con
        .query(
            "SELECT lower(name) AS lname, blueprint, path FROM codex_entry \
             WHERE branch = $1 AND blueprint IS NOT NULL AND blueprint <> '' \
             AND lower(name) = ANY($2::text[])",
            &[&CODEX_BRANCH, &vec![key.to_lowercase()]],
        )
        .await?;
    let mut cands: Vec<(u8, String)> = Vec::new();
    for r in rows {
        let lname: Option<String> = r.try_get(0)?;
        if lname.as_deref() != Some(key) {
            continue;
        }
        let path: Option<String> = r.try_get(2)?;
        cands.push((item_role_rank(path.as_deref().unwrap_or("")), r.try_get(1)?));
    }
    Ok(pick_blueprint(&cands))
}

/// `resolve_item_blueprint`.
pub async fn resolve_blueprint(state: &AppState, name: &str) -> Option<String> {
    let key = strip(name).to_lowercase();
    if key.is_empty() {
        return None;
    }
    if let Some(hit) = BP_CACHE.lock().ok().and_then(|c| c.get(&key).cloned()) {
        return hit;
    }
    match lookup_blueprint(state, &key).await {
        Ok(bp) => {
            if let Ok(mut c) = BP_CACHE.lock() {
                c.insert(key, bp.clone());
            }
            bp
        }
        Err(e) => {
            tracing::warn!("chaos: blueprint resolve failed for {name:?}: {e:#}");
            None
        }
    }
}

// --- relay ------------------------------------------------------------------

/// `chaos.normalize`: the Trovesaurus payload as `{name, identifier,
/// blueprint, start, end}`, or `None` without a name.
pub fn normalize(payload: &Value) -> Option<Map<String, Value>> {
    let empty = Value::Object(Map::new());
    let payload = match payload {
        Value::Object(m) if matches!(m.get("data"), Some(Value::Object(_))) => &m["data"],
        Value::Array(items) => items.first().unwrap_or(&empty),
        other => other,
    };
    let p = payload.as_object()?;
    let name = p.get("name").filter(|n| py_truthy(n))?;
    let identifier = match p.get("identifier") {
        Some(Value::String(s)) => Value::String(s.replace('\\', "/")),
        other => other.cloned().unwrap_or(Value::Null),
    };
    let blueprint = match p.get("blueprint") {
        Some(Value::String(s)) => Value::String(s.to_lowercase()),
        other => other.cloned().unwrap_or(Value::Null),
    };
    let to_int =
        |k: &str| p.get(k).and_then(int_of).and_then(|i| i64::try_from(i).ok()).map_or(Value::Null, Value::from);
    let mut out = Map::new();
    out.insert("name".into(), name.clone());
    out.insert("identifier".into(), identifier);
    out.insert("blueprint".into(), blueprint);
    out.insert("start".into(), to_int("start"));
    out.insert("end".into(), to_int("end"));
    Some(out)
}

/// `refresh_chaos_chest`: true when an item was stored.
async fn refresh(state: AppState) -> anyhow::Result<()> {
    let url = &state.settings().trove_chaos_chest_url;
    let body = http::get(url, &[("user-agent", http::KIWI_UA.to_owned())], Duration::from_secs(15)).await?;
    let payload = pyjson::loads(&body).map_err(|e| anyhow::anyhow!("chaos chest payload: {e:?}"))?;
    let Some(item) = normalize(&payload) else {
        tracing::info!("Chaos Chest relay: no item");
        return Ok(());
    };
    store::save_chaos_feed(state.mongo(), &item).await?;
    tracing::info!("Chaos Chest relay: updated");
    Ok(())
}

pub fn start(state: &AppState) {
    jobs::spawn_periodic(state, "chaos", refresh, |state: AppState| async move {
        Duration::from_secs(state.settings().trove_chaos_refresh_seconds.max(0) as u64)
    });
}

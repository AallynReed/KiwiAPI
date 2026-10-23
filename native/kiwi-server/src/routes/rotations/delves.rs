//! `app/trove/delves.py`'s refresher (`delves`): the current week from the
//! external source, hourly on the delve Monday, else at the daily reset.

use std::time::Duration;

use kiwi_core::rotations as rot;
use serde_json::Value;

use super::{gamedata::Invalid, http, store};
use crate::{auth::pyutil::bson_int, dt, features, jobs, pyjson, state::AppState};

const BROWSER_UA: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";
const DISABLED_RECHECK: Duration = Duration::from_secs(300);

/// `normalize_payload`: the floor list and total of a source payload, as the
/// Python values (`total` keeps a bool, which only the insert path coerces).
pub fn normalize(payload: &Value) -> (Vec<Value>, Value) {
    let Value::Object(p) = payload else { return (Vec::new(), Value::from(0)) };
    let depths = match p.get("depths") {
        Some(Value::Array(items)) => items.clone(),
        _ => Vec::new(),
    };
    let total = match p.get("total") {
        Some(v @ Value::Bool(_)) => v.clone(),
        Some(v @ Value::Number(n)) if n.is_i64() || n.is_u64() => v.clone(),
        _ => Value::from(depths.len()),
    };
    (depths, total)
}

fn source_headers(state: &AppState, week: i64) -> Vec<(&'static str, String)> {
    let mut headers = vec![
        ("user-agent", BROWSER_UA.to_owned()),
        ("accept", "application/json, text/plain, */*".to_owned()),
        ("accept-language", "en-US,en;q=0.9".to_owned()),
    ];
    let referer = &state.settings().trove_delve_source_referer;
    if !referer.is_empty() {
        headers.push(("referer", format!("{referer}?week={week}")));
    }
    headers
}

/// `store_week`: never replaces stored depths with an empty payload. A new
/// row goes through `DelveRotation(...)` (`list[dict]`, `int`); an existing
/// one gets the raw values assigned, which Beanie saves unvalidated.
pub(super) async fn store_week(state: &AppState, week: i64, payload: &Value) -> anyhow::Result<usize> {
    let (depths, total) = normalize(payload);
    let db = state.mongo();
    let existing = store::delve_week(db, week).await?;
    if let Some(e) = &existing
        && depths.is_empty()
        && !e.depths.is_empty()
    {
        return Ok(e.depths.len());
    }
    let invalid = |_| anyhow::anyhow!("delve week {week}: payload does not fit DelveRotation");
    let total = match (&total, &existing) {
        (Value::Bool(b), None) => bson_int(i64::from(*b)),
        _ => store::to_bson(&total).map_err(invalid)?,
    };
    if existing.is_none() && !depths.iter().all(Value::is_object) {
        return Err(invalid(Invalid));
    }
    let bson_depths = depths.iter().map(store::to_bson).collect::<Result<Vec<_>, _>>().map_err(invalid)?;
    store::save_delve_week(db, existing.as_ref(), week, bson_depths, total).await?;
    Ok(depths.len())
}

async fn refresh_current_week(state: &AppState) -> anyhow::Result<()> {
    let week = rot::delve_week_id(dt::now().timestamp_micros());
    let url = format!("{}?week={week}&limit=200", state.settings().trove_delve_source_url);
    let body = http::get(&url, &source_headers(state, week), Duration::from_secs(30)).await?;
    let payload = pyjson::loads(&body).map_err(|e| anyhow::anyhow!("delve payload: {e:?}"))?;
    let count = store_week(state, week, &payload).await?;
    tracing::info!("Delve refresh: week {} -> {count} depths", rot::delve_week_id(dt::now().timestamp_micros()));
    Ok(())
}

fn next_pull() -> Duration {
    Duration::from_micros(rot::delve_next_pull(dt::now().timestamp_micros()) as u64)
}

/// One `_refresh_cycle`; returns the sleep before the next one.
async fn cycle(state: &AppState) -> Duration {
    match features::is_enabled(state, features::DELVES_FLAG).await {
        Ok(true) => {
            if let Err(e) = refresh_current_week(state).await {
                tracing::warn!("Delve refresh failed: {e:#}");
            }
            next_pull()
        }
        Ok(false) => DISABLED_RECHECK,
        Err(e) => {
            tracing::warn!("Delve refresh failed: {e:#}");
            next_pull()
        }
    }
}

pub fn start(state: &AppState) {
    if state.settings().trove_delve_source_url.is_empty() {
        tracing::info!("delves: no source URL configured (set TROVE_DELVE_SOURCE_URL) - refresher off");
        return;
    }
    jobs::spawn(state, "delves", |state| async move {
        loop {
            let sleep = cycle(&state).await;
            tokio::time::sleep(sleep).await;
        }
    });
}

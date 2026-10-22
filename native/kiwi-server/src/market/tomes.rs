//! `GET /site/tomes` (`app/trove/tomes/service.py`): the static payout table
//! priced at active-listing medians, with codex art where it resolves.

use std::{path::Path, sync::OnceLock};

use axum::{extract::State, response::Response};
use kiwi_core::{
    gems::pynum::PyNum,
    market::tomes::{self as core, Reward, Tome},
};
use serde_json::{Value, json};

use super::{
    SiteFeature,
    reads::{ICON_BRANCH, site},
    store::{self, Live, Store, internal},
};
use crate::{error::ApiResult, features::Tomes, gamedata::BASELINE_DIR, state::AppState};

fn s<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key)?.as_str()
}

/// `tomes.json` as the service reads it; `None` where Python would raise.
pub fn parse_table(raw: &Value) -> Option<Vec<Tome>> {
    let mut out = Vec::new();
    for t in raw.get("tomes")?.as_array()? {
        let mut rewards = Vec::new();
        for r in t.get("rewards")?.as_array()? {
            let price = r.get("price").filter(|p| crate::features::py_truthy(p));
            let p = |k: &str| price.and_then(|p| p.get(k)).filter(|v| !v.is_null());
            rewards.push(Reward {
                item: s(r, "item")?.to_owned(),
                amount: r.get("amount")?.as_i64()?,
                source: match p("source") {
                    None => None,
                    Some(v) => Some(v.as_str()?.to_owned()),
                },
                price_item: match p("item") {
                    None => None,
                    Some(v) => Some(v.as_str()?.to_owned()),
                },
                per: match p("per") {
                    None => None,
                    Some(v) => Some(v.as_i64()?),
                },
            });
        }
        let note = match t.get("note") {
            None | Some(Value::Null) => None,
            Some(v) => Some(v.as_str()?.to_owned()),
        };
        out.push(Tome { name: s(t, "name")?.to_owned(), kind: s(t, "type")?.to_owned(), note, rewards });
    }
    Some(out)
}

/// The module-global table (only a successful load is kept).
fn table() -> ApiResult<&'static [Tome]> {
    static TABLE: OnceLock<Vec<Tome>> = OnceLock::new();
    if let Some(t) = TABLE.get() {
        return Ok(t);
    }
    let path = Path::new(BASELINE_DIR).join("tomes.json");
    let raw = crate::gamedata::read_json(&path).ok_or_else(|| internal(format!("unreadable {}", path.display())))?;
    let parsed = parse_table(&raw).ok_or_else(|| internal("tomes.json does not match the service"))?;
    Ok(TABLE.get_or_init(|| parsed))
}

fn num(n: PyNum) -> Value {
    match n {
        PyNum::Int(i) => Value::from(i),
        PyNum::Float(f) => Value::from(f),
    }
}

/// `valued_tomes()` over a given table.
pub async fn valued<S: Store>(s: &S, tomes: &[Tome]) -> ApiResult<Value> {
    let medians = store::active_medians(s, &core::market_names(tomes)).await?;
    let resolved =
        store::blueprints_for_names(s, ICON_BRANCH, &core::icon_lookup_names(tomes)).await.unwrap_or_default();
    let priced: Vec<_> = tomes.iter().map(|t| core::price_tome(t, &medians)).collect();
    let rendered: Vec<Value> = priced
        .iter()
        .zip(tomes)
        .map(|(p, t)| {
            let rewards: Vec<Value> = p
                .rewards
                .iter()
                .map(|r| {
                    let mut v = json!({
                        "item": r.item, "amount": r.amount, "source": r.source, "via": r.via,
                        "unit_price": r.unit_price, "value": r.value, "listings": r.listings,
                    });
                    if let Some(per) = r.per {
                        v["per"] = Value::from(per);
                    }
                    v
                })
                .collect();
            json!({
                "name": p.name, "type": p.kind, "note": p.note, "rewards": rewards,
                "value": p.value.map(num), "known_value": p.known_value.map(num),
                "untradeable": p.untradeable, "unlisted": p.unlisted, "status": p.status,
                "blueprint": core::pick_icon(t, &resolved),
            })
        })
        .collect();
    Ok(json!({
        "tomes": rendered,
        "best_regular": core::best_regular(&priced).map(num),
        "priced_items": medians.len(),
        "icon_branch": ICON_BRANCH,
        "weekly_reset_at": kiwi_core::rotations::server_time(s.now_us()).weekly_reset_at,
    }))
}

pub async fn site_tomes(State(state): State<AppState>, _: SiteFeature<Tomes>) -> Response {
    let body = match table() {
        Ok(t) => valued(&Live(&state), t).await,
        Err(e) => Err(e),
    };
    site(body, 120)
}

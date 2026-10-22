//! Pure parts of the market scope (`app/trove/market`): sort tokens, the
//! price-history outlier filter, merchant bands, category ordering, the codex
//! name -> blueprint pick behind item thumbnails, plus the dump parser, the
//! anomaly scan and the tome valuation in the submodules.

pub mod parser;
pub mod signals;
pub mod tomes;

#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};

use crate::{pystr, rotations as rt};

pub const LISTING_LIFETIME_SECONDS: i64 = 7 * 86400;
pub const LISTING_STALE_SECONDS: i64 = 3 * 3600;
pub const LIQUIDITY_SOLD_MARGIN_SECONDS: i64 = 6 * 3600;
pub const PRICE_OUTLIER_Z: f64 = 3.5;
const PRICE_OUTLIER_MIN_SAMPLES: usize = 5;

/// `pg_store.order_by`: the ORDER BY for a `+col` / `-col` token.
pub fn order_by(sort: &str) -> String {
    let s = pystr::strip(if sort.is_empty() { "-last_seen" } else { sort });
    let desc = s.starts_with('-');
    match s.trim_start_matches(['+', '-']) {
        col @ ("last_seen" | "price_each" | "created_at" | "price" | "stack" | "name") => {
            format!("{col} {}", if desc { "DESC" } else { "ASC" })
        }
        _ => "last_seen DESC".into(),
    }
}

/// `_filter_price_outliers` over each point's `price_each`: which points the
/// log-space modified-Z filter drops, `None` when it leaves them all.
pub fn price_outliers(prices: &[f64]) -> Option<Vec<bool>> {
    if prices.len() < PRICE_OUTLIER_MIN_SAMPLES {
        return None;
    }
    let mut logs: Vec<f64> = prices.iter().filter(|&&v| v > 0.0).map(|v| v.log10()).collect();
    if logs.len() < PRICE_OUTLIER_MIN_SAMPLES {
        return None;
    }
    logs.sort_by(f64::total_cmp);
    let median = logs[logs.len() / 2];
    let mut dev: Vec<f64> = logs.iter().map(|x| (x - median).abs()).collect();
    dev.sort_by(f64::total_cmp);
    let mad = dev[dev.len() / 2];
    if mad < 1e-9 {
        return None;
    }
    Some(prices.iter().map(|&v| v > 0.0 && 0.6745 * (v.log10() - median).abs() / mad > PRICE_OUTLIER_Z).collect())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Band {
    pub name: &'static str,
    pub kind: &'static str,
    pub starts_at: i64,
    pub ends_at: i64,
}

fn cycle_bands(anchor: i64, interval: i64, duration: i64, start: i64, end: i64, name: &'static str) -> Vec<Band> {
    let mut out = Vec::new();
    if interval <= 0 {
        return out;
    }
    let mut k = (start - duration - anchor).div_euclid(interval).max(0);
    loop {
        let s = anchor + k * interval;
        if s > end {
            break;
        }
        let e = s + duration;
        if e >= start {
            out.push(Band { name, kind: "merchant", starts_at: s, ends_at: e });
        }
        k += 1;
    }
    out
}

/// `service.event_bands`: Corruxion and Fluxion windows overlapping `[start, end]`.
pub fn event_bands(start: i64, end: i64) -> Vec<Band> {
    let secs = |us: i64| us.div_euclid(rt::US);
    let dur = secs(rt::DRAGON_DURATION);
    let corr = secs(rt::FIRST_CORRUXION + rt::TROVE_OFFSET);
    let flux = secs(rt::FIRST_FLUXION + rt::TROVE_OFFSET);
    let mut bands = cycle_bands(corr, secs(rt::DRAGON_INTERVAL), dur, start, end, "Corruxion");
    bands.extend(cycle_bands(flux, secs(rt::FLUXION_INTERVAL), dur, start, end, "Fluxion"));
    bands.sort_by_key(|b| b.starts_at);
    bands
}

fn dedup(items: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut seen = HashSet::new();
    items.into_iter().filter(|s| seen.insert(s.clone())).collect()
}

/// `sorted(set(items), key=str.lower)`.
pub fn sorted_lower(items: &[String]) -> Vec<String> {
    let mut out = dedup(items.iter().cloned());
    out.sort_by_cached_key(|s| s.to_lowercase());
    out
}

/// `sorted({n.strip() for n in names if n and n.strip()})`.
pub fn clean_names(names: &[String]) -> Vec<String> {
    let mut out = dedup(names.iter().map(|n| pystr::strip(n).to_owned()).filter(|n| !n.is_empty()));
    out.sort();
    out
}

/// `sorted({i.strip() for i in items if i and i.strip()}, key=str.lower)`.
pub fn clean_items(items: &[String]) -> Vec<String> {
    let cleaned: Vec<String> = items.iter().map(|n| pystr::strip(n).to_owned()).filter(|n| !n.is_empty()).collect();
    sorted_lower(&cleaned)
}

/// `_sorted_categories`: indices of `(order, name)` pairs by order, then
/// lower-cased name (stable).
pub fn category_order(docs: &[(i64, &str)]) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..docs.len()).collect();
    idx.sort_by_cached_key(|&i| (docs[i].0, docs[i].1.to_lowercase()));
    idx
}

/// `admin_reorder_categories`: the saves it makes, in order, as
/// `(index into docs, new order)`. `docs` are `(id, order, name)` in store
/// order; an id listed twice is saved twice, as in Python.
pub fn reorder<T: PartialEq>(docs: &[(T, i64, &str)], ids: &[Option<T>]) -> Vec<(usize, i64)> {
    let ordered: Vec<usize> =
        ids.iter().filter_map(|id| id.as_ref().and_then(|id| docs.iter().position(|d| &d.0 == id))).collect();
    let rest: Vec<usize> = (0..docs.len()).filter(|&i| !ids.iter().any(|id| id.as_ref() == Some(&docs[i].0))).collect();
    let keys: Vec<(i64, &str)> = rest.iter().map(|&i| (docs[i].1, docs[i].2)).collect();
    let tail: Vec<usize> = category_order(&keys).into_iter().map(|j| rest[j]).collect();
    let mut orders: Vec<i64> = docs.iter().map(|d| d.1).collect();
    let mut saves = Vec::new();
    for (pos, &i) in ordered.iter().chain(tail.iter()).enumerate() {
        let pos = pos as i64;
        if orders[i] != pos {
            orders[i] = pos;
            saves.push((i, pos));
        }
    }
    saves
}

/// `codexes.pg_store._item_role_rank`.
pub fn item_role_rank(path: Option<&str>) -> u8 {
    let p = path.unwrap_or("").to_lowercase();
    let is_item = p.starts_with("prefabs/item/");
    if is_item && !p.contains("/mount/") {
        return if p.contains("_debug") || p.contains("_notrade") { 1 } else { 0 };
    }
    if is_item {
        return 2;
    }
    if p.starts_with("prefabs/collections/") {
        return 3;
    }
    4
}

/// The pick in `codexes.pg_store.blueprints_for_names` from its
/// `(lower(name), blueprint, path)` rows: the best-ranked role per name, kept
/// only when that role agrees on one blueprint.
pub fn pick_blueprints<'a>(
    rows: impl IntoIterator<Item = (&'a str, &'a str, Option<&'a str>)>,
) -> HashMap<String, String> {
    let mut cands: HashMap<&str, Vec<(u8, &str)>> = HashMap::new();
    for (lname, bp, path) in rows {
        cands.entry(lname).or_default().push((item_role_rank(path), bp));
    }
    let mut out = HashMap::new();
    for (lname, items) in cands {
        let best = items.iter().map(|c| c.0).min().unwrap_or(u8::MAX);
        let top: HashSet<&str> = items.iter().filter(|c| c.0 == best).map(|c| c.1).collect();
        if top.len() == 1 {
            out.insert(lname.to_owned(), top.into_iter().next().unwrap_or_default().to_owned());
        }
    }
    out
}

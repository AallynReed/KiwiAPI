//! The in-game store catalog (`app/trove/store`): the StoreLog parser, the
//! stored model, the ingest rules and the per-product records.

pub mod parser;
pub mod py;

use parser::{ParsedCategory, ParsedProduct, ParsedStoreDump};

pub const DAY: i64 = 86_400;

#[derive(Clone, Debug, PartialEq)]
pub struct Price {
    pub currency: String,
    pub cost: f64,
    pub can_purchase: bool,
    pub monthly: i64,
    pub sale: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Texture {
    pub texture: String,
    pub x: i64,
    pub y: i64,
    pub text: String,
    pub overlay: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PricePoint {
    pub ts: i64,
    pub prices: Vec<Price>,
    pub price_string: String,
}

/// `StoreProductDoc`, field for field.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Product {
    pub code: String,
    pub kind: String,
    pub name: String,
    pub image: String,
    pub info: String,
    pub informational: bool,
    pub tradable: bool,
    pub prices: Vec<Price>,
    pub price_string: String,
    pub price_string_currency: String,
    pub price_string_sale: String,
    pub promo: String,
    pub deal_expires_at: Option<i64>,
    pub interact_label: String,
    pub interact_enabled: bool,
    pub trial_limits: String,
    pub class_level: Option<i64>,
    pub class_power_rank: Option<i64>,
    pub class_shield_frame: Option<i64>,
    pub class_sub_name: String,
    pub class_icon: String,
    pub textures: Vec<Texture>,
    pub loot_title: String,
    pub loot_body: String,
    pub categories: Vec<i64>,
    pub first_seen: i64,
    pub last_seen: i64,
    pub price_history: Vec<PricePoint>,
    pub availability: Vec<Vec<i64>>,
}

/// `StoreCategoryDoc`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Category {
    pub index: i64,
    pub label: String,
    pub icon: String,
    pub codes: Vec<String>,
    pub last_seen: i64,
}

/// `StoreStateDoc`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct State {
    pub key: String,
    pub last_anchor: Option<i64>,
    pub title: String,
    pub product_count: i64,
    pub category_count: i64,
}

impl Default for State {
    fn default() -> Self {
        State { key: "state".into(), last_anchor: None, title: String::new(), product_count: 0, category_count: 0 }
    }
}

/// What Python raises mid-ingest (an IndexError, or a deal countdown that
/// `int()` or BSON cannot hold).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IngestError(pub String);

impl Product {
    /// `StoreProductDoc(code=..., kind=..., first_seen=anchor)`.
    pub fn new(code: &str, kind: &str, first_seen: i64) -> Self {
        Product { code: code.into(), kind: kind.into(), first_seen, ..Default::default() }
    }
}

/// `_price_signature`: the sorted set of (currency, cost, monthly, sale) plus
/// the price string.
fn signature(prices: &[Price]) -> Vec<(&str, f64, i64, &str)> {
    let mut sig: Vec<(&str, f64, i64, &str)> = Vec::new();
    for p in prices {
        let t = (p.currency.as_str(), p.cost, p.monthly, p.sale.as_str());
        if !sig.contains(&t) {
            sig.push(t);
        }
    }
    sig.sort_by(|a, b| {
        a.0.cmp(b.0)
            .then(a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
            .then(a.2.cmp(&b.2))
            .then(a.3.cmp(b.3))
    });
    sig
}

fn price_changed(new: &[Price], new_ps: &str, old: &[Price], old_ps: &str) -> bool {
    signature(new) != signature(old) || new_ps != old_ps
}

/// `_record_availability`: extend the last interval when it ended at the
/// previous ingest, keep it on a same-anchor re-ingest, else open a new one.
pub fn record_availability(p: &mut Product, anchor: i64, prev_anchor: Option<i64>) -> Result<(), IngestError> {
    if p.availability.is_empty() && p.first_seen != 0 && p.last_seen != 0 && p.last_seen != anchor {
        p.availability = vec![vec![p.first_seen, p.last_seen]];
    }
    let last_end = match p.availability.last() {
        Some(iv) => Some(*iv.get(1).ok_or_else(|| IngestError("IndexError: list index out of range".into()))?),
        None => None,
    };
    match (last_end, prev_anchor) {
        (Some(end), Some(prev)) if end == prev => {
            if let Some(iv) = p.availability.last_mut() {
                iv[1] = anchor;
            }
        }
        (Some(end), _) if end == anchor => {}
        _ => p.availability.push(vec![anchor, anchor]),
    }
    Ok(())
}

/// `_apply_parsed`: refresh every field from the dump; true when the price
/// signature changed (a history point was appended).
pub fn apply_parsed(p: &mut Product, parsed: &ParsedProduct, anchor: i64) -> Result<bool, IngestError> {
    let prices: Vec<Price> = parsed
        .prices
        .iter()
        .map(|pr| Price {
            currency: pr.currency.clone(),
            cost: pr.cost,
            can_purchase: pr.can_purchase,
            monthly: pr.monthly,
            sale: pr.sale.clone(),
        })
        .collect();
    let changed = price_changed(&prices, &parsed.price_string, &p.prices, &p.price_string);
    let deal_expires_at = match parsed.deal_seconds {
        None => None,
        Some(s) if s.is_finite() && s.trunc().abs() < 9.2e18 => Some(
            anchor.checked_add(s.trunc() as i64).ok_or_else(|| IngestError("OverflowError: deal_expires_at".into()))?,
        ),
        Some(s) => return Err(IngestError(format!("cannot convert {s} to int"))),
    };

    p.kind = parsed.kind.clone();
    p.name = parsed.name.clone();
    p.image = parsed.image.clone();
    p.info = parsed.info.clone();
    p.informational = parsed.informational;
    p.tradable = parsed.tradable;
    p.prices = prices.clone();
    p.price_string = parsed.price_string.clone();
    p.price_string_currency = parsed.price_string_currency.clone();
    p.price_string_sale = parsed.price_string_sale.clone();
    p.promo = parsed.promo.clone();
    p.deal_expires_at = deal_expires_at;
    p.interact_label = parsed.interact_label.clone();
    p.interact_enabled = parsed.interact_enabled;
    p.trial_limits = parsed.trial_limits.clone();
    p.class_level = parsed.class_level;
    p.class_power_rank = parsed.class_power_rank;
    p.class_shield_frame = parsed.class_shield_frame;
    p.class_sub_name = parsed.class_sub_name.clone();
    p.class_icon = parsed.class_icon.clone();
    p.textures = parsed
        .textures
        .iter()
        .map(|t| Texture { texture: t.texture.clone(), x: t.x, y: t.y, text: t.text.clone(), overlay: t.overlay })
        .collect();
    if !parsed.loot_body.is_empty() {
        p.loot_title = parsed.loot_title.clone();
        p.loot_body = parsed.loot_body.clone();
    }
    p.categories = parsed.categories.clone();
    p.last_seen = anchor;
    if changed {
        p.price_history.push(PricePoint { ts: anchor, prices, price_string: parsed.price_string.clone() });
    }
    Ok(changed)
}

/// One product of `insert_dump`: (doc to save, created, price changed).
pub fn ingest_product(
    existing: Option<Product>,
    parsed: &ParsedProduct,
    anchor: i64,
    prev_anchor: Option<i64>,
) -> Result<(Product, bool, bool), IngestError> {
    let created = existing.is_none();
    let mut p = existing.unwrap_or_else(|| Product::new(&parsed.code, &parsed.kind, anchor));
    record_availability(&mut p, anchor, prev_anchor)?;
    let changed = apply_parsed(&mut p, parsed, anchor)?;
    Ok((p, created, changed))
}

/// One category of `insert_dump`.
pub fn ingest_category(existing: Option<Category>, c: &ParsedCategory, anchor: i64) -> Category {
    let mut cat = existing.unwrap_or_else(|| Category { index: c.index, ..Default::default() });
    cat.label = c.label.clone();
    cat.icon = c.icon.clone();
    cat.codes = c.codes.clone();
    cat.last_seen = anchor;
    cat
}

/// The state doc after `insert_dump`.
pub fn ingest_state(mut state: State, dump: &ParsedStoreDump, anchor: i64) -> State {
    state.last_anchor = Some(anchor);
    if !dump.title.is_empty() {
        state.title = dump.title.clone();
    }
    state.product_count = dump.products.len() as i64;
    state.category_count = dump.categories.len() as i64;
    state
}

pub const NO_STORE_DATA: &str = "No store data found in the upload - is this a StoreLog.cfg?";

/// `insert_dump`'s summary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Summary {
    pub products: usize,
    pub categories: usize,
    pub created: usize,
    pub price_changes: usize,
    pub anchor: i64,
    pub done_marker: bool,
}

/// `_interval_days`: whole days an interval covers, at least one.
pub fn interval_days(start: i64, end: i64) -> i64 {
    (((end - start) as f64 / DAY as f64).round_ties_even() as i64 + 1).max(1)
}

/// `_records`.
#[derive(Clone, Debug, PartialEq)]
pub struct Records {
    pub times_available: i64,
    pub returns: i64,
    pub total_days_seen: i64,
    pub longest_run_days: i64,
    pub first_seen: i64,
    pub last_seen: i64,
    pub currently_active: bool,
    pub gap_days: Option<i64>,
    pub price_low: Vec<(String, f64)>,
    pub price_high: Vec<(String, f64)>,
    pub price_changes: i64,
}

fn fold(acc: &mut Vec<(String, f64)>, currency: &str, cost: f64, keep_new: fn(f64, f64) -> bool) {
    match acc.iter_mut().find(|(c, _)| c == currency) {
        Some((_, v)) => {
            if keep_new(cost, *v) {
                *v = cost;
            }
        }
        None => acc.push((currency.to_owned(), cost)),
    }
}

pub fn is_active(last_seen: i64, anchor: Option<i64>) -> bool {
    anchor == Some(last_seen)
}

pub fn records(p: &Product, anchor: Option<i64>, now: i64) -> Records {
    let runs: Vec<i64> =
        p.availability.iter().filter(|iv| iv.len() == 2).map(|iv| interval_days(iv[0], iv[1])).collect();
    let active = is_active(p.last_seen, anchor);
    let (mut low, mut high) = (Vec::new(), Vec::new());
    for set in p.price_history.iter().map(|pt| &pt.prices).chain(std::iter::once(&p.prices)) {
        for pr in set {
            fold(&mut low, &pr.currency, pr.cost, |new, old| new < old);
            fold(&mut high, &pr.currency, pr.cost, |new, old| new > old);
        }
    }
    Records {
        times_available: runs.len() as i64,
        returns: (runs.len() as i64 - 1).max(0),
        total_days_seen: runs.iter().sum(),
        longest_run_days: runs.iter().copied().max().unwrap_or(0),
        first_seen: p.first_seen,
        last_seen: p.last_seen,
        currently_active: active,
        gap_days: if active || p.last_seen == 0 { None } else { Some((now - p.last_seen).div_euclid(DAY).max(0)) },
        price_low: low,
        price_high: high,
        price_changes: p.price_history.len() as i64,
    }
}

/// `timeline()`'s axis: oldest first sighting to now (or the latest sighting).
pub fn timeline_span(products: &[Product], anchor: Option<i64>, now: i64) -> (i64, i64) {
    let start = products.iter().map(|p| p.first_seen).filter(|&f| f != 0).min().unwrap_or(match anchor {
        Some(a) if a != 0 => a,
        _ => now,
    });
    let end = products.iter().map(|p| p.last_seen).filter(|&l| l != 0).fold(now, i64::max);
    (start, end)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interval_days_rounds_half_even() {
        assert_eq!(interval_days(0, 0), 1);
        assert_eq!(interval_days(0, DAY / 2), 1);
        assert_eq!(interval_days(0, 3 * DAY / 2), 3);
        assert_eq!(interval_days(0, 5 * DAY / 2), 3);
        assert_eq!(interval_days(0, 6 * DAY), 7);
        assert_eq!(interval_days(10 * DAY, 0), 1);
    }

    #[test]
    fn availability_rules() {
        let mut p = Product { first_seen: 100, last_seen: 200, ..Default::default() };
        record_availability(&mut p, 300, Some(250)).unwrap();
        assert_eq!(p.availability, vec![vec![100, 200], vec![300, 300]]);
        record_availability(&mut p, 400, Some(300)).unwrap();
        assert_eq!(p.availability, vec![vec![100, 200], vec![300, 400]]);
        record_availability(&mut p, 400, Some(300)).unwrap();
        assert_eq!(p.availability, vec![vec![100, 200], vec![300, 400]]);
        p.availability.push(vec![1]);
        assert!(record_availability(&mut p, 500, None).is_err());
    }
}

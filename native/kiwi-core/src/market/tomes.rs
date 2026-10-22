//! `app/trove/tomes/service.py`: tome payouts priced at marketplace medians.

use std::collections::{BTreeSet, HashMap};

use crate::gems::{
    py_round,
    pynum::{PyNum, py_sum},
};

#[derive(Clone, Debug, PartialEq)]
pub struct Reward {
    pub item: String,
    pub amount: i64,
    /// `price.source` (`market`, `flux`, `none`, or absent).
    pub source: Option<String>,
    /// `price.item`: a different market item this reward is priced through.
    pub price_item: Option<String>,
    /// `price.per`, as written (`None` when absent).
    pub per: Option<i64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Tome {
    pub name: String,
    pub kind: String,
    pub note: Option<String>,
    pub rewards: Vec<Reward>,
}

/// One median row: `(median_each, count)`.
pub type Medians = HashMap<String, (f64, i64)>;

#[derive(Clone, Debug, PartialEq)]
pub struct PricedReward {
    pub item: String,
    pub amount: i64,
    pub source: Option<String>,
    pub via: Option<String>,
    pub unit_price: Option<f64>,
    pub value: Option<f64>,
    pub listings: i64,
    /// Present only when `via` is.
    pub per: Option<i64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PricedTome {
    pub name: String,
    pub kind: String,
    pub note: Option<String>,
    pub rewards: Vec<PricedReward>,
    /// `round(sum(values), 2)` when every reward is priced (an int 0 when none carries a value).
    pub value: Option<PyNum>,
    /// `round(sum(values), 2)`: the int 0 when nothing is priced.
    pub known_value: Option<PyNum>,
    pub untradeable: Vec<String>,
    pub unlisted: Vec<String>,
    pub status: &'static str,
}

/// `_lookup_name`.
fn lookup_name(r: &Reward) -> Option<&str> {
    if r.source.as_deref() != Some("market") {
        return None;
    }
    Some(r.price_item.as_deref().filter(|s| !s.is_empty()).unwrap_or(&r.item))
}

/// `market_names`: every market name the table prices through, sorted.
pub fn market_names(tomes: &[Tome]) -> Vec<String> {
    let names: BTreeSet<&str> = tomes.iter().flat_map(|t| t.rewards.iter().filter_map(lookup_name)).collect();
    names.into_iter().map(str::to_owned).collect()
}

fn price_reward(r: &Reward, medians: &Medians) -> PricedReward {
    let mut out = PricedReward {
        item: r.item.clone(),
        amount: r.amount,
        source: r.source.clone(),
        via: None,
        unit_price: None,
        value: None,
        listings: 0,
        per: None,
    };
    match r.source.as_deref() {
        Some("flux") => out.unit_price = Some(1.0),
        Some("market") => {
            let name = lookup_name(r).unwrap_or(&r.item);
            let per = r.per.filter(|&p| p != 0).unwrap_or(1);
            if name != r.item {
                out.via = Some(name.to_owned());
                out.per = Some(per);
            }
            if let Some(&(median, count)) = medians.get(name) {
                out.unit_price = Some(median / per as f64);
                out.listings = count;
            }
        }
        _ => {}
    }
    out.value = out.unit_price.map(|u| py_round(u * r.amount as f64, 2));
    out
}

/// `_price_tome`.
pub fn price_tome(t: &Tome, medians: &Medians) -> PricedTome {
    let rewards: Vec<PricedReward> = t.rewards.iter().map(|r| price_reward(r, medians)).collect();
    let untradeable: Vec<String> =
        rewards.iter().filter(|r| r.source.as_deref() == Some("none")).map(|r| r.item.clone()).collect();
    let unlisted: Vec<String> = rewards
        .iter()
        .filter(|r| r.source.as_deref() == Some("market") && r.unit_price.is_none())
        .map(|r| r.via.clone().unwrap_or_else(|| r.item.clone()))
        .collect();
    let known = py_sum(rewards.iter().filter_map(|r| r.value.map(PyNum::Float)));
    let complete = !rewards.is_empty() && untradeable.is_empty() && unlisted.is_empty();
    let rounded = match known {
        PyNum::Int(i) => PyNum::Int(i),
        PyNum::Float(f) => PyNum::Float(py_round(f, 2)),
    };
    let status = if complete {
        "priced"
    } else if rewards.is_empty() {
        "no_payout_data"
    } else if !untradeable.is_empty() {
        "untradeable"
    } else {
        "unlisted"
    };
    PricedTome {
        name: t.name.clone(),
        kind: t.kind.clone(),
        note: t.note.clone(),
        value: complete.then_some(rounded),
        known_value: (!rewards.is_empty()).then_some(rounded),
        rewards,
        untradeable,
        unlisted,
        status,
    }
}

/// `_icon_candidates`.
pub fn icon_candidates(t: &Tome) -> Vec<String> {
    let mut forms = Vec::with_capacity(3);
    if t.kind == "legendary" {
        forms.push(format!("Legendary Tome: {}", t.name));
    }
    forms.push(format!("Tome: {}", t.name));
    forms.push(t.name.clone());
    forms
}

/// The `_icons` pick: first candidate whose blueprint (keyed by lower-cased
/// name) mentions `tome`.
pub fn pick_icon(t: &Tome, resolved: &HashMap<String, String>) -> Option<String> {
    icon_candidates(t).iter().find_map(|form| {
        resolved.get(&form.to_lowercase()).filter(|bp| !bp.is_empty() && bp.to_lowercase().contains("tome")).cloned()
    })
}

/// Every candidate form, sorted (the one bulk lookup `_icons` makes).
pub fn icon_lookup_names(tomes: &[Tome]) -> Vec<String> {
    let all: BTreeSet<String> = tomes.iter().flat_map(icon_candidates).collect();
    all.into_iter().collect()
}

/// `best_regular`: the highest complete regular value.
pub fn best_regular(priced: &[PricedTome]) -> Option<PyNum> {
    priced.iter().filter(|t| t.kind == "regular").filter_map(|t| t.value).fold(None, |m: Option<PyNum>, v| match m {
        Some(m) if m.f64() >= v.f64() => Some(m),
        _ => Some(v),
    })
}

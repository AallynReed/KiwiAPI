//! `app/trove/market/signals.py`: per-item robust-z anomalies against each
//! item's own daily history, and the market-wide breadth reading.

use crate::gems::py_round;

pub const DAY: i64 = 86400;
const MIN_BASELINE_DAYS: usize = 6;
const MIN_SAMPLES: i64 = 3;
const Z_ELEVATED: f64 = 3.5;
const Z_EXTREME: f64 = 6.0;
const MAD_TO_SIGMA: f64 = 0.6745;
const MAD_FLOOR_RATIO: f64 = 0.05;
const MIN_PRICE_MOVE: f64 = 0.25;
const MIN_COUNT_MOVE: f64 = 1.00;
const BREADTH_SHARE: f64 = 0.66;
const BREADTH_MIN_MOVE: f64 = 0.15;

/// One `daily_series_all` row.
#[derive(Clone, Debug, PartialEq)]
pub struct DayRow {
    pub name: String,
    pub bucket: i64,
    pub listings: i64,
    pub stack: i64,
    pub p50: f64,
    pub stack_med: f64,
    pub stack_max: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Signal {
    pub name: String,
    pub pattern: &'static str,
    pub reading: &'static str,
    pub severity: &'static str,
    pub day: i64,
    pub price: f64,
    pub baseline: f64,
    pub change: Option<f64>,
    pub excess: Option<f64>,
    pub listings: i64,
    pub stack_med: f64,
    pub stack_max: i64,
    pub price_z: Option<f64>,
    pub supply_z: Option<f64>,
    pub stack_z: Option<f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Breadth {
    pub items: usize,
    pub median_move: Option<f64>,
    pub share_up: Option<f64>,
    pub share_down: Option<f64>,
    pub verdict: &'static str,
    pub reading: &'static str,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Scan {
    pub generated_for: Option<i64>,
    pub market: Breadth,
    pub signals: Vec<Signal>,
    pub scanned_items: usize,
}

/// `statistics.median`.
fn median(values: &[f64]) -> f64 {
    let mut v = values.to_vec();
    v.sort_by(f64::total_cmp);
    let n = v.len();
    if n % 2 == 1 { v[n / 2] } else { (v[n / 2 - 1] + v[n / 2]) / 2.0 }
}

/// `_score`: `(robust_z, baseline, relative_change)`.
fn score(value: f64, history: &[f64]) -> Option<(f64, f64, f64)> {
    if history.len() < MIN_BASELINE_DAYS {
        return None;
    }
    let med = median(history);
    if med <= 0.0 {
        return None;
    }
    let dev: Vec<f64> = history.iter().map(|x| (x - med).abs()).collect();
    let mad = median(&dev).max(MAD_FLOOR_RATIO * med);
    Some((MAD_TO_SIGMA * (value - med) / mad, med, (value - med) / med))
}

fn flagged(z: f64, rel: f64, min_move: f64) -> bool {
    if z.abs() < Z_ELEVATED {
        return false;
    }
    if rel >= 0.0 { rel >= min_move } else { rel <= (1.0 / (1.0 + min_move)) - 1.0 }
}

fn severity(scores: &[f64]) -> &'static str {
    let peak = scores.iter().fold(0.0f64, |m, s| if s.abs() > m { s.abs() } else { m });
    if peak >= Z_EXTREME {
        "extreme"
    } else if peak >= Z_ELEVATED {
        "elevated"
    } else {
        "normal"
    }
}

fn classify(up: bool, down: bool, supply_up: bool, supply_down: bool, stack_up: bool) -> (&'static str, &'static str) {
    if down && supply_up && stack_up {
        return (
            "supply_flood",
            "Price collapsed while supply and stack sizes both spiked - the shape a duplicated item makes.",
        );
    }
    if down && supply_up {
        return ("oversupply", "A lot more of this appeared and the price fell with it.");
    }
    if up && supply_down {
        return ("squeeze", "Supply dried up and the price ran up behind it.");
    }
    if stack_up && !down {
        return ("odd_stacks", "Stack sizes are far larger than this item normally trades in.");
    }
    if up {
        return ("spike", "Priced well above its own recent range.");
    }
    if down {
        return ("slump", "Priced well below its own recent range.");
    }
    if supply_up {
        return ("supply_surge", "Many more listings than this item usually gets.");
    }
    ("unusual", "Moving outside its normal range.")
}

/// Why a scan cannot finish: Python raises on these (a 500).
#[derive(Debug, PartialEq, Eq)]
pub struct ScanError;

fn scan_item(name: &str, series: &[&DayRow], market_move: f64) -> Result<Option<Signal>, ScanError> {
    if series.len() < MIN_BASELINE_DAYS + 1 {
        return Ok(None);
    }
    let (latest, history) = (series[series.len() - 1], &series[..series.len() - 1]);
    if latest.listings < MIN_SAMPLES {
        return Ok(None);
    }
    let p50: Vec<f64> = history.iter().map(|d| d.p50).collect();
    let listings: Vec<f64> = history.iter().map(|d| d.listings as f64).collect();
    let stacks: Vec<f64> = history.iter().map(|d| d.stack_med).collect();
    let factor = 1.0 + market_move;
    let deflated = if factor > 0.0 { latest.p50 / factor } else { latest.p50 };
    let price = score(deflated, &p50);
    let change = score(latest.p50, &p50).map(|s| s.2);
    let supply = score(latest.listings as f64, &listings);
    let stack = score(latest.stack_med, &stacks);

    let hit = |s: Option<(f64, f64, f64)>, min| s.is_some_and(|(z, _, rel)| flagged(z, rel, min));
    let (price_hit, supply_hit, stack_hit) =
        (hit(price, MIN_PRICE_MOVE), hit(supply, MIN_COUNT_MOVE), hit(stack, MIN_COUNT_MOVE));
    if !(price_hit || supply_hit || stack_hit) {
        return Ok(None);
    }
    let z = |s: Option<(f64, f64, f64)>| s.map(|s| s.0);
    let hits: Vec<f64> = [(price, price_hit), (supply, supply_hit), (stack, stack_hit)]
        .into_iter()
        .filter_map(|(s, h)| if h { z(s) } else { None })
        .collect();
    let positive = |s: Option<(f64, f64, f64)>, h: bool| h && z(s).is_some_and(|z| z > 0.0);
    let negative = |s: Option<(f64, f64, f64)>, h: bool| h && z(s).is_some_and(|z| z < 0.0);
    let (pattern, reading) = classify(
        positive(price, price_hit),
        negative(price, price_hit),
        positive(supply, supply_hit),
        negative(supply, supply_hit),
        positive(stack, stack_hit),
    );
    // round(base, 2) on a missing baseline is a TypeError in Python.
    let base = price.ok_or(ScanError)?.1;
    let round = |v: Option<f64>, n| v.map(|v| py_round(v, n));
    Ok(Some(Signal {
        name: name.to_owned(),
        pattern,
        reading,
        severity: severity(&hits),
        day: latest.bucket,
        price: py_round(latest.p50, 2),
        baseline: py_round(base, 2),
        change: round(change, 4),
        excess: round(price.map(|s| s.2), 4),
        listings: latest.listings,
        stack_med: py_round(latest.stack_med, 1),
        stack_max: latest.stack_max,
        price_z: round(z(price), 2),
        supply_z: round(z(supply), 2),
        stack_z: round(z(stack), 2),
    }))
}

fn breadth(by_item: &[(String, Vec<&DayRow>)]) -> Breadth {
    let mut moves = Vec::new();
    for (_, series) in by_item {
        if series.len() < MIN_BASELINE_DAYS + 1 {
            continue;
        }
        let latest = series[series.len() - 1];
        if latest.listings < MIN_SAMPLES {
            continue;
        }
        let history: Vec<f64> = series[..series.len() - 1].iter().map(|d| d.p50).collect();
        if let Some((_, _, rel)) = score(latest.p50, &history) {
            moves.push(rel);
        }
    }
    if moves.is_empty() {
        return Breadth {
            items: 0,
            median_move: None,
            share_up: None,
            share_down: None,
            verdict: "no_data",
            reading: "",
        };
    }
    let n = moves.len();
    let up = moves.iter().filter(|&&m| m >= BREADTH_MIN_MOVE).count() as f64 / n as f64;
    let down = moves.iter().filter(|&&m| m <= -BREADTH_MIN_MOVE).count() as f64 / n as f64;
    let (verdict, reading) = if up >= BREADTH_SHARE {
        (
            "flux_weaker",
            "Most of the market got more expensive at once. When unrelated items all rise together it is usually \
             flux losing value rather than the items gaining it - which is what a flux duplication looks like from \
             the outside.",
        )
    } else if down >= BREADTH_SHARE {
        (
            "flux_stronger",
            "Most of the market got cheaper at once, which points at flux itself gaining value rather than every \
             item independently falling.",
        )
    } else {
        ("normal", "Items are moving independently, which is what ordinary trading looks like.")
    };
    Breadth {
        items: n,
        median_move: Some(py_round(median(&moves), 4)),
        share_up: Some(py_round(up, 3)),
        share_down: Some(py_round(down, 3)),
        verdict,
        reading,
    }
}

/// `scan` after the query: `rows` as `daily_series_all` returns them.
pub fn scan(rows: &[DayRow]) -> Result<Scan, ScanError> {
    let mut by_item: Vec<(String, Vec<&DayRow>)> = Vec::new();
    let mut index = std::collections::HashMap::new();
    for r in rows {
        let i = *index.entry(r.name.as_str()).or_insert_with(|| {
            by_item.push((r.name.clone(), Vec::new()));
            by_item.len() - 1
        });
        by_item[i].1.push(r);
    }
    for (_, series) in &mut by_item {
        series.sort_by_key(|r| r.bucket);
    }
    let market = breadth(&by_item);
    let market_move = market.median_move.unwrap_or(0.0);
    let mut signals = Vec::new();
    for (name, series) in &by_item {
        if let Some(s) = scan_item(name, series, market_move)? {
            signals.push(s);
        }
    }
    let key = |s: &Signal| {
        let peak = [s.price_z, s.supply_z, s.stack_z].iter().map(|z| z.unwrap_or(0.0).abs()).fold(0.0f64, f64::max);
        (s.severity == "extreme", peak)
    };
    signals.sort_by(|a, b| {
        let (ka, kb) = (key(a), key(b));
        kb.0.cmp(&ka.0).then(kb.1.partial_cmp(&ka.1).unwrap_or(std::cmp::Ordering::Equal))
    });
    let generated_for = by_item.iter().map(|(_, s)| s[s.len() - 1].bucket).max();
    Ok(Scan { generated_for, market, signals, scanned_items: by_item.len() })
}

//! `app/trove/market/parser.py`: the GrainusMod.cfg dump the scraper posts.

use std::{collections::HashSet, sync::LazyLock};

use regex::Regex;

use crate::{gems::py_round, pystr};

pub const MAX_REASONABLE_PRICE: i64 = 50_000_000;

/// Seconds from the UUID v1 epoch (1582-10-15) to the Unix epoch.
const UUID_EPOCH_OFFSET: i64 = 12_219_292_800;

static LISTING_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"([0-9a-fA-F-]{36});([^;\r\n]+);([^;\r\n]*);(\d{1,4});(\d{1,9})").expect("listing regex")
});
static DIGIT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\d$").expect("digit regex"));

#[derive(Clone, Debug, PartialEq)]
pub struct ParsedListing {
    /// The UUID as an integer (`UUID.int`).
    pub id: u128,
    pub name: String,
    pub kind: Option<String>,
    pub stack: i64,
    pub price: i64,
    pub price_each: f64,
    pub created_at: i64,
}

/// `str(UUID)`.
pub fn uuid_str(id: u128) -> String {
    let h = format!("{id:032x}");
    format!("{}-{}-{}-{}-{}", &h[..8], &h[8..12], &h[12..16], &h[16..20], &h[20..])
}

/// `UUID(hex)` for the regex's 36-character run: dashes anywhere, 32 hex digits.
fn parse_uuid(s: &str) -> Option<u128> {
    let hex: String = s.chars().filter(|&c| c != '-').collect();
    if hex.len() != 32 {
        return None;
    }
    u128::from_str_radix(&hex, 16).ok()
}

/// The decimal value of a `\d` character: Unicode lays each set of decimal
/// digits out as one contiguous 0-9 run.
fn digit_value(c: char) -> u32 {
    if let Some(d) = c.to_digit(10) {
        return d;
    }
    let mut n = 0;
    let mut cp = c as u32;
    while let Some(prev) = cp.checked_sub(1).and_then(char::from_u32) {
        let mut buf = [0u8; 4];
        if !DIGIT.is_match(prev.encode_utf8(&mut buf)) {
            break;
        }
        n += 1;
        cp -= 1;
    }
    n % 10
}

/// `int(s)` of a run of `\d` characters.
fn py_int(s: &str) -> i64 {
    s.chars().fold(0, |acc, c| acc * 10 + digit_value(c) as i64)
}

/// `n / d` for ints, correctly rounded like Python's true division.
fn int_div(n: u64, d: u64) -> f64 {
    if n < 1 << 53 {
        return n as f64 / d as f64;
    }
    // Round n/d to 53 significant bits, half to even.
    let bits = 64 - (n / d).leading_zeros() as i32;
    let shift = bits - 53;
    let (num, den) = if shift >= 0 { (n as u128, (d as u128) << shift) } else { ((n as u128) << -shift, d as u128) };
    let mut t = num / den;
    let rem = num % den;
    if 2 * rem > den || (2 * rem == den && t & 1 == 1) {
        t += 1;
    }
    if shift >= 0 { t as f64 * 2f64.powi(shift) } else { t as f64 / 2f64.powi(-shift) }
}

/// `listing_created_at`: the UUID v1 timestamp in Unix seconds, through
/// Python's float microseconds (`uid.time / 10`).
pub fn created_at(id: u128) -> i64 {
    let time_low = (id >> 96) as u64;
    let time_mid = ((id >> 80) & 0xffff) as u64;
    let time_hi = ((id >> 64) & 0xffff) as u64;
    let ticks = ((time_hi & 0x0fff) << 48) | (time_mid << 32) | time_low;
    let us = int_div(ticks, 10).round_ties_even() as i64;
    us.div_euclid(1_000_000) - UUID_EPOCH_OFFSET
}

/// `parse_dump`: every listing in order, first sighting of a UUID wins,
/// non-positive and absurd prices dropped.
pub fn parse_dump(text: &str) -> Vec<ParsedListing> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for caps in LISTING_RE.captures_iter(text) {
        let Some(id) = parse_uuid(&caps[1]) else { continue };
        if seen.contains(&id) {
            continue;
        }
        let stack = py_int(&caps[4]);
        let price = py_int(&caps[5]);
        if stack <= 0 || price <= 0 || price > MAX_REASONABLE_PRICE {
            continue;
        }
        seen.insert(id);
        let kind = pystr::strip(&caps[3]);
        out.push(ParsedListing {
            id,
            name: pystr::strip(&caps[2]).to_owned(),
            kind: (!kind.is_empty()).then(|| kind.to_owned()),
            stack,
            price,
            price_each: py_round(price as f64 / stack as f64, 3),
            created_at: created_at(id),
        });
    }
    out
}

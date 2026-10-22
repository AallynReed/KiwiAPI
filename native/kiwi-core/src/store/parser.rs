//! `app/trove/store/parser.py`: the StoreLog.cfg dump the store bot uploads.
//! Malformed rows are dropped, never fatal; duplicate keys keep the first.

use std::collections::{BTreeMap, HashSet};

use super::py::{float, int, splitlines, unquote};
use crate::pystr::strip;

pub const PRODUCT_FIELD_COUNT: usize = 18;
pub const KINDS: [&str; 6] = ["product", "starter", "patron", "interactable", "trial", "class"];

#[derive(Clone, Debug, PartialEq)]
pub struct ParsedPrice {
    pub currency: String,
    pub cost: f64,
    pub can_purchase: bool,
    pub monthly: i64,
    pub sale: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedTexture {
    pub texture: String,
    pub x: i64,
    pub y: i64,
    pub text: String,
    pub overlay: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ParsedProduct {
    pub code: String,
    pub kind: String,
    pub name: String,
    pub image: String,
    pub owned: bool,
    pub tradable: bool,
    pub informational: bool,
    pub info: String,
    pub prices: Vec<ParsedPrice>,
    pub price_string: String,
    pub price_string_currency: String,
    pub price_string_sale: String,
    pub promo: String,
    pub deal_seconds: Option<f64>,
    pub interact_label: String,
    pub interact_enabled: bool,
    pub trial_limits: String,
    pub class_level: Option<i64>,
    pub class_power_rank: Option<i64>,
    pub class_shield_frame: Option<i64>,
    pub class_sub_name: String,
    pub class_icon: String,
    pub textures: Vec<ParsedTexture>,
    pub loot_title: String,
    pub loot_body: String,
    pub categories: Vec<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedCategory {
    pub index: i64,
    pub label: String,
    pub icon: String,
    pub codes: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ParsedStoreDump {
    pub categories: Vec<ParsedCategory>,
    pub products: Vec<ParsedProduct>,
    pub balance: Option<(f64, f64)>,
    pub title: String,
    pub instructions: String,
    pub done: bool,
}

/// Exact-duplicate drop keeping order (`x not in out`, float `==`).
fn push_unique<T: PartialEq>(out: &mut Vec<T>, item: T) {
    if !out.contains(&item) {
        out.push(item);
    }
}

fn parse_prices(raw: &str) -> Vec<ParsedPrice> {
    let mut out = Vec::new();
    for chunk in raw.split('|').filter(|c| !c.is_empty()) {
        let parts: Vec<&str> = chunk.split(':').collect();
        let [cur, cost, can, monthly, sale] = parts[..] else { continue };
        let (Some(cost), Some(monthly)) = (float(cost), int(monthly)) else { continue };
        push_unique(
            &mut out,
            ParsedPrice { currency: cur.to_owned(), cost, can_purchase: can == "1", monthly, sale: unquote(sale) },
        );
    }
    out
}

fn parse_textures(raw: &str) -> Vec<ParsedTexture> {
    let mut out = Vec::new();
    for chunk in raw.split('|').filter(|c| !c.is_empty()) {
        let parts: Vec<&str> = chunk.split(':').collect();
        let [tex, x, y, text, overlay] = parts[..] else { continue };
        let (Some(x), Some(y)) = (int(x), int(y)) else { continue };
        push_unique(
            &mut out,
            ParsedTexture { texture: unquote(tex), x, y, text: unquote(text), overlay: overlay == "1" },
        );
    }
    out
}

fn parse_product(value: &str) -> Option<ParsedProduct> {
    let f: Vec<&str> = value.split(';').collect();
    if f.len() != PRODUCT_FIELD_COUNT {
        return None;
    }
    let code = unquote(f[0]);
    let kind = f[1];
    if code.is_empty() || !KINDS.contains(&kind) {
        return None;
    }

    let (mut ps, mut ps_cur, mut ps_sale) = (String::new(), String::new(), String::new());
    if let [a, b, c] = f[9].split(':').collect::<Vec<_>>()[..] {
        (ps, ps_cur, ps_sale) = (unquote(a), b.to_owned(), unquote(c));
    }

    let deal_seconds = if f[11].is_empty() { None } else { float(f[11]) };

    let (mut i_label, mut i_enabled, mut i_trial) = (String::new(), false, String::new());
    if let [a, b, c] = f[12].split(':').collect::<Vec<_>>()[..] {
        (i_label, i_enabled, i_trial) = (unquote(a), b == "1", unquote(c));
    }

    let (mut level, mut pr, mut shield) = (None, None, None);
    let (mut sub, mut icon) = (String::new(), String::new());
    if let [a, b, c, d, e] = f[13].split(':').collect::<Vec<_>>()[..]
        && let (Some(a), Some(b), Some(c)) = (int(a), int(b), int(c))
    {
        (level, pr, shield) = (Some(a), Some(b), Some(c));
        (sub, icon) = (unquote(d), unquote(e));
    }

    let categories = if f[17].is_empty() { Vec::new() } else { f[17].split(',').filter_map(int).collect() };

    Some(ParsedProduct {
        code,
        kind: kind.to_owned(),
        name: unquote(f[2]),
        image: unquote(f[3]),
        owned: f[4] == "1",
        tradable: f[5] == "1",
        informational: f[6] == "1",
        info: unquote(f[7]),
        prices: parse_prices(f[8]),
        price_string: ps,
        price_string_currency: ps_cur,
        price_string_sale: ps_sale,
        promo: unquote(f[10]),
        deal_seconds,
        interact_label: i_label,
        interact_enabled: i_enabled,
        trial_limits: i_trial,
        class_level: level,
        class_power_rank: pr,
        class_shield_frame: shield,
        class_sub_name: sub,
        class_icon: icon,
        textures: parse_textures(f[14]),
        loot_title: unquote(f[15]),
        loot_body: unquote(f[16]),
        categories,
    })
}

fn parse_category(index: &str, value: &str) -> Option<ParsedCategory> {
    let index = int(index)?;
    let [label, icon, codes] = value.split(';').collect::<Vec<_>>()[..] else { return None };
    let codes = codes.split(',').filter(|c| !c.is_empty()).map(str::to_owned).collect();
    Some(ParsedCategory { index, label: unquote(label), icon: unquote(icon), codes })
}

/// `parse_dump(text)`.
pub fn parse_dump(text: &str) -> ParsedStoreDump {
    let mut categories: BTreeMap<i64, ParsedCategory> = BTreeMap::new();
    let mut products: Vec<ParsedProduct> = Vec::new();
    let mut codes: HashSet<String> = HashSet::new();
    let mut out = ParsedStoreDump::default();

    for line in splitlines(text) {
        let line = strip(line);
        if line.is_empty() || line.starts_with(['[', '#', ';']) {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else { continue };
        let (key, value) = (strip(key), strip(value));

        if key == "done" {
            out.done = value.to_lowercase() == "true";
        } else if let Some(idx) = key.strip_prefix("cat$") {
            if let Some(cat) = parse_category(idx, value) {
                categories.entry(cat.index).or_insert(cat);
            }
        } else if key.starts_with("prod$") {
            if let Some(prod) = parse_product(value)
                && codes.insert(prod.code.clone())
            {
                products.push(prod);
            }
        } else if key == "meta$balance" {
            if let [a, b] = value.split(';').collect::<Vec<_>>()[..]
                && let (Some(a), Some(b)) = (float(a), float(b))
            {
                out.balance = Some((a, b));
            }
        } else if key == "meta$title" {
            out.title = unquote(value);
        } else if key == "meta$instructions" {
            out.instructions = unquote(value);
        }
    }
    out.categories = categories.into_values().collect();
    out.products = products;
    out
}

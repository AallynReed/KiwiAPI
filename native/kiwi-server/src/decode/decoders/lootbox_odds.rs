//! `lootbox_odds.py`: the game's drop-rate disclosure text, split per box.

use std::sync::OnceLock;

use regex::Regex;

use crate::{
    decode::{
        common::locale_at,
        json::J,
        registry::{Built, Ctx},
    },
    obj,
};

const ITEM_INDENT: usize = 3;

fn odds() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"^(?P<name>.+?)\s+[-–—]\s+(?P<pct>[\d.]+)\s*%\s*$").expect("regex"))
}

fn indent(line: &str) -> usize {
    line.chars().count() - line.trim_start_matches([' ', '\t']).chars().count()
}

fn pct(s: &str) -> f64 {
    s.parse().unwrap_or(f64::NAN)
}

fn boxed(name: &str, lines: &[&str]) -> J {
    let mut notes = Vec::new();
    let mut rows: Vec<(usize, String, f64)> = Vec::new();
    for line in lines {
        if let Some(m) = odds().captures(line.trim()) {
            rows.push((indent(line), m["name"].trim().to_owned(), pct(&m["pct"])));
        } else if !line.trim().is_empty() {
            notes.push(J::from(line.trim()));
        }
    }
    let tiered = rows.iter().any(|(i, _, _)| *i >= ITEM_INDENT);
    let mut tiers: Vec<J> = Vec::new();
    for (ind, label, p) in rows {
        if tiered && ind < ITEM_INDENT {
            tiers.push(obj! {"name" => label, "chance" => p, "items" => Vec::<J>::new()});
        } else {
            if tiers.is_empty() {
                tiers.push(obj! {"name" => "", "chance" => J::Null, "items" => Vec::<J>::new()});
            }
            let last = tiers.last_mut().and_then(|t| t.as_dict_mut()).expect("tier");
            last["items"].as_list_mut().expect("items").push(obj! {"name" => label, "chance" => p});
        }
    }
    obj! {"name" => name, "notes" => notes, "tiers" => tiers}
}

pub fn build(ctx: &Ctx) -> Built {
    let text = locale_at(&ctx.tree, "languages/en/store_loot_probability.binfab");
    let body = text.get("$Store_Lootbox_Probabilities_Desc").cloned().unwrap_or_default().replace("\\n", "\n");
    let lines: Vec<&str> = body.split('\n').collect();
    let mut boxes = Vec::new();
    let mut name: Option<String> = None;
    let mut chunk: Vec<&str> = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let s = line.trim();
        let blank_near =
            i == 0 || lines[i - 1].trim().is_empty() || (i + 1 < lines.len() && lines[i + 1].trim().is_empty());
        if !s.is_empty()
            && blank_near
            && indent(line) == 0
            && !odds().is_match(s)
            && !s.ends_with('.')
            && s.chars().count() <= 60
        {
            if let Some(n) = name.take().filter(|n| !n.is_empty()) {
                boxes.push(boxed(&n, &chunk));
            }
            name = Some(s.replace('\u{fffd}', "'"));
            chunk.clear();
        } else if name.as_ref().is_some_and(|n| !n.is_empty()) {
            chunk.push(line);
        }
    }
    if let Some(n) = name.filter(|n| !n.is_empty()) {
        boxes.push(boxed(&n, &chunk));
    }
    let boxes: Vec<J> = boxes
        .into_iter()
        .filter(|b| b.get("tiers").is_some_and(J::truthy) || b.get("notes").is_some_and(J::truthy))
        .collect();
    Ok(obj! {
        "title" => text.get("$Store_Lootbox_Probabilities_Title").cloned().unwrap_or_default(),
        "boxes" => boxes,
    })
}

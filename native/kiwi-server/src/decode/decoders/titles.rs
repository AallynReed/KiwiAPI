//! `titles.py`: every title the client can show, from `prefabs_titles.binfab`.

use std::sync::OnceLock;

use indexmap::IndexMap;
use regex::Regex;

use crate::decode::{
    common::locale_at,
    json::{J, Map},
    registry::{Built, Ctx},
};

const KEY_PREFIX: &str = "$prefabs_titles_";

fn part() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"^(.*?)_(male_name|female_name|name|description)$").expect("regex"))
}

pub fn build(ctx: &Ctx) -> Built {
    let mut titles: IndexMap<String, IndexMap<String, String>> = IndexMap::new();
    for (key, text) in locale_at(&ctx.tree, "languages/en/prefabs_titles.binfab") {
        let Some(rest) = key.strip_prefix(KEY_PREFIX) else { continue };
        let (tid, p) = match part().captures(rest) {
            Some(m) => (m[1].to_owned(), m[2].to_owned()),
            None => (rest.to_owned(), "name".to_owned()),
        };
        titles.entry(tid).or_default().insert(p, text);
    }
    let mut out: Vec<Map> = Vec::new();
    for (tid, parts) in titles {
        let get = |k: &str| parts.get(k).cloned().unwrap_or_default();
        let mut row = Map::new();
        row.insert("id".into(), J::Str(tid));
        let mut name = get("name");
        row.insert("name".into(), J::Str(name.clone()));
        if !get("male_name").is_empty() || !get("female_name").is_empty() {
            row.insert("male".into(), J::Str(get("male_name")));
            row.insert("female".into(), J::Str(get("female_name")));
            if name.is_empty() {
                name = get("male_name");
            }
            row.insert("name".into(), J::Str(name.clone()));
        }
        if !get("description").is_empty() {
            row.insert("description".into(), J::Str(get("description")));
        }
        if !name.is_empty() {
            out.push(row);
        }
    }
    let key = |r: &Map| {
        let name = r["name"].as_str().unwrap_or("").trim_start_matches([',', ' ']).to_lowercase();
        (name, r["id"].as_str().unwrap_or("").to_owned())
    };
    out.sort_by_cached_key(key);
    Ok(J::List(out.into_iter().map(J::Dict).collect()))
}

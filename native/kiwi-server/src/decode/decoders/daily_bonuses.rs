//! `daily_bonuses.py`: the Daily Bonus for each day of the week.

use std::sync::OnceLock;

use indexmap::IndexMap;
use kiwi_core::abc;
use regex::bytes::Regex;

use super::badges::Text;
use crate::decode::{
    ability::modifiers,
    common::walk_values,
    json::{J, Map},
    registry::{Built, Ctx, DecodeError},
    value::{Dict, W, parse},
};
use crate::obj;

const TABLE: &str = "prefabs/loot/dailybonus.binfab";
const SWF: &str = "ui/welcomescreen.swf";
const WINDOW: &str = "WelcomeDailyBonusWindow";

fn leaf_of(v: Option<&W>) -> &Dict {
    match v {
        Some(W::Dict(d)) => d,
        Some(o) => o.leaf(),
        None => &crate::decode::value::EMPTY,
    }
}

fn abc_j(v: &abc::Val) -> J {
    match v {
        abc::Val::None => J::Null,
        abc::Val::Bool(b) => J::Bool(*b),
        abc::Val::Int(i) => J::Int(i128::from(*i)),
        abc::Val::Float(f) => J::Float(*f),
        abc::Val::Str(s) => J::Str(s.clone()),
        abc::Val::List(xs) => J::List(xs.iter().map(abc_j).collect()),
    }
}

fn abc_str(v: &abc::Val) -> String {
    match v {
        abc::Val::Str(s) => s.clone(),
        abc::Val::Int(i) => i.to_string(),
        abc::Val::Float(f) => crate::pyfloat::repr(*f),
        abc::Val::Bool(b) => if *b { "True" } else { "False" }.into(),
        abc::Val::None => "None".into(),
        abc::Val::List(_) => String::new(),
    }
}

/// `_sprite_child_refs`: character ids placed on a DefineSprite's timeline.
fn sprite_child_refs(body: &[u8]) -> Vec<u16> {
    let mut refs = Vec::new();
    let mut p = 4;
    loop {
        let Some(head) = body.get(p..p + 2) else { return refs };
        p += 2;
        let packed = u16::from_le_bytes([head[0], head[1]]);
        let (code, mut len) = (packed >> 6, (packed & 0x3F) as usize);
        if len == 0x3F {
            let Some(raw) = body.get(p..p + 4) else { return refs };
            p += 4;
            len = u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]) as usize;
        }
        let end = (p + len).min(body.len());
        let nested = &body[p.min(body.len())..end];
        p = end;
        if code == 0 {
            return refs;
        }
        if !matches!(code, 4 | 26 | 70) || nested.len() < 4 {
            continue;
        }
        let h = |o: usize| nested.get(o..o + 2).map(|b| u16::from_le_bytes([b[0], b[1]]));
        if code == 4 {
            refs.extend(h(0));
        } else if code == 26 && nested[0] & 0x02 != 0 {
            refs.extend(h(3));
        } else if code == 70 && nested[0] & 0x02 != 0 {
            let mut off = 4;
            if nested[1] & 0x08 != 0 || nested[1] & 0x10 != 0 {
                off = nested[off..].iter().position(|&b| b == 0).map_or(nested.len(), |e| off + e + 1);
            }
            if off + 2 <= nested.len() {
                refs.extend(h(off));
            }
        }
    }
}

fn titles(movie: &[u8]) -> Result<IndexMap<String, String>, DecodeError> {
    static KEY: OnceLock<Regex> = OnceLock::new();
    let key = KEY.get_or_init(|| Regex::new(r"(?-u)\$Welcome_DailyBonus_[A-Za-z0-9_]+").expect("regex"));
    let tags =
        kiwi_core::swf::tags(movie).ok_or_else(|| DecodeError { kind: "SwfError", msg: "truncated rect".into() })?;
    let mut texts: IndexMap<u16, String> = IndexMap::new();
    let mut sprites: Vec<Vec<u16>> = Vec::new();
    for (code, body) in tags {
        if code == 37 {
            if let Some(m) = key.find(body) {
                let id = u16::from_le_bytes([body.first().copied().unwrap_or(0), body.get(1).copied().unwrap_or(0)]);
                texts.insert(id, String::from_utf8_lossy(m.as_bytes()).into_owned());
            }
        } else if code == 39 {
            sprites.push(sprite_child_refs(body));
        }
    }
    let mut out = IndexMap::new();
    for kids in sprites {
        let keys: Vec<&String> = kids.iter().filter_map(|k| texts.get(k)).collect();
        let day_titles: Vec<&&String> = keys.iter().filter(|k| k.ends_with("Day")).collect();
        for weekday in keys.iter().filter(|k| !k.ends_with("Day")) {
            if day_titles.len() == 1 {
                out.insert((*weekday).clone(), (*day_titles[0]).clone());
            }
        }
    }
    Ok(out)
}

fn effects(raw: Option<&W>) -> (Vec<J>, Vec<J>) {
    let (mut stats, mut multipliers): (Vec<J>, Vec<J>) = (Vec::new(), Vec::new());
    let rows: Vec<W> = match raw {
        Some(x) if x.is_list() => x.elems().to_vec(),
        _ => Vec::new(),
    };
    for row in rows {
        let e = leaf_of(Some(&row));
        let params = leaf_of(e.get(1));
        match e.str(0) {
            Some("stats") => {
                let found = modifiers(&params.get(0).map(walk_values).unwrap_or_else(|| vec![W::None]));
                let fresh: Vec<J> = found.into_iter().filter(|m| !stats.contains(m)).collect();
                stats.extend(fresh);
            }
            Some("pvpexperience") => {
                if let Some(v) = params.get(0).filter(|v| matches!(v, W::Int(_) | W::Float(_))) {
                    multipliers.push(obj! {"kind" => "pvpexperience", "multiplier" => J::from(v)});
                }
            }
            Some("claim") => {
                if let Some(c) = params.str(1) {
                    multipliers.push(
                        obj! {"kind" => "claim", "claim" => c, "multiplier" => params.get(0).map_or(J::Null, J::from)},
                    );
                }
            }
            _ => {}
        }
    }
    (stats, multipliers)
}

pub fn build(ctx: &Ctx) -> Built {
    let tree = &ctx.tree;
    let swf = tree.read(SWF).filter(|s| !s.is_empty()).ok_or_else(|| DecodeError::value(format!("{SWF} missing")))?;
    let arrays =
        abc::movie_static_arrays(&swf).ok_or_else(|| DecodeError { kind: "SwfError", msg: "bad ABC block".into() })?;
    let window = arrays.iter().find(|(c, _)| c == WINDOW).map(|(_, a)| a.clone()).unwrap_or_default();
    let get = |name: &str| -> Vec<abc::Val> {
        match window.iter().find(|(n, _)| n == name).map(|(_, v)| v) {
            Some(abc::Val::List(xs)) => xs.clone(),
            _ => Vec::new(),
        }
    };
    let (sets, weekdays) = (get("BONUSES"), get("DAYS_L"));
    if sets.len() != 7 || weekdays.len() != 7 {
        return Err(DecodeError::value(format!("{WINDOW}.BONUSES/DAYS_L not found")));
    }
    let movie = kiwi_core::swf::movie(&swf).ok_or_else(|| DecodeError { kind: "SwfError", msg: "bad movie".into() })?;
    let titles = titles(&movie)?;
    let text = Text::new(tree);
    let pf = parse(&tree.read(TABLE).unwrap_or_default())
        .map_err(|_| DecodeError { kind: "WireError", msg: "no reading fills the object exactly".into() })?;
    let mut days: Vec<Map> = Vec::new();
    let rows = pf.root.as_ref().filter(|r| !r.is_empty()).map(|r| r.leaf().iter_of(0)).unwrap_or_default();
    for row in rows {
        let r = leaf_of(Some(&row));
        let Some(day) = r.int(0).filter(|d| (0..7).contains(d)) else { continue };
        let d = day as usize;
        let set = abc_str(&sets[d]);
        let lines: Vec<String> = (1..=4).map(|i| format!("$Welcome_daily_bonus_details_{set}_{i}")).collect();
        let (stats, multipliers) = effects(r.get(8));
        let name_key = r.str(1).and_then(|k| titles.get(k)).cloned().unwrap_or_default();
        let mut m = Map::new();
        m.insert("day".into(), J::Int(day));
        m.insert("weekday".into(), abc_j(&weekdays[d]));
        m.insert("name".into(), J::from(text.of(&name_key)));
        m.insert("set".into(), abc_j(&sets[d]));
        m.insert(
            "bonuses".into(),
            J::from(lines.iter().map(|k| text.of(k)).filter(|t| !t.is_empty()).collect::<Vec<_>>()),
        );
        m.insert(
            "patron_bonuses".into(),
            J::from(
                lines.iter().map(|k| text.of(&format!("{k}_patron"))).filter(|t| !t.is_empty()).collect::<Vec<_>>(),
            ),
        );
        m.insert("stats".into(), J::List(stats));
        m.insert("multipliers".into(), J::List(multipliers));
        days.push(m);
    }
    days.sort_by_key(|d| d["day"].as_int().unwrap_or(0));
    Ok(obj! {"days" => J::List(days.into_iter().map(J::Dict).collect())})
}

//! `badges.py`: every badge group, its ranks, what each asks and pays.

use std::{
    cell::RefCell,
    collections::HashMap,
    sync::{Arc, OnceLock},
};

use indexmap::{IndexMap, IndexSet};
use regex::bytes::Regex;

use crate::decode::{
    ability::{Prefabs, identity, modifiers},
    common::{casefold, commas, locale, stem, walk_values},
    json::{J, Map},
    registry::{Built, Ctx},
    tree::Tree,
    value::{Dict, W, parse},
};

pub const EXE_PATH: &str = "Trove_x64.exe";
const BADGES: &str = "meta/badges";
const CLAIMS: &str = "claim/badge";
const CATEGORIES: &str = "collections/collection_badge";
const BADGE_DIR: &str = "prefabs/collections/badge/";
const RANKS: [&str; 7] = ["bronze", "silver", "gold", "platinum", "diamond", "obsidian", "trovium"];
const DIFFICULTIES: [&str; 3] = ["Normal", "Hard", "Ultra"];
const BADGE_LINK: i128 = 299;
const MODEL: i128 = 37;
const DRAGON_SOUL: i128 = 301;
const GEM_STATUS: [&str; 2] = ["$GemStatsBadgeFormat", "$AllGemStatsBadgeFormat"];

fn status(kind: &str) -> Option<&'static str> {
    match kind {
        "friends" => Some("$FriendsBadageStatusFormat"),
        "loyalty" => Some("$LoyaltyBadgeStatusFormat"),
        "referafriend" => Some("$RAFBadgeStatusFormat"),
        _ => None,
    }
}

/// `codexes.badges.parse_metric_names`: the `PlayerMetric` enum names, by ordinal.
pub fn parse_metric_names(exe: &[u8]) -> Vec<String> {
    static IDENT: OnceLock<Regex> = OnceLock::new();
    static TAIL: OnceLock<Regex> = OnceLock::new();
    let ident = IDENT.get_or_init(|| Regex::new(r"(?-u)^[A-Za-z_][A-Za-z0-9_]*$").expect("regex"));
    let tail = TAIL.get_or_init(|| Regex::new(r"(?-u)[A-Za-z_][A-Za-z0-9_]*$").expect("regex"));
    let Some(end) = memchr::memmem::find(exe, b"KPlayerMetric\x00") else { return Vec::new() };
    let mut names: Vec<String> = Vec::new();
    let mut pos = end;
    while pos > 0 {
        let mut stop = pos - 1;
        while stop > 0 && exe[stop] == 0 {
            stop -= 1;
        }
        if stop == 0 {
            break;
        }
        let mut start = stop;
        while start > 0 && (0x20..0x7F).contains(&exe[start - 1]) {
            start -= 1;
        }
        let run = &exe[start..=stop];
        if !ident.is_match(run) {
            if let Some(m) = tail.find(run)
                && m.len() >= 3
            {
                names.push(String::from_utf8_lossy(m.as_bytes()).into_owned());
            }
            break;
        }
        if run.len() < 3 {
            break;
        }
        names.push(String::from_utf8_lossy(run).into_owned());
        pos = start;
    }
    if names.is_empty() {
        return names;
    }
    names.reverse();
    let mut out = vec![String::new()];
    out.extend(names);
    out
}

/// `$key -> English text`, parsing only the tables whose bytes hold a wanted key.
pub struct Text {
    raw: RefCell<IndexMap<String, Vec<u8>>>,
    map: RefCell<HashMap<String, String>>,
}

impl Text {
    pub fn new(tree: &Tree) -> Text {
        let raw = tree
            .files("languages/en/", ".binfab")
            .into_iter()
            .map(|p| (p.clone(), tree.read(&p).unwrap_or_default()))
            .collect();
        Text { raw: RefCell::new(raw), map: RefCell::default() }
    }

    pub fn get(&self, key: Option<&W>) -> String {
        match key {
            Some(W::Str(s)) => self.of(s),
            _ => String::new(),
        }
    }

    pub fn of(&self, key: &str) -> String {
        if !key.starts_with('$') {
            return String::new();
        }
        if let Some(v) = self.map.borrow().get(key) {
            return v.clone();
        }
        let hits: Vec<String> = self
            .raw
            .borrow()
            .iter()
            .filter(|(_, data)| memchr::memmem::find(data, key.as_bytes()).is_some())
            .map(|(p, _)| p.clone())
            .collect();
        for path in hits {
            let data = self.raw.borrow_mut().shift_remove(&path).unwrap_or_default();
            let mut map = self.map.borrow_mut();
            for (k, v) in locale(Some(&data)) {
                map.entry(k).or_insert(v);
            }
        }
        self.map.borrow_mut().entry(key.to_owned()).or_default().clone()
    }
}

fn rows(v: Option<&W>) -> Vec<Arc<Dict>> {
    v.map(W::iter)
        .unwrap_or_default()
        .iter()
        .filter(|r| r.is_obj())
        .map(|r| Arc::new(Dict::new(r.leaf().items().to_vec())))
        .collect()
}

/// A `{0}/{1} ...` progress line turned into its goal.
fn goal(fmt: &str, amount: &J, rest: &[&str]) -> String {
    if !fmt.contains("{0}/{1}") {
        return String::new();
    }
    let mut out = fmt.replace("{0}/{1}", &fmt_commas(amount));
    for (i, text) in rest.iter().enumerate() {
        out = out.replace(&format!("{{{}}}", i + 2), text);
    }
    out
}

/// `f"{v:,}"`.
pub fn fmt_commas(v: &J) -> String {
    match v {
        J::Int(i) => commas(*i),
        J::Bool(b) => commas(i128::from(*b)),
        J::Float(f) => {
            let r = crate::pyfloat::repr(*f);
            match r.split_once('.') {
                Some((int, frac)) if !r.contains('e') => {
                    let (sign, digits) = int.strip_prefix('-').map_or(("", int), |d| ("-", d));
                    format!("{sign}{}.{frac}", commas(digits.parse::<i128>().unwrap_or(0)))
                }
                _ => r,
            }
        }
        _ => String::new(),
    }
}

fn val(d: &Dict, i: i128, default: J) -> J {
    d.get(i).map_or(default, J::from)
}

fn metric_name(metrics: &[String], mid: &J) -> String {
    match mid {
        J::Int(i) if *i > 0 && (*i as usize) < metrics.len() => metrics[*i as usize].clone(),
        _ => String::new(),
    }
}

struct Dragon {
    dragon: String,
    souls: Vec<(String, String)>,
}

fn dragons(tree: &Tree, prefabs: &Prefabs, text: &Text) -> IndexMap<i128, Dragon> {
    let mut out: IndexMap<i128, Dragon> = IndexMap::new();
    for path in tree.files("prefabs/dragonsoul/", ".binfab") {
        let pf = prefabs.get(&path["prefabs/".len()..]);
        if let Some(did) = pf.as_ref().and_then(|p| p.root.as_ref()).and_then(|r| r.leaf().int(0)) {
            out.insert(did, Dragon { dragon: stem(&path), souls: Vec::new() });
        }
    }
    for path in tree.files("prefabs/item/dragon/", ".binfab") {
        if path.matches('/').count() != 3 {
            continue;
        }
        let rel = &path["prefabs/".len()..path.len() - ".binfab".len()];
        let pf = prefabs.get(rel);
        let did = pf.as_ref().and_then(|p| p.component(DRAGON_SOUL)).and_then(|c| c.leaf().int(0));
        if let Some(d) = did.and_then(|d| out.get_mut(&d)) {
            let name = text.of(identity(pf.as_deref()).name());
            d.souls.push((rel.to_owned(), name));
        }
    }
    out
}

fn requirement(req: Option<&W>, text: &Text, metrics: &[String], dragons: &IndexMap<i128, Dragon>) -> Map {
    let top = req.map(W::leaf).cloned_or_empty();
    let kind = top.get(0).filter(|v| v.truthy()).map_or(J::from(""), J::from);
    let p = top.get(1).map(W::leaf).cloned_or_empty();
    let mut out = Map::new();
    out.insert("kind".into(), kind.clone());
    let kind_s = kind.as_str().unwrap_or("").to_owned();
    match kind_s.as_str() {
        "metric" => {
            let (mid, amount) = (val(&p, 0, J::Int(0)), val(&p, 1, J::Int(0)));
            out.insert("metric_id".into(), mid.clone());
            out.insert("amount".into(), amount.clone());
            let name = metric_name(metrics, &mid);
            if !name.is_empty() {
                out.insert("metric".into(), J::from(name.as_str()));
                let label = text.of(&format!("$Metrics_{name}"));
                if !label.is_empty() {
                    out.insert("label".into(), J::from(label.as_str()));
                    out.insert("text".into(), J::from(format!("{} {label}", fmt_commas(&amount))));
                }
            }
        }
        "dragonsouls" => {
            let (did, amount) = (val(&p, 0, J::Int(0)), val(&p, 1, J::Int(0)));
            out.insert("dragon_id".into(), did.clone());
            out.insert("amount".into(), amount.clone());
            if let Some(dragon) = did.as_int().and_then(|d| dragons.get(&d)) {
                out.insert("dragon".into(), J::from(dragon.dragon.as_str()));
                let names: IndexSet<&String> = dragon.souls.iter().map(|(_, n)| n).filter(|n| !n.is_empty()).collect();
                if !dragon.souls.is_empty() {
                    out.insert(
                        "soul_items".into(),
                        J::List(dragon.souls.iter().map(|(r, _)| J::from(r.as_str())).collect()),
                    );
                }
                if names.len() == 1 {
                    let soul = names[0].clone();
                    out.insert("soul_name".into(), J::from(soul.as_str()));
                    if let Some(base) = soul.strip_suffix(" Dragon Soul") {
                        out.insert(
                            "text".into(),
                            J::from(goal(&text.of("$DragonSoulsBadgeStatusFormat"), &amount, &[base])),
                        );
                    }
                }
            }
        }
        "STBossKilled" => {
            let diff = val(&p, 2, J::Int(0));
            out.insert("boss".into(), val(&p, 0, J::from("")));
            out.insert("amount".into(), val(&p, 1, J::Int(0)));
            out.insert("difficulty_id".into(), diff.clone());
            if let Some(d) = diff.as_int().and_then(|d| usize::try_from(d).ok()).and_then(|d| DIFFICULTIES.get(d)) {
                out.insert("difficulty".into(), J::from(*d));
            }
        }
        "UpgradeGems" => {
            let (scope, amount) = (val(&p, 0, J::Int(0)), val(&p, 1, J::Int(0)));
            out.insert("all_stats".into(), J::Bool(scope == J::Int(1)));
            out.insert("amount".into(), amount.clone());
            if scope == J::Int(0) || scope == J::Int(1) {
                let i = usize::from(scope == J::Int(1));
                out.insert("text".into(), J::from(goal(&text.of(GEM_STATUS[i]), &amount, &[])));
            }
        }
        "SubClassEquipped" => {
            let (level, power) = (val(&p, 0, J::Int(0)), val(&p, 1, J::Int(0)));
            out.insert("level".into(), level.clone());
            out.insert("power_rank".into(), power.clone());
            let fmt = text.of(if level.truthy() { "$SubClassEquippedType" } else { "$SubClassEquippedTypeNoLevel" });
            let t = if level.truthy() {
                fmt.replace("{0}", &fmt_commas(&level)).replace("{1}", &fmt_commas(&power))
            } else {
                fmt.replace("{0}", &fmt_commas(&power))
            };
            out.insert("text".into(), J::from(t));
        }
        "minprestigelevelsacrossclasses" => {
            let (level, classes) = (val(&p, 0, J::Int(0)), val(&p, 1, J::Int(0)));
            out.insert("paragon_level".into(), level.clone());
            out.insert("classes".into(), classes.clone());
            let fmt = text.of("$MinPrestigeLevelsAcrossClassesBadgeStatusType");
            if !fmt.is_empty() {
                out.insert(
                    "text".into(),
                    J::from(format!("{} {}", fmt_commas(&classes), fmt.replace("{0}", &fmt_commas(&level)))),
                );
            }
        }
        "tinyquesttotalpetsoflevel" => {
            out.insert("ally_level".into(), val(&p, 0, J::Int(0)));
            out.insert("allies".into(), val(&p, 1, J::Int(0)));
        }
        "tinyquestconcurrentbuffedpets" => {
            out.insert("allies".into(), val(&p, 0, J::Int(0)));
        }
        "entitlementUnlocked" => {
            out.insert("entitlement".into(), val(&p, 0, J::from("")));
            let t = text.get(p.get(1));
            if !t.is_empty() {
                out.insert("text".into(), J::from(t));
            }
        }
        k if status(k).is_some() => {
            let amount = val(&p, 0, J::Int(0));
            out.insert("amount".into(), amount.clone());
            out.insert("text".into(), J::from(goal(&text.of(status(k).unwrap_or("")), &amount, &[])));
        }
        "none" => {}
        _ => {
            let payload: Map = p
                .items()
                .iter()
                .filter(|(_, v)| matches!(v, W::Int(_) | W::Float(_) | W::Str(_)))
                .map(|(k, v)| {
                    let key = match k {
                        crate::decode::value::K::Int(i) => i.to_string(),
                        crate::decode::value::K::Str(s) => s.to_string(),
                    };
                    (key, J::from(v))
                })
                .collect();
            out.insert("payload".into(), J::Dict(payload));
        }
    }
    out.retain(|_, v| *v != J::from(""));
    out
}

trait OrEmpty {
    fn cloned_or_empty(self) -> Dict;
}

impl OrEmpty for Option<&Dict> {
    fn cloned_or_empty(self) -> Dict {
        self.map(|d| Dict::new(d.items().to_vec())).unwrap_or_default()
    }
}

/// `_grants`: what one claim hands out.
pub fn grants(claim: Option<&W>, prefabs: &Prefabs, text: &Text) -> Vec<J> {
    let top = claim.map(W::leaf).cloned_or_empty();
    let kind = top.get(0);
    let Some(params) = top.get(1).filter(|p| p.is_obj()) else { return Vec::new() };
    let secs = params.sections();
    let sec = |i: usize| secs.get(i).and_then(W::as_dict).cloned_or_empty();
    let named = |r: &str| text.of(identity(prefabs.get(r).as_deref()).name());
    let mut out: Vec<Map> = Vec::new();
    let kind_s = kind.and_then(W::as_str).unwrap_or("");
    match kind_s {
        "ClaimCollectable" => {
            for row in rows(sec(1).get(0)) {
                let r = row.get(1).map_or(J::from(""), J::from);
                let rs = r.as_str().unwrap_or("").to_owned();
                let mut g = Map::new();
                g.insert("type".into(), J::from("collectable"));
                if rs.ends_with(".blueprint") {
                    g.insert("blueprint".into(), r);
                } else if prefabs.exists(&rs) {
                    g.insert("prefab".into(), r);
                    g.insert("name".into(), J::from(named(&rs)));
                } else {
                    g.insert("id".into(), r);
                }
                out.push(g);
            }
        }
        "ClaimPrefab" => {
            for row in rows(sec(1).get(0)) {
                let r = row.get(0).map_or(J::from(""), J::from);
                let rs = r.as_str().unwrap_or("").to_owned();
                let mut g = Map::new();
                g.insert("type".into(), J::from("item"));
                g.insert("prefab".into(), r);
                g.insert("name".into(), J::from(named(&rs)));
                g.insert("count".into(), row.get(1).map_or(J::Int(1), J::from));
                out.push(g);
            }
        }
        "ClaimPoints" => {
            let mut g = Map::new();
            g.insert("type".into(), J::from("points"));
            g.insert("amount".into(), sec(2).get(0).map_or(J::Int(0), J::from));
            g.insert("name".into(), J::from(text.get(sec(0).get(3))));
            g.insert("icon".into(), sec(0).get(6).map_or(J::from(""), J::from));
            out.push(g);
        }
        "ClaimCompound" => {
            let mut all = Vec::new();
            for sub in sec(1).iter_of(0) {
                all.extend(grants(Some(&sub), prefabs, text));
            }
            return all;
        }
        _ => {
            let mut g = Map::new();
            g.insert("type".into(), kind.filter(|k| k.truthy()).map_or(J::from(""), J::from));
            out.push(g);
        }
    }
    out.into_iter()
        .map(|mut g| {
            g.retain(|_, v| *v != J::from(""));
            J::Dict(g)
        })
        .collect()
}

fn collectibles(tree: &Tree, prefabs: &Prefabs, text: &Text) -> IndexMap<(String, i128), Map> {
    let mut out = IndexMap::new();
    for path in tree.files(BADGE_DIR, ".binfab") {
        if path.matches('/').count() != 3 {
            continue;
        }
        let rel = &path["prefabs/".len()..path.len() - ".binfab".len()];
        let pf = prefabs.get(rel);
        let link = pf.as_ref().and_then(|p| p.component(BADGE_LINK)).map(W::leaf).cloned_or_empty();
        let Some(gid) = link.str(0) else { continue };
        let ident = identity(pf.as_deref());
        let mut row = Map::new();
        row.insert("prefab".into(), J::from(rel));
        row.insert("name".into(), J::from(text.of(ident.name())));
        row.insert("description".into(), J::from(text.of(ident.desc())));
        if let Some(m) = pf
            .as_ref()
            .and_then(|p| p.component(MODEL))
            .and_then(|c| c.leaf().str(0))
            .filter(|m| m.ends_with(".blueprint"))
        {
            row.insert("blueprint".into(), J::from(m));
        }
        let stats = modifiers(&link.get(2).map(walk_values).unwrap_or_else(|| vec![W::None]));
        if !stats.is_empty() {
            row.insert("stats".into(), J::List(stats));
        }
        out.insert((casefold(gid), link.int(1).unwrap_or(0)), row);
    }
    out
}

struct Listed {
    category: J,
    tags: Vec<J>,
}

fn categories(prefabs: &Prefabs, text: &Text) -> IndexMap<String, Listed> {
    let mut out = IndexMap::new();
    let pf = prefabs.get(CATEGORIES);
    let root = pf.as_ref().and_then(|p| p.root.as_ref()).filter(|r| !r.is_empty());
    for cat in rows(root.and_then(|r| r.get(0))) {
        let t = text.get(cat.get(1));
        let name = if t.is_empty() { cat.get(0).map_or(J::from(""), J::from) } else { J::from(t) };
        for item in rows(cat.get(3)) {
            if let Some(r) = item.str(0) {
                let tags: Vec<J> = item
                    .iter_of(1)
                    .iter()
                    .filter_map(W::as_str)
                    .map(|t| {
                        let x = text.of(t);
                        J::from(if x.is_empty() { t.to_owned() } else { x })
                    })
                    .collect();
                out.insert(casefold(r), Listed { category: name.clone(), tags });
            }
        }
    }
    out
}

fn group_name(ranks: &[Map], category: &str, sole: bool) -> String {
    let titles: IndexSet<String> = ranks
        .iter()
        .map(|r| {
            let name = r.get("name").and_then(J::as_str).unwrap_or("");
            name.split_once("Badge: ").map_or(String::new(), |(_, t)| t.to_owned())
        })
        .collect();
    if titles.len() == 1 && !titles.contains("") {
        return titles[0].clone();
    }
    if !category.is_empty() && sole {
        return category.to_owned();
    }
    let labels: IndexSet<String> = ranks
        .iter()
        .map(|r| r.get("requirement").and_then(|q| q.get("label")).and_then(J::as_str).unwrap_or("").to_owned())
        .collect();
    if labels.len() == 1 && !labels.contains("") {
        return labels[0].clone();
    }
    category.to_owned()
}

pub fn build(ctx: &Ctx) -> Built {
    let tree = &ctx.tree;
    let prefabs = Prefabs::new(tree);
    let text = Text::new(tree);
    let metrics = tree.read(EXE_PATH).map(|e| parse_metric_names(&e)).unwrap_or_default();
    let dragons = dragons(tree, &prefabs, &text);
    let claims: Option<W> = parse(&tree.read(&format!("prefabs/{CLAIMS}.binfab")).unwrap_or_default())
        .ok()
        .and_then(|pf| pf.root_leaf().get(0).filter(|v| v.truthy()).cloned());
    let Some(meta) = prefabs.get(BADGES) else { return Ok(J::List(Vec::new())) };
    let Some(mroot) = meta.root.as_ref() else { return Ok(J::List(Vec::new())) };
    let mut table: IndexMap<String, Arc<Dict>> = IndexMap::new();
    let mut table_other: Vec<(W, Arc<Dict>)> = Vec::new();
    for r in rows(mroot.get(0)) {
        match r.get(0) {
            Some(W::Str(s)) => {
                table.insert(s.to_string(), r.clone());
            }
            Some(other) => table_other.push((other.clone(), r.clone())),
            None => table_other.push((W::None, r.clone())),
        }
    }
    let collectibles = collectibles(tree, &prefabs, &text);
    let listed = categories(&prefabs, &text);
    let mut groups_by_category: IndexMap<String, IndexSet<String>> = IndexMap::new();
    for ((gid, _), c) in &collectibles {
        let prefab = casefold(c.get("prefab").and_then(J::as_str).unwrap_or(""));
        if let Some(cat) = listed.get(&prefab) {
            let key = cat.category.as_str().unwrap_or("").to_owned();
            groups_by_category.entry(key).or_default().insert(gid.clone());
        }
    }

    let claim_of = |cid: &J| -> Option<W> {
        let table = claims.as_ref()?.as_dict()?;
        let node = match cid {
            J::Str(s) => table.get_s(s),
            J::Int(i) => table.get(*i),
            _ => None,
        }?;
        Some(node.clone())
    };

    let mut out: Vec<Map> = Vec::new();
    for group in rows(mroot.get(1)) {
        let gid = group.get(0).map_or(J::from(""), J::from);
        let key = casefold(gid.as_str().unwrap_or(""));
        let mut ranks: Vec<Map> = Vec::new();
        for rank in rows(group.get(1)) {
            let n = rank.int(0).unwrap_or(0);
            let mut row = Map::new();
            row.insert("rank".into(), rank.get(0).map_or(J::Int(0), J::from));
            row.insert(
                "tier".into(),
                J::from(usize::try_from(n).ok().and_then(|i| RANKS.get(i)).copied().unwrap_or("")),
            );
            let badge = collectibles.get(&(key.clone(), n));
            if let Some(b) = badge {
                row.extend(b.clone());
            }
            row.insert("requirement".into(), J::Dict(requirement(rank.get(3), &text, &metrics, &dragons)));
            let mut rewards = Vec::new();
            for rid in rank.iter_of(2) {
                let Some(rid) = rid.as_str() else { continue };
                let rf = table.get(rid).map(|d| Dict::new(d.items().to_vec())).unwrap_or_default();
                let cid = rf.get(2).map_or(J::from(""), J::from);
                let node = claim_of(&cid);
                let g = grants(node.as_ref().map(W::leaf).and_then(|l| l.get(1)), &prefabs, &text);
                if let Some(b) = badge {
                    let mine: Vec<String> =
                        g.iter().map(|x| casefold(x.get("prefab").and_then(J::as_str).unwrap_or(""))).collect();
                    if mine == vec![casefold(b.get("prefab").and_then(J::as_str).unwrap_or(""))] {
                        continue;
                    }
                }
                let mut reward = Map::new();
                reward.insert("id".into(), J::from(rid));
                reward.insert("name".into(), J::from(text.get(rf.get(1))));
                if !g.is_empty() {
                    reward.insert("grants".into(), J::List(g));
                }
                reward.retain(|_, v| *v != J::from(""));
                rewards.push(J::Dict(reward));
            }
            row.insert("rewards".into(), J::List(rewards));
            row.retain(|_, v| *v != J::from(""));
            ranks.push(row);
        }
        let mine: Vec<&Map> = collectibles.iter().filter(|((g, _), _)| *g == key).map(|(_, c)| c).collect();
        let pref = |c: &Map| casefold(c.get("prefab").and_then(J::as_str).unwrap_or(""));
        let cats: Vec<&Listed> = mine.iter().filter_map(|c| listed.get(&pref(c))).collect();
        let category = cats.first().map_or(J::from(""), |c| c.category.clone());
        let category_s = category.as_str().unwrap_or("").to_owned();
        let mut tags: Vec<String> =
            cats.iter().flat_map(|c| c.tags.iter().filter_map(|t| t.as_str().map(str::to_owned))).collect();
        tags.sort();
        tags.dedup();
        let ranked: Vec<Option<&J>> = ranks.iter().map(|r| r.get("prefab")).collect();
        let unranked: Vec<J> = mine
            .iter()
            .filter(|c| !ranked.contains(&c.get("prefab")) && listed.contains_key(&pref(c)))
            .map(|c| J::Dict((*c).clone()))
            .collect();
        let descriptions: IndexSet<String> =
            ranks.iter().map(|r| r.get("description").and_then(J::as_str).unwrap_or("").to_owned()).collect();
        let sole = groups_by_category.get(&category_s).is_some_and(|s| s.len() == 1 && s.contains(&key));
        let name = group_name(&ranks, &category_s, sole);
        let mut entry = Map::new();
        entry.insert("slug".into(), J::from(key.as_str()));
        entry.insert("name".into(), if name.is_empty() { gid.clone() } else { J::from(name) });
        entry.insert(
            "description".into(),
            J::from(if descriptions.len() == 1 { descriptions[0].clone() } else { String::new() }),
        );
        entry.insert("category".into(), category);
        entry.insert("tags".into(), J::from(tags));
        entry.insert("ranks".into(), J::List(ranks.into_iter().map(J::Dict).collect()));
        if !unranked.is_empty() {
            entry.insert("unranked".into(), J::List(unranked));
        }
        out.push(entry);
    }
    out.sort_by_cached_key(|g| {
        (g["category"].as_str().unwrap_or("").to_lowercase(), g["name"].as_str().unwrap_or("").to_lowercase())
    });
    let _ = table_other;
    Ok(J::List(out.into_iter().map(J::Dict).collect()))
}

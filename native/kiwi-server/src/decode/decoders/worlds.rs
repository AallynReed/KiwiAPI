//! `worlds.py`: worlds, biomes, Delve gateways and the Shadow Tower.

use indexmap::IndexMap;

use crate::decode::{
    common::{Locale, locale_at, r4, resolve_stat_name_in, rp, rs, stat_key, stem, title},
    json::{J, Map},
    registry::{Built, Ctx},
    tree::Tree,
    value::{Dict, K, Prefab, W, parse},
};
use crate::obj;

const PORTALS: &str = "prefabs/placeable/portal/";
const MATCHMAKERS: &str = "prefabs/placeable/matchmaker/";
const IDENTITY: i128 = 49;
const BLUEPRINT: i128 = 37;
const PLACER: i128 = 50;
const PORTAL: i128 = 154;
const REQUIREMENTS: i128 = 239;
const DELVE: i128 = 493;
const TYPE_ORDER: [i128; 5] = [1, 15, 14, 22, 12];
const ZONE_LEVELS: [&str; 4] = ["$ZoneLevel_Novice", "$ZoneLevel_Adept", "$ZoneLevel_Elite", "$ZoneLevel_Master"];
const TOWER_DIFFICULTIES: [&str; 3] =
    ["$ShadowTower_DifficultyNormal", "$ShadowTower_DifficultyHard", "$ShadowTower_DifficultyUltra"];
const SYNERGY_PREFIX: &str = "TQ_Tag_Biome_";

fn portal_type(kind: &W) -> Option<&'static str> {
    match kind {
        W::Int(1) | W::Int(15) => Some("$WorldPortalType_Adventure"),
        W::Int(12) => Some("$WorldPortalType_Arena"),
        W::Int(14) => Some("$WorldPortalType_Adventure2"),
        W::Int(22) => Some("$WorldPortalType_Discovery"),
        _ => None,
    }
}

fn key_str(k: &K) -> String {
    match k {
        K::Int(i) => i.to_string(),
        K::Str(s) => s.to_string(),
    }
}

/// `_list`: a dict's values by `str(key)`, a list's or tuple's items.
fn list(v: Option<&W>) -> Vec<W> {
    match v {
        Some(W::Dict(d)) => {
            let mut items: Vec<&(K, W)> = d.items().iter().collect();
            items.sort_by_key(|(k, _)| key_str(k));
            items.into_iter().map(|(_, v)| v.clone()).collect()
        }
        Some(x) => x.elems().to_vec(),
        None => Vec::new(),
    }
}

fn strs(v: Option<&W>) -> Vec<String> {
    list(v).iter().filter_map(|s| s.as_str().filter(|s| !s.is_empty()).map(str::to_owned)).collect()
}

fn rel_of(path: &str) -> &str {
    rs(rp(path, "prefabs/"), ".binfab")
}

fn parse_at(tree: &Tree, path: &str) -> Option<Prefab> {
    parse(&tree.read(path).unwrap_or_default()).ok()
}

struct Text {
    table: Locale,
}

impl Text {
    fn new(tree: &Tree) -> Text {
        let mut paths = tree.files("languages/en/", ".binfab");
        paths.sort_by_cached_key(|p| (stem(p).starts_with("platform_"), p.clone()));
        let mut table = Locale::new();
        for p in paths {
            for (k, v) in locale_at(tree, &p) {
                table.entry(k).or_insert(v);
            }
        }
        Text { table }
    }

    fn of(&self, v: Option<&W>) -> String {
        match v {
            Some(W::Str(s)) if s.starts_with('$') => self.table.get(&**s).cloned().unwrap_or_default(),
            _ => String::new(),
        }
    }

    fn key(&self, k: &str) -> String {
        self.table.get(k).cloned().unwrap_or_default()
    }
}

struct Blueprints(IndexMap<String, String>);

impl Blueprints {
    fn new(tree: &Tree) -> Blueprints {
        Blueprints(
            tree.files("blueprints/", ".blueprint")
                .iter()
                .map(|p| {
                    let b = rp(p, "blueprints/");
                    (b.to_lowercase(), b.to_owned())
                })
                .collect(),
        )
    }

    fn of(&self, raw: Option<&W>) -> String {
        let Some(raw) = raw.and_then(W::as_str).filter(|s| !s.is_empty()) else { return String::new() };
        let name = raw.replace('\\', "/").to_lowercase();
        self.0
            .get(&name)
            .filter(|s| !s.is_empty())
            .or_else(|| self.0.get(&format!("{name}.blueprint")))
            .cloned()
            .unwrap_or_default()
    }
}

fn leaf_of(pf: Option<&Prefab>, cid: i128) -> &Dict {
    pf.and_then(|p| p.component(cid)).map_or(&crate::decode::value::EMPTY, W::leaf)
}

fn difficulty_names(text: &Text) -> Vec<String> {
    let mut names: Vec<String> = ZONE_LEVELS.iter().map(|k| text.key(k)).collect();
    loop {
        let next = text.key(&format!("$ZoneLevel_Uber_{}", names.len() as i64 - 3));
        if next.is_empty() {
            return names;
        }
        names.push(next);
    }
}

fn requirements(pf: &Prefab, text: &Text) -> Vec<Map> {
    let mut out = Vec::new();
    for row in list(leaf_of(Some(pf), REQUIREMENTS).get(0)) {
        let leaf = row.leaf();
        let params = leaf.get(1);
        let own = match params {
            Some(p) if p.is_obj() && p.sections().len() > 1 => p.leaf(),
            _ => &crate::decode::value::EMPTY,
        };
        let k = leaf.str(0).map(str::to_lowercase).unwrap_or_default();
        let mut m = Map::new();
        if k == "zonerankpowerrank" {
            if own.get(1).is_some_and(W::truthy) {
                m.insert("kind".into(), J::from("power_rank_of_world"));
            } else {
                m.insert("kind".into(), J::from("power_rank_of_rank"));
                m.insert("rank".into(), own.get(0).map_or(J::Int(0), J::from));
            }
        } else if k == "tags" {
            m.insert("kind".into(), J::from("tags"));
            m.insert("tags".into(), J::from(strs(own.get(0))));
        } else if k == "stat"
            && let Some(sid) = own.int(0)
        {
            m.insert("kind".into(), J::from("stat"));
            m.insert("stat".into(), J::from(resolve_stat_name_in(&text.table, &stat_key(sid))));
            m.insert("at_least".into(), J::Float(r4(own.get(1).and_then(W::as_num).unwrap_or(0.0))));
        } else if !k.is_empty() {
            m.insert("kind".into(), J::from(k));
        } else {
            continue;
        }
        out.push(m);
    }
    out
}

fn worlds(tree: &Tree, text: &Text, blueprints: &Blueprints, levels: &[String]) -> Vec<J> {
    let mut items: IndexMap<String, (String, Prefab)> = IndexMap::new();
    for path in tree.files(PORTALS, ".binfab") {
        if path.ends_with("_interactive.binfab") {
            continue;
        }
        let Some(pf) = parse_at(tree, &path) else { continue };
        let refs: Vec<String> =
            pf.component(PLACER).map(|p| p.strings().iter().map(|s| s.to_string()).collect()).unwrap_or_default();
        for r in refs {
            if r.ends_with("_interactive") && !items.contains_key(&r.to_lowercase()) {
                items.insert(r.to_lowercase(), (rel_of(&path).to_owned(), pf.clone()));
            }
        }
    }
    let mut out: Vec<(usize, Map)> = Vec::new();
    for path in tree.files(PORTALS, "_interactive.binfab") {
        let Some(pf) = parse_at(tree, &path) else { continue };
        let portal = leaf_of(Some(&pf), PORTAL);
        let Some(kind) = portal.get(0).cloned() else { continue };
        let name = text.of(portal.get(22));
        let Some(ptype) = portal_type(&kind) else { continue };
        if name.is_empty() {
            continue;
        }
        let rel = rel_of(&path);
        let item = items.get(&rel.to_lowercase());
        let mut entry = Map::new();
        entry.insert("slug".into(), J::from(rs(&stem(rel), "_interactive")));
        entry.insert("name".into(), J::from(name.as_str()));
        let desc = text.of(portal.get(23));
        if !desc.is_empty() {
            entry.insert("description".into(), J::from(desc));
        }
        entry.insert("type".into(), J::from(text.key(ptype)));
        let reqs = requirements(&pf, text);
        if let Some(rank) = portal.get(1).map_or(Some(0), W::as_int)
            && (rank > 0 || reqs.iter().any(|r| r.get("rank") == Some(&J::Int(rank))))
        {
            entry.insert("rank".into(), J::Int(rank));
            if let Some(l) = usize::try_from(rank).ok().and_then(|r| levels.get(r)) {
                entry.insert("difficulty".into(), J::from(l.as_str()));
            }
        }
        let scenarios = strs(portal.get(3));
        if !scenarios.is_empty() {
            entry.insert("scenarios".into(), J::from(scenarios));
        }
        if !reqs.is_empty() {
            entry.insert("requires".into(), J::List(reqs.into_iter().map(J::Dict).collect()));
        }
        let item_rel = item.map(|(r, _)| r.as_str()).filter(|r| !r.is_empty()).unwrap_or(rel);
        entry.insert("prefab".into(), J::from(item_rel));
        if let Some((_, ipf)) = item {
            let label = text.of(leaf_of(Some(ipf), IDENTITY).get(1));
            if !label.is_empty() && label != name {
                entry.insert("item_name".into(), J::from(label));
            }
        }
        let mut bp = item.map(|(_, ipf)| blueprints.of(leaf_of(Some(ipf), BLUEPRINT).get(0))).unwrap_or_default();
        if bp.is_empty() {
            bp = blueprints.of(leaf_of(Some(&pf), BLUEPRINT).get(0));
        }
        if !bp.is_empty() {
            entry.insert("blueprint".into(), J::from(bp));
        }
        let order = TYPE_ORDER.iter().position(|t| W::Int(*t).as_int() == kind.as_int()).unwrap_or(0);
        out.push((order, entry));
    }
    out.sort_by_cached_key(|(o, e)| {
        (*o, e.get("rank").and_then(J::as_int).unwrap_or(-1), e["slug"].as_str().unwrap_or("").to_owned())
    });
    out.into_iter().map(|(_, e)| J::Dict(e)).collect()
}

fn objective_rows(v: &W, out: &mut Vec<Dict>) {
    match v {
        W::Obj(sections) => {
            let leaf = v.leaf();
            if leaf.str(0).is_some()
                && leaf.str(1).is_some_and(|s| s.starts_with('$'))
                && leaf.get(3).is_some_and(W::is_obj)
            {
                out.push(Dict::new(leaf.items().to_vec()));
            }
            for sec in sections.iter() {
                if let Some(d) = sec.as_dict() {
                    for x in d.values() {
                        objective_rows(x, out);
                    }
                }
            }
        }
        W::List(_) | W::Rep(_) | W::Tuple(_) | W::Dict(_) => {
            for x in list(Some(v)) {
                objective_rows(&x, out);
            }
        }
        _ => {}
    }
}

fn zone_tags(node: &W, found: &mut Vec<String>) {
    match node {
        W::Obj(sections) => {
            let leaf = node.leaf();
            if leaf.str(0).is_some_and(|k| k.to_lowercase() == "zonetag")
                && let Some(p) = leaf.get(1).filter(|p| p.is_obj() && p.sections().len() > 1)
            {
                for t in strs(p.leaf().get(0)) {
                    if !found.contains(&t) {
                        found.push(t);
                    }
                }
            }
            for sec in sections.iter() {
                if let Some(d) = sec.as_dict() {
                    for x in d.values() {
                        zone_tags(x, found);
                    }
                }
            }
        }
        W::List(_) | W::Rep(_) | W::Tuple(_) | W::Dict(_) => {
            for x in list(Some(node)) {
                zone_tags(&x, found);
            }
        }
        _ => {}
    }
}

fn tagged_adventures(tree: &Tree, text: &Text) -> Vec<J> {
    let mut tags: IndexMap<String, IndexMap<String, J>> = IndexMap::new();
    let mut paths = tree.files("prefabs/meta/activities/", ".binfab");
    paths.extend(tree.files("prefabs/meta/goldenthread/", ".binfab"));
    for path in paths {
        let Some(pf) = parse_at(tree, &path) else { continue };
        let Some(root) = &pf.root else { continue };
        let mut rows = Vec::new();
        objective_rows(root, &mut rows);
        for row in rows {
            let mut found = Vec::new();
            zone_tags(row.get(3).expect("objective"), &mut found);
            let id = row.str(0).unwrap_or("").to_owned();
            for tag in found {
                let name = text.of(row.get(1));
                let adventure = if name.is_empty() {
                    obj! {"id" => id.as_str()}
                } else {
                    obj! {"id" => id.as_str(), "name" => name}
                };
                tags.entry(tag).or_default().entry(id.clone()).or_insert(adventure);
            }
        }
    }
    let mut sorted: Vec<(String, IndexMap<String, J>)> = tags.into_iter().collect();
    sorted.sort_by_cached_key(|(t, _)| t.to_lowercase());
    sorted.into_iter().map(|(t, a)| obj! {"tag" => t, "adventures" => J::List(a.into_values().collect())}).collect()
}

fn py_str(v: Option<&W>) -> String {
    match v {
        None => String::new(),
        Some(W::Str(s)) => s.to_string(),
        Some(W::Int(i)) => i.to_string(),
        Some(W::Float(f)) => crate::pyfloat::repr(*f),
        Some(W::None) => "None".into(),
        Some(other) => format!("{other:?}"),
    }
}

fn biomes(tree: &Tree, text: &Text) -> Vec<J> {
    let pf = parse_at(tree, "prefabs/challenge/challenge.binfab");
    let mut out = Vec::new();
    for row in list(pf.as_ref().and_then(|p| p.root.as_ref()).map(W::leaf).and_then(|l| l.get(0))) {
        let leaf = row.leaf();
        let objective = leaf.get(2).map_or(&crate::decode::value::EMPTY, W::leaf);
        let own = match objective.get(1) {
            Some(p) if p.is_obj() && p.sections().len() > 1 => p.leaf(),
            _ => &crate::decode::value::EMPTY,
        };
        let zones: Vec<String> = strs(own.get(0)).into_iter().filter(|z| z.starts_with("biome/")).collect();
        if objective.str(0) != Some("quest") || zones.is_empty() {
            continue;
        }
        let level = list(leaf.get(1)).into_iter().find(W::is_obj);
        let level = level.as_ref().map_or(&crate::decode::value::EMPTY, W::leaf);
        let mut entry = Map::new();
        entry.insert("slug".into(), J::from(rs(&py_str(Some(leaf.get(0).unwrap_or(&W::Str("".into())))), "_quest")));
        entry.insert("name".into(), J::from(text.of(level.get(0))));
        let desc = text.of(level.get(1));
        if !desc.is_empty() {
            entry.insert("description".into(), J::from(desc));
        }
        entry.insert("challenge".into(), leaf.get(0).map_or(J::Null, J::from));
        entry.insert("zones".into(), J::from(zones.iter().map(|z| rp(z, "biome/").to_owned()).collect::<Vec<_>>()));
        out.push(J::Dict(entry));
    }
    out
}

fn synergies(tree: &Tree, text: &Text) -> Vec<J> {
    let pf = parse_at(tree, "prefabs/meta/tinyquests.binfab");
    let holder = pf.as_ref().and_then(|p| p.root.as_ref()).map(W::leaf).and_then(|l| l.get(11)).cloned();
    let table = holder.filter(|h| h.is_obj() && !h.is_empty()).and_then(|h| h.section(0).get(0).cloned());
    let mut out = Vec::new();
    let Some(W::Dict(t)) = table else { return out };
    let mut rows: Vec<&(K, W)> = t.items().iter().collect();
    rows.sort_by(|a, b| match (&a.0, &b.0) {
        (K::Str(x), K::Str(y)) => x.cmp(y),
        (K::Int(x), K::Int(y)) => x.cmp(y),
        (K::Int(_), K::Str(_)) => std::cmp::Ordering::Less,
        (K::Str(_), K::Int(_)) => std::cmp::Ordering::Greater,
    });
    for (tag, row) in rows {
        let K::Str(tag) = tag else { continue };
        let leaf = row.leaf();
        let name = text.of(leaf.get(0));
        if !tag.starts_with(SYNERGY_PREFIX) || name.is_empty() {
            continue;
        }
        let mut entry = Map::new();
        entry.insert("tag".into(), J::from(tag.to_string()));
        entry.insert("name".into(), J::from(name));
        if let Some(icon) = leaf.str(1).filter(|s| !s.is_empty()) {
            entry.insert("icon".into(), J::from(icon));
        }
        out.push(J::Dict(entry));
    }
    out
}

fn delves(tree: &Tree, text: &Text, blueprints: &Blueprints) -> Vec<J> {
    let mut out: Vec<Map> = Vec::new();
    for path in tree.files(MATCHMAKERS, ".binfab") {
        let Some(pf) = parse_at(tree, &path) else { continue };
        let Some(delve) = pf.component(DELVE) else { continue };
        let ident = leaf_of(Some(&pf), IDENTITY);
        let name = text.of(ident.get(1));
        if name.is_empty() {
            continue;
        }
        let rel = rel_of(&path);
        let sub = rp(rel, rel_of(MATCHMAKERS));
        let mut entry = Map::new();
        entry.insert("slug".into(), J::from(sub.replace('/', "_")));
        entry.insert("name".into(), J::from(name));
        let desc = text.of(ident.get(5));
        if !desc.is_empty() {
            entry.insert("description".into(), J::from(desc));
        }
        entry.insert("group".into(), J::from(sub.rsplit_once('/').map_or("", |(a, _)| a)));
        if let Some(depth) = delve.leaf().int(0).filter(|d| *d >= 0) {
            entry.insert("depth".into(), J::Int(depth));
        }
        for r in list(delve.leaf().get(1)) {
            let row = r.leaf();
            let key = match row.get(1).unwrap_or(&W::Int(0)) {
                W::Int(1) => Some("delve_biomes"),
                W::Int(7) => Some("delve_bosses"),
                _ => None,
            };
            if let (Some(key), Some(id)) = (key, row.str(0).filter(|s| !s.is_empty())) {
                entry
                    .entry(key.to_owned())
                    .or_insert_with(|| J::List(Vec::new()))
                    .as_list_mut()
                    .expect("list")
                    .push(J::from(id));
            }
        }
        entry.insert("prefab".into(), J::from(rel));
        let bp = blueprints.of(leaf_of(Some(&pf), BLUEPRINT).get(0));
        if !bp.is_empty() {
            entry.insert("blueprint".into(), J::from(bp));
        }
        out.push(entry);
    }
    out.sort_by_cached_key(|e| {
        (
            e["group"].as_str().unwrap_or("").to_owned(),
            e.get("depth").and_then(J::as_int).unwrap_or(-1),
            e["name"].as_str().unwrap_or("").to_lowercase(),
            e["slug"].as_str().unwrap_or("").to_owned(),
        )
    });
    out.into_iter().map(J::Dict).collect()
}

fn tower(tree: &Tree, text: &Text, levels: &[String]) -> Vec<J> {
    let pf = parse_at(tree, "prefabs/tower/floor_sets.binfab");
    let sets = pf.as_ref().and_then(|p| p.root.as_ref()).map(W::leaf).and_then(|l| l.get(0)).cloned();
    let mut out = Vec::new();
    let Some(W::Dict(sets)) = sets else { return out };
    let mut floors: Vec<(i128, &W)> = sets
        .items()
        .iter()
        .filter_map(|(k, v)| match k {
            K::Int(i) => Some((*i, v)),
            K::Str(_) => None,
        })
        .collect();
    floors.sort_by_key(|(k, _)| *k);
    for (floor, row) in floors {
        let leaf = row.leaf();
        let boss = list(leaf.get(1)).into_iter().find(W::is_obj);
        let boss = boss.as_ref().map_or(&crate::decode::value::EMPTY, W::leaf);
        let quest = boss.str(0).unwrap_or("").to_owned();
        let bname = text.of(boss.get(1));
        let mut diffs = Vec::new();
        for (i, lv) in list(leaf.get(2)).into_iter().filter(W::is_obj).enumerate() {
            let d = lv.leaf();
            let mut r = Map::new();
            let label = if i < 3 { title(&text.key(TOWER_DIFFICULTIES[i])) } else { i.to_string() };
            r.insert("difficulty".into(), J::from(label));
            if let Some(f) = d.int(0) {
                r.insert("flux".into(), J::Int(f));
            }
            if let Some(rank) = d.int(4) {
                r.insert("loot_rank".into(), J::Int(rank));
                if let Some(l) = usize::try_from(rank).ok().and_then(|x| levels.get(x)) {
                    r.insert("loot_difficulty".into(), J::from(l.as_str()));
                }
            }
            for (key, field) in [("leaderboard_icon", 5), ("portrait", 6)] {
                if let Some(s) = d.str(field).filter(|s| !s.is_empty()) {
                    r.insert(key.into(), J::from(s));
                }
            }
            diffs.push(J::Dict(r));
        }
        if !quest.is_empty() {
            out.push(obj! {
                "slug" => quest.as_str(), "name" => if bname.is_empty() { quest.clone() } else { bname },
                "floor" => floor + 1, "scenarios" => strs(leaf.get(0)), "difficulties" => diffs,
            });
        }
    }
    out
}

pub fn build(ctx: &Ctx) -> Built {
    let tree = &ctx.tree;
    let text = Text::new(tree);
    let blueprints = Blueprints::new(tree);
    let levels = difficulty_names(&text);
    Ok(obj! {
        "difficulties" => J::List(levels.iter().enumerate().map(|(i, n)| obj! {"rank" => i, "name" => n.as_str()}).collect()),
        "worlds" => worlds(tree, &text, &blueprints, &levels),
        "biomes" => biomes(tree, &text),
        "zone_tags" => tagged_adventures(tree, &text),
        "tinyquest_biomes" => synergies(tree, &text),
        "delve_gateways" => delves(tree, &text, &blueprints),
        "shadow_tower" => tower(tree, &text, &levels),
    })
}

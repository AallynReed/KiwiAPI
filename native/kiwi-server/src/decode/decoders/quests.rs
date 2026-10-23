//! `quests.py`: adventures, quest lines, Tiny Quests and the Golden Thread.

use std::sync::OnceLock;

use indexmap::{IndexMap, IndexSet};
use regex::Regex;

use super::{
    badges::{EXE_PATH, Text, fmt_commas, grants, parse_metric_names},
    recipes::requirement,
};
use crate::decode::{
    ability::{Prefabs, identity},
    common::stat_key,
    json::{J, Map},
    registry::{Built, Ctx},
    tree::Tree,
    value::{Dict, K, W, parse, py_eq},
};
use crate::obj;

const SETS_DIR: &str = "prefabs/meta/activities/";
const GOLDEN: &str = "meta/goldenthread/goldenthread";
const META: &str = "meta/activity";

fn activity_type(t: i128) -> J {
    match t {
        0 => J::from("Event"),
        1 => J::from("Expertise"),
        2 => J::from("ClubMemberAdventure"),
        3 => J::from("ClubNonMemberAdventure"),
        4 => J::from("WorldObjective"),
        5 => J::from("NPCAdventure"),
        6 => J::from("GeodeNPCAdventure"),
        7 => J::from("CrystallogyAdventure"),
        8 => J::from("RepeatableEvent"),
        10 => J::from("AutoUseTome"),
        11 => J::from("TinyQuests"),
        _ => J::Int(t),
    }
}

fn objectives(kind: &str) -> &'static [(i128, &'static str)] {
    match kind {
        "adventurecompleted" => &[(0, "adventures"), (1, "goal"), (3, "activity_types")],
        "itemacquired" => &[(0, "items"), (1, "goal"), (4, "take_items"), (5, "requirement"), (6, "item_tags")],
        "itemconsumed" => &[(0, "items"), (1, "goal"), (4, "requirement")],
        "itemfished" => &[(0, "items"), (1, "goal"), (3, "tags"), (4, "requirement")],
        "interacted" => {
            &[(0, "targets"), (2, "requirement"), (3, "goal"), (5, "tags"), (6, "removes"), (7, "remove_one")]
        }
        "npckilled" => &[(0, "npcs"), (1, "goal"), (3, "npc_tags"), (4, "require_both"), (5, "requirement")],
        "craftingcomplete" => &[(0, "stations"), (1, "recipes"), (3, "goal"), (4, "removes"), (5, "remove_one")],
        "quest" => &[(0, "quests"), (1, "requirement"), (2, "goal")],
        "trigger" => &[(0, "triggers"), (2, "goal"), (3, "requirement")],
        "metric" => &[(0, "metric"), (1, "goal")],
        "blockplaced" => &[(2, "goal")],
        "collectionunlock" => &[(4, "goal")],
        "itemlooted" | "itemupgrade" | "upgradematerial" | "tomelevel" => &[(1, "goal")],
        "jump" | "walk" | "tinyquestinstacompleted" => &[(0, "goal")],
        "classlevel" | "mastery" => &[(0, "level")],
        "professionrank" => &[(0, "rank"), (1, "professions")],
        "stattotal" => &[(0, "amount"), (1, "stats")],
        "requestworld" => &[(0, "world_type"), (2, "world")],
        "activeui" => &[(0, "ui")],
        _ => &[],
    }
}

fn rows(v: Option<&W>) -> Vec<Dict> {
    v.map(W::iter)
        .unwrap_or_default()
        .iter()
        .filter(|r| r.is_obj())
        .map(|r| Dict::new(r.leaf().items().to_vec()))
        .collect()
}

fn strs(v: Option<&W>) -> Vec<String> {
    v.filter(|x| x.truthy())
        .map(W::iter)
        .unwrap_or_default()
        .iter()
        .filter_map(|s| s.as_str().filter(|s| !s.is_empty()).map(str::to_owned))
        .collect()
}

fn humanize(stem: &str) -> String {
    stem.replace('_', " ")
        .split_whitespace()
        .map(|w| {
            if matches!(w, "npc" | "pvp" | "cm") {
                w.to_uppercase()
            } else {
                let mut c = w.chars();
                match c.next() {
                    Some(f) => f.to_uppercase().chain(c.flat_map(char::to_lowercase)).collect(),
                    None => String::new(),
                }
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

struct Game {
    prefabs: Prefabs,
    text: Text,
    metrics: Vec<String>,
    tree: Tree,
    claims: IndexMap<String, W>,
    names: IndexMap<String, String>,
}

impl Game {
    fn load_claims(&mut self, wanted: &IndexSet<String>) {
        let mut clash: IndexSet<String> = IndexSet::new();
        for path in self.tree.files("prefabs/claim/", ".binfab") {
            let data = self.tree.read(&path).unwrap_or_default();
            if !wanted.iter().any(|w| memchr::memmem::find(&data, w.as_bytes()).is_some()) {
                continue;
            }
            let Ok(pf) = parse(&data) else { continue };
            let Some(W::Dict(table)) = pf.root.as_ref().and_then(|r| r.leaf().get(0).cloned()) else { continue };
            for (cid, rec) in table.items() {
                let K::Str(cid) = cid else { continue };
                let cid = cid.to_string();
                if let Some(prev) = self.claims.get(&cid)
                    && !py_eq(prev, rec)
                {
                    clash.insert(cid.clone());
                }
                self.claims.entry(cid).or_insert_with(|| rec.clone());
            }
        }
        for cid in clash {
            self.claims.shift_remove(&cid);
        }
    }

    fn say(&self, key: Option<&W>) -> String {
        match key {
            Some(W::Str(s)) if s.starts_with('@') => s[1..].to_owned(),
            other => self.text.get(other),
        }
    }

    fn say_str(&self, key: &str) -> String {
        self.say(Some(&W::Str(key.into())))
    }

    fn prefab_name(&self, r: &str) -> String {
        let ident = identity(self.prefabs.get(r).as_deref());
        match ident.name_key {
            Some(k) => self.say_str(&k),
            None => String::new(),
        }
    }
}

fn requirement_text(req: &mut Map, g: &Game) {
    if let Some(J::Str(m)) = req.get("message").cloned() {
        req.insert("message".into(), J::from(g.say_str(&m)));
    }
    if let Some(J::List(pair)) = req.get("text").cloned() {
        let fmt = pair.first().and_then(J::as_str).unwrap_or("");
        let n = pair.get(1).map(fmt_commas).unwrap_or_default();
        req.insert("text".into(), J::from(g.say_str(fmt).replace("{0}", &n)));
    }
    if let Some(J::List(of)) = req.get_mut("of") {
        for c in of.iter_mut() {
            if let Some(m) = c.as_dict_mut() {
                requirement_text(m, g);
            }
        }
    }
    if let Some(J::Dict(c)) = req.get_mut("condition") {
        requirement_text(c, g);
    }
}

fn empty_or_none(v: &J) -> bool {
    matches!(v, J::Null) || *v == J::from("")
}

fn value(label: &str, v: Option<&W>, g: &Game) -> Option<J> {
    match label {
        "requirement" => {
            let mut req = requirement(v, &mut IndexSet::new())?;
            requirement_text(&mut req, g);
            req.retain(|_, x| !empty_or_none(x));
            Some(J::Dict(req))
        }
        "items" | "targets" | "npcs" | "stations" | "removes" => {
            let out: Vec<J> = strs(v)
                .into_iter()
                .map(|p| {
                    let name = g.prefab_name(&p);
                    if name.is_empty() {
                        obj! {"prefab" => p}
                    } else {
                        obj! {"prefab" => p, "name" => name}
                    }
                })
                .collect();
            (!out.is_empty()).then_some(J::List(out))
        }
        "adventures" => {
            let out: Vec<J> = strs(v)
                .into_iter()
                .map(|a| match g.names.get(&a).filter(|n| !n.is_empty()) {
                    Some(n) => obj! {"id" => a.as_str(), "name" => n.as_str()},
                    None => obj! {"id" => a},
                })
                .collect();
            (!out.is_empty()).then_some(J::List(out))
        }
        "activity_types" => {
            let types: Vec<J> = v
                .filter(|x| x.truthy())
                .map(W::iter)
                .unwrap_or_default()
                .iter()
                .filter_map(W::as_int)
                .map(activity_type)
                .collect();
            (!types.is_empty()).then_some(J::List(types))
        }
        "metric" => {
            let id = v.map_or(J::Null, J::from);
            let name = match v {
                Some(W::Int(i)) if *i > 0 && (*i as usize) < g.metrics.len() => g.metrics[*i as usize].clone(),
                _ => String::new(),
            };
            Some(if name.is_empty() {
                obj! {"id" => id}
            } else {
                let label = g.text.of(&format!("$Metrics_{name}"));
                obj! {"id" => id, "name" => name, "label" => label}
            })
        }
        "stats" => {
            let mut out = Vec::new();
            for s in v.filter(|x| x.truthy()).map(W::iter).unwrap_or_default() {
                if let W::Int(s) = s {
                    let name = g.text.of(&stat_key(s));
                    out.push(if name.is_empty() {
                        obj! {"id" => s}
                    } else {
                        obj! {"id" => s, "name" => name}
                    });
                }
            }
            (!out.is_empty()).then_some(J::List(out))
        }
        "take_items" | "remove_one" | "require_both" => v.is_some_and(W::truthy).then_some(J::Bool(true)),
        _ => match v {
            Some(x) if x.is_list() => {
                let s = strs(Some(x));
                (!s.is_empty()).then(|| J::from(s))
            }
            Some(x) => {
                let j = J::from(x);
                (!empty_or_none(&j)).then_some(j)
            }
            None => None,
        },
    }
}

fn objective(node: Option<&W>, g: &Game) -> Map {
    let mut out = Map::new();
    let Some(node) = node else { return out };
    let leaf = node.leaf();
    let Some(kind) = leaf.str(0).filter(|k| !k.is_empty()) else { return out };
    let own = match leaf.get(1) {
        Some(p) if p.is_obj() && !p.is_empty() => {
            p.sections().last().and_then(W::as_dict).unwrap_or(&crate::decode::value::EMPTY)
        }
        _ => &crate::decode::value::EMPTY,
    };
    let key = kind.to_lowercase();
    out.insert("kind".into(), J::from(key.as_str()));
    if key == "objectivesetall" || key == "objectivesetany" {
        out.insert("match".into(), J::from(if key == "objectivesetall" { "all" } else { "any" }));
        let subs: Vec<W> = match own.get(0).filter(|v| v.truthy()) {
            Some(W::Dict(d)) => d.values().cloned().collect(),
            _ => Vec::new(),
        };
        let objs: Vec<J> = subs.iter().map(|s| objective(Some(s), g)).filter(|o| !o.is_empty()).map(J::Dict).collect();
        out.insert("objectives".into(), J::List(objs));
        return out;
    }
    for (idx, label) in objectives(&key) {
        if let Some(v) = value(label, own.get(*idx), g) {
            out.insert((*label).into(), v);
        }
    }
    out
}

fn entry(adv: &Dict, g: &Game) -> Map {
    let mut row = Map::new();
    row.insert("slug".into(), adv.get(0).map_or(J::from(""), J::from));
    row.insert("name".into(), J::from(g.say(adv.get(1))));
    let desc = g.say(adv.get(2)).trim().to_owned();
    let summary = g.say(adv.get(15)).trim().to_owned();
    row.insert("description".into(), J::from(if desc.is_empty() { summary.clone() } else { desc.clone() }));
    if !desc.is_empty() && !summary.is_empty() && summary != desc {
        row.insert("summary".into(), J::from(summary));
    }
    let obj = objective(adv.get(3), g);
    if !obj.is_empty() {
        row.insert("objective".into(), J::Dict(obj));
    }
    if let Some(cid) = adv.str(4).filter(|c| !c.is_empty())
        && let Some(claim) = g.claims.get(cid)
    {
        row.insert("claim".into(), J::from(cid));
        let gr = grants(claim.leaf().get(1), &g.prefabs, &g.text);
        if !gr.is_empty() {
            row.insert("rewards".into(), J::List(gr));
        }
    }
    match adv.get(12) {
        Some(W::Int(l)) if *l > 0 => {
            row.insert("time_limit".into(), J::Int(*l));
        }
        Some(W::Float(l)) if *l > 0.0 => {
            row.insert(
                "time_limit".into(),
                if l.fract() == 0.0 && l.is_finite() { J::Int(*l as i128) } else { J::Float(*l) },
            );
        }
        _ => {}
    }
    if adv.get(14).is_some_and(W::truthy) {
        row.insert("daily_reset".into(), J::Bool(true));
    }
    if let Some(icon) = adv.str(13).filter(|s| !s.is_empty()) {
        row.insert("icon".into(), J::from(icon));
        match adv.get(20) {
            Some(W::Int(1)) => {
                row.insert("icon_kind".into(), J::from("Blueprint"));
            }
            Some(W::Int(2)) => {
                row.insert("icon_kind".into(), J::from("Prefab"));
            }
            _ => {}
        }
    }
    row
}

fn load(tree: &Tree, r: &str) -> Option<W> {
    parse(&tree.read(&format!("prefabs/{r}.binfab")).unwrap_or_default()).ok().and_then(|pf| pf.root)
}

fn leaf_of(v: Option<&W>) -> Dict {
    v.map(|x| Dict::new(x.leaf().items().to_vec())).unwrap_or_default()
}

fn meta(tree: &Tree) -> (Vec<J>, IndexMap<String, Vec<String>>) {
    let root = leaf_of(load(tree, META).as_ref());
    let mut limits: IndexMap<i128, Map> = IndexMap::new();
    for (field, label) in [(3, "active"), (5, "daily_rewards")] {
        for r in rows(root.get(field)) {
            if let (Some(t), Some(n)) = (r.int(1), r.int(0)) {
                limits.entry(t).or_default().insert(label.into(), J::Int(n));
            }
        }
    }
    let mut sorted: Vec<(i128, Map)> = limits.into_iter().collect();
    sorted.sort_by_key(|(t, _)| *t);
    let out: Vec<J> = sorted
        .into_iter()
        .map(|(t, v)| {
            let mut m = Map::new();
            m.insert("type".into(), activity_type(t));
            m.extend(v);
            J::Dict(m)
        })
        .collect();
    let mut threads: IndexMap<String, Vec<String>> = IndexMap::new();
    if let Some(W::Dict(d)) = root.get(4).filter(|v| v.truthy()) {
        for (k, v) in d.items() {
            if let K::Str(k) = k
                && !k.is_empty()
            {
                threads.insert(k.to_string(), strs(v.leaf().get(1)));
            }
        }
    }
    (out, threads)
}

fn ordered(mut rws: Vec<Map>, order: &[String]) -> Vec<Map> {
    let mut pos: IndexMap<&str, usize> = IndexMap::new();
    for (i, a) in order.iter().enumerate() {
        pos.insert(a.as_str(), i);
    }
    for r in rws.iter_mut() {
        if let Some(p) = r.get("slug").and_then(J::as_str).and_then(|s| pos.get(s)).copied() {
            r.insert("step".into(), J::from(p + 1));
        }
    }
    rws.sort_by_key(|r| r.get("step").and_then(J::as_int).unwrap_or(order.len() as i128 + 1));
    rws
}

fn chain_title(rws: &[Map]) -> String {
    static R: OnceLock<Regex> = OnceLock::new();
    let re =
        R.get_or_init(|| Regex::new(r"^(?:(.{3,60}?) \d+\s*/\s*\d+\s*[:.]|\d+\s*/\s*\d+ (.{3,60}?):)").expect("regex"));
    let mut titles: IndexMap<String, usize> = IndexMap::new();
    for r in rws {
        if let Some(m) = re.captures(r.get("description").and_then(J::as_str).unwrap_or("")) {
            let t = m
                .get(1)
                .filter(|x| !x.as_str().is_empty())
                .or_else(|| m.get(2))
                .map_or("", |x| x.as_str())
                .trim()
                .to_owned();
            *titles.entry(t).or_insert(0) += 1;
        }
    }
    let mut best: Option<(&String, usize)> = None;
    for (t, n) in &titles {
        if best.is_none_or(|(_, b)| *n > b) {
            best = Some((t, *n));
        }
    }
    best.map(|(t, _)| t.clone()).unwrap_or_default()
}

fn slug(text: &str) -> String {
    static R: OnceLock<Regex> = OnceLock::new();
    let re = R.get_or_init(|| Regex::new(r"[^a-z0-9]+").expect("regex"));
    re.replace_all(&text.to_lowercase(), "-").trim_matches('-').to_owned()
}

pub fn build(ctx: &Ctx) -> Built {
    let tree = &ctx.tree;
    let mut g = Game {
        prefabs: Prefabs::identities(tree),
        text: Text::new(tree),
        metrics: tree.read(EXE_PATH).map(|e| parse_metric_names(&e)).unwrap_or_default(),
        tree: tree.clone(),
        claims: IndexMap::new(),
        names: IndexMap::new(),
    };
    let (limits, threads) = meta(tree);
    let mut sources: Vec<(String, Option<W>, Vec<Dict>)> = Vec::new();
    for path in tree.files(SETS_DIR, ".binfab") {
        let stem = path[SETS_DIR.len()..path.len() - ".binfab".len()].to_owned();
        let root = leaf_of(load(tree, &path["prefabs/".len()..path.len() - ".binfab".len()]).as_ref());
        let advs = rows(root.get(1));
        if !advs.is_empty() {
            sources.push((stem, root.get(2).cloned(), advs));
        }
    }
    let golden = rows(leaf_of(load(tree, GOLDEN).as_ref()).get(0));
    let mut wanted: IndexSet<String> = IndexSet::new();
    for advs in sources.iter().map(|(_, _, a)| a).chain(std::iter::once(&golden)) {
        for a in advs {
            if let Some(id) = a.str(0)
                && !g.names.contains_key(id)
            {
                let n = g.say(a.get(1));
                g.names.insert(id.to_owned(), n);
            }
            if let Some(c) = a.str(4).filter(|c| !c.is_empty()) {
                wanted.insert(c.to_owned());
            }
        }
    }
    g.load_claims(&wanted);

    let mut groups: Vec<Map> = Vec::new();
    for (stem, typ, advs) in &sources {
        let mut rws: Vec<Map> = advs.iter().map(|a| entry(a, &g)).collect();
        let mut group = Map::new();
        group.insert("slug".into(), J::from(stem.replace('_', "-")));
        group.insert("name".into(), J::from(humanize(stem)));
        group.insert("source".into(), J::from(stem.as_str()));
        group.insert(
            "type".into(),
            match typ {
                Some(W::Int(t)) => activity_type(*t),
                _ => J::Null,
            },
        );
        if let Some(order) = threads.get(stem) {
            group.insert("thread".into(), J::from(stem.as_str()));
            rws = ordered(rws, order);
        }
        group.insert("entries".into(), J::List(rws.into_iter().map(J::Dict).collect()));
        groups.push(group);
    }
    let mut threaded: IndexMap<String, String> = IndexMap::new();
    for (tid, ids) in &threads {
        for a in ids {
            threaded.insert(a.clone(), tid.clone());
        }
    }
    static ID_STEM: OnceLock<Regex> = OnceLock::new();
    let id_stem = ID_STEM.get_or_init(|| Regex::new(r"_\d+(?:_\w+)?$").expect("regex"));
    let mut stems: IndexMap<String, Vec<Map>> = IndexMap::new();
    for a in &golden {
        let aid = a.str(0).unwrap_or("").to_owned();
        let key = threaded
            .get(&aid)
            .filter(|t| !t.is_empty())
            .cloned()
            .unwrap_or_else(|| id_stem.replace(&aid, "").into_owned());
        stems.entry(key).or_default().push(entry(a, &g));
    }
    let mut chains: IndexMap<String, (String, Vec<Map>)> = IndexMap::new();
    for (stem, rws) in stems {
        let title = chain_title(&rws);
        if rws.len() == 1 && !threads.contains_key(&stem) {
            let (key, name) = if title.is_empty() {
                ("golden-thread-other".to_owned(), "Other Golden Thread Quests".to_owned())
            } else {
                (slug(&title), title)
            };
            chains.entry(key).or_insert_with(|| (name, Vec::new())).1.extend(rws);
        } else {
            let first = rws.first().and_then(|r| r.get("name")).and_then(J::as_str).unwrap_or("").to_owned();
            let name = if stem == "golden_thread" {
                "Golden Thread".to_owned()
            } else if !title.is_empty() {
                title
            } else if !first.is_empty() {
                first
            } else {
                humanize(&stem)
            };
            chains.insert(stem.replace('_', "-"), (name, rws));
        }
    }
    for (key, (name, mut rws)) in chains {
        let stem = key.replace('-', "_");
        let mut group = Map::new();
        group.insert("slug".into(), J::from(key.as_str()));
        group.insert("name".into(), J::from(name));
        group.insert("source".into(), J::from("goldenthread"));
        group.insert("type".into(), J::Null);
        if let Some(order) = threads.get(&stem) {
            group.insert("thread".into(), J::from(stem.as_str()));
            rws = ordered(rws, order);
        }
        group.insert("entries".into(), J::List(rws.into_iter().map(J::Dict).collect()));
        groups.push(group);
    }
    let groups: Vec<J> = groups
        .into_iter()
        .map(|mut grp| {
            grp.retain(|_, v| !matches!(v, J::Null));
            J::Dict(grp)
        })
        .collect();
    Ok(obj! {"limits" => limits, "groups" => groups})
}

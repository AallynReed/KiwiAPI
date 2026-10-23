//! `npcs.py`: enemies, bosses and friendly NPCs, and what each one does.

use indexmap::{IndexMap, IndexSet};

use super::ally_abilities::detail;
use crate::decode::{
    ability::{Prefabs, WALK_LIMIT, identity, refs, text_keys, walk},
    common::{Locale, locale_at, r4, resolve_stat_name, rp, rs, stat_key},
    json::{J, Map},
    registry::{Built, Ctx},
    tree::Tree,
    value::{Dict, Prefab, W, parse},
};
use crate::obj;

const ROOT: &str = "npc/";
const NPC_TYPE: i128 = 44;
const SIDE: i128 = 6;
const STATS: i128 = 7;
const NPC: i128 = 43;
const PLACER: i128 = 50;
const SIGN: i128 = 103;
const TAGS: i128 = 106;
const AI: i128 = 111;
const ANIM_EVENTS: i128 = 145;
const CARRIED: i128 = 160;
const CRAFTING: i128 = 61;
const STATION: i128 = 76;
const SHOP: i128 = 268;
const ADVENTURES: i128 = 411;
const NAME: i128 = 4;
const ABILITY_LIST: i128 = 19;
const DESCRIPTION: i128 = 24;
const ICON: i128 = 25;
const BOSS_TAGS: [(&str, &str); 3] = [("delveBoss", "delve"), ("worldboss", "world"), ("shadowtitan", "shadow_tower")];
const NEUTRAL_MULTIPLIERS: [usize; 6] = [11, 12, 24, 27, 45, 46];

fn list(v: Option<&W>) -> Vec<W> {
    match v {
        Some(W::Dict(d)) => d.values().cloned().collect(),
        Some(x) => x.elems().to_vec(),
        None => Vec::new(),
    }
}

fn rel_of(r: &str) -> String {
    rs(rp(r, "prefabs/"), ".binfab").to_owned()
}

fn leaf_of(pf: &Prefab, cid: i128) -> &Dict {
    pf.component(cid).map_or(&crate::decode::value::EMPTY, W::leaf)
}

struct Text {
    table: Locale,
}

impl Text {
    fn new(tree: &Tree) -> Text {
        let mut table = Locale::new();
        for path in tree.files("languages/en/", ".binfab") {
            for (k, v) in locale_at(tree, &path) {
                table.entry(k).or_insert(v);
            }
        }
        Text { table }
    }

    fn of(&self, v: Option<&W>) -> String {
        match v {
            Some(W::Str(s)) => self.s(s),
            _ => String::new(),
        }
    }

    fn s(&self, v: &str) -> String {
        if v.is_empty() {
            return String::new();
        }
        if v.starts_with('$') {
            return self.table.get(v).cloned().unwrap_or_default();
        }
        rp(v, "@").to_owned()
    }
}

fn first(v: Option<&W>) -> String {
    list(v).iter().find_map(|x| x.as_str().filter(|s| !s.is_empty()).map(str::to_owned)).unwrap_or_default()
}

fn adventures(tree: &Tree, text: &Text) -> (IndexMap<String, J>, IndexMap<String, Vec<String>>) {
    let mut info: IndexMap<String, J> = IndexMap::new();
    let mut counts: IndexMap<String, Vec<String>> = IndexMap::new();
    fn visit(v: &W, text: &Text, info: &mut IndexMap<String, J>, counts: &mut IndexMap<String, Vec<String>>) {
        match v {
            W::Obj(sections) => {
                let leaf = v.leaf();
                if let (Some(aid), Some(key)) = (leaf.str(0), leaf.str(1))
                    && key.starts_with('$')
                    && let Some(objective) = leaf.get(3).filter(|o| o.is_obj())
                {
                    let mut row = Map::new();
                    row.insert("id".into(), J::from(aid));
                    row.insert("name".into(), J::from(text.s(key)));
                    let desc = text.of(leaf.get(2));
                    if !desc.is_empty() {
                        row.insert("description".into(), J::from(desc));
                    }
                    info.entry(aid.to_owned()).or_insert(J::Dict(row));
                    if !objective.is_empty() && objective.section(0).str(0) == Some("npckilled") {
                        let body = objective.section(0).get(1);
                        let own = match body {
                            Some(b) if b.is_obj() && b.sections().len() > 1 => b.leaf(),
                            _ => &crate::decode::value::EMPTY,
                        };
                        for r in list(own.get(0)) {
                            if let Some(r) = r.as_str().filter(|s| !s.is_empty()) {
                                counts.entry(rel_of(r).to_lowercase()).or_default().push(aid.to_owned());
                            }
                        }
                    }
                }
                for sec in sections.iter() {
                    if let Some(d) = sec.as_dict() {
                        for x in d.values() {
                            visit(x, text, info, counts);
                        }
                    }
                }
            }
            W::List(_) | W::Rep(_) | W::Tuple(_) | W::Dict(_) => {
                for x in list(Some(v)) {
                    visit(&x, text, info, counts);
                }
            }
            _ => {}
        }
    }
    for path in tree.files("prefabs/meta/activities/", ".binfab") {
        let Ok(pf) = parse(&tree.read(&path).unwrap_or_default()) else { continue };
        if let Some(root) = &pf.root {
            visit(root, text, &mut info, &mut counts);
        }
        for (_, obj) in &pf.components {
            visit(obj, text, &mut info, &mut counts);
        }
    }
    (info, counts)
}

fn placers(tree: &Tree, text: &Text) -> IndexMap<String, Vec<J>> {
    let mut out: IndexMap<String, Vec<J>> = IndexMap::new();
    for path in tree.files("prefabs/placeable/npc/", ".binfab") {
        let Ok(pf) = parse(&tree.read(&path).unwrap_or_default()) else { continue };
        let Some(placer) = pf.component(PLACER) else { continue };
        let r = rel_of(&path);
        for s in placer.strings() {
            if rel_of(&s).starts_with(ROOT) {
                let row = obj! {"prefab" => r.as_str(), "name" => text.s(identity(Some(&pf)).name())};
                out.entry(rel_of(&s).to_lowercase()).or_default().push(row);
            }
        }
    }
    out
}

fn stats(pf: &Prefab) -> Vec<J> {
    let mut rows = Vec::new();
    for (ordinal, value) in list(leaf_of(pf, STATS).get(0)).iter().enumerate() {
        let v = match value {
            W::Int(i) => *i as f64,
            W::Float(f) => *f,
            _ => continue,
        };
        if v == 0.0 || (NEUTRAL_MULTIPLIERS.contains(&ordinal) && v == 1.0) {
            continue;
        }
        let key = stat_key(ordinal as i128);
        rows.push(obj! {"stat" => key.as_str(), "name" => resolve_stat_name(&key), "value" => r4(v)});
    }
    rows
}

fn model(pf: &Prefab) -> Map {
    let comp = leaf_of(pf, 40);
    let skeleton = comp.str(0).unwrap_or("");
    let mut parts: Vec<String> = Vec::new();
    for r in list(comp.get(1)) {
        if let Some(p) = r.leaf().str(0).filter(|s| !s.is_empty())
            && !parts.iter().any(|x| x == p)
        {
            parts.push(p.to_owned());
        }
    }
    let mut out = Map::new();
    if !skeleton.is_empty() {
        out.insert("skeleton".into(), J::from(rs(skeleton, ".skeleton.gr2")));
    }
    if !parts.is_empty() {
        out.insert("blueprints".into(), J::from(parts));
    }
    out
}

fn first_obj(v: Option<&W>) -> Option<W> {
    match v {
        Some(x) if x.is_obj() => Some(x.clone()),
        other => list(other).into_iter().find(W::is_obj),
    }
}

fn ability_rows(pf: &Prefab) -> Vec<J> {
    let mut rows: Vec<J> = Vec::new();
    for row in list(leaf_of(pf, NPC).get(ABILITY_LIST)) {
        let leaf = row.leaf();
        let need = first_obj(leaf.get(4)).and_then(|o| o.leaf().get(0).cloned());
        for r in list(leaf.get(0)) {
            if let Some(r) = r.as_str().filter(|s| !s.is_empty()) {
                let mut entry = Map::new();
                entry.insert("ref".into(), J::from(rel_of(r)));
                if let Some(W::Str(n)) = &need
                    && !matches!(&**n, "none" | "null" | "")
                {
                    entry.insert("requires".into(), J::from(n.to_string()));
                }
                rows.push(J::Dict(entry));
            }
        }
    }
    for row in list(leaf_of(pf, ANIM_EVENTS).get(0)) {
        let leaf = row.leaf();
        if let Some(r) = leaf.str(1).filter(|s| !s.is_empty()) {
            rows.push(obj! {"ref" => rel_of(r), "event" => leaf.str(0).unwrap_or("")});
        }
    }
    let mut out: Vec<J> = Vec::new();
    for r in rows {
        if !out.contains(&r) {
            out.push(r);
        }
    }
    out
}

fn ability(prefabs: &Prefabs, r: &str, text: &Text) -> Map {
    let mut out = Map::new();
    let Some(pf) = prefabs.get(r) else { return out };
    let keys = text_keys(Some(&pf));
    let title = keys.name_key.as_deref().map(|k| text.s(k)).unwrap_or_default();
    if !title.is_empty() {
        out.insert("name".into(), J::from(title));
    }
    let desc = keys.description_key.as_deref().map(|k| text.s(k)).unwrap_or_default();
    if !desc.is_empty() {
        out.insert("text".into(), J::from(desc));
    }
    out.extend(detail(prefabs, r, &text.table));
    let mut summons: Vec<String> = Vec::new();
    for (_, node, _) in walk(prefabs, r, &[], WALK_LIMIT, crate::decode::ability::ABILITIES) {
        let mut found = refs(&node, &[ROOT]);
        found.extend(refs(&node, &["prefabs/npc/"]));
        for s in found {
            let s = rel_of(&s);
            if !summons.contains(&s) {
                summons.push(s);
            }
        }
    }
    if !summons.is_empty() {
        out.insert("summons".into(), J::from(summons));
    }
    out
}

fn services(pf: &Prefab, text: &Text) -> Map {
    let mut out = Map::new();
    if let Some(craft) = pf.component(CRAFTING) {
        let mut tabs = Vec::new();
        for tab in list(craft.get(14)) {
            let leaf = tab.leaf();
            let recipes: Vec<J> =
                list(leaf.get(1)).iter().filter_map(|r| r.as_str().filter(|s| !s.is_empty()).map(J::from)).collect();
            if !recipes.is_empty() {
                tabs.push(obj! {"name" => text.of(leaf.get(0)), "recipes" => recipes});
            }
        }
        let title = match pf.component(STATION) {
            Some(st) if st.sections().len() > 1 => text.of(st.section(1).get(0)),
            _ => String::new(),
        };
        if !tabs.is_empty() {
            out.insert(
                "crafting".into(),
                if title.is_empty() {
                    obj! {"tabs" => tabs}
                } else {
                    obj! {"title" => title, "tabs" => tabs}
                },
            );
        }
    }
    if let Some(shop) = pf.component(SHOP).filter(|s| s.sections().len() > 1) {
        let mut stores: Vec<String> = Vec::new();
        for f in [1, 2] {
            for s in list(shop.section(1).get(f)) {
                if let Some(s) = s.as_str().filter(|s| !s.is_empty())
                    && !stores.iter().any(|x| x == s)
                {
                    stores.push(s.to_owned());
                }
            }
        }
        if !stores.is_empty() {
            out.insert("stores".into(), J::from(stores));
        }
    }
    out
}

fn offered(pf: &Prefab) -> Vec<String> {
    let rows = match pf.component(ADVENTURES) {
        Some(c) if c.sections().len() > 1 => list(c.section(1).get(1)),
        _ => Vec::new(),
    };
    let mut out: Vec<String> = Vec::new();
    for r in rows {
        if let Some(a) = r.leaf().str(0).filter(|s| !s.is_empty())
            && !out.iter().any(|x| x == a)
        {
            out.push(a.to_owned());
        }
    }
    out
}

fn entry(r: &str, pf: &Prefab, text: &Text) -> Option<Map> {
    let npc = leaf_of(pf, NPC);
    let sign = leaf_of(pf, SIGN);
    let plate = npc.str(NAME).unwrap_or("");
    let (title, sign_text) = (text.s(&first(sign.get(0))), text.s(&first(sign.get(1))));
    let mut name = text.s(plate);
    if !title.is_empty() && (plate.is_empty() || plate.starts_with('@')) {
        name = title.clone();
    }
    if name.is_empty() {
        return None;
    }
    let tags: IndexSet<String> =
        list(leaf_of(pf, TAGS).get(0)).iter().filter_map(|t| t.as_str().map(str::to_owned)).collect();
    let sub = &r[ROOT.len()..];
    let folder = sub.rsplit_once('/').map_or("", |(a, _)| a);
    let mut e = Map::new();
    e.insert("slug".into(), J::from(sub.replace('/', "_")));
    e.insert("name".into(), J::from(name));
    let desc = text.of(npc.get(DESCRIPTION));
    if !desc.is_empty() {
        e.insert("description".into(), J::from(desc));
    }
    e.insert("prefab".into(), J::from(r));
    e.insert("group".into(), J::from(folder.split('/').take(2).collect::<Vec<_>>().join("/")));
    if let Some((_, kind)) = BOSS_TAGS.iter().find(|(t, _)| tags.contains(*t)) {
        e.insert("boss".into(), J::from(*kind));
    }
    if tags.contains("elite") {
        e.insert("elite".into(), J::Bool(true));
    }
    match leaf_of(pf, SIDE).get(1) {
        Some(W::Int(1)) => {
            e.insert("side".into(), J::from("players"));
        }
        Some(W::Int(2)) => {
            e.insert("side".into(), J::from("enemies"));
        }
        _ => {}
    }
    if let Some(ai) = leaf_of(pf, AI).get(0).map(W::leaf).and_then(|l| l.str(0)).filter(|s| !s.is_empty()) {
        e.insert("behavior".into(), J::from(ai));
    }
    let m = model(pf);
    if !m.is_empty() {
        e.insert("model".into(), J::Dict(m));
    }
    if let Some(icon) = npc.str(ICON).filter(|s| !s.is_empty()) {
        e.insert("icon".into(), J::from(icon));
    }
    if !title.is_empty() || !sign_text.is_empty() {
        let mut s = Map::new();
        if !title.is_empty() {
            s.insert("title".into(), J::from(title));
        }
        if !sign_text.is_empty() {
            s.insert("text".into(), J::from(sign_text));
        }
        e.insert("sign".into(), J::Dict(s));
    }
    let st = stats(pf);
    if !st.is_empty() {
        e.insert("stats".into(), J::List(st));
    }
    Some(e)
}

pub fn build(ctx: &Ctx) -> Built {
    let tree = &ctx.tree;
    let prefabs = Prefabs::new(tree);
    let text = Text::new(tree);
    let (adventure_info, adventure_kills) = adventures(tree, &text);
    let placers = placers(tree, &text);
    let mut npcs: Vec<Map> = Vec::new();
    let mut abilities: IndexMap<String, Map> = IndexMap::new();
    let mut skipped = 0;
    for path in tree.files(&format!("prefabs/{ROOT}"), ".binfab") {
        let Ok(pf) = parse(&tree.read(&path).unwrap_or_default()) else { continue };
        if pf.type_id != Some(NPC_TYPE) || pf.component(NPC).is_none() {
            continue;
        }
        let r = rel_of(&path);
        let Some(mut e) = entry(&r, &pf, &text) else {
            skipped += 1;
            continue;
        };
        let rows = ability_rows(&pf);
        let carried: Vec<String> = list(leaf_of(&pf, CARRIED).get(1))
            .iter()
            .filter_map(|x| x.as_str().filter(|s| !s.is_empty()).map(rel_of))
            .collect();
        let all: Vec<String> =
            rows.iter().map(|x| x.str_or("ref", "").to_owned()).chain(carried.iter().cloned()).collect();
        for a in all {
            if !abilities.contains_key(&a) {
                let d = ability(&prefabs, &a, &text);
                abilities.insert(a, d);
            }
        }
        let rows: Vec<J> =
            rows.into_iter().filter(|x| abilities.get(x.str_or("ref", "")).is_some_and(|d| !d.is_empty())).collect();
        if !rows.is_empty() {
            e.insert("abilities".into(), J::List(rows));
        }
        let mut seen = IndexSet::new();
        let carried: Vec<String> = carried
            .into_iter()
            .filter(|c| seen.insert(c.clone()))
            .filter(|c| abilities.get(c).is_some_and(|d| !d.is_empty()))
            .collect();
        if !carried.is_empty() {
            e.insert("effects".into(), J::from(carried));
        }
        e.extend(services(&pf, &text));
        let offers: Vec<J> = offered(&pf)
            .into_iter()
            .map(|a| adventure_info.get(&a).cloned().unwrap_or_else(|| obj! {"id" => a.as_str()}))
            .collect();
        if !offers.is_empty() {
            e.insert("offers_adventures".into(), J::List(offers));
        }
        if let Some(counted) = adventure_kills.get(&r.to_lowercase()).filter(|c| !c.is_empty()) {
            let mut seen = IndexSet::new();
            let list: Vec<J> = counted
                .iter()
                .filter(|a| seen.insert((*a).clone()))
                .filter_map(|a| adventure_info.get(a).cloned())
                .collect();
            e.insert("defeat_adventures".into(), J::List(list));
        }
        if let Some(placed) = placers.get(&r.to_lowercase()).filter(|p| !p.is_empty()) {
            e.insert("placed_by".into(), J::List(placed.clone()));
        }
        npcs.push(e);
    }
    npcs.sort_by_cached_key(|e| {
        (
            e["group"].as_str().unwrap_or("").to_owned(),
            e["name"].as_str().unwrap_or("").to_lowercase(),
            e["slug"].as_str().unwrap_or("").to_owned(),
        )
    });
    let mut groups: IndexMap<String, (i64, i64)> = IndexMap::new();
    for e in &npcs {
        let g = groups.entry(e["group"].as_str().unwrap_or("").to_owned()).or_default();
        g.0 += 1;
        if e.contains_key("boss") {
            g.1 += 1;
        }
    }
    let mut groups: Vec<(String, (i64, i64))> = groups.into_iter().collect();
    groups.sort_by(|a, b| a.0.cmp(&b.0));
    let mut abilities: Vec<(String, Map)> = abilities.into_iter().filter(|(_, v)| !v.is_empty()).collect();
    abilities.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(obj! {
        "groups" => J::List(groups.into_iter().map(|(k, (n, b))| obj! {"id" => k, "npcs" => n, "bosses" => b}).collect()),
        "unnamed" => skipped,
        "npcs" => J::List(npcs.into_iter().map(J::Dict).collect()),
        "abilities" => J::Dict(abilities.into_iter().map(|(k, v)| (k, J::Dict(v))).collect()),
    })
}

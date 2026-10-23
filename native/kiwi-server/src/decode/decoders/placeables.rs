//! `placeables.py`: the blocks, decorations and building pieces a player can
//! own, grouped the way the game offers them.

use std::{cell::RefCell, collections::HashMap, sync::OnceLock};

use indexmap::{IndexMap, IndexSet};
use regex::Regex;

use super::{
    pvp_powerups::all_names,
    recipes::{PROFESSIONS, components, label, tabs, tiers},
};
use crate::decode::{
    common::{Locale, lookup, stem},
    json::{J, Map},
    registry::{Built, Ctx},
    tree::Tree,
    value::{Dict, W, parse},
};
use crate::obj;

const ROOT: &str = "placeable/";
const IDENTITY: i128 = 49;
const PLACER: i128 = 50;
const BLUEPRINT: i128 = 37;
const CRAFTING: i128 = 61;
const STATION: i128 = 76;
const TAGS: i128 = 106;
const VERSIONED: i128 = 311;
const FIXTURE: i128 = 409;
const WANTED: [i128; 8] = [IDENTITY, PLACER, BLUEPRINT, CRAFTING, STATION, TAGS, VERSIONED, FIXTURE];
const COMPOSTABLE: i128 = 26;
const SPLIT_STATION: i64 = 150;
const SMALL_STATION: i64 = 5;
const TEMPLATE_TABLES: [&str; 5] = ["trophies", "blocks", "torches", "signs", "particleinteractables"];
const CATEGORIES: [&str; 5] = ["Crafting", "Items", "Building", "Decoration", "Blocks"];
const ITEM_RARITY: [&str; 7] = ["Common", "Uncommon", "Rare", "Epic", "Legendary", "Relic", "Resplendent"];
const COLOURED: [i128; 5] = [18, 24, 25, 29, 55];
const RESTRICTION_TEXT: [(i128, &str); 6] = [
    (0x11, "$PlaceableNotInClubWorldOrGroupAccessLocations"),
    (8, "$PlaceableNotOnCornerstone"),
    (2, "$PlaceableNotOnPublicWorlds"),
    (0x10, "$PlaceableNotInClubWorld"),
    (4, "$PlaceableNotWhenAtMaxGardening"),
    (1, "$PlaceableNotInGroupAccessLocations"),
];
const BLOCK_TYPES: [(i32, &str); 59] = [
    (1, "Dirt"),
    (2, "Grass"),
    (3, "Stone"),
    (4, "Prefab"),
    (5, "With_Game_Object"),
    (7, "Sign"),
    (11, "Trophy"),
    (12, "Player_Trophy"),
    (15, "Bedrock"),
    (20, "Sand"),
    (24, "Player_Color"),
    (27, "Resource"),
    (28, "Reflective"),
    (30, "Canopy"),
    (38, "Tag"),
    (50, "Hard_Dirt"),
    (51, "Hard_Stone"),
    (57, "Plug"),
    (58, "Socket"),
    (59, "Prefab_Decoration"),
    (60, "Prefab_Indestructible"),
    (61, "Chest"),
    (62, "Sprout"),
    (63, "Plant"),
    (64, "Plant"),
    (69, "Flowing_Lava"),
    (70, "Plasma"),
    (71, "Flowing_Plasma"),
    (75, "Spawner"),
    (80, "Chocolate"),
    (81, "Flowing_Chocolate"),
    (83, "Acid"),
    (84, "Flowing_Acid"),
    (87, "Flowing_Balefire"),
    (89, "ParticlesInteractable"),
    (90, "Music_Note"),
    (91, "Music_Instrument"),
    (92, "Numbers"),
    (93, "Particles"),
    (94, "LED"),
    (95, "Milk"),
    (96, "Flowing_Milk"),
    (104, "Glowing_Grass"),
    (105, "Prefab_No_Collide"),
    (107, "Prefab_Intangible"),
    (120, "Railroad_Track"),
    (121, "Monorail_Track"),
    (122, "Railroad_Track"),
    (151, "Air_Vent"),
    (154, "Vanishing"),
    (158, "Air_Current_Source"),
    (159, "Air_Current"),
    (160, "Collision_Target"),
    (161, "Transmute"),
    (162, "Transmute_Passable"),
    (172, "Mimic"),
    (173, "Boat_Booster"),
    (174, "Anti_Gravity"),
    (176, "SpawnOnDestroy"),
];
const BLOCK_TYPES_TAIL: [(i32, &str); 3] = [(177, "Matchmaker"), (180, "Sky_Projector"), (181, "Jukebox")];
const FISH_SIZES: [&str; 4] = ["", "$FishSize_Medium", "$FishSize_Large", "$FishSize_ExtraLarge"];

/// `_rel`: a prefab reference, lowercased, or "".
fn rel(v: Option<&W>) -> String {
    match v {
        Some(W::Str(s)) if !s.is_empty() && !s.to_lowercase().ends_with(".blueprint") => {
            let low = s.trim().to_lowercase();
            let low = low.strip_prefix("prefabs/").unwrap_or(&low);
            low.strip_suffix(".binfab").unwrap_or(low).to_owned()
        }
        _ => String::new(),
    }
}

fn rel_s(s: &str) -> String {
    rel(Some(&W::Str(s.into())))
}

fn root_of(tree: &Tree, r: &str) -> Dict {
    parse(&tree.read(&format!("prefabs/{r}.binfab")).unwrap_or_default())
        .ok()
        .and_then(|pf| pf.root.map(|root| Dict::new(root.leaf().items().to_vec())))
        .unwrap_or_default()
}

fn slug(text: &str) -> String {
    static R: OnceLock<Regex> = OnceLock::new();
    let re = R.get_or_init(|| Regex::new(r"[^a-z0-9]+").expect("regex"));
    re.replace_all(&text.to_lowercase(), "-").trim_matches('-').to_owned()
}

type Comps = IndexMap<i128, W>;

fn prefabs_under(tree: &Tree, prefix: &str) -> IndexMap<String, Comps> {
    let mut out = IndexMap::new();
    for path in tree.files(&format!("prefabs/{prefix}"), ".binfab") {
        let comps = components(&tree.read(&path).unwrap_or_default(), &WANTED);
        if !comps.is_empty() {
            out.insert(rel_s(&path), comps);
        }
    }
    out
}

fn leaf(comps: &Comps, cid: i128) -> &Dict {
    comps.get(&cid).map_or(&crate::decode::value::EMPTY, W::leaf)
}

fn templates(tree: &Tree) -> IndexMap<String, Dict> {
    let mut out = IndexMap::new();
    for table in TEMPLATE_TABLES {
        for row in root_of(tree, &format!("blocks/{table}")).iter_of(1) {
            if row.is_obj()
                && row.sections().len() > 1
                && let Some(k) = row.section(0).str(0)
            {
                let last = row
                    .sections()
                    .last()
                    .and_then(W::as_dict)
                    .map(|d| Dict::new(d.items().to_vec()))
                    .unwrap_or_default();
                out.entry(k.to_lowercase()).or_insert(last);
            }
        }
    }
    out
}

type Rows = Vec<(IndexSet<String>, Vec<(String, i128)>)>;

struct Items<'a> {
    tree: &'a Tree,
    text: &'a Locale,
    cache: RefCell<HashMap<String, String>>,
}

impl Items<'_> {
    fn name(&self, r: &str) -> String {
        if let Some(n) = self.cache.borrow().get(r) {
            return n.clone();
        }
        let comps = components(&self.tree.read(&format!("prefabs/{r}.binfab")).unwrap_or_default(), &[IDENTITY]);
        let n = lookup(self.text, leaf(&comps, IDENTITY).get(1).filter(|v| v.truthy()));
        self.cache.borrow_mut().insert(r.to_owned(), n.clone());
        n
    }
}

struct Station {
    slug: String,
    name: String,
    tabs: Vec<String>,
    count: i64,
}

#[derive(Clone, PartialEq, Eq, Hash)]
enum GKey {
    Crafted(String, Option<usize>),
    Fixed(&'static str),
}

fn fmt_scalar(v: &W) -> String {
    match v {
        W::Int(i) => i.to_string(),
        W::Float(f) => crate::pyfloat::repr(*f),
        W::Str(s) => s.to_string(),
        W::None => "None".into(),
        other => format!("{other:?}"),
    }
}

pub fn build(ctx: &Ctx) -> Built {
    let tree = &ctx.tree;
    let text = all_names(tree);
    let items = Items { tree, text: &text, cache: RefCell::default() };
    let placeables = prefabs_under(tree, ROOT);
    let npcs = prefabs_under(tree, "npc/");
    let templates = templates(tree);
    let mut known: IndexMap<String, String> = IndexMap::new();
    for p in tree.files("prefabs/recipes/", ".binfab") {
        let s = stem(&p);
        known.insert(s.to_lowercase(), s);
    }
    let mut results: HashMap<String, Vec<String>> = HashMap::new();
    let mut ingredients: HashMap<String, Vec<String>> = HashMap::new();
    for rid in known.values() {
        let root = root_of(tree, &format!("recipes/{rid}"));
        let pick = |i: i128| -> Vec<String> {
            root.iter_of(i).iter().map(|o| rel(o.leaf().get(0))).filter(|r| r.starts_with(ROOT)).collect()
        };
        results.insert(rid.clone(), pick(1));
        ingredients.insert(rid.clone(), pick(0));
    }
    let mut loaded: IndexSet<String> = IndexSet::new();
    for group in root_of(tree, "collections/collection_recipe").iter_of(0) {
        for row in group.leaf().iter_of(3) {
            if let Some(rid) = row.leaf().str(0)
                && let Some(k) = known.get(&rid.to_lowercase())
            {
                loaded.insert(k.clone());
            }
        }
    }
    let valid: IndexSet<String> = tree
        .files("blueprints/", ".blueprint")
        .iter()
        .map(|p| p.strip_prefix("blueprints/").unwrap_or(p).to_lowercase())
        .collect();
    let mut stems: HashMap<String, Vec<String>> = HashMap::new();
    for bp in &valid {
        let base = bp.rsplit('/').next().unwrap_or(bp);
        let base = base.strip_suffix(".blueprint").unwrap_or(base);
        let base = base.split('[').next().unwrap_or(base);
        stems.entry(base.to_owned()).or_default().push(bp.clone());
    }

    let mut places: IndexMap<String, String> = IndexMap::new();
    for (r, comps) in &placeables {
        let data = leaf(comps, PLACER).get(0).map(W::leaf).and_then(|b| b.get(2));
        let target = match data {
            Some(d) if d.is_obj() && !d.is_empty() => rel(d.section(0).get(0)),
            _ => String::new(),
        };
        if !target.is_empty() {
            places.insert(r.clone(), target);
        }
    }
    let mut placer: IndexMap<String, String> = IndexMap::new();
    for (k, v) in places.iter().rev() {
        placer.insert(v.clone(), k.clone());
    }
    let name_of = |r: &str| -> String {
        placeables.get(r).map(|c| lookup(&text, leaf(c, IDENTITY).get(1).filter(|v| v.truthy()))).unwrap_or_default()
    };

    let mut stations: IndexMap<String, Station> = IndexMap::new();
    let mut offers: IndexMap<String, Vec<(String, usize)>> = IndexMap::new();
    let mut rank: HashMap<(String, String), usize> = HashMap::new();
    for (r, comps) in placeables.iter().chain(npcs.iter()) {
        let Some(crafting) = comps.get(&CRAFTING) else { continue };
        let st = leaf(comps, STATION);
        let prof = st.int(2).and_then(|p| usize::try_from(p).ok()).and_then(|p| PROFESSIONS.get(p)).copied();
        let tab_list: Vec<(J, Vec<String>)> = match prof {
            Some(p) => tiers(tree, p).into_iter().map(|(n, _, ids)| (n, ids)).collect(),
            None => tabs(crafting),
        };
        let mut title = label(&st.get(0).map_or(J::Null, J::from), &text);
        if title.is_empty() {
            title = name_of(placer.get(r).map_or("", String::as_str));
        }
        let s = r.strip_prefix(ROOT).unwrap_or(r);
        let s = s.strip_prefix("crafting/").unwrap_or(s).replace('/', "_");
        stations.insert(
            r.clone(),
            Station { slug: s, name: title, tabs: tab_list.iter().map(|(n, _)| label(n, &text)).collect(), count: 0 },
        );
        for (i, (_, ids)) in tab_list.iter().enumerate() {
            let mut seen: IndexSet<String> = IndexSet::new();
            for rid in ids.iter().map(|x| known.get(&x.to_lowercase()).cloned().unwrap_or_default()) {
                if !seen.insert(rid.clone()) || !loaded.contains(&rid) {
                    continue;
                }
                for res in results.get(&rid).into_iter().flatten() {
                    let spots = offers.entry(res.clone()).or_default();
                    if !spots.contains(&(r.clone(), i)) {
                        spots.push((r.clone(), i));
                    }
                    let n = rank.len();
                    rank.entry((r.clone(), res.clone())).or_insert(n);
                }
            }
        }
    }
    for spots in offers.values() {
        let mut seen: IndexSet<&String> = IndexSet::new();
        for (s, _) in spots {
            if seen.insert(s)
                && let Some(st) = stations.get_mut(s)
            {
                st.count += 1;
            }
        }
    }
    let mut made_by: IndexMap<String, Vec<String>> = IndexMap::new();
    let mut used_in: IndexMap<String, Vec<String>> = IndexMap::new();
    let mut sorted_loaded: Vec<&String> = loaded.iter().collect();
    sorted_loaded.sort();
    for rid in sorted_loaded {
        for res in results.get(rid).into_iter().flatten() {
            made_by.entry(res.clone()).or_default().push(rid.clone());
        }
        for ing in ingredients.get(rid).into_iter().flatten() {
            used_in.entry(ing.clone()).or_default().push(rid.clone());
        }
    }

    let mut claims: IndexMap<String, Vec<String>> = IndexMap::new();
    for path in tree.files("prefabs/claim/", ".binfab") {
        let table = rel_s(&path);
        if table == "claim/test" {
            continue;
        }
        let root = W::Dict(std::sync::Arc::new(root_of(tree, &table)));
        for s in root.strings() {
            let r = rel_s(&s);
            if r.starts_with(ROOT) {
                let list = claims.entry(r).or_default();
                if !list.contains(&table) {
                    list.push(table.clone());
                }
            }
        }
    }
    let mut fishing: IndexMap<String, J> = IndexMap::new();
    for row in root_of(tree, "fish/fish").iter_of(0) {
        let row = row.leaf();
        let fish = rel(row.get(0));
        for size in 1..4 {
            let trophy = rel(row.get(size));
            if !fish.is_empty() && !trophy.is_empty() && !fishing.contains_key(&trophy) {
                let size_name = text.get(FISH_SIZES[size as usize]).cloned().unwrap_or_default();
                fishing.insert(
                    trophy,
                    obj! {"fish" => fish.as_str(), "fish_name" => items.name(&fish), "size" => size_name},
                );
            }
        }
    }
    let mut fixtures: IndexMap<String, String> = IndexMap::new();
    for section in root_of(tree, "meta/inventory").iter_of(2) {
        let section = section.leaf();
        for r in section.iter_of(0) {
            let r = rel(Some(&r));
            if !r.is_empty() {
                fixtures.entry(r).or_insert_with(|| lookup(&text, section.get(1).filter(|v| v.truthy())));
            }
        }
    }
    let master = root_of(tree, "crafting/deconstruct");
    let put = |v: &mut Vec<((J, J), String)>, key: (J, J), val: String| match v.iter_mut().find(|(k, _)| *k == key) {
        Some(slot) => slot.1 = val,
        None => v.push((key, val)),
    };
    let mut by_type: Vec<((J, J), String)> = Vec::new();
    for r in master.iter_of(0) {
        let r = r.leaf();
        put(&mut by_type, (r.get(0).map_or(J::Null, J::from), r.get(2).map_or(J::Null, J::from)), rel(r.get(1)));
    }
    let mut by_prefab: Vec<((J, J), String)> = Vec::new();
    for r in master.iter_of(2) {
        let r = r.leaf();
        put(&mut by_prefab, (J::from(rel(r.get(0))), r.get(2).map_or(J::Null, J::from)), rel(r.get(1)));
    }
    let find = |v: &[((J, J), String)], key: &(J, J)| v.iter().find(|(k, _)| k == key).map(|(_, t)| t.clone());
    let mut decon_rows: HashMap<String, Rows> = HashMap::new();
    for table in by_type.iter().chain(by_prefab.iter()).map(|(_, t)| t) {
        if table.is_empty() || decon_rows.contains_key(table) {
            continue;
        }
        let mut rows: Rows = Vec::new();
        for r in root_of(tree, table).iter_of(0) {
            let r = r.leaf();
            let need: IndexSet<String> = r.iter_of(0).iter().filter_map(|t| t.as_str().map(str::to_owned)).collect();
            let outs: Vec<(String, i128)> = r
                .iter_of(1)
                .iter()
                .filter_map(|o| {
                    let p = rel(o.leaf().get(0));
                    let n = o.leaf().int(1)?;
                    (!p.is_empty() && n != 0).then_some((p, n))
                })
                .collect();
            rows.push((need, outs));
        }
        decon_rows.insert(table.clone(), rows);
    }
    let collected: IndexSet<String> =
        by_prefab.iter().filter_map(|((p, _), _)| p.as_str().map(str::to_owned)).collect();

    let deconstruct = |r: &str, comps: &Comps, rarity: &str, item_type: Option<&W>| -> Map {
        let mut tags: Vec<String> = leaf(comps, TAGS)
            .iter_of(0)
            .iter()
            .filter_map(|t| t.as_str().filter(|s| !s.is_empty()).map(str::to_owned))
            .collect();
        if comps.contains_key(&VERSIONED) {
            let v = leaf(comps, VERSIONED).get(2).cloned().unwrap_or(W::Int(0));
            tags = tags.iter().map(|t| format!("{t}_ver{}", fmt_scalar(&v))).collect();
        }
        let mut keys: IndexSet<String> = tags.into_iter().collect();
        keys.insert(rarity.to_owned());
        let it = item_type.map_or(J::Null, J::from);
        let mut out = Map::new();
        for (station, lbl) in [("loot_collector", 2), ("compost", 8)] {
            let table = find(&by_prefab, &(J::from(r), J::Int(lbl)))
                .filter(|t| !t.is_empty())
                .or_else(|| find(&by_type, &(it.clone(), J::Int(lbl))))
                .unwrap_or_default();
            let mut total: IndexMap<String, i128> = IndexMap::new();
            for (need, outs) in decon_rows.get(&table).into_iter().flatten() {
                if need.iter().all(|n| keys.contains(n)) {
                    for (item, n) in outs {
                        *total.entry(item.clone()).or_insert(0) += n;
                    }
                }
            }
            if !total.is_empty() {
                let rows: Vec<J> = total
                    .into_iter()
                    .map(|(i, n)| obj! {"item" => i.as_str(), "name" => items.name(&i), "count" => n})
                    .collect();
                out.insert(station.into(), J::List(rows));
            }
        }
        out
    };
    let model = |r: &str, comps: &Comps, template: &Dict| -> String {
        if let Some(own) = leaf(comps, BLUEPRINT).str(0).filter(|s| !s.is_empty()) {
            return own.to_owned();
        }
        if let Some(name) = template.str(1).filter(|s| !s.is_empty()) {
            let m = name.replace('\\', "/").to_lowercase();
            let m = m.strip_suffix(".blueprint").unwrap_or(&m).to_owned();
            if valid.contains(&format!("{m}.blueprint")) {
                return format!("{m}.blueprint");
            }
            if let Some(hits) = stems.get(&m)
                && hits.len() == 1
            {
                return hits[0].clone();
            }
        }
        let twin =
            places.get(r).and_then(|t| placeables.get(t)).and_then(|c| leaf(c, BLUEPRINT).str(0).map(str::to_owned));
        twin.unwrap_or_default()
    };

    let mut groups: IndexMap<GKey, Vec<(usize, Map)>> = IndexMap::new();
    for (r, comps) in &placeables {
        let ident = leaf(comps, IDENTITY);
        let name = lookup(&text, ident.get(1).filter(|v| v.truthy()));
        if name.is_empty() {
            continue;
        }
        let item_type = ident.get(10);
        let compostable = item_type.is_some_and(|t| J::from(t) == J::Int(COMPOSTABLE));
        let mut spots: Vec<(String, usize)> = offers.get(r).cloned().unwrap_or_default();
        spots.sort_by_cached_key(|(s, i)| (-stations[s].count, stations[s].slug.clone(), *i));
        if spots.is_empty()
            && !fishing.contains_key(r)
            && !claims.contains_key(r)
            && !fixtures.contains_key(r)
            && !used_in.contains_key(r)
            && !compostable
            && !collected.contains(r)
        {
            continue;
        }
        let rarity =
            ident.int(9).and_then(|i| usize::try_from(i).ok()).and_then(|i| ITEM_RARITY.get(i)).copied().unwrap_or("");
        let category = ident.str(2).filter(|c| CATEGORIES.contains(c)).unwrap_or("");
        let block = leaf(comps, PLACER).get(0).map_or(&crate::decode::value::EMPTY, W::leaf);
        let key = match block.get(2) {
            Some(d) if d.is_obj() && !d.is_empty() => d.section(0).get(6).cloned(),
            _ => None,
        };
        let empty = Dict::default();
        let template =
            key.as_ref().and_then(W::as_str).and_then(|k| templates.get(&k.to_lowercase())).unwrap_or(&empty);
        let mut entry = Map::new();
        entry.insert("slug".into(), J::from(r.strip_prefix(ROOT).unwrap_or(r).replace('/', "_")));
        entry.insert("name".into(), J::from(name));
        entry.insert("description".into(), J::from(lookup(&text, ident.get(5).filter(|v| v.truthy()))));
        entry.insert("prefab".into(), J::from(r.as_str()));
        if !category.is_empty() {
            entry.insert("category".into(), J::from(category));
        }
        if !rarity.is_empty() {
            entry.insert("rarity".into(), J::from(rarity));
        }
        let bp = model(r, comps, template);
        if !bp.is_empty() {
            entry.insert("blueprint".into(), J::from(bp));
        }
        if let Some(btype) = block.int(0) {
            entry.insert("block_type".into(), J::Int(btype));
            if COLOURED.contains(&btype)
                && let Some(param) = block.int(1)
            {
                entry.insert("color".into(), J::from(format!("#{:06x}", param & 0xFFFFFF)));
            }
        }
        if let Some(light) = template.int(7).filter(|l| *l > 0) {
            entry.insert("light".into(), J::Int(light));
        }
        if let Some(vfx) = template.str(3).filter(|s| !s.is_empty()) {
            entry.insert("vfx".into(), J::from(vfx));
        }
        if let Some(mask) = leaf(comps, PLACER).int(4).filter(|m| *m != 0) {
            let (mut lines, mut left) = (Vec::new(), mask);
            for (bits, line) in RESTRICTION_TEXT {
                if left & bits == bits {
                    lines.push(text.get(line).cloned().unwrap_or_default());
                    left &= !bits;
                }
            }
            entry.insert(
                "restrictions".into(),
                J::from(lines.into_iter().filter(|l| !l.is_empty()).collect::<Vec<_>>()),
            );
        }
        if let Some(target) = places.get(r) {
            entry.insert("places".into(), J::from(target.as_str()));
            if let Some(st) = stations.get(target) {
                entry.insert("station".into(), obj! {"slug" => st.slug.as_str(), "name" => st.name.as_str()});
            }
        }
        if !spots.is_empty() {
            let at: Vec<J> = spots
                .iter()
                .map(|(s, i)| obj! {"station" => stations[s].slug.as_str(), "name" => stations[s].name.as_str(), "tab" => stations[s].tabs[*i].as_str()})
                .collect();
            entry.insert("crafted_at".into(), J::List(at));
            entry.insert("recipes".into(), J::from(made_by.get(r).cloned().unwrap_or_default()));
        }
        if let Some(u) = used_in.get(r) {
            entry.insert("used_in".into(), J::from(u.clone()));
        }
        if let Some(f) = fishing.get(r) {
            entry.insert("fishing".into(), f.clone());
        }
        if let Some(c) = claims.get(r) {
            entry.insert("claims".into(), J::from(c.clone()));
        }
        if let Some(section) = fixtures.get(r) {
            entry.insert("club_section".into(), J::from(section.as_str()));
            let tiers: Vec<String> = leaf(comps, FIXTURE)
                .iter_of(1)
                .iter()
                .map(|t| lookup(&text, t.leaf().get(6).filter(|v| v.truthy())))
                .collect();
            if tiers.iter().any(|t| !t.is_empty()) {
                entry.insert("tiers".into(), J::from(tiers));
            }
        }
        entry.extend(deconstruct(r, comps, rarity, item_type));
        let gkey = if let Some((s, i)) = spots.first() {
            let home = stations[s].count;
            if home < SMALL_STATION {
                GKey::Fixed("small")
            } else {
                GKey::Crafted(s.clone(), (home > SPLIT_STATION).then_some(*i))
            }
        } else if comps.contains_key(&VERSIONED) {
            GKey::Fixed("fish_trophies")
        } else if compostable {
            GKey::Fixed("trophies")
        } else if fixtures.contains_key(r) {
            GKey::Fixed("club_fixtures")
        } else {
            GKey::Fixed("other")
        };
        let rk = spots.first().map_or(0, |(s, _)| rank.get(&(s.clone(), r.clone())).copied().unwrap_or(0));
        groups.entry(gkey).or_default().push((rk, entry));
    }
    let mut categories = Map::new();
    for c in CATEGORIES {
        categories.insert(
            c.into(),
            J::from(text.get(&format!("$AuctionItemCategory_{c}")).cloned().unwrap_or_else(|| c.to_owned())),
        );
    }
    let mut block_types = Map::new();
    for (k, v) in BLOCK_TYPES.iter().chain(BLOCK_TYPES_TAIL.iter()) {
        let name = text.get(&format!("$Block_{v}_name")).cloned().unwrap_or_else(|| v.replace('_', " "));
        block_types.insert(k.to_string(), J::from(name));
    }
    Ok(obj! {"categories" => categories, "block_types" => block_types, "groups" => finish(groups, &stations)})
}

fn finish(groups: IndexMap<GKey, Vec<(usize, Map)>>, stations: &IndexMap<String, Station>) -> Vec<J> {
    let fixed = |k: &str| match k {
        "small" => "Other crafting stations",
        "fish_trophies" => "Fish trophies",
        "trophies" => "Compostable decorations",
        "club_fixtures" => "Club fixtures",
        _ => "Other placeables",
    };
    let order = |k: &GKey| match k {
        GKey::Crafted(..) => 0,
        GKey::Fixed(f) => ["crafted", "small", "fish_trophies", "trophies", "club_fixtures", "other"]
            .iter()
            .position(|x| x == f)
            .unwrap_or(5),
    };
    let mut labels: IndexMap<GKey, String> = IndexMap::new();
    for gkey in groups.keys() {
        let l = match gkey {
            GKey::Crafted(s, i) => {
                let st = &stations[s];
                let tab = i.map(|i| st.tabs[i].clone()).unwrap_or_default();
                if tab.is_empty() { st.name.clone() } else { tab }
            }
            GKey::Fixed(f) => fixed(f).to_owned(),
        };
        labels.insert(gkey.clone(), l);
    }
    let names: Vec<&String> = labels.values().collect();
    let mut rows: Vec<(bool, usize, String, Map, Vec<J>)> = Vec::new();
    for (gkey, mut members) in groups {
        let entries: Vec<Map> = if matches!(gkey, GKey::Crafted(..)) {
            members.sort_by_key(|m| m.0);
            members.into_iter().map(|(_, e)| e).collect()
        } else {
            let mut e: Vec<Map> = members.into_iter().map(|(_, e)| e).collect();
            e.sort_by_cached_key(|e| {
                (e["name"].as_str().unwrap_or("").to_lowercase(), e["slug"].as_str().unwrap_or("").to_owned())
            });
            e
        };
        let cats: Vec<&str> = entries.iter().map(|e| e.get("category").and_then(J::as_str).unwrap_or("")).collect();
        let count = |c: &str| cats.iter().filter(|x| **x == c).count();
        let mut best = CATEGORIES[0];
        for c in CATEGORIES {
            if count(c) > count(best) {
                best = c;
            }
        }
        let lbl = labels[&gkey].clone();
        let mut row = Map::new();
        row.insert("name".into(), J::from(lbl.as_str()));
        row.insert("category".into(), J::from(best));
        if let GKey::Crafted(s, _) = &gkey {
            let st = &stations[s];
            if names.iter().filter(|n| ***n == lbl).count() > 1 {
                row.insert("name".into(), J::from(format!("{lbl} ({})", st.name)));
            }
            row.insert("station".into(), obj! {"slug" => st.slug.as_str(), "name" => st.name.as_str()});
        }
        let cat_rank = CATEGORIES.iter().rev().position(|c| *c == best).unwrap_or(0);
        let name_low = row["name"].as_str().unwrap_or("").to_lowercase();
        rows.push((order(&gkey) > 1, cat_rank, name_low, row, entries.into_iter().map(J::Dict).collect()));
    }
    rows.sort_by(|a, b| (a.0, a.1, &a.2).cmp(&(b.0, b.1, &b.2)));
    let mut out = Vec::new();
    let mut slugs: IndexSet<String> = IndexSet::new();
    for (_, _, _, row, entries) in rows {
        let mut s = slug(row["name"].as_str().unwrap_or(""));
        while slugs.contains(&s) {
            s.push_str("-2");
        }
        slugs.insert(s.clone());
        let mut m = Map::new();
        m.insert("slug".into(), J::from(s));
        m.extend(row);
        m.insert("entries".into(), J::List(entries));
        out.push(J::Dict(m));
    }
    out
}

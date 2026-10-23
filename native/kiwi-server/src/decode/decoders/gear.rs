//! `gear.py`: what the client knows about equipment, for the /gear-guide page.

use std::sync::OnceLock;

use indexmap::{IndexMap, IndexSet};
use regex::{Regex, bytes::Regex as BRegex};

use super::pvp_powerups::all_names;
use crate::decode::{
    ability::{Prefabs, identity},
    common::{Locale, MOD_OPS, r4, rp, rs, stat_key},
    json::{J, Map, dumps},
    registry::{Built, Ctx},
    tree::Tree,
    value::{Dict, K, W},
};
use crate::obj;

const IDENTITY: i128 = 49;
const SLOT_COMP: i128 = 65;
const TEMPLATE: i128 = 128;
const BANNER_TIERS: i128 = 351;
const STATION: i128 = 146;
const BLUEPRINT: i128 = 37;
const RARITIES: [&str; 32] = [
    "Common",
    "Uncommon",
    "Rare",
    "Epic",
    "Legendary",
    "Relic",
    "Resplendent",
    "Shadow1",
    "Shadow2",
    "Shadow3",
    "Shadow4",
    "Shadow5",
    "Radiant1",
    "Stellar1",
    "Radiant2",
    "Stellar2",
    "Radiant3",
    "Stellar3",
    "Radiant4",
    "Stellar4",
    "Radiant5",
    "Stellar5",
    "Crystal1",
    "Crystal2",
    "Crystal3",
    "Crystal4",
    "Crystal5",
    "Mystic1",
    "Mystic2",
    "Mystic3",
    "Mystic4",
    "Mystic5",
];
const SLOTS: [(i128, &str, &str); 5] = [
    (0, "Weapon", "$EquipmentSlot_Weapon"),
    (4, "Hat", "$EquipmentSlot_Hat"),
    (5, "Face", "$EquipmentSlot_Face"),
    (6, "Ring", "$EquipmentSlot_Ring"),
    (33, "Banner", "$EquipmentSlot_PVPBanner"),
];
const ITEM_TYPES: [(i128, i128, &str, &str); 13] = [
    (4, 0, "Gun", "$ItemType_Gun"),
    (5, 0, "Bow", "$ItemType_Bow"),
    (6, 0, "Staff", "$ItemType_Staff"),
    (8, 0, "Spear", "$ItemType_Spear"),
    (9, 0, "Fist", "$ItemType_Fist"),
    (14, 0, "Melee", "$ItemType_Melee"),
    (12, 4, "Hat", "$ItemType_Hat"),
    (13, 5, "Face", "$ItemType_Face"),
    (15, 6, "Ring_MaxHealth", "$Stat_MaxHealth"),
    (16, 6, "Ring_Lasermancy", "$Stat_Mining"),
    (17, 6, "Ring_PhysicalDamage", "$Stat_PhysicalDamage"),
    (18, 6, "Ring_MagicDamage", "$Stat_SpellDamage"),
    (20, 33, "Banner", "$ItemType_Banner"),
];
const UPGRADE_TYPES: [&str; 14] = [
    "AddStat",
    "AddQuality",
    "ReduceMinimumLevel",
    "IncreaseRarity",
    "PickNewStats",
    "AddStatBonus",
    "PickNewParticles",
    "AddGemQuality",
    "RepairGem",
    "AddGemStatBonus",
    "ReselectGemStats",
    "MoveGemUpgrade",
    "UpgradeTreeBonus",
    "UpgradeGemTier",
];
const UPGRADE_LABEL: [&str; 7] = [
    "$ItemUpgrade_AddStat",
    "$ItemUpgrade_AddQuality",
    "$ItemUpgrade_ReduceMinLevel",
    "$ItemUpgrade_IncreaseRarity",
    "$ItemUpgrade_NewStats",
    "$ItemUpgrade_AddStatBonus",
    "$ItemUpgrade_NewParticles",
];
const PVE: i128 = 1;

fn required_level(rid: usize) -> i64 {
    match rid {
        0 | 1 => 0,
        2 => 4,
        3 => 6,
        4 => 8,
        5..=21 => 10,
        22..=26 => 20,
        _ => 30,
    }
}

fn rows(v: Option<&W>) -> Vec<&Dict> {
    v.map(W::elems).unwrap_or_default().iter().filter(|r| r.is_obj()).map(W::leaf).collect()
}

fn family(key: &str) -> (String, Option<i64>) {
    static R: OnceLock<Regex> = OnceLock::new();
    let re = R.get_or_init(|| Regex::new(r"^([A-Za-z]+?)(\d)$").expect("regex"));
    match re.captures(key) {
        Some(m) => (m[1].to_owned(), m[2].parse().ok()),
        None => (key.to_owned(), None),
    }
}

fn stat_name(sid: i128, loc: &Locale) -> String {
    let key = stat_key(sid);
    loc.get(&key)
        .filter(|s| !s.is_empty())
        .or_else(|| loc.get(key.strip_suffix("_controller").unwrap_or(&key)).filter(|s| !s.is_empty()))
        .cloned()
        .unwrap_or(key)
}

fn name_of(prefabs: &Prefabs, r: &str, loc: &Locale) -> String {
    let ident = identity(prefabs.get(r).as_deref());
    loc.get(ident.name())
        .filter(|s| !s.is_empty())
        .cloned()
        .unwrap_or_else(|| r.rsplit('/').next().unwrap_or(r).to_owned())
}

fn model(tree: &Tree, prefabs: &Prefabs, r: &str, valid: &IndexSet<String>) -> Option<String> {
    static R: OnceLock<BRegex> = OnceLock::new();
    let re = R.get_or_init(|| BRegex::new(r"(?-u)[A-Za-z0-9_/\[\]\-]+\.blueprint").expect("regex"));
    if let Some(bp) = prefabs
        .get(r)
        .and_then(|p| p.component(BLUEPRINT).and_then(|c| c.leaf().str(0).map(str::to_owned)))
        .filter(|s| !s.is_empty())
    {
        let name = if bp.to_lowercase().ends_with(".blueprint") { bp } else { format!("{bp}.blueprint") };
        if valid.contains(&name.to_lowercase()) {
            return Some(name);
        }
    }
    let data = tree.read(&format!("prefabs/{r}.binfab")).unwrap_or_default();
    let named: IndexSet<String> = re
        .find_iter(&data)
        .map(|m| String::from_utf8_lossy(m.as_bytes()).to_lowercase())
        .filter(|n| valid.contains(n))
        .collect();
    let flags: Vec<&String> = named.iter().filter(|n| n.rsplit('/').next().unwrap_or(n).contains("flag")).collect();
    if named.len() > 1 && flags.len() == 1 {
        return Some(flags[0].clone());
    }
    (named.len() == 1).then(|| named[0].clone())
}

fn costs(v: Option<&W>, prefabs: &Prefabs, loc: &Locale) -> Vec<J> {
    let leaf = v.map_or(&crate::decode::value::EMPTY, W::leaf);
    costs_of(leaf, prefabs, loc)
}

fn costs_of(leaf: &Dict, prefabs: &Prefabs, loc: &Locale) -> Vec<J> {
    rows(leaf.get(0))
        .into_iter()
        .filter_map(|c| {
            let item = c.str(0)?;
            Some(obj! {"item" => item, "name" => name_of(prefabs, item, loc), "count" => c.get(1).map_or(J::Int(1), J::from)})
        })
        .collect()
}

fn mods(v: Option<&W>, loc: &Locale) -> Vec<Map> {
    let mut out = Vec::new();
    for r in rows(v) {
        let (sid, op) = (r.int(0).unwrap_or(0), r.int(1).unwrap_or(0));
        let Some(val) = r.get(2).and_then(W::as_num) else { continue };
        if !(0..MOD_OPS.len() as i128).contains(&op) {
            continue;
        }
        let percent = op == 0 || op == 4;
        let mut row = Map::new();
        row.insert("name".into(), J::from(stat_name(sid, loc)));
        row.insert("value".into(), J::Float(r4(if percent { val * 100.0 } else { val })));
        if percent {
            row.insert("percent".into(), J::Bool(true));
        }
        if r.int(5).unwrap_or(0) & PVE != 0 {
            row.insert("pve".into(), J::Bool(true));
        }
        out.push(row);
    }
    out
}

fn sorted_items(v: Option<&W>) -> Vec<(i128, W)> {
    let Some(W::Dict(d)) = v.filter(|x| x.truthy()) else { return Vec::new() };
    let mut items: Vec<(i128, W)> = d
        .items()
        .iter()
        .filter_map(|(k, v)| match k {
            K::Int(i) => Some((*i, v.clone())),
            K::Str(_) => None,
        })
        .collect();
    items.sort_by_key(|(k, _)| *k);
    items
}

fn rarities(prefabs: &Prefabs, loc: &Locale) -> (Vec<J>, Option<&'static str>) {
    let pr_pf = prefabs.get("meta/powerrank");
    let mut pr: IndexMap<i128, i128> = IndexMap::new();
    for r in rows(pr_pf.as_ref().and_then(|p| p.root.as_ref()).map(W::leaf).and_then(|l| l.get(0))) {
        if let (Some(a), Some(b)) = (r.int(0), r.int(1)) {
            pr.insert(a, b);
        }
    }
    let up_pf = prefabs.get("crafting/upgrade");
    let up_root = up_pf.as_ref().and_then(|p| p.root.as_ref()).map_or(&crate::decode::value::EMPTY, W::leaf);
    let mut upgrade: IndexMap<i128, Option<W>> = IndexMap::new();
    for r in rows(up_root.get(0)) {
        if let Some(k) = r.int(0) {
            upgrade.insert(k, r.get(1).filter(|v| v.truthy()).cloned());
        }
    }
    let raise_from = up_root.int(1);
    let mut out = Vec::new();
    for (rid, key) in RARITIES.iter().enumerate() {
        let (fam, level) = family(key);
        let base = loc.get(&format!("$Rarity_{fam}")).cloned().unwrap_or_else(|| fam.clone());
        let c = upgrade.get(&(rid as i128)).cloned().flatten();
        let mut improve = Map::new();
        for (k, v) in sorted_items(c.as_ref()) {
            if k != 0 {
                improve.insert(k.to_string(), J::List(costs(Some(&v), prefabs, loc)));
            }
        }
        let raise_cost = if raise_from.is_some_and(|r| rid as i128 == r + 1) {
            let zero = match &c {
                Some(W::Dict(d)) => d.get(0).cloned(),
                _ => None,
            };
            costs(zero.as_ref(), prefabs, loc)
        } else {
            Vec::new()
        };
        let name = match level {
            Some(l) => format!("{base} {l}"),
            None => base,
        };
        out.push(obj! {
            "id" => rid, "key" => *key, "family" => fam, "level" => level, "name" => name,
            "required_level" => required_level(rid), "power_rank" => pr.get(&(rid as i128 + 1)).copied(),
            "aura" => obj! {"weapon" => rid >= 12, "hat" => rid >= 13},
            "improve_cost" => if improve.is_empty() { J::Null } else { J::Dict(improve) },
            "raise_cost" => if raise_cost.is_empty() { J::Null } else { J::List(raise_cost) },
        });
    }
    let rf = raise_from.and_then(|r| usize::try_from(r).ok()).and_then(|r| RARITIES.get(r)).copied();
    (out, rf)
}

fn stations(tree: &Tree, prefabs: &Prefabs, loc: &Locale) -> Vec<J> {
    let mut out = Vec::new();
    for path in tree.files("prefabs/placeable/crafting/", "_interactive.binfab") {
        let rel = rs(rp(&path, "prefabs/"), ".binfab");
        let Some(pf) = prefabs.get(rel) else { continue };
        let Some(comp) = pf.component(STATION).filter(|c| c.sections().len() >= 2) else { continue };
        let mut ops = Vec::new();
        for (i, row) in rows(comp.section(1).get(1)).into_iter().enumerate() {
            if row.get(2).is_some_and(W::truthy) && i < 7 {
                let label = loc.get(UPGRADE_LABEL[i]).cloned().unwrap_or_else(|| UPGRADE_TYPES[i].to_owned());
                let cost = costs_of(row, prefabs, loc);
                ops.push(obj! {"key" => UPGRADE_TYPES[i], "label" => label, "cost" => if cost.is_empty() { J::Null } else { J::List(cost) }});
            }
        }
        if !ops.is_empty() {
            out.push(
                obj! {"prefab" => rel, "name" => name_of(prefabs, rs(rel, "_interactive"), loc), "operations" => ops},
            );
        }
    }
    out
}

pub fn build(ctx: &Ctx) -> Built {
    let tree = &ctx.tree;
    let loc = all_names(tree);
    let prefabs = Prefabs::new(tree);
    let (rarities, raise_from) = rarities(&prefabs, &loc);
    let mut named: Vec<Map> = Vec::new();
    let mut banners: IndexMap<String, Map> = IndexMap::new();
    let paths: Vec<String> =
        ["prefabs/equipment/", "prefabs/item/banner/"].iter().flat_map(|r| tree.files(r, ".binfab")).collect();
    for path in paths {
        let rel = rs(rp(&path, "prefabs/"), ".binfab");
        let Some(pf) = prefabs.get(rel) else { continue };
        let ident = pf.component(IDENTITY).map_or(&crate::decode::value::EMPTY, W::leaf);
        let slot_c = pf.component(SLOT_COMP).map_or(&crate::decode::value::EMPTY, W::leaf);
        let Some(slot) = slot_c.int(0).and_then(|s| SLOTS.iter().find(|(id, _, _)| *id == s)) else { continue };
        let mut rid = ident.get(9).cloned();
        for i in [17, 20] {
            if let Some(W::Int(v)) = ident.get(i)
                && *v != -1
            {
                rid = Some(W::Int(*v));
            }
        }
        let rarity = match rid {
            Some(W::Int(r)) if (0..RARITIES.len() as i128).contains(&r) => Some(RARITIES[r as usize]),
            _ => None,
        };
        let itype = ident.int(10).and_then(|t| ITEM_TYPES.iter().find(|x| x.0 == t));
        let name = ident.str(1).and_then(|k| loc.get(k)).filter(|s| !s.is_empty()).cloned();
        if let (Some(tmpl), Some(it)) = (pf.component(TEMPLATE), itype) {
            let stats: Vec<J> = rows(tmpl.leaf().get(8))
                .into_iter()
                .map(|s| {
                    let suffix = if s.get(3).is_some_and(|v| J::from(v) == J::Int(0)) { " %" } else { "" };
                    J::from(format!("{}{suffix}", stat_name(s.int(0).unwrap_or(0), &loc)))
                })
                .collect();
            if rarity != Some("Common")
                && !stats.is_empty()
                && let Some(n) = &name
            {
                let mut m = Map::new();
                m.insert("name".into(), J::from(n.as_str()));
                m.insert("prefab".into(), J::from(rel));
                m.insert("slot".into(), J::from(slot.1));
                m.insert("item_type".into(), J::from(it.2));
                m.insert("rarity".into(), J::from(rarity));
                m.insert("stats".into(), J::List(stats));
                named.push(m);
            }
        }
        if slot.0 == 33 {
            let mut fixed = mods(slot_c.get(1), &loc);
            let tiers_c = pf.component(BANNER_TIERS).map_or(&crate::decode::value::EMPTY, W::leaf);
            let mut tiers: Vec<Vec<Map>> = sorted_items(tiers_c.get(1))
                .iter()
                .map(|(_, g)| mods(g.leaf().get(0), &loc))
                .filter(|t| !t.is_empty())
                .collect();
            let Some(name) = name.clone() else { continue };
            if fixed.is_empty() && tiers.is_empty() {
                continue;
            }
            let mut pve = Vec::new();
            for s in fixed.iter_mut().chain(tiers.iter_mut().flatten()) {
                pve.push(s.shift_remove("pve").is_some_and(|v| v.truthy()));
            }
            let pvp = !pve.iter().any(|p| *p);
            let fixed_j = J::List(fixed.into_iter().map(J::Dict).collect());
            let tiers_j = J::List(
                tiers.into_iter().map(|t| obj! {"stats" => J::List(t.into_iter().map(J::Dict).collect())}).collect(),
            );
            let key = dumps(&J::List(vec![J::Bool(pvp), fixed_j.clone(), tiers_j.clone()]), None);
            let profile = banners.entry(key).or_insert_with(|| {
                let mut m = Map::new();
                m.insert("pvp".into(), J::Bool(pvp));
                m.insert("stats".into(), fixed_j);
                m.insert("tiers".into(), tiers_j);
                m.insert("items".into(), J::List(Vec::new()));
                m
            });
            let items = profile.get_mut("items").and_then(J::as_list_mut).expect("items");
            let rj = J::from(rarity);
            if items.iter().all(|i| i.get("name") != Some(&J::from(name.as_str())) || i.get("rarity") != Some(&rj)) {
                let desc = ident.get(5).filter(|d| d.truthy());
                let description = match desc {
                    Some(W::Str(d)) => loc.get(&**d).map_or(J::Null, J::from),
                    Some(_) => J::Null,
                    None => J::Null,
                };
                items.push(
                    obj! {"name" => name.as_str(), "rarity" => rj, "prefab" => rel, "description" => description},
                );
            }
        }
    }
    let valid: IndexSet<String> =
        tree.files("blueprints/", ".blueprint").iter().map(|p| rp(p, "blueprints/").to_lowercase()).collect();
    let stations = stations(tree, &prefabs, &loc);
    let mut refs: IndexSet<String> = IndexSet::new();
    let item_of = |c: &J| c.get("item").and_then(J::as_str).map(str::to_owned);
    for r in &rarities {
        if let Some(J::Dict(ic)) = r.get("improve_cost") {
            for lst in ic.values() {
                refs.extend(lst.as_list().into_iter().flatten().filter_map(item_of));
            }
        }
        if let Some(J::List(rc)) = r.get("raise_cost") {
            refs.extend(rc.iter().filter_map(item_of));
        }
    }
    for st in &stations {
        for op in st.get("operations").and_then(J::as_list).into_iter().flatten() {
            if let Some(J::List(c)) = op.get("cost") {
                refs.extend(c.iter().filter_map(item_of));
            }
        }
        refs.insert(rs(st.str_or("prefab", ""), "_interactive").to_owned());
    }
    for b in banners.values() {
        for i in b.get("items").and_then(J::as_list).into_iter().flatten() {
            refs.extend(i.get("prefab").and_then(J::as_str).map(str::to_owned));
        }
    }
    let mut sorted_refs: Vec<String> = refs.into_iter().collect();
    sorted_refs.sort();
    let mut icons = Map::new();
    for r in sorted_refs {
        if let Some(bp) = model(tree, &prefabs, &r, &valid) {
            icons.insert(r, J::from(bp));
        }
    }
    let slots: Vec<J> = SLOTS
        .iter()
        .map(|(sid, key, label)| {
            let types: Vec<J> = ITEM_TYPES
                .iter()
                .filter(|t| t.1 == *sid)
                .map(|(_, _, tkey, tlabel)| obj! {"key" => *tkey, "name" => loc.get(*tlabel).cloned().unwrap_or_else(|| (*tkey).to_owned())})
                .collect();
            obj! {"key" => *key, "name" => loc.get(*label).cloned().unwrap_or_else(|| (*key).to_owned()), "item_types" => types}
        })
        .collect();
    let rindex = |r: Option<&str>| r.and_then(|r| RARITIES.iter().position(|x| *x == r)).map_or(-1, |i| i as i64);
    named.sort_by_cached_key(|n| {
        (
            rindex(n["rarity"].as_str()),
            n["slot"].as_str().unwrap_or("").to_owned(),
            n["name"].as_str().unwrap_or("").to_owned(),
        )
    });
    let best = |p: &Map| {
        p.get("items")
            .and_then(J::as_list)
            .into_iter()
            .flatten()
            .filter_map(|i| i.get("rarity").and_then(J::as_str))
            .map(|r| rindex(Some(r)))
            .max()
            .unwrap_or(-1)
    };
    let mut profiles: Vec<Map> = banners.into_values().collect();
    profiles.sort_by_cached_key(|b| {
        let first = b
            .get("items")
            .and_then(J::as_list)
            .and_then(|l| l.first())
            .map(|i| i.str_or("name", "").to_owned())
            .unwrap_or_default();
        (b.get("pvp") == Some(&J::Bool(true)), -best(b), first)
    });
    for p in profiles.iter_mut() {
        if let Some(items) = p.get_mut("items").and_then(J::as_list_mut) {
            items.sort_by_cached_key(|i| i.str_or("name", "").to_owned());
        }
    }
    let raise_to =
        raise_from.and_then(|r| RARITIES.iter().position(|x| *x == r)).and_then(|i| RARITIES.get(i + 1)).copied();
    Ok(obj! {
        "server_side" => vec![
            J::from("The value range of every rolled Hat, Face, Weapon and Ring stat line"),
            J::from("Which bonus stats a dropped or crafted item rolls, and how often"),
        ],
        "rarities" => rarities,
        "forge" => obj! {
            "star_step" => 0.2, "max_star" => 5, "max_lines" => 3, "max_boosts_per_line" => 2,
            "pearl_multipliers" => vec![J::Float(1.0), J::Float(1.1), J::Float(1.3)], "chaos_min_lines" => 3,
            "tome_max_reduction" => 10, "raise_from" => raise_from, "raise_to" => raise_to, "stations" => stations,
        },
        "power_rank" => obj! {"per_stat_boost" => 0.0375, "per_star" => 0.06},
        "slots" => slots,
        "named_items" => J::List(named.into_iter().map(J::Dict).collect()),
        "banners" => J::List(profiles.into_iter().map(J::Dict).collect()),
        "icons" => icons,
    })
}

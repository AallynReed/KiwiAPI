//! `mount_abilities.py`: every mount, its stats and the abilities it adds.

use super::ally_abilities::{abilities_text, detail};
use crate::decode::{
    ability::{Prefabs, identity, modifiers},
    common::{collection_members, locale_at, rs, stem, walk_values},
    json::{J, Map},
    registry::{Built, Ctx},
    value::{Prefab, W},
};
use crate::obj;

pub const MOUNT: i128 = 80;
const MOUNT_PASSIVES: i128 = 3;
const MOUNT_TRIGGERED: i128 = 4;
const MOUNT_STATS: i128 = 6;
const TRIGGER_ABILITY: i128 = 2;
const UNLOCK: i128 = 369;
const SHARED: &str = "abilities/equipment/mount_block_harvester";

pub fn ability_refs(mount: &W) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for r in mount.leaf().iter_of(MOUNT_PASSIVES) {
        if let W::Str(s) = r {
            out.push(s.to_string());
        }
    }
    for row in mount.leaf().iter_of(MOUNT_TRIGGERED) {
        if let Some(s) = row.leaf().str(TRIGGER_ABILITY) {
            out.push(s.to_owned());
        }
    }
    out.extend(mount.strings().iter().filter(|s| s.starts_with("abilities/")).map(|s| s.to_string()));
    let mut seen: Vec<String> = Vec::new();
    for r in out {
        let r = rs(&r, ".binfab").to_owned();
        if !seen.contains(&r) && r != SHARED {
            seen.push(r);
        }
    }
    seen
}

fn slot(v: Option<&W>) -> String {
    match v {
        None | Some(W::Int(0)) => "$EquipmentSlot_Mount".into(),
        Some(W::Int(1)) => "$EquipmentSlot_Cart".into(),
        Some(W::Int(2)) => "$EquipmentSlot_Wings".into(),
        Some(W::Int(3)) => "$EquipmentSlot_Boat".into(),
        Some(W::Int(i)) => format!("slot_{i}"),
        Some(W::Str(s)) => format!("slot_{s}"),
        Some(W::Float(f)) => format!("slot_{}", crate::pyfloat::repr(*f)),
        Some(_) => "slot_None".into(),
    }
}

pub fn stat_groups(pf: &Prefab, mount: &W) -> Vec<J> {
    let mut groups = Vec::new();
    if let Some(unlock) = pf.component(UNLOCK) {
        let stats = modifiers(&walk_values(unlock));
        if !stats.is_empty() {
            groups.push(obj! {"slot" => "unlock", "stats" => stats});
        }
    }
    for group in mount.leaf().iter_of(MOUNT_STATS) {
        let leaf = group.leaf();
        let stats = match leaf.get(1).filter(|v| v.truthy()) {
            Some(v) => modifiers(&walk_values(v)),
            None => Vec::new(),
        };
        if !stats.is_empty() {
            groups.push(obj! {"slot" => slot(leaf.get(0)), "stats" => stats});
        }
    }
    groups
}

pub fn build(ctx: &Ctx) -> Built {
    let tree = &ctx.tree;
    let prefabs = Prefabs::new(tree);
    let names = locale_at(tree, "languages/en/prefabs_collections_mount.binfab");
    let text = abilities_text(tree);
    let mut merged = locale_at(tree, "languages/en/prefabs_collections.binfab");
    merged.extend(names.clone());
    let owned = collection_members(prefabs.get("collections/collection_mount").as_deref(), &merged);
    let mut mounts: Vec<Map> = Vec::new();
    for path in tree.files("prefabs/collections/mount/", ".binfab") {
        if path.matches('/').count() != 3 {
            continue;
        }
        let slug = stem(&path);
        let Some(pf) = prefabs.get(&format!("collections/mount/{slug}")) else { continue };
        let Some(mount) = pf.component(MOUNT) else { continue };
        let ident = identity(Some(&pf));
        let Some(name) = names.get(ident.name()).filter(|n| !n.is_empty()).cloned() else { continue };
        let mut powers = Vec::new();
        for r in ability_refs(mount) {
            let Some(apf) = prefabs.get(&r) else { continue };
            let aid = identity(Some(&apf));
            let mut row = Map::new();
            row.insert("ref".into(), J::from(r.as_str()));
            let title = text.get(aid.name()).cloned().unwrap_or_default();
            let t = text.get(aid.desc()).cloned().unwrap_or_default();
            if !title.is_empty() {
                row.insert("name".into(), J::from(title.as_str()));
            }
            if !t.is_empty() {
                row.insert("text".into(), J::from(t.as_str()));
            }
            let d = detail(&prefabs, &r, &text);
            if !d.is_empty() || !title.is_empty() || !t.is_empty() {
                row.extend(d);
                powers.push(J::Dict(row));
            }
        }
        let mut entry = Map::new();
        entry.insert("slug".into(), J::from(slug.as_str()));
        entry.insert("name".into(), J::from(name));
        entry.insert("description".into(), J::from(names.get(ident.desc()).cloned().unwrap_or_default()));
        entry.insert("prefab".into(), J::from(format!("collections/mount/{slug}")));
        entry.insert("stats".into(), J::List(stat_groups(&pf, mount)));
        entry.insert("abilities".into(), J::List(powers));
        if let Some((gid, label)) = owned.get(&format!("collections/mount/{slug}")) {
            if !gid.is_empty() {
                entry.insert("group_id".into(), J::from(gid.as_str()));
            }
            if !label.is_empty() {
                entry.insert("group".into(), J::from(label.as_str()));
            }
        }
        mounts.push(entry);
    }
    mounts.sort_by_cached_key(|m| m["name"].as_str().unwrap_or("").to_lowercase());
    Ok(J::List(mounts.into_iter().map(J::Dict).collect()))
}

//! `class_abilities.py`: each class's abilities (cost, cooldown, damage,
//! healing, effects), with the curated `icon`/`type` carried over.

use std::sync::OnceLock;

use indexmap::IndexMap;
use regex::Regex;

use crate::decode::{
    ability::{Prefabs, describe, identity, refs, walk},
    common::{Locale, locale_at, stem},
    json::{J, Map},
    registry::{Built, Ctx},
    value::strings_of,
};

const OWNED: [&str; 3] = ["stages", "healing", "effects"];

fn resolve(aloc: &Locale, key: &str) -> (Option<String>, String) {
    for suffix in ["_item_name", "_name", ""] {
        if let Some(name) = aloc.get(&format!("{key}{suffix}")).filter(|n| !n.is_empty()) {
            let desc = if suffix == "_item_name" { "_item_description" } else { "_description" };
            return (Some(name.clone()), aloc.get(&format!("{key}{desc}")).cloned().unwrap_or_default());
        }
    }
    (None, String::new())
}

fn label(aloc: &Locale, folder: &str, rel: &str, prefabs: &Prefabs) -> (Option<String>, String) {
    let pf = prefabs.get(rel);
    let ident = identity(pf.as_deref());
    if let Some(k) = &ident.name_key
        && let Some(name) = aloc.get(k)
    {
        return (Some(name.clone()), aloc.get(ident.desc()).cloned().unwrap_or_default());
    }
    let texts = pf.as_ref().map(|p| strings_of(p.components.iter().map(|(_, o)| o))).unwrap_or_default();
    for text in texts {
        if !text.starts_with("$prefabs_abilities") || text.ends_with("_description") {
            continue;
        }
        let mut base: &str = &text;
        for suffix in ["_item_name", "_name"] {
            if let Some(b) = base.strip_suffix(suffix) {
                base = b;
                break;
            }
        }
        let (name, desc) = resolve(aloc, base);
        if name.is_some() {
            return (name, desc);
        }
    }
    resolve(aloc, &format!("$prefabs_abilities_{folder}_{}", rel.rsplit('/').next().unwrap_or(rel)))
}

fn rows_mut<'a>(ability: &'a mut Map, key: &str) -> &'a mut Vec<J> {
    if !matches!(ability.get(key), Some(J::List(_))) {
        ability.insert(key.into(), J::List(Vec::new()));
    }
    ability.get_mut(key).and_then(J::as_list_mut).expect("list")
}

fn depth_of(row: &J) -> i128 {
    row.get("_depth").and_then(J::as_int).unwrap_or(0)
}

fn assign_owners(abilities: &mut [Map]) {
    for key in OWNED {
        let mut best: IndexMap<String, (i128, usize)> = IndexMap::new();
        for (index, ability) in abilities.iter().enumerate() {
            for row in ability.get(key).and_then(J::as_list).into_iter().flatten() {
                let rank = (depth_of(row), index);
                let prefab = row.str_or("prefab", "").to_owned();
                if best.get(&prefab).is_none_or(|b| rank < *b) {
                    best.insert(prefab, rank);
                }
            }
        }
        for (index, ability) in abilities.iter_mut().enumerate() {
            let rows = ability.get(key).and_then(J::as_list).cloned().unwrap_or_default();
            let kept: Vec<J> = rows
                .into_iter()
                .filter(|r| best.get(r.str_or("prefab", "")) == Some(&(depth_of(r), index)))
                .map(|mut r| {
                    if let Some(m) = r.as_dict_mut() {
                        m.shift_remove("_depth");
                    }
                    r
                })
                .collect();
            ability.insert(key.into(), J::List(kept));
        }
    }
}

fn icon_token(icon: &str, folder: &str) -> String {
    icon.split('_')
        .filter(|p| !p.is_empty())
        .filter(|p| !matches!(*p, "ico" | "icon") && *p != folder && !p.chars().all(|c| c.is_ascii_digit()))
        .collect()
}

fn non_alnum() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"[^a-z0-9]").expect("regex"))
}

fn get_or<'a>(m: &'a J, key: &str, default: &'a J) -> &'a J {
    m.get(key).unwrap_or(default)
}

fn overlay_curated(mut abilities: Vec<Map>, curated: &[J], folder: &str) -> Vec<Map> {
    let empty = J::Str(String::new());
    let mut by_name: IndexMap<String, &J> = IndexMap::new();
    for a in curated {
        by_name.insert(a.str_or("name", "").to_owned(), a);
    }
    let mut claimed: Vec<String> = Vec::new();
    for ability in abilities.iter_mut() {
        let name = ability.get("name").and_then(J::as_str).unwrap_or("").to_owned();
        if let Some(hit) = by_name.get(&name).filter(|h| h.truthy()) {
            claimed.push(hit.str_or("name", "").to_owned());
            ability.insert("icon".into(), get_or(hit, "icon", &empty).clone());
            ability.insert("type".into(), get_or(hit, "type", &empty).clone());
        }
    }
    for ability in abilities.iter_mut() {
        if ability.get("type").is_some_and(J::truthy) {
            continue;
        }
        let prefab = ability.get("prefab").and_then(J::as_str).unwrap_or("");
        let token =
            non_alnum().replace_all(&prefab.rsplit('/').next().unwrap_or(prefab).to_lowercase(), "").into_owned();
        let fits: Vec<&J> = curated
            .iter()
            .filter(|a| !claimed.iter().any(|c| c == a.str_or("name", "")))
            .filter(|a| {
                let tok = icon_token(a.str_or("icon", ""), folder);
                !tok.is_empty() && token.contains(&tok)
            })
            .collect();
        if fits.len() == 1 {
            ability.insert("icon".into(), get_or(fits[0], "icon", &empty).clone());
            ability.insert("type".into(), get_or(fits[0], "type", &empty).clone());
            ability.insert("matched_by".into(), J::from("icon"));
        }
    }
    for a in curated {
        let name = a.str_or("name", "");
        let dup = abilities.iter().any(|x| x.get("icon") == a.get("icon") && x.get("type") == a.get("type"));
        if claimed.iter().any(|c| c == name) || dup {
            continue;
        }
        let mut row = Map::new();
        row.insert("name".into(), a.get("name").cloned().unwrap_or(J::Null));
        row.insert("description".into(), J::from(""));
        row.insert("icon".into(), get_or(a, "icon", &empty).clone());
        row.insert("type".into(), get_or(a, "type", &empty).clone());
        row.insert("stages".into(), a.get("stages").cloned().unwrap_or(J::List(Vec::new())));
        row.insert("active".into(), J::Bool(false));
        abilities.push(row);
    }
    abilities
}

#[allow(clippy::too_many_arguments)]
fn ability(
    prefabs: &Prefabs,
    aloc: &Locale,
    rel: &str,
    name: &str,
    desc: &str,
    stop: &[String],
    prefix: &str,
    active: bool,
) -> Map {
    let stop: Vec<String> = stop.iter().filter(|s| *s != rel).cloned().collect();
    let info = describe(prefabs, rel, &stop, &[prefix]);
    let mut out = Map::new();
    out.insert("name".into(), J::from(name));
    out.insert("description".into(), J::from(desc));
    out.insert("prefab".into(), J::from(rel));
    out.insert("icon".into(), J::from(""));
    out.insert("type".into(), J::from(""));
    out.insert("active".into(), J::Bool(active));
    for key in ["energy", "cooldown"] {
        if let Some(v) = info.get(key) {
            out.insert(key.into(), v.clone());
        }
    }
    for key in ["stages", "healing", "effects", "vfx"] {
        out.insert(key.into(), info.get(key).cloned().unwrap_or(J::List(Vec::new())));
    }
    for eff in rows_mut(&mut out, "effects").iter_mut() {
        let Some(m) = eff.as_dict_mut() else { continue };
        let name_key = m.shift_remove("name_key").and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default();
        let desc_key =
            m.shift_remove("description_key").and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default();
        if let Some(t) = aloc.get(&name_key).filter(|t| !t.is_empty()) {
            m.insert("title".into(), J::from(t.as_str()));
        }
        if let Some(t) = aloc.get(&desc_key).filter(|t| !t.is_empty()) {
            m.insert("description".into(), J::from(t.as_str()));
        }
    }
    out
}

fn merge(into: &mut Map, extra: &Map) {
    for key in ["stages", "healing", "effects", "vfx"] {
        for row in extra.get(key).and_then(J::as_list).cloned().unwrap_or_default() {
            let list = rows_mut(into, key);
            if !list.contains(&row) {
                list.push(row);
            }
        }
    }
    if !into.get("description").is_some_and(J::truthy) {
        into.insert("description".into(), extra.get("description").cloned().unwrap_or(J::Null));
    }
    for key in ["energy", "cooldown"] {
        if !into.contains_key(key)
            && let Some(v) = extra.get(key)
        {
            into.insert(key.into(), v.clone());
        }
    }
}

fn afolder_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"^abilities/([a-z0-9_]+)/").expect("regex"))
}

pub fn build(ctx: &Ctx) -> Built {
    let tree = &ctx.tree;
    let prefabs = Prefabs::new(tree);
    let baseline = ctx.baseline("class_abilities.json")?;
    let mut curated_by_class: IndexMap<String, Vec<J>> = IndexMap::new();
    for c in baseline.as_list().into_iter().flatten() {
        let abilities = c.get("abilities").and_then(J::as_list).cloned().unwrap_or_default();
        curated_by_class.insert(c.str_or("name", "").to_owned(), abilities);
    }
    let mut display = Locale::new();
    for name in ["prefabs_class.binfab", "ui.binfab", "new.binfab"] {
        display.extend(locale_at(tree, &format!("languages/en/{name}")));
    }
    let mut out = Vec::new();
    for class_path in tree.files("prefabs/class/", ".binfab") {
        let folder = stem(&class_path);
        if class_path.matches('/').count() != 2 || folder.ends_with("_ultimate") {
            continue;
        }
        let cls = prefabs.get(&format!("class/{folder}"));
        let texts = cls.as_ref().and_then(|c| c.root.as_ref()).map(|r| r.strings()).unwrap_or_default();
        let Some(key) = texts.iter().find(|s| s.starts_with("$DisplayName")).map(|s| s.to_string()) else { continue };
        let afolder = texts
            .iter()
            .find_map(|s| afolder_re().captures(s).map(|m| m[1].to_owned()))
            .unwrap_or_else(|| folder.clone());
        let prefix = format!("abilities/{afolder}/");
        let aloc = locale_at(tree, &format!("languages/en/prefabs_abilities_{afolder}.binfab"));

        let class_refs: Vec<String> = cls
            .as_ref()
            .map(|c| refs(c, &[&prefix]))
            .unwrap_or_default()
            .into_iter()
            .filter(|r| prefabs.get(r).is_some())
            .collect();
        let mut reachable: Vec<String> = Vec::new();
        for root in &class_refs {
            for (rel, _, _) in walk(&prefabs, root, &[], 400, &[&prefix]) {
                if !reachable.contains(&rel) {
                    reachable.push(rel);
                }
            }
        }
        let mut ordered = class_refs.clone();
        ordered.extend(reachable.iter().filter(|r| !class_refs.contains(r)).cloned());
        let entries: Vec<(String, String, String)> = ordered
            .iter()
            .filter_map(|rel| {
                let (name, desc) = label(&aloc, &afolder, rel, &prefabs);
                name.filter(|n| !n.is_empty()).map(|n| (rel.clone(), n, desc))
            })
            .collect();
        let mut boundaries: Vec<String> = class_refs.clone();
        for (rel, _, _) in &entries {
            if !boundaries.contains(rel) {
                boundaries.push(rel.clone());
            }
        }

        let mut abilities: Vec<Map> = Vec::new();
        let mut by_name: IndexMap<String, usize> = IndexMap::new();
        for (rel, name, desc) in &entries {
            let a = ability(&prefabs, &aloc, rel, name, desc, &boundaries, &prefix, true);
            if let Some(&i) = by_name.get(name) {
                merge(&mut abilities[i], &a);
                continue;
            }
            by_name.insert(name.clone(), abilities.len());
            abilities.push(a);
        }
        for path in tree.files(&format!("prefabs/{prefix}"), ".binfab") {
            let rel = &path["prefabs/".len()..path.len() - ".binfab".len()];
            if reachable.iter().any(|r| r == rel) {
                continue;
            }
            let (name, desc) = label(&aloc, &afolder, rel, &prefabs);
            let Some(name) = name.filter(|n| !n.is_empty()) else { continue };
            if by_name.contains_key(&name) {
                continue;
            }
            by_name.insert(name.clone(), abilities.len());
            abilities.push(ability(&prefabs, &aloc, rel, &name, &desc, &boundaries, &prefix, false));
        }

        let cname = display.get(&key).cloned().unwrap_or_else(|| key.replace("$DisplayName_", ""));
        assign_owners(&mut abilities);
        let curated = curated_by_class.get(&cname).cloned().unwrap_or_default();
        let abilities = overlay_curated(abilities, &curated, &folder);
        let mut entry = Map::new();
        entry.insert("name".into(), J::from(cname));
        entry.insert("game_folder".into(), J::from(folder));
        entry.insert("abilities".into(), J::List(abilities.into_iter().map(J::Dict).collect()));
        out.push(J::Dict(entry));
    }
    Ok(J::List(out))
}

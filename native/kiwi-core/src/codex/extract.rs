//! `app/trove/codexes/extract.py` plus the indexer's per-prefab parse
//! (`_Maps`, `_parse_entry`): one prefab -> its entry row and child rows.

use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    sync::LazyLock,
};

use regex::bytes::Regex;
use serde_json::{Map, Value, json};

use super::{
    PyResult, abilities, bonuses, geode, int_value,
    links::{self, Link},
    localize::{self, Loc},
    loot::StyleRow,
    mastery::{self, Multipliers},
    models::{AbilityRow, StatRow, ability_rows, stat_rows},
    powerrank::{self, PowerRankTable},
    py,
    recipe::{self, ItemMeta},
    styles,
    types::PREFABS_ROOT,
    wire,
};

const BONUS_TYPES: [&str; 12] =
    ["ally", "mount", "badge", "wings", "aura", "boat", "sail", "flask", "tome", "magrider", "fishingpole", "skin"];

static PATH_RE: LazyLock<Regex> = LazyLock::new(|| py::bytes_re(r"[A-Za-z0-9_][A-Za-z0-9_\-/\[\].]*"));
static BLUEPRINT_REF_RE: LazyLock<Regex> =
    LazyLock::new(|| py::bytes_re(r"[A-Za-z0-9_][A-Za-z0-9_\-/\[\].]*\.blueprint"));
static CREDITED_RE: LazyLock<Regex> =
    LazyLock::new(|| py::bytes_re(r"[A-Za-z0-9_][A-Za-z0-9_\-/.]*\[[^\[\]\x00-\x1f]{1,80}\](?:\.blueprint)?"));

/// Reads archived blobs by content sha.
pub trait Blobs {
    fn get(&self, sha: &str) -> Option<Vec<u8>>;
}

fn runs(content: &[u8]) -> Vec<&[u8]> {
    let mut hits: Vec<(usize, std::cmp::Reverse<usize>, &[u8])> = Vec::new();
    for rx in [&*PATH_RE, &*CREDITED_RE] {
        for m in rx.find_iter(content) {
            hits.push((m.start(), std::cmp::Reverse(m.len()), m.as_bytes()));
        }
    }
    hits.sort();
    hits.into_iter().map(|h| h.2).collect()
}

fn unprefixed(run: &[u8]) -> Vec<String> {
    let s = py::ascii_ignore(run);
    let chars: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    if !chars.is_empty() && chars[0] as usize == chars.len() - 1 {
        out.push(chars[1..].iter().collect());
    }
    for i in 0..chars.len().min(5) {
        out.push(chars[i..].iter().collect());
    }
    out
}

fn is_ui_model(name: &str) -> bool {
    let stem = py::last_part(name, "/");
    let stem = stem.strip_suffix(".blueprint").unwrap_or(stem);
    let stem = stem.split_once('[').map_or(stem, |(a, _)| a);
    py::lower(stem).ends_with("_ui")
}

/// `blueprint_ref`.
pub fn blueprint_ref(
    content: &[u8],
    valid: &HashSet<String>,
    path: &str,
    model_size: Option<&dyn Fn(&str) -> usize>,
) -> Option<String> {
    let p = py::lower(path);
    if p.contains("item/skin") || (p.contains("item/unlocker") && py::contains(content, b"equipment_")) {
        return None;
    }
    if valid.is_empty() {
        return BLUEPRINT_REF_RE.find(content).map(|m| py::ascii_ignore(m.as_bytes()));
    }
    let mut candidates: Vec<String> = Vec::new();
    for run in runs(content) {
        for cand in unprefixed(run) {
            let cand_bp = if cand.ends_with(".blueprint") { cand } else { format!("{cand}.blueprint") };
            if !valid.contains(&py::lower(&cand_bp)) {
                continue;
            }
            if !candidates.contains(&cand_bp) {
                candidates.push(cand_bp);
            }
            break;
        }
    }
    if candidates.is_empty() {
        return None;
    }
    let ui: Vec<&String> = candidates.iter().filter(|c| is_ui_model(c)).collect();
    let Some(size) = model_size else {
        return Some(ui.first().copied().unwrap_or(&candidates[0]).clone());
    };
    if let Some(c) = ui.iter().find(|c| size(c) > 0) {
        return Some((*c).clone());
    }
    if size(&candidates[0]) > 0 {
        return Some(candidates[0].clone());
    }
    let mut biggest = &candidates[0];
    let mut best = size(biggest);
    for c in &candidates[1..] {
        let s = size(c);
        if s > best {
            biggest = c;
            best = s;
        }
    }
    Some(if best > 0 { biggest.clone() } else { candidates[0].clone() })
}

/// `blueprint_stems`: basename without its `[author]` credit -> blueprints.
pub fn blueprint_stems(valid: &HashSet<String>) -> HashMap<String, Vec<String>> {
    let mut stems: HashMap<String, Vec<String>> = HashMap::new();
    for bp in valid {
        let base = py::last_part(bp, "/");
        let base = base.strip_suffix(".blueprint").unwrap_or(base);
        let base = base.split_once('[').map_or(base, |(a, _)| a);
        stems.entry(base.to_owned()).or_default().push(bp.clone());
    }
    stems
}

/// `model_blueprint`.
pub fn model_blueprint(model: &str, valid: &HashSet<String>, stems: &HashMap<String, Vec<String>>) -> Option<String> {
    let m = py::lower(&model.replace('\\', "/"));
    let m = m.strip_suffix(".blueprint").unwrap_or(&m);
    let full = format!("{m}.blueprint");
    if valid.contains(&full) {
        return Some(full);
    }
    match stems.get(m).map(Vec::as_slice) {
        Some([only]) => Some(only.clone()),
        _ => None,
    }
}

/// `conventional_blueprint`.
pub fn conventional_blueprint(path: &str, stems: &HashMap<String, Vec<String>>) -> Option<String> {
    let p = path.replace('\\', "/");
    let p = py::lower(p.strip_suffix(".binfab").unwrap_or(&p));
    let parts: Vec<&str> = p.split('/').collect();
    if parts.len() < 3 || parts[0] != "placeable" {
        return None;
    }
    let prefix = match parts[1] {
        "deco" => "deco_",
        "frameworks" => "fw_",
        _ => return None,
    };
    match stems.get(&format!("{prefix}{}", parts[parts.len() - 1])).map(Vec::as_slice) {
        Some([only]) => Some(only.clone()),
        _ => None,
    }
}

fn name_from_path(path: &str) -> String {
    let stem = py::last_part(path, "/");
    let stem = stem.strip_suffix(".binfab").unwrap_or(stem);
    let name = py::title(py::strip(&stem.replace('_', " ")));
    if name.is_empty() { stem.to_owned() } else { name }
}

/// The voxel count `len(decode_render_map(raw))`, 0 for empty or undecodable.
pub fn model_voxel_count(raw: &[u8]) -> usize {
    use crate::blueprint;
    let Some(version) = blueprint::version(raw) else { return 0 };
    let payload = match version {
        3 | 4 => {
            if !matches!(bp_uleb(raw, 9), Some(c) if c != 0) {
                return 0;
            }
            raw.to_vec()
        }
        5 => {
            let Some(body) = crate::zlib::inflate(&raw[9..], usize::MAX).map(|i| i.data) else { return 0 };
            if body.len() < 28 || i32::from_le_bytes([body[24], body[25], body[26], body[27]]) <= 0 {
                return 0;
            }
            body
        }
        _ => return 0,
    };
    blueprint::decode_any(version, &payload).map_or(0, |d| blueprint::render_cells(&d.voxels).len())
}

/// `codec.read_uleb128`: masked to 32 bits, at most 10 bytes.
fn bp_uleb(data: &[u8], mut pos: usize) -> Option<u32> {
    let mut result: u128 = 0;
    let mut shift = 0u32;
    loop {
        let byte = *data.get(pos)?;
        pos += 1;
        result |= u128::from(byte & 0x7F) << shift;
        if byte & 0x80 == 0 {
            return Some(result as u32);
        }
        shift += 7;
        if shift >= 64 {
            return None;
        }
    }
}

/// `_Maps`: the lookup tables one index pass reads, with its memo caches.
pub struct Maps<S> {
    pub loc: Loc,
    pub mount_categories: HashMap<String, String>,
    pub multipliers: Multipliers,
    pub geode_multipliers: Multipliers,
    pub geode_members: HashMap<String, String>,
    pub upgrade_trees: HashMap<String, Vec<u8>>,
    pub recipe_catalogue: HashMap<String, i64>,
    pub recipe_providers: HashMap<String, Vec<Value>>,
    pub store: Option<S>,
    pub prefab_shas: HashMap<String, String>,
    pub valid_blueprints: HashSet<String>,
    pub blueprint_stems: HashMap<String, Vec<String>>,
    pub blueprint_shas: HashMap<String, String>,
    pub block_models: HashMap<String, String>,
    pub power_rank_table: PowerRankTable,
    pub style_rows: HashMap<String, StyleRow>,
    item_meta: RefCell<HashMap<String, ItemMeta>>,
    ability_meta: RefCell<HashMap<String, Option<abilities::AbilityMeta>>>,
    model_size: RefCell<HashMap<String, usize>>,
}

impl<S> Default for Maps<S> {
    fn default() -> Self {
        Maps {
            loc: Loc::new(),
            mount_categories: HashMap::new(),
            multipliers: Multipliers::new(),
            geode_multipliers: Multipliers::new(),
            geode_members: HashMap::new(),
            upgrade_trees: HashMap::new(),
            recipe_catalogue: HashMap::new(),
            recipe_providers: HashMap::new(),
            store: None,
            prefab_shas: HashMap::new(),
            valid_blueprints: HashSet::new(),
            blueprint_stems: HashMap::new(),
            blueprint_shas: HashMap::new(),
            block_models: HashMap::new(),
            power_rank_table: PowerRankTable::new(),
            style_rows: HashMap::new(),
            item_meta: RefCell::default(),
            ability_meta: RefCell::default(),
            model_size: RefCell::default(),
        }
    }
}

fn full_prefab(rel: &str) -> String {
    let key = rel.replace('\\', "/");
    let key = key.strip_suffix(".binfab").unwrap_or(&key);
    if key.starts_with(PREFABS_ROOT) { format!("{key}.binfab") } else { format!("{PREFABS_ROOT}{key}.binfab") }
}

/// One parsed prefab: the `codex_entry` columns plus its child rows.
#[derive(Clone, Debug, PartialEq)]
pub struct Parsed {
    pub path: String,
    pub codex_type: String,
    pub name: String,
    pub category: String,
    pub description: String,
    pub tradable: Option<bool>,
    pub mastery: Option<i128>,
    pub mastery_geode: Option<i128>,
    pub power_rank: Option<i128>,
    pub name_key: Option<String>,
    pub desc_key: Option<String>,
    pub blueprint: Option<String>,
    pub data: Map<String, Value>,
    pub stats: Vec<StatRow>,
    pub abilities: Vec<AbilityRow>,
    pub links: Vec<Link>,
}

impl<S: Blobs> Maps<S> {
    fn read(&self, rel: &str) -> Option<Vec<u8>> {
        let store = self.store.as_ref()?;
        let sha = self.prefab_shas.get(&full_prefab(rel))?;
        store.get(sha)
    }

    pub fn prefab_exists(&self, rel: &str) -> bool {
        self.prefab_shas.contains_key(&full_prefab(rel))
    }

    pub fn ability_meta(&self, r: &str) -> Option<abilities::AbilityMeta> {
        let norm = abilities::normalize_ref(r);
        if norm.is_empty() {
            return None;
        }
        if let Some(hit) = self.ability_meta.borrow().get(&norm) {
            return hit.clone();
        }
        let meta = self
            .read(&norm)
            .filter(|c| !c.is_empty())
            .map(|c| abilities::display(abilities::extract_keys(&c), &self.loc));
        self.ability_meta.borrow_mut().insert(norm, meta.clone());
        meta
    }

    pub fn model_size(&self, name: &str) -> usize {
        let key = py::lower(&name.replace('\\', "/"));
        let key = key.strip_prefix("blueprints/").unwrap_or(&key).to_owned();
        if let Some(&hit) = self.model_size.borrow().get(&key) {
            return hit;
        }
        let raw = match (self.blueprint_shas.get(&key), &self.store) {
            (Some(sha), Some(store)) => store.get(sha),
            _ => None,
        };
        let size = raw.filter(|r| !r.is_empty()).map_or(0, |r| model_voxel_count(&r));
        self.model_size.borrow_mut().insert(key, size);
        size
    }

    pub fn item_meta(&self, rel: &str) -> PyResult<ItemMeta> {
        let norm = py::lower(rel.replace('\\', "/").strip_suffix(".binfab").unwrap_or(&rel.replace('\\', "/")));
        if let Some(hit) = self.item_meta.borrow().get(&norm) {
            return Ok(hit.clone());
        }
        let mut meta = ItemMeta::default();
        if let Some(content) = self.read(&norm).filter(|c| !c.is_empty()) {
            let ident = wire::decode_identity(&content)?.unwrap_or_default();
            meta.name = self.loc.get(ident.name_key.as_deref().unwrap_or("")).cloned().unwrap_or_default();
            meta.desc = self.loc.get(ident.desc_key.as_deref().unwrap_or("")).cloned().unwrap_or_default();
            let size = |n: &str| self.model_size(n);
            meta.blueprint = blueprint_ref(&content, &self.valid_blueprints, &norm, Some(&size));
        }
        if meta.blueprint.is_none() {
            if let Some(model) = self.block_models.get(&norm) {
                meta.blueprint = model_blueprint(model, &self.valid_blueprints, &self.blueprint_stems);
            }
            if meta.blueprint.is_none() {
                meta.blueprint = conventional_blueprint(&norm, &self.blueprint_stems);
            }
        }
        self.item_meta.borrow_mut().insert(norm, meta.clone());
        Ok(meta)
    }

    fn enrich(&self, data: &mut Map<String, Value>) {
        if let Some(Value::Array(stats)) = data.get_mut("stats") {
            for s in stats.iter_mut().filter_map(Value::as_object_mut) {
                let key = s.get("stat").and_then(Value::as_str).unwrap_or("").to_owned();
                s.insert("stat_name".into(), json!(localize::resolve_stat_name(&self.loc, &key)));
                if let Some(slot) = s.get("slot").and_then(Value::as_str).filter(|v| !v.is_empty()).map(str::to_owned) {
                    s.insert("slot_name".into(), json!(localize::resolve_slot_name(&self.loc, Some(&slot))));
                }
            }
        }
        if let Some(Value::Array(list)) = data.get_mut("abilities") {
            for a in list.iter_mut().filter_map(Value::as_object_mut) {
                if a.get("hidden").and_then(Value::as_bool).unwrap_or(false) {
                    continue;
                }
                let r = a.get("ref").and_then(Value::as_str).unwrap_or("").to_owned();
                let resolved = if r.is_empty() { None } else { self.ability_meta(&r) };
                if let Some(m) = resolved {
                    if !m.name.is_empty() {
                        a.insert("name".into(), json!(m.name));
                    }
                    if !m.desc_key.is_empty() {
                        a.insert("key".into(), json!(m.desc_key));
                    }
                    if !m.description.is_empty() {
                        a.insert("description".into(), json!(m.description));
                    }
                }
                if a.get("description").and_then(Value::as_str).is_none_or(str::is_empty) {
                    let key = a.get("key").and_then(Value::as_str).map(str::to_owned);
                    let text = localize::resolve_text(&self.loc, key.as_deref());
                    if !text.is_empty() {
                        a.insert("description".into(), json!(text));
                    }
                }
            }
        }
    }

    fn recipe_entry(&self, path: &str, content: &[u8]) -> PyResult<Parsed> {
        let mut resolve = |p: &str| self.item_meta(p);
        let exists = |p: &str| self.prefab_exists(p);
        let parsed = recipe::parse_recipe(content, Some(&mut resolve), &exists)?;
        let size = |n: &str| self.model_size(n);
        let mut blueprint = blueprint_ref(content, &self.valid_blueprints, path, Some(&size));
        if blueprint.is_none()
            && let Some(out_path) = parsed.output.as_ref().and_then(|o| o.get("path")).and_then(Value::as_str)
            && !out_path.is_empty()
        {
            blueprint = self.item_meta(out_path)?.blueprint;
        }
        let mut rdata = Map::new();
        rdata.insert("output".into(), parsed.output.clone().map_or(Value::Null, Value::Object));
        rdata.insert("ingredients".into(), Value::Array(parsed.ingredients));
        rdata.insert("requirements".into(), json!(parsed.requirements));
        let mut data = Map::new();
        data.insert("recipe".into(), Value::Object(rdata));
        Ok(Parsed {
            path: path.to_owned(),
            codex_type: "recipe".into(),
            name: if parsed.name.is_empty() { name_from_path(path) } else { parsed.name },
            category: parsed.category.to_owned(),
            description: parsed.description,
            tradable: None,
            mastery: None,
            mastery_geode: None,
            power_rank: None,
            name_key: None,
            desc_key: None,
            blueprint,
            data,
            stats: Vec::new(),
            abilities: Vec::new(),
            links: Vec::new(),
        })
    }

    /// `extract_entry`.
    pub fn extract_entry(&self, codex_type: &str, path: &str, content: &[u8]) -> PyResult<Parsed> {
        if codex_type == "recipe" {
            return self.recipe_entry(path, content);
        }
        let ident = wire::decode_identity(content)?.unwrap_or_default();
        let mut name_key = ident.name_key.clone();
        let desc_key = ident.desc_key.clone();
        let look = |k: &Option<String>| {
            k.as_ref().filter(|k| !k.is_empty()).and_then(|k| self.loc.get(k)).filter(|v| !v.is_empty()).cloned()
        };
        let mut name = look(&name_key).unwrap_or_else(|| name_from_path(path));
        let description = look(&desc_key).unwrap_or_default();

        let mut data = Map::new();
        let mut power_rank = None;
        if BONUS_TYPES.contains(&codex_type) {
            let stats = bonuses::extract_stat_bonuses(content);
            let abilities = bonuses::extract_abilities(content);
            if !stats.is_empty() {
                data.insert("stats".into(), Value::Array(stats.iter().map(|s| Value::Object(s.to_value())).collect()));
            }
            if !abilities.is_empty() {
                data.insert("abilities".into(), Value::Array(abilities.into_iter().map(Value::Object).collect()));
            }
            self.enrich(&mut data);
            power_rank = powerrank::decode_power_rank(content, &self.power_rank_table);
        }
        let mut category = ident.category.clone().unwrap_or_default();

        if codex_type == "style" {
            let rel = match path.strip_prefix(PREFABS_ROOT) {
                Some(r) => r.strip_suffix(".binfab").unwrap_or(r),
                None => path.strip_suffix(".binfab").unwrap_or(path),
            };
            let row = self.style_rows.get(&styles::equipment_id(rel));
            let family = row.map(|r| r.family).filter(|f| !f.is_empty()).unwrap_or_else(|| styles::style_family(rel));
            let mut style = styles::style_identity(rel);
            style.insert("family".into(), json!(family));
            if let Some(row) = row {
                style.insert("catalogue".into(), json!(row.source));
                style.insert("raw_category".into(), row.raw_category.map_or(Value::Null, Value::from));
                if let Some(b) = row.base_mastery {
                    style.insert("base_mastery".into(), json!(b));
                }
                if !row.name_key.is_empty() && name_key.as_deref().is_none_or(str::is_empty) {
                    name_key = Some(row.name_key.clone());
                    if let Some(v) = self.loc.get(&row.name_key).filter(|v| !v.is_empty()) {
                        name = v.clone();
                    }
                }
            }
            data.insert("style".into(), Value::Object(style));
            if !family.is_empty() {
                category = family.to_owned();
            }
        }

        let size = |n: &str| self.model_size(n);
        Ok(Parsed {
            path: path.to_owned(),
            codex_type: codex_type.to_owned(),
            name,
            category,
            description,
            tradable: ident.tradable,
            mastery: None,
            mastery_geode: None,
            power_rank,
            name_key,
            desc_key,
            blueprint: blueprint_ref(content, &self.valid_blueprints, path, Some(&size)),
            data,
            stats: Vec::new(),
            abilities: Vec::new(),
            links: Vec::new(),
        })
    }

    fn attach_geode_companion(&self, entry: &mut Parsed, rel: &str, content: &[u8]) {
        let tree_ref = geode::find_upgrade_tree_ref(content);
        let rarity = self.geode_members.get(&py::lower(rel)).cloned();
        if tree_ref.is_none() && rarity.as_deref().is_none_or(str::is_empty) {
            return;
        }
        let levels = tree_ref
            .as_ref()
            .and_then(|r| self.upgrade_trees.get(r))
            .map_or_else(Vec::new, |t| geode::parse_upgrade_tree(t));
        let levels: Vec<Value> = levels
            .into_iter()
            .map(|mut l| {
                for s in &mut l.stats {
                    let key = s.get("stat").and_then(Value::as_str).unwrap_or("").to_owned();
                    s.insert("stat_name".into(), json!(localize::resolve_stat_name(&self.loc, &key)));
                }
                for a in &mut l.abilities {
                    let key = a.get("key").and_then(Value::as_str).map(str::to_owned);
                    let text = localize::resolve_text(&self.loc, key.as_deref());
                    if !text.is_empty() {
                        a.insert("description".into(), json!(text));
                    }
                }
                json!({
                    "level": l.level,
                    "stats": l.stats.into_iter().map(Value::Object).collect::<Vec<_>>(),
                    "abilities": l.abilities.into_iter().map(Value::Object).collect::<Vec<_>>(),
                })
            })
            .collect();
        let mut gc = Map::new();
        gc.insert("upgrade_tree".into(), tree_ref.map_or(Value::Null, Value::from));
        gc.insert("rarity".into(), rarity.map_or(Value::Null, Value::from));
        gc.insert("levels".into(), Value::Array(levels));
        entry.data.insert("geode_companion".into(), Value::Object(gc));
    }

    /// `_parse_entry` once the blob is read: entry, child rows and links.
    pub fn parse_entry(&self, path: &str, ctype: &str, content: &[u8]) -> PyResult<Parsed> {
        let mut entry = self.extract_entry(ctype, path, content)?;
        let rel = path.strip_prefix(PREFABS_ROOT).unwrap_or(path);
        let rel = rel.strip_suffix(".binfab").unwrap_or(rel);
        if ctype == "mount" {
            let category = self
                .mount_categories
                .get(&py::lower(rel))
                .filter(|c| !c.is_empty())
                .cloned()
                .unwrap_or(entry.category.clone());
            if py::lower(&category).contains("dragon") {
                entry.codex_type = "dragon".into();
            }
            entry.category = category;
        }
        if ctype == "recipe" {
            let (value, source, prefab_byte) = recipe::resolve_recipe_mastery(rel, content, &self.multipliers);
            entry.mastery = value;
            let rdata = entry
                .data
                .entry("recipe")
                .or_insert_with(|| Value::Object(Map::new()))
                .as_object_mut()
                .expect("recipe dict");
            rdata.insert("mastery".into(), value.map_or(Value::Null, int_value));
            rdata.insert("mastery_source".into(), json!(source));
            if let Some(b) = prefab_byte {
                rdata.insert("mastery_prefab_byte".into(), int_value(b));
            }
            let stem = py::lower(py::last_part(&rel.replace('\\', "/"), "/"));
            let cat = self.recipe_catalogue.get(&stem);
            rdata.insert("in_catalogue".into(), json!(cat.is_some()));
            if let Some(order) = cat {
                rdata.insert("catalogue_order".into(), json!(order));
            }
            if let Some(provs) = self.recipe_providers.get(&stem).filter(|p| !p.is_empty()) {
                rdata.insert("providers".into(), Value::Array(provs.clone()));
            }
            entry.mastery_geode = mastery::geode_mastery_for(rel, &self.geode_multipliers);
        } else {
            entry.mastery = mastery::mastery_for(rel, &self.multipliers);
            entry.mastery_geode = mastery::geode_mastery_for(rel, &self.geode_multipliers);
        }
        if py::lower(rel).starts_with("item/companion/") {
            self.attach_geode_companion(&mut entry, rel, content);
        }
        entry.stats = stat_rows(path, &entry.data);
        entry.abilities = ability_rows(path, &entry.data);
        if ctype == "recipe" {
            entry.links = links::recipe_links(path, entry.data.get("recipe").unwrap_or(&Value::Null));
        }
        Ok(entry)
    }
}

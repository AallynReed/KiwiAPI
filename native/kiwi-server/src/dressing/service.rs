//! `app/trove/dressing/service.py`: a selection resolved against the
//! catalogue, its placements on the class rig, the assembled model and the
//! still; plus the archive lookups those share with the embed and the
//! creature renders (`render.source.blueprint_by_basename`,
//! `trove_layout.game_file_paths`).

use std::{
    collections::HashMap,
    sync::{Arc, LazyLock, Mutex},
};

use axum::response::{IntoResponse, Response};
use base64::Engine;
use kiwi_core::{
    dressing::{self as kd, Catalogue, DressClass, Opt, Race},
    rig::Placement,
};
use mongodb::bson::{Bson, Document, doc};
use serde_json::{Map, Value, json};

use crate::{
    blueprints::cache::{BuildError, Cached, Fmt},
    creature,
    error::ApiError,
    state::AppState,
    workshop::game::GameIndex,
};

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

pub const DROP_UNKNOWN: &str = "unknown";
pub const DROP_NO_SOCKET: &str = "no_socket";
pub const DROP_MISSING: &str = "missing_asset";
pub const DROP_COVERED: &str = "covered";
pub const NONE: &str = "none";
pub const BLUEPRINT_SLOTS: [&str; 6] = ["head", "hair", "eyes", "hat", "face", "weapon"];
const ID_SLOTS: [&str; 6] = ["hat", "face", "weapon", "head", "hair", "eyes"];
const STYLE_SLOTS: [&str; 3] = ["hat", "face", "weapon"];
const RACE_SLOTS: [&str; 3] = ["head", "hair", "eyes"];
const SLOT_COLOR: [(&str, &str); 2] = [("hair", "hair_color"), ("eyes", "eye_color")];
const BROW_MAX_OFF: i64 = 24;
pub const STILL_VERSION: &str = "d1";
pub const STILL_MIN_DIM: i64 = 32;
pub const STILL_MAX_DIM: i64 = 512;

/// `DIRECT_APS[slot]`.
fn direct_aps(slot: &str) -> &'static [&'static str] {
    match slot {
        "head" => &["head"],
        "hair" => &["hair", "head"],
        "eyes" => &["face"],
        _ => &[],
    }
}

pub fn is_direct(slot: &str) -> bool {
    !direct_aps(slot).is_empty()
}

/// `attach_point(slot, skeleton)`.
pub fn attach_point(slot: &str, skeleton: &str) -> Option<&'static str> {
    let rig = crate::rigs::rig(skeleton)?;
    direct_aps(slot).iter().copied().find(|ap| rig.has_ap(ap))
}

fn get<'a, T>(v: &'a [(String, T)], k: &str) -> Option<&'a T> {
    v.iter().find(|(key, _)| key == k).map(|(_, t)| t)
}

fn has<T>(v: &[(String, T)], k: &str) -> bool {
    v.iter().any(|(key, _)| key == k)
}

fn pop<T>(v: &mut Vec<(String, T)>, k: &str) -> Option<T> {
    let i = v.iter().position(|(key, _)| key == k)?;
    Some(v.remove(i).1)
}

#[derive(Clone, Debug, PartialEq)]
pub struct Issue {
    pub slot: String,
    pub value: String,
    pub reason: &'static str,
}

/// `service.Outfit`.
#[derive(Clone, Debug)]
pub struct Outfit {
    pub cls: DressClass,
    pub costume: Arc<Opt>,
    pub race: Option<Race>,
    pub styles: Vec<(String, Arc<Opt>)>,
    pub blueprints: Vec<(String, String)>,
    pub weapon_family: String,
    pub colors: Vec<(String, [i64; 3])>,
    pub hair_scale: Option<f64>,
    pub issues: Vec<Issue>,
}

fn hex(rgb: &[i64; 3]) -> String {
    format!("{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2])
}

impl Outfit {
    pub fn dropped(&self) -> Vec<String> {
        self.issues.iter().map(|i| i.slot.clone()).collect()
    }

    pub fn issue_header(&self) -> String {
        self.issues.iter().map(|i| format!("{}={}", i.slot, i.reason)).collect::<Vec<_>>().join(",")
    }

    /// `Outfit.ident`.
    pub fn ident(&self) -> String {
        let picks: Vec<String> =
            ID_SLOTS.iter().filter_map(|s| get(&self.styles, s).map(|o| format!("{s}={}", o.key))).collect();
        let raw: Vec<String> =
            ID_SLOTS.iter().filter_map(|s| get(&self.blueprints, s).map(|r| format!("{s}~{r}"))).collect();
        let mut fam = if self.weapon_family.is_empty() { String::new() } else { format!("@{}", self.weapon_family) };
        let mut sorted: Vec<&(String, [i64; 3])> = self.colors.iter().collect();
        sorted.sort_by(|a, b| a.0.cmp(&b.0));
        let tints: Vec<String> = sorted.iter().map(|(s, rgb)| format!("{s}#{}", hex(rgb))).collect();
        if !tints.is_empty() {
            fam = format!("{fam}({})", tints.join(","));
        }
        if let Some(hs) = self.hair_scale {
            fam = format!("{fam}~h{}", crate::pyfloat::repr(hs));
        }
        let race = self.race.as_ref().map_or("", |r| r.key.as_str());
        format!("{}/{}/{race}/{}/{}{fam}", self.cls.key, self.costume.key, picks.join(","), raw.join(","))
    }

    /// `Outfit.tint_for(slot)`.
    pub fn tint_for(&self, slot: &str) -> Option<([i64; 3], Option<i64>)> {
        if let Some(own) = get(&self.colors, slot) {
            return Some((*own, None));
        }
        if slot == "head"
            && let Some(hair) = get(&self.colors, "hair")
        {
            return Some((*hair, Some(BROW_MAX_OFF)));
        }
        None
    }

    pub fn head_scaled(&self, ap: &str) -> f64 {
        kiwi_core::rig::scale_for(ap, crate::rigs::rig(&self.cls.skeleton), self.costume.head_scale, 1.0)
    }

    pub fn piece_scale(&self, slot: &str, ap: &str) -> f64 {
        match self.hair_scale {
            Some(hs) if slot == "hair" => hs,
            _ => self.head_scaled(ap),
        }
    }

    /// `DressOutfit(**outfit.as_dict(), dropped, issues)`, by alias.
    pub fn outfit_value(&self) -> Value {
        let mut m = Map::new();
        m.insert("class".into(), json!(self.cls.key));
        m.insert("costume".into(), json!(self.costume.key));
        let slot = |s: &str| -> Value {
            match get(&self.styles, s) {
                Some(o) => json!(o.key),
                None => get(&self.blueprints, s).map_or(Value::Null, |r| json!(r)),
            }
        };
        for s in ["hat", "face", "weapon", "head", "hair", "eyes"] {
            m.insert(s.into(), slot(s));
        }
        m.insert("race".into(), self.race.as_ref().map_or(Value::Null, |r| json!(r.key)));
        for (s, param) in SLOT_COLOR {
            m.insert(param.into(), get(&self.colors, s).map_or(Value::Null, |rgb| json!(format!("#{}", hex(rgb)))));
        }
        m.insert("dropped".into(), json!(self.dropped()));
        let issues: Vec<Value> =
            self.issues.iter().map(|i| json!({"slot": i.slot, "value": i.value, "reason": i.reason})).collect();
        m.insert("issues".into(), Value::Array(issues));
        Value::Object(m)
    }
}

/// A raw selection: what the router or the embed token hands `resolve`.
#[derive(Clone, Debug, Default)]
pub struct Selection {
    pub class_key: String,
    pub costume: Option<String>,
    pub picks: Vec<(String, Option<String>)>,
    pub weapon_family: Option<String>,
    pub race: Option<String>,
    pub colors: Vec<(String, Option<String>)>,
    pub hair_scale: Option<f64>,
}

fn pick<'a>(v: &'a [(String, Option<String>)], k: &str) -> &'a str {
    get(v, k).and_then(|o| o.as_deref()).unwrap_or("")
}

/// `resolve(class_key, costume, picks, ...)`: `None` for an unknown class.
pub async fn resolve(state: &AppState, cat: &Catalogue, sel: &Selection) -> Result<Option<Outfit>, ApiError> {
    let Some(cls) = cat.class(&sel.class_key.to_lowercase()) else { return Ok(None) };
    let wardrobe = cat.costumes_of(&cls.key);
    let Some(first) = wardrobe.first() else { return Ok(None) };
    let costume = match cat.get("costume", &sel.costume.as_deref().unwrap_or("").to_lowercase()) {
        Some(c) if c.skeleton == cls.skeleton => c.clone(),
        _ => first.clone(),
    };
    let race = cat
        .race(&sel.race.as_deref().unwrap_or("").to_lowercase())
        .or_else(|| cat.races.iter().find(|r| !r.first("head").is_empty()))
        .cloned();
    let branch = state.settings().trove_render_branch.clone();

    let mut styles: Vec<(String, Arc<Opt>)> = Vec::new();
    let mut raw: Vec<(String, String)> = Vec::new();
    let mut issues: Vec<Issue> = Vec::new();
    let drop = |issues: &mut Vec<Issue>, slot: &str, value: &str, reason: &'static str| {
        issues.push(Issue { slot: slot.into(), value: value.into(), reason });
    };
    for slot in BLUEPRINT_SLOTS {
        let key = kiwi_core::pystr::strip(pick(&sel.picks, slot)).to_lowercase();
        if key.is_empty() {
            continue;
        }
        if key == NONE {
            raw.push((slot.into(), NONE.into()));
            continue;
        }
        if let Some(opt) = cat.get(slot, &key) {
            if !RACE_SLOTS.contains(&slot) && kd::sockets_for_slot(&cls.sockets, opt.slot_id).is_empty() {
                drop(&mut issues, slot, &key, DROP_NO_SOCKET);
                continue;
            }
            if blueprint_path(state, &opt.blueprint, &opt.prefab, &branch, &opt.reference).await?.is_none() {
                drop(&mut issues, slot, &key, DROP_MISSING);
                continue;
            }
            styles.push((slot.into(), opt.clone()));
            continue;
        }
        let found = match kd::blueprint_ref(&key) {
            Some(r) => {
                let base = r.rsplit('/').next().unwrap_or(&r).to_owned();
                let exact = if r.contains('/') { r.clone() } else { String::new() };
                blueprint_path(state, &base, &r, &branch, &exact).await?.map(|_| r)
            }
            None => None,
        };
        match found {
            Some(r) => raw.push((slot.into(), r)),
            None => drop(&mut issues, slot, &key, DROP_UNKNOWN),
        }
    }

    let mut fam = kiwi_core::pystr::strip(sel.weapon_family.as_deref().unwrap_or("")).to_owned();
    if !fam.is_empty() && kd::sockets_for_family(&cls.sockets, &fam).is_empty() {
        fam.clear();
    }
    if fam.is_empty() && has(&raw, "weapon") {
        fam = cls.weapons.first().cloned().unwrap_or_default();
    }
    if get(&styles, "hat").is_some_and(|h| h.covers_head) {
        for slot in ["hair", "face"] {
            let asked = pop(&mut styles, slot).map(|_| ()).or_else(|| pop(&mut raw, slot).map(|_| ()));
            if asked.is_some() {
                drop(&mut issues, slot, pick(&sel.picks, slot), DROP_COVERED);
            }
        }
    }
    if let Some(r) = &race {
        for slot in ["head", "eyes"] {
            if has(&styles, slot) || has(&raw, slot) {
                continue;
            }
            if slot == "eyes" && (has(&styles, "face") || has(&raw, "face")) {
                continue;
            }
            let default = r.first(slot);
            if !default.is_empty() {
                raw.push((slot.into(), default.to_lowercase()));
            }
        }
    }
    for slot in RACE_SLOTS {
        if (has(&styles, slot) || has(&raw, slot)) && attach_point(slot, &cls.skeleton).is_none() {
            pop(&mut styles, slot);
            pop(&mut raw, slot);
            if !issues.iter().any(|i| i.slot == slot) {
                drop(&mut issues, slot, pick(&sel.picks, slot), DROP_NO_SOCKET);
            }
        }
    }
    let mut colors = Vec::new();
    for (slot, param) in SLOT_COLOR {
        if let Some(rgb) = kd::color_ref(pick(&sel.colors, param)) {
            colors.push((slot.to_owned(), rgb));
        }
    }
    let hair_scale = sel.hair_scale.filter(|h| (0.05..=1.0).contains(h)).map(|h| crate::pyfloat::round(h, 4));
    Ok(Some(Outfit {
        cls: cls.clone(),
        costume,
        race,
        styles,
        blueprints: raw,
        weapon_family: fam,
        colors,
        hair_scale,
        issues,
    }))
}

// --- the archive lookups -------------------------------------------------------------------

#[cfg(test)]
pub struct TestArchive {
    pub paths: Vec<String>,
    pub basenames: HashMap<String, String>,
    pub blueprints: HashMap<String, Vec<u8>>,
    pub files: HashMap<String, Vec<u8>>,
}

#[cfg(test)]
pub static TEST_ARCHIVE: std::sync::OnceLock<TestArchive> = std::sync::OnceLock::new();

#[cfg(test)]
pub fn test_game_file(path: &str) -> Option<Option<Vec<u8>>> {
    TEST_ARCHIVE.get().map(|a| a.files.get(path).cloned())
}

/// `trove_layout.game_file_paths(branch)` (with `game_file_map`'s index).
pub async fn game_paths(state: &AppState, branch: &str) -> Result<Arc<GameIndex>, ApiError> {
    #[cfg(test)]
    if let Some(a) = TEST_ARCHIVE.get() {
        return Ok(Arc::new(crate::workshop::game::from_paths(1, a.paths.clone())));
    }
    crate::workshop::game::index(state, branch).await.map_err(|e| {
        tracing::error!("game file index {branch}: {e:#}");
        ApiError::internal()
    })
}

type Basenames = Arc<HashMap<String, String>>;
static BASENAMES: LazyLock<Mutex<HashMap<String, (String, Basenames)>>> = LazyLock::new(Default::default);

fn internal(e: impl std::fmt::Display) -> ApiError {
    tracing::error!("dressing: {e}");
    ApiError::internal()
}

/// `render.source.blueprint_by_basename(branch)`.
pub async fn blueprint_by_basename(state: &AppState, branch: &str) -> Result<Basenames, ApiError> {
    #[cfg(test)]
    if let Some(a) = TEST_ARCHIVE.get() {
        return Ok(Arc::new(a.basenames.clone()));
    }
    let db = state.inner.db.mongo.as_ref().ok_or_else(|| internal("no mongo"))?;
    let coll = db.collection::<Document>("update_state");
    let query = doc! {"branch": branch, "path": {"$regex": "^blueprints/"}};
    let sig = coll.count_documents(query.clone()).await.map_err(internal)?.to_string();
    if let Some((s, idx)) = BASENAMES.lock().ok().and_then(|m| m.get(branch).cloned())
        && s == sig
    {
        return Ok(idx);
    }
    let mut cursor = coll.find(query).projection(doc! {"path": 1, "_id": 0}).await.map_err(internal)?;
    let mut seen: HashMap<String, Option<String>> = HashMap::new();
    while cursor.advance().await.map_err(internal)? {
        let d = cursor.deserialize_current().map_err(internal)?;
        let Some(Bson::String(full)) = d.get("path") else { continue };
        let path: String = full.chars().skip(11).collect();
        let last = path.rsplit('/').next().unwrap_or(&path);
        let base = last.strip_suffix(".blueprint").unwrap_or(last).to_lowercase();
        let value = if seen.contains_key(&base) { None } else { Some(path.clone()) };
        seen.insert(base, value);
    }
    let idx: Basenames =
        Arc::new(seen.into_iter().filter_map(|(b, p)| p.filter(|p| !p.is_empty()).map(|p| (b, p))).collect());
    if let Ok(mut m) = BASENAMES.lock() {
        m.insert(branch.to_owned(), (sig, idx.clone()));
    }
    Ok(idx)
}

/// `get_blueprint_bytes(name, branch)`.
pub async fn blueprint_bytes(state: &AppState, name: &str, branch: &str) -> Option<Vec<u8>> {
    #[cfg(test)]
    if let Some(a) = TEST_ARCHIVE.get() {
        let n = name.replace('\\', "/").to_lowercase();
        let n = n.strip_prefix("blueprints/").unwrap_or(&n);
        return a.blueprints.get(n).cloned().filter(|b| !b.is_empty());
    }
    crate::blueprints::render::blueprint_bytes(state, name, branch).await
}

/// `blueprint_path(basename, hint, branch, ref)`.
pub async fn blueprint_path(
    state: &AppState,
    basename: &str,
    hint: &str,
    branch: &str,
    reference: &str,
) -> Result<Option<String>, ApiError> {
    let paths = game_paths(state, branch).await?;
    let cands = paths.all.get(&format!("{basename}.blueprint")).cloned().unwrap_or_default();
    if !reference.is_empty() {
        let exact = format!("blueprints/{reference}.blueprint");
        if cands.contains(&exact) {
            return Ok(Some(exact));
        }
    }
    let index = blueprint_by_basename(state, branch).await?;
    if let Some(found) = index.get(basename) {
        return Ok(Some(found.clone()));
    }
    Ok(kiwi_core::workshop::nearest_path(&cands, hint).cloned())
}

pub async fn redis_get(state: &AppState, key: &str) -> Option<String> {
    let mut conn = state.inner.db.redis.clone()?;
    match redis::cmd("GET").arg(key).query_async::<Option<String>>(&mut conn).await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("dressing: redis get failed for {key}: {e}");
            None
        }
    }
}

pub async fn redis_set(state: &AppState, key: &str, value: &str) {
    let Some(mut conn) = state.inner.db.redis.clone() else { return };
    let ttl = state.settings().trove_render_cache_ttl;
    let set: redis::RedisResult<()> =
        redis::cmd("SET").arg(key).arg(value).arg("EX").arg(ttl).query_async(&mut conn).await;
    if let Err(e) = set {
        tracing::warn!("dressing: redis set failed for {key}: {e}");
    }
}

// --- placements, model, still --------------------------------------------------------------

async fn read(
    state: &AppState,
    branch: &str,
    basename: &str,
    hint: &str,
    reference: &str,
) -> Result<Option<Vec<u8>>, ApiError> {
    match blueprint_path(state, basename, hint, branch, reference).await? {
        Some(path) => Ok(blueprint_bytes(state, &path, branch).await),
        None => Ok(None),
    }
}

/// `_placements(outfit, branch)`.
pub async fn placements(state: &AppState, o: &Outfit, branch: &str) -> Result<Vec<Placement>, ApiError> {
    let mut out = Vec::new();
    let mut place = |ap: &str, raw: &[u8], scale: f64, tint| {
        out.push(Placement { ap: ap.to_owned(), raw: raw.to_vec(), scale, tint });
    };
    for (basename, ap) in &o.costume.parts {
        if let Some(raw) = read(state, branch, basename, &o.costume.prefab, o.costume.ref_of(basename)).await? {
            place(ap, &raw, o.costume.scale_of(basename), None);
        }
    }
    for slot in STYLE_SLOTS {
        let Some(opt) = get(&o.styles, slot) else { continue };
        let Some(raw) = read(state, branch, &opt.blueprint, &opt.prefab, &opt.reference).await? else { continue };
        for socket in kd::sockets_for_slot(&o.cls.sockets, opt.slot_id) {
            place(&socket.ap, &raw, o.head_scaled(&socket.ap), None);
        }
    }
    for (slot, opt) in &o.styles {
        if !RACE_SLOTS.contains(&slot.as_str()) {
            continue;
        }
        let Some(ap) = attach_point(slot, &o.cls.skeleton) else { continue };
        if let Some(data) = read(state, branch, &opt.blueprint, &opt.prefab, "").await? {
            place(ap, &data, o.piece_scale(slot, ap), o.tint_for(slot));
        }
    }
    for (slot, r) in &o.blueprints {
        if r == NONE {
            continue;
        }
        let aps: Vec<String> = if is_direct(slot) {
            attach_point(slot, &o.cls.skeleton).map(|a| vec![a.to_owned()]).unwrap_or_default()
        } else if slot == "hat" || slot == "face" {
            let fam = if slot == "hat" { "Hat" } else { "Face" };
            kd::sockets_for_family(&o.cls.sockets, fam).into_iter().map(|s| s.ap.clone()).collect()
        } else {
            kd::sockets_for_family(&o.cls.sockets, &o.weapon_family).into_iter().map(|s| s.ap.clone()).collect()
        };
        let Some(first) = aps.first() else { continue };
        let base = r.rsplit('/').next().unwrap_or(r);
        let exact = if r.contains('/') { r.as_str() } else { "" };
        let Some(data) = read(state, branch, base, r, exact).await? else { continue };
        let tint = o.tint_for(slot);
        let scale = if RACE_SLOTS.contains(&slot.as_str()) { o.piece_scale(slot, first) } else { o.head_scaled(first) };
        for ap in &aps {
            place(ap, &data, scale, tint);
        }
    }
    Ok(out)
}

/// `service.model(outfit, fmt)`: `None` when the outfit draws nothing.
pub async fn model(state: &AppState, o: &Outfit, fmt: Fmt) -> Result<Option<Cached>, Response> {
    let branch = creature::render_branch(state);
    let st = state.clone();
    let outfit = o.clone();
    let build = || async move {
        let pl = placements(&st, &outfit, &branch).await.map_err(BuildError::Api)?;
        if pl.is_empty() {
            return Err(creature::no_payload());
        }
        let skeleton = outfit.cls.skeleton.clone();
        let built = tokio::task::spawn_blocking(move || -> Result<Option<Map<String, Value>>, String> {
            let Some(rig) = crate::rigs::rig(&skeleton) else { return Ok(None) };
            let parts = kiwi_core::rig::placement_parts(rig, &pl)?;
            if parts.is_empty() {
                return Ok(None);
            }
            match kiwi_core::rig::payload(rig, &skeleton, &parts) {
                Value::Object(m) => Ok(Some(m)),
                _ => Ok(None),
            }
        })
        .await
        .map_err(|e| BuildError::Other(creature::internal(e)))?
        .map_err(|e| BuildError::Other(creature::internal(e)))?;
        built.ok_or_else(creature::no_payload)
    };
    let sig = creature::signature(state).await?;
    creature::cached(state, sig.as_deref(), &format!("dress:{}", o.ident()), fmt, build).await
}

/// Python's `f"{x:g}"`.
pub fn format_g(x: f64) -> String {
    if x.is_nan() {
        return "nan".into();
    }
    if x.is_infinite() {
        return if x > 0.0 { "inf" } else { "-inf" }.into();
    }
    let e = format!("{x:.5e}");
    let (mant, exp) = e.split_once('e').unwrap_or((&e, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    let trim = |s: &str| -> String {
        if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_owned() } else { s.to_owned() }
    };
    if (-4..6).contains(&exp) {
        trim(&format!("{x:.*}", (5 - exp) as usize))
    } else {
        format!("{}e{}{:02}", trim(mant), if exp < 0 { '-' } else { '+' }, exp.abs())
    }
}

/// `service.still(outfit, dim, camera)`: the PNG, `None` when nothing draws.
pub async fn still(state: &AppState, o: &Outfit, dim: i64, az: f64, el: f64) -> Result<Option<Vec<u8>>, Response> {
    let branch = creature::render_branch(state);
    let dim = dim.clamp(STILL_MIN_DIM, STILL_MAX_DIM);
    let sig = creature::signature(state).await?.unwrap_or_else(|| "-".into());
    let key = format!(
        "render:dress:{STILL_VERSION}:{}:{sig}:{dim}:az={},el={}:{}",
        crate::blueprints::cache::ASSEMBLY_VERSION,
        format_g(az),
        format_g(el),
        o.ident()
    );
    if let Some(hit) = redis_get(state, &key).await.filter(|s| !s.is_empty())
        && let Ok(png) = B64.decode(hit.as_bytes())
    {
        return Ok(Some(png).filter(|p| !p.is_empty()));
    }
    let pl = placements(state, o, &branch).await.map_err(IntoResponse::into_response)?;
    let png = if pl.is_empty() {
        None
    } else {
        let skeleton = o.cls.skeleton.clone();
        let d = dim as usize;
        tokio::task::spawn_blocking(move || -> Result<Option<Vec<u8>>, String> {
            let Some(rig) = crate::rigs::rig(&skeleton) else { return Ok(None) };
            let cells = kiwi_core::rig::placement_voxels(rig, &pl)?;
            if cells.is_empty() {
                return Ok(None);
            }
            Ok(kiwi_core::voxel::render_baked(&cells, d, az, el)
                .map(|rgba| kiwi_pil::png::encode_rgba_optimized(&rgba, d, d)))
        })
        .await
        .map_err(creature::internal)?
        .map_err(creature::internal)?
    };
    redis_set(state, &key, &B64.encode(png.as_deref().unwrap_or_default())).await;
    Ok(png)
}

#[cfg(test)]
mod tests {
    #[test]
    fn format_g_matches_python() {
        for (x, want) in [
            (30.0, "30"),
            (8.0, "8"),
            (-0.0, "-0"),
            (8.5, "8.5"),
            (-12.345678, "-12.3457"),
            (1e-5, "1e-05"),
            (0.0001, "0.0001"),
            (123456.0, "123456"),
            (1234567.0, "1.23457e+06"),
            (359.9999999, "360"),
            (0.1, "0.1"),
        ] {
            assert_eq!(super::format_g(x), want, "{x}");
        }
    }
}

//! The Dressing Room's pure half (`app/trove/dressing`): the class socket
//! table and the character-creation table read off the wire, the option
//! labels, and the selection validators.

use std::sync::LazyLock;

use regex::Regex;

use crate::codex::py::bytes_re;

/// `sockets.SLOTS`: slot number -> display family.
pub const SLOTS: [(i64, &str); 9] = [
    (8, "Gun"),
    (10, "Bow"),
    (12, "Staff"),
    (16, "Spear"),
    (18, "Fist"),
    (24, "Hat"),
    (26, "Face"),
    (28, "Melee"),
    (40, "Banner"),
];

pub const HAT_SLOT: i64 = 24;
pub const FACE_SLOT: i64 = 26;
pub const BANNER_SLOT: i64 = 40;
const WEAPON_SLOTS: [i64; 6] = [8, 10, 12, 16, 18, 28];

pub fn family(slot: i64) -> Option<&'static str> {
    SLOTS.iter().find(|(s, _)| *s == slot).map(|(_, f)| *f)
}

#[derive(Clone, Debug, PartialEq)]
pub struct Socket {
    pub bone: String,
    pub ap: String,
    pub slot: i64,
    pub family: &'static str,
    pub flag: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ClassForm {
    pub skeleton: String,
    pub name_key: String,
    pub sockets: Vec<Socket>,
}

static SOCKET_RE: LazyLock<regex::bytes::Regex> = LazyLock::new(|| {
    bytes_re(r"(?s)\x08(.)([\x20-\x7e]{2,40})\x18(.)([\x20-\x7e]{0,40})\x20([\x00-\x7f])\x30([\x00-\x7f])\x1e")
});
static SKELETON_RE: LazyLock<regex::bytes::Regex> = LazyLock::new(|| bytes_re(r"([A-Za-z0-9_]+)\.skeleton\.gr2"));
static DISPLAY_RE: LazyLock<regex::bytes::Regex> = LazyLock::new(|| bytes_re(r"\$DisplayName_[A-Za-z0-9_]+"));
static RACE_RE: LazyLock<regex::bytes::Regex> = LazyLock::new(|| bytes_re(r"\$CustomHead_Race_([A-Za-z0-9_]+)"));
static PIECE_RE: LazyLock<regex::bytes::Regex> = LazyLock::new(|| bytes_re(r"\$CustomHead_Piece_([A-Za-z0-9_\[\]]+)"));

fn ascii(b: &[u8]) -> String {
    crate::codex::py::ascii_ignore(b)
}

/// `_ap_key(bone)`.
fn ap_key(bone: &str) -> String {
    if bone.len() >= 3 && bone[..3].eq_ignore_ascii_case("ap_") {
        bone[3..].to_lowercase()
    } else {
        bone.to_lowercase()
    }
}

/// `sockets.parse_class(data)`.
pub fn parse_class(data: &[u8]) -> Option<ClassForm> {
    let skel = SKELETON_RE.captures(data)?;
    let mut sockets: Vec<Socket> = Vec::new();
    for m in SOCKET_RE.captures_iter(data) {
        let (len_name, raw_name, len_target, raw_target) = (&m[1], &m[2], &m[3], &m[4]);
        let (slot, flag) = (i64::from(m[5][0]), i64::from(m[6][0]));
        if usize::from(len_name[0]) != raw_name.len() || usize::from(len_target[0]) != raw_target.len() {
            continue;
        }
        let bone = ascii(raw_name);
        if !bone.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
            continue;
        }
        let Some(fam) = family(slot) else { continue };
        if sockets.iter().any(|s| s.bone == bone && s.slot == slot) {
            continue;
        }
        sockets.push(Socket { ap: ap_key(&bone), bone, slot, family: fam, flag });
    }
    if sockets.is_empty() {
        return None;
    }
    Some(ClassForm {
        skeleton: ascii(&skel[1]).to_lowercase(),
        name_key: DISPLAY_RE.find(data).map(|m| ascii(m.as_bytes())).unwrap_or_default(),
        sockets,
    })
}

/// `style_slot(flags)` given the identity's field 10.
pub fn style_slot(flag10: Option<i128>) -> Option<i64> {
    let s = i64::try_from(flag10?).ok()?;
    family(s).map(|_| s)
}

/// `weapon_families(sockets)`.
pub fn weapon_families(sockets: &[Socket]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for s in sockets {
        if WEAPON_SLOTS.contains(&s.slot) && !out.iter().any(|f| f == s.family) {
            out.push(s.family.to_owned());
        }
    }
    out
}

/// `sockets_for_family(sockets, family)`.
pub fn sockets_for_family<'a>(sockets: &'a [Socket], family: &str) -> Vec<&'a Socket> {
    let want = crate::pystr::strip(family).to_lowercase();
    sockets.iter().filter(|s| s.family.to_lowercase() == want).collect()
}

/// `sockets_for_slot(sockets, slot)`.
pub fn sockets_for_slot(sockets: &[Socket], slot: i64) -> Vec<&Socket> {
    sockets.iter().filter(|s| s.slot == slot).collect()
}

// --- customhead -------------------------------------------------------------

/// `customhead.piece_kind(blueprint)`.
pub fn piece_kind(blueprint: &str) -> Option<&'static str> {
    let b = blueprint.to_lowercase();
    if b.contains("_hair") {
        Some("hair")
    } else if b.contains("_head") {
        Some("head")
    } else if b.contains("eye") {
        Some("eyes")
    } else {
        None
    }
}

/// One `customhead.parse` row.
#[derive(Clone, Debug, PartialEq)]
pub struct RaceRow {
    pub race: String,
    pub key: String,
    pub name_key: String,
    /// kind -> blueprints, kinds in first-seen order.
    pub pieces: Vec<(String, Vec<String>)>,
}

impl RaceRow {
    pub fn pieces_of(&self, kind: &str) -> &[String] {
        self.pieces.iter().find(|(k, _)| k == kind).map(|(_, v)| v.as_slice()).unwrap_or(&[])
    }
}

/// `customhead.parse(data)`.
pub fn parse_heads(data: &[u8]) -> Vec<RaceRow> {
    let races: Vec<(usize, String)> =
        RACE_RE.captures_iter(data).map(|c| (c.get(0).map_or(0, |m| m.start()), ascii(&c[1]))).collect();
    if races.is_empty() {
        return Vec::new();
    }
    let pieces: Vec<(usize, String)> =
        PIECE_RE.captures_iter(data).map(|c| (c.get(0).map_or(0, |m| m.start()), ascii(&c[1]))).collect();
    let mut out = Vec::new();
    for (i, (lo, name)) in races.iter().enumerate() {
        let hi = races.get(i + 1).map_or(data.len(), |r| r.0);
        let mut grouped: Vec<(String, Vec<String>)> = Vec::new();
        for (po, bp) in &pieces {
            if !(*lo <= *po && *po < hi) {
                continue;
            }
            let Some(kind) = piece_kind(bp) else { continue };
            let slot = match grouped.iter().position(|(k, _)| k == kind) {
                Some(j) => j,
                None => {
                    grouped.push((kind.to_owned(), Vec::new()));
                    grouped.len() - 1
                }
            };
            if !grouped[slot].1.contains(bp) {
                grouped[slot].1.push(bp.clone());
            }
        }
        out.push(RaceRow {
            race: name.clone(),
            key: name.to_lowercase(),
            name_key: format!("$CustomHead_Race_{name}"),
            pieces: grouped,
        });
    }
    out
}

// --- catalogue labels ---------------------------------------------------------

static NAME_KEY_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\$[A-Za-z0-9_]+_(?:skinset_)?name$").expect("name key regex"));
static HELMET_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"_helm(?:_|$|[^a-z])").expect("helmet regex"));
static CREDIT_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[([^\]]+)\]").expect("credit regex"));
static PIECE_PREFIX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^c_p_(?:head|hair|eyes)_?").expect("piece prefix regex"));
static WORD_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[_/]+").expect("word regex"));

/// `_NAME_KEY_RE.match(s)`.
pub fn is_name_key(s: &str) -> bool {
    NAME_KEY_RE.is_match(s)
}

/// `_HELMET_RE.search(blueprint)`.
pub fn covers_head(blueprint: &str) -> bool {
    HELMET_RE.is_match(blueprint)
}

/// `_CREDIT_RE.search(bp).group(1)`.
pub fn credit(bp: &str) -> Option<String> {
    CREDIT_RE.captures(bp).map(|c| c[1].to_owned())
}

/// The titlecase of one character (`ß` -> `Ss`, `ǆ` -> `ǅ`).
fn titlecase(c: char) -> String {
    match c {
        '\u{1c4}'..='\u{1c6}' => '\u{1c5}'.into(),
        '\u{1c7}'..='\u{1c9}' => '\u{1c8}'.into(),
        '\u{1ca}'..='\u{1cc}' => '\u{1cb}'.into(),
        '\u{1f1}'..='\u{1f3}' => '\u{1f2}'.into(),
        _ => {
            let up: String = c.to_uppercase().collect();
            let mut it = up.chars();
            match it.next() {
                Some(f) => std::iter::once(f).chain(it.as_str().to_lowercase().chars()).collect(),
                None => String::new(),
            }
        }
    }
}

/// `str.capitalize()`.
fn capitalize(w: &str) -> String {
    let mut c = w.chars();
    match c.next() {
        Some(f) => titlecase(f) + &c.as_str().to_lowercase(),
        None => String::new(),
    }
}

/// `_humanize(stem)`.
pub fn humanize(stem: &str) -> String {
    let body = CREDIT_RE.replace_all(stem, "");
    let body = PIECE_PREFIX.replace(&body, "");
    let words: Vec<String> = WORD_RE.split(&body).filter(|w| !w.is_empty()).map(capitalize).collect();
    if words.is_empty() { stem.to_owned() } else { words.join(" ") }
}

/// `_stem(path)`.
pub fn stem(path: &str) -> String {
    let p = path.replace('\\', "/");
    let last = p.rsplit('/').next().unwrap_or_default();
    last.strip_suffix(".binfab").unwrap_or(last).to_lowercase()
}

// --- selection --------------------------------------------------------------

static BP_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[a-z0-9_][a-z0-9_.\-\[\]]*(?:/[a-z0-9_][a-z0-9_.\-\[\]]*)*$").expect("blueprint ref regex")
});

/// `service.blueprint_ref(value)`.
pub fn blueprint_ref(value: &str) -> Option<String> {
    let v = crate::pystr::strip(value).to_lowercase().replace('\\', "/");
    if v.is_empty() || v.chars().count() > 200 || v.contains("..") {
        return None;
    }
    let v = v.strip_suffix(".blueprint").unwrap_or(&v);
    (!v.is_empty() && BP_RE.is_match(v)).then(|| v.to_owned())
}

/// `service.color_ref(value)`.
pub fn color_ref(value: &str) -> Option<[i64; 3]> {
    let v = crate::pystr::strip(value).to_lowercase();
    let hex = v.strip_prefix('#').unwrap_or(&v);
    if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
        return None;
    }
    let n = i64::from_str_radix(hex, 16).ok()?;
    Some([(n >> 16) & 255, (n >> 8) & 255, n & 255])
}

// --- the catalogue ----------------------------------------------------------

pub const CLASS_ROOT: &str = "prefabs/class/";
pub const SKIN_ROOT: &str = "prefabs/skins/";
pub const EQUIP_ROOT: &str = "prefabs/equipment/";
pub const CUSTOM_HEADS: &str = "prefabs/custom_heads_service.binfab";
pub const STYLE_SLOTS: [&str; 6] = ["hat", "face", "weapon", "head", "hair", "eyes"];
pub const RACE_SLOTS: [&str; 3] = ["head", "hair", "eyes"];

/// `catalogue.Option`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Opt {
    pub key: String,
    pub name: String,
    pub slot: String,
    pub family: String,
    pub slot_id: i64,
    pub blueprint: String,
    pub prefab: String,
    pub skeleton: String,
    pub covers_head: bool,
    pub credit: String,
    pub parts: Vec<(String, String)>,
    pub refs: Vec<(String, String)>,
    pub reference: String,
    pub head_scale: f64,
    pub scales: Vec<(String, f64)>,
}

impl Opt {
    pub fn ref_of(&self, basename: &str) -> &str {
        self.refs.iter().find(|(b, _)| b == basename).map_or("", |(_, r)| r)
    }

    pub fn scale_of(&self, basename: &str) -> f64 {
        self.scales.iter().find(|(b, _)| b == basename).map_or(1.0, |(_, s)| *s)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Race {
    pub key: String,
    pub name: String,
    pub pieces: Vec<(String, Vec<String>)>,
}

impl Race {
    pub fn pieces_of(&self, kind: &str) -> &[String] {
        self.pieces.iter().find(|(k, _)| k == kind).map(|(_, v)| v.as_slice()).unwrap_or(&[])
    }

    /// `Race.first(kind)`.
    pub fn first(&self, kind: &str) -> &str {
        self.pieces_of(kind).first().map_or("", String::as_str)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct DressClass {
    pub key: String,
    pub name: String,
    pub skeleton: String,
    pub sockets: Vec<Socket>,
    pub weapons: Vec<String>,
}

/// `catalogue.Catalogue`: every map keeps Python's dict order.
#[derive(Clone, Debug, Default)]
pub struct Catalogue {
    pub classes: Vec<DressClass>,
    pub costumes: Vec<(String, Vec<std::sync::Arc<Opt>>)>,
    pub styles: Vec<(String, Vec<std::sync::Arc<Opt>>)>,
    pub options: std::collections::HashMap<String, std::sync::Arc<Opt>>,
    pub races: Vec<Race>,
}

impl Catalogue {
    pub fn is_empty(&self) -> bool {
        self.classes.is_empty()
    }

    pub fn class(&self, key: &str) -> Option<&DressClass> {
        self.classes.iter().find(|c| c.key == key)
    }

    pub fn costumes_of(&self, key: &str) -> &[std::sync::Arc<Opt>] {
        self.costumes.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_slice()).unwrap_or(&[])
    }

    pub fn styles_of(&self, slot: &str) -> &[std::sync::Arc<Opt>] {
        self.styles.iter().find(|(k, _)| k == slot).map(|(_, v)| v.as_slice()).unwrap_or(&[])
    }

    pub fn race(&self, key: &str) -> Option<&Race> {
        self.races.iter().find(|r| r.key == key)
    }

    pub fn get(&self, slot: &str, key: &str) -> Option<&std::sync::Arc<Opt>> {
        self.options.get(&format!("{slot}:{key}"))
    }
}

fn put<T>(v: &mut Vec<(String, T)>, k: String, val: T) {
    match v.iter_mut().find(|(key, _)| *key == k) {
        Some(slot) => slot.1 = val,
        None => v.push((k, val)),
    }
}

fn label(loc: &std::collections::HashMap<String, String>, key: &str) -> String {
    crate::codex::wire::clean_localized_text(loc.get(key).map_or("", String::as_str))
}

fn sort_by_name(options: &mut [std::sync::Arc<Opt>]) {
    options.sort_by_cached_key(|o| o.name.to_lowercase());
}

/// `_name_key(data)`.
fn name_key(data: &[u8]) -> Option<String> {
    crate::binfab::harvest_strings(data, 2, 512).into_iter().map(|h| h.text).find(|s| is_name_key(s))
}

/// `_blueprint_ref(data)`.
fn style_ref(data: &[u8]) -> String {
    for h in crate::binfab::harvest_strings(data, 2, 512) {
        let low = h.text.to_lowercase();
        if low.ends_with(".blueprint") {
            let r = low.replace('\\', "/");
            return r[..r.len() - ".blueprint".len()].to_owned();
        }
    }
    String::new()
}

/// What `_build` reads out of the archive, in the server's order.
pub struct Inputs<'a> {
    pub loc: &'a std::collections::HashMap<String, String>,
    pub classes: &'a [(String, Vec<u8>)],
    pub skins: &'a [(String, Vec<u8>)],
    pub rig_map: &'a crate::rig::RigMap,
    pub equipment: &'a [(String, Vec<u8>)],
    pub heads: Option<&'a [u8]>,
}

/// `_build_classes`.
pub fn build_classes(files: &[(String, Vec<u8>)], loc: &std::collections::HashMap<String, String>) -> Vec<DressClass> {
    let mut out: Vec<(String, DressClass)> = Vec::new();
    for (path, data) in files {
        let Some(parsed) = parse_class(data) else { continue };
        let key = stem(path);
        let name = Some(label(loc, &parsed.name_key)).filter(|n| !n.is_empty()).unwrap_or_else(|| humanize(&key));
        let weapons = weapon_families(&parsed.sockets);
        let c = DressClass { key: key.clone(), name, skeleton: parsed.skeleton, sockets: parsed.sockets, weapons };
        put(&mut out, key, c);
    }
    out.into_iter().map(|(_, c)| c).collect()
}

/// `_build`: `Ok(Catalogue::default())` when no class form is wearable.
pub fn build<'r>(i: &Inputs<'_>, rig_for: impl Fn(&str) -> Option<&'r crate::rig::Rig>) -> Result<Catalogue, String> {
    use std::sync::Arc;
    let classes = build_classes(i.classes, i.loc);
    if classes.is_empty() {
        return Ok(Catalogue::default());
    }
    let mut by_skeleton: Vec<(String, String)> = Vec::new();
    for c in &classes {
        put(&mut by_skeleton, c.skeleton.clone(), c.key.clone());
    }
    let mut names: Vec<(String, String)> = Vec::new();
    let mut refs: Vec<(String, Vec<(String, String)>)> = Vec::new();
    for (path, data) in i.skins {
        if let Some(key) = name_key(data) {
            let text = label(i.loc, &key);
            if !text.is_empty() {
                put(&mut names, stem(path), text);
            }
        }
        if let Some(found) = crate::codex::wire::extract_rig_refs(data)
            && !found.refs.is_empty()
        {
            put(&mut refs, path.clone(), found.refs);
        }
    }

    let mut costumes: Vec<(String, Vec<Arc<Opt>>)> = Vec::new();
    for prefab in &i.rig_map.order {
        let Some((skeleton, parts)) = i.rig_map.creatures.get(prefab) else { continue };
        if !prefab.starts_with(SKIN_ROOT) {
            continue;
        }
        let Some((_, class_key)) = by_skeleton.iter().find(|(s, _)| s == skeleton) else { continue };
        let key = stem(prefab);
        let head = i.rig_map.head_scale.get(prefab).copied().unwrap_or(1.0);
        let rig = rig_for(skeleton);
        let scales = parts
            .iter()
            .map(|(b, ap)| {
                let mesh = i.rig_map.mesh_scale.get(&(prefab.clone(), b.clone())).copied().unwrap_or(1.0);
                (b.clone(), crate::rig::scale_for(ap, rig, head, mesh))
            })
            .collect();
        let name = names.iter().find(|(s, _)| *s == key).map(|(_, n)| n.clone()).unwrap_or_else(|| humanize(&key));
        let opt = Opt {
            key,
            name,
            slot: "costume".into(),
            prefab: prefab.clone(),
            skeleton: skeleton.clone(),
            parts: parts.clone(),
            refs: refs.iter().find(|(p, _)| p == prefab).map(|(_, r)| r.clone()).unwrap_or_default(),
            head_scale: head,
            scales,
            ..Opt::default()
        };
        match costumes.iter_mut().find(|(k, _)| k == class_key) {
            Some((_, v)) => v.push(Arc::new(opt)),
            None => costumes.push((class_key.clone(), vec![Arc::new(opt)])),
        }
    }
    for (_, v) in costumes.iter_mut() {
        sort_by_name(v);
    }

    let mut styles: Vec<(String, Vec<Arc<Opt>>)> = STYLE_SLOTS.iter().map(|s| ((*s).to_owned(), Vec::new())).collect();
    for (path, data) in i.equipment {
        let ident = crate::codex::wire::decode_identity(data).map_err(|e| e.0)?;
        let flag = if ident.is_some() { crate::codex::wire::identity_flag(data, 10).map_err(|e| e.0)? } else { None };
        let Some(slot_id) = style_slot(flag) else { continue };
        if slot_id == BANNER_SLOT {
            continue;
        }
        let reference = style_ref(data);
        let blueprint = reference.rsplit('/').next().unwrap_or_default().to_owned();
        if blueprint.is_empty() {
            continue;
        }
        let slot = match slot_id {
            HAT_SLOT => "hat",
            FACE_SLOT => "face",
            _ => "weapon",
        };
        let key = stem(path);
        let name = label(i.loc, ident.as_ref().and_then(|d| d.name_key.as_deref()).unwrap_or(""));
        let opt = Opt {
            name: if name.is_empty() { humanize(&key) } else { name },
            key,
            slot: slot.into(),
            family: family(slot_id).unwrap_or_default().into(),
            slot_id,
            covers_head: slot == "hat" && covers_head(&blueprint),
            blueprint,
            reference,
            prefab: path.clone(),
            ..Opt::default()
        };
        if let Some((_, v)) = styles.iter_mut().find(|(s, _)| s == slot) {
            v.push(Arc::new(opt));
        }
    }
    for (_, v) in styles.iter_mut() {
        sort_by_name(v);
    }

    let mut races: Vec<Race> = Vec::new();
    if let Some(data) = i.heads {
        let mut slots: Vec<(String, Vec<Arc<Opt>>)> =
            RACE_SLOTS.iter().map(|s| ((*s).to_owned(), Vec::new())).collect();
        let mut seen = std::collections::HashSet::new();
        for row in parse_heads(data) {
            let name =
                Some(label(i.loc, &row.name_key)).filter(|n| !n.is_empty()).unwrap_or_else(|| humanize(&row.race));
            let race = Race { key: row.key.clone(), name, pieces: row.pieces.clone() };
            match races.iter_mut().find(|r| r.key == row.key) {
                Some(r) => *r = race,
                None => races.push(race),
            }
            for (kind, blueprints) in &row.pieces {
                for bp in blueprints {
                    let key = bp.to_lowercase();
                    if !seen.insert((kind.clone(), key.clone())) {
                        continue;
                    }
                    let lbl = label(i.loc, &format!("$CustomHead_Piece_{bp}"));
                    let opt = Opt {
                        name: if lbl.is_empty() { humanize(&key) } else { lbl },
                        slot: kind.clone(),
                        blueprint: key.clone(),
                        prefab: CUSTOM_HEADS.into(),
                        credit: credit(bp).unwrap_or_default(),
                        key,
                        ..Opt::default()
                    };
                    if let Some((_, v)) = slots.iter_mut().find(|(s, _)| s == kind) {
                        v.push(Arc::new(opt));
                    }
                }
            }
        }
        for (slot, mut v) in slots {
            sort_by_name(&mut v);
            put(&mut styles, slot, v);
        }
    }

    let classes: Vec<DressClass> =
        classes.into_iter().filter(|c| costumes.iter().any(|(k, v)| *k == c.key && !v.is_empty())).collect();
    costumes.retain(|(k, _)| classes.iter().any(|c| c.key == *k));
    let mut options = std::collections::HashMap::new();
    for (_, group) in costumes.iter().chain(styles.iter()) {
        for opt in group {
            options.insert(format!("{}:{}", opt.slot, opt.key), opt.clone());
        }
    }
    Ok(Catalogue { classes, costumes, styles, options, races })
}

// --- the VFX backdrop's matrices (`embed/backdrop.py`) ---------------------------------

type M4 = [[f64; 4]; 4];

/// `a @ b` as OpenBLAS's gemm kernel rounds it: one fused chain over k.
fn matmul(a: &M4, b: &M4) -> M4 {
    std::array::from_fn(|i| {
        std::array::from_fn(|j| {
            let mut acc = a[i][0] * b[0][j];
            for k in 1..4 {
                acc = a[i][k].mul_add(b[k][j], acc);
            }
            acc
        })
    })
}

fn diag(v: [f64; 4]) -> M4 {
    std::array::from_fn(|i| std::array::from_fn(|j| if i == j { v[i] } else { 0.0 }))
}

/// `np.array(m).reshape(4, 4).T`.
fn from_rest(m: &[f64; 16]) -> M4 {
    std::array::from_fn(|i| std::array::from_fn(|j| m[j * 4 + i]))
}

/// `[round(float(v), 6) for v in m.T.flatten()]`.
fn flat6(m: &M4, round6: impl Fn(f64) -> f64) -> Vec<f64> {
    (0..16).map(|n| round6(m[n % 4][n / 4])).collect()
}

/// `np.linalg.inv(a)` as OpenBLAS's `dgesv` computes it (left-looking
/// `getf2`, then the triangular solves); `None` for a singular matrix.
pub fn inv4(a: &M4) -> Option<M4> {
    const N: usize = 4;
    let mut col: M4 = std::array::from_fn(|j| std::array::from_fn(|i| a[i][j]));
    let mut ipiv = [0usize; N];
    for j in 0..N {
        for (i, &p) in ipiv.iter().enumerate().take(j) {
            col[j].swap(i, p);
        }
        for i in 1..j {
            let d = (0..i).fold(0.0, |d, k| d + col[k][i] * col[j][k]);
            col[j][i] -= d;
        }
        for i in j..N {
            let acc = (0..j).fold(0.0, |acc, k| acc + col[k][i] * col[j][k]);
            col[j][i] -= acc;
        }
        let mut jp = j;
        for i in j + 1..N {
            if col[j][i].abs() > col[j][jp].abs() {
                jp = i;
            }
        }
        ipiv[j] = jp;
        if jp != j {
            for c in col.iter_mut().take(j + 1) {
                c.swap(j, jp);
            }
        }
        if col[j][j] == 0.0 {
            return None;
        }
        let r = 1.0 / col[j][j];
        for v in col[j].iter_mut().skip(j + 1) {
            *v *= r;
        }
    }
    let lu = |i: usize, k: usize| col[k][i];
    let inv: [f64; N] = std::array::from_fn(|i| 1.0 / lu(i, i));
    let mut x = [[0.0; N]; N];
    for (c, out) in x.iter_mut().enumerate() {
        let mut b: [f64; N] = std::array::from_fn(|i| if i == c { 1.0 } else { 0.0 });
        for (i, &p) in ipiv.iter().enumerate() {
            b.swap(i, p);
        }
        for k in 0..N {
            for i in k + 1..N {
                b[i] -= lu(i, k) * b[k];
            }
        }
        for i in (0..N).rev() {
            let xi = b[i] * inv[i];
            for (r, br) in b.iter_mut().enumerate().take(i) {
                *br -= lu(r, i) * xi;
            }
            out[i] = xi;
        }
    }
    Some(std::array::from_fn(|r| std::array::from_fn(|c| x[c][r])))
}

/// A weapon's matrix: `ORIENT.get(family, eye(4)) @ diag([0.5 / 12] * 3 + [1])`.
pub fn weapon_matrix(gun: bool, round6: impl Fn(f64) -> f64) -> Vec<f64> {
    let s = 0.5 / 12.0;
    let orient: M4 = if gun {
        [[1.0, 0.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, -1.0, 0.0, 0.0], [0.0, 0.0, 0.0, 1.0]]
    } else {
        diag([1.0; 4])
    };
    flat6(&matmul(&orient, &diag([s, s, s, 1.0])), round6)
}

/// A head part's matrix: `diag(0.5) @ inv(rest[hat]) @ rest[part] @ diag(size)`.
pub fn head_matrix(hat: &[f64; 16], part: &[f64; 16], size: f64, round6: impl Fn(f64) -> f64) -> Option<Vec<f64>> {
    let to_hat = matmul(&diag([0.5, 0.5, 0.5, 1.0]), &inv4(&from_rest(hat))?);
    let m = matmul(&matmul(&to_hat, &from_rest(part)), &diag([size, size, size, 1.0]));
    Some(flat6(&m, round6))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helpers() {
        assert_eq!(humanize("c_p_hair_long_bob[kiwi]"), "Long Bob");
        assert_eq!(humanize("[x]"), "[x]");
        assert_eq!(stem("Prefabs\\Skins\\Knight_A.binfab"), "knight_a");
        assert!(covers_head("equipment_hat_knight_helm"));
        assert!(covers_head("x_helm_02"));
        assert!(!covers_head("x_helmet"));
        assert_eq!(blueprint_ref(" A/B.Blueprint "), Some("a/b".into()));
        assert_eq!(blueprint_ref("a/../b"), None);
        assert_eq!(blueprint_ref("/a"), None);
        assert_eq!(color_ref("#FFa000"), Some([255, 160, 0]));
        assert_eq!(color_ref("ffa0001"), None);
        assert_eq!(piece_kind("c_p_head_eyes"), Some("head"));
    }
}

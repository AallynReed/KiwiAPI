//! `lint.py`: the Trove Creations checks, run on a compiled blueprint.

use std::collections::{HashMap, HashSet, VecDeque};

use serde_json::{Value, json};

use super::{
    codec::{self, Doc},
    materials,
    py::commas,
};

pub const DIMENSIONS: [(&str, [i64; 3]); 10] = [
    ("melee", [10, 10, 35]),
    ("gun", [5, 12, 5]),
    ("staff", [12, 12, 35]),
    ("bow", [3, 9, 21]),
    ("fist", [6, 9, 5]),
    ("spear", [11, 11, 45]),
    ("mask", [10, 10, 5]),
    ("hat", [20, 14, 20]),
    ("hair", [20, 14, 20]),
    ("deco", [12, 12, 12]),
];

const NEEDS_ATTACHMENT: [&str; 9] = ["melee", "gun", "staff", "bow", "spear", "fist", "mask", "hat", "hair"];

/// `CREATION_TYPES`.
pub fn creation_types() -> Vec<&'static str> {
    DIMENSIONS.iter().map(|(k, _)| *k).chain(["other"]).collect()
}

struct Finding {
    level: &'static str,
    title: String,
    body: String,
    voxels: Vec<usize>,
}

fn finding(level: &'static str, title: impl Into<String>, body: impl Into<String>) -> Finding {
    Finding { level, title: title.into(), body: body.into(), voxels: Vec::new() }
}

fn plural(n: usize) -> &'static str {
    if n != 1 { "s are" } else { " is" }
}

fn dimensions(kind: &str, [sx, sy, sz]: [i64; 3]) -> Vec<Finding> {
    let Some((_, [lx, ly, lz])) = DIMENSIONS.iter().find(|(k, _)| *k == kind) else { return Vec::new() };
    let (lx, ly, lz) = (*lx, *ly, *lz);
    if sx <= lx && sy <= ly && sz <= lz {
        return Vec::new();
    }
    let rotations = [(sx, sz, sy), (sy, sx, sz), (sy, sz, sx), (sz, sx, sy), (sz, sy, sx)];
    if rotations.iter().any(|&(a, b, c)| a <= lx && b <= ly && c <= lz) {
        return vec![finding(
            "error",
            "The model is facing the wrong way",
            format!(
                "A {kind} should fit within {lx}x{ly}x{lz}, and yours ({sx}x{sy}x{sz}) would if it were turned. Use the \
                 rotate buttons under 'Turn it' - weapons point forward along the long axis, hats point up."
            ),
        )];
    }
    vec![finding(
        "error",
        "The model is too big",
        format!("A {kind} should not exceed {lx}x{ly}x{lz} voxels, but yours is {sx}x{sy}x{sz}."),
    )]
}

/// Cells in first-insertion order, each with the last voxel index that held it.
struct CellIndex {
    at: HashMap<(i64, i64, i64), usize>,
    order: Vec<((i64, i64, i64), usize)>,
}

impl CellIndex {
    fn new(doc: &Doc) -> Self {
        let mut at: HashMap<(i64, i64, i64), usize> = HashMap::with_capacity(doc.voxels.len());
        let mut order: Vec<((i64, i64, i64), usize)> = Vec::with_capacity(doc.voxels.len());
        for (i, v) in doc.voxels.iter().enumerate() {
            let k = (v.x, v.y, v.z);
            match at.get(&k) {
                Some(&slot) => order[slot].1 = i,
                None => {
                    at.insert(k, order.len());
                    order.push((k, i));
                }
            }
        }
        CellIndex { at, order }
    }

    fn contains(&self, k: &(i64, i64, i64)) -> bool {
        self.at.contains_key(k)
    }
}

fn floating(cells: &CellIndex) -> Vec<Finding> {
    let Some(&(start, _)) = cells.order.first() else { return Vec::new() };
    let mut seen = HashSet::from([start]);
    let mut queue = VecDeque::from([start]);
    while let Some((x, y, z)) = queue.pop_front() {
        for n in [(x + 1, y, z), (x - 1, y, z), (x, y + 1, z), (x, y - 1, z), (x, y, z + 1), (x, y, z - 1)] {
            if cells.contains(&n) && seen.insert(n) {
                queue.push_back(n);
            }
        }
    }
    let stray: Vec<usize> = cells.order.iter().filter(|(c, _)| !seen.contains(c)).map(|(_, i)| *i).collect();
    if stray.is_empty() {
        return Vec::new();
    }
    let mut f = finding(
        "warning",
        format!("{} voxel{} not connected to the model", commas(stray.len() as i64), plural(stray.len())),
        "These only touch the rest of the model at a corner, or not at all. Trove renders them, but the style \
         guide asks for face-connected geometry.",
    );
    f.voxels = stray;
    vec![f]
}

fn attachment(kind: &str, attach: Option<[i64; 3]>, [sx, sy, sz]: [i64; 3], cells: &CellIndex) -> Vec<Finding> {
    let mut out = Vec::new();
    let Some([ax, ay, az]) = attach else {
        if NEEDS_ATTACHMENT.contains(&kind) {
            out.push(finding(
                "warning",
                "This model stores no attachment point",
                "Older blueprints (format v3/v4) don't record an origin, so there's nothing to check and nothing to \
                 show. The game positions these by other means.",
            ));
        }
        return out;
    };
    if ["melee", "staff", "bow", "spear", "gun", "fist"].contains(&kind) && !cells.contains(&(ax, ay, az)) {
        out.push(finding(
            "warning",
            "The attachment point isn't on a voxel",
            format!(
                "The hand grips ({ax}, {ay}, {az}), but there's no voxel there. On a weapon the attachment point should \
                 sit inside the handle."
            ),
        ));
    }
    if ["melee", "staff", "bow"].contains(&kind) {
        let limit = if kind == "bow" { 3 } else { 4 };
        if ay > limit || sy - ay > 6 {
            out.push(finding(
                "warning",
                "The handle sits too high or too low",
                format!(
                    "There shouldn't be more than 5 voxels above or {limit} below the attachment point; yours has {} \
                     above and {ay} below.",
                    sy - ay - 1
                ),
            ));
        }
        let blocked: Vec<usize> = cells
            .order
            .iter()
            .filter(|((x, y, z), _)| {
                (x - ax).abs() <= 1 && (y - ay).abs() <= 1 && (z - az).abs() <= 1 && !(*x == ax && *y == ay)
            })
            .map(|(_, i)| *i)
            .collect();
        if !blocked.is_empty() {
            let mut f = finding(
                "warning",
                "The grip is crowded",
                "Around the attachment point there should be nothing in a 3x3x3 cube except the handle running \
                 lengthwise - that space is the hand.",
            );
            f.voxels = blocked;
            out.push(f);
        }
    }
    if kind == "staff" {
        if !(8..=14).contains(&az) {
            out.push(finding(
                "warning",
                "The staff handle is the wrong length",
                format!("The handle behind the attachment point should be 8-14 voxels; yours is {az}."),
            ));
        }
        if sz - az < 17 {
            out.push(finding(
                "warning",
                "The staff head is too close to the grip",
                format!(
                    "There should be at least 16 voxels between the attachment point and the tip; yours has {}.",
                    sz - az - 1
                ),
            ));
        }
    }
    if kind == "spear" && !(8..=12).contains(&az) {
        out.push(finding(
            "warning",
            "The spear grip is in the wrong place",
            format!("The attachment point should be 9-13 voxels from the base of the shaft; yours is {}.", az + 1),
        ));
    }
    if kind == "gun" && az != 0 {
        out.push(finding(
            "warning",
            "There's geometry behind the grip",
            format!("A gun's attachment point should sit at the back of the model (z = 0); yours is at z = {az}."),
        ));
    }
    if kind == "hat" || kind == "hair" {
        if ay > -6 {
            out.push(finding(
                "error",
                "The model isn't high enough above its attachment point",
                format!(
                    "The attachment point should be at least 6 voxels below the model (the head fills that space); \
                     yours is {}.",
                    if ay < 0 { -ay } else { 0 }
                ),
            ));
        }
        if ax > 10 || sx - ax > 10 || az > 9 || sz - az > 11 {
            out.push(finding(
                "warning",
                "The model is off-centre over the head",
                format!(
                    "There shouldn't be more than ~10 voxels to any side of the attachment point; yours reaches {ax} \
                     right, {} left, {az} back, {} front.",
                    sx - ax - 1,
                    sz - az - 1
                ),
            ));
        }
    }
    if kind == "mask" {
        if az > -6 {
            out.push(finding(
                "error",
                "The mask isn't far enough in front of its attachment point",
                format!(
                    "The attachment point should be 6 voxels behind the mask (that space is the head); yours is {}.",
                    if az < 0 { -az } else { 0 }
                ),
            ));
        }
        if ax > 5 || sx - ax > 5 || ay > 4 || sy - ay > 6 {
            out.push(finding(
                "warning",
                "The mask is off-centre on the face",
                format!(
                    "There shouldn't be more than ~5 voxels to any side of the attachment point; yours reaches {ax} \
                     right, {} left, {} up, {ay} down.",
                    sx - ax - 1,
                    sy - ay - 1
                ),
            ));
        }
    }
    out
}

/// `lint.check(decoded, kind)`.
pub fn check(doc: &Doc, kind: &str) -> Value {
    let kind = if creation_types().contains(&kind) { kind } else { "other" };
    let attach = codec::attachment(doc);
    let cells = CellIndex::new(doc);

    let mut findings: Vec<Finding> = Vec::new();
    findings.extend(dimensions(kind, doc.size));
    if kind == "spear" && doc.size[2] != 45 {
        findings.push(finding(
            "error",
            "A spear has to be exactly 45 voxels long",
            format!("Yours is {}. The game positions the shaft assuming that length.", doc.size[2]),
        ));
    }
    if kind == "bow" && 3 < doc.size[0] && doc.size[0] <= 5 {
        findings.push(finding(
            "warning",
            "This bow is thicker than the guidelines allow",
            format!(
                "A bow should be at most 3 voxels thick; yours is {}. Up to 5 is sometimes accepted if the design \
                 really needs it.",
                doc.size[0]
            ),
        ));
    }
    let dark: Vec<usize> = doc
        .voxels
        .iter()
        .enumerate()
        .filter(|(_, v)| {
            !materials::is_procedural(v.ty, v.r, v.g, v.b)
                && u16::from(v.r.min(v.g).min(v.b)) + u16::from(v.r.max(v.g).max(v.b)) < 20
        })
        .map(|(i, _)| i)
        .collect();
    if !dark.is_empty() {
        let mut f = finding(
            "error",
            format!("{} voxel{} almost black", commas(dark.len() as i64), plural(dark.len())),
            "Trove's style guide asks for nothing darker than (10, 10, 10) - pure black reads as a hole in game. Use \
             a dark grey instead.",
        );
        f.voxels = dark;
        findings.push(f);
    }
    if kind != "deco" {
        findings.extend(floating(&cells));
    }
    if doc.voxels.iter().all(|v| v.ty == 21 && v.w == 0) {
        findings.push(finding(
            "info",
            "No materials are used",
            "Every voxel is a plain rough solid. Glass, glowing and the metal/waxy/iridescent finishes are what give a \
             model depth in game - worth trying where they suit the design.",
        ));
    }
    if kind != "other" {
        findings.extend(attachment(kind, attach, doc.size, &cells));
    }
    let rank = |l: &str| match l {
        "error" => 0,
        "warning" => 1,
        _ => 2,
    };
    findings.sort_by_key(|f| rank(f.level));

    let mut satisfied = vec![
        "A blueprint stores one material per voxel, so alpha-on-solid and specular-on-glass - two of the classic \
         .qb mistakes - can't happen here."
            .to_owned(),
    ];
    if !findings.iter().any(|f| f.title.ends_with("almost black")) {
        satisfied.push("No near-black voxels.".into());
    }
    if let Some(a) = attach.filter(|_| NEEDS_ATTACHMENT.contains(&kind)) {
        satisfied.push(format!("Attachment point recorded at ({}, {}, {}).", a[0], a[1], a[2]));
    }
    let count = |l: &str| findings.iter().filter(|f| f.level == l).count();
    let counts = json!({"error": count("error"), "warning": count("warning"), "info": count("info")});
    let findings: Vec<Value> = findings
        .into_iter()
        .map(|f| json!({"level": f.level, "title": f.title, "body": f.body, "voxels": f.voxels}))
        .collect();
    json!({
        "kind": kind,
        "attachment": attach,
        "findings": findings,
        "satisfied": satisfied,
        "counts": counts,
    })
}

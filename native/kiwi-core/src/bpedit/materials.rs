//! `materials.py`: the `(type, w)` material model the editor edits.

use serde_json::{Value, json};

use super::{py::round_half_even, qb};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Class {
    Solid,
    Glass,
}

/// `PALETTE_TYPES`, in order.
pub const PALETTE: [(u16, &str, Class); 5] = [
    (21, "Solid", Class::Solid),
    (18, "Glass", Class::Glass),
    (54, "Tiled Glass", Class::Glass),
    (55, "Glowing Solid", Class::Solid),
    (56, "Glowing Glass", Class::Glass),
];

pub const SPECULAR_NAMES: [&str; 6] = ["Rough", "Metal", "Water", "Iridescent", "Waxy", "Wave"];
pub const NO_SPECULAR: u16 = 55;
pub const GLASS_LEVELS: i64 = 8;
pub const PLACEHOLDER_TYPE: u16 = 39;
const AUTHORED: [u16; 6] = [21, 18, 54, 55, 56, 24];

const INTERNAL_NAMES: [(u16, &str); 13] = [
    (1, "Terrain (dirt)"),
    (2, "Terrain (grass)"),
    (3, "Terrain (stone)"),
    (4, "Terrain (shadow)"),
    (24, "Placeable colour"),
    (28, "Decorative"),
    (38, "Trigger"),
    (39, "Deco placeholder"),
    (59, "Attachment"),
    (79, "Terrain (ice)"),
    (94, "Collision"),
    (100, "Terrain (snow)"),
    (174, "Terrain (lava)"),
];

fn palette_entry(t: i64) -> Option<(&'static str, Class)> {
    PALETTE.iter().find(|(p, _, _)| i64::from(*p) == t).map(|(_, l, c)| (*l, *c))
}

pub fn alpha_for_w(w: i64) -> i64 {
    16 + 32 * w.clamp(0, GLASS_LEVELS - 1)
}

fn opacity_pct(w: i64) -> i64 {
    round_half_even(alpha_for_w(w) as f64 / 255.0 * 100.0)
}

pub fn is_procedural(t: u16, r: u8, g: u8, b: u8) -> bool {
    !AUTHORED.contains(&t) && r.max(g).max(b) <= 24
}

pub fn class(t: u16) -> Class {
    palette_entry(i64::from(t)).map(|(_, c)| c).unwrap_or(Class::Solid)
}

pub fn is_editable(t: u16) -> bool {
    palette_entry(i64::from(t)).is_some()
}

/// `describe(type, w)`.
pub fn describe(t: i64, w: i64) -> String {
    if let Some((label, cls)) = palette_entry(t) {
        if cls == Class::Glass {
            return format!("{label} - {}% opacity", opacity_pct(w));
        }
        if t == i64::from(NO_SPECULAR) {
            return if w == 0 { label.to_owned() } else { format!("{label} - unused finish {w}") };
        }
        let finish = usize::try_from(w)
            .ok()
            .and_then(|i| SPECULAR_NAMES.get(i))
            .map(|s| (*s).to_owned())
            .unwrap_or_else(|| format!("finish {w}"));
        return format!("{label} - {finish}");
    }
    match INTERNAL_NAMES.iter().find(|(k, _)| i64::from(*k) == t) {
        Some((_, name)) => format!("{name} (type {t})"),
        None => format!("Unknown type {t}"),
    }
}

fn hex(rgb: (u8, u8, u8)) -> String {
    format!("#{:02x}{:02x}{:02x}", rgb.0, rgb.1, rgb.2)
}

/// `palette()`.
pub fn palette() -> Value {
    let types: Vec<Value> = PALETTE
        .iter()
        .map(|&(t, label, cls)| {
            let options: Vec<Value> = if cls == Class::Glass {
                (0..GLASS_LEVELS)
                    .map(|w| {
                        let a = alpha_for_w(w) as u8;
                        json!({"w": w, "label": format!("{}%", opacity_pct(w)), "alpha": alpha_for_w(w),
                               "qb_map": "_a", "qb": hex((a, a, a))})
                    })
                    .collect()
            } else if t == NO_SPECULAR {
                vec![json!({"w": 0, "label": SPECULAR_NAMES[0]})]
            } else {
                SPECULAR_NAMES
                    .iter()
                    .enumerate()
                    .map(|(w, name)| {
                        let q = qb::specular_rgb(w as i64).map(hex);
                        json!({"w": w, "label": name, "qb_map": "_s", "qb": q})
                    })
                    .collect()
            };
            let q = qb::type_index(t).and_then(qb::type_rgb).map(hex);
            json!({"type": t, "label": label, "class": if cls == Class::Glass { "glass" } else { "solid" },
                   "options": options, "qb_map": "_t", "qb": q})
        })
        .collect();
    json!({"types": types, "glass_levels": GLASS_LEVELS})
}

/// `validate_material`: `Err` is the `ValueError` text.
pub fn validate(t: i64, w: i64) -> Result<(u16, u8), String> {
    let Some((_, cls)) = palette_entry(t) else {
        return Err(format!("Type {t} is not an editable material."));
    };
    let w = if cls == Class::Glass {
        w.clamp(0, GLASS_LEVELS - 1)
    } else if t == i64::from(NO_SPECULAR) {
        0
    } else {
        w.clamp(0, SPECULAR_NAMES.len() as i64 - 1)
    };
    Ok((t as u16, w as u8))
}

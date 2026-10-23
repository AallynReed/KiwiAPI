//! `app/trove/render/voxel.py`: the viewer payload (`pack_blueprint`) and the
//! catalog-style thumbnail render (`render_blueprint_png(..., contain=True)`).

pub mod pillow;
pub mod raster;

#[cfg(test)]
mod tests;

use serde_json::{Value, json};

use crate::{
    blueprint::{self, Rendered},
    bpedit::{Fault, codec},
};

pub const RENDER_VOXEL_CAP: usize = 250_000;

/// Why `pack_blueprint` raised.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PackError {
    /// `BlueprintEmpty`.
    Empty,
    /// `BlueprintTooLarge` (the voxel count).
    TooLarge(usize),
    /// Any other `BlueprintError`.
    Invalid(String),
    /// Not a `BlueprintError`: a 500.
    Unhandled(String),
}

/// `decode_render_map`: the render cells in dict order, or the codec's fault.
pub fn render_map(raw: &[u8]) -> Result<Vec<Rendered>, Fault> {
    let doc = codec::decode(raw)?;
    Ok(blueprint::render_cells(&doc.voxels))
}

/// `pack_blueprint(raw, path, cap=cap)`.
pub fn pack(raw: &[u8], path: &str, cap: usize) -> Result<Value, PackError> {
    if codec::is_empty(raw) {
        return Err(PackError::Empty);
    }
    let cells = render_map(raw).map_err(|f| match f {
        Fault::Blueprint(m) => PackError::Invalid(m),
        other => PackError::Unhandled(other.message().to_owned()),
    })?;
    if cells.len() > cap {
        return Err(PackError::TooLarge(cells.len()));
    }
    let n = cells.len();
    let (mut xs, mut ys, mut zs) = (Vec::with_capacity(n), Vec::with_capacity(n), Vec::with_capacity(n));
    let (mut rgb, mut kind, mut level, mut spec) =
        (Vec::with_capacity(n), Vec::with_capacity(n), Vec::with_capacity(n), Vec::with_capacity(n));
    for c in &cells {
        xs.push(c.x);
        ys.push(c.y);
        zs.push(c.z);
        rgb.push((u32::from(c.r) << 16) | (u32::from(c.g) << 8) | u32::from(c.b));
        kind.push(c.kind);
        level.push(c.level);
        spec.push(c.spec);
    }
    let extent = |v: &[i64]| v.iter().max().copied().unwrap_or(0) - v.iter().min().copied().unwrap_or(0) + 1;
    let mut out = json!({
        "path": path,
        "count": n,
        "size": [extent(&xs), extent(&ys), extent(&zs)],
        "x": xs, "y": ys, "z": zs, "rgb": rgb, "kind": kind, "level": level,
    });
    if spec.iter().any(|&s| s != 0) {
        out["spec"] = json!(spec);
    }
    Ok(out)
}

/// `render_blueprint_png(raw, dim, None, contain=True)` up to the PNG encode:
/// the RGBA pixels, or the codec's fault.
pub fn render_thumbnail(raw: &[u8], dim: usize) -> Result<Vec<u8>, Fault> {
    let cells = render_map(raw)?;
    let rgba = raster::render(&cells, dim, true).ok_or_else(|| Fault::Blueprint("no voxels to render".into()))?;
    Ok(pillow::contain_square(rgba, dim))
}

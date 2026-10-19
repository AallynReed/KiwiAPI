//! Python bindings for kiwi-core, imported as `kiwi_native`.
//!
//! Functions that return `None` are telling the Python caller to run its own
//! reference implementation, which then raises exactly what it always raised.

use std::collections::HashMap;
use std::sync::RwLock;

use kiwi_core::blueprint::{self, Decoded, InVoxel, Rendered};
use kiwi_core::wire;
use pyo3::intern;
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict, PyList, PyString, PyTuple};

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[pyfunction]
fn calculate_hash(py: Python<'_>, data: &[u8]) -> u32 {
    py.detach(|| kiwi_core::troveio::calculate_hash(data))
}

#[pyfunction]
#[pyo3(signature = (data, min_len=2, max_len=512))]
fn harvest_strings(py: Python<'_>, data: &[u8], min_len: u128, max_len: u128) -> Vec<(usize, u128, String)> {
    py.detach(|| {
        kiwi_core::binfab::harvest_strings(data, min_len, max_len)
            .into_iter()
            .map(|h| (h.offset, h.field, h.text))
            .collect()
    })
}

fn voxel_dicts<'py>(py: Python<'py>, d: &Decoded) -> PyResult<Bound<'py, PyList>> {
    let keys = [
        intern!(py, "x"),
        intern!(py, "y"),
        intern!(py, "z"),
        intern!(py, "r"),
        intern!(py, "g"),
        intern!(py, "b"),
        intern!(py, "w"),
        intern!(py, "type"),
    ];
    let list = PyList::empty(py);
    for v in &d.voxels {
        let dict = PyDict::new(py);
        dict.set_item(keys[0], v.x)?;
        dict.set_item(keys[1], v.y)?;
        dict.set_item(keys[2], v.z)?;
        dict.set_item(keys[3], v.r)?;
        dict.set_item(keys[4], v.g)?;
        dict.set_item(keys[5], v.b)?;
        dict.set_item(keys[6], v.w)?;
        dict.set_item(keys[7], v.ty)?;
        list.append(dict)?;
    }
    Ok(list)
}

/// `(version, size, pos, voxel dicts, entity_blob, offset)` for `DecodedBlueprint`.
#[pyfunction]
fn bp_decode<'py>(py: Python<'py>, version: u32, payload: &[u8]) -> PyResult<Option<Bound<'py, PyTuple>>> {
    let Some(d) = py.detach(|| blueprint::decode_any(version, payload)) else { return Ok(None) };
    let voxels = voxel_dicts(py, &d)?;
    let blob = PyBytes::new(py, &payload[d.entity_start..]);
    let t = (d.version, tuple3(d.size), tuple3(d.pos), voxels, blob, tuple3(d.offset));
    Ok(Some(t.into_pyobject(py)?))
}

fn tuple3(v: [i64; 3]) -> (i64, i64, i64) {
    (v[0], v[1], v[2])
}

const KIND_NAMES: [&str; 4] = ["S", "G", "E", "GE"];

/// `to_render_voxels(decode(...))`: `{(x, y, z): (r, g, b, kind, level, spec)}`.
#[pyfunction]
fn bp_render_map<'py>(py: Python<'py>, version: u32, payload: &[u8]) -> PyResult<Option<Bound<'py, PyDict>>> {
    let Some(cells) = py.detach(|| blueprint::decode_any(version, payload).map(|d| blueprint::render_cells(&d.voxels)))
    else {
        return Ok(None);
    };
    let out = PyDict::new(py);
    for c in &cells {
        out.set_item((c.x, c.y, c.z), (c.r, c.g, c.b, KIND_NAMES[c.kind as usize], c.level, c.spec))?;
    }
    Ok(Some(out))
}

type Columns = (Vec<i64>, Vec<i64>, Vec<i64>, Vec<u32>, Vec<u16>, Vec<u16>, Vec<u16>);

fn columns(cells: &[Rendered]) -> Columns {
    let mut c: Columns = Default::default();
    for v in cells {
        c.0.push(v.x);
        c.1.push(v.y);
        c.2.push(v.z);
        c.3.push((u32::from(v.r) << 16) | (u32::from(v.g) << 8) | u32::from(v.b));
        c.4.push(u16::from(v.kind));
        c.5.push(v.level);
        c.6.push(u16::from(v.spec));
    }
    c
}

/// `pack_blueprint`'s parallel arrays: `(x, y, z, rgb, kind, level, spec)`.
#[pyfunction]
fn bp_render_columns(py: Python<'_>, version: u32, payload: &[u8]) -> Option<Columns> {
    py.detach(|| blueprint::decode_any(version, payload).map(|d| columns(&blueprint::render_cells(&d.voxels))))
}

type GridRow = (i64, i64, i64, u32, u8, u16, u8);

/// `assembly._decode_grid` as `[(x, y, z, rgb, kind, level, spec)]`.
#[pyfunction]
fn bp_grid(py: Python<'_>, version: u32, payload: &[u8]) -> Option<Vec<GridRow>> {
    py.detach(|| {
        blueprint::grid(version, payload).map(|cells| {
            cells
                .iter()
                .map(|v| {
                    let rgb = (u32::from(v.r) << 16) | (u32::from(v.g) << 8) | u32::from(v.b);
                    (v.x, v.y, v.z, rgb, v.kind, v.level, v.spec)
                })
                .collect()
        })
    })
}

/// `assembly._decode_grid` as parallel arrays.
#[pyfunction]
fn bp_grid_columns(py: Python<'_>, version: u32, payload: &[u8]) -> Option<Columns> {
    py.detach(|| blueprint::grid(version, payload).map(|cells| columns(&cells)))
}

fn opt_int(d: &Bound<'_, PyDict>, key: &Bound<'_, pyo3::types::PyString>) -> Result<Option<i64>, ()> {
    match d.get_item(key).map_err(|_| ())? {
        None => Ok(None),
        Some(v) => v.extract::<i64>().map(Some).map_err(|_| ()),
    }
}

fn read_voxels(py: Python<'_>, voxels: &Bound<'_, PyAny>) -> Option<Vec<InVoxel>> {
    let list = voxels.cast::<PyList>().ok()?;
    let (kx, ky, kz) = (intern!(py, "x"), intern!(py, "y"), intern!(py, "z"));
    let (kr, kg, kb) = (intern!(py, "r"), intern!(py, "g"), intern!(py, "b"));
    let (kw, kt) = (intern!(py, "w"), intern!(py, "type"));
    let mut out = Vec::with_capacity(list.len());
    for item in list.iter() {
        let d = item.cast::<PyDict>().ok()?;
        let req = |k| opt_int(d, k).ok().flatten();
        out.push(InVoxel::new(
            req(kx)?,
            req(ky)?,
            req(kz)?,
            req(kr)?,
            req(kg)?,
            req(kb)?,
            opt_int(d, kw).ok()?,
            opt_int(d, kt).ok()?,
        ));
    }
    Some(out)
}

/// `codec.encode` minus the v5 zlib step. None = let Python handle it.
#[pyfunction]
#[pyo3(signature = (voxels, version, size, pos, offset, entity_blob))]
fn bp_encode<'py>(
    py: Python<'py>,
    voxels: &Bound<'py, PyAny>,
    version: &Bound<'py, PyAny>,
    size: &Bound<'py, PyAny>,
    pos: &Bound<'py, PyAny>,
    offset: &Bound<'py, PyAny>,
    entity_blob: &[u8],
) -> Option<Bound<'py, PyBytes>> {
    let triple = |o: &Bound<'py, PyAny>| -> Option<Option<[i64; 3]>> {
        if o.is_none() {
            return Some(None);
        }
        let (a, b, c) = o.extract::<(i64, i64, i64)>().ok()?;
        Some(Some([a, b, c]))
    };
    let version = version.extract::<u32>().ok()?;
    let size = triple(size)?;
    let pos = triple(pos)?;
    let offset = triple(offset)?.unwrap_or([0, 0, 0]);
    let voxels = read_voxels(py, voxels)?;
    let bytes = py.detach(|| blueprint::encode(&voxels, version, size, pos, offset, entity_blob))?;
    Some(PyBytes::new(py, &bytes))
}

/// `wem.convert`: `(bytes, mime, extension)`, or None to let Python handle it.
#[pyfunction]
#[pyo3(signature = (raw, codebooks=None))]
fn wem_convert<'py>(py: Python<'py>, raw: &[u8], codebooks: Option<&[u8]>) -> Option<(Bound<'py, PyBytes>, &'static str, &'static str)> {
    let c = py.detach(|| kiwi_core::audio::convert(raw, codebooks))?;
    Some((PyBytes::new(py, &c.bytes), c.mime, c.ext))
}

type MatrixTuple = (f64, f64, f64, f64, i64, i64);

fn matrix_tuple(m: kiwi_core::swf::Matrix) -> MatrixTuple {
    (m.sx, m.sy, m.r0, m.r1, m.tx, m.ty)
}

/// `_decode_lossless` pixels after inflation, as RGBA bytes.
#[pyfunction]
fn swf_lossless<'py>(
    py: Python<'py>,
    fmt: u8,
    has_alpha: bool,
    width: usize,
    height: usize,
    colors: usize,
    data: &[u8],
) -> Option<Bound<'py, PyBytes>> {
    let px = py.detach(|| kiwi_core::swf::lossless_pixels(fmt, has_alpha, width, height, colors, data))?;
    Some(PyBytes::new(py, &px))
}

/// `_shape_bitmap_fills` as `[(bitmap id, (sx, sy, r0, r1, tx, ty))]`.
#[pyfunction]
fn swf_shape_fills(body: &[u8], version: u32) -> Option<Vec<(u16, MatrixTuple)>> {
    kiwi_core::swf::shape_bitmap_fills(body, version)
        .map(|v| v.into_iter().map(|(id, m)| (id, matrix_tuple(m))).collect())
}

/// PlaceObject bit fields: `(matrix or None, bytes consumed)`.
#[pyfunction]
fn swf_place_bits(data: &[u8], has_matrix: bool, has_cxform: bool) -> Option<(Option<MatrixTuple>, usize)> {
    kiwi_core::swf::place_bits(data, has_matrix, has_cxform).map(|(m, n)| (m.map(matrix_tuple), n))
}

fn wire_value<'py>(py: Python<'py>, v: &wire::Val, obj_cls: &Bound<'py, PyAny>, rep_cls: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
    stacker::maybe_grow(64 * 1024, 4 * 1024 * 1024, || {
        let conv = |x: &wire::Val| wire_value(py, x, obj_cls, rep_cls);
        Ok(match v {
            wire::Val::Int(i) => i.into_pyobject(py)?.into_any(),
            wire::Val::Float(f) => f.into_pyobject(py)?.into_any(),
            wire::Val::Str(s) => PyString::new(py, s).into_any(),
            wire::Val::Bytes(b) => PyBytes::new(py, b).into_any(),
            wire::Val::Packed(xs) => PyTuple::new(py, xs.iter().map(conv).collect::<PyResult<Vec<_>>>()?)?.into_any(),
            wire::Val::Sec(chain) => {
                let d = PyDict::new(py);
                for (idx, xs) in wire::section_fields(chain) {
                    let val = if xs.len() == 1 {
                        conv(&xs[0])?
                    } else {
                        let items = xs.iter().map(|s| conv(s)).collect::<PyResult<Vec<_>>>()?;
                        rep_cls.call1((PyList::new(py, items)?,))?
                    };
                    d.set_item(idx, val)?;
                }
                d.into_any()
            }
            wire::Val::Obj(chain) => {
                let secs = wire::obj_sections(chain);
                let items = secs.iter().map(|s| conv(s)).collect::<PyResult<Vec<_>>>()?;
                obj_cls.call1((PyList::new(py, items)?,))?
            }
            wire::Val::List(chain) => {
                let items = wire::list_values(chain)
                    .iter()
                    .map(|x| match x {
                        Some(x) => conv(x),
                        None => Ok(py.None().into_bound(py)),
                    })
                    .collect::<PyResult<Vec<_>>>()?;
                PyList::new(py, items)?.into_any()
            }
            wire::Val::Map(chain) => {
                let d = PyDict::new(py);
                for (k, x) in wire::map_items(chain) {
                    let val = match x {
                        Some(x) => conv(&x)?,
                        None => py.None().into_bound(py),
                    };
                    match k {
                        wire::MapKey::Int(i) => d.set_item(i, val)?,
                        wire::MapKey::Str(s) => d.set_item(&*s, val)?,
                    }
                }
                d.into_any()
            }
        })
    })
}

/// `wire.parse`: `("entity", type_id, [(cid, Obj)])`, `("object", Obj)`,
/// `("none",)` where Python raises WireError, or None when the budget or the
/// nesting limit stopped it.
static WIRE_SECTIONS: RwLock<Option<HashMap<i128, i128>>> = RwLock::new(None);

/// Install `COMPONENT_SECTIONS` once, so each parse doesn't convert the dict.
#[pyfunction]
fn wire_set_sections(sections: HashMap<i128, i128>) {
    *WIRE_SECTIONS.write().unwrap_or_else(|e| e.into_inner()) = Some(sections);
}

#[pyfunction]
fn wire_parse<'py>(
    py: Python<'py>,
    data: &[u8],
    budget: u64,
    obj_cls: &Bound<'py, PyAny>,
    rep_cls: &Bound<'py, PyAny>,
) -> PyResult<Option<Bound<'py, PyTuple>>> {
    let parsed = {
        let guard = WIRE_SECTIONS.read().unwrap_or_else(|e| e.into_inner());
        let empty = HashMap::new();
        wire::parse(data, guard.as_ref().unwrap_or(&empty), budget)
    };
    let out = match parsed {
        wire::Outcome::Abort => return Ok(None),
        wire::Outcome::NoReading => ("none",).into_pyobject(py)?,
        wire::Outcome::Parsed(wire::Prefab::Object(v)) => ("object", wire_value(py, &v, obj_cls, rep_cls)?).into_pyobject(py)?,
        wire::Outcome::Parsed(wire::Prefab::Entity { type_id, components }) => {
            let comps = components
                .iter()
                .map(|(cid, v)| Ok((*cid, wire_value(py, v, obj_cls, rep_cls)?)))
                .collect::<PyResult<Vec<_>>>()?;
            ("entity", type_id, comps).into_pyobject(py)?
        }
    };
    Ok(Some(out))
}

#[pymodule]
fn kiwi_native(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(wire_parse, m)?)?;
    m.add_function(wrap_pyfunction!(wire_set_sections, m)?)?;
    m.add_function(wrap_pyfunction!(swf_lossless, m)?)?;
    m.add_function(wrap_pyfunction!(swf_shape_fills, m)?)?;
    m.add_function(wrap_pyfunction!(swf_place_bits, m)?)?;
    m.add_function(wrap_pyfunction!(wem_convert, m)?)?;
    m.add_function(wrap_pyfunction!(calculate_hash, m)?)?;
    m.add_function(wrap_pyfunction!(harvest_strings, m)?)?;
    m.add_function(wrap_pyfunction!(bp_decode, m)?)?;
    m.add_function(wrap_pyfunction!(bp_render_map, m)?)?;
    m.add_function(wrap_pyfunction!(bp_render_columns, m)?)?;
    m.add_function(wrap_pyfunction!(bp_grid, m)?)?;
    m.add_function(wrap_pyfunction!(bp_grid_columns, m)?)?;
    m.add_function(wrap_pyfunction!(bp_encode, m)?)?;
    Ok(())
}

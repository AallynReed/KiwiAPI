//! Python bindings for kiwi-core, imported as `kiwi_native`.
//!
//! Functions that return `None` are telling the Python caller to run its own
//! reference implementation, which then raises exactly what it always raised.

use kiwi_core::blueprint::{self, Decoded, InVoxel, Rendered};
use pyo3::intern;
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict, PyList, PyTuple};

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

#[pymodule]
fn kiwi_native(m: &Bound<'_, PyModule>) -> PyResult<()> {
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

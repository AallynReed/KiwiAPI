//! Python bindings for kiwi-core, imported as `kiwi_native`.
//!
//! Functions that return `None` are telling the Python caller to run its own
//! reference implementation, which then raises exactly what it always raised.

use std::collections::HashMap;
use std::sync::RwLock;

use kiwi_core::blueprint::{self, Decoded, InVoxel, Rendered};
use kiwi_core::detection;
use kiwi_core::wire;
use pyo3::intern;
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict, PyFloat, PyList, PyString, PyTuple};

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

/// A score Python compares exactly: an int within 2^53, or a float.
fn exact_f64(v: &Bound<'_, PyAny>) -> Option<f64> {
    if v.is_instance_of::<PyFloat>() {
        return v.extract::<f64>().ok();
    }
    let i = v.extract::<i64>().ok()?;
    (i.unsigned_abs() <= 1 << 53).then_some(i as f64)
}

/// `_board_families_py` over the board's entry dicts: `[(canonical stem, entry
/// indices)]`, or None when an entry can't be read exactly.
#[pyfunction]
fn det_board_families(
    py: Python<'_>,
    entries: &Bound<'_, PyAny>,
    min_size: usize,
    band_pct: f64,
    max_edit: i64,
    stem_fn: &Bound<'_, PyAny>,
) -> PyResult<Option<Vec<detection::Family>>> {
    let Ok(entries) = entries.cast::<PyList>() else { return Ok(None) };
    let (k_name, k_score, k_rank) = (intern!(py, "player_name"), intern!(py, "score"), intern!(py, "rank"));
    let n = entries.len();
    let (mut names, mut stems, mut scores, mut ranks) =
        (Vec::with_capacity(n), Vec::with_capacity(n), Vec::with_capacity(n), Vec::with_capacity(n));
    let mut cache: HashMap<String, String> = HashMap::new();
    for e in entries.iter() {
        let Ok(e) = e.cast_into::<PyDict>() else { return Ok(None) };
        let (Some(name), Some(score), Some(rank)) = (e.get_item(k_name)?, e.get_item(k_score)?, e.get_item(k_rank)?)
        else {
            return Ok(None);
        };
        let (Ok(name), Some(score), Ok(rank)) = (name.extract::<String>(), exact_f64(&score), rank.extract::<i64>())
        else {
            return Ok(None);
        };
        let stem = match detection::name_stem(&name) {
            Some(s) => s,
            None => match cache.get(&name) {
                Some(s) => s.clone(),
                None => {
                    let s = stem_fn.call1((&name,))?.extract::<String>()?;
                    cache.insert(name.clone(), s.clone());
                    s
                }
            },
        };
        names.push(name);
        stems.push(stem);
        scores.push(score);
        ranks.push(rank);
    }
    let rows: Vec<detection::Entry> = (0..n)
        .map(|i| detection::Entry { name: &names[i], stem: &stems[i], score: scores[i], rank: ranks[i] })
        .collect();
    Ok(Some(py.detach(|| detection::board_families(&rows, min_size, band_pct, max_edit))))
}

type History = (
    Vec<(String, Vec<usize>)>,
    Vec<(String, Vec<i64>)>,
    Vec<detection::Group>,
    Vec<Vec<String>>,
);

/// `_cluster_history_signals`' CPU work: (active hours, board sets, per-board
/// lockstep groups, schedule groups), or None for input Python must handle.
#[pyfunction]
#[allow(clippy::too_many_arguments)]
fn det_history(
    py: Python<'_>,
    series_by_board: &Bound<'_, PyDict>,
    anchors: Vec<i64>,
    min_gain: f64,
    pct: f64,
    tol: f64,
    min_match: i64,
    min_ratio: f64,
    min_density: f64,
    min_group: usize,
    max_cell: usize,
    sched_min_hours: i64,
    sched_min_sim: f64,
) -> PyResult<Option<History>> {
    let slot_of: HashMap<i64, usize> = anchors.iter().enumerate().map(|(i, a)| (*a, i)).collect();
    let mut boards: Vec<detection::Board> = Vec::with_capacity(series_by_board.len());
    for (uuid, series) in series_by_board.iter() {
        let (Ok(uuid), Ok(series)) = (uuid.extract::<i64>(), series.cast_into::<PyDict>()) else { return Ok(None) };
        let mut names = Vec::with_capacity(series.len());
        for (name, sc) in series.iter() {
            let (Ok(name), Ok(sc)) = (name.extract::<String>(), sc.cast_into::<PyDict>()) else { return Ok(None) };
            let mut pts = Vec::with_capacity(sc.len());
            for (a, v) in sc.iter() {
                let Some(&slot) = a.extract::<i64>().ok().and_then(|a| slot_of.get(&a)) else { continue };
                let Some(v) = exact_f64(&v) else { return Ok(None) };
                pts.push((slot, v));
            }
            pts.sort_by_key(|p| p.0);
            names.push((name, pts));
        }
        boards.push((uuid, names));
    }
    let p = detection::Params { min_gain, pct, tol, min_match, min_ratio, min_density, min_group, max_cell };
    let out = py.detach(|| {
        let (hours, on) = detection::history_features(&boards, min_gain);
        let groups = detection::comovement_groups(&boards, &p);
        let sched = if sched_min_hours > 0 {
            detection::schedule_groups(&hours, sched_min_hours as usize, sched_min_sim, min_group, min_density, max_cell, anchors.len())
        } else {
            Vec::new()
        };
        let hours = hours.into_iter().map(|(n, s)| (n, s.into_iter().collect())).collect();
        (hours, on, groups, sched)
    });
    Ok(Some(out))
}

type DumpLine<'py> = (String, String, String, String, String, Bound<'py, PyList>);

/// `parser.py`'s board lines with entries already built as `entry_cls`
/// (``ParsedEntry``) tuples, or None to parse the whole dump in Python.
#[pyfunction]
fn lb_parse<'py>(py: Python<'py>, text: &str, entry_cls: &Bound<'py, PyAny>) -> PyResult<Option<Vec<DumpLine<'py>>>> {
    let Some(lines) = py.detach(|| kiwi_core::lbdump::parse(text)) else { return Ok(None) };
    // tuple.__new__(ParsedEntry, ...) skips the NamedTuple's Python-level __new__.
    let new = py.get_type::<PyTuple>().getattr(intern!(py, "__new__"))?;
    let mut out = Vec::with_capacity(lines.len());
    for l in lines {
        let entries = PyList::empty(py);
        for (rank, name, score) in &l.entries {
            entries.append(new.call1((entry_cls, (*rank, *name, *score)))?)?;
        }
        out.push((
            l.name_id.to_owned(),
            l.category_id.to_owned(),
            l.uuid.to_owned(),
            l.name.to_owned(),
            l.category.to_owned(),
            entries,
        ));
    }
    Ok(Some(out))
}

/// One capture's per-board scores, decoded from a binary COPY.
#[pyclass(frozen)]
struct ActCapture(kiwi_core::activity::Capture);

/// Decode a binary COPY of `(board_uuid, player_id, score)`; None if malformed.
#[pyfunction]
fn act_decode(py: Python<'_>, data: &[u8]) -> Option<ActCapture> {
    py.detach(|| kiwi_core::activity::decode_copy(data)).map(ActCapture)
}

/// `_pair_estimate`: (sorted active player ids, boards in both captures,
/// {board: active count} for boards with any).
#[pyfunction]
fn act_pair(
    py: Python<'_>,
    early: &ActCapture,
    late: &ActCapture,
    boards: Vec<i32>,
) -> (Vec<i64>, usize, HashMap<i32, usize>) {
    let p = py.detach(|| kiwi_core::activity::pair(&early.0, &late.0, &boards));
    (p.active, p.boards, p.per_board)
}

/// `_class_active_sets` by player id: `[(class, active ids, clean ids | None)]`.
#[pyfunction]
#[allow(clippy::too_many_arguments)]
fn act_class_sets(
    py: Python<'_>,
    early: &ActCapture,
    late: &ActCapture,
    gates: &ActCapture,
    crossed: bool,
    short: bool,
    power_rank: f64,
    effort: f64,
    xp: f64,
    xp_board: i32,
) -> Vec<kiwi_core::activity::ClassSet> {
    let reset = kiwi_core::activity::ResetWindow { crossed, short };
    let floors = kiwi_core::activity::Floors { power_rank, effort, xp, xp_board };
    py.detach(|| kiwi_core::activity::class_sets(&early.0, &late.0, &gates.0, &reset, &floors))
}

/// `duplicate_groups_at` over a binary COPY of `(board_uuid, player_id, rank,
/// score)`: `[(player_id, board_uuid, occurrences)]`, or None if malformed.
#[pyfunction]
fn dup_groups(py: Python<'_>, data: &[u8]) -> Option<Vec<(i64, i32, i32)>> {
    py.detach(|| kiwi_core::activity::duplicate_groups(data))
}

/// Player id -> name key, for rename fingerprints.
#[pyclass(frozen)]
struct RenKeys(rustc_hash::FxHashMap<i64, i64>);

#[pyfunction]
fn ren_keys(ids: Vec<i64>, keys: Vec<i64>) -> RenKeys {
    RenKeys(ids.into_iter().zip(keys).collect())
}

/// One capture's rename fingerprints.
#[pyclass(frozen)]
struct RenFp(kiwi_core::renames::Fps);

/// `_fingerprints` over a lifetime-board capture; None if a player has no key.
#[pyfunction]
fn ren_fingerprint(py: Python<'_>, capture: &ActCapture, min_score: f64, keys: &RenKeys) -> Option<RenFp> {
    py.detach(|| kiwi_core::renames::fingerprint(&capture.0, min_score, &keys.0)).map(RenFp)
}

/// Names only in `a` / only in `b` with at least `min_boards` boards:
/// `([(key, [(board, score)], ids)], [...])`.
#[pyfunction]
fn ren_diff(
    py: Python<'_>,
    a: &RenFp,
    b: &RenFp,
    min_boards: usize,
) -> (kiwi_core::renames::Side, kiwi_core::renames::Side) {
    py.detach(|| kiwi_core::renames::diff(&a.0, &b.0, min_boards))
}

/// `player_board_agg` folded in memory, capture by capture.
#[pyclass]
struct AggBuilder(kiwi_core::agg::Agg);

type AggRecord = (i64, i32, i32, i64, i64, i64, i32, f64, i64, Option<i64>);

#[pymethods]
impl AggBuilder {
    #[new]
    fn new() -> Self {
        AggBuilder(Default::default())
    }

    /// Fold one capture's binary COPY; the number of (player, board) rows it
    /// held, or None if the bytes aren't that COPY.
    fn fold(&mut self, py: Python<'_>, anchor: i64, data: &[u8]) -> Option<usize> {
        let agg = &mut self.0;
        py.detach(|| agg.fold_copy(anchor, data))
    }

    fn __len__(&self) -> usize {
        self.0.rows.len()
    }

    /// Rows in `player_board_agg` column order (player_id, board_uuid, best_rank,
    /// appearances, first_seen, last_seen, latest_rank, latest_score,
    /// last_folded_anchor, last_rise).
    fn records(&self) -> Vec<AggRecord> {
        self.0
            .rows
            .iter()
            .map(|(&(p, b), r)| {
                (p, b, r.best_rank, r.appearances, r.first_seen, r.last_seen, r.latest_rank,
                 r.latest_score, r.last_folded, r.last_rise)
            })
            .collect()
    }
}

fn abc_value<'py>(py: Python<'py>, v: &kiwi_core::abc::Val) -> PyResult<Bound<'py, PyAny>> {
    use kiwi_core::abc::Val;
    Ok(match v {
        Val::None => py.None().into_bound(py),
        Val::Bool(b) => pyo3::types::PyBool::new(py, *b).to_owned().into_any(),
        Val::Int(i) => i.into_pyobject(py)?.into_any(),
        Val::Float(f) => PyFloat::new(py, *f).into_any(),
        Val::Str(s) => PyString::new(py, s).into_any(),
        Val::List(xs) => PyList::new(py, xs.iter().map(|x| abc_value(py, x)).collect::<PyResult<Vec<_>>>()?)?.into_any(),
    })
}

/// `abc.static_arrays` over a whole movie: `{class: {static: [values]}}`.
#[pyfunction]
fn swf_static_arrays<'py>(py: Python<'py>, swf: &[u8]) -> PyResult<Option<Bound<'py, PyDict>>> {
    let Some(parsed) = py.detach(|| kiwi_core::abc::movie_static_arrays(swf)) else { return Ok(None) };
    let out = PyDict::new(py);
    for (class, arrays) in &parsed {
        let d = PyDict::new(py);
        for (name, v) in arrays {
            d.set_item(name, abc_value(py, v)?)?;
        }
        out.set_item(class, d)?;
    }
    Ok(Some(out))
}

/// `zlib.decompressobj()` over `data[offset:]` (`strict`: the stream must end, as
/// one-shot `zlib.decompress` requires). None where Python's zlib raises.
#[pyfunction]
#[pyo3(signature = (data, offset=0, strict=false))]
fn zlib_inflate<'py>(py: Python<'py>, data: &[u8], offset: usize, strict: bool) -> Option<Bound<'py, PyBytes>> {
    let z = py.detach(|| kiwi_core::zlib::inflate(data.get(offset..).unwrap_or(&[]), usize::MAX))?;
    (z.eof || !strict).then(|| PyBytes::new(py, &z.data))
}

/// `zlib.compress(data, level)`, or the `.tmod` stream (`chunk` + `sync`).
#[pyfunction]
#[pyo3(signature = (data, level, chunk=usize::MAX, sync=false))]
fn zlib_deflate<'py>(py: Python<'py>, data: &[u8], level: u32, chunk: usize, sync: bool) -> Option<Bound<'py, PyBytes>> {
    let z = py.detach(|| kiwi_core::zlib::deflate(data, level, chunk, sync))?;
    Some(PyBytes::new(py, &z))
}

/// `extract._decompress` for FWS/CWS movies; None for ZWS or bad input.
#[pyfunction]
fn swf_decompress<'py>(py: Python<'py>, raw: &[u8]) -> Option<Bound<'py, PyBytes>> {
    let m = py.detach(|| kiwi_core::swf::movie(raw).map(|m| m.into_owned()))?;
    Some(PyBytes::new(py, &m))
}

/// `build_tmod`'s serializer over normalized `(path, content)` files.
#[pyfunction]
fn tmod_pack<'py>(
    py: Python<'py>,
    version: u16,
    properties: Vec<(String, String)>,
    files: Vec<(String, pyo3::pybacked::PyBackedBytes)>,
) -> Option<Bound<'py, PyBytes>> {
    let props: Vec<(&str, &str)> = properties.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let files: Vec<(&str, &[u8])> = files.iter().map(|(p, c)| (p.as_str(), &c[..])).collect();
    let out = py.detach(|| kiwi_core::tmod::pack(version, &props, &files))?;
    Some(PyBytes::new(py, &out))
}

type TmodEntry = (String, u32, u32, u32, u32);
type TmodHeader = (Vec<(String, String)>, Vec<TmodEntry>, bool);

/// `_read_header`'s success path: `(properties, entries, repaired)`.
#[pyfunction]
fn tmod_header(py: Python<'_>, data: &[u8], header_size: usize, prop_count: u16) -> Option<TmodHeader> {
    let h = py.detach(|| kiwi_core::tmod::header(data, header_size, prop_count))?;
    let entries = h.entries.into_iter().map(|e| (e.path, e.index, e.offset, e.size, e.checksum)).collect();
    Some((h.properties, entries, h.repaired))
}

/// Every entry's content as base64, from the file stream at `header_size`.
#[pyfunction]
fn tmod_contents(py: Python<'_>, data: &[u8], header_size: usize, entries: Vec<TmodEntry>) -> Vec<String> {
    let compressed = data.get(header_size..).unwrap_or(&[]);
    let entries: Vec<_> = entries
        .into_iter()
        .map(|(path, index, offset, size, checksum)| kiwi_core::tmod::Entry { path, index, offset, size, checksum })
        .collect();
    py.detach(|| kiwi_core::tmod::contents(compressed, &entries))
}

type GemLine = ((f64, f64), (f64, f64), (f64, f64));

/// `calculate_builds`' ranking: the top `limit` rows as
/// `(boosts, cfirst, csecond, light, final, coefficient)`.
#[pyfunction]
#[allow(clippy::too_many_arguments)]
fn gem_rank(
    py: Python<'_>,
    empowered: Option<GemLine>,
    lesser: Option<GemLine>,
    base: (f64, f64, f64, f64, f64, f64),
    class_bonus: Option<f64>,
    farm: bool,
    light_target: i64,
    precise: bool,
    limit: usize,
) -> Vec<(Vec<u8>, f64, f64, f64, f64, f64)> {
    let pair = |(a, b): (f64, f64)| [a, b];
    let lines = |l: GemLine| [pair(l.0), pair(l.1), pair(l.2)];
    let gems = empowered.zip(lesser).map(|(e, l)| kiwi_core::gems::GemLines { empowered: lines(e), lesser: lines(l) });
    let (first, second, third, fourth, fifth, sixth) = base;
    let base = kiwi_core::gems::Base { first, second, third, fourth, fifth, sixth };
    py.detach(|| kiwi_core::gems::rank(gems.as_ref(), &base, class_bonus, farm, light_target, precise, limit))
        .into_iter()
        .map(|(b, a, c, d, e, f)| (b.to_vec(), a, c, d, e, f))
        .collect()
}

#[pymodule]
fn kiwi_native(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<AggBuilder>()?;
    m.add_class::<RenKeys>()?;
    m.add_class::<RenFp>()?;
    m.add_function(wrap_pyfunction!(ren_keys, m)?)?;
    m.add_function(wrap_pyfunction!(swf_static_arrays, m)?)?;
    m.add_function(wrap_pyfunction!(swf_decompress, m)?)?;
    m.add_function(wrap_pyfunction!(zlib_inflate, m)?)?;
    m.add_function(wrap_pyfunction!(zlib_deflate, m)?)?;
    m.add_function(wrap_pyfunction!(tmod_header, m)?)?;
    m.add_function(wrap_pyfunction!(tmod_pack, m)?)?;
    m.add_function(wrap_pyfunction!(tmod_contents, m)?)?;
    m.add_function(wrap_pyfunction!(gem_rank, m)?)?;
    m.add_function(wrap_pyfunction!(ren_fingerprint, m)?)?;
    m.add_function(wrap_pyfunction!(ren_diff, m)?)?;
    m.add_function(wrap_pyfunction!(dup_groups, m)?)?;
    m.add_function(wrap_pyfunction!(act_class_sets, m)?)?;
    m.add_class::<ActCapture>()?;
    m.add_function(wrap_pyfunction!(act_decode, m)?)?;
    m.add_function(wrap_pyfunction!(act_pair, m)?)?;
    m.add_function(wrap_pyfunction!(lb_parse, m)?)?;
    m.add_function(wrap_pyfunction!(det_board_families, m)?)?;
    m.add_function(wrap_pyfunction!(det_history, m)?)?;
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

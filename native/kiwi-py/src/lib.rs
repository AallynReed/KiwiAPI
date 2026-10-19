//! Python bindings for kiwi-core, imported as `kiwi_native`.

use pyo3::prelude::*;

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

#[pymodule]
fn kiwi_native(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(calculate_hash, m)?)?;
    m.add_function(wrap_pyfunction!(harvest_strings, m)?)?;
    Ok(())
}

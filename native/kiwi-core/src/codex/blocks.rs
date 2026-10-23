//! `app/trove/codexes/blocks.py`: placeable/block prefab -> model name.

use std::collections::HashMap;

use super::{py, wire::real_fields};

/// `parse_block_models`: first model per lowercased prefab path.
pub fn parse_block_models(content: &[u8]) -> HashMap<String, String> {
    let mut models = HashMap::new();
    let mut pending: Option<String> = None;
    for f in real_fields(content) {
        let lowered = py::lower(&f.text);
        if f.field == 0 && (lowered.starts_with("placeable/") || lowered.starts_with("block/")) {
            pending = Some(lowered);
        } else if f.field == 1
            && let Some(p) = pending.take()
        {
            models.entry(p).or_insert(f.text);
        }
    }
    models
}

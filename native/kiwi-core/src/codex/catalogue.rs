//! `app/trove/codexes/catalogue.py`: recipe catalogue membership and order.

use std::collections::{HashMap, HashSet};

use super::providers::trimmed_tokens;
use crate::binfab::harvest_strings;

/// recipe id -> first source order (duplicates only flag the first entry).
pub fn parse_recipe_catalogue(data: &[u8], known: &HashSet<String>) -> HashMap<String, i64> {
    let mut out = HashMap::new();
    let mut order = 0;
    for h in harvest_strings(data, 2, 512) {
        let lowered = h.text.to_ascii_lowercase();
        for tok in trimmed_tokens(lowered.as_bytes(), known) {
            out.entry(tok).or_insert_with(|| {
                order += 1;
                order - 1
            });
        }
    }
    out
}

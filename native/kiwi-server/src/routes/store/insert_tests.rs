//! The insert's documents against what Python's `save()` stored in the
//! store ingest scenarios (`testdata/store/golden.json`, `ingest`): same
//! fields, order and BSON types (canonical extended JSON).

use std::collections::BTreeMap;

use kiwi_core::store::{
    Category, NO_STORE_DATA, Product, State, ingest_category, ingest_product, ingest_state, parser::parse_dump,
};
use mongodb::bson::{Bson, Document};
use serde_json::Value;

use super::{
    insert::{category_doc, product_doc, state_doc},
    model,
    tests::{arr, golden, s, to_doc},
};

fn canonical(d: Document) -> Value {
    Bson::Document(d).into_canonical_extjson()
}

fn without_id(v: &Value) -> Value {
    let mut v = v.clone();
    v.as_object_mut().expect("doc").shift_remove("_id");
    v
}

#[test]
fn saved_documents_match_python() {
    let mut checked = 0;
    for scenario in arr(golden(), "ingest") {
        let mut products: BTreeMap<String, Product> = scenario["seed"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| model::product(&to_doc(d)).expect("seed"))
            .map(|p| (p.code.clone(), p))
            .collect();
        let mut categories: BTreeMap<i64, Category> = BTreeMap::new();
        let mut state: Option<State> = None;
        for step in arr(scenario, "steps") {
            let anchor = step["anchor"].as_i64().unwrap();
            let dump = parse_dump(s(&step["text"]));
            if dump.products.is_empty() && dump.categories.is_empty() {
                assert_eq!(step["error"], NO_STORE_DATA);
                continue;
            }
            let prev = state.as_ref().and_then(|s| s.last_anchor);
            for p in &dump.products {
                let (doc, _, _) = ingest_product(products.get(&p.code).cloned(), p, anchor, prev).unwrap();
                products.insert(p.code.clone(), doc);
            }
            for c in &dump.categories {
                categories.insert(c.index, ingest_category(categories.get(&c.index).cloned(), c, anchor));
            }
            state = Some(ingest_state(state.unwrap_or_default(), &dump, anchor));
            for want in arr(step, "products") {
                let code = s(&want["code"]);
                if !dump.products.iter().any(|p| p.code == code) {
                    continue;
                }
                assert_eq!(canonical(product_doc(&products[code])), without_id(want), "product {code}");
                checked += 1;
            }
            for want in arr(step, "categories") {
                let idx = want["index"]["$numberInt"].as_str().unwrap().parse::<i64>().unwrap();
                if dump.categories.iter().any(|c| c.index == idx) {
                    assert_eq!(canonical(category_doc(&categories[&idx])), without_id(want), "category {idx}");
                }
            }
            let want = &arr(step, "state")[0];
            assert_eq!(canonical(state_doc(state.as_ref().unwrap())), without_id(want), "state");
        }
    }
    assert!(checked > 5, "only {checked} product documents compared");
}

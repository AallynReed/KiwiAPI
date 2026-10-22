//! Spot checks; the golden fixtures from Python run in kiwi-server
//! (`src/market/tests.rs`).

use std::collections::HashMap;

use super::{
    parser::{created_at, parse_dump, uuid_str},
    signals::{DayRow, scan},
    tomes::{Reward, Tome, price_tome},
    *,
};
use crate::gems::pynum::PyNum;

#[test]
fn sort_tokens() {
    assert_eq!(order_by("+price_each"), "price_each ASC");
    assert_eq!(order_by("-+price"), "price DESC");
    assert_eq!(order_by(""), "last_seen DESC");
    assert_eq!(order_by("price; drop"), "last_seen DESC");
}

#[test]
fn uuid_time_rounds_through_a_float() {
    // 2026-10-22: ticks/10 = ...9999999.9 µs, which the float rounds up into the next second.
    let ticks: u64 = 0x01B2_1DD2_1381_4000 + 1_792_000_000 * 10_000_000 + 9_999_999;
    let low = ticks & 0xffff_ffff;
    let mid = (ticks >> 32) & 0xffff;
    let hi = ((ticks >> 48) & 0x0fff) | 0x1000;
    let id = (low as u128) << 96 | (mid as u128) << 80 | (hi as u128) << 64 | 0x8000_0000_0000_0000;
    assert_eq!(created_at(id), 1_792_000_001);
    assert_eq!(&uuid_str(id)[14..15], "1");
}

#[test]
fn dump_dedup_and_unicode_digits() {
    let id = "1f0a7c00-0000-1000-8000-000000000001";
    let text = format!("{id};  Glim ;;\u{0663};\u{0661}\u{0662},{};Glim;;1;1,{id};Again;;1;1", id.to_uppercase());
    let rows = parse_dump(&text);
    assert_eq!(rows.len(), 1);
    assert_eq!((rows[0].name.as_str(), rows[0].kind.as_deref(), rows[0].stack, rows[0].price), ("Glim", None, 3, 12));
    assert_eq!(rows[0].price_each, 4.0);
}

#[test]
fn outliers_need_spread() {
    assert_eq!(price_outliers(&[5.0; 6]), None);
    assert_eq!(price_outliers(&[1.0, 2.0, 3.0, 4.0]), None);
    let dropped = price_outliers(&[100.0, 101.0, 99.0, 100.5, 98.0, 1e7]).unwrap();
    assert_eq!(dropped, vec![false, false, false, false, false, true]);
}

#[test]
fn reorder_saves_listed_first() {
    let docs = [(1, 0, "b"), (2, 1, "a"), (3, 1, "C")];
    assert_eq!(reorder(&docs, &[Some(3), Some(3)]), vec![(2, 0), (2, 1), (0, 2), (1, 3)]);
    assert_eq!(reorder(&docs, &[]), vec![(2, 2)]);
}

#[test]
fn blueprint_pick_prefers_item_role() {
    let rows = [
        ("x", "bp1", Some("prefabs/item/x")),
        ("x", "bp2", Some("prefabs/collections/x")),
        ("y", "a", None),
        ("y", "b", None),
    ];
    let got = pick_blueprints(rows);
    assert_eq!(got.get("x").map(String::as_str), Some("bp1"));
    assert!(!got.contains_key("y"));
}

#[test]
fn tome_known_value_stays_int_zero() {
    let reward = |source: &str| Reward {
        item: "Ore".into(),
        amount: 10,
        source: Some(source.into()),
        price_item: None,
        per: None,
    };
    let tome = Tome { name: "T".into(), kind: "regular".into(), note: None, rewards: vec![reward("market")] };
    let priced = price_tome(&tome, &HashMap::new());
    assert_eq!((priced.value, priced.known_value, priced.status), (None, Some(PyNum::Int(0)), "unlisted"));
    let medians = HashMap::from([("Ore".to_owned(), (2.5, 4))]);
    let priced = price_tome(&tome, &medians);
    assert_eq!(priced.value, Some(PyNum::Float(25.0)));
    assert_eq!(priced.rewards[0].listings, 4);
}

#[test]
fn breadth_without_history() {
    let rows: Vec<DayRow> = (0..3)
        .map(|d| DayRow {
            name: "A".into(),
            bucket: d * 86400,
            listings: 5,
            stack: 5,
            p50: 1.0,
            stack_med: 1.0,
            stack_max: 1,
        })
        .collect();
    let s = scan(&rows).unwrap();
    assert_eq!((s.market.verdict, s.scanned_items, s.generated_for), ("no_data", 1, Some(2 * 86400)));
}

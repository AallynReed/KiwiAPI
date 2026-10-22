use super::*;

#[test]
fn python_numbers() {
    assert_eq!(py_sum([0.1, 0.2, 0.3]), 0.6);
    assert_eq!(py_sum([1e16, 1.0, -1e16]), 1.0);
    assert_eq!(median(&[3.0, 1.0, 2.0, 10.0]), 2.5);
    assert_eq!(py_round(2.675, 2), 2.67);
    assert_eq!(round_val(1234.0), Value::from(1234));
    assert_eq!(round_val(1234.567), fv(1234.57));
    assert_eq!(fmt_num(1234567.0), "1,234,567");
    assert_eq!(fmt_num(-1234.5), "-1,234.50");
    assert_eq!(fixed(0.125, 2), "0.12");
    assert_eq!(g3(0.0), "0");
    assert_eq!(g3(0.5), "0.5");
    assert_eq!(g3(1234.5), "1.23e+03");
    assert_eq!(g3(0.00001234), "1.23e-05");
    assert_eq!(py_int("١٢"), Some(12));
    assert_eq!(py_int("1_0"), Some(10));
    assert_eq!(py_int("_1"), None);
}

#[test]
fn stems_and_cadence() {
    assert_eq!(name_stem("Σίσυφος٣"), "σίσυφος");
    assert_eq!(name_stem("ｆｕｌｌ１２"), "ｆｕｌｌ");
    assert_eq!(name_stem("anana 5"), "anana");
    assert_eq!(dominant_stem(&["bot1".into(), "bot2".into(), "x".into()]), ("bot".into(), 2));
    assert_eq!(effective_reset_kind(Some("none"), 4001), "none");
    assert_eq!(effective_reset_kind(Some("bogus"), 4001), "weekly");
    assert_eq!(reset_kind(32000), "daily");
    // Monday 2026-10-19 11:00 UTC.
    assert_eq!(reset_boundary_before(1_792_584_000, "weekly"), 1_792_407_600);
    assert_eq!(reset_boundary_before(1_792_407_599, "daily"), 1_792_321_200);
    assert_eq!(reset_boundary_before(1_792_407_599, "none"), 0);
}

fn entry(name: &str, rank: i64, score: f64) -> Entry {
    Entry { player_name: name.into(), rank, score }
}

#[test]
fn runaway_is_flagged() {
    let board = Board {
        uuid: 4001,
        name_id: "LB".into(),
        name: "Effort".into(),
        category_id: "C".into(),
        category: "Effort".into(),
        contest_type: None,
        reset_kind: "weekly".into(),
    };
    let mut entries = vec![entry("Runaway", 1, 1e6)];
    entries.extend((2..60).map(|i| entry(&format!("P{i}"), i, 2500.0 - i as f64 * 20.0)));
    let checks = Checks {
        z_threshold: 5.0,
        z_repr: "5.0".into(),
        velocity_multiplier: 10.0,
        min_board_size: 20,
        cohort_pct: 0.05,
        excluded: BTreeSet::new(),
    };
    let mut det = Detector::new();
    let plan = det.scan(&board, &entries, &checks).expect("flagged players need velocity");
    assert!(plan.higher_is_better);
    assert!(plan.need.contains(&"Runaway".to_string()));
    let head = Head {
        anchor: Some(1),
        z_threshold: 5.0,
        velocity_multiplier: 10.0,
        min_board_size: 20,
        cheater_detection_enabled: true,
        alt_clusters_enabled: false,
        computed_at: 2,
    };
    let out = det.finish(&head, Vec::new(), 0);
    let p = &out["players"][0];
    assert_eq!(p["player_name"], "Runaway");
    let kinds: Vec<&str> =
        p["leaderboards"][0]["evidence"].as_array().unwrap().iter().map(|e| e["type"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["score_outlier", "rank_gap"]);
    assert_eq!(out["total_flagged"], 1);
}

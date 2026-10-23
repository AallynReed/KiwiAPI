//! Golden checks against `testdata/rigs/*` (gen_workshop_fixtures.py).

use std::collections::HashMap;
use std::sync::OnceLock;

use kiwi_core::rig::{self, Placement, Row};
use serde_json::{Value, json};

use crate::{pyjson, request_id, workshop::tests as wt};

fn engine() -> &'static Value {
    static E: OnceLock<Value> = OnceLock::new();
    E.get_or_init(|| wt::load("rigs", "engine.json"))
}

fn rows() -> Vec<Row> {
    engine()["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            let t = |i: usize| wt::s(&r[i]).to_owned();
            (t(0), t(1), t(2), t(3), r[4].as_f64().unwrap(), r[5].as_f64().unwrap())
        })
        .collect()
}

fn blob(name: &str) -> Vec<u8> {
    wt::b64(&engine()["blobs"][name])
}

fn install() {
    let _ = super::TEST_ROWS.set(rows());
    wt::install_game();
    let files = [
        ("blueprints/equipment/weapon/equipment_weapon_1h_sword_001.blueprint", "sword"),
        ("blueprints/creatures/beetle/c_m_goldenbeetle_head.blueprint", "head"),
        ("blueprints/skins/adventurer/c_c_snowman_hat_01.blueprint", "hat"),
        ("blueprints/npc/merchant/c_c_snowman_hat_01.blueprint", "glass"),
        ("blueprints/skins/adventurer/char_hair_face_mustache_001.blueprint", "stache"),
    ];
    let _ = super::model::TEST_FILES.set(files.iter().map(|(p, b)| ((*p).to_owned(), blob(b))).collect());
}

fn same(a: &Value, b: &Value) -> bool {
    pyjson::dumps(a) == pyjson::dumps(b)
}

fn strs(v: &Value) -> Vec<String> {
    v.as_array().unwrap().iter().map(|x| wt::s(x).to_owned()).collect()
}

fn pairs(v: &[(String, String)]) -> Value {
    Value::Array(v.iter().map(|(a, b)| json!([a, b])).collect())
}

fn sorted_scales(m: &HashMap<String, f64>) -> Value {
    let mut v: Vec<(&String, &f64)> = m.iter().collect();
    v.sort_by(|a, b| a.0.cmp(b.0));
    Value::Array(v.into_iter().map(|(k, s)| json!([k, s])).collect())
}

fn baked(cells: &rig::Cells) -> Value {
    Value::Array(cells.iter().map(|((x, y, z), (r, g, b, k, l, s))| json!([[x, y, z], [r, g, b, k, l, s]])).collect())
}

#[test]
fn rig_data() {
    let e = engine();
    for (name, want) in e["head_aps"].as_object().unwrap() {
        let mut got: Vec<String> = super::rig(name).map(|r| r.head_aps().iter().cloned().collect()).unwrap_or_default();
        got.sort();
        assert_eq!(json!(got), *want, "{name}");
    }
    assert!(super::has_baked_rig("adventurer") && !super::has_baked_rig("nope"));
    assert!(super::rigs().len() > 300);
}

#[test]
fn binding_map() {
    let e = engine();
    let rm = rig::RigMap::build(&rows());
    let baked = |s: &Option<String>| s.as_deref().filter(|n| super::has_baked_rig(n)).and_then(super::rig);
    for (case, sc) in e["resolve"].as_array().unwrap().iter().zip(e["scales_for"].as_array().unwrap()) {
        let (skel, attach) = rm.resolve(&strs(&case[0]));
        assert_eq!(json!(skel), case[1], "{case}");
        assert!(same(&pairs(&attach), &case[2]), "{case}");
        let got = rm.scales_for(&attach, baked(&skel));
        assert_eq!(json!(got.head), sc[2], "{sc}");
        assert!(same(&sorted_scales(&got.parts), &sc[3]), "{sc}: {:?}", got.parts);
    }
    for case in e["creature_scales"].as_array().unwrap() {
        let got = rm.creature_scales(wt::s(&case[0]), super::rig);
        assert_eq!(json!(got.head), case[1], "{case}");
        assert!(same(&sorted_scales(&got.parts), &case[2]), "{case}");
    }
    for case in e["prefab_path"].as_array().unwrap() {
        let (canon, cands) = rm.prefab_path(wt::s(&case[0]));
        assert_eq!(json!([canon, cands]), json!([case[1], case[2]]), "{case}");
    }
    for case in e["creature_for"].as_array().unwrap() {
        let got = match rm.creature_for(wt::s(&case[0])) {
            Some((skel, prefab, parts)) => json!([skel, prefab, pairs(&parts)]),
            None => json!([null, null, []]),
        };
        assert_eq!(got, json!([case[1], case[2], case[3]]), "{case}");
    }
}

#[test]
fn assembly() {
    let e = engine();
    let mut bad = Vec::new();
    for case in e["assemble"].as_array().unwrap() {
        let name = wt::s(&case["rig"]);
        let files: Vec<(String, Vec<u8>)> =
            strs(&case["files"]).iter().map(|n| (format!("blueprints/{n}.blueprint"), blob(n))).collect();
        let overrides: HashMap<String, String> =
            case["overrides"].as_object().unwrap().iter().map(|(k, v)| (k.clone(), wt::s(v).to_owned())).collect();
        let scales: HashMap<String, f64> =
            case["scales"].as_object().unwrap().iter().map(|(k, v)| (k.clone(), v.as_f64().unwrap())).collect();
        let got = match super::rig(name) {
            Some(r) => rig::assemble(r, name, &files, &overrides, &scales).unwrap(),
            None => None,
        };
        let got = got.unwrap_or(Value::Null);
        if !same(&got, &case["model"]) {
            bad.push(format!("assemble {name} {}: {}", case["files"], diff(&got, &case["model"])));
        }
    }
    for case in e["placements"].as_array().unwrap() {
        let name = wt::s(&case["rig"]);
        let pl: Vec<Placement> = case["placements"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| Placement {
                ap: wt::s(&p["ap"]).to_owned(),
                raw: blob(wt::s(&p["blob"])),
                scale: p["scale"].as_f64().unwrap(),
                tint: p.get("tint").map(|t| {
                    let c: Vec<i64> = t.as_array().unwrap().iter().map(|x| x.as_i64().unwrap()).collect();
                    ([c[0], c[1], c[2]], p.get("max_off").and_then(Value::as_i64))
                }),
            })
            .collect();
        let (model, cells) = match super::rig(name) {
            Some(r) => {
                let parts = rig::placement_parts(r, &pl).unwrap();
                let model = if parts.is_empty() { Value::Null } else { rig::payload(r, name, &parts) };
                (model, rig::placement_voxels(r, &pl).unwrap())
            }
            None => (Value::Null, Vec::new()),
        };
        if !same(&model, &case["model"]) {
            bad.push(format!("placements {name}: {}", diff(&model, &case["model"])));
        }
        if !same(&baked(&cells), &case["baked"]) {
            bad.push(format!("placement bake {name}: {}", diff(&baked(&cells), &case["baked"])));
        }
    }
    for case in e["voxels"].as_array().unwrap() {
        let name = wt::s(&case["rig"]);
        let parts: Vec<(String, Vec<u8>, Option<f64>)> = case["parts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| (wt::s(&p[0]).to_owned(), blob(wt::s(&p[1])), p.get(2).and_then(Value::as_f64)))
            .collect();
        let cells = super::rig(name).map(|r| rig::assemble_voxels(r, &parts).unwrap()).unwrap_or_default();
        if !same(&baked(&cells), &case["baked"]) {
            bad.push(format!("voxels {name}: {}", diff(&baked(&cells), &case["baked"])));
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

#[test]
fn unbury_every_rig() {
    let place = |ap: &str, blob_name: &str, scale: f64| Placement {
        ap: ap.to_owned(),
        raw: blob(blob_name),
        scale,
        tint: None,
    };
    let mut bad = Vec::new();
    let cases = engine()["unbury"].as_array().unwrap();
    assert!(cases.len() > 900);
    for case in cases {
        let (name, ap) = (wt::s(&case[0]), wt::s(&case[1]));
        let parts =
            rig::placement_parts(super::rig(name).unwrap(), &[place(ap, "cube", 1.0), place(ap, "glow", 0.5)]).unwrap();
        let g = parts.last().unwrap().to_value();
        let got = json!([name, ap, g["x"][0], g["y"][0], g["z"][0]]);
        if !same(&got, case) {
            bad.push(format!("{} vs {}", pyjson::dumps(&got), pyjson::dumps(case)));
        }
    }
    assert!(bad.is_empty(), "{} of {}:\n{}", bad.len(), cases.len(), bad[..bad.len().min(10)].join("\n"));
}

/// Both dumps around their first difference.
fn diff(got: &Value, want: &Value) -> String {
    let (a, b) = (pyjson::dumps(got), pyjson::dumps(want));
    let at = a.bytes().zip(b.bytes()).position(|(x, y)| x != y).unwrap_or(a.len().min(b.len()));
    let window = |s: &str| s.get(at.saturating_sub(120)..(at + 200).min(s.len())).unwrap_or_default().to_owned();
    format!("at byte {at}:\n got  ...{}\n want ...{}", window(&a), window(&b))
}

#[test]
fn names_guard_paths() {
    for (name, ok) in
        [("adventurer", true), ("a_1", true), ("Adventurer", false), ("", false), ("a/b", false), ("..", false)]
    {
        assert_eq!(rig::valid_name(name), ok, "{name}");
    }
    assert!(rig::valid_name("abc\n"));
    assert!(super::load_animation("adventurer", "unarmed_ability_mine_begin").is_some());
    assert!(super::load_animation("adventurer", "../adventurer").is_none());
    assert!(super::load_animation_graph("adventurer").is_some());
}

/// `/site/rigs/...` path segments as Starlette hands them over.
fn segments(path: &str) -> Vec<String> {
    path.split('/').map(|p| percent_encoding::percent_decode_str(p).decode_utf8_lossy().into_owned()).collect()
}

#[tokio::test]
async fn routes_replay() {
    install();
    let st = wt::state();
    let mut bad = Vec::new();
    for case in wt::load("rigs", "routes.json").as_array().unwrap() {
        let path = wt::s(&case["path"]).split('?').next().unwrap().to_owned();
        let (parts, items) = (wt::parts_for(case), wt::items_for(case));
        let fut = async {
            if path == "/site/blueprint-editor/model" {
                super::model::model_form(&st, items).await
            } else if path == "/site/blueprint-editor/model-save" {
                super::model::model_save_form(items).await
            } else if path == "/site/blueprint-editor/game-model" {
                super::model::game_model_reply(&st, &parts).await
            } else if path.ends_with("/graph") {
                super::graph_reply(segments(&path)[3].clone()).await
            } else {
                let seg = segments(&path);
                super::anim_reply(seg[3].clone(), seg[5].clone()).await
            }
        };
        let resp = request_id::scope(wt::s(&case["request_id"]).to_owned(), fut).await;
        let (status, headers, body) = wt::body_of(resp).await;
        let name = wt::s(&case["name"]);
        let zip = name == "save_zip" || name == "save_loose";
        wt::compare(case, status, &headers, &body, zip, &mut bad);
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

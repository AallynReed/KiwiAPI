//! Golden checks: every decoder, serializer and the crafting tree against
//! what the production Python produced (`gen_codexes_fixtures.py`).

use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::LazyLock,
};

use chrono::{DateTime, Utc};
use kiwi_core::codex::{
    abilities, badges, bonuses,
    extract::{blueprint_ref, model_voxel_count},
    geode, links, localize, loot, mastery, powerrank, providers, recipe, types, unlocks, upgrades, wire,
};
use serde_json::{Map, Value, json};

use super::{
    crafting, indexer,
    pg::{Cell, P, Row},
    reads, store,
};
use crate::pyjson;

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata/codexes")
}

static GOLD: LazyLock<Value> = LazyLock::new(|| {
    pyjson::loads(&std::fs::read(dir().join("golden.json")).expect("golden.json")).expect("golden json")
});

fn read(name: &str) -> Vec<u8> {
    std::fs::read(dir().join(name)).expect(name)
}

fn table(key: &str) -> Vec<u8> {
    read(GOLD["tables"][key].as_str().unwrap())
}

fn arg(p: &P) -> Value {
    match p {
        P::Null => Value::Null,
        P::Bool(b) => json!(b),
        P::Int(i) => json!(i),
        P::Float(f) => json!(f),
        P::Text(s) => json!(s),
        P::Json(s) => pyjson::loads(s.as_bytes()).unwrap(),
        P::TextList(v) => json!(v),
        P::Ts(t) => json!(t.to_rfc3339()),
    }
}

fn args(v: &[P]) -> Value {
    Value::Array(v.iter().map(arg).collect())
}

fn int(v: Option<i128>) -> Value {
    v.map_or(Value::Null, kiwi_core::codex::int_value)
}

fn pr_table() -> powerrank::PowerRankTable {
    powerrank::parse_power_rank_table(&table("powerrank"))
}

#[test]
fn power_rank_table() {
    let mut got: Vec<Value> = pr_table().into_iter().map(|(k, v)| json!([k as u64, v as i64])).collect();
    got.sort_by_key(|v| v[0].as_u64());
    assert_eq!(Value::Array(got), GOLD["powerrank_table"]);
}

#[test]
fn prefab_decoders() {
    let table = pr_table();
    for (rel, g) in GOLD["prefabs"].as_object().unwrap() {
        let data = read(g["file"].as_str().unwrap());
        let ident = wire::decode_identity(&data).unwrap().map(
            |i| json!({"name_key": i.name_key, "category": i.category, "desc_key": i.desc_key, "tradable": i.tradable}),
        );
        assert_eq!(ident.unwrap_or(Value::Null), g["identity"], "{rel} identity");
        let stats: Vec<Value> =
            bonuses::extract_stat_bonuses(&data).iter().map(|s| Value::Object(s.to_value())).collect();
        assert_eq!(Value::Array(stats), g["stats"], "{rel} stats");
        let abil: Vec<Value> = bonuses::extract_abilities(&data).into_iter().map(Value::Object).collect();
        assert_eq!(Value::Array(abil), g["abilities"], "{rel} abilities");
        assert_eq!(int(powerrank::decode_power_rank(&data, &table)), g["power_rank"], "{rel} power rank");

        let parsed = recipe::parse_recipe(&data, None, &|_| false).unwrap();
        let rec = json!({
            "name": parsed.name, "description": parsed.description, "category": parsed.category,
            "output": parsed.output.map_or(Value::Null, Value::Object), "ingredients": parsed.ingredients,
            "requirements": parsed.requirements,
        });
        assert_eq!(rec, g["recipe"], "{rel} recipe");
        assert_eq!(int(recipe::recipe_mastery_from_prefab(&data)), g["recipe_mastery"], "{rel} recipe mastery");

        let rig = wire::extract_rig_refs(&data).map(|r| {
            let to_map = |v: &[(String, String)]| Value::Object(v.iter().map(|(k, x)| (k.clone(), json!(x))).collect());
            json!({
                "skeleton": r.skeleton, "parts": to_map(&r.parts), "refs": to_map(&r.refs),
                "scales": Value::Object(r.scales.iter().map(|(k, v)| (k.clone(), json!(v))).collect()),
                "head_scale": r.head_scale,
            })
        });
        assert_eq!(rig.unwrap_or(Value::Null), g["rig"], "{rel} rig");

        let valid: HashSet<String> =
            g["valid"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_owned()).collect();
        let sizes = g["sizes"].as_object().unwrap();
        let size = |n: &str| {
            let k = n.to_lowercase();
            sizes.get(k.strip_prefix("blueprints/").unwrap_or(&k)).and_then(Value::as_u64).unwrap_or(0) as usize
        };
        assert_eq!(json!(blueprint_ref(&data, &valid, rel, Some(&size))), g["blueprint"], "{rel} blueprint");
        assert_eq!(json!(blueprint_ref(&data, &valid, rel, None)), g["blueprint_named"], "{rel} blueprint by name");
        assert_eq!(json!(geode::find_upgrade_tree_ref(&data)), g["upgrade_tree_ref"], "{rel} tree ref");
        let (nk, dk) = abilities::extract_keys(&data);
        assert_eq!(json!({"name_key": nk, "desc_key": dk}), g["ability_keys"], "{rel} ability keys");
    }
}

#[test]
fn voxel_counts() {
    for (file, n) in GOLD["voxel_counts"].as_object().unwrap() {
        assert_eq!(model_voxel_count(&read(file)) as u64, n.as_u64().unwrap(), "{file}");
    }
    assert_eq!(model_voxel_count(b"kiwib\x05\x00\x00\x00junk"), 0);
    assert_eq!(model_voxel_count(b"nope"), 0);
}

#[test]
fn metric_names() {
    use base64::Engine;
    let exe = base64::engine::general_purpose::STANDARD.decode(GOLD["metric_exe_b64"].as_str().unwrap()).unwrap();
    assert_eq!(json!(badges::parse_metric_names(&exe)), GOLD["metric_exe_names"]);
    assert!(badges::parse_metric_names(b"no sentinel here").is_empty());
}

#[test]
fn collection_tables() {
    let data = table("geode_table");
    let members: Map<String, Value> =
        geode::geode_companion_members(&data).into_iter().map(|(k, v)| (k, json!(v))).collect();
    assert_eq!(Value::Object(members), GOLD["geode_members"]);
    let map: Map<String, Value> =
        wire::collection_category_map(&data).into_iter().map(|(k, v)| (k, json!(v))).collect();
    assert_eq!(Value::Object(map), GOLD["collection_map"]);
}

#[test]
fn multipliers_and_mastery() {
    let geode = mastery::parse_geode_multipliers(&table("geode_multipliers"));
    let got: Map<String, Value> = geode
        .iter()
        .map(|(k, r)| {
            (
                k.clone(),
                json!({"multiplier": r.multiplier as i64, "base": r.base as i64, "predicted": r.predicted as i64}),
            )
        })
        .collect();
    assert_eq!(Value::Object(got), GOLD["geode_multipliers"]);
    for (p, want) in GOLD["mastery_for"].as_object().unwrap() {
        let got =
            json!([int(mastery::mastery_for(p, &Default::default())), int(mastery::geode_mastery_for(p, &geode))]);
        assert_eq!(&got, want, "{p}");
    }
}

fn link(branch: &str, l: &links::Link) -> Value {
    args(&store::link_args(branch, l))
}

#[test]
fn unlocks_table() {
    let pairs = unlocks::parse_unlocks(&table("unlocks"));
    assert_eq!(json!(pairs), GOLD["unlocks"]);
    let edges: Vec<Value> = links::unlock_links(&pairs).iter().map(|l| link("live-us", l)).collect();
    assert_eq!(Value::Array(edges), GOLD["unlock_links"]);
}

#[test]
fn badge_requirements() {
    let names: Vec<String> =
        GOLD["metric_names"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_owned()).collect();
    let (rows, errors) = badges::parse_badges(&table("badges"), &names);
    let got: Vec<Value> = rows
        .iter()
        .map(|r| {
            json!({
                "collection": r.collection, "badge_id": r.badge_id, "rank": r.rank, "rank_name": r.rank_name,
                "completion_kind": r.completion_kind, "requirement_key": r.requirement_key, "label": r.label,
                "amount": r.amount, "difficulty": r.difficulty as i64, "context": r.context, "status": r.status,
                "blocker": r.blocker, "reason": r.reason,
            })
        })
        .collect();
    let want = GOLD["badges"]["rows"].as_array().unwrap();
    assert_eq!(got.len(), want.len());
    for (g, w) in got.iter().zip(want) {
        assert_eq!(g, w);
    }
    assert_eq!(json!(errors), GOLD["badges"]["errors"]);
    assert_eq!(json!(badges::parse_badges(&table("badges"), &[]).1), GOLD["badges_unresolved"]);
    let req: Vec<Value> =
        badges::requirement_rows(&rows).iter().map(|r| args(&store::requirement_args("live-us", r))).collect();
    assert_eq!(Value::Array(req), GOLD["requirement_rows"]);
}

#[test]
fn upgrade_trees() {
    for key in ["barrier", "prestige"] {
        let path =
            format!("prefabs/upgrade/trees/{}.binfab", if key == "barrier" { "barrier" } else { "prestige_bard" });
        let tree = upgrades::parse_upgrade_costs(&table(&format!("tree_{key}")), &path);
        let want = &GOLD[format!("tree_{key}").as_str()];
        assert_eq!(tree.system_kind, want["system_kind"].as_str().unwrap());
        assert_eq!(tree.system_key, want["system_key"].as_str().unwrap());
        let nodes: Vec<Value> = tree
            .nodes
            .iter()
            .map(|n| json!({"node_key": n.node_key, "rank": n.rank, "costs": n.costs, "requires": n.requires}))
            .collect();
        let want_nodes: Vec<Value> = want["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| json!({"node_key": n["node_key"], "rank": n["rank"], "costs": n["costs"], "requires": n["requires"]}))
            .collect();
        assert_eq!(nodes, want_nodes, "{key}");
        let keys: Vec<String> = tree.nodes.iter().map(|n| n.node_key.clone()).collect();
        let fx: Map<String, Value> = upgrades::parse_upgrade_effects(&table(&format!("fx_{key}")), &keys)
            .into_iter()
            .map(|(k, (n, d, a))| (k, json!({"name_key": n, "desc_key": d, "abilities": a})))
            .collect();
        assert_eq!(Value::Object(fx), GOLD[format!("fx_{key}").as_str()], "{key} effects");
        let edges: Vec<Value> = links::upgrade_links(&tree, &path).iter().map(|l| link("live-us", l)).collect();
        assert_eq!(Value::Array(edges), GOLD[format!("links_{key}").as_str()], "{key} links");
    }
    let levels: Vec<Value> = geode::parse_upgrade_tree(&table("fx_gleemur"))
        .into_iter()
        .map(|l| json!({"level": l.level, "stats": l.stats, "abilities": l.abilities}))
        .collect();
    assert_eq!(Value::Array(levels), GOLD["gleemur_levels"]);
}

fn style_rows(rows: &[loot::StyleRow]) -> Value {
    Value::Array(
        rows.iter()
            .map(|r| {
                json!({"equipment": r.equipment, "family": r.family, "name_key": r.name_key,
                       "raw_category": r.raw_category, "base_mastery": r.base_mastery})
            })
            .collect(),
    )
}

fn project_style(v: &Value) -> Value {
    Value::Array(
        v.as_array()
            .unwrap()
            .iter()
            .map(|r| {
                json!({"equipment": r["equipment"], "family": r["family"], "name_key": r["name_key"],
                       "raw_category": r["raw_category"], "base_mastery": r["base_mastery"]})
            })
            .collect(),
    )
}

#[test]
fn style_catalogues() {
    let banner = loot::parse_style_catalogue(&table("pvpbanner"), "prefabs/loot/pvpbanner.binfab").unwrap();
    assert_eq!(style_rows(&banner[..banner.len().min(40)]), project_style(&GOLD["pvpbanner"]));
    let hat = loot::parse_style_catalogue(&table("hat"), "prefabs/loot/hat.binfab").unwrap();
    assert_eq!(style_rows(&hat), project_style(&GOLD["hat_rows"]));
    assert_eq!(
        loot::parse_style_catalogue(&[0, 0, 1, 0, 0x04, 0x08, 1, b'a', 0, 0, 0, 0xff], "prefabs/loot/hat.binfab"),
        Err(kiwi_core::codex::PyErr("index out of range".into()))
    );
}

#[test]
fn locale_and_names() {
    let mut loc = localize::Loc::new();
    wire::extract_localization_map(&table("locale"), &mut loc);
    let got: Map<String, Value> = loc.iter().map(|(k, v)| (k.clone(), json!(v))).collect();
    assert_eq!(Value::Object(got), GOLD["locale"]);
    for (k, want) in GOLD["stat_names"].as_object().unwrap() {
        assert_eq!(&json!(localize::resolve_stat_name(&loc, k)), want, "{k}");
    }
    for (k, want) in GOLD["slot_names"].as_object().unwrap() {
        assert_eq!(&json!(localize::resolve_slot_name(&loc, Some(k))), want, "{k}");
    }
}

#[test]
fn bench_providers() {
    let rel = "prefabs/placeable/crafting/workbench_prismatic_blue_interactive.binfab";
    let data = read(&rel.replace('/', "__"));
    let known: HashSet<String> =
        GOLD["provider_known"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_owned()).collect();
    let map = providers::build_provider_map(&[(rel.to_owned(), data.clone())], &known).unwrap();
    let got: Map<String, Value> = map.into_iter().map(|(k, v)| (k, Value::Array(v))).collect();
    assert_eq!(Value::Object(got), GOLD["provider_map"]);
    let cats: Vec<Value> = providers::extract_categories(&data, &known)
        .unwrap()
        .into_iter()
        .map(|(c, o, r)| json!({"category": c, "order": o, "recipes": r}))
        .collect();
    assert_eq!(Value::Array(cats), GOLD["provider_categories"]);
}

#[test]
fn paths_and_types() {
    for (p, want) in GOLD["entry_paths"].as_object().unwrap() {
        assert_eq!(&json!(links::entry_path(p)), want, "{p:?}");
    }
    for (p, want) in GOLD["classify"].as_object().unwrap() {
        assert_eq!(&json!(types::classify(p)), want, "{p}");
    }
}

#[test]
fn search_filters() {
    for case in GOLD["filters"].as_array().unwrap() {
        let a = &case["args"];
        let s = |k: &str| a.get(k).and_then(Value::as_str).map(str::to_owned);
        let f = store::Filter {
            codex_type: s("codex_type"),
            search: s("search"),
            category: s("category"),
            tradable: a.get("tradable").and_then(Value::as_bool),
            stat: s("stat"),
            ability: s("ability"),
        };
        let (wh, params) = store::build_filter("pts", &f);
        assert_eq!(json!(wh), case["where"]);
        assert_eq!(args(&params), case["params"]);
    }
}

fn row_from(v: &Value) -> Row {
    Row(v
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, x)| {
            let c = match (k.as_str(), x) {
                ("indexed_at", Value::String(s)) => {
                    Cell::Ts(DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc))
                }
                ("data", v) => Cell::Json(v.clone()),
                (_, Value::Null) => Cell::Null,
                (_, Value::Bool(b)) => Cell::Bool(*b),
                (_, Value::String(s)) => Cell::Text(s.clone()),
                (_, Value::Number(n)) if n.is_f64() => Cell::Float(n.as_f64().unwrap()),
                (_, Value::Number(n)) => Cell::Int(n.as_i64().unwrap()),
                (_, other) => Cell::Json(other.clone()),
            };
            (k.clone(), c)
        })
        .collect())
}

#[test]
fn entry_serializers() {
    for case in GOLD["serialize"].as_array().unwrap() {
        let r = row_from(&case["row"]);
        assert_eq!(pyjson::pydantic(&reads::entry_out(&r)), case["v1"].as_str().unwrap());
        assert_eq!(pyjson::dumps(&reads::site_row(&r)), case["site"].as_str().unwrap());
    }
}

#[test]
fn crafting_tree() {
    let g = &GOLD["crafting"];
    let rows: Vec<(String, String, Value)> = g["recipes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| (r["path"].as_str().unwrap().to_owned(), r["name"].as_str().unwrap().to_owned(), r["data"].clone()))
        .collect();
    let map = crafting::recipe_map(&rows);
    let mut walk = crafting::Walk::new(&map);
    let tree = walk.node("item/a", "A", 2, 0);
    assert_eq!(walk.counter as u64, g["node_count"].as_u64().unwrap());
    assert_eq!(json!(walk.names.iter().collect::<Vec<_>>()), g["names"]);
    let prices: HashMap<String, (f64, i64)> = g["prices"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| (k.clone(), (v["median_each"].as_f64().unwrap(), v["count"].as_i64().unwrap())))
        .collect();
    let (v, _, _) = crafting::annotate(&tree, &prices);
    assert_eq!(pyjson::dumps(&v), g["tree"].as_str().unwrap());
}

/// Starlette tries routes in registration order; matchit prefers static
/// segments and falls back to the parameter, which lands on the same route.
#[tokio::test]
async fn route_precedence() {
    use axum::{Router, body::Body, extract::MatchedPath, http::Request, routing::get};
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    let echo = || get(|m: MatchedPath| async move { m.as_str().to_owned() });
    let mut app = Router::new();
    for p in [
        "/v1/codexes/types",
        "/v1/codexes/search",
        "/v1/codexes/links",
        "/v1/codexes/stats",
        "/v1/codexes/requirements",
        "/v1/codexes/upgrades",
        "/v1/codexes/upgrades/{system_key}",
        "/v1/codexes/{codex_type}",
        "/v1/codexes/{codex_type}/categories",
        "/v1/codexes/{codex_type}/stat-keys",
        "/v1/codexes/{codex_type}/abilities",
        "/v1/codexes/{codex_type}/entry",
    ] {
        app = app.route(p, echo());
    }
    for (uri, want) in [
        ("/v1/codexes/search", "/v1/codexes/search"),
        ("/v1/codexes/search/entry", "/v1/codexes/{codex_type}/entry"),
        ("/v1/codexes/upgrades/categories", "/v1/codexes/upgrades/{system_key}"),
        ("/v1/codexes/types/categories", "/v1/codexes/{codex_type}/categories"),
        ("/v1/codexes/render", "/v1/codexes/{codex_type}"),
        ("/v1/codexes/upgrades/x/entry", ""),
    ] {
        let res = app.clone().oneshot(Request::get(uri).body(Body::empty()).unwrap()).await.unwrap();
        let body = res.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(String::from_utf8_lossy(&body), want, "{uri}");
    }
}

#[test]
fn decisions_and_sorts() {
    assert_eq!(indexer::index_decision(false, 27, 27, true, Some(3)), "full");
    assert_eq!(indexer::index_decision(true, 26, 27, false, None), "rebuild");
    assert_eq!(indexer::index_decision(true, 27, 27, true, Some(3)), "delta");
    assert_eq!(indexer::index_decision(true, 27, 27, true, None), "noop");
    assert_eq!(
        store::sort_keys(),
        "name, -name, category, -category, mastery, -mastery, mastery_geode, -mastery_geode, power_rank, -power_rank, indexed_at, -indexed_at, stat_value, -stat_value"
    );
    let t = crate::starlette::Table::api();
    let get = axum::http::Method::GET;
    let any = "/v1/codexes/{codex_type}";
    assert!(crate::starlette::defers(t, any, &get, &"/v1/codexes/rend%65r?x=1".parse().unwrap()));
    assert!(!crate::starlette::defers(t, any, &get, &"/v1/codexes/mount".parse().unwrap()));
}

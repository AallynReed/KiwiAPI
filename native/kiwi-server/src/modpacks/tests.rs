//! Checks against `testdata/modpacks/golden.json`
//! (`native/port/parity/gen_modpacks_fixtures.py`).

use std::{collections::HashMap, sync::Arc, sync::OnceLock};

use axum::{
    Router,
    body::Body,
    http::{HeaderMap, HeaderValue, Method, Request, StatusCode},
    response::{IntoResponse, Response},
};
use base64::Engine;
use chrono::{DateTime, Utc};
use mongodb::bson::{Bson, Document, oid::ObjectId};
use serde_json::Value;
use tower::ServiceExt;

use super::{
    artifact::{self, Meta},
    body::{
        AdminListQuery, CollaboratorBody, CreateBody, CreateVariantBody, SetEntriesBody, TakedownBody, UpdateBody,
        UpdateVariantBody,
    },
    hub::{self, ModRef, Release},
    model::{Entry, Pack},
    service::{self, Bases, EntryView},
    text,
};
use crate::{
    auth::models::SiteUser,
    config::Settings,
    db::Db,
    error::ApiError,
    middleware::{self, EmbedSource, Mw, MwConfig, forwarded::TrustedHosts},
    pyjson, request_id,
    state::{AppState, Inner},
    validation::{Params, read_body, validate},
};

fn golden() -> &'static Value {
    static G: OnceLock<Value> = OnceLock::new();
    G.get_or_init(|| serde_json::from_str(include_str!("../../testdata/modpacks/golden.json")).expect("fixture"))
}

fn s(v: &Value) -> &str {
    v.as_str().unwrap_or_else(|| panic!("string: {v}"))
}

fn arr<'a>(v: &'a Value, key: &str) -> &'a Vec<Value> {
    v[key].as_array().unwrap_or_else(|| panic!("section {key}"))
}

fn b64(v: &Value) -> Vec<u8> {
    base64::engine::general_purpose::STANDARD.decode(s(v)).unwrap()
}

fn bdoc(v: &Value) -> Document {
    match Bson::try_from(v.clone()).expect("extjson") {
        Bson::Document(d) => d,
        other => panic!("not a document: {other:?}"),
    }
}

/// Canonical extended JSON text, key order included.
fn canon(d: Document) -> String {
    serde_json::to_string(&Bson::Document(d).into_canonical_extjson()).unwrap()
}

fn now() -> DateTime<Utc> {
    crate::dt::parse_utc(s(&golden()["now"])).unwrap()
}

fn err_json(e: &ApiError) -> Value {
    serde_json::json!({"status": e.status.as_u16(), "code": e.code, "message": e.message})
}

fn pack(name: &str) -> Pack {
    Pack::from_doc(&bdoc(&golden()["docs"][name]["in"])).unwrap()
}

fn settings() -> Settings {
    let vars: Vec<(String, String)> =
        golden()["env"].as_object().unwrap().iter().map(|(k, v)| (k.clone(), s(v).to_owned())).collect();
    Settings::from_vars(vars).expect("settings")
}

fn state() -> AppState {
    AppState { inner: Arc::new(Inner { settings: settings(), db: Db { mongo: None, redis: None, pg: None } }) }
}

fn bases(st: &AppState) -> Bases<'_> {
    Bases::of(st)
}

fn view_from(v: &Value) -> EntryView {
    let opt = |k: &str| v[k].as_str().map(str::to_owned);
    let reason = v["reason"].as_str().map(|r| match r {
        "missing" => "missing",
        "removed" => "removed",
        "unavailable" => "unavailable",
        "no build" => "no build",
        "not a .tmod" => "not a .tmod",
        other => panic!("reason {other}"),
    });
    EntryView {
        custom: v["custom"].as_bool().unwrap_or(false),
        custom_sha: opt("custom_sha"),
        custom_filename: opt("custom_filename"),
        handle: s(&v["handle"]).into(),
        slug: s(&v["slug"]).into(),
        title: s(&v["title"]).into(),
        author: v["author"].as_str().unwrap_or("").into(),
        branch: s(&v["branch"]).into(),
        version_locked: v["version_locked"].as_bool().unwrap(),
        locked_tag: opt("locked_tag"),
        version: opt("version"),
        is_beta: v["is_beta"].as_bool().unwrap_or(false),
        available: v["available"].as_bool().unwrap(),
        reason,
    }
}

#[test]
fn strings() {
    let g = &golden()["strings"];
    for c in arr(g, "slugify") {
        assert_eq!(text::slugify(s(&c[0])), s(&c[1]), "slugify {}", c[0]);
    }
    for c in arr(g, "tags") {
        let input: Vec<String> = c[0].as_array().unwrap().iter().map(|t| s(t).to_owned()).collect();
        let want: Vec<String> = c[1].as_array().unwrap().iter().map(|t| s(t).to_owned()).collect();
        assert_eq!(text::clean_tags(&input), want);
    }
    for c in arr(g, "urls") {
        let got = match text::clean_url(c[0].as_str(), s(&c[1])) {
            Ok(v) => serde_json::json!({"ok": v}),
            Err(e) => serde_json::json!({"error": err_json(&e)}),
        };
        assert_eq!(got, c[2], "url {}", c[0]);
    }
    for c in arr(g, "filenames") {
        let name = c[0].as_str().unwrap_or("");
        assert_eq!(text::safe_filename(name, "modpack"), s(&c[1]), "{name:?}");
        assert_eq!(text::safe_filename(name, "mod"), s(&c[2]), "{name:?}");
    }
    for c in arr(g, "escape") {
        assert_eq!(text::re_escape(s(&c[0])), s(&c[1]));
    }
    for c in arr(g, "quote") {
        assert_eq!(text::quote(s(&c[0])), s(&c[1]));
    }
    for c in arr(g, "casefold") {
        assert_eq!(text::casefold(s(&c[0])), s(&c[1]), "casefold {}", c[0]);
    }
    let mut input: Vec<&str> = arr(g, "sort_input").iter().map(s).collect();
    input.sort_by_cached_key(|t| text::casefold(t));
    let want: Vec<&str> = arr(g, "sorted").iter().map(s).collect();
    assert_eq!(input, want);
}

#[test]
fn casefold_matches_python_everywhere_sampled() {
    assert_eq!(text::casefold("ΣΑΣ ς ẞ ﬃ İ"), "σασ σ ss ffi i\u{307}");
    assert_eq!(text::casefold("\u{10400}\u{1e921}"), "\u{10428}\u{1e943}");
    assert_eq!(text::casefold("plain ascii 123"), "plain ascii 123");
}

#[test]
fn documents() {
    let g = &golden()["docs"];
    for name in ["full", "minimal", "legacy"] {
        let p = pack(name);
        assert_eq!(canon(p.to_doc()), canon(bdoc(&g[name]["stored"])), "{name}");
        assert_eq!(Pack::from_doc(&p.to_doc()).unwrap(), p, "{name} round trip");
    }
    for c in arr(g, "invalid") {
        assert_eq!(Pack::from_doc(&bdoc(&c["in"])).is_ok(), c["valid"].as_bool().unwrap(), "{}", c["in"]);
    }
}

#[test]
fn resolution() {
    for c in arr(golden(), "resolution") {
        let name = s(&c["name"]);
        let e = {
            let mut d = Document::new();
            d.insert("_id", ObjectId::new());
            d.insert("slug", "s");
            d.insert("title", "t");
            d.insert("owner_id", ObjectId::new());
            d.insert("owner_username", "o");
            d.insert(
                "variants",
                vec![Bson::Document(mongodb::bson::doc! {"name": "v", "entries": [bdoc(&c["entry"])]})],
            );
            Pack::from_doc(&d).unwrap().variants[0].entries[0].clone()
        };
        let m = (!c["mod"].is_null()).then(|| ModRef::from_doc(&bdoc(&c["mod"])).unwrap());
        let rel = (!c["release"].is_null()).then(|| Release::from_doc(&bdoc(&c["release"])).unwrap());
        let mut calls: Vec<&str> = Vec::new();
        let (view, got) = match e.custom() {
            Some(sha) => {
                calls.push("blob");
                (service::custom_view(&e, sha, c["present"].as_bool().unwrap()), None)
            }
            None => {
                if e.project_id.is_some() {
                    calls.push("mod");
                }
                let rel = match &m {
                    Some(m) if !m.unavailable() => {
                        calls.push(if service::locked_tag(&e).is_some() { "locked" } else { "latest" });
                        rel.clone()
                    }
                    _ => None,
                };
                service::hub_view(&e, m.as_ref(), rel)
            }
        };
        let want_calls: Vec<&str> = arr(c, "calls").iter().map(s).collect();
        assert_eq!(calls, want_calls, "{name} lookups");
        assert_eq!(pyjson::pydantic(&view.to_json()), s(&c["view"]), "{name}");
        assert_eq!(got.is_some(), c["has_release"].as_bool().unwrap(), "{name}");
        assert_eq!(got.as_ref().map(hub::release_download_filename).as_deref(), c["download_name"].as_str(), "{name}");
        assert_eq!(view_from(&serde_json::from_str(s(&c["view"])).unwrap()), view, "{name} parse");
    }
}

#[test]
fn views() {
    let g = &golden()["views"];
    let st = state();
    let views: HashMap<&str, EntryView> = g["views"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| (k.as_str(), view_from(&serde_json::from_str(s(v)).unwrap())))
        .collect();
    let keys_for = |vname: &str, n: usize| -> Vec<String> {
        match g["variant_views"].get(vname) {
            Some(list) => list.as_array().unwrap().iter().map(|k| s(k).to_owned()).collect(),
            None => vec!["a".to_owned(); n],
        }
    };
    let viewers: HashMap<&str, Option<ObjectId>> = [
        ("anon", None),
        ("owner", Some(ObjectId::parse_str("65d000000000000000000001").unwrap())),
        ("collab", Some(ObjectId::parse_str("65d000000000000000000002").unwrap())),
        ("other", Some(ObjectId::parse_str("65d000000000000000000003").unwrap())),
    ]
    .into_iter()
    .collect();
    for c in arr(g, "cases") {
        let p = pack(s(&c["doc"]));
        let name = s(&c["doc"]);
        assert_eq!(pyjson::pydantic(&Value::Object(service::pack_card(&p))), s(&c["card"]), "{name} card");
        assert_eq!(
            pyjson::pydantic(&Value::Object(service::public_pack_card(bases(&st), &p))),
            s(&c["public_card"]),
            "{name} public card"
        );
        assert_eq!(pyjson::pydantic(&service::master_card(&p)), s(&c["master"]), "{name} master");
        let per_variant: Vec<Vec<EntryView>> = p
            .variants
            .iter()
            .map(|v| keys_for(&v.name, v.entries.len()).iter().map(|k| views[k.as_str()].clone()).collect())
            .collect();
        let public_variants: Vec<Value> = p
            .variants
            .iter()
            .zip(&per_variant)
            .map(|(v, vs)| service::public_variant_json(bases(&st), &p, v, vs))
            .collect();
        assert_eq!(
            pyjson::pydantic(&Value::Object(service::public_detail_json(bases(&st), &p, public_variants))),
            s(&c["public"]),
            "{name} public detail"
        );
        for (key, want) in c["details"].as_object().unwrap() {
            let (who, starred) = key.split_once(':').unwrap();
            let viewer = viewers[who];
            let variants: Vec<Value> =
                p.variants.iter().zip(&per_variant).map(|(v, vs)| service::variant_json(v, vs.clone())).collect();
            let got = service::detail_json(&p, variants, viewer, starred == "True");
            assert_eq!(pyjson::pydantic(&Value::Object(got)), s(want), "{name} detail {key}");
        }
        for (who, want) in c["can_view"].as_object().unwrap() {
            assert_eq!(p.can_view(viewers[who.as_str()]), want.as_bool().unwrap(), "{name} can_view {who}");
        }
    }
}

fn tmods() -> HashMap<String, Vec<u8>> {
    arr(&golden()["configs"], "tmods").iter().map(|t| (s(&t["name"]).to_owned(), b64(&t["tmod"]))).collect()
}

#[test]
fn configs_and_conflicts() {
    let g = &golden()["configs"];
    for t in arr(g, "tmods") {
        let data = b64(&t["tmod"]);
        let got = hub::pack_config_for_artifact(&data)
            .map(|(n, d)| serde_json::json!([n, base64::engine::general_purpose::STANDARD.encode(d)]));
        assert_eq!(got.unwrap_or(Value::Null), t["config"], "{}", t["name"]);
        let fp = service::fingerprint(&data).map(|(title, files)| serde_json::json!([title, files]));
        assert_eq!(fp.unwrap_or(Value::Null), t["fingerprint"], "{}", t["name"]);
    }
    let all = tmods();
    for p in arr(g, "pairs") {
        let (a, b) = (s(&p["a"]), s(&p["b"]));
        let fa = service::fingerprint(&all[a]).unwrap();
        let fb = service::fingerprint(&all[b]).unwrap();
        let got = service::pair_conflicts(&format!("“{a}”"), &fa, b, &fb);
        let want: Vec<&str> = arr(p, "messages").iter().map(s).collect();
        assert_eq!(got, want, "{a} vs {b}");
    }
}

#[test]
fn artifacts() {
    let g = &golden()["artifacts"];
    let z = arr(g, "zip_time");
    let when = [0, 1, 2, 3, 4, 5].map(|i| z[i].as_u64().unwrap() as u16);
    let all = tmods();
    let blobs: HashMap<String, Vec<u8>> =
        g["blobs"].as_object().unwrap().iter().map(|(sha, name)| (sha.clone(), all[s(name)].clone())).collect();
    let entries = arr(g, "entries");
    for c in arr(g, "cases") {
        let label = format!("{} {} {}", s(&c["set"]), s(&c["title"]), s(&c["format"]));
        let idx: Vec<usize> =
            g["sets"][s(&c["set"])].as_array().unwrap().iter().map(|i| i.as_u64().unwrap() as usize).collect();
        let mut chosen: Vec<Entry> = Vec::new();
        let mut resolved = Vec::new();
        for &i in &idx {
            let e = &entries[i];
            let d = bdoc(&e["entry"]);
            let entry = Entry {
                project_id: d.get_object_id("project_id").ok(),
                handle: d.get_str("handle").unwrap_or("").into(),
                slug: d.get_str("slug").unwrap_or("").into(),
                title: d.get_str("title").unwrap_or("").into(),
                branch: d.get_str("branch").unwrap_or("main").into(),
                version_locked: d.get_bool("version_locked").unwrap_or(false),
                locked_tag: d.get_str("locked_tag").ok().map(str::to_owned),
                custom_sha: d.get_str("custom_sha").ok().map(str::to_owned),
                custom_filename: d.get_str("custom_filename").ok().map(str::to_owned),
                author: d.get_str("author").unwrap_or("").into(),
            };
            let v = &e["view"];
            let view = EntryView {
                custom: entry.custom().is_some(),
                custom_sha: entry.custom_sha.clone(),
                custom_filename: entry.custom_filename.clone(),
                handle: s(&v["handle"]).into(),
                slug: s(&v["slug"]).into(),
                title: s(&v["title"]).into(),
                author: String::new(),
                branch: s(&v["branch"]).into(),
                version_locked: v["version_locked"].as_bool().unwrap(),
                locked_tag: None,
                version: v["version"].as_str().map(str::to_owned),
                is_beta: false,
                available: v["available"].as_bool().unwrap(),
                reason: None,
            };
            let rel = (!e["release"].is_null()).then(|| Release::from_doc(&bdoc(&e["release"])).unwrap());
            chosen.push(entry);
            resolved.push((view, rel));
        }
        let (downloadable, mut records) = service::collect_builds(&chosen, &resolved, &blobs);
        let meta = Meta {
            title: s(&c["title"]).into(),
            handle: "owner".into(),
            slug: "my-pack".into(),
            author: "Owner Name".into(),
            variant: s(&c["variant"]).into(),
            warnings: s(&c["warnings"]).into(),
        };
        let got = if downloadable.is_empty() {
            Err(ApiError::bad_request(
                "This modpack has no downloadable mods right now (every entry is missing a published build).",
            ))
        } else {
            let configs = artifact::collect_configs(&downloadable, &mut records);
            artifact::assemble(&meta, &downloadable, &records, &configs, s(&c["format"]), when)
        };
        match got {
            Ok((blob, name, media)) => {
                assert_eq!(name, s(&c["filename"]), "{label}");
                assert_eq!(media, s(&c["media"]), "{label}");
                assert!(blob == b64(&c["blob"]), "{label}: bytes differ");
            }
            Err(e) => assert_eq!(err_json(&e), c["error"], "{label}"),
        }
    }
}

#[test]
fn indent2_matches_json_dumps() {
    let v = serde_json::json!({"a": [], "b": {}, "c": [1, {"d": "é\n"}], "e": null, "f": true});
    assert_eq!(
        artifact::dumps_indent2(&v),
        "{\n  \"a\": [],\n  \"b\": {},\n  \"c\": [\n    1,\n    {\n      \"d\": \"\\u00e9\\n\"\n    }\n  ],\n  \"e\": null,\n  \"f\": true\n}"
    );
}

fn json_body<T: crate::validation::FromBody>(v: &Value) -> T {
    validate::<T>(&crate::validation::BodyInput::Json(v.clone())).unwrap_or_else(|e| panic!("body {v}: {e}"))
}

fn actor(id: &str, username: &str, display: &str) -> SiteUser {
    mongodb::bson::from_document(mongodb::bson::doc! {
        "_id": ObjectId::parse_str(id).unwrap(), "username": username, "display_name": display,
    })
    .unwrap()
}

fn fields_without_id(d: Document) -> String {
    let mut d = d;
    d.remove("_id");
    canon(d)
}

fn check(label: &str, case: &Value, res: Result<Option<Pack>, ApiError>) {
    match res {
        Ok(Some(p)) => assert_eq!(fields_without_id(p.to_doc()), fields_without_id(bdoc(&case["stored"])), "{label}"),
        Ok(None) => assert!(case["stored"].is_null(), "{label}: Python saved"),
        Err(e) => assert_eq!(err_json(&e), case["error"], "{label}"),
    }
}

#[test]
fn writes() {
    let g = &golden()["writes"];
    let now = now();
    for c in arr(g, "create") {
        let a = arr(c, "actor");
        let actor = actor(s(&a[0]), s(&a[1]), s(&a[2]));
        let req: CreateBody = json_body(&c["body"]);
        let slug = format!("{}-2", text::slugify(&req.title));
        check("create", c, Ok(Some(service::new_pack(&actor, &req, slug, now, now))));
    }
    for c in arr(g, "update") {
        let mut p = pack("full");
        let req: UpdateBody = json_body(&c["body"]);
        let res = service::apply_update(&mut p, "owner_now", &req, now).map(|_| Some(p));
        check(&format!("update {}", c["body"]), c, res);
    }
    for c in arr(g, "variants") {
        let mut p = pack("full");
        let args = arr(c, "args");
        let op = s(&c["op"]);
        let res = match op {
            "add" => service::apply_add_variant(&mut p, s(&args[0]), args[1].as_str(), now),
            "update" => service::apply_update_variant(&mut p, s(&args[0]), args[1].as_str(), now),
            "delete" => service::apply_delete_variant(&mut p, s(&args[0]), now),
            "add_31" => {
                let vs = p.variants.clone();
                p.variants = (0..10).flat_map(|_| vs.clone()).collect();
                service::apply_add_variant(&mut p, "Another", None, now)
            }
            "delete_last" => {
                p.variants.truncate(1);
                service::apply_delete_variant(&mut p, "default", now)
            }
            other => panic!("op {other}"),
        };
        check(&format!("variant {op} {args:?}"), c, res.map(|_| Some(p)));
    }
    let owner = ObjectId::parse_str("65d000000000000000000001").unwrap();
    let users: HashMap<&str, ObjectId> = [
        ("newbie", ObjectId::parse_str("65d000000000000000000003").unwrap()),
        ("collab", ObjectId::parse_str("65d000000000000000000002").unwrap()),
        ("owner", owner),
    ]
    .into_iter()
    .collect();
    for c in arr(g, "collaborators") {
        let mut p = pack("full");
        let name = s(&c["name"]);
        let op = s(&c["op"]);
        let res: Result<Option<Pack>, ApiError> = match op {
            "add" | "add_21" => {
                if op == "add_21" {
                    p.collaborators = arr(c, "pack_collaborators")
                        .iter()
                        .map(|v| {
                            let d = bdoc(v);
                            super::model::Collaborator {
                                user_id: d.get_object_id("user_id").unwrap(),
                                username: d.get_str("username").unwrap().into(),
                            }
                        })
                        .collect();
                }
                let uname = service::collaborator_name(name);
                if uname.is_empty() {
                    Err(ApiError::bad_request("Enter a username to collaborate with."))
                } else {
                    match users.get(uname.as_str()) {
                        None => {
                            Err(ApiError::not_found(format!("No site user '@{uname}'. They must sign in once first.")))
                        }
                        Some(&uid) => service::apply_add_collaborator(&mut p, uid, &uname, now)
                            .map(|changed| changed.then_some(p)),
                    }
                }
            }
            "remove" => {
                service::apply_remove_collaborator(&mut p, name, now);
                Ok(Some(p))
            }
            "remove_as_collab" => service::require_primary_owner(&p, users["collab"]).map(|_| Some(p)),
            other => panic!("op {other}"),
        };
        check(&format!("collaborator {op} {name}"), c, res);
    }
}

#[test]
fn set_entries_resolution() {
    let m = ModRef {
        id: ObjectId::parse_str("65e000000000000000000001").unwrap(),
        slug: "mod-a".into(),
        title: "Mod A".into(),
        owner_username: "Alice".into(),
        owner_handle: "alice".into(),
        visibility: "public".into(),
        is_beta: false,
        taken_down: false,
    };
    let req: SetEntriesBody = json_body(&serde_json::json!({"entries": [
        {"handle": "alice", "slug": "mod-a", "branch": "  ", "version_locked": true, "locked_tag": ""},
        {"handle": "alice", "slug": "mod-a", "branch": "main", "locked_tag": "v1"},
        {"custom_sha": "a".repeat(64), "title": "", "custom_filename": ""},
        {"custom_sha": "a".repeat(64), "title": "Again"},
        {"handle": "alice", "slug": "mod-a", "branch": " beta ", "version_locked": true, "locked_tag": "v2"},
    ]}));
    let inputs = req
        .entries
        .iter()
        .map(|item| match &item.custom_sha {
            Some(_) => service::Input::Custom { item, present: true },
            None => service::Input::Hub { item, m: Some(m.clone()) },
        })
        .collect();
    let got = service::resolve_inputs(inputs).unwrap();
    assert_eq!(got.len(), 3);
    assert_eq!((got[0].branch.as_str(), got[0].version_locked, got[0].locked_tag.as_deref()), ("main", true, None));
    assert_eq!((got[1].title.as_str(), got[1].custom_filename.as_deref()), ("Uploaded mod", Some("")));
    assert_eq!((got[2].branch.as_str(), got[2].locked_tag.as_deref()), ("beta", Some("v2")));
    let mut hidden = m.clone();
    hidden.visibility = "draft".into();
    let item = &req.entries[0];
    let e = service::resolve_inputs(vec![service::Input::Hub { item, m: Some(hidden) }]).unwrap_err();
    assert_eq!(e.message, "Mod 'alice/mod-a' isn't available to add.");
}

#[test]
fn upload_vetting() {
    let all = tmods();
    assert_eq!(service::vet_upload(b"").unwrap_err().message, "The uploaded file is empty.");
    assert_eq!(service::vet_upload(&all["garbage"]).unwrap_err().message, "That isn't a valid .tmod file.");
    assert_eq!(service::vet_upload(&all["untitled"]).unwrap_err().message, "The .tmod has no title in its header.");
    let (fp, title, author) = service::vet_upload(&all["preview"]).unwrap();
    assert_eq!((fp.0.as_str(), title.as_str(), author.as_str()), ("preview mod", "Preview Mod", "Au"));
    let add = service::upload_addition(&[], &"f".repeat(64), "A: B?", "Au", None).unwrap();
    assert_eq!(add.custom_filename.as_deref(), Some("A B.tmod"));
    assert!(service::upload_addition(std::slice::from_ref(&add), &"f".repeat(64), "A", "", None).is_none());
}

// --- HTTP ------------------------------------------------------------------

fn app(state: AppState) -> Router {
    let mw =
        Mw::new(MwConfig::from_lookup(&|_| None), None, EmbedSource::Fixed(vec![]), TrustedHosts::parse("127.0.0.1"));
    let inner = middleware::wrap(super::router().merge(crate::mods_moderation::router()), Arc::new(mw), None)
        .fallback(|| async { StatusCode::IM_A_TEAPOT })
        .with_state(state);
    middleware::outer(inner)
}

async fn parts(resp: Response) -> (u16, HeaderMap, Vec<u8>) {
    let status = resp.status().as_u16();
    let headers = resp.headers().clone();
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.expect("body");
    (status, headers, body.to_vec())
}

fn latin1(s: &str) -> Vec<u8> {
    s.chars().map(|c| c as u32 as u8).collect()
}

/// A signed-in case: what the route checks once the session resolved.
async fn after_auth(method: &str, url: &str, headers: &HeaderMap, body: Vec<u8>) -> Response {
    let path = url.split('?').next().unwrap();
    if path.ends_with("/upload") || path.ends_with("/banner") {
        let ct = headers.get("content-type").map(|v| v.as_bytes());
        return match kiwi_core::form::parse_form(ct, &body) {
            Ok(items) => match crate::images::upload_file(items) {
                Ok(_) => StatusCode::OK.into_response(),
                Err(e) => e.into_response(),
            },
            Err(kiwi_core::form::FormError::Http(m)) => {
                ApiError::http(StatusCode::BAD_REQUEST, Some(&m)).into_response()
            }
        };
    }
    if path == "/admin/modpacks" {
        let mut p = Params::new(Vec::new(), url.split_once('?').map(|(_, q)| q), headers);
        let q = <AdminListQuery as crate::validation::FromParams>::from_params(&mut p);
        return match p.v.finish(q) {
            Ok(_) => StatusCode::OK.into_response(),
            Err(e) => e.into_response(),
        };
    }
    let input = match read_body(headers, Body::from(body)).await {
        Ok(i) => i,
        Err(e) => return e.into_response(),
    };
    let res = match (method, path) {
        ("POST", "/v1/modpacks/hub/projects") => validate::<CreateBody>(&input).map(|_| ()),
        ("PATCH", p) if p.contains("/variants/") => validate::<UpdateVariantBody>(&input).map(|_| ()),
        ("PATCH", _) => validate::<UpdateBody>(&input).map(|_| ()),
        ("POST", p) if p.ends_with("/variants") => validate::<CreateVariantBody>(&input).map(|_| ()),
        ("PUT", _) => validate::<SetEntriesBody>(&input).map(|_| ()),
        ("POST", p) if p.ends_with("/collaborators") => validate::<CollaboratorBody>(&input).map(|_| ()),
        ("POST", p) if p.ends_with("/takedown") => validate::<TakedownBody>(&input).map(|_| ()),
        other => panic!("no route for {other:?}"),
    };
    match res {
        Ok(()) => StatusCode::OK.into_response(),
        Err(e) => e.into_response(),
    }
}

#[tokio::test]
async fn http_replay() {
    let app = app(state());
    let mut bad = Vec::new();
    for c in arr(golden(), "http") {
        let name = s(&c["name"]);
        let mut headers = HeaderMap::new();
        if let Some(ct) = c["content_type"].as_str() {
            headers.insert("content-type", HeaderValue::from_str(ct).unwrap());
        }
        headers.insert("x-request-id", HeaderValue::from_str(s(&c["request_id"])).unwrap());
        let body = latin1(s(&c["body"]));
        let (status, got_headers, out) = if c["who"].is_null() {
            let mut req = Request::builder()
                .method(Method::from_bytes(s(&c["method"]).as_bytes()).unwrap())
                .uri(s(&c["url"]))
                .body(Body::from(body))
                .unwrap();
            *req.headers_mut() = headers;
            parts(app.clone().oneshot(req).await.unwrap()).await
        } else {
            let (m, u) = (s(&c["method"]).to_owned(), s(&c["url"]).to_owned());
            let resp =
                request_id::scope(s(&c["request_id"]).to_owned(), async { after_auth(&m, &u, &headers, body).await })
                    .await;
            parts(resp).await
        };
        let out = String::from_utf8(out).unwrap();
        let mut mismatch = u64::from(status) != c["status"].as_u64().unwrap() || out != s(&c["resp"]);
        for h in c["resp_headers"].as_array().unwrap() {
            let got = got_headers.get(s(&h[0])).map(|v| v.to_str().unwrap().to_owned());
            if c["who"].is_null() && got.as_deref() != Some(s(&h[1])) {
                mismatch = true;
            }
        }
        if mismatch {
            bad.push(format!("{name}: {status} {out} {got_headers:?}\n  want {} {}", c["status"], s(&c["resp"])));
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

#[test]
fn disposition_and_paths() {
    assert_eq!(
        super::routes::disposition("A é.zip").as_bytes().to_vec(),
        b"attachment; filename=\"A \xe9.zip\"".to_vec()
    );
    assert_eq!(
        super::routes::disposition("日本 pack.zip").as_bytes().to_vec(),
        b"attachment; filename=\"__ pack.zip\"; filename*=UTF-8''%E6%97%A5%E6%9C%AC%20pack.zip".to_vec()
    );
}

/// Starlette walks the routers in include order; matchit has to land on the
/// same route, backtracking from the `hub` literals to the catalog params.
#[tokio::test]
async fn routes_resolve_like_starlette() {
    use axum::{extract::MatchedPath, middleware::Next};
    let probe = super::router()
        .route_layer(axum::middleware::from_fn(|req: Request<Body>, _: Next| async move {
            req.extensions().get::<MatchedPath>().map(|m| m.as_str().to_owned()).unwrap_or_default().into_response()
        }))
        .with_state(state());
    for (path, want) in [
        ("/v1/modpacks/hub/projects", "/v1/modpacks/hub/projects"),
        ("/v1/modpacks/hub/me/projects", "/v1/modpacks/hub/me/projects"),
        ("/v1/modpacks/hub/me", "/v1/modpacks/{handle}/{slug}"),
        ("/v1/modpacks/hub/x/download", "/v1/modpacks/{handle}/{slug}/download"),
        ("/v1/modpacks/hub/projects/download", "/v1/modpacks/{handle}/{slug}/download"),
        ("/v1/modpacks/hub/me/download", "/v1/modpacks/{handle}/{slug}/download"),
        ("/v1/modpacks/for-mod/x/download", "/v1/modpacks/for-mod/{handle}/{slug}"),
        ("/v1/modpacks/for-mod/x", "/v1/modpacks/{handle}/{slug}"),
        ("/v1/modpacks/hub/projects/a/b/download", "/v1/modpacks/hub/projects/{handle}/{slug}/download"),
        ("/site/modpacks/projects/me/projects", "/site/modpacks/projects/{handle}/{slug}"),
        ("/site/modpacks/me/projects", "/site/modpacks/me/projects"),
    ] {
        let req = Request::builder().uri(path).body(Body::empty()).unwrap();
        let (status, _, body) = parts(probe.clone().oneshot(req).await.unwrap()).await;
        assert_eq!((status, String::from_utf8(body).unwrap()), (200, want.to_owned()), "{path}");
    }
}

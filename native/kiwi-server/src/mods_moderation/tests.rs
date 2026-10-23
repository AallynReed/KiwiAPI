//! Golden checks against `native/port/parity/gen_mods_moderation_fixtures.py`:
//! Beanie's load and `save()` shapes, every listing rebuilt from the documents
//! Python read (with the filter, sort and paging it sent), every write's `$set`
//! / upsert / filter rebuilt from the document it started from, the takedown
//! emails, the stray import's per-mod documents and run outcomes, and the
//! routing, 401s, 405s and 422s through the middleware.

use std::{collections::HashMap, sync::OnceLock};

use axum::{
    Router,
    body::Body,
    extract::Request,
    http::{HeaderMap, Method, StatusCode, Uri},
};
use chrono::{DateTime, Utc};
use mongodb::bson::{self, Bson, Document, doc, oid::ObjectId};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tower::ServiceExt;

use super::{
    AssignBody, ImportQuery, MasterQuery, PackQuery, ResolvedQuery, StrayQuery, TakedownBody,
    model::{
        CLAIM, CONTENT_REPORT, F, IMAGE_ASSET, MOD_PROFILE, MOD_PROJECT, MOD_RELEASE, MODPACK, Model, REMIX_CLAIM,
        STRAY_STATE, TAKEN_DOWN,
    },
    service::{self as svc},
    strayimport as stray,
};
use crate::{
    admin::site_users::StatusQuery,
    auth::models::SiteUser,
    config::Settings,
    db::Db,
    dt,
    error::ApiError,
    middleware::{EmbedSource, Mw, MwConfig, forwarded::TrustedHosts, outer, wrap},
    pyjson,
    state::{AppState, Inner},
    validation::{BodyInput, ErrorItem, FromParams, Params, ValidationError, validate},
};

fn golden() -> &'static Value {
    static G: OnceLock<Value> = OnceLock::new();
    G.get_or_init(|| {
        serde_json::from_str(include_str!("../../testdata/mods_moderation/golden.json")).expect("fixture parses")
    })
}

fn s(v: &Value) -> &str {
    v.as_str().expect("string")
}

fn ext_doc(v: &Value) -> Document {
    match Bson::try_from(v.clone()).expect("extended JSON") {
        Bson::Document(d) => d,
        other => panic!("not a document: {other}"),
    }
}

fn ext_text(b: Bson) -> String {
    serde_json::to_string(&b.into_canonical_extjson()).unwrap()
}

fn doc_text(d: &Document) -> String {
    ext_text(Bson::Document(d.clone()))
}

fn same(v: &Value) -> String {
    serde_json::to_string(v).unwrap()
}

fn now() -> DateTime<Utc> {
    dt::parse_utc(s(&golden()["now"])).unwrap()
}

fn admin_id() -> ObjectId {
    ObjectId::parse_str(s(&golden()["admin_id"])).unwrap()
}

fn assert_body(v: &Value, case: &Value, what: &str) {
    assert_eq!(pyjson::pydantic(v), s(&case["body"]), "{what}");
}

fn assert_error(e: &ApiError, case: &Value, what: &str) {
    let want: Value = serde_json::from_str(s(&case["body"])).unwrap();
    assert_eq!(u64::from(e.status.as_u16()), case["status"].as_u64().unwrap(), "{what}: status");
    assert_eq!(e.code, s(&want["error"]["code"]), "{what}: code");
    assert_eq!(e.message, s(&want["error"]["message"]), "{what}: message");
    assert_eq!(e.details.clone().unwrap_or(Value::Null), want["error"]["details"], "{what}: details");
}

fn step(name: &str) -> &'static Value {
    golden()["scenario"]["steps"].as_array().unwrap().iter().find(|c| c["name"] == name).expect(name)
}

fn ops<'a>(case: &'a Value, op: &str, coll: &str) -> Vec<&'a Value> {
    case["ops"].as_array().unwrap().iter().filter(|o| o["op"] == op && o["coll"] == coll).collect()
}

fn one<'a>(case: &'a Value, op: &str, coll: &str) -> &'a Value {
    let found = ops(case, op, coll);
    assert_eq!(found.len(), 1, "{} {op} {coll}", case["name"]);
    found[0]
}

fn load(spec: &'static [F], v: &Value) -> Model {
    Model::load_at(spec, &ext_doc(v), now()).expect("loads")
}

fn models(spec: &'static [F], v: &Value) -> Vec<Model> {
    v.as_array().unwrap().iter().map(|d| load(spec, d)).collect()
}

fn query_of<T: FromParams>(url: &str) -> T {
    let uri: Uri = url.parse().unwrap();
    let headers = HeaderMap::new();
    let mut p = Params::new(Vec::new(), uri.query(), &headers);
    let v = T::from_params(&mut p);
    p.v.finish(v).map_err(|e| format!("{:?}", e.0.len())).expect("valid query")
}

fn spec_for(coll: &str) -> &'static [F] {
    match coll {
        "mod_projects" => MOD_PROJECT,
        "mod_releases" => MOD_RELEASE,
        "mod_profiles" => MOD_PROFILE,
        "mod_claim_requests" => CLAIM,
        "mod_remix_claims" => REMIX_CLAIM,
        "content_reports" => CONTENT_REPORT,
        "modpack_projects" => MODPACK,
        other => panic!("no spec for {other}"),
    }
}

/// Every `save()` of the step, rebuilt by `mutate` from the document it started from.
fn check_saves(name: &str, mutate: &dyn Fn(&str, &mut Model)) -> Vec<Model> {
    let case = step(name);
    let mut out = Vec::new();
    for op in case["ops"].as_array().unwrap().iter().filter(|o| o["op"] == "save") {
        let coll = s(&op["coll"]);
        let mut m = load(spec_for(coll), &op["before"]);
        mutate(coll, &mut m);
        assert_eq!(doc_text(&m.to_doc()), same(&op["update"]["$set"]), "{name}: {coll} $set");
        m.saved();
        out.push(m);
    }
    out
}

fn site_user_of(case: &Value, id: &str) -> SiteUser {
    let hit = ops(case, "find_one", "site_users")
        .into_iter()
        .find(|o| o["result"]["_id"]["$oid"] == id)
        .expect("site user read");
    bson::from_document(ext_doc(&hit["result"])).unwrap()
}

// --- models -----------------------------------------------------------------------------

#[test]
fn model_normalization() {
    let specs: [(&str, &'static [F]); 10] = [
        ("mod_project", MOD_PROJECT),
        ("mod_release", MOD_RELEASE),
        ("mod_profile", MOD_PROFILE),
        ("claim", CLAIM),
        ("remix_claim", REMIX_CLAIM),
        ("content_report", CONTENT_REPORT),
        ("taken_down", TAKEN_DOWN),
        ("stray_state", STRAY_STATE),
        ("image_asset", IMAGE_ASSET),
        ("modpack", MODPACK),
    ];
    let mut n = 0;
    for (name, spec) in specs {
        for (i, row) in golden()["models"][name].as_array().unwrap().iter().enumerate() {
            let got = Model::load_at(spec, &ext_doc(&row["raw"]), now());
            if row["error"] == true {
                assert!(got.is_err(), "{name} #{i} should fail");
            } else {
                assert_eq!(doc_text(&got.expect("loads").to_doc()), same(&row["saved"]), "{name} #{i}");
            }
            n += 1;
        }
    }
    assert!(n >= 25);
}

// --- listings -------------------------------------------------------------------------------

#[test]
fn report_lists() {
    for (name, resolved) in [("reports", false), ("reports_resolved", true)] {
        let case = step(name);
        let q: ResolvedQuery = query_of(s(&case["url"]));
        assert_eq!(q.0, resolved);
        let find = one(case, "find", "content_reports");
        assert_eq!(doc_text(&doc! {"resolved": resolved}), same(&find["filter"]));
        assert_eq!(find["sort"], json!([["created_at", {"$numberInt": "-1"}]]));
        let items: Vec<Value> = models(CONTENT_REPORT, &find["result"]).iter().map(svc::report_dto).collect();
        assert_body(&json!({"items": items}), case, name);
    }
}

fn sort_value(spec: Option<Document>) -> Value {
    let mut out = Vec::new();
    for (k, v) in spec.unwrap_or_default() {
        out.push(json!([k, {"$numberInt": v.as_i32().unwrap().to_string()}]));
    }
    Value::Array(out)
}

fn int_value(n: i64) -> Value {
    json!({"$numberInt": n.to_string()})
}

#[test]
fn project_lists() {
    for name in ["projects", "projects_size", "projects_popular", "projects_stars", "projects_bogus"] {
        let case = step(name);
        let q: MasterQuery = query_of(s(&case["url"]));
        let filter = svc::master_filter(q.q.as_deref(), q.owner.as_deref(), q.visibility.as_deref(), true);
        let count = one(case, "count", "mod_projects");
        assert_eq!(doc_text(&filter), same(&count["filter"]), "{name}: filter");
        let find = one(case, "find", "mod_projects");
        assert_eq!(doc_text(&filter), same(&find["filter"]), "{name}: find filter");
        let by_size = svc::project_sort(&q.sort).is_none();
        assert_eq!(sort_value(svc::project_sort(&q.sort)), find["sort"], "{name}: sort");
        let (skip, limit) = if by_size { (0, 0) } else { (q.page.offset, q.page.limit) };
        assert_eq!((&find["skip"], &find["limit"]), (&int_value(skip), &int_value(limit)), "{name}: paging");
        let mut docs = models(MOD_PROJECT, &find["result"]);
        let mut sizes = HashMap::new();
        if !docs.is_empty() {
            let agg = one(case, "aggregate", "mod_releases");
            let ids: Vec<ObjectId> = docs.iter().map(|p| p.id).collect();
            let pipeline = Bson::Array(svc::sizes_pipeline(&ids).into_iter().map(Bson::Document).collect());
            assert_eq!(ext_text(pipeline), same(&agg["pipeline"]), "{name}: pipeline");
            let rows: Vec<Document> = agg["result"].as_array().unwrap().iter().map(ext_doc).collect();
            let images = match ops(case, "find", "mod_images").first() {
                Some(f) => {
                    let mut logged: Vec<String> =
                        f["filter"]["sha"]["$in"].as_array().unwrap().iter().map(|v| s(v).to_owned()).collect();
                    logged.sort();
                    assert_eq!(svc::size_shas(&docs), logged, "{name}: image shas");
                    models(IMAGE_ASSET, &f["result"])
                }
                None => {
                    assert!(svc::size_shas(&docs).is_empty());
                    Vec::new()
                }
            };
            sizes = svc::compute_sizes(&docs, &rows, &images);
        }
        if by_size {
            docs.sort_by_key(|p| std::cmp::Reverse(sizes.get(&p.id).copied().unwrap_or(0)));
            docs = docs.into_iter().skip(q.page.offset as usize).take(q.page.limit as usize).collect();
        }
        let items: Vec<Value> =
            docs.iter().map(|p| svc::master_card(p, sizes.get(&p.id).copied().unwrap_or(0))).collect();
        let n = items.len();
        let total = count["result"]["$numberInt"].as_str().unwrap().parse::<i64>().unwrap();
        assert_body(&json!({"items": items, "count": n, "total": total}), case, name);
    }
}

#[test]
fn stray_lists() {
    for name in ["stray", "stray_all", "stray_rejected"] {
        let case = step(name);
        let q: StrayQuery = query_of(s(&case["url"]));
        let filter = svc::stray_filter(q.status.as_deref(), q.q.as_deref());
        let count = one(case, "count", "mod_projects");
        assert_eq!(doc_text(&filter), same(&count["filter"]), "{name}: filter");
        let find = one(case, "find", "mod_projects");
        assert_eq!(find["sort"], json!([["updated_at", {"$numberInt": "-1"}]]));
        assert_eq!((&find["skip"], &find["limit"]), (&int_value(q.page.offset), &int_value(q.page.limit)));
        let items: Vec<Value> = models(MOD_PROJECT, &find["result"]).iter().map(svc::stray_card).collect();
        let n = items.len();
        let total = count["result"]["$numberInt"].as_str().unwrap().parse::<i64>().unwrap();
        assert_body(&json!({"items": items, "count": n, "total": total}), case, name);
    }
}

type ClaimList = (&'static str, &'static str, &'static [F], fn(&Model) -> Value);

#[test]
fn claim_lists() {
    let lists: [ClaimList; 4] = [
        ("claims", "mod_claim_requests", CLAIM, svc::claim_dto),
        ("claims_all", "mod_claim_requests", CLAIM, svc::claim_dto),
        ("remix_claims", "mod_remix_claims", REMIX_CLAIM, svc::remix_claim_dto),
        ("remix_claims_after", "mod_remix_claims", REMIX_CLAIM, svc::remix_claim_dto),
    ];
    for (name, coll, spec, dto) in lists {
        let case = step(name);
        let q: StatusQuery = query_of(s(&case["url"]));
        let find = one(case, "find", coll);
        assert_eq!(doc_text(&svc::status_filter(q.0.as_deref())), same(&find["filter"]), "{name}");
        assert_eq!(find["sort"], json!([["created_at", {"$numberInt": "-1"}]]));
        assert_eq!(find["limit"], int_value(100));
        let items: Vec<Value> = models(spec, &find["result"]).iter().map(dto).collect();
        assert_body(&json!({"items": items}), case, name);
    }
}

#[test]
fn modpack_lists() {
    for name in ["modpacks", "modpacks_q"] {
        let case = step(name);
        let PackQuery(q) = query_of(s(&case["url"]));
        let filter = svc::master_filter(q.q.as_deref(), q.owner.as_deref(), q.visibility.as_deref(), false);
        let count = one(case, "count", "modpack_projects");
        assert_eq!(doc_text(&filter), same(&count["filter"]), "{name}: filter");
        let find = one(case, "find", "modpack_projects");
        assert_eq!(find["sort"], json!([["updated_at", {"$numberInt": "-1"}]]));
        assert_eq!((&find["skip"], &find["limit"]), (&int_value(q.page.offset), &int_value(q.page.limit)));
        let items: Vec<Value> = models(MODPACK, &find["result"]).iter().map(svc::master_pack_card).collect();
        let n = items.len();
        let total = count["result"]["$numberInt"].as_str().unwrap().parse::<i64>().unwrap();
        assert_body(&json!({"items": items, "count": n, "total": total}), case, name);
    }
}

// --- writes ------------------------------------------------------------------------------------

#[test]
fn report_dismissal() {
    check_saves("dismiss", &|_, m| m.set("resolved", true));
    assert!(ops(step("dismiss_again"), "save", "content_reports").is_empty());
    assert!(step("dismiss_bad")["ops"].as_array().unwrap().is_empty());
    for name in ["dismiss", "dismiss_again", "dismiss_bad"] {
        assert_body(&json!({"ok": true}), step(name), name);
    }
}

fn takedown_reply(p: &Model) -> Value {
    json!({
        "slug": p.str("slug"), "handle": p.str("owner_handle"),
        "taken_down": p.bool("taken_down"), "takedown_reason": p.json_str_opt("takedown_reason"),
    })
}

fn email_matches(case: &Value, kind: &str, label: &str, reason: &str, url: &str) {
    let e = &case["emails"][0];
    let (subject, text, html) = svc::takedown_email(kind, label, reason, url);
    assert_eq!((s(&e["subject"]), s(&e["text"]), s(&e["html"])), (subject.as_str(), text.as_str(), html.as_str()));
    assert_eq!(e["to"], "owner@kiwi-mail.net");
}

#[test]
fn project_takedowns() {
    for (name, reason) in [("takedown", "  Stolen <b>assets</b>  "), ("takedown_default", "")] {
        let saved = check_saves(name, &|_, m| svc::apply_takedown(m, reason, now()));
        let p = &saved[0];
        let case = step(name);
        assert_body(&takedown_reply(p), case, name);
        let releases = models(MOD_RELEASE, &one(case, "find", "mod_releases")["result"]);
        let shas: Vec<String> = releases.iter().flat_map(svc::release_shas).collect();
        let upserts: Vec<&Value> =
            ops(case, "update_one", "mod_taken_down_artifacts").into_iter().filter(|o| o["upsert"] == true).collect();
        assert_eq!(upserts.len(), shas.len(), "{name}: upserts");
        for op in &upserts {
            let sha = s(&op["filter"]["sha256"]);
            assert!(shas.iter().any(|x| x == sha), "{name}: {sha}");
            let (f, u) = svc::takedown_upsert(sha, p, now());
            assert_eq!((doc_text(&f), doc_text(&u)), (same(&op["filter"]), same(&op["update"])), "{name}: upsert");
        }
        let (a, c) = ("a".repeat(64), "c".repeat(64));
        let blob = |sha: &str| -> Option<&[u8]> {
            if sha == a {
                Some(b"first release bytes")
            } else if sha == c {
                Some(b"")
            } else {
                None
            }
        };
        let pending = models(TAKEN_DOWN, &one(case, "find", "mod_taken_down_artifacts")["result"]);
        let fills: Vec<&Value> =
            ops(case, "update_one", "mod_taken_down_artifacts").into_iter().filter(|o| o["upsert"] == false).collect();
        assert_eq!(fills.len(), pending.len());
        for (row, op) in pending.iter().zip(fills) {
            let md5 = svc::md5_hex(blob(row.str("sha256")));
            assert_eq!(doc_text(&doc! {"_id": row.id}), same(&op["filter"]));
            assert_eq!(doc_text(&doc! {"$set": {"md5": md5}}), same(&op["update"]));
        }
        let resolve = one(case, "update_many", "content_reports");
        assert_eq!(doc_text(&svc::resolve_reports_filter("mod", p.id)), same(&resolve["filter"]));
        assert_eq!(resolve["update"], json!({"$set": {"resolved": true}}));
        let url = format!("/mods/{}/{}", p.str("owner_handle"), p.str("slug"));
        email_matches(case, "mod", p.str("title"), p.str("takedown_reason"), &url);
    }
    for name in ["takedown_missing", "takedown_bad_id", "restore_missing"] {
        assert_error(&ApiError::not_found("Mod project not found"), step(name), name);
    }
}

#[test]
fn project_restore() {
    let saved = check_saves("restore", &|_, m| svc::apply_restore(m, now()));
    let p = &saved[0];
    let case = step("restore");
    assert_body(&json!({"slug": p.str("slug"), "handle": p.str("owner_handle"), "taken_down": false}), case, "restore");
    let del = one(case, "delete_many", "mod_taken_down_artifacts");
    assert_eq!(doc_text(&doc! {"project_id": p.id}), same(&del["filter"]));
}

#[test]
fn profile_moderation() {
    let saved = check_saves("profile_takedown", &|_, m| svc::apply_takedown(m, "Spam", now()));
    let p = &saved[0];
    let case = step("profile_takedown");
    let reply = json!({"handle": p.str("handle"), "taken_down": true, "takedown_reason": "Spam"});
    assert_body(&reply, case, "profile takedown");
    let resolve = one(case, "update_many", "content_reports");
    assert_eq!(doc_text(&svc::resolve_reports_filter("profile", p.id)), same(&resolve["filter"]));
    email_matches(case, "profile", "owner", "Spam", "/mods/owner");
    let saved = check_saves("profile_restore", &|_, m| svc::apply_restore(m, now()));
    assert_body(&json!({"handle": saved[0].str("handle"), "taken_down": false}), step("profile_restore"), "restore");
    assert_error(&ApiError::not_found("Profile not found"), step("profile_missing"), "missing");
}

#[test]
fn stray_queue() {
    let saved = check_saves("stray_approve", &|_, m| svc::mark_stray(m, "approved", now()));
    let case = step("stray_approve");
    assert_body(&svc::stray_card(&saved[0]), case, "approve");
    let ann = one(case, "update_many", "mod_releases");
    let (f, u) = svc::announce_update(saved[0].id, now());
    assert_eq!((doc_text(&f), doc_text(&u)), (same(&ann["filter"]), same(&ann["update"])));
    let saved = check_saves("stray_reject", &|_, m| svc::mark_stray(m, "rejected", now()));
    assert_body(&svc::stray_card(&saved[0]), step("stray_reject"), "reject");
    assert_error(&ApiError::not_found("Stray mod not found"), step("stray_not_stray"), "not stray");
}

#[test]
fn claim_approval() {
    let case = step("claim_approve");
    let claim = load(CLAIM, &one(case, "find_one", "mod_claim_requests")["result"]);
    let user = site_user_of(case, &claim.oid("claimant_id").unwrap().to_hex());
    let saved = check_saves("claim_approve", &|coll, m| match coll {
        "mod_projects" => svc::apply_handover(m, &user, None, now()),
        _ => svc::resolve(m, "approved", admin_id(), now()),
    });
    let (project, claim) = (&saved[0], &saved[1]);
    let clash = ops(case, "find_one", "mod_projects")[1];
    assert_eq!(
        doc_text(&doc! {"$and": [{"owner_id": user.id}, {"slug": project.str("slug")}]}),
        same(&clash["filter"])
    );
    let updates = ops(case, "update_many", "mod_releases");
    assert_eq!(doc_text(&doc! {"project_id": project.id}), same(&updates[0]["filter"]));
    assert_eq!(doc_text(&doc! {"$set": {"owner_id": user.id}}), same(&updates[0]["update"]));
    let (f, u) = svc::announce_update(project.id, now());
    assert_eq!((doc_text(&f), doc_text(&u)), (same(&updates[1]["filter"]), same(&updates[1]["update"])));
    let rest = one(case, "update_many", "mod_claim_requests");
    assert_eq!(doc_text(&svc::pending_claims_of(project.id)), same(&rest["filter"]));
    assert_eq!(
        doc_text(&doc! {"$set": {"status": "rejected", "resolved_at": dt::to_bson(now())}}),
        same(&rest["update"])
    );
    let mut reply = svc::claim_dto(claim);
    reply["handle"] = json!(project.str("owner_handle"));
    reply["slug"] = json!(project.str("slug"));
    assert_body(&reply, case, "approve");
    assert_error(&ApiError::bad_request("This claim is already resolved."), step("claim_approve_again"), "again");
    let saved = check_saves("claim_reject", &|_, m| svc::resolve(m, "rejected", admin_id(), now()));
    assert_body(&svc::claim_dto(&saved[0]), step("claim_reject"), "reject");
    assert_error(&ApiError::not_found("Claim not found"), step("claim_missing"), "missing");
}

#[test]
fn stray_assignment() {
    let case = step("assign");
    let body: Value = serde_json::from_str(s(&case["sent"])).unwrap();
    let user = site_user_of(case, s(&body["user_id"]));
    let finds: Vec<&Value> = ops(case, "find_one", "mod_projects");
    let slugs = ["clash-mod", "clash-mod", "clash-mod-2"];
    for (f, slug) in finds[1..4].iter().zip(slugs) {
        assert_eq!(doc_text(&doc! {"$and": [{"owner_id": user.id}, {"slug": slug}]}), same(&f["filter"]));
    }
    assert_eq!(svc::slugify("Clash Mod"), "clash-mod");
    let saved = check_saves("assign", &|_, m| svc::apply_handover(m, &user, Some("clash-mod-2".into()), now()));
    let rest = one(case, "update_many", "mod_claim_requests");
    assert_eq!(doc_text(&svc::pending_claims_of(saved[0].id)), same(&rest["filter"]));
    assert_eq!(
        doc_text(&doc! {"$set": {"status": "rejected", "resolved_by": admin_id(), "resolved_at": dt::to_bson(now())}}),
        same(&rest["update"])
    );
    let err = "Not a stray mod (already claimed or missing).";
    let reply = json!({
        "assigned": [{"id": saved[0].id.to_hex(), "slug": "clash-mod-2"}],
        "errors": [{"id": s(&body["project_ids"][1]), "error": err}, {"id": "junk", "error": err}],
        "owner_handle": "newowner", "owner_username": "New Owner",
    });
    assert_body(&reply, case, "assign");
    assert_error(&ApiError::not_found("No such user."), step("assign_no_user"), "no user");
}

#[test]
fn remix_claims() {
    let case = step("remix_approve");
    let reads = ops(case, "find_one", "mod_projects");
    let original = load(MOD_PROJECT, &reads[1]["result"]);
    let saved = check_saves("remix_approve", &|coll, m| match coll {
        "mod_projects" if m.id == original.id => {
            m.set("fork_count", crate::auth::pyutil::bson_int(m.int("fork_count") + 1));
        }
        "mod_projects" => svc::link_remix(m, &original, admin_id(), now()),
        _ => svc::resolve(m, "approved", admin_id(), now()),
    });
    assert_eq!(saved.len(), 3);
    assert_body(&svc::remix_claim_dto(&saved[2]), case, "approve");
    assert_error(&ApiError::bad_request("This claim is already resolved."), step("remix_approve_again"), "again");
    let saved = check_saves("remix_reject", &|_, m| svc::resolve(m, "rejected", admin_id(), now()));
    assert_body(&svc::remix_claim_dto(&saved[0]), step("remix_reject"), "reject");
}

#[test]
fn modpack_moderation() {
    for (name, reason) in [("pack_takedown", "  "), ("pack_takedown2", " Broken links ")] {
        let saved = check_saves(name, &|_, m| svc::apply_pack_takedown(m, reason, now()));
        let case = step(name);
        assert_body(&takedown_reply(&saved[0]), case, name);
        let resolve = one(case, "update_many", "content_reports");
        assert_eq!(doc_text(&svc::resolve_reports_filter("modpack", saved[0].id)), same(&resolve["filter"]));
        let reason = saved[0].opt_str("takedown_reason").unwrap_or(svc::DEFAULT_REASON);
        email_matches(case, "modpack", "Kiwi Pack", reason, "/modpacks/owner/pack");
    }
    let saved = check_saves("pack_restore", &|_, m| svc::apply_restore(m, now()));
    let reply = json!({"slug": saved[0].str("slug"), "handle": saved[0].str("owner_handle"), "taken_down": false});
    assert_body(&reply, step("pack_restore"), "restore");
    let case = step("pack_delete");
    assert_eq!(case["status"], 204);
    assert_eq!(case["body"], "");
    let pack = load(MODPACK, &one(case, "find_one", "modpack_projects")["result"]);
    assert_eq!(doc_text(&doc! {"modpack_id": pack.id}), same(&one(case, "delete_many", "modpack_stars")["filter"]));
    assert_eq!(doc_text(&doc! {"_id": pack.id}), same(&one(case, "delete_one", "modpack_projects")["filter"]));
    assert_error(&ApiError::not_found("Modpack not found"), step("pack_missing"), "missing");
}

#[test]
fn project_purge() {
    let case = step("project_delete");
    assert_eq!((case["status"].as_u64(), s(&case["body"])), (Some(204), ""));
    let p = load(MOD_PROJECT, &one(case, "find_one", "mod_projects")["result"]);
    assert!(p.bool("taken_down"));
    let order: Vec<(String, String)> =
        case["ops"].as_array().unwrap().iter().map(|o| (s(&o["op"]).to_owned(), s(&o["coll"]).to_owned())).collect();
    let deletes: Vec<&(String, String)> = order.iter().filter(|(op, _)| op.starts_with("delete")).collect();
    let want = [
        ("delete_many", "mod_releases", doc! {"project_id": p.id}),
        ("delete_many", "content_reports", doc! {"$and": [{"target_type": "mod"}, {"target_id": p.id}]}),
        ("delete_many", "mod_stars", doc! {"project_id": p.id}),
        ("delete_many", "mod_issue_events", doc! {"project_id": p.id}),
        ("delete_many", "mod_issues", doc! {"project_id": p.id}),
        ("delete_one", "mod_projects", doc! {"_id": p.id}),
    ];
    assert_eq!(deletes.len(), want.len());
    for (op, coll, filter) in want {
        assert_eq!(doc_text(&filter), same(&one(case, op, coll)["filter"]), "{op} {coll}");
    }
    assert_eq!(case["repos"], json!([p.id.to_hex()]));
    assert_eq!(svc::repo_dir("data/mods", &p.id), std::path::Path::new("data/mods/git").join(format!("{}.git", p.id)));
    assert_error(&ApiError::not_found("Mod project not found"), step("project_delete_missing"), "missing");
}

#[test]
fn stray_import_routes() {
    let case = step("import_state");
    let inserted = one(case, "insert_one", "stray_import_state");
    let id = ObjectId::parse_str(s(&inserted["doc"]["_id"]["$oid"])).unwrap();
    let st = Model::build_at(STRAY_STATE, doc! {"key": stray::SOURCE}, now(), id).unwrap();
    assert_eq!(doc_text(&st.to_doc()), same(&inserted["doc"]));
    assert_eq!(doc_text(&doc! {"key": stray::SOURCE}), same(&one(case, "find_one", "stray_import_state")["filter"]));
    assert_body(&stray::state_dto(&st), case, "state");
    assert_error(&ApiError::bad_request("An owner needs an author id."), step("import_owner_no_author"), "no author");
    assert_error(&ApiError::not_found("No such user."), step("import_owner_missing"), "no owner");
    assert_body(&json!({"started": true, "resync": false, "author_id": "7"}), step("import_start"), "start");
    assert_body(&json!({"started": true, "resync": true, "author_id": null}), step("import_start_resync"), "resync");
    let q: ImportQuery = query_of(s(&step("import_start")["url"]));
    assert!(q.resync && q.author_id.as_deref() == Some(" 7 "));
    let running = &golden()["scenario"]["already_running"];
    let doc = ext_doc(&running["ops"][0]["result"]);
    let mut st = Model::load_at(STRAY_STATE, &doc, now()).unwrap();
    st.saved();
    assert!(stray::still_running(&st, false, now()) && !stray::still_running(&st, true, now()));
    let mut want = json!({"started": false, "reason": "An import is already running."});
    if let (Value::Object(o), Value::Object(d)) = (&mut want, stray::state_dto(&st)) {
        o.extend(d);
    }
    assert_body(&want, running, "already running");
    let stale = &golden()["scenario"]["stale_run"];
    let st = Model::load_at(STRAY_STATE, &ext_doc(&stale["ops"][0]["result"]), now()).unwrap();
    assert!(!stray::still_running(&st, false, now()));
    assert_body(&json!({"started": true, "resync": false, "author_id": null}), stale, "stale run");
}

// --- emails, slugs, tags ---------------------------------------------------------------------------

#[test]
fn takedown_emails() {
    for c in golden()["emails"].as_array().unwrap() {
        let a: Vec<&Value> = c["args"].as_array().unwrap().iter().collect();
        if a[0].as_str().is_none_or(str::is_empty) {
            assert!(c["email"].is_null());
            continue;
        }
        let (subject, text, html) = svc::takedown_email(s(a[1]), s(a[2]), s(a[3]), s(a[4]));
        let e = &c["email"];
        assert_eq!(subject, s(&e["subject"]));
        assert_eq!(text, s(&e["text"]));
        assert_eq!(html, s(&e["html"]));
    }
}

#[test]
fn slugs_and_tags() {
    for c in golden()["slugs"].as_array().unwrap() {
        assert_eq!(svc::slugify(s(&c["title"])), s(&c["slug"]), "{c}");
    }
    for c in golden()["tags"].as_array().unwrap() {
        let raw: Vec<String> = c["raw"].as_array().unwrap().iter().map(|v| s(v).to_owned()).collect();
        let want: Vec<String> = c["clean"].as_array().unwrap().iter().map(|v| s(v).to_owned()).collect();
        assert_eq!(stray::clean_tags(&raw), want, "{c}");
    }
}

// --- stray import -------------------------------------------------------------------------------------

#[test]
fn stray_value_helpers() {
    let h = &golden()["stray_helpers"];
    for c in h["latest"].as_array().unwrap() {
        match stray::latest_download(&c["in"]) {
            Ok(got) => assert_eq!(got.map(|m| Value::Object(m.clone())).unwrap_or(Value::Null), c["out"], "{c}"),
            Err(_) => assert!(c.get("error").is_some(), "{c}"),
        }
    }
    for c in h["times"].as_array().unwrap() {
        match stray::upload_time(c["in"].as_object().unwrap()) {
            Ok(got) => {
                assert_eq!(got.map(|t| Value::String(dt::isoformat(&t))).unwrap_or(Value::Null), c["out"], "{c}")
            }
            Err(_) => assert!(c["error"].is_string(), "{c}"),
        }
    }
    let at = |ts: i64| stray::upload_time(json!({"date": ts}).as_object().unwrap());
    assert_eq!(at(253_402_300_799).unwrap().map(|t| dt::isoformat(&t)).as_deref(), Some("9999-12-31T23:59:59+00:00"));
    assert!(at(253_402_300_800).is_err());
    for c in h["py_str"].as_array().unwrap() {
        assert_eq!(stray::py_str(&c["in"]), s(&c["out"]), "{c}");
    }
    for c in h["ints"].as_array().unwrap() {
        match stray::py_int(&c["in"]) {
            Ok(n) => assert_eq!(json!(n), c["out"], "{c}"),
            Err(e) => assert_eq!(e, s(&c["error"]), "{c}"),
        }
    }
    for c in h["images"].as_array().unwrap() {
        let data = hex::decode(s(&c["hex"])).unwrap();
        let got = stray::sniff_image(&data).map(|(t, w, h)| json!([t, w, h])).unwrap_or(Value::Null);
        assert_eq!(got, c["out"], "{}", c["name"]);
    }
}

fn stray_case(name: &str) -> &'static Value {
    golden()["stray"]["cases"].as_array().unwrap().iter().find(|c| c["name"] == name).expect(name)
}

fn file_bytes(fileid: &str) -> Vec<u8> {
    hex::decode(s(&golden()["stray"]["files"][fileid])).unwrap()
}

fn oid_of(v: &Value) -> ObjectId {
    ObjectId::parse_str(s(&v["_id"]["$oid"])).unwrap()
}

#[test]
fn stray_new_mods() {
    for name in ["new_tmod", "new_zip_pending", "new_no_title"] {
        let case = stray_case(name);
        let m = case["mod"].as_object().unwrap();
        let resync = case["resync"] == true;
        let prep = stray::prepare(m).unwrap().unwrap();
        let dl = prep.latest.unwrap();
        let fileid = stray::py_str(&dl["fileid"]);
        let parsed =
            stray::parse_release(dl, fileid.clone(), stray::file_format(dl).unwrap(), file_bytes(&fileid)).unwrap();
        let author = stray::resolve_author(Some(&parsed), &prep.ts_author);
        let ins = one(case, "insert_one", "mod_projects");
        let slug = s(&ins["doc"]["slug"]);
        let slug_finds: Vec<&Value> = ops(case, "find_one", "mod_projects");
        assert_eq!(
            doc_text(&doc! {"$and": [{"source": stray::SOURCE}, {"source_id": &prep.sid}]}),
            same(&slug_finds[0]["filter"])
        );
        assert_eq!(svc::slugify(&prep.name), s(&slug_finds[1]["filter"]["$and"][1]["slug"]), "{name}: base slug");
        let banner = ins["doc"]["banner_sha"].as_str();
        if let Some(img) = ops(case, "insert_one", "mod_images").first() {
            let data = match name {
                "new_tmod" => hex::decode(s(&golden()["stray_helpers"]["images"][0]["hex"])).unwrap(),
                other => panic!("{other}: unexpected image"),
            };
            let (ct, w, h) = stray::sniff_image(&data).unwrap();
            let sha = hex::encode(Sha256::digest(&data));
            let fields = doc! {"sha": &sha, "content_type": ct, "byte_size": data.len() as i32, "owner_id": Bson::Null,
            "width": w.map_or(Bson::Null, |v| Bson::Int32(v as i32)), "height": h.map_or(Bson::Null, |v| Bson::Int32(v as i32))};
            let asset = Model::build_at(IMAGE_ASSET, fields, now(), oid_of(&img["doc"])).unwrap();
            assert_eq!(doc_text(&asset.to_doc()), same(&img["doc"]), "{name}: image");
            assert_eq!(banner, Some(sha.as_str()));
        }
        let fields = stray::new_project_fields(&prep, m, slug, &author, resync, banner, &parsed.fileid);
        let mut proj = Model::build_at(MOD_PROJECT, fields, now(), oid_of(&ins["doc"])).unwrap();
        assert_eq!(doc_text(&proj.to_doc()), same(&ins["doc"]), "{name}: project insert");
        let rel = one(case, "insert_one", "mod_releases");
        let sha = hex::encode(Sha256::digest(&parsed.data));
        let filename = stray::release_filename(&parsed, proj.str("title"));
        let published_at = parsed.published_at.unwrap_or_else(now);
        proj.set_dt("last_release_at", published_at);
        let fields = stray::new_release_fields(&proj, &parsed, &sha, &filename, published_at);
        let release = Model::build_at(MOD_RELEASE, fields, now(), oid_of(&rel["doc"])).unwrap();
        assert_eq!(doc_text(&release.to_doc()), same(&rel["doc"]), "{name}: release insert");
        let save = one(case, "save", "mod_projects");
        assert_eq!(doc_text(&proj.to_doc()), same(&save["update"]["$set"]), "{name}: project save");
        assert_eq!(case["outcome"], if resync { "pending" } else { "imported" });
    }
}

#[test]
fn stray_refreshes() {
    let case = stray_case("existing_refresh");
    let m = case["mod"].as_object().unwrap();
    let prep = stray::prepare(m).unwrap().unwrap();
    let rel_save = one(case, "save", "mod_releases");
    let proj_save = one(case, "save", "mod_projects");
    let mut ex = load(MOD_PROJECT, &proj_save["before"]);
    stray::refresh_fields(&mut ex, m, &prep);
    let fid = stray::changed_file_id(&ex, &prep).expect("a newer file");
    let dl = prep.latest.unwrap();
    let parsed = stray::parse_release(dl, fid.clone(), stray::file_format(dl).unwrap(), file_bytes(&fid)).unwrap();
    let mut rel = load(MOD_RELEASE, &rel_save["before"]);
    let published_at = parsed.published_at.unwrap_or_else(now);
    let sha = hex::encode(Sha256::digest(&parsed.data));
    let filename = stray::release_filename(&parsed, ex.str("title"));
    stray::replace_release(&mut rel, &parsed, &sha, &filename, published_at, now());
    assert_eq!(doc_text(&rel.to_doc()), same(&rel_save["update"]["$set"]), "release replace");
    ex.set_dt("last_release_at", published_at);
    stray::adopt_file(&mut ex, fid, &parsed, &prep.ts_author);
    stray::sync_display(&mut ex, &prep.ts_author);
    ex.set_dt("updated_at", now());
    assert_eq!(doc_text(&ex.to_doc()), same(&proj_save["update"]["$set"]), "project refresh");

    let case = stray_case("existing_same_file");
    let m = case["mod"].as_object().unwrap();
    let prep = stray::prepare(m).unwrap().unwrap();
    let save = one(case, "save", "mod_projects");
    let mut ex = load(MOD_PROJECT, &save["before"]);
    stray::refresh_fields(&mut ex, m, &prep);
    assert!(stray::changed_file_id(&ex, &prep).is_none());
    stray::sync_display(&mut ex, &prep.ts_author);
    ex.set_dt("updated_at", now());
    assert_eq!(doc_text(&ex.to_doc()), same(&save["update"]["$set"]), "same file");
}

#[test]
fn stray_outcomes() {
    assert!(stray::prepare(stray_case("no_id")["mod"].as_object().unwrap()).unwrap().is_none());
    for name in ["bad_downloads_count", "bad_name"] {
        assert!(stray::prepare(stray_case(name)["mod"].as_object().unwrap()).is_err(), "{name}");
        assert!(stray_case(name)["outcome"]["raised"].is_string());
    }
    let none = stray::prepare(stray_case("no_downloads")["mod"].as_object().unwrap()).unwrap().unwrap();
    assert!(none.latest.is_none());
    let bad = stray_case("bad_changes");
    let prep = stray::prepare(bad["mod"].as_object().unwrap()).unwrap().unwrap();
    let dl = prep.latest.unwrap();
    let parsed = stray::parse_release(dl, "13".into(), "tmod".into(), file_bytes("13")).unwrap();
    let proj = load(MOD_PROJECT, &one(bad, "insert_one", "mod_projects")["doc"]);
    let fields = stray::new_release_fields(&proj, &parsed, "x", "f", now());
    assert!(Model::build_at(MOD_RELEASE, fields, now(), ObjectId::new()).is_err());
}

#[test]
fn stray_runs() {
    for c in golden()["runs"].as_array().unwrap() {
        let name = s(&c["name"]);
        let body = c["content"].as_str().unwrap().as_bytes();
        let status = c["status"].as_u64().unwrap() as u16;
        let url = stray::CATALOG_URL;
        let outcome: Result<Vec<serde_json::Map<String, Value>>, String> = match crate::feeds::image::status_message(
            status,
            crate::error::status_phrase(status).unwrap(),
            None,
            url,
        ) {
            Some(msg) => Err(msg),
            None => match pyjson::loads(body) {
                Ok(v) => stray::catalog_mods(v, c["author_id"].as_str()),
                Err(e) => Err(stray::json_error(body, e)),
            },
        };
        let want = ext_doc(&c["state"]);
        let mut st =
            Model::build_at(STRAY_STATE, doc! {"key": stray::SOURCE}, now(), want.get_object_id("_id").unwrap())
                .unwrap();
        stray::reset_for_run(&mut st, false, now());
        let result = outcome.map(|mods| {
            st.set("total", mods.len() as i32);
            st.set("processed", mods.len() as i32);
        });
        stray::finish(&mut st, &result, now());
        assert_eq!(doc_text(&st.to_doc()), doc_text(&want), "{name}");
    }
}

// --- parameters, routing ------------------------------------------------------------------------------

fn body_input(text: &str) -> Result<BodyInput, ApiError> {
    if text.is_empty() {
        return Ok(BodyInput::Missing);
    }
    match pyjson::loads(text.as_bytes()) {
        Ok(Value::Null) => Ok(BodyInput::Missing),
        Ok(v) => Ok(BodyInput::Json(v)),
        Err(pyjson::LoadsError::Syntax { msg, pos }) => {
            Err(ValidationError(vec![ErrorItem::json_invalid(pos, msg)]).into())
        }
        Err(e) => panic!("{e:?}"),
    }
}

fn param_error<T: FromParams>(url: &str) -> ApiError {
    let uri: Uri = url.parse().unwrap();
    let headers = HeaderMap::new();
    let mut p = Params::new(Vec::new(), uri.query(), &headers);
    let v = T::from_params(&mut p);
    p.v.finish(v).err().map(Into::into).expect("parameter error")
}

#[test]
fn parameter_errors() {
    for c in golden()["params"].as_array().unwrap() {
        let url = s(&c["url"]);
        let what = format!("{} {url}", s(&c["method"]));
        let err = if let Some(text) = c["sent"].as_str() {
            let b = body_input(text).unwrap();
            if url.ends_with("/assign") {
                validate::<AssignBody>(&b).err().expect("body error")
            } else {
                validate::<TakedownBody>(&b).err().expect("body error")
            }
        } else {
            let path = url.split('?').next().unwrap();
            match path {
                "/admin/mods/reports" => param_error::<ResolvedQuery>(url),
                "/admin/mods/projects" => param_error::<MasterQuery>(url),
                "/admin/mods/stray" => param_error::<StrayQuery>(url),
                "/admin/mods/stray/import" => param_error::<ImportQuery>(url),
                "/admin/modpacks" => param_error::<PackQuery>(url),
                other => panic!("no extractor for {other}"),
            }
        };
        assert_error(&err, c, &what);
    }
}

fn state() -> AppState {
    let settings = Settings::default();
    AppState { inner: std::sync::Arc::new(Inner { settings, db: Db { mongo: None, redis: None, pg: None } }) }
}

fn stack() -> Router {
    let mw = std::sync::Arc::new(Mw::new(
        MwConfig::from_lookup(&|_| None),
        None,
        EmbedSource::Fixed(Vec::new()),
        TrustedHosts::parse("127.0.0.1"),
    ));
    outer(
        wrap(super::router(), mw, None).fallback(|| async { (StatusCode::IM_A_TEAPOT, "proxied") }).with_state(state()),
    )
}

async fn send(method: &str, url: &str, rid: &str, body: Option<&str>) -> (u16, HeaderMap, String) {
    let mut req = Request::builder()
        .method(Method::from_bytes(method.as_bytes()).unwrap())
        .uri(url)
        .header("host", "testserver")
        .header("x-request-id", rid);
    if body.is_some() {
        req = req.header("content-type", "application/json");
    }
    let req = req.body(body.map_or_else(Body::empty, |b| Body::from(b.to_owned()))).unwrap();
    let resp = stack().oneshot(req).await.unwrap();
    let status = resp.status().as_u16();
    let headers = resp.headers().clone();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    (status, headers, String::from_utf8(bytes.to_vec()).unwrap())
}

#[tokio::test]
async fn unauthenticated_routing() {
    for (i, c) in golden()["unauth"].as_array().unwrap().iter().enumerate() {
        let (method, url) = (s(&c["method"]), s(&c["url"]));
        let want: Value = serde_json::from_str(s(&c["body"])).unwrap();
        let rid = s(&want["error"]["request_id"]);
        let (status, headers, body) = send(method, url, rid, c["body_sent"].as_str()).await;
        let what = format!("#{i} {method} {url}");
        assert_eq!(u64::from(status), c["status"].as_u64().unwrap(), "{what}: status");
        for h in ["allow", "www-authenticate"] {
            assert_eq!(headers.get(h).map(|v| v.to_str().unwrap()), c[h].as_str(), "{what}: {h}");
        }
        if method != "HEAD" {
            assert_eq!(body, s(&c["body"]), "{what}: body");
        }
    }
}

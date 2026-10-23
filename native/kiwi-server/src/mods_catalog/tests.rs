//! Checks against `testdata/mods_catalog/golden.json`
//! (`native/port/parity/gen_mods_catalog_fixtures.py`).

use std::sync::{Arc, OnceLock};

use axum::{
    Router,
    body::Body,
    extract::{MatchedPath, Request},
    http::{Method, StatusCode},
    middleware::{Next, from_fn},
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Utc};
use mongodb::bson::{Bson, Document, oid::ObjectId};
use serde_json::{Value, json};
use sha1::{Digest, Sha1};
use tower::ServiceExt;

use super::{
    bodies::{
        ClaimBody, CollaboratorBody, CreateProject, Lookup, RemixClaimBody, RemixStatus, UpdateProfile, UpdateProject,
    },
    dto::{self, Urls},
    git,
    model::{Claim, Collaborator, ImageAsset, Profile, Project, Release, RemixClaim, star_doc},
    service, writes,
};
use crate::{
    auth::models::SiteUser,
    config::Settings,
    db::Db,
    pyjson,
    state::{AppState, Inner},
    validation::{BodyInput, FromBody, validate},
};

fn golden() -> &'static Value {
    static G: OnceLock<Value> = OnceLock::new();
    G.get_or_init(|| serde_json::from_str(include_str!("../../testdata/mods_catalog/golden.json")).expect("fixture"))
}

fn arr<'a>(key: &str) -> &'a Vec<Value> {
    golden()[key].as_array().unwrap_or_else(|| panic!("section {key}"))
}

fn s(v: &Value) -> &str {
    v.as_str().unwrap_or_else(|| panic!("string: {v}"))
}

fn doc(v: &Value) -> Document {
    match Bson::try_from(v.clone()).expect("extended JSON") {
        Bson::Document(d) => d,
        other => panic!("not a document: {other:?}"),
    }
}

/// Field order and BSON types, recursively.
fn same_bson(a: &Bson, b: &Bson, path: &str) {
    match (a, b) {
        (Bson::Document(x), Bson::Document(y)) => {
            let kx: Vec<&String> = x.keys().collect();
            let ky: Vec<&String> = y.keys().collect();
            assert_eq!(kx, ky, "key order at {path}");
            for (k, v) in x {
                same_bson(v, &y[k.as_str()], &format!("{path}.{k}"));
            }
        }
        (Bson::Array(x), Bson::Array(y)) => {
            assert_eq!(x.len(), y.len(), "array length at {path}");
            for (i, (p, q)) in x.iter().zip(y).enumerate() {
                same_bson(p, q, &format!("{path}[{i}]"));
            }
        }
        _ => assert_eq!(a, b, "value at {path}"),
    }
}

fn same_doc(got: &Document, want: &Value, what: &str) {
    same_bson(&Bson::Document(got.clone()), &Bson::Document(doc(want)), what);
}

fn same_json(got: &Value, want: &Value, what: &str) {
    assert_eq!(pyjson::dumps(got), pyjson::dumps(want), "{what}");
}

fn urls() -> Urls {
    let env = &golden()["env"];
    Urls {
        api: s(&env["API_URL"]).trim_end_matches('/').to_owned(),
        app: s(&env["APP_URL"]).trim_end_matches('/').to_owned(),
    }
}

fn now() -> DateTime<Utc> {
    crate::dt::parse_utc(s(&golden()["now"])).expect("now")
}

fn user(v: &Value) -> SiteUser {
    mongodb::bson::from_document(doc(v)).expect("site user")
}

#[test]
fn helpers() {
    for c in arr("slugify") {
        assert_eq!(writes::slugify(s(&c[0])), s(&c[1]), "{c}");
    }
    for c in arr("clean_tags") {
        let input: Vec<String> = serde_json::from_value(c[0].clone()).unwrap();
        let want: Vec<String> = serde_json::from_value(c[1].clone()).unwrap();
        assert_eq!(writes::clean_tags(&input), want, "{c}");
    }
    for c in arr("safe_filename") {
        assert_eq!(dto::safe_filename(s(&c[0])), s(&c[1]), "{c}");
    }
    for c in arr("re_escape") {
        assert_eq!(service::py_re_escape(s(&c[0])), s(&c[1]), "{c}");
    }
    for c in arr("author_ident") {
        assert_eq!(git::author_ident(s(&c[0])), s(&c[1]), "{c}");
    }
    for c in arr("round4") {
        assert_eq!(service::round4(c[0].as_f64().unwrap()), c[1].as_f64().unwrap(), "{c}");
    }
    for c in arr("i18n") {
        let map: Vec<(String, String)> = serde_json::from_value(c["map"].clone()).unwrap();
        let got = writes::clean_i18n(&map, s(&c["base"]), c["max"].as_u64().unwrap() as usize);
        match (got, c.get("ok")) {
            (Ok(m), Some(ok)) => assert_eq!(m, serde_json::from_value::<Vec<(String, String)>>(ok.clone()).unwrap()),
            (Err(e), None) => assert_eq!(e.message, s(&c["error"]), "{c}"),
            (got, _) => panic!("{c}: {:?}", got.map_err(|e| e.message)),
        }
    }
    for c in arr("clean_url") {
        match (writes::clean_url(s(&c["in"]), s(&c["field"])), c.get("ok")) {
            (Ok(v), Some(ok)) => assert_eq!(json!(v), *ok, "{c}"),
            (Err(e), None) => assert_eq!(e.message, s(&c["error"]), "{c}"),
            (got, _) => panic!("{c}: {:?}", got.map_err(|e| e.message)),
        }
    }
    for c in arr("sniff") {
        use base64::Engine;
        let data = base64::engine::general_purpose::STANDARD.decode(s(&c["b64"])).unwrap();
        let got = writes::sniff_image(&data).map(|(ct, w, h)| json!([ct, w, h]));
        assert_eq!(got.unwrap_or(Value::Null), c["out"], "{c}");
    }
    for c in arr("facets") {
        let mut counts: Vec<(String, i64)> = Vec::new();
        for r in c["rows"].as_array().unwrap() {
            let tag = kiwi_core::pystr::strip(s(&r["_id"])).to_owned();
            if tag.is_empty() {
                continue;
            }
            let n = r["n"].as_i64().unwrap();
            match counts.iter_mut().find(|(t, _)| *t == tag) {
                Some(e) => e.1 += n,
                None => counts.push((tag, n)),
            }
        }
        same_json(&service::facets_from_counts(&counts), &c["out"], "facets");
    }
    for c in arr("lookup_modes") {
        let body: Lookup = validate(&BodyInput::Json(c["body"].clone())).map_err(|e| e.message).unwrap();
        assert_eq!(body.mode(), s(&c["mode"]));
    }
}

fn release_list() -> Vec<Release> {
    arr("releases").iter().map(|r| Release::from_doc(&doc(&r["doc"]))).collect()
}

#[test]
fn project_dtos() {
    let u = urls();
    let rels = release_list();
    for c in arr("projects") {
        let p = Project::from_doc(&doc(&c["doc"]));
        let id = p.id.to_hex();
        same_json(&Value::Object(dto::project_card(&p, false)), &c["card"], &format!("card {id}"));
        same_json(&Value::Object(dto::project_card(&p, true)), &c["card_reveal"], &format!("reveal {id}"));
        same_json(&Value::Object(dto::public_mod_dto(&u, &p, None)), &c["public"], &format!("public {id}"));
        same_json(
            &Value::Object(dto::public_mod_dto(&u, &p, Some(&rels[..2]))),
            &c["public_releases"],
            &format!("public+releases {id}"),
        );
        same_json(&Value::Object(dto::remix_card(&p, now())), &c["remix"], &format!("remix {id}"));
        assert_eq!(dto::effective_visibility(&p, now()), s(&c["effective"]), "{id}");
        same_json(&super::announce::event_project(&u, &p), &c["event_project"], &format!("event {id}"));
        same_doc(&p.to_doc(), &c["to_db"], &format!("to_db {id}"));
    }
}

#[test]
fn release_dtos() {
    let u = urls();
    for c in arr("releases") {
        let r = Release::from_doc(&doc(&c["doc"]));
        same_json(&Value::Object(dto::release_dto(&r)), &c["dto"], "release dto");
        same_json(&Value::Object(dto::public_release_dto(&u, &r)), &c["public"], "public release");
        same_json(&super::announce::event_release(&u, &r), &c["event"], "event release");
        same_doc(&r.to_doc(), &c["to_db"], "release to_db");
    }
    let rs = release_list();
    let want = arr("change");
    let got = [
        super::announce::event_change(&rs[1], Some(&rs[0]), 2),
        super::announce::event_change(&rs[0], None, 1),
        super::announce::event_change(&rs[1], Some(&rs[1]), 5),
        super::announce::event_change(&rs[2], Some(&rs[0]), 3),
    ];
    for (g, w) in got.iter().zip(want) {
        same_json(g, w, "change");
    }
}

#[test]
fn profile_dtos() {
    let u = urls();
    for c in arr("profiles") {
        let usr = user(&c["user"]);
        let prof = c["profile"].as_object().map(|_| Profile::from_doc(&doc(&c["profile"])));
        let mods: Vec<Project> = c["mods"].as_array().unwrap().iter().map(|m| Project::from_doc(&doc(m))).collect();
        let got = service::profile_dto(&u, &usr, prof.as_ref(), c["is_owner"].as_bool().unwrap(), &mods);
        same_json(&got, &c["out"], &format!("profile {}", usr.username));
    }
    let p = Profile::from_doc(&doc(&golden()["profile_db"]));
    same_doc(&p.to_doc(), &golden()["profile_db"], "profile to_db");
    for c in arr("avatars") {
        let usr = user(&c["user"]);
        let got = dto::profile_avatar_url(&u, &usr, c["avatar_sha"].as_str(), c["small"].as_bool().unwrap());
        assert_eq!(json!(got), c["out"], "{c}");
    }
}

#[test]
fn claim_dtos() {
    for c in arr("claims") {
        let mut cl = Claim::from_doc(&doc(&c["to_db"]));
        same_json(&Value::Object(dto::claim_dto(&cl)), &c["dto"], "claim");
        same_doc(&cl.to_doc(), &c["to_db"], "claim to_db");
        cl.created_at = now();
        same_json(&Value::Object(dto::claim_dto(&cl)), &c["fresh"], "fresh claim");
    }
    for c in arr("remix_claims") {
        let mut cl = RemixClaim::from_doc(&doc(&c["to_db"]));
        same_json(&Value::Object(dto::remix_claim_dto(&cl)), &c["dto"], "remix claim");
        same_doc(&cl.to_doc(), &c["to_db"], "remix claim to_db");
        cl.created_at = now();
        same_json(&Value::Object(dto::remix_claim_dto(&cl)), &c["fresh"], "fresh remix claim");
    }
}

fn oid(s: &str) -> ObjectId {
    ObjectId::parse_str(s).unwrap()
}

#[test]
fn inserted_documents() {
    let g = &golden()["new_docs"];
    let t = now();
    let mut p = Project::new("my-cool-mod".into(), "My Cool Mod".into());
    p.id = oid("66e000000000000000000001");
    (p.created_at, p.updated_at) = (t, t);
    p.summary = "s".into();
    p.description = "d".into();
    p.tags = vec!["x".into()];
    p.source_visibility = "private".into();
    p.owner_id = Some(oid("65f0000000000000000000a9"));
    p.owner_username = "  Helpful Person  ".into();
    p.owner_handle = "helper".into();
    (p.inspired_by_slug, p.inspired_by_handle, p.inspired_by_title, p.inspired_by_owner) =
        (Some("o".into()), Some("h".into()), Some("T".into()), Some("O".into()));
    same_doc(&p.to_doc(), &g["project"], "new project");
    same_json(&Value::Object(dto::project_card(&p, false)), &g["project_card"], "new project card (µs)");

    let orig = Project::from_doc(&doc(&arr("projects")[0]["doc"]));
    let mut f = Project::new("dragon-pack-2".into(), orig.title.clone());
    f.id = oid("66e000000000000000000002");
    (f.created_at, f.updated_at) = (t, t);
    (f.summary, f.description, f.tags, f.mode) =
        (orig.summary.clone(), orig.description.clone(), orig.tags.clone(), orig.mode.clone());
    f.owner_id = Some(oid("65f0000000000000000000a9"));
    (f.owner_username, f.owner_handle) = ("Helpful Person".into(), "helper".into());
    f.forked_from_id = Some(orig.id);
    f.forked_from_slug = Some(orig.slug.clone());
    f.forked_from_handle = Some(orig.owner_handle.clone());
    f.forked_from_title = Some(orig.title.clone());
    f.forked_from_owner = Some(orig.owner_username.clone());
    f.is_remix = true;
    same_doc(&f.to_doc(), &g["fork"], "fork");

    let img = ImageAsset {
        id: oid("66e000000000000000000003"),
        sha: "a".repeat(64),
        content_type: "image/png".into(),
        byte_size: 123,
        owner_id: oid("65f0000000000000000000a9"),
        width: Some(70000),
        height: None,
        created_at: t,
    };
    same_doc(&img.to_doc(), &g["image"], "image");
    let star = star_doc(oid("66e000000000000000000004"), orig.id, oid("65f0000000000000000000a9"), t);
    same_doc(&star, &g["star"], "star");
    let mut prof = Profile::new(oid("65f0000000000000000000a9"), "helper".into());
    prof.id = oid("66e000000000000000000005");
    (prof.created_at, prof.updated_at) = (t, t);
    same_doc(&prof.to_doc(), &g["profile"], "profile");
    let _ = Collaborator { user_id: orig.id, username: String::new() };
}

fn check_body<T: FromBody>(c: &Value) {
    let got = validate::<T>(&BodyInput::Json(c["body"].clone()));
    match (got, c["errors"].as_array()) {
        (Ok(_), None) => {}
        (Err(e), Some(want)) => {
            let details = e.details.clone().unwrap_or(Value::Null);
            same_json(&details, &Value::Array(want.clone()), &format!("{} {}", c["model"], c["body"]));
        }
        (Ok(_), Some(_)) => panic!("{}: expected errors", c["body"]),
        (Err(e), None) => panic!("{}: unexpected {:?}", c["body"], e.details),
    }
}

#[test]
fn request_bodies() {
    for c in arr("bodies") {
        match s(&c["model"]) {
            "CreateProject" => check_body::<CreateProject>(c),
            "UpdateProject" => check_body::<UpdateProject>(c),
            "UpdateProfile" => check_body::<UpdateProfile>(c),
            "Lookup" => check_body::<Lookup>(c),
            "RemixStatus" => check_body::<RemixStatus>(c),
            "RemixClaim" => check_body::<RemixClaimBody>(c),
            "Collaborator" => check_body::<CollaboratorBody>(c),
            "Claim" => check_body::<ClaimBody>(c),
            other => panic!("model {other}"),
        }
    }
}

#[test]
fn repo_layout_and_fork_commit() {
    let dir = std::env::temp_dir().join(format!("kiwi-mods-repo-{}", ObjectId::new().to_hex())).join("x.git");
    git::init_bare(&dir).unwrap();
    let want = &golden()["repo"];
    let mut dirs = Vec::new();
    let mut files = serde_json::Map::new();
    fn walk(
        base: &std::path::Path,
        d: &std::path::Path,
        dirs: &mut Vec<String>,
        files: &mut serde_json::Map<String, Value>,
    ) {
        for e in std::fs::read_dir(d).unwrap().flatten() {
            let rel = e.path().strip_prefix(base).unwrap().to_string_lossy().replace('\\', "/");
            if e.path().is_dir() {
                dirs.push(rel);
                walk(base, &e.path(), dirs, files);
            } else {
                files.insert(rel, Value::String(std::fs::read_to_string(e.path()).unwrap()));
            }
        }
    }
    walk(&dir, &dir, &mut dirs, &mut files);
    dirs.sort();
    assert_eq!(json!(dirs), want["dirs"]);
    let norm = |v: &Value| v.as_str().unwrap_or("").replace("filemode = true", "filemode = false");
    let want_files = want["files"].as_object().unwrap();
    assert_eq!(files.len(), want_files.len());
    for (k, v) in want_files {
        assert_eq!(norm(&files[k]), norm(v), "{k}");
    }
    assert!(git::branches_in(&dir).is_empty());
    std::fs::write(dir.join("refs/heads/main"), "0f1b13514c2a7a7afab80ede88f48727fdf1f28c\n").unwrap();
    std::fs::create_dir_all(dir.join("refs/heads/feat")).unwrap();
    std::fs::write(dir.join("refs/heads/feat/x"), "ref: refs/heads/main\n").unwrap();
    std::fs::write(dir.join("packed-refs"), "# pack-refs\n1111111111111111111111111111111111111111 refs/heads/alpha\n^2222222222222222222222222222222222222222\n").unwrap();
    let got = git::branches_in(&dir);
    let names: Vec<&str> = got.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, ["alpha", "feat/x", "main"]);
    assert_eq!(got[1].1, "0f1b13514c2a7a7afab80ede88f48727fdf1f28c");
    let _ = std::fs::remove_dir_all(dir.parent().unwrap());

    let fc = &golden()["fork_commit"];
    let raw = git::fork_commit(
        "4b825dc642cb6eb9a060e54bf8d69288fbee4904",
        &git::author_ident("Helpful Person"),
        1792000000,
        "Fork of dragon-pack",
    );
    assert_eq!(String::from_utf8(raw.clone()).unwrap(), s(&fc["raw"]));
    let mut h = Sha1::new();
    h.update(format!("commit {}\0", raw.len()).as_bytes());
    h.update(&raw);
    assert_eq!(hex::encode(h.finalize()), s(&fc["sha"]));
}

fn git_cli(dir: &std::path::Path, args: &[&str], stdin: &[u8], index: Option<&std::path::Path>) -> String {
    use std::io::Write;
    let mut cmd = std::process::Command::new("git");
    cmd.arg("--git-dir").arg(dir).args(args);
    for (k, v) in [
        ("GIT_AUTHOR_NAME", "t"),
        ("GIT_AUTHOR_EMAIL", "t@t"),
        ("GIT_COMMITTER_NAME", "t"),
        ("GIT_COMMITTER_EMAIL", "t@t"),
    ] {
        cmd.env(k, v);
    }
    if let Some(i) = index {
        cmd.env("GIT_INDEX_FILE", i);
    }
    let mut child =
        cmd.stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).spawn().expect("spawn git");
    child.stdin.take().unwrap().write_all(stdin).unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "git {args:?}");
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

/// `gitstore._fork` (dulwich) and `git::fork_dirs` agree on the commit id.
#[tokio::test]
async fn fork_matches_dulwich() {
    use base64::Engine;
    if std::process::Command::new("git").arg("--version").output().is_err() {
        return;
    }
    let want = &golden()["fork_repo"];
    let base = std::env::temp_dir().join(format!("kiwi-mods-fork-{}", ObjectId::new().to_hex()));
    let (src, dst) = (base.join("src.git"), base.join("dst.git"));
    git::init_bare(&src).unwrap();
    git::init_bare(&dst).unwrap();
    let mut info: Vec<u8> = Vec::new();
    for f in want["files"].as_array().unwrap() {
        let data = base64::engine::general_purpose::STANDARD.decode(s(&f[2])).unwrap();
        let sha = git_cli(&src, &["hash-object", "-w", "--stdin"], &data, None);
        info.extend_from_slice(format!("{:o} {sha}\t{}\0", f[1].as_u64().unwrap(), s(&f[0])).as_bytes());
    }
    let index = base.join("index");
    git_cli(&src, &["update-index", "-z", "--add", "--index-info"], &info, Some(&index));
    let tree = git_cli(&src, &["write-tree"], b"", Some(&index));
    let commit = git_cli(&src, &["commit-tree", &tree, "-m", "init"], b"", None);
    std::fs::write(src.join("refs/heads/main"), format!("{commit}\n")).unwrap();
    let c = git::ForkCommit {
        author: git::author_ident("Helpful Person"),
        message: "Fork of dragon-pack".into(),
        ts: 1792000000,
    };
    let got = git::fork_dirs(&src, &dst, "main", "main", &c).await.unwrap();
    assert_eq!(got.as_deref(), Some(s(&want["sha"])));
    assert_eq!(std::fs::read_to_string(dst.join("refs/heads/main")).unwrap(), s(&want["ref"]));
    assert_eq!(git::branches_in(&dst), [("main".to_owned(), s(&want["sha"]).to_owned())]);
    assert!(git::fork_dirs(&src, &dst, "nope", "x", &c).await.unwrap().is_none());
    let _ = std::fs::remove_dir_all(&base);
}

fn test_state() -> AppState {
    let vars: Vec<(String, String)> =
        golden()["env"].as_object().unwrap().iter().map(|(k, v)| (k.clone(), s(v).to_owned())).collect();
    let settings = Settings::from_vars(vars).expect("settings");
    AppState { inner: Arc::new(Inner { settings, db: Db { mongo: None, redis: None, pg: None } }) }
}

async fn matched(req: Request, _next: Next) -> Response {
    let m = req.extensions().get::<MatchedPath>().map(|m| m.as_str().to_owned()).unwrap_or_default();
    m.into_response()
}

async fn route_of(app: &Router, method: Method, path: &str) -> (u16, String) {
    let req = Request::builder().method(method).uri(path).body(Body::empty()).unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status().as_u16();
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    (status, String::from_utf8_lossy(&body).into_owned())
}

/// Starlette tries routes in order, so `/v1/mods/hub/<x>` that is no hub
/// route falls through to `/v1/mods/{handle}/{slug}`; matchit must agree.
#[tokio::test]
async fn route_precedence() {
    let app: Router = super::router()
        .merge(crate::tools::router())
        .layer(from_fn(matched))
        .fallback(|| async { StatusCode::IM_A_TEAPOT })
        .with_state(test_state());
    let cases = [
        ("/v1/mods/hub/projects", "/v1/mods/hub/projects"),
        ("/v1/mods/hub/tags", "/v1/mods/hub/tags"),
        ("/v1/mods/hub/xyz", "/v1/mods/{handle}/{slug}"),
        ("/v1/mods/hub/me", "/v1/mods/{handle}/{slug}"),
        ("/v1/mods/hub/profile", "/v1/mods/{handle}/{slug}"),
        ("/v1/mods/hub/creator-links", "/v1/mods/{handle}/{slug}"),
        ("/v1/mods/popular", "/v1/mods/popular"),
        ("/v1/mods/a/b", "/v1/mods/{handle}/{slug}"),
        ("/v1/mods/hub/me/projects", "/v1/mods/hub/me/projects"),
        ("/v1/mods/hub/me/projects/a/b", "/v1/mods/hub/me/projects/{handle}/{slug}"),
        ("/v1/mods/hub/projects/me/projects", "/v1/mods/hub/projects/{handle}/{slug}"),
        ("/v1/mods/hub/projects/a/b/branches", "/v1/mods/hub/projects/{handle}/{slug}/{x}"),
        ("/v1/mods/hub/me/projects/a/b/commits", "/v1/mods/hub/projects/{handle}/{slug}/{x}"),
        ("/v1/mods/popular/x", "/v1/mods/{handle}/{slug}"),
        ("/v1/mods/lookup/x", "/v1/mods/{handle}/{slug}"),
        ("/v1/mods/categories/x", "/v1/mods/{handle}/{slug}"),
        ("/v1/mods/read/x", "/v1/mods/{handle}/{slug}"),
        ("/v1/mods/hub/image", "/v1/mods/{handle}/{slug}"),
        ("/v1/mods/hub/releases", "/v1/mods/{handle}/{slug}"),
        ("/v1/mods/hub/projects/a/b/star", "/v1/mods/hub/projects/{handle}/{slug}/star"),
        ("/v1/mods/hub/profile/someone", "/v1/mods/hub/profile/{handle}"),
        ("/site/mods/me/projects", "/site/mods/me/projects"),
        ("/site/mods/projects/a/b", "/site/mods/projects/{handle}/{slug}"),
    ];
    for (path, want) in cases {
        let (status, got) = route_of(&app, Method::GET, path).await;
        if want.ends_with("{x}") {
            assert_eq!(status, 418, "{path}: {got}");
        } else {
            assert_eq!((status, got.as_str()), (200, want), "{path}");
        }
    }
    for path in ["/v1/mods/hub/projects/x", "/v1/mods/a", "/v1/mods/hub/creator-links/x", "/site/mods/creator-token"] {
        assert_eq!(route_of(&app, Method::GET, path).await.0, 418, "{path}");
    }
}

#[test]
fn write_allowlist_matches_python() {
    let routes: Vec<&str> = super::write_auth::API_ROUTES.iter().map(|(r, _)| *r).collect();
    assert_eq!(routes.len(), 19);
    assert!(routes.contains(&"/v1/mods/hub/projects/{handle}/{slug}"));
    assert!(!routes.contains(&"/v1/mods/hub/me/profile"));
}

#[test]
fn popularity_scores() {
    let raw = service::raw_scores(&[(0, 0, 0), (3, 1, 10), (100, 0, 0)]).unwrap();
    assert_eq!(raw[0], 0.0);
    let want = 4.0 * 3f64.ln_1p() + 2.0 * 1f64.ln_1p() + 10f64.ln_1p();
    assert_eq!(raw[1], want);
    assert!(service::raw_scores(&[(0, -1, 0)]).is_none());
    assert_eq!(writes::thousands(40_000), "40,000");
    assert_eq!(writes::thousands(120), "120");
    assert_eq!(writes::thousands(1_000_000), "1,000,000");
}

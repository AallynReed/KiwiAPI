//! Golden checks against `testdata/mods_git/golden.json`
//! (`native/port/parity/gen_mods_git_fixtures.py`, dulwich 1.2.17).

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::LazyLock,
};

use axum::{
    body::Body,
    http::{HeaderMap, HeaderValue, header},
    response::IntoResponse,
};
use base64::{Engine, engine::general_purpose::STANDARD as B64};
use mongodb::bson::{Bson, Document, doc, oid::ObjectId};
use serde_json::{Value, json};

use super::{archive, odb, project::Project, refs, service, smart, store, store::StoreError, tokens::GitToken};
use crate::{auth::models::SiteUser, error::ApiError, validation::ValidationError};

static GOLDEN: LazyLock<Value> =
    LazyLock::new(|| serde_json::from_str(include_str!("../../testdata/mods_git/golden.json")).unwrap());

const OWNER: &str = "65b000000000000000000001";
const OTHER: &str = "65b000000000000000000002";

fn s(v: &Value) -> &str {
    v.as_str().unwrap_or_else(|| panic!("not a string: {v}"))
}

fn scratch(name: &str) -> PathBuf {
    let d = Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/mods_git_tests").join(name);
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

fn repo(root: &Path, pid: &str) -> PathBuf {
    store::repo_dir(root.to_str().unwrap(), pid)
}

fn now_ts() -> i64 {
    crate::dt::parse_utc(s(&GOLDEN["now"])).unwrap().timestamp()
}

fn user(id: &str, display: Option<&str>, username: &str) -> SiteUser {
    let mut d = doc! {"_id": ObjectId::parse_str(id).unwrap(), "username": username};
    if let Some(x) = display {
        d.insert("display_name", x);
    }
    mongodb::bson::from_document(d).unwrap()
}

fn project(mode: &str, collab: Option<&str>) -> Project {
    let mut d = doc! {
        "_id": ObjectId::new(), "slug": "my mod: <v2>", "title": "T", "owner_id": ObjectId::parse_str(OWNER).unwrap(),
        "mode": mode, "visibility": "public",
    };
    if let Some(c) = collab {
        d.insert(
            "collaborators",
            vec![Bson::Document(doc! {"user_id": ObjectId::parse_str(c).unwrap(), "username": "bob"})],
        );
    }
    Project::from_doc(&d).unwrap()
}

fn outcome<T>(r: Result<T, ApiError>, ok: impl FnOnce(T) -> Value) -> Value {
    match r {
        Ok(v) => json!({"ok": ok(v)}),
        Err(e) if e.status.as_u16() == 500 && e.message == "Internal server error" => json!({"exception": true}),
        Err(e) => json!({"error": [e.status.as_u16(), e.code, e.message]}),
    }
}

/// A Python `{"exception": ...}` matches any unhandled 500 here.
fn same(got: &Value, want: &Value, what: &str) {
    if want.get("exception").is_some() {
        assert!(got.get("exception").is_some(), "{what}: want an unhandled error, got {got}");
    } else {
        assert_eq!(got, want, "{what}");
    }
}

fn decode_adds(v: &Value) -> Vec<(String, Vec<u8>)> {
    v.as_array().unwrap().iter().map(|a| (s(&a[0]).to_owned(), B64.decode(s(&a[1])).unwrap())).collect()
}

fn strs(v: &Value) -> Vec<String> {
    v.as_array().unwrap().iter().map(|x| s(x).to_owned()).collect()
}

fn replay_ops(root: &Path) -> Vec<Value> {
    let mut results: Vec<Value> = Vec::new();
    for op in GOLDEN["ops"].as_array().unwrap() {
        let kind = s(&op[0]);
        let r = match kind {
            "ensure" => {
                odb::Repo::ensure(&repo(root, s(&op[1]))).unwrap();
                json!({"ok": null})
            }
            "commit" => {
                let author = store::author_ident(s(&op[5]));
                let ts = op[7].as_i64().unwrap();
                match store::write_commit(
                    &repo(root, s(&op[1])),
                    s(&op[2]),
                    &decode_adds(&op[3]),
                    &strs(&op[4]),
                    &author,
                    s(&op[6]),
                    ts,
                ) {
                    Ok(sha) => json!({"ok": sha}),
                    Err(StoreError::NothingToCommit) => json!({"nothing": true}),
                    Err(e) => panic!("{op}: {e:?}"),
                }
            }
            "branch" => {
                let mut start = s(&op[3]).to_owned();
                if let Some(rest) = start.strip_prefix('@') {
                    let upper = rest.ends_with('U');
                    let idx: usize = rest.trim_end_matches('U').parse().unwrap();
                    start = s(&results[idx]["ok"]).to_owned();
                    if upper {
                        start = start.to_uppercase();
                    }
                }
                match store::create_branch(&repo(root, s(&op[1])), s(&op[2]), &start) {
                    Ok(sha) => json!({"ok": sha}),
                    Err(StoreError::Refused(m)) => json!({"refused": m}),
                    Err(e) => panic!("{op}: {e:?}"),
                }
            }
            "delete_branch" => {
                store::delete_branch(&repo(root, s(&op[1])), s(&op[2])).unwrap();
                json!({"ok": null})
            }
            "fork" => {
                let author = store::author_ident(s(&op[5]));
                let r = store::fork(
                    &repo(root, s(&op[1])),
                    &repo(root, s(&op[2])),
                    s(&op[3]),
                    s(&op[4]),
                    &author,
                    s(&op[6]),
                    op[7].as_i64().unwrap(),
                )
                .unwrap();
                json!({"ok": r})
            }
            other => panic!("op {other}"),
        };
        results.push(r);
    }
    results
}

fn game(extra: &[(&str, &str)]) -> service::GameMap {
    let mut g: service::GameMap = [
        ("a.txt", "ui/hud/a.txt"),
        ("t.dds", "textures/t.dds"),
        ("readme_root.txt", "ui/readme_root.txt"),
        ("x.blueprint", "blueprints/deco/x.blueprint"),
        ("m.txt", "Models/Deep/M.TXT"),
        ("dup.txt", ""),
    ]
    .into_iter()
    .map(|(a, b)| (a.to_owned(), b.to_owned()))
    .collect();
    for (a, b) in extra {
        g.insert((*a).to_owned(), (*b).to_owned());
    }
    g
}

fn commit_case(
    dir: &Path,
    p: &Project,
    actor: &SiteUser,
    branch: &str,
    msg: &str,
    adds: Vec<(&str, Vec<u8>)>,
    deletes: &[&str],
) -> Value {
    let adds = adds.into_iter().map(|(a, b)| (a.to_owned(), b)).collect();
    let deletes: Vec<String> = deletes.iter().map(|d| (*d).to_owned()).collect();
    let r = service::check_commit(p, actor, branch, msg, adds, &deletes, 4, 4096)
        .and_then(|req| service::commit_core(dir, &req, &store::author_ident(service::actor_name(actor)), now_ts()));
    outcome(r, |v| v)
}

fn replay_writes(root: &Path) -> Vec<(String, Value)> {
    let p5d = repo(root, "p5");
    let p5 = project("files", None);
    let me = user(OWNER, Some("Alice Example"), "alice");
    let bob = user(OTHER, None, "bob");
    let mut out: Vec<(String, Value)> = Vec::new();
    let mut push = |n: &str, v: Value| out.push((n.to_owned(), v));
    push(
        "commit_ok",
        commit_case(
            &p5d,
            &p5,
            &me,
            "main",
            "  add files ",
            vec![("UI\\HUD\\A.txt", b"a".to_vec()), ("textures/T.dds", vec![b't'; 10])],
            &[],
        ),
    );
    push("commit_same", commit_case(&p5d, &p5, &me, "main", "again", vec![("ui/hud/a.txt", b"a".to_vec())], &[]));
    push("commit_blank_message", commit_case(&p5d, &p5, &me, "main", " \n ", vec![], &[]));
    push(
        "commit_too_many",
        commit_case(
            &p5d,
            &p5,
            &me,
            "main",
            "m",
            (0..5).map(|i| (["ui/0.txt", "ui/1.txt", "ui/2.txt", "ui/3.txt", "ui/4.txt"][i], b"x".to_vec())).collect(),
            &[],
        ),
    );
    push("commit_bad_branch", commit_case(&p5d, &p5, &me, "a..b", "m", vec![], &[]));
    push(
        "commit_big_file",
        commit_case(
            &p5d,
            &p5,
            &me,
            "main",
            "m",
            vec![("ui/ok.txt", b"x".to_vec()), ("Ui/Big 'one'.bin", vec![b'x'; 4097])],
            &[],
        ),
    );
    push("commit_bad_path", commit_case(&p5d, &p5, &me, "main", "m", vec![("ui/./x", b"x".to_vec())], &[]));
    push("commit_tmod", commit_case(&p5d, &p5, &me, "main", "m", vec![("ui/Built.TMOD ", b"x".to_vec())], &[]));
    push("commit_bad_delete", commit_case(&p5d, &p5, &me, "main", "m", vec![], &["ui/"]));
    push("commit_releases_mode", commit_case(&p5d, &project("releases", None), &me, "main", "m", vec![], &[]));
    push("commit_not_owner", commit_case(&p5d, &p5, &bob, "main", "m", vec![], &[]));
    let bob2 = user(OTHER, Some(""), "bob");
    push(
        "commit_collaborator",
        commit_case(
            &p5d,
            &project("files", Some(OTHER)),
            &bob2,
            "side/one",
            "bob was here",
            vec![("ui/hud/a.txt", b"a2".to_vec()), ("particles/p.pkfx", b"p".to_vec())],
            &["textures/t.dds"],
        ),
    );
    let br = |name: &str, from: Option<&str>, dir: &Path| {
        outcome(service::create_branch_core(dir, &p5, &me, name, from), |v| v)
    };
    push("branch_ok", br(" feat-1 ", None, &p5d));
    push("branch_from_sha_bad", br("feat-2", Some("nope"), &p5d));
    push("branch_exists", br("feat-1", Some("main"), &p5d));
    push("branch_invalid", br("/lead", None, &p5d));
    push("branch_dotlock_500", br("x.lock", None, &p5d));
    push("branch_empty_repo", br("feat", None, &repo(root, "p6")));
    push("branch_from_side", br("feat-3", Some("side/one"), &p5d));
    push("delete_default", outcome(service::delete_branch_core(&p5d, &p5, &me, "main"), |_| Value::Null));
    push("delete_ok", outcome(service::delete_branch_core(&p5d, &p5, &me, "feat-1"), |_| Value::Null));
    let fix = |branch: &str, g: &service::GameMap| {
        let r = service::fix_prepare(&p5d, &p5, &me, branch).and_then(|(b, sha, paths)| {
            service::fix_core(&p5d, &b, &sha, &paths, g, &store::author_ident(service::actor_name(&me)), now_ts())
        });
        outcome(r, |v| v)
    };
    let g = game(&[("p.pkfx", "particles/fx/p.pkfx")]);
    push("fix_ok", fix("", &g));
    push("fix_nothing", fix("main", &g));
    push("fix_no_branch", fix("ghost", &g));
    push("fix_bad_branch", fix("..", &g));
    push("fix_side", fix("side/one", &g));
    push("fix_no_game", fix("side/one", &service::GameMap::new()));
    out
}

fn dump(root: &Path) -> Value {
    let mut out = serde_json::Map::new();
    let mut repos: Vec<PathBuf> = fs::read_dir(root.join("git")).unwrap().map(|e| e.unwrap().path()).collect();
    repos.sort();
    for r in repos {
        let mut files = serde_json::Map::new();
        let mut dirs = Vec::new();
        let mut stack = vec![r.clone()];
        let mut all = Vec::new();
        while let Some(d) = stack.pop() {
            for e in fs::read_dir(&d).unwrap() {
                let p = e.unwrap().path();
                if p.is_dir() {
                    stack.push(p.clone());
                }
                all.push(p);
            }
        }
        all.sort();
        for p in all {
            let rel = p.strip_prefix(&r).unwrap().to_string_lossy().replace('\\', "/");
            if p.is_dir() {
                dirs.push(Value::String(rel));
            } else {
                files.insert(rel, Value::String(B64.encode(fs::read(&p).unwrap())));
            }
        }
        out.insert(r.file_name().unwrap().to_string_lossy().into_owned(), json!({"files": files, "dirs": dirs}));
    }
    Value::Object(out)
}

/// The golden repositories, with the `filemode` this platform probes.
fn expected_store() -> Value {
    let mut want = GOLDEN["store"].clone();
    if cfg!(unix) {
        for (_, r) in want.as_object_mut().unwrap() {
            let cfg = r["files"]["config"].clone();
            let text =
                String::from_utf8(B64.decode(s(&cfg)).unwrap()).unwrap().replace("filemode = false", "filemode = true");
            r["files"]["config"] = Value::String(B64.encode(text));
        }
    }
    want
}

fn sorted_dirs(v: &Value) -> Vec<String> {
    let mut d: Vec<String> = v.as_array().unwrap().iter().map(|x| s(x).to_owned()).collect();
    d.sort();
    d
}

#[test]
fn store_writes_what_dulwich_writes() {
    let root = scratch("store");
    let results = replay_ops(&root);
    assert_eq!(Value::Array(results), GOLDEN["results"]);
    let writes = replay_writes(&root);
    for (i, (name, got)) in writes.iter().enumerate() {
        let want = &GOLDEN["writes"][i];
        assert_eq!(s(&want[0]), name);
        same(got, &want[1], name);
    }
    let got = dump(&root);
    let want = expected_store();
    let got_repos: Vec<&String> = got.as_object().unwrap().keys().collect();
    let want_repos: Vec<&String> = want.as_object().unwrap().keys().collect();
    assert_eq!(got_repos, want_repos);
    for (name, w) in want.as_object().unwrap() {
        let g = &got[name];
        assert_eq!(sorted_dirs(&g["dirs"]), sorted_dirs(&w["dirs"]), "{name} dirs");
        let gf: BTreeMap<&String, &Value> = g["files"].as_object().unwrap().iter().collect();
        let wf: BTreeMap<&String, &Value> = w["files"].as_object().unwrap().iter().collect();
        assert_eq!(gf.keys().collect::<Vec<_>>(), wf.keys().collect::<Vec<_>>(), "{name} files");
        for (k, v) in wf {
            assert_eq!(gf[k], v, "{name}/{k}");
        }
    }
}

fn read_store(name: &str) -> PathBuf {
    let root = scratch(name);
    replay_ops(&root);
    root
}

#[test]
fn reads_match_python() {
    let root = read_store("reads");
    let r = &GOLDEN["reads"];
    for (pid, want) in r["branches"].as_object().unwrap() {
        same(&outcome(service::list_branches_core(&repo(&root, pid), "main"), |v| v), want, pid);
    }
    for c in r["trees"].as_array().unwrap() {
        let got = outcome(service::get_tree_core(&repo(&root, s(&c[0])), "main", s(&c[1])), |v| v);
        same(&got, &c[2], &format!("tree {} {}", c[0], c[1]));
    }
    for c in r["commits"].as_array().unwrap() {
        let got = service::list_commits_core(
            &repo(&root, s(&c[0])),
            "main",
            c[1].as_str(),
            c[2].as_u64().unwrap() as usize,
            c[3].as_u64().unwrap() as usize,
        );
        let got = outcome(got, |v| json!([v["items"], v["total"]]));
        same(&got, &c[4], &format!("commits {c}"));
        if let Ok(v) = service::list_commits_core(
            &repo(&root, s(&c[0])),
            "main",
            c[1].as_str(),
            c[2].as_u64().unwrap() as usize,
            c[3].as_u64().unwrap() as usize,
        ) {
            assert_eq!(v["count"], v["items"].as_array().unwrap().len());
        }
    }
    for c in r["blobs"].as_array().unwrap() {
        let got = match service::file_bytes_core(&repo(&root, s(&c[0])), s(&c[1]), s(&c[2])) {
            Ok(b) => json!({"ok_bytes": B64.encode(b)}),
            Err(e) => outcome::<()>(Err(e), |_| Value::Null),
        };
        same(&got, &c[3], &format!("blob {c}"));
    }
    for c in r["compare"].as_array().unwrap() {
        let got = outcome(service::compare_core(&repo(&root, "p1"), s(&c[0]), s(&c[1])), |v| v);
        same(&got, &c[2], &format!("compare {} {}", c[0], c[1]));
    }
    let g = game(&[]);
    for c in r["placement"].as_array().unwrap() {
        let got = outcome(service::placement_core(&repo(&root, s(&c[0])), "main", s(&c[1]), &g), |v| v);
        same(&got, &c[2], &format!("placement {} {}", c[0], c[1]));
    }
    let got = outcome(service::placement_core(&repo(&root, "p1"), "main", "", &service::GameMap::new()), |v| v);
    same(&got, &r["placement_no_game"], "placement without game");
    let clock: Vec<u16> = r["archive_clock"].as_array().unwrap().iter().map(|x| x.as_u64().unwrap() as u16).collect();
    let clock: [u16; 6] = clock.try_into().unwrap();
    let p = project("files", None);
    for c in r["archives"].as_array().unwrap() {
        let got = outcome(service::archive_core(&repo(&root, s(&c[0])), &p, s(&c[1]), Some(clock)), |(n, b)| {
            json!([n, B64.encode(b)])
        });
        same(&got, &c[2], &format!("archive {} {}", c[0], c[1]));
    }
}

#[test]
fn packed_objects_read_like_loose_ones() {
    if std::process::Command::new("git").arg("--version").output().is_err() {
        eprintln!("git not installed: skipped");
        return;
    }
    let root = read_store("packed");
    let dir = repo(&root, "p1");
    let before: Vec<Value> = ["main", "dev", "fromsha"]
        .iter()
        .map(|r| service::list_commits_core(&dir, "main", Some(r), 50, 0).unwrap())
        .chain(["main", "dev"].iter().map(|r| service::get_tree_core(&dir, "main", r).unwrap()))
        .collect();
    let st = std::process::Command::new("git")
        .args(["-c", "safe.directory=*", "repack", "-a", "-d", "-k", "-f", "--depth=10", "--window=50"])
        .current_dir(&dir)
        .status()
        .unwrap();
    assert!(st.success());
    let loose: usize = fs::read_dir(dir.join("objects"))
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().len() == 2)
        .map(|e| fs::read_dir(e.path()).unwrap().count())
        .sum();
    assert_eq!(loose, 0, "repack left loose objects");
    let after: Vec<Value> = ["main", "dev", "fromsha"]
        .iter()
        .map(|r| service::list_commits_core(&dir, "main", Some(r), 50, 0).unwrap())
        .chain(["main", "dev"].iter().map(|r| service::get_tree_core(&dir, "main", r).unwrap()))
        .collect();
    assert_eq!(before, after);
    let blob = service::file_bytes_core(&dir, "main", "textures/t.dds").unwrap();
    assert_eq!(blob, (0..=255u8).collect::<Vec<_>>().repeat(20));
}

#[test]
fn delta_application() {
    let base = b"hello world, hello git";
    let delta = [22u8, 11, 0x91, 6, 5, 6, b'!', b' ', b'b', b'y', b'e', b'!'];
    assert_eq!(odb::apply_delta(base, &delta).unwrap(), b"world! bye!");
    assert!(odb::apply_delta(b"short", &delta).is_none());
}

fn api_body(e: ApiError) -> Value {
    let mut v: Value = serde_json::from_str(&e.body()).unwrap();
    v["error"]["request_id"] = Value::Null;
    v
}

fn golden_body(resp: &str) -> Value {
    let mut v: Value = serde_json::from_str(resp).unwrap();
    if v.get("error").is_some() {
        v["error"]["request_id"] = Value::Null;
    }
    v
}

#[test]
fn commit_form_matches_fastapi() {
    let root = scratch("forms");
    for c in GOLDEN["http"].as_array().unwrap() {
        let name = s(&c["name"]);
        if !name.starts_with("form_") {
            continue;
        }
        let body = B64.decode(s(&c["body"])).unwrap();
        let ct = c["content_type"].as_str().map(str::as_bytes);
        let want = golden_body(s(&c["resp"]));
        let status = c["status"].as_u64().unwrap();
        let items = match kiwi_core::form::parse_form(ct, &body) {
            Ok(items) => items,
            Err(kiwi_core::form::FormError::Http(msg)) => {
                assert_eq!(status, 400, "{name}");
                let e = ApiError::http(axum::http::StatusCode::BAD_REQUEST, Some(&msg));
                assert_eq!(api_body(e), want, "{name}");
                continue;
            }
        };
        match super::commit_form(&items) {
            Err(errs) => {
                assert_eq!(status, 422, "{name}");
                assert_eq!(api_body(ApiError::from(ValidationError(errs))), want, "{name}");
            }
            Ok(f) => {
                assert_eq!(status, 201, "{name}");
                let me = user(OWNER, Some("Alice Example"), "alice");
                let p = project("files", None);
                let req =
                    service::check_commit(&p, &me, &f.branch, &f.message, super::commit_adds(&f), &f.deletes, 4, 4096)
                        .unwrap();
                let got =
                    service::commit_core(&repo(&root, "p7"), &req, &store::author_ident("Alice Example"), now_ts())
                        .unwrap();
                assert_eq!(got, want, "{name}");
            }
        }
    }
}

#[test]
fn git_route_errors() {
    for c in GOLDEN["http"].as_array().unwrap() {
        let name = s(&c["name"]);
        let want = golden_body(s(&c["resp"]));
        let resp = match name {
            "refs_bad_service" => ApiError::forbidden("Unsupported service").into_response(),
            "refs_unknown_anon" => super::authorize_decision(None, None, false, false).unwrap_err(),
            "push_anon" => {
                let p = project("files", None);
                super::authorize_decision(Some(&p), None, true, true).unwrap_err()
            }
            _ => continue,
        };
        assert_eq!(resp.status().as_u16() as u64, c["status"].as_u64().unwrap(), "{name}");
        let challenge =
            c["headers"].as_array().unwrap().iter().find(|h| h[0] == "www-authenticate").map(|h| s(&h[1]).to_owned());
        assert_eq!(
            resp.headers().get(header::WWW_AUTHENTICATE).map(|v| v.to_str().unwrap().to_owned()),
            challenge,
            "{name}"
        );
        let body = futures::executor::block_on(axum::body::to_bytes(resp.into_body(), usize::MAX)).unwrap();
        assert_eq!(golden_body(std::str::from_utf8(&body).unwrap()), want, "{name}");
    }
}

#[test]
fn basic_auth_password() {
    for c in GOLDEN["basic"].as_array().unwrap() {
        let mut h = HeaderMap::new();
        let v = s(&c[0]);
        if !v.is_empty() {
            let bytes: Vec<u8> = v.chars().map(|ch| ch as u32 as u8).collect();
            h.insert(header::AUTHORIZATION, HeaderValue::from_bytes(&bytes).unwrap());
        }
        assert_eq!(super::basic_token(&h).map(Value::String).unwrap_or(Value::Null), c[1], "{v:?}");
    }
}

#[test]
fn authorize_matches_python() {
    let owner = user(OWNER, Some("Alice Example"), "alice");
    let other = user(OTHER, None, "bob");
    let mk = |extra: Document| {
        let mut d = doc! {"_id": ObjectId::new(), "slug": "s", "title": "T", "owner_id": ObjectId::parse_str(OWNER).unwrap(),
        "visibility": "public", "mode": "files", "source_visibility": "public"};
        d.extend(extra);
        Project::from_doc(&d).unwrap()
    };
    let collab = vec![Bson::Document(doc! {"user_id": ObjectId::parse_str(OTHER).unwrap(), "username": "bob"})];
    for c in GOLDEN["authorize"].as_array().unwrap() {
        let p = match s(&c[0]) {
            "none" => None,
            "public" => Some(mk(doc! {})),
            "private_src" => Some(mk(doc! {"source_visibility": "private"})),
            "releases" => Some(mk(doc! {"mode": "releases"})),
            "draft" => Some(mk(doc! {"visibility": "draft"})),
            "collab_draft" => Some(mk(doc! {"visibility": "draft", "collaborators": collab.clone()})),
            x => panic!("{x}"),
        };
        let u = match s(&c[1]) {
            "anon" => None,
            "owner" => Some(&owner),
            _ => Some(&other),
        };
        let push = c[2].as_bool().unwrap();
        let visible = p.as_ref().is_some_and(|p| super::project::source_visible(p, u, "aallyn", crate::dt::now()));
        let got = match super::authorize_decision(p.as_ref(), u, push, visible) {
            Ok(()) => json!({"ok": true}),
            Err(r) => {
                let status = r.status().as_u16();
                let challenge = r.headers().get(header::WWW_AUTHENTICATE).map(|v| v.to_str().unwrap().to_owned());
                let body = futures::executor::block_on(axum::body::to_bytes(r.into_body(), usize::MAX)).unwrap();
                let v: Value = serde_json::from_slice(&body).unwrap();
                let mut hdrs = serde_json::Map::new();
                if let Some(ch) = challenge {
                    hdrs.insert("WWW-Authenticate".into(), Value::String(ch));
                }
                json!({"error": [status, v["error"]["code"], v["error"]["message"], hdrs]})
            }
        };
        assert_eq!(got, c[3], "{c}");
    }
}

#[test]
fn beanie_saves() {
    for c in GOLDEN["saves"]["projects"].as_array().unwrap() {
        let Bson::Document(raw) = Bson::try_from(c["raw"].clone()).unwrap() else { panic!() };
        let p = Project::from_doc(&raw).unwrap();
        assert_eq!(Bson::Document(p.save_set()).into_canonical_extjson(), c["set"]);
    }
    let t = &GOLDEN["saves"]["token"];
    let Bson::Document(raw) = Bson::try_from(t["set"].clone()).unwrap() else { panic!() };
    let mut tok = GitToken::from_doc(&raw).unwrap();
    assert_eq!(Bson::Document(tok.to_doc()).into_canonical_extjson(), t["set"]);
    tok.last_used_at = crate::dt::parse_utc(s(&GOLDEN["now"]));
    assert_eq!(tok.dto(), t["dto"]);
    let mut fresh = tok.clone();
    fresh.created_at = crate::dt::parse_utc(s(&GOLDEN["now"])).unwrap();
    fresh.last_used_at = None;
    fresh.id = ObjectId::parse_str(s(&t["fresh_dto"]["id"])).unwrap_or(fresh.id);
    let mut want = t["fresh_dto"].clone();
    want["id"] = Value::String(fresh.id.to_hex());
    want["name"] = Value::String(fresh.name.clone());
    want["prefix"] = Value::String(fresh.prefix.clone());
    assert_eq!(fresh.dto(), want);
}

#[test]
fn pure_helpers() {
    let p = &GOLDEN["pure"];
    for c in p["norm_path"].as_array().unwrap() {
        let got = match service::norm_path(s(&c[0])) {
            Ok(v) => json!({"ok": v}),
            Err(e) => json!({"error": [e.status.as_u16(), e.message]}),
        };
        assert_eq!(got, c[1], "{c}");
    }
    for c in p["branch_names"].as_array().unwrap() {
        let got = match service::valid_branch_name(s(&c[0])) {
            Ok(v) => json!({"ok": v}),
            Err(e) => json!({"error": e.message}),
        };
        assert_eq!(got, c[1], "{c}");
    }
    for c in p["safe_filename"].as_array().unwrap() {
        assert_eq!(service::safe_filename(s(&c[0])), s(&c[1]), "{c}");
    }
    for c in p["author_ident"].as_array().unwrap() {
        assert_eq!(String::from_utf8(store::author_ident(s(&c[0]))).unwrap(), s(&c[1]), "{c}");
    }
}

#[test]
fn refnames() {
    for ok in ["refs/heads/main", "refs/heads/a/b", "HEAD", "refs/heads/a//b"] {
        assert!(refs::refname_ok(ok.as_bytes()), "{ok}");
    }
    for bad in
        ["refs/heads/x.lock", "refs/heads/.x", "refs/heads/a..b", "refs/heads/a b", "heads/main", "refs/heads/x/"]
    {
        assert!(!refs::refname_ok(bad.as_bytes()), "{bad}");
    }
}

#[test]
fn zip_layout() {
    let z = archive::build_zip(
        &[("a.txt".into(), b"aaaa".to_vec()), ("é/b".into(), Vec::new())],
        Some([2026, 1, 2, 3, 4, 5]),
    );
    assert_eq!(&z[..4], b"PK\x03\x04");
    assert_eq!(u16::from_le_bytes([z[8], z[9]]), 8);
    let end = z.len() - 22;
    assert_eq!(&z[end..end + 4], b"PK\x05\x06");
    assert_eq!(u16::from_le_bytes([z[end + 10], z[end + 11]]), 2);
}

#[test]
fn report_rewrites() {
    let rejected =
        vec![(b"refs/heads/main".to_vec(), b"a built .tmod belongs in a release, not in the files (x.tmod)".to_vec())];
    let plain = [
        smart::pkt_line(b"unpack ok\n"),
        smart::pkt_line(b"ok refs/heads/main\n"),
        smart::pkt_line(b"ok refs/heads/dev\n"),
        b"0000".to_vec(),
    ]
    .concat();
    let want = [
        smart::pkt_line(b"unpack ok\n"),
        smart::pkt_line(b"ng refs/heads/main a built .tmod belongs in a release, not in the files (x.tmod)\n"),
        smart::pkt_line(b"ok refs/heads/dev\n"),
        b"0000".to_vec(),
    ]
    .concat();
    assert_eq!(smart::rewrite_report(&plain, &rejected, false), want);
    let banded = [
        smart::pkt_line(&[&[2u8][..], b"progress\n"].concat()),
        smart::pkt_line(&[&[1u8][..], &plain[..]].concat()),
        b"0000".to_vec(),
    ]
    .concat();
    let want_banded = [
        smart::pkt_line(&[&[2u8][..], b"progress\n"].concat()),
        smart::pkt_line(&[&[1u8][..], &want[..]].concat()),
        b"0000".to_vec(),
    ]
    .concat();
    assert_eq!(smart::rewrite_report(&banded, &rejected, true), want_banded);
    let cmds = [smart::pkt_line(b"0000000000000000000000000000000000000000 1111111111111111111111111111111111111111 refs/heads/x\0report-status side-band-64k\n"), b"0000PACK".to_vec()].concat();
    let (c, caps) = smart::parse_commands(&cmds).unwrap();
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].name, b"refs/heads/x");
    assert_eq!(caps, b"report-status side-band-64k\n");
    assert!(smart::parse_commands(&cmds[..20]).is_none());
}

fn git(dir: &Path, args: &[&str], stdin: Option<&[u8]>) -> Vec<u8> {
    use std::io::Write;
    let mut c = std::process::Command::new("git")
        .args(["-c", "safe.directory=*"])
        .args(args)
        .current_dir(dir)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut si = c.stdin.take().unwrap();
    if let Some(b) = stdin {
        si.write_all(b).unwrap();
    }
    drop(si);
    let out = c.wait_with_output().unwrap();
    assert!(out.status.success(), "git {args:?}");
    out.stdout
}

#[tokio::test]
async fn smart_http_with_git() {
    if std::process::Command::new("git").arg("--version").output().is_err() {
        eprintln!("git not installed: skipped");
        return;
    }
    let root = scratch("smart");
    let a = repo(&root, "a");
    let b = repo(&root, "b");
    let me = store::author_ident("Alice");
    let c1 =
        store::write_commit(&a, "main", &[("ui/a.txt".into(), b"a".to_vec())], &[], &me, "one", 1_700_000_000).unwrap();
    let c2 =
        store::write_commit(&a, "main", &[("ui/Built.TMOD".into(), b"t".to_vec())], &[], &me, "two", 1_700_000_100)
            .unwrap();
    odb::Repo::ensure(&b).unwrap();

    let adv = smart::advertise(smart::Service::ReceivePack, &b).await.unwrap();
    assert!(adv.starts_with(b"001f# service=git-receive-pack\n0000"));
    let adv = smart::advertise(smart::Service::UploadPack, &a).await.unwrap();
    assert!(adv.starts_with(b"001e# service=git-upload-pack\n0000"));
    assert!(String::from_utf8_lossy(&adv).contains(&c2));

    let pack1 = git(&a, &["pack-objects", "--stdout", "--revs"], Some(format!("{c1}\n").as_bytes()));
    let first = [
        smart::pkt_line(format!("{} {c1} refs/heads/main\0report-status\n", odb::ZERO_HEX).as_bytes()),
        b"0000".to_vec(),
        pack1,
    ]
    .concat();
    let out = smart::receive_pack(&b, Body::from(first), false).await.unwrap();
    assert!(String::from_utf8_lossy(&out).contains("ok refs/heads/main"), "{}", String::from_utf8_lossy(&out));
    assert_eq!(refs::get(&b, b"refs/heads/main").unwrap().unwrap(), c1.as_bytes());
    assert!(!b.join("logs").exists(), "reflog written");

    let pack2 = git(&a, &["pack-objects", "--stdout", "--revs"], Some(format!("{c2}\n^{c1}\n").as_bytes()));
    let second = [
        smart::pkt_line(format!("{c1} {c2} refs/heads/main\0report-status side-band-64k\n").as_bytes()),
        smart::pkt_line(format!("{} {c2} refs/heads/other\n", odb::ZERO_HEX).as_bytes()),
        b"0000".to_vec(),
        pack2,
    ]
    .concat();
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    std::io::Write::write_all(&mut gz, &second).unwrap();
    let out = smart::receive_pack(&b, Body::from(gz.finish().unwrap()), true).await.unwrap();
    let text = String::from_utf8_lossy(&out);
    assert!(
        text.contains("ng refs/heads/main a built .tmod belongs in a release, not in the files (ui/Built.TMOD)"),
        "{text}"
    );
    assert!(text.contains("ng refs/heads/other a built .tmod"), "{text}");
    assert_eq!(refs::get(&b, b"refs/heads/main").unwrap().unwrap(), c1.as_bytes());
    assert!(refs::get(&b, b"refs/heads/other").unwrap().is_none());

    let want =
        [smart::pkt_line(format!("want {c1}\n").as_bytes()), b"0000".to_vec(), smart::pkt_line(b"done\n")].concat();
    let body = smart::upload_pack(&b, Body::from(want), false).await.unwrap();
    let bytes = axum::body::to_bytes(body, usize::MAX).await.unwrap();
    assert!(bytes.windows(4).any(|w| w == b"PACK"), "{}", String::from_utf8_lossy(&bytes));
}

#[tokio::test]
async fn push_replies_match_dulwich() {
    if std::process::Command::new("git").arg("--version").output().is_err() {
        eprintln!("git not installed: skipped");
        return;
    }
    let dir = repo(&scratch("smart_golden"), "smart");
    odb::Repo::ensure(&dir).unwrap();
    let bodies: [&[u8]; 2] = [
        include_bytes!("../../testdata/mods_git/push_ok.bin"),
        include_bytes!("../../testdata/mods_git/push_blocked.bin"),
    ];
    for (body, want) in bodies.iter().zip(GOLDEN["smart"]["replies"].as_array().unwrap()) {
        let out = smart::receive_pack(&dir, Body::from(body.to_vec()), false).await.unwrap();
        assert_eq!(B64.encode(&out), s(&want[1]), "{}: {}", want[0], String::from_utf8_lossy(&out));
    }
    let heads: serde_json::Map<String, Value> = refs::all(&dir)
        .unwrap()
        .into_iter()
        .filter(|(k, _)| k.starts_with(b"refs/"))
        .map(|(k, v)| (String::from_utf8(k).unwrap(), Value::String(String::from_utf8(v).unwrap())))
        .collect();
    assert_eq!(Value::Object(heads), GOLDEN["smart"]["refs"]);
}

#[tokio::test]
async fn routes_answer_before_any_store() {
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    let settings = crate::config::Settings::default();
    let db = crate::db::Db { mongo: None, redis: None, pg: None };
    let state = crate::state::AppState { inner: std::sync::Arc::new(crate::state::Inner { settings, db }) };
    let app = crate::routes::app(state);
    let cases = [
        ("PUT", "/v1/mods/hub/me/git-tokens", 405, Some("GET")),
        ("GET", "/v1/mods/hub/me/git-tokens/abc", 405, Some("DELETE")),
        ("GET", "/v1/mods/hub/projects/a/b/fix-placement", 405, Some("POST")),
        ("PATCH", "/v1/mods/hub/projects/a/b/commits", 405, Some("GET")),
        ("POST", "/git/mods/a/b.git/info/refs", 405, Some("GET")),
        ("GET", "/git/mods/a/b.git/git-receive-pack", 405, Some("POST")),
        ("GET", "/git/mods/a/b.git/info/refs", 422, None),
        ("GET", "/git/mods/a/b.git/info/refs?service=git-annex", 403, None),
        ("GET", "/git/mods/a/b/info/refs?service=git-upload-pack", 404, None),
        ("GET", "/git/mods/a/.git/info/refs?service=git-upload-pack", 404, None),
        ("GET", "/git/mods/a/.git/info/refs", 404, None),
    ];
    for (method, path, status, allow) in cases {
        let peer = axum::extract::ConnectInfo(std::net::SocketAddr::from(([127, 0, 0, 1], 50000)));
        let req = axum::http::Request::builder().method(method).uri(path).extension(peer).body(Body::empty()).unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status().as_u16(), status, "{method} {path}");
        assert_eq!(resp.headers().get(header::ALLOW).and_then(|v| v.to_str().ok()), allow, "{method} {path}");
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        if status == 403 {
            let v: Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(v["error"]["message"], "Unsupported service");
        }
    }
}

#[test]
fn non_git_segments_route_like_starlette() {
    use crate::starlette::{Outcome, Table};
    let t = Table::api();
    let get = axum::http::Method::GET;
    assert!(matches!(t.resolve(&get, "/git/mods/a/b.git/info/refs"), Outcome::Full(_)));
    assert_eq!(t.resolve(&get, "/git/mods/a/b/info/refs"), Outcome::NotFound);
    assert_eq!(t.resolve(&get, "/git/mods/a/.git/info/refs"), Outcome::NotFound);
    assert!(matches!(t.resolve(&axum::http::Method::POST, "/git/mods/a/b.git/info/refs"), Outcome::Partial(_)));
    assert!(matches!(t.resolve(&get, "/v1/mods/hub/projects/a/b/raw/main/"), Outcome::Full(_)));
}

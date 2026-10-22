//! Golden checks against fixtures generated from the Python code
//! (`native/port/parity/gen_btt_fixtures.py`): the pure functions, the BSON
//! the loops write, and every route case replayed through the middleware
//! stack with the fixture documents as the store.

use std::sync::{Arc, OnceLock};

use axum::{
    Router,
    body::Body,
    extract::Request,
    http::{HeaderName, HeaderValue, Method, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete, get},
};
use hyper_util::{client::legacy::Client, rt::TokioExecutor};
use kiwi_core::btt::{
    check_block_values, compare_versions, conventional_prefix, parse_version, platform_order, py_strip,
};
use mongodb::bson::{Bson, Document, doc, oid::ObjectId};
use serde_json::Value;
use tower::ServiceExt;

use super::{
    Store,
    blocklist::{self, BlockAdd, EntryPath},
    first_route_405,
    github::{build_changelog_groups, is_rate_limited, normalize_release},
    releases::{self, ChangelogQuery, ChannelQuery, CheckQuery, ListQuery, PlatformQuery},
    workshop::{self, parse_items},
};
use crate::{
    auth::pyutil::bson_int,
    config::Settings,
    db::Db,
    error::{ApiError, ApiResult},
    middleware::{EmbedSource, Mw, MwConfig, forwarded::TrustedHosts, outer, wrap},
    pyjson::{self, PydanticJson},
    state::{AppState, Inner},
    validation::{Field, FromBody, Path, Query, Validator, read_body},
};

fn golden() -> &'static Value {
    static G: OnceLock<Value> = OnceLock::new();
    G.get_or_init(|| serde_json::from_str(include_str!("../../../testdata/btt/golden.json")).expect("fixture parses"))
}

fn cases(key: &str) -> &'static Vec<Value> {
    golden()[key].as_array().unwrap_or_else(|| panic!("fixture section {key}"))
}

fn s(v: &Value) -> &str {
    v.as_str().expect("string")
}

fn bson_hex(d: Document) -> String {
    let mut out = Vec::new();
    d.to_writer(&mut out).expect("bson encodes");
    hex::encode(out)
}

#[test]
fn versions() {
    for c in cases("versions") {
        let (a, b) = (s(&c["a"]), s(&c["b"]));
        let parsed = parse_version(a).map(|(nums, suffix)| serde_json::json!([nums, suffix]));
        assert_eq!(parsed.unwrap_or(Value::Null), c["parse_a"], "parse {a:?}");
        assert_eq!(compare_versions(a, b).map(Value::from).unwrap_or(Value::Null), c["cmp"], "compare {a:?} {b:?}");
    }
}

#[test]
fn prefixes() {
    for c in cases("prefixes") {
        let got = conventional_prefix(s(&c["msg"]));
        assert_eq!(got.map(Value::from).unwrap_or(Value::Null), c["prefix"], "{:?}", c["msg"]);
    }
}

#[test]
fn platforms() {
    for c in cases("platform_order") {
        let names: Vec<&str> = c["names"].as_array().unwrap().iter().map(s).collect();
        let got: Vec<Value> = platform_order(&names, s(&c["platform"])).into_iter().map(Value::from).collect();
        assert_eq!(Value::Array(got), c["order"], "{c}");
    }
}

#[test]
fn normalized_releases() {
    for c in cases("normalize") {
        match normalize_release(&c["raw"]) {
            Ok(None) => assert!(c["bson"].is_null() && c.get("error").is_none(), "{}", c["raw"]),
            Ok(Some(d)) => assert_eq!(bson_hex(doc! { "doc": d }), s(&c["bson"]), "{}", c["raw"]),
            Err(e) => assert!(c.get("error").is_some(), "{}: unexpected error {e}", c["raw"]),
        }
    }
}

#[test]
fn changelog_groups() {
    for c in cases("groups") {
        let (tags, commits) = (c["tags"].as_array().unwrap(), c["commits"].as_array().unwrap());
        match build_changelog_groups(tags, commits) {
            Ok(groups) => assert_eq!(bson_hex(doc! { "groups": groups }), s(&c["bson"]), "{c}"),
            Err(e) => assert!(c.get("error").is_some(), "{c}: unexpected error {e}"),
        }
    }
}

#[test]
fn github_sample() {
    let g = &golden()["github_sample"];
    for (raw, want) in g["releases"].as_array().unwrap().iter().zip(g["releases_bson"].as_array().unwrap()) {
        let got = normalize_release(raw).unwrap().map(|d| bson_hex(doc! { "doc": d }));
        assert_eq!(got.map(Value::from).unwrap_or(Value::Null), *want, "release {}", raw["id"]);
    }
    let groups = build_changelog_groups(g["tags"].as_array().unwrap(), g["commits"].as_array().unwrap()).unwrap();
    assert_eq!(bson_hex(doc! { "groups": groups }), s(&g["groups_bson"]));
}

#[test]
fn rate_limit_bodies() {
    for c in cases("rate_limited") {
        assert_eq!(is_rate_limited(&c["body"]), c["value"].as_bool().unwrap(), "{}", c["body"]);
    }
}

#[test]
fn steam_items() {
    let steam = &golden()["steam"];
    let items: Vec<Bson> = parse_items(steam["details"].as_array().unwrap()).into_iter().map(Bson::Document).collect();
    assert_eq!(bson_hex(doc! { "items": items }), s(&steam["bson"]));
}

#[test]
fn block_values() {
    let dups: Vec<&str> = golden()["store"]["dups"].as_array().unwrap().iter().map(s).collect();
    for c in cases("block_values") {
        let (kind, raw, note) = (s(&c["kind"]), s(&c["raw"]), s(&c["note"]));
        match check_block_values(kind, raw) {
            Ok(values) => {
                let got: Vec<Value> = values
                    .iter()
                    .filter(|v| !dups.contains(&v.as_str()))
                    .map(|v| serde_json::json!([v, py_strip(note)]))
                    .collect();
                assert_eq!(Value::Array(got), c["added"], "{c}");
            }
            Err(msg) => {
                assert_eq!(c["status"], 400, "{c}");
                assert_eq!(msg, s(&c["message"]), "{c}");
            }
        }
    }
}

#[tokio::test]
#[ignore = "needs network access to api.github.com"]
async fn github_live() {
    let client = super::http_client(std::time::Duration::from_secs(15)).unwrap();
    let settings = Settings::from_vars([("PY_UPSTREAM", "http://py:8000")]).unwrap();
    let releases = super::github::fetch_list(&client, &settings, "releases?per_page=5").await.unwrap();
    assert!(releases.iter().filter_map(|r| normalize_release(r).unwrap()).count() > 0);
    let commits = super::github::fetch_list(&client, &settings, "commits?per_page=5").await.unwrap();
    assert!(!build_changelog_groups(&[], &commits).unwrap().is_empty());
}

fn keys(d: &Document) -> Vec<&str> {
    d.keys().map(String::as_str).collect()
}

/// The documents the loops and the admin routes write, in a scratch database
/// (Python's `insert()` order, then the same order after the update path).
#[tokio::test]
#[ignore = "needs MONGO_URI pointing at a throwaway mongo (writes and drops a scratch database)"]
async fn mongo_writes() {
    let client = mongodb::Client::with_uri_str(std::env::var("MONGO_URI").unwrap()).await.unwrap();
    let db = client.database(&format!("kiwi_btt_test_{}", ObjectId::new().to_hex()));
    let releases = db.collection::<Document>(releases::RELEASES);
    let raw = &cases("normalize")[0]["raw"];
    for _ in 0..2 {
        assert_eq!(super::github::store_releases(&releases, std::slice::from_ref(raw)).await.unwrap(), 1);
    }
    let stored: Vec<Document> =
        futures::TryStreamExt::try_collect(releases.find(doc! {}).await.unwrap()).await.unwrap();
    assert_eq!(stored.len(), 1);
    let want = [
        "_id",
        "release_id",
        "tag_name",
        "name",
        "body",
        "html_url",
        "prerelease",
        "published_at",
        "assets",
        "fetched_at",
    ];
    assert_eq!(keys(&stored[0]), want);
    assert!(matches!(stored[0].get("release_id"), Some(Bson::Int32(101))));

    let logs = db.collection::<Document>(releases::CHANGELOGS);
    let commits = Value::Array(cases("groups")[0]["commits"].as_array().unwrap().clone());
    let tags = cases("groups")[0]["tags"].as_array().unwrap();
    let (n, limited) = super::github::store_changelog(&logs, "r/x", tags, commits).await.unwrap();
    assert!(n > 0 && !limited);
    let limit_body = serde_json::json!({"message": "API rate limit exceeded"});
    assert_eq!(super::github::store_changelog(&logs, "r/x", tags, limit_body).await.unwrap(), (0, true));
    let log = logs.find_one(doc! { "repo": "r/x" }).await.unwrap().unwrap();
    assert_eq!(keys(&log), ["_id", "repo", "groups", "rate_limited", "fetched_at"]);
    assert_eq!(log.get_array("groups").unwrap().len(), n);
    assert_eq!(log.get_bool("rate_limited"), Ok(true));

    let items = db.collection::<Document>(workshop::ITEMS);
    let parsed = parse_items(golden()["steam"]["details"].as_array().unwrap());
    let names = [("76561198000000001".to_owned(), "Alice".to_owned())].into_iter().collect();
    super::workshop::store_items(&items, parsed.clone(), &names).await.unwrap();
    super::workshop::store_items(&items, parsed[..1].to_vec(), &names).await.unwrap();
    let left: Vec<Document> = futures::TryStreamExt::try_collect(items.find(doc! {}).await.unwrap()).await.unwrap();
    assert_eq!(left.len(), parsed.len(), "a sweep under half the stored count prunes nothing");
    let want = [
        "_id",
        "published_file_id",
        "title",
        "description",
        "creator",
        "preview_url",
        "file_size",
        "created",
        "updated",
        "subscriptions",
        "favorites",
        "tags",
        "creator_name",
        "seen_at",
    ];
    assert_eq!(keys(&left[0]), want);

    let st = AppState {
        inner: Arc::new(Inner {
            settings: Settings::from_vars([("PY_UPSTREAM", "http://py:8000")]).unwrap(),
            db: Db { mongo: Some(db.clone()), redis: None, pg: None },
            upstream: Client::builder(TokioExecutor::new()).build_http(),
        }),
    };
    let store = super::Mongo(&st);
    let index = mongodb::IndexModel::builder()
        .keys(doc! { "kind": 1, "value": 1 })
        .options(mongodb::options::IndexOptions::builder().unique(true).build())
        .build();
    db.collection::<Document>(blocklist::BLOCKLIST).create_index(index).await.unwrap();
    let req = BlockAdd { kind: "mod".into(), values: "5 6".into(), note: " n ".into() };
    let added = blocklist::admin_add_core(&store, &req).await.unwrap();
    assert_eq!(added["added"].as_array().unwrap().len(), 2);
    let again = blocklist::admin_add_core(&store, &req).await.unwrap();
    assert_eq!(again["added"], serde_json::json!([]));
    let entry = db.collection::<Document>(blocklist::BLOCKLIST).find_one(doc! {}).await.unwrap().unwrap();
    assert_eq!(keys(&entry), ["_id", "kind", "value", "note", "created_at"]);
    let id = s(&added["added"][0]["id"]).to_owned();
    assert_eq!(blocklist::admin_remove_core(&store, &id).await.unwrap().status(), StatusCode::NO_CONTENT);
    assert!(blocklist::admin_remove_core(&store, &id).await.is_err());
    let (page, total) = store.release_page(Some(false), 0, 10).await.unwrap();
    assert_eq!((page.len(), total), (1, 1));
    db.drop().await.unwrap();
}

// --- routes ------------------------------------------------------------------------

/// Fixture JSON (`$date` / `$oid` markers) as the stored BSON.
fn to_bson(v: &Value) -> Bson {
    match v {
        Value::Null => Bson::Null,
        Value::Bool(b) => Bson::Boolean(*b),
        Value::Number(n) => n.as_i64().map_or_else(|| Bson::Double(n.as_f64().unwrap()), bson_int),
        Value::String(s) => Bson::String(s.clone()),
        Value::Array(a) => Bson::Array(a.iter().map(to_bson).collect()),
        Value::Object(m) => {
            if let Some(ms) = m.get("$date") {
                return Bson::DateTime(mongodb::bson::DateTime::from_millis(ms.as_i64().unwrap()));
            }
            if let Some(oid) = m.get("$oid") {
                return Bson::ObjectId(ObjectId::parse_str(s(oid)).unwrap());
            }
            Bson::Document(m.iter().map(|(k, v)| (k.clone(), to_bson(v))).collect())
        }
    }
}

fn docs(v: &Value) -> Vec<Document> {
    v.as_array().unwrap().iter().map(|d| to_bson(d).as_document().unwrap().clone()).collect()
}

struct Fixture {
    releases: Vec<Document>,
    changelog: Option<Document>,
    workshop: Vec<Document>,
    blocklist: Vec<Document>,
    md5s: Vec<Bson>,
    existing: ObjectId,
    dups: Vec<String>,
}

impl Fixture {
    fn load(with_changelog: bool) -> Fixture {
        let st = &golden()["store"];
        Fixture {
            releases: docs(&st["releases"]),
            changelog: with_changelog.then(|| to_bson(&st["changelog"]).as_document().unwrap().clone()),
            workshop: docs(&st["workshop"]),
            blocklist: docs(&st["blocklist"]),
            md5s: st["takedown_md5s"].as_array().unwrap().iter().map(to_bson).collect(),
            existing: ObjectId::parse_str(s(&st["existing_entry"])).unwrap(),
            dups: st["dups"].as_array().unwrap().iter().map(|d| s(d).to_owned()).collect(),
        }
    }

    fn channel(&self, prerelease: Option<bool>) -> Vec<Document> {
        self.releases
            .iter()
            .filter(|d| prerelease.is_none_or(|p| d.get_bool("prerelease").unwrap_or(false) == p))
            .cloned()
            .collect()
    }
}

impl Store for Fixture {
    async fn release_page(&self, prerelease: Option<bool>, offset: u64, limit: i64) -> ApiResult<(Vec<Document>, u64)> {
        let all = self.channel(prerelease);
        let total = all.len() as u64;
        Ok((all.into_iter().skip(offset as usize).take(limit as usize).collect(), total))
    }

    async fn channel_releases(&self, prerelease: bool) -> ApiResult<Vec<Document>> {
        Ok(self.channel(Some(prerelease)))
    }

    async fn changelog(&self, _repo: &str) -> ApiResult<Option<Document>> {
        Ok(self.changelog.clone())
    }

    async fn workshop_items(&self) -> ApiResult<Vec<Document>> {
        Ok(self.workshop.clone())
    }

    async fn blocklist(&self, newest_first: bool) -> ApiResult<Vec<Document>> {
        let mut out = self.blocklist.clone();
        if newest_first {
            out.reverse();
        }
        Ok(out)
    }

    async fn takedown_md5s(&self) -> ApiResult<Vec<Bson>> {
        Ok(self.md5s.clone())
    }

    async fn insert_block(&self, entry: Document) -> ApiResult<bool> {
        Ok(!self.dups.iter().any(|d| entry.get_str("value") == Ok(d)))
    }

    async fn delete_block(&self, id: ObjectId) -> ApiResult<bool> {
        Ok(id == self.existing)
    }
}

/// A store-less state: enough for routing, 405s and rejected auth.
fn state() -> AppState {
    let settings = Settings::from_vars([("PY_UPSTREAM", "http://py:8000")]).unwrap();
    let upstream = Client::builder(TokioExecutor::new()).build_http();
    AppState { inner: Arc::new(Inner { settings, db: Db { mongo: None, redis: None, pg: None }, upstream }) }
}

fn stack(routes: Router<AppState>) -> Router {
    let mw = Arc::new(Mw::new(
        MwConfig::from_lookup(&|_| None),
        None,
        EmbedSource::Fixed(Vec::new()),
        TrustedHosts::parse("127.0.0.1"),
    ));
    outer(wrap(routes, mw, None).fallback(|| async { (StatusCode::IM_A_TEAPOT, "proxied") }).with_state(state()))
}

/// `AdminBody` without the superuser check (the fixture overrides it).
async fn body_of<T: FromBody>(req: Request) -> Result<T, Response> {
    let (parts, body) = req.into_parts();
    let body = read_body(&parts.headers, body).await.map_err(IntoResponse::into_response)?;
    let mut v = Validator::default();
    let value = T::from_body(Field::new(&mut v, vec!["body".into()], body.input()));
    match v.finish(value) {
        Ok(Some(t)) => Ok(t),
        Ok(None) => Err(ApiError::internal().into_response()),
        Err(e) => Err(e.into_response()),
    }
}

/// The production routes with the auth dependencies overridden and the
/// fixture as the store.
fn fixture_app(fx: Fixture) -> Router {
    let fx = Arc::new(fx);
    let f = |fx: &Arc<Fixture>| fx.clone();
    let (a, b, c, d, e, g, h, i, j, k) =
        (f(&fx), f(&fx), f(&fx), f(&fx), f(&fx), f(&fx), f(&fx), f(&fx), f(&fx), f(&fx));
    let routes = Router::new()
        .route(
            "/v1/btt/releases",
            get(move |Query(q): Query<ListQuery>| async move { releases::list_core(&*a, &q).await.map(PydanticJson) }),
        )
        .route(
            "/v1/btt/latest",
            get(move |Query(q): Query<ChannelQuery>| async move {
                releases::latest_core(&*b, &q).await.map(PydanticJson)
            }),
        )
        .route(
            "/v1/btt/check",
            get(move |Query(q): Query<CheckQuery>| async move { releases::check_core(&*c, &q).await.map(PydanticJson) }),
        )
        .route(
            "/v1/btt/latest/{platform}",
            get(move |Path(q): Path<PlatformQuery>| async move {
                releases::platform_core(&*d, &q).await.map(PydanticJson)
            }),
        )
        .route(
            "/v1/btt/changelog",
            get(move |Query(q): Query<ChangelogQuery>| async move {
                let repo = s(&golden()["store"]["repo"]);
                releases::changelog_core(&*e, &q, repo).await.map(PydanticJson)
            }),
        )
        .route("/v1/btt/workshop", get(move || async move { workshop::catalog_core(&*g).await.map(PydanticJson) }))
        .route(
            "/v1/btt/mod-blocklist",
            get(move || async move { blocklist::public_core(&*h).await.map(PydanticJson) }),
        )
        .route(
            "/admin/btt/mod-blocklist",
            get(move || async move { blocklist::admin_list_core(&*i).await.map(PydanticJson) })
                .post(move |req: Request| async move {
                    let req = match body_of::<BlockAdd>(req).await {
                        Ok(r) => r,
                        Err(resp) => return resp,
                    };
                    match blocklist::admin_add_core(&*j, &req).await {
                        Ok(v) => (StatusCode::CREATED, PydanticJson(v)).into_response(),
                        Err(e) => e.into_response(),
                    }
                })
                .fallback(|| async { first_route_405("GET") }),
        )
        .route(
            "/admin/btt/mod-blocklist/{entry_id}",
            delete(move |Path(EntryPath(id)): Path<EntryPath>| async move {
                blocklist::admin_remove_core(&*k, &id).await
            }),
        );
    stack(routes)
}

fn mask(v: &mut Value, path: &[&str], pattern: &regex::Regex, name: &str) {
    match path.split_first() {
        None => {
            assert!(pattern.is_match(v.as_str().unwrap_or("")), "{name}: {v} does not match {pattern}");
            *v = Value::String("<masked>".into());
        }
        Some((&"*", rest)) => v.as_array_mut().unwrap().iter_mut().for_each(|x| mask(x, rest, pattern, name)),
        Some((head, rest)) => mask(v.get_mut(*head).unwrap(), rest, pattern, name),
    }
}

fn mask_pattern(path: &str) -> regex::Regex {
    regex::Regex::new(match path {
        "fetched_at" => r"^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(\.\d{6})?Z$",
        "added.*.created_at" => r"^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(\.\d{6})?\+00:00$",
        "added.*.id" => r"^[0-9a-f]{24}$",
        other => panic!("no format for {other}"),
    })
    .unwrap()
}

#[tokio::test]
async fn routes_match_python() {
    for c in cases("http") {
        let name = s(&c["name"]);
        let app = if c["real"] == true {
            stack(super::router())
        } else {
            fixture_app(Fixture::load(c["nochangelog"] != true))
        };
        let mut req =
            Request::builder().method(Method::from_bytes(s(&c["method"]).as_bytes()).unwrap()).uri(s(&c["url"]));
        for (k, v) in c["headers"].as_object().unwrap() {
            if k != "x-test-auth" {
                req = req.header(HeaderName::from_bytes(k.as_bytes()).unwrap(), HeaderValue::from_str(s(v)).unwrap());
            }
        }
        let body = c["body"].as_str().map_or_else(Body::empty, |b| Body::from(b.to_owned()));
        let req = req.header("host", "testserver").header("x-request-id", s(&c["request_id"])).body(body).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        let header = |n: &str| resp.headers().get(n).map(|v| v.to_str().unwrap().to_owned());
        assert_eq!(resp.status().as_u16() as u64, c["status"].as_u64().unwrap(), "{name}: status");
        assert_eq!(
            header("content-type").map(Value::from).unwrap_or(Value::Null),
            c["content_type"],
            "{name}: content-type"
        );
        assert_eq!(header("allow").map(Value::from).unwrap_or(Value::Null), c["allow"], "{name}: allow");
        if c["content_length"].is_null() {
            assert_eq!(header("content-length"), None, "{name}: content-length");
        }
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let mut text = String::from_utf8(bytes.to_vec()).unwrap();
        let masks = c["mask"].as_array().unwrap();
        if !masks.is_empty() {
            let mut v: Value = serde_json::from_str(&text).unwrap();
            for m in masks {
                let path: Vec<&str> = s(m).split('.').collect();
                mask(&mut v, &path, &mask_pattern(s(m)), name);
            }
            text = pyjson::dumps(&v);
        }
        assert_eq!(text, s(&c["response"]), "{name}: body");
    }
}

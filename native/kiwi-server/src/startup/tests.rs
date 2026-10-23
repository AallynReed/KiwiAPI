//! Against `testdata/startup/golden.json` (gen_startup_fixtures.py): the
//! Mongo commands `init_db` sends on a fresh, a second and a legacy boot, the
//! index catalog each leaves (both sides run the same mongod emulation), the
//! Postgres scripts and the seeded documents.

use std::cell::RefCell;

use indexmap::IndexMap;
use mongodb::{
    bson::{Bson, Document, doc, oid::ObjectId},
    options::UpdateModifications,
};
use serde_json::Value;

use super::mongo::{self, Mongo, OpError, OpResult};
use crate::config::Settings;

fn golden() -> Value {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/testdata/startup/golden.json");
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn canon(d: Document) -> Value {
    Bson::Document(d).into_canonical_extjson()
}

fn from_canon(v: &Value) -> Document {
    match Bson::try_from(v.clone()).unwrap() {
        Bson::Document(d) => d,
        other => panic!("not a document: {other:?}"),
    }
}

fn settings() -> Settings {
    Settings::from_vars([("PY_UPSTREAM", "http://127.0.0.1:9")]).unwrap()
}

/// mongod 7's index catalog as the fixture generator emulates it.
#[derive(Default)]
struct Fake {
    catalog: RefCell<IndexMap<String, IndexMap<String, Document>>>,
    log: RefCell<Vec<Document>>,
}

fn key_items(spec: &Document) -> Vec<(String, Bson)> {
    spec.get_document("key").unwrap().iter().map(|(k, v)| (k.clone(), v.clone())).collect()
}

fn options(spec: &Document) -> Document {
    spec.iter()
        .filter(|(k, _)| !matches!(k.as_str(), "key" | "name" | "v"))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

fn fail(code: i32) -> OpError {
    OpError { code: Some(code), message: format!("emulated error {code}") }
}

impl Fake {
    fn with(pre: &Value) -> Fake {
        let fake = Fake::default();
        for (coll, specs) in pre.as_object().unwrap() {
            let mut ixs = IndexMap::new();
            ixs.insert("_id_".to_owned(), doc! {"v": 2, "key": {"_id": 1}, "name": "_id_"});
            for s in specs.as_array().unwrap() {
                let s = from_canon(s);
                let mut spec = doc! {"v": 2};
                spec.extend(s.clone());
                ixs.insert(s.get_str("name").unwrap().to_owned(), spec);
            }
            fake.catalog.borrow_mut().insert(coll.clone(), ixs);
        }
        fake
    }

    fn record(&self, op: &str, coll: &str, rest: Document) {
        let mut d = doc! {"op": op, "collection": coll};
        d.extend(rest);
        self.log.borrow_mut().push(d);
    }

    fn create(&self, coll: &str, docs: &[Document]) -> Result<(), i32> {
        let mut catalog = self.catalog.borrow_mut();
        let mut staged = catalog
            .get(coll)
            .cloned()
            .unwrap_or_else(|| IndexMap::from([("_id_".to_owned(), doc! {"v": 2, "key": {"_id": 1}, "name": "_id_"})]));
        for d in docs {
            let name = d.get_str("name").unwrap();
            let mut spec = doc! {"v": 2, "key": d.get_document("key").unwrap().clone(), "name": name};
            spec.extend(
                d.iter().filter(|(k, _)| !matches!(k.as_str(), "key" | "name")).map(|(k, v)| (k.clone(), v.clone())),
            );
            if let Some(same) = staged.get(name) {
                if key_items(same) == key_items(&spec) && options(same) == options(&spec) {
                    continue;
                }
                return Err(86);
            }
            if staged.values().any(|s| key_items(s) == key_items(&spec)) {
                return Err(85);
            }
            staged.insert(name.to_owned(), spec);
        }
        catalog.insert(coll.to_owned(), staged);
        Ok(())
    }

    fn catalog_json(&self) -> Value {
        let out: serde_json::Map<String, Value> = self
            .catalog
            .borrow()
            .iter()
            .map(|(c, ixs)| (c.clone(), Value::Array(ixs.values().cloned().map(canon).collect())))
            .collect();
        Value::Object(out)
    }
}

fn update_bson(u: UpdateModifications) -> Bson {
    match u {
        UpdateModifications::Document(d) => Bson::Document(d),
        UpdateModifications::Pipeline(p) => Bson::Array(p.into_iter().map(Bson::Document).collect()),
        _ => panic!("unexpected update"),
    }
}

impl Mongo for Fake {
    async fn create_indexes(&self, coll: &str, indexes: Vec<Document>) -> OpResult<()> {
        let result = self.create(coll, &indexes);
        let mut rest = doc! {"indexes": indexes};
        if let Err(code) = result {
            rest.insert("error", code);
        }
        self.record("createIndexes", coll, rest);
        result.map_err(fail)
    }

    async fn list_indexes(&self, coll: &str) -> OpResult<Vec<Document>> {
        Ok(self.catalog.borrow().get(coll).map(|ixs| ixs.values().cloned().collect()).unwrap_or_default())
    }

    async fn drop_index(&self, coll: &str, name: &str) -> OpResult<()> {
        let mut catalog = self.catalog.borrow_mut();
        let code = match catalog.get_mut(coll) {
            None => Some(26),
            Some(ixs) if name == "_id_" || !ixs.contains_key(name) => Some(27),
            Some(ixs) => {
                ixs.shift_remove(name);
                None
            }
        };
        drop(catalog);
        let mut rest = doc! {"index": name};
        if let Some(code) = code {
            rest.insert("error", code);
        }
        self.record("dropIndexes", coll, rest);
        code.map_or(Ok(()), |c| Err(fail(c)))
    }

    async fn count_one(&self, coll: &str, filter: Document) -> OpResult<u64> {
        self.record("count_documents", coll, doc! {"filter": filter, "limit": 1});
        Ok(0)
    }

    async fn find(&self, coll: &str, filter: Document, projection: Document) -> OpResult<Vec<Document>> {
        self.record("find", coll, doc! {"filter": filter, "projection": projection});
        Ok(Vec::new())
    }

    async fn find_one(&self, _: &str, _: Document, _: Document, _: Document) -> OpResult<Option<Document>> {
        Ok(None)
    }

    async fn update_many(&self, coll: &str, filter: Document, update: UpdateModifications) -> OpResult<u64> {
        self.record("update_many", coll, doc! {"filter": filter, "update": update_bson(update)});
        Ok(0)
    }

    async fn update_one(&self, coll: &str, filter: Document, update: Document) -> OpResult<()> {
        self.record("update_one", coll, doc! {"filter": filter, "update": update});
        Ok(())
    }
}

fn is_source_drop(op: &Value) -> bool {
    op["op"] == "dropIndexes" && op["collection"] == "mod_projects" && op["index"] == "source_1_source_id_1"
}

fn by_name(catalog: &Value) -> Value {
    let mut out = catalog.clone();
    for ixs in out.as_object_mut().unwrap().values_mut() {
        ixs.as_array_mut().unwrap().sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    }
    out
}

/// Runs `init_db` on `fake` and checks it against the golden `run`. Python
/// drops the source index on every boot; Rust only when it is the sparse
/// one, which moves that index in the catalog order and nothing else.
async fn check(fake: &Fake, run: &Value, source_dropped: bool) {
    mongo::init_db(fake, &settings()).await;
    let want: Vec<Value> = run["commands"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|op| op["op"] != "listIndexes" && (source_dropped || !is_source_drop(op)))
        .cloned()
        .collect();
    let got: Vec<Value> = fake.log.borrow_mut().drain(..).map(canon).collect();
    assert_eq!(got.len(), want.len(), "command count");
    for (i, (g, w)) in got.iter().zip(&want).enumerate() {
        assert_eq!(exact(g), exact(w), "command {i}");
    }
    assert_eq!(exact(&by_name(&fake.catalog_json())), exact(&by_name(&run["catalog"])));
}

/// serde_json maps compare without order; BSON key order matters here.
fn exact(v: &Value) -> String {
    serde_json::to_string(v).unwrap()
}

#[tokio::test]
async fn init_db_matches_python_on_a_fresh_database() {
    let g = golden();
    assert_eq!(g["settings"]["usage_retention_days"], 30);
    assert_eq!(g["settings"]["pageview_retention_days"], 90);
    assert_eq!(g["settings"]["email_outbox_retention_days"], 30);
    let fake = Fake::default();
    check(&fake, &g["fresh"], false).await;
    assert_eq!(exact(&fake.catalog_json()), exact(&g["fresh"]["catalog"]), "fresh catalog order");
    check(&fake, &g["again"], false).await;
}

#[tokio::test]
async fn init_db_migrates_legacy_indexes_like_python() {
    let g = golden();
    let fake = Fake::with(&g["legacy_pre"]);
    check(&fake, &g["legacy"], true).await;
    assert_eq!(exact(&fake.catalog_json()), exact(&g["legacy"]["catalog"]), "legacy catalog order");
}

#[test]
fn every_beanie_model_is_declared_in_order() {
    let g = golden();
    let names: Vec<&str> = super::indexes::beanie().iter().map(|(c, _)| *c).collect();
    let want: Vec<&str> = g["models"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
    assert_eq!(names, want);
}

fn sql_tokens(sql: &str) -> String {
    sql.lines()
        .map(|l| l.split("--").next().unwrap())
        .collect::<Vec<_>>()
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[test]
fn postgres_schemas_are_pythons_without_comments() {
    let g = golden();
    let want = g["pg_schemas"].as_array().unwrap();
    assert_eq!(want.len(), super::pg::SCHEMAS.len());
    for ((name, sql), py) in super::pg::SCHEMAS.iter().zip(want) {
        assert_eq!(sql_tokens(sql), sql_tokens(py.as_str().unwrap()), "{name}");
    }
}

fn epoch() -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::from_timestamp(1_767_225_600, 0).unwrap()
}

#[test]
fn seeded_documents_match_beanie() {
    let g = golden();
    let seeds = &g["seeds"];
    let zero = ObjectId::from_bytes([0; 12]);

    let user = super::admin_user(zero, "parity@aallyn.net".into(), "$argon2id$x".into(), epoch());
    let mut row = doc! {"_id": zero};
    row.extend(crate::portal::store::user_fields(&user));
    assert_eq!(exact(&canon(row)), exact(&seeds["admin_user"]));

    let supporters: Vec<Value> = super::DEFAULT_SUPPORTERS
        .iter()
        .map(|n| {
            let s =
                crate::routes::leaf::supporters::Supporter { name: (*n).into(), added_by: None, created_at: epoch() };
            canon(s.to_doc(zero))
        })
        .collect();
    assert_eq!(exact(&Value::Array(supporters)), exact(&seeds["supporters"]));

    let item = crate::market::store::InterestItem { name: "Glim".into(), added_by: None, added_at: epoch() };
    assert_eq!(exact(&canon(item.to_doc(zero))), exact(&seeds["interest_item"]));
}

#[test]
fn interest_seed_skips_the_repeated_name() {
    let g = golden();
    let file: Vec<String> = serde_json::from_value(g["seeds"]["interest_items_file"].clone()).unwrap();
    let seed = super::interest_seed(&file);
    assert_eq!(file.len(), 318);
    assert_eq!(seed.len(), 317);
    let mut want: Vec<&str> = file.iter().map(String::as_str).collect();
    let second = want.iter().rposition(|n| *n == "Primal Paragon Pinata").unwrap();
    want.remove(second);
    assert_eq!(seed, want);
}

/// Opt-in, writes: point MONGO_URI/MONGO_DB and POSTGRES_DSN at throwaway
/// stores (mongo:7, postgres:17) and run with `--ignored`.
#[tokio::test]
#[ignore = "writes to MONGO_URI and POSTGRES_DSN"]
async fn live_startup_is_idempotent() {
    if std::env::var("PY_UPSTREAM").is_err() {
        // SAFETY: only this ignored test runs in its process invocation.
        unsafe { std::env::set_var("PY_UPSTREAM", "http://127.0.0.1:9") };
    }
    let state = crate::state::AppState::new(Settings::from_env().unwrap()).await.unwrap();
    super::run(&state).await;
    super::run(&state).await;
    let db = state.mongo();
    let live = mongo::Live(db);
    let mut total = 0;
    for (coll, specs) in super::indexes::beanie() {
        let names: Vec<String> =
            live.list_indexes(coll).await.unwrap().iter().map(|d| d.get_str("name").unwrap().to_owned()).collect();
        for s in specs {
            assert!(names.iter().any(|n| n == s.get_str("name").unwrap()), "{coll}: {s}");
            total += 1;
        }
    }
    assert_eq!(total, 146);
    let client = state.pg().get().await.unwrap();
    let tables: i64 = client
        .query_one("SELECT count(*) FROM pg_tables WHERE schemaname = 'public' AND tablename NOT LIKE 'entry_p_%'", &[])
        .await
        .unwrap()
        .get(0);
    assert!(tables >= 21, "{tables} tables");
}

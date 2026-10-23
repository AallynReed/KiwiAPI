//! The Discord area against `testdata/discord/golden.json`
//! (`native/port/parity/gen_discord_fixtures.py`): slash commands over a fake
//! domain, embed builders, contexts, the dashboard routes over fake Discord
//! REST and an in-memory store, models, reconcile, permissions and signatures.

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Mutex, OnceLock},
};

use chrono::{DateTime, Utc};
use kiwi_core::gems::{GemError, evaluator::StatInput};
use mongodb::bson::{Bson, Document, oid::ObjectId};
use serde_json::{Value, json};

use super::{
    commands::{self, Domain, Ping},
    contexts, dashboard,
    embeds::{self, PlatformRelease},
    models::{Club, GuildConfig, demote_rank, promote_rank, reconcile},
    rest::{self, RResult, Rest, RestError},
};
use crate::{
    auth::models::SiteUser,
    error::ApiError,
    feeds::store::News,
    gamedata::Gamedata,
    i18n::{Lang, discord_locale_to_lang},
    middleware::pyurl::quote,
    pyjson,
    validation::read_body,
};

pub(crate) fn golden() -> &'static Value {
    static G: OnceLock<Value> = OnceLock::new();
    G.get_or_init(|| {
        let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata/discord/golden.json");
        serde_json::from_slice(&std::fs::read(p).expect("golden.json")).expect("golden parses")
    })
}

fn now() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(golden()["settings"]["now"].as_str().unwrap()).unwrap().with_timezone(&Utc)
}

fn asset() -> String {
    golden()["settings"]["asset_url"].as_str().unwrap().to_owned()
}

fn gamedata() -> &'static Gamedata {
    static G: OnceLock<Gamedata> = OnceLock::new();
    G.get_or_init(|| Gamedata::new(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/gamedata"), None))
}

// --- slash commands ------------------------------------------------------------------------

struct FakeDomain {
    s: Value,
    recorded: Mutex<Vec<String>>,
}

const GUILD_LANG: [(&str, &str); 9] = [
    ("111", "fr"),
    ("222", "ja"),
    ("333", "de"),
    ("444", "ko"),
    ("555", "ru"),
    ("666", "zh-CN"),
    ("777", "th"),
    ("888", "pt-PT"),
    ("999", "es"),
];

fn news_of(v: &Value) -> News {
    News {
        url: v["url"].as_str().unwrap().into(),
        title: v["title"].as_str().unwrap().into(),
        author: String::new(),
        summary: v["summary"].as_str().unwrap().into(),
        category: String::new(),
        categories: Vec::new(),
        image: v["image"].as_str().map(str::to_owned),
        published_at: v["published_at"].as_str().map(|s| crate::dt::parse_utc(s).unwrap()),
    }
}

impl Domain for FakeDomain {
    fn now_us(&self) -> i64 {
        now().timestamp_micros()
    }
    fn asset_url(&self) -> String {
        asset()
    }
    async fn guild_language(&self, guild_id: &Value, locale: Option<&str>) -> &'static str {
        if crate::feeds::pyval::truthy(guild_id) {
            let g = crate::feeds::pyval::py_str(guild_id);
            if let Some((_, l)) = GUILD_LANG.iter().find(|(k, _)| *k == g) {
                return l;
            }
        }
        discord_locale_to_lang(locale)
    }
    async fn record_command(&self, name: &str) {
        self.recorded.lock().unwrap().push(name.to_owned());
    }
    async fn status(&self) -> Value {
        self.s["status"].clone()
    }
    async fn activity(&self, _: &str) -> anyhow::Result<(Value, Value)> {
        Ok((self.s["live"].clone(), self.s["series"].clone()))
    }
    async fn chaos(&self) -> anyhow::Result<Value> {
        Ok(self.s["chaos"].clone())
    }
    fn server_time(&self) -> Value {
        self.s["server_time"].clone()
    }
    fn daily_buffs(&self) -> Value {
        self.s["daily"].clone()
    }
    fn weekly_buffs(&self) -> Value {
        self.s["weekly"].clone()
    }
    async fn challenge(&self) -> anyhow::Result<Value> {
        Ok(self.s["challenge"].clone())
    }
    fn biome_rotation(&self) -> anyhow::Result<Value> {
        Ok(self.s["biomes"].clone())
    }
    fn wild_mana(&self) -> anyhow::Result<Value> {
        Ok(self.s["wild_mana"].clone())
    }
    fn stampy(&self) -> anyhow::Result<Value> {
        Ok(self.s["stampy"].clone())
    }
    fn corruxion(&self) -> Value {
        self.s["corruxion"].clone()
    }
    fn fluxion(&self) -> Value {
        self.s["fluxion"].clone()
    }
    async fn giveaways(&self) -> anyhow::Result<(Vec<Value>, Vec<Value>, Vec<Value>)> {
        let g = self.s["giveaways"].as_array().unwrap();
        let part = |i: usize| g[i].as_array().unwrap().clone();
        Ok((part(0), part(1), part(2)))
    }
    async fn news(&self, limit: usize) -> Vec<News> {
        self.s["news"].as_array().unwrap().iter().take(limit).map(news_of).collect()
    }
    async fn item_summary(&self, name: &str) -> anyhow::Result<Option<Value>> {
        Ok((name.to_lowercase() == "dragon egg").then(|| self.s["summary"].clone()))
    }
    async fn history(&self, env: &str, _: i64) -> anyhow::Result<Value> {
        Ok(self.s["history"][env].clone())
    }
    fn evaluate_gem(&self, tier: i64, gem_type: i64, level: i64, stats: &[StatInput]) -> Result<Value, GemError> {
        crate::gems::evaluate_json(gamedata(), tier, gem_type, level, stats, true)
    }
    async fn btt_latest(&self) -> anyhow::Result<Vec<(&'static str, PlatformRelease)>> {
        let l = &self.s["latest"];
        Ok(["windows", "linux", "android"]
            .into_iter()
            .map(|p| {
                let e = &l[p];
                let entry =
                    (!e.is_null()).then(|| (e[0].clone(), e[1].clone(), (!e[2].is_null()).then(|| e[2].clone())));
                (p, entry)
            })
            .collect())
    }
    async fn changelog(&self) -> anyhow::Result<Option<Value>> {
        Ok((!self.s["changelog"].is_null()).then(|| self.s["changelog"].clone()))
    }
    async fn ping(&self) -> (Ping, Option<Ping>) {
        let p = &self.s["ping"];
        let ms = |v: &Value| (v.as_f64().unwrap() - 0.0) * 1000.0;
        (Ok(ms(&p[0])), Some(Ok(ms(&p[1]))))
    }
    async fn luxion(&self) -> anyhow::Result<Value> {
        Ok(self.s.get("luxion").cloned().unwrap_or_else(|| json!({"active": false, "schedule": []})))
    }
    async fn latest_update(&self) -> anyhow::Result<Option<Value>> {
        Ok(self.s.get("update").cloned().filter(|v| !v.is_null()))
    }
}

#[tokio::test]
async fn slash_commands_match_python() {
    let g = &golden()["commands"];
    let scenarios = g["scenarios"].as_array().unwrap();
    let mut failures = Vec::new();
    for c in g["cases"].as_array().unwrap() {
        let d =
            FakeDomain { s: scenarios[c["scenario"].as_u64().unwrap() as usize].clone(), recorded: Mutex::default() };
        let got = commands::handle(&d, &c["interaction"]).await;
        let recorded: Vec<Value> = d.recorded.lock().unwrap().iter().map(|s| json!(s)).collect();
        assert_eq!(Value::Array(recorded), c["recorded"], "{}", c["interaction"]);
        match (got, c.get("ok")) {
            (Ok(v), Some(want)) => {
                let (a, b) = (pyjson::dumps(&v), pyjson::dumps(want));
                if a != b {
                    failures.push(format!("{}\n  rust: {a}\n  py:   {b}", c["interaction"]));
                }
            }
            (Err(()), None) => {}
            (got, _) => failures.push(format!("{}: {got:?} vs {}", c["interaction"], c)),
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} differ:\n{}",
        failures.len(),
        g["cases"].as_array().unwrap().len(),
        failures[..failures.len().min(5)].join("\n")
    );
}

#[tokio::test]
async fn announcement_registry_matches_python() {
    let g = &golden()["registry"];
    let d = FakeDomain { s: g["scenario"].clone(), recorded: Mutex::default() };
    for c in g["cases"].as_array().unwrap() {
        let l = Lang::new(c["lang"].as_str());
        let key = c["key"].as_str().unwrap();
        let built = super::announcements::build_embed(&d, l, key).await.unwrap().unwrap();
        assert_eq!(pyjson::dumps(&built), pyjson::dumps(&c["embed"]), "{key} {l:?}");
        let cap = super::announcements::caption(&d, l, key).await.unwrap();
        assert_eq!(cap.unwrap_or(Value::Null), c["caption"], "{key}");
        match (contexts::context(&d, l, key).await, c["context"].get("ok")) {
            (Ok(ctx), Some(want)) => assert_eq!(pyjson::dumps(&Value::Object(ctx)), pyjson::dumps(want), "{key} {l:?}"),
            (Err(_), None) => {}
            (got, _) => panic!("{key}: {got:?} vs {}", c["context"]),
        }
    }
}

#[test]
fn builders_match_python() {
    let g = &golden()["builders"];
    let n = now();
    let minute = n.timestamp().div_euclid(60);
    for c in g["builders"].as_array().unwrap() {
        let l = Lang::new(c["lang"].as_str());
        let input = &c["input"];
        let got = match c["kind"].as_str().unwrap() {
            "luxion" => embeds::luxion_embed(l, &asset(), input, n.timestamp()).unwrap(),
            "luxion_caption" => embeds::luxion_caption(l, input, n.timestamp()).unwrap(),
            "live_board" => embeds::live_board_embed(
                l,
                &asset(),
                input["overall"].as_str().unwrap(),
                minute,
                &crate::dt::isoformat(&n),
            ),
            "market_watch" => {
                embeds::market_watch_embed(l, "Dragon Egg", &input["summary"], input["max"].as_f64()).unwrap()
            }
            "game_update" => embeds::game_update_embed(l, &asset(), (!input.is_null()).then_some(input)),
            other => panic!("{other}"),
        };
        assert_eq!(pyjson::dumps(&got), pyjson::dumps(&c["out"]), "{c}");
    }
    for c in g["fmt_g"].as_array().unwrap() {
        let x = match &c[0] {
            Value::Number(n) => n.as_f64().unwrap(),
            Value::Null => f64::NAN,
            other => panic!("{other}"),
        };
        let x = if c[1] == "inf" {
            f64::INFINITY
        } else if c[1] == "nan" {
            f64::NAN
        } else {
            x
        };
        assert_eq!(embeds::fmt_g(x), c[1].as_str().unwrap(), "{c}");
    }
    for c in g["flux"].as_array().unwrap() {
        assert_eq!(embeds::flux(Some(&c[0])).unwrap(), c[1].as_str().unwrap(), "{c}");
    }
    for c in g["title"].as_array().unwrap() {
        assert_eq!(embeds::title(c[0].as_str().unwrap()), c[1].as_str().unwrap());
    }
}

#[test]
fn contexts_match_python() {
    let g = &golden()["contexts"];
    for (lang, kinds) in g["defaults"].as_object().unwrap() {
        for (kind, want) in kinds.as_object().unwrap() {
            let got = contexts::default_template(Lang::new(Some(lang)), kind).unwrap().to_value();
            assert_eq!(pyjson::dumps(&got), pyjson::dumps(want), "{lang} {kind}");
        }
    }
    for c in g["live"].as_array().unwrap() {
        let l = Lang::new(c["lang"].as_str());
        let got = match c["kind"].as_str().unwrap() {
            "challenge" => contexts::ctx_challenge(l, &c["input"]).unwrap(),
            _ => contexts::ctx_status(l, &c["input"]),
        };
        assert_eq!(pyjson::dumps(&Value::Object(got)), pyjson::dumps(&c["out"]), "{c}");
    }
}

#[test]
fn meta_matches_python() {
    let m = &golden()["meta"];
    let keys: Vec<Value> = super::announcements::keys().into_iter().map(|k| json!(k)).collect();
    assert_eq!(Value::Array(keys), m["keys"]);
    for (t, want) in super::announcements::TYPES.iter().zip(m["catalog"].as_array().unwrap()) {
        assert_eq!(
            json!({"key": t.key, "label": t.label, "description": t.description, "category": t.category}),
            *want
        );
        assert_eq!(json!(contexts::has_customization(t.key)), m["customizable"][t.key]);
    }
    assert_eq!(Value::Array(crate::admin::discord::COMMAND_DEFS.clone()), m["commands"]);
}

// --- signatures ------------------------------------------------------------------------------

#[test]
fn verify_matches_python() {
    let v = &golden()["verify"];
    for c in v["cases"].as_array().unwrap() {
        let got = super::verify(
            c["key"].as_str(),
            c["sig"].as_str().unwrap(),
            c["ts"].as_str().unwrap(),
            c["body"].as_str().unwrap().as_bytes(),
        );
        assert_eq!(got, c["ok"].as_bool().unwrap(), "{c}");
    }
    for c in v["fromhex"].as_array().unwrap() {
        let got = super::fromhex(c[0].as_str().unwrap()).map(hex::encode);
        assert_eq!(got.as_deref(), c[1].as_str(), "{c}");
    }
}

// --- models, reconcile, permissions ------------------------------------------------------------

fn bson_of(v: &Value) -> Document {
    match Bson::try_from(v.clone()).unwrap() {
        Bson::Document(d) => d,
        other => panic!("{other}"),
    }
}

fn ext(d: &Document) -> String {
    serde_json::to_string(&Bson::Document(d.clone()).into_canonical_extjson()).unwrap()
}

fn ext_v(v: &Value) -> String {
    serde_json::to_string(v).unwrap()
}

#[test]
fn guild_config_round_trip_matches_beanie() {
    let m = &golden()["models"];
    for c in m["guild_configs"].as_array().unwrap() {
        let mut cfg = GuildConfig::from_doc(&bson_of(&c["in"]), now()).unwrap();
        assert_eq!(cfg.migrate_legacy(), c["migrated"].as_bool().unwrap());
        assert_eq!(ext(&cfg.to_doc()), ext_v(&c["out"]), "{}", c["in"]);
    }
    let want_cfg = &m["new_config"];
    let mut cfg = GuildConfig::new(5, now());
    let ts = bson_of(want_cfg).get("created_at").cloned().unwrap();
    let Bson::DateTime(at) = ts else { panic!() };
    cfg.created_at = crate::dt::from_bson(at);
    cfg.updated_at = cfg.created_at;
    assert_eq!(ext(&cfg.to_doc()), ext_v(want_cfg));
    let want_club = &m["new_club"];
    let Bson::DateTime(at) = bson_of(want_club).get("created_at").cloned().unwrap() else { panic!() };
    let club = Club::new(5, "New Club", Some(7), crate::dt::from_bson(at));
    assert_eq!(ext(&club.to_doc()), ext_v(want_club));
}

#[test]
fn reconcile_and_ranks_match_python() {
    let r = &golden()["reconcile"];
    for c in r["reconcile"].as_array().unwrap() {
        let ids = |k: &str| c[k].as_array().unwrap().iter().map(|v| v.as_i64().unwrap()).collect();
        let mut cfg = reconcile_base();
        assert_eq!(reconcile(&mut cfg, &ids("channels"), &ids("roles")), c["changed"].as_bool().unwrap(), "{c}");
        assert_eq!(ext(&cfg.to_doc()), ext_v(&c["out"]), "{c}");
    }
    for c in r["ranks"].as_array().unwrap() {
        let linked = c[1].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_owned()).collect();
        let rank = c[0].as_str().unwrap();
        assert_eq!(promote_rank(rank, &linked), c[2].as_str(), "{c}");
        assert_eq!(demote_rank(rank, &linked), c[3].as_str(), "{c}");
    }
}

/// The document `gen_reconcile` starts from.
fn reconcile_base() -> GuildConfig {
    let d = golden()["reconcile"]["reconcile"][0]["out"].clone();
    let mut doc = bson_of(&d);
    let ann = mongodb::bson::doc! {
        "a": {"enabled": true, "channel_id": 1, "ping_role_ids": [10, 11], "ping_role_id": null, "template": null,
              "last_anchor": null, "channel_missing": false},
        "b": {"enabled": false, "channel_id": 2, "ping_role_ids": [], "ping_role_id": null, "template": null,
              "last_anchor": null, "channel_missing": true},
        "c": {"enabled": true, "channel_id": null, "ping_role_ids": [], "ping_role_id": null, "template": null,
              "last_anchor": null, "channel_missing": true},
        "d": {"enabled": false, "channel_id": 3, "ping_role_ids": [], "ping_role_id": null, "template": null,
              "last_anchor": null, "channel_missing": true},
    };
    doc.insert("announcements", ann);
    doc.insert(
        "live_board",
        mongodb::bson::doc! {"enabled": false, "channel_id": 9, "message_id": 4, "channel_missing": false},
    );
    doc.insert("config_perms", mongodb::bson::doc! {"x": [10], "y": [10, 12]});
    GuildConfig::from_doc(&doc, now()).unwrap()
}

#[test]
fn permissions_match_python() {
    let p = &golden()["permissions"];
    let guild = &p["guild"];
    for c in p["cases"].as_array().unwrap() {
        let base = rest::guild_permissions(guild, &c["member"]).unwrap();
        let eff = rest::effective_channel_permissions(guild, &c["member"], &c["channel"]).unwrap();
        assert_eq!(base, c["base"].as_i64().unwrap() as i128, "{c}");
        assert_eq!(eff, c["effective"].as_i64().unwrap() as i128, "{c}");
    }
    let channels =
        golden()["dashboard"]["worlds"][0]["paths"][format!("/guilds/{GID}/channels")].as_array().unwrap().clone();
    let me = &golden()["dashboard"]["worlds"][0]["paths"][format!("/guilds/{GID}/members/{BOT}")];
    for (cid, want) in [CH_TEXT, CH_NEWS, CH_LOCKED, 1].iter().zip(p["preflight"].as_array().unwrap()) {
        assert_eq!(rest::preflight_for(guild, me, &channels, *cid).unwrap(), *want);
    }
    assert_eq!(Value::Array(rest::text_channels(&channels).unwrap()), p["text_channels"]);
    let roles = guild["roles"].as_array().unwrap();
    assert_eq!(Value::Array(rest::assignable_roles(roles, GID).unwrap()), p["assignable"]);
}

const GID: i64 = 4242424242424242;
const BOT: i64 = 999000000000000001;
const CH_TEXT: i64 = 800000000000000001;
const CH_NEWS: i64 = 800000000000000002;
const CH_LOCKED: i64 = 800000000000000004;

// --- dashboard routes ------------------------------------------------------------------------

struct FakeDeps {
    world: Value,
    db: Mutex<HashMap<String, Vec<Document>>>,
    ids: Mutex<u32>,
    sent: Mutex<Vec<Value>>,
    cleared: Mutex<Vec<Value>>,
}

impl FakeDeps {
    fn new(world: Value, db: &Value) -> Self {
        let mut m = HashMap::new();
        for (k, docs) in db.as_object().unwrap() {
            m.insert(k.clone(), docs.as_array().unwrap().iter().map(bson_of).collect());
        }
        FakeDeps { world, db: Mutex::new(m), ids: Mutex::new(0), sent: Mutex::default(), cleared: Mutex::default() }
    }

    fn next_id(&self) -> ObjectId {
        let mut n = self.ids.lock().unwrap();
        *n += 1;
        ObjectId::parse_str(format!("66000000000000000000{:04x}", *n)).unwrap()
    }

    fn insert(&self, coll: &str, d: Document) -> ObjectId {
        let id = self.next_id();
        let mut doc = Document::new();
        doc.insert("_id", id);
        doc.extend(d);
        self.db.lock().unwrap().entry(coll.into()).or_default().push(doc);
        id
    }

    fn set(&self, coll: &str, d: Document) {
        let id = d.get_object_id("_id").unwrap();
        let mut db = self.db.lock().unwrap();
        let docs = db.entry(coll.into()).or_default();
        match docs.iter_mut().find(|x| x.get_object_id("_id").ok() == Some(id)) {
            Some(old) => {
                for (k, v) in d {
                    old.insert(k, v);
                }
            }
            None => docs.push(d),
        }
    }

    fn find(&self, coll: &str, f: impl Fn(&Document) -> bool) -> Vec<Document> {
        self.db.lock().unwrap().get(coll).map(|d| d.iter().filter(|x| f(x)).cloned().collect()).unwrap_or_default()
    }

    fn dump(&self) -> Value {
        let db = self.db.lock().unwrap();
        let mut out = serde_json::Map::new();
        for k in ["guild_configs", "clubs"] {
            let docs: Vec<Value> = db
                .get(k)
                .map(|d| d.iter().map(|x| Bson::Document(x.clone()).into_canonical_extjson()).collect())
                .unwrap_or_default();
            out.insert(k.into(), Value::Array(docs));
        }
        Value::Object(out)
    }
}

fn int_of(d: &Document, k: &str) -> Option<i64> {
    match d.get(k) {
        Some(Bson::Int32(i)) => Some(i64::from(*i)),
        Some(Bson::Int64(i)) => Some(*i),
        _ => None,
    }
}

impl Rest for FakeDeps {
    async fn get(&self, path: &str, _: u64) -> RResult<Option<Value>> {
        if let Some(m) = self.world["errors"].get(path) {
            return Err(RestError::Rest(m.as_str().unwrap().into()));
        }
        if self.world["bad"].as_array().unwrap().iter().any(|p| p == path) {
            return Err(RestError::Other("bad payload".into()));
        }
        Ok(self.world["paths"].get(path).cloned())
    }
    async fn send(&self, method: &str, path: &str, reason: Option<&str>) -> RResult<()> {
        let r = reason.filter(|r| !r.is_empty()).map(|r| quote(kiwi_core::pystr::prefix(r, 400), ""));
        self.sent.lock().unwrap().push(json!([method, path, r]));
        if let Some(m) = self.world["errors"].get(path) {
            return Err(RestError::Rest(m.as_str().unwrap().into()));
        }
        Ok(())
    }
    async fn bot_user_id(&self) -> RResult<Option<i64>> {
        Ok(self.world["bot_id"].as_i64())
    }
    async fn user_guilds(&self, _: &str) -> Option<Vec<Value>> {
        self.world["user_guilds"].as_array().cloned()
    }
}

impl dashboard::Deps for FakeDeps {
    fn now(&self) -> DateTime<Utc> {
        now()
    }
    fn invite_url(&self) -> Option<String> {
        let id = golden()["settings"]["client_id"].as_str()?;
        Some(format!("https://discord.com/oauth2/authorize?client_id={id}&scope=bot&permissions=5084151364439105"))
    }
    fn reconnect_url(&self) -> Option<String> {
        let s = &golden()["settings"];
        s["oauth"]
            .as_bool()
            .unwrap()
            .then(|| format!("{}/v1/site-auth/oauth/discord/start", s["api_url"].as_str().unwrap()))
    }
    async fn discord_token(&self, user: ObjectId) -> Result<Option<String>, ApiError> {
        Ok(self.world["tokens"].get(user.to_hex()).and_then(Value::as_str).map(str::to_owned))
    }
    async fn clear_token(&self, user: ObjectId) -> Result<(), ApiError> {
        self.cleared.lock().unwrap().push(json!(user.to_hex()));
        Ok(())
    }
    async fn find_config(&self, guild_id: i64) -> Result<Option<Document>, ApiError> {
        Ok(self.find("guild_configs", |d| int_of(d, "guild_id") == Some(guild_id)).into_iter().next())
    }
    async fn find_configs(&self, ids: &[i64]) -> Result<Vec<Document>, ApiError> {
        Ok(self.find("guild_configs", |d| int_of(d, "guild_id").is_some_and(|g| ids.contains(&g))))
    }
    async fn save_config(&self, d: Document) -> Result<(), ApiError> {
        self.set("guild_configs", d);
        Ok(())
    }
    async fn insert_config(&self, d: Document) -> Result<ObjectId, ApiError> {
        Ok(self.insert("guild_configs", d))
    }
    async fn clubs(&self, guild_id: i64) -> Result<Vec<Document>, ApiError> {
        let mut v = self.find("clubs", |d| int_of(d, "guild_id") == Some(guild_id));
        v.sort_by_key(|d| d.get_datetime("created_at").ok().copied());
        Ok(v)
    }
    async fn count_clubs(&self, guild_id: i64) -> Result<u64, ApiError> {
        Ok(self.find("clubs", |d| int_of(d, "guild_id") == Some(guild_id)).len() as u64)
    }
    async fn get_club(&self, id: ObjectId) -> Result<Option<Document>, ApiError> {
        Ok(self.find("clubs", |d| d.get_object_id("_id").ok() == Some(id)).into_iter().next())
    }
    async fn insert_club(&self, d: Document) -> Result<ObjectId, ApiError> {
        Ok(self.insert("clubs", d))
    }
    async fn save_club(&self, d: Document) -> Result<(), ApiError> {
        self.set("clubs", d);
        Ok(())
    }
    async fn delete_club(&self, id: ObjectId) -> Result<(), ApiError> {
        let mut db = self.db.lock().unwrap();
        if let Some(docs) = db.get_mut("clubs")
            && let Some(i) = docs.iter().position(|d| d.get_object_id("_id").ok() == Some(id))
        {
            docs.remove(i);
        }
        Ok(())
    }
    async fn anchor(&self, key: &str) -> Result<Option<String>, ()> {
        match self.world["anchors"].get(key).and_then(Value::as_str) {
            Some("RAISE") => Err(()),
            other => Ok(other.map(str::to_owned)),
        }
    }
}

fn site_user(discord_id: Option<i64>) -> SiteUser {
    let mut d = Document::new();
    d.insert("_id", ObjectId::parse_str("65a1b2c3d4e5f60718293a4b").unwrap());
    if let Some(i) = discord_id {
        d.insert("discord_id", i);
    }
    mongodb::bson::from_document(d).unwrap()
}

fn path_params(template: &str, path: &str) -> Vec<(String, String)> {
    let decoded = percent_encoding::percent_decode_str(path).decode_utf8_lossy().into_owned();
    template
        .split('/')
        .zip(decoded.split('/'))
        .filter_map(|(t, v)| {
            t.strip_prefix('{').and_then(|n| n.strip_suffix('}')).map(|n| (n.to_owned(), v.to_owned()))
        })
        .collect()
}

async fn run_route(deps: &FakeDeps, c: &Value) -> Result<Value, ApiError> {
    use dashboard::*;
    let template = c["template"].as_str().unwrap();
    let path = path_params(template, c["path"].as_str().unwrap());
    let headers = {
        let mut h = axum::http::HeaderMap::new();
        h.insert("content-type", "application/json".parse().unwrap());
        h
    };
    let raw = match &c["body"] {
        Value::Null => Vec::new(),
        Value::String(s) => s.as_bytes().to_vec(),
        other => serde_json::to_vec(other).unwrap(),
    };
    let body = read_body(&headers, axum::body::Body::from(raw)).await?;
    let user = site_user(c["user_discord_id"].as_i64().filter(|i| *i != 0));
    let tail = template.trim_start_matches("/v1/site-auth/discord");
    match (c["method"].as_str().unwrap(), tail) {
        ("GET", "/guilds") => list_guilds(deps, &user).await,
        ("GET", "/announcement-meta") => Ok(contexts::announcement_meta(&super::announcements::keys())),
        ("GET", "/guilds/{guild_id}") => {
            let GuildPath(g) = validate_path(path, &headers)?;
            guild_detail(deps, &user, g).await
        }
        ("PUT", "/guilds/{guild_id}/announcements") => {
            let (GuildPath(g), b) = validate_parts::<GuildPath, AnnouncementsUpdate>(path, &headers, &body)?;
            set_announcements(deps, &user, g, b).await
        }
        ("PUT", "/guilds/{guild_id}/announcements/{key}/template") => {
            let (KeyPath(g, k), b) = validate_parts::<KeyPath, TemplateUpdate>(path, &headers, &body)?;
            set_template(deps, &user, g, &k, b.0).await
        }
        ("PUT", "/guilds/{guild_id}/language") => {
            let (GuildPath(g), b) = validate_parts::<GuildPath, LanguageUpdate>(path, &headers, &body)?;
            set_language(deps, &user, g, &b.0).await
        }
        ("PUT", "/guilds/{guild_id}/live-board") => {
            let (GuildPath(g), b) = validate_parts::<GuildPath, LiveBoardUpdate>(path, &headers, &body)?;
            set_live_board(deps, &user, g, b).await
        }
        ("PUT", "/guilds/{guild_id}/market-watch") => {
            let (GuildPath(g), b) = validate_parts::<GuildPath, MarketWatchUpdate>(path, &headers, &body)?;
            set_market_watch(deps, &user, g, b).await
        }
        ("GET", "/guilds/{guild_id}/clubs") => {
            let GuildPath(g) = validate_path(path, &headers)?;
            list_clubs(deps, &user, g).await
        }
        ("POST", "/guilds/{guild_id}/clubs") => {
            let GuildPath(g) = validate_path(path, &headers)?;
            create_club(deps, &user, g).await
        }
        ("PUT", "/guilds/{guild_id}/clubs/{club_id}") => {
            let (ClubPath(g, club), b) = validate_parts::<ClubPath, ClubUpdate>(path, &headers, &body)?;
            update_club(deps, &user, g, &club, b).await
        }
        ("DELETE", "/guilds/{guild_id}/clubs/{club_id}") => {
            let ClubPath(g, club) = validate_path(path, &headers)?;
            delete_club(deps, &user, g, &club).await
        }
        ("GET", "/guilds/{guild_id}/clubs/{club_id}/roster") => {
            let ClubPath(g, club) = validate_path(path, &headers)?;
            roster(deps, &user, g, &club).await
        }
        ("POST", "/guilds/{guild_id}/clubs/{club_id}/roster/{member_id}/{action}") => {
            let p: ActionPath = validate_path(path, &headers)?;
            roster_action(deps, &user, &p).await
        }
        ("PUT", "/guilds/{guild_id}/permissions") => {
            let (GuildPath(g), b) = validate_parts::<GuildPath, PermissionsUpdate>(path, &headers, &body)?;
            set_permissions(deps, &user, g, b).await
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn dashboard_routes_match_python() {
    let g = &golden()["dashboard"];
    let mut failures = Vec::new();
    for c in g["cases"].as_array().unwrap() {
        let world = g["worlds"][c["world"].as_u64().unwrap() as usize].clone();
        let deps = FakeDeps::new(world, &g["dbs"][c["db_before"].as_u64().unwrap() as usize]);
        let name = c["name"].as_str().unwrap();
        let got = run_route(&deps, c).await;
        let (status, body) = match got {
            Ok(v) => (200, json!({"bytes": pyjson::pydantic(&v)})),
            Err(e) => {
                let mut v: Value = serde_json::from_str(&e.body()).unwrap();
                v["error"]["request_id"] = Value::Null;
                (e.status.as_u16(), json!({"error_json": v}))
            }
        };
        if status != c["status"].as_u64().unwrap() as u16 {
            failures.push(format!("{name}: status {status} vs {}", c["status"]));
            continue;
        }
        if ext_v(&body) != ext_v(&c["response"]) {
            failures.push(format!("{name}: body\n  rust: {}\n  py:   {}", ext_v(&body), ext_v(&c["response"])));
            continue;
        }
        if ext_v(&deps.dump()) != ext_v(&c["db_after"]) {
            failures.push(format!("{name}: db\n  rust: {}\n  py:   {}", ext_v(&deps.dump()), ext_v(&c["db_after"])));
            continue;
        }
        if Value::Array(deps.sent.lock().unwrap().clone()) != c["sent"] {
            failures.push(format!("{name}: sent {:?} vs {}", deps.sent.lock().unwrap(), c["sent"]));
        }
        if Value::Array(deps.cleared.lock().unwrap().clone()) != c["cleared"] {
            failures.push(format!("{name}: cleared"));
        }
    }
    assert!(failures.is_empty(), "{} differ:\n{}", failures.len(), failures.join("\n"));
}

/// Guild ids past `i64` against the Python handlers' answers
/// (testdata/routing/golden.json, gen_routing_fixtures.py).
#[tokio::test]
async fn big_guild_ids_answer_like_python() {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/routing/golden.json");
    let golden: Value = serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap();
    for c in golden["discord_big"].as_array().unwrap() {
        let mut world = c["world"].clone();
        world["bad"] = json!([]);
        let deps = FakeDeps::new(world, &json!({}));
        let user = site_user(c["linked"].as_bool().unwrap().then_some(312345678901234567));
        let params: Vec<(String, String)> = c["params"]
            .as_array()
            .unwrap()
            .iter()
            .map(|kv| (kv[0].as_str().unwrap().to_owned(), kv[1].as_str().unwrap().to_owned()))
            .collect();
        let route = c["route"].as_str().unwrap();
        let err = dashboard::big_guild(&deps, &user, route, &params).await.expect("a big guild id");
        let want = &c["want"];
        assert_eq!(err.status.as_u16() as u64, want["status"].as_u64().unwrap(), "{route} {}", c["world"]);
        if want["status"] != 500 {
            assert_eq!(
                (err.code, err.message.as_str()),
                (want["code"].as_str().unwrap(), want["message"].as_str().unwrap())
            );
        }
    }
    let small = vec![("guild_id".to_owned(), "4242".to_owned())];
    let deps = FakeDeps::new(json!({"paths": {}, "errors": {}, "bad": []}), &json!({}));
    assert!(dashboard::big_guild(&deps, &site_user(None), "/guilds/{guild_id}", &small).await.is_none());
    use super::rest::Snowflake;
    assert_eq!(Snowflake::parse(" -0_09223372036854775809.00 "), Snowflake::Big("-9223372036854775809".into()));
    assert_eq!(Snowflake::parse("-9223372036854775808"), Snowflake::Id(i64::MIN));
    assert_eq!(Snowflake::parse("+0012"), Snowflake::Id(12));
}

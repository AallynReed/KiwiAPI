//! Golden tests against the Python bot (`native/port/parity/gen_bot_fixtures.py`):
//! the same scenarios run through a fake backend must make the same Discord
//! requests and leave the same documents.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicI64, AtomicU32, Ordering},
    },
};

use anyhow::Result;
use chrono::{DateTime, Utc};
use mongodb::bson::{Bson, Document, doc, oid::ObjectId};
use serde_json::{Value, json};

use super::{
    Backend, Bot, RestError, TrackedQuery, announcer::refresh_token, cache, market_watch::match_item, message, runner,
};
use crate::{
    discord_common::{
        contexts, embeds,
        i18n::{Catalogs, LANGS},
        sources::{self, Sources},
    },
    feeds::store::News,
};

pub(super) fn golden() -> &'static Value {
    static G: OnceLock<Value> = OnceLock::new();
    G.get_or_init(|| serde_json::from_str(include_str!("../../testdata/bot/golden.json")).expect("fixture parses"))
}

pub(super) fn bot_catalogs() -> &'static Catalogs {
    static C: OnceLock<Catalogs> = OnceLock::new();
    C.get_or_init(|| Catalogs::with_dirs("../../no-site-in-the-bot-container", "../../assets/locales"))
}

pub(super) fn ext_doc(v: &Value) -> Document {
    match Bson::try_from(v.clone()).expect("extended JSON") {
        Bson::Document(d) => d,
        other => panic!("not a document: {other}"),
    }
}

fn text(v: &Value) -> String {
    serde_json::to_string(v).unwrap()
}

fn num(b: Option<&Bson>) -> Option<f64> {
    match b? {
        Bson::Int32(i) => Some(f64::from(*i)),
        Bson::Int64(i) => Some(*i as f64),
        Bson::Double(f) => Some(*f),
        Bson::Boolean(b) => Some(f64::from(u8::from(*b))),
        _ => None,
    }
}

fn int(b: Option<&Bson>) -> Option<i64> {
    match b? {
        Bson::Int32(i) => Some(i64::from(*i)),
        Bson::Int64(i) => Some(*i),
        _ => None,
    }
}

fn outcome(v: Option<&Value>) -> Result<(), RestError> {
    match v {
        None => Ok(()),
        Some(Value::String(s)) if s == "transport" => Err(RestError::Transport("Connection refused".into())),
        Some(v) => match v.as_u64().unwrap_or(200) {
            200 => Ok(()),
            404 => Err(RestError::NotFound),
            403 => Err(RestError::Forbidden),
            code => Err(RestError::Http(code as u16)),
        },
    }
}

fn parse_dt(v: &Value) -> Option<DateTime<Utc>> {
    v.as_str().and_then(crate::dt::parse_utc)
}

pub(super) struct Fake {
    now_us: i64,
    src: Value,
    db: Mutex<HashMap<String, Vec<Document>>>,
    pub(super) next_id: AtomicU32,
    next_msg: AtomicI64,
    fetch: HashMap<i64, Value>,
    script: Mutex<HashMap<String, VecDeque<Value>>>,
    pub(super) requests: Mutex<Vec<Value>>,
}

impl Fake {
    fn new(src: &Value) -> Self {
        Fake {
            now_us: golden()["now_us"].as_i64().unwrap(),
            src: src.clone(),
            db: Mutex::new(HashMap::new()),
            next_id: AtomicU32::new(0),
            next_msg: AtomicI64::new(0),
            fetch: HashMap::new(),
            script: Mutex::new(HashMap::new()),
            requests: Mutex::new(Vec::new()),
        }
    }

    pub(super) fn scenario(s: &Value) -> Self {
        let mut f = Fake::new(&s["sources"]);
        let mut db = HashMap::new();
        for (coll, docs) in s["seed"].as_object().unwrap() {
            db.insert(coll.clone(), docs.as_array().unwrap().iter().map(ext_doc).collect());
        }
        f.db = Mutex::new(db);
        f.fetch = s["fetch"].as_object().unwrap().iter().map(|(k, v)| (k.parse().unwrap(), v.clone())).collect();
        let script = s["script"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.as_array().unwrap().iter().cloned().collect()))
            .collect();
        f.script = Mutex::new(script);
        f
    }

    fn record(&self, r: Value) {
        self.requests.lock().unwrap().push(r);
    }

    fn scripted(&self, key: &str) -> Result<(), RestError> {
        let next = self.script.lock().unwrap().get_mut(key).and_then(VecDeque::pop_front);
        outcome(next.as_ref())
    }

    fn coll(&self, name: &str) -> Vec<Document> {
        self.db.lock().unwrap().get(name).cloned().unwrap_or_default()
    }

    fn src(&self, k: &str) -> Value {
        self.src.get(k).cloned().unwrap_or(Value::Null)
    }

    pub(super) fn now_bson(&self) -> mongodb::bson::DateTime {
        mongodb::bson::DateTime::from_millis(self.now_us.div_euclid(1000))
    }
}

impl Sources for Fake {
    fn now_us(&self) -> i64 {
        self.now_us
    }
    async fn challenge(&self) -> Result<Value> {
        Ok(self.src("challenge"))
    }
    async fn chaos(&self) -> Result<Value> {
        Ok(self.src("chaos"))
    }
    async fn luxion(&self) -> Result<Value> {
        Ok(self.src("luxion"))
    }
    async fn status(&self) -> Value {
        self.src("status")
    }
    fn server_time(&self) -> Value {
        self.src("server_time")
    }
    fn biome_rotation(&self) -> Result<Value> {
        Ok(self.src("biome_rotation"))
    }
    fn wild_mana(&self) -> Result<Value> {
        Ok(self.src("wild_mana"))
    }
    fn stampy(&self) -> Result<Value> {
        Ok(self.src("stampy"))
    }
    fn corruxion(&self) -> Value {
        self.src("corruxion")
    }
    fn fluxion(&self) -> Value {
        self.src("fluxion")
    }
    fn daily_buff(&self) -> Value {
        self.src("daily_buff")
    }
    fn weekly_buff(&self) -> Value {
        self.src("weekly_buff")
    }
    async fn news(&self, limit: usize) -> Result<Vec<News>> {
        let rows = self.src("news");
        Ok(rows
            .as_array()
            .unwrap()
            .iter()
            .take(limit)
            .map(|n| News {
                url: n["url"].as_str().unwrap().into(),
                title: n["title"].as_str().unwrap().into(),
                author: "Team Trove".into(),
                summary: n.get("summary").and_then(Value::as_str).unwrap_or("").into(),
                category: "News".into(),
                categories: Vec::new(),
                image: n.get("image").and_then(Value::as_str).map(str::to_owned),
                published_at: n.get("published_at").and_then(parse_dt),
            })
            .collect())
    }
    async fn newest_open_giveaway(&self) -> Result<Option<DateTime<Utc>>> {
        Ok(parse_dt(&self.src("newest_open_giveaway")))
    }
    async fn giveaways(&self) -> Result<[Vec<Value>; 3]> {
        let g = self.src("giveaways");
        let list = |i: usize| g[i].as_array().unwrap().clone();
        Ok([list(0), list(1), list(2)])
    }
    async fn latest_update(&self) -> Result<Option<Value>> {
        Ok(Some(self.src("latest_update")).filter(|v| !v.is_null()))
    }
    async fn internal_get(&self, path: &str) -> Option<Value> {
        self.src.get("internal").and_then(|i| i.get(path)).cloned()
    }
}

impl Backend for Fake {
    async fn guild_configs(&self) -> Result<Vec<Document>> {
        Ok(self.coll("guild_configs"))
    }
    async fn guild_config(&self, guild_id: i64) -> Result<Option<Document>> {
        Ok(self.coll("guild_configs").into_iter().find(|d| int(d.get("guild_id")) == Some(guild_id)))
    }
    async fn save(&self, coll: &'static str, id: ObjectId, d: Document) -> Result<()> {
        let mut db = self.db.lock().unwrap();
        let docs = db.entry(coll.into()).or_default();
        match docs.iter_mut().find(|x| x.get_object_id("_id").ok() == Some(id)) {
            Some(existing) => {
                for (k, v) in d {
                    existing.insert(k, v);
                }
            }
            None => {
                let mut new = doc! {"_id": id};
                new.extend(d);
                docs.push(new);
            }
        }
        Ok(())
    }
    async fn insert(&self, coll: &'static str, fields: Document) -> Result<ObjectId> {
        let n = self.next_id.fetch_add(1, Ordering::SeqCst) + 1;
        let id = ObjectId::parse_str(format!("66000000000000000000{n:04x}")).unwrap();
        let mut d = doc! {"_id": id};
        d.extend(fields);
        self.db.lock().unwrap().entry(coll.into()).or_default().push(d);
        Ok(id)
    }
    async fn delete(&self, coll: &'static str, id: ObjectId) -> Result<()> {
        let mut db = self.db.lock().unwrap();
        if let Some(docs) = db.get_mut(coll)
            && let Some(i) = docs.iter().position(|x| x.get_object_id("_id").ok() == Some(id))
        {
            docs.remove(i);
        }
        Ok(())
    }
    async fn tracked(&self, q: TrackedQuery) -> Result<Vec<Document>> {
        Ok(self
            .coll("tracked_announcements")
            .into_iter()
            .filter(|d| match &q {
                TrackedQuery::GuildKind(g, k) => {
                    int(d.get("guild_id")) == Some(*g) && d.get_str("kind").ok() == Some(k.as_str())
                }
                TrackedQuery::Expired(now) => num(d.get("expires_at")).is_some_and(|e| e <= *now as f64),
                TrackedQuery::Refreshing => num(d.get("refresh")) == Some(1.0),
            })
            .collect())
    }
    async fn site_user_by_discord(&self, discord_id: i64) -> Result<Option<Document>> {
        Ok(self.coll("site_users").into_iter().find(|d| int(d.get("discord_id")) == Some(discord_id)))
    }
    async fn record_presence(&self, guilds: i64, members: i64) -> Result<()> {
        let mut db = self.db.lock().unwrap();
        let docs = db.entry("bot_presence".into()).or_default();
        let set = doc! {
            "guild_count": crate::outbound::int_bson(guilds),
            "member_count": crate::outbound::int_bson(members),
            "updated_at": self.now_bson(),
        };
        match docs.iter_mut().find(|d| d.get_str("_id").ok() == Some("current")) {
            Some(d) => d.extend(set),
            None => {
                let mut d = doc! {"_id": "current"};
                d.extend(set);
                docs.push(d);
            }
        }
        Ok(())
    }
    async fn fetch_channel(&self, id: i64) -> Result<(), RestError> {
        self.record(json!({"method": "GET", "path": format!("/channels/{id}")}));
        outcome(Some(self.fetch.get(&id).unwrap_or(&json!(404))))
    }
    async fn send_message(&self, channel: i64, payload: Value, png: Option<Vec<u8>>) -> Result<i64, RestError> {
        let path = format!("/channels/{channel}/messages");
        self.record(match &png {
            Some(p) => json!({"method": "POST", "path": path, "payload_json": payload, "files": [
                {"name": "files[0]", "filename": "embed-image.png", "content_type": "application/octet-stream",
                 "size": p.len()}]}),
            None => json!({"method": "POST", "path": path, "json": payload}),
        });
        self.scripted(&format!("POST {path}"))?;
        Ok(1_300_000_000_000_000_000 + self.next_msg.fetch_add(1, Ordering::SeqCst) + 1)
    }
    async fn edit_message(&self, channel: i64, message: i64, payload: Value) -> Result<(), RestError> {
        let path = format!("/channels/{channel}/messages/{message}");
        self.record(json!({"method": "PATCH", "path": path, "json": payload}));
        self.scripted(&format!("PATCH {path}"))
    }
    async fn delete_message(&self, channel: i64, message: i64) -> Result<(), RestError> {
        let path = format!("/channels/{channel}/messages/{message}");
        self.record(json!({"method": "DELETE", "path": path}));
        self.scripted(&format!("DELETE {path}"))
    }
    async fn render_design(&self, design_id: &str) -> Option<Vec<u8>> {
        self.src.get("designs")?.get(design_id)?.as_str().map(|s| s.as_bytes().to_vec())
    }
}

pub(super) fn make_bot<B: Backend>(b: B, s: &Value) -> Bot<B> {
    let cached: HashSet<i64> = s["cached"].as_array().unwrap().iter().map(|v| v.as_i64().unwrap()).collect();
    let cache = Arc::new(cache::Cache::default());
    cache.upsert_guild(1, cache::Guild { member_count: None, channels: cached, roles: HashSet::new() });
    Bot::new(b, cache, bot_catalogs(), golden()["asset"].as_str().unwrap().into())
}

/// Python's `AllowedMentions(roles=...)` payload, as the port sends it on
/// purpose: exactly the ping roles (see `message::allowed_mentions`).
fn intended(mut req: Value) -> Value {
    for key in ["json", "payload_json"] {
        if let Some(Value::Object(body)) = req.get_mut(key)
            && let Some(am) = body.get_mut("allowed_mentions")
            && let Some(roles) = am.get("roles").cloned()
        {
            *am = json!({"parse": [], "roles": roles});
        }
    }
    req
}

fn ids(v: &Value) -> HashSet<i64> {
    v.as_array().unwrap().iter().map(|x| x.as_i64().unwrap()).collect()
}

async fn act<B: Backend>(bot: &Bot<B>, a: &Value) -> Result<Value> {
    Ok(match a["do"].as_str().unwrap() {
        "announce_all" => bot.run_all_announcements().await.map(|_| Value::Null)?,
        "announce" => bot.run_announcement_type(a["key"].as_str().unwrap()).await.map(|_| Value::Null)?,
        "cleanup" => {
            bot.cleanup_tracked().await;
            Value::Null
        }
        "refresh" => bot.refresh_tracked().await.map(|_| Value::Null)?,
        "boards" => bot.refresh_boards().await.map(|_| Value::Null)?,
        "market" => bot.run_market_watch().await.map(|_| Value::Null)?,
        "event" => {
            runner::handle_event(bot, &a["payload"]).await;
            Value::Null
        }
        "reconcile" => {
            let gid = a["guild_id"].as_i64().unwrap();
            bot.reconcile_guild(gid, &ids(&a["channels"]), &ids(&a["roles"])).await?;
            Value::Null
        }
        "avatar" => Value::Bool(
            bot.sync_avatar(a["discord_id"].as_i64().unwrap(), a["avatar"].as_str().map(str::to_owned)).await?,
        ),
        "presence" => {
            bot.b.record_presence(a["guilds"].as_i64().unwrap(), a["members"].as_i64().unwrap()).await?;
            Value::Null
        }
        other => panic!("unknown action {other}"),
    })
}

/// Runs a scenario's actions and checks each outcome and every request.
pub(super) async fn replay<B: Backend>(bot: &Bot<B>, fake: &Fake, s: &Value) {
    let name = s["name"].as_str().unwrap();
    for (a, want) in s["actions"].as_array().unwrap().iter().zip(s["results"].as_array().unwrap()) {
        let got = match act(bot, a).await {
            Ok(v) => json!({"ok": v}),
            Err(_) => json!({"raised": want.get("raised").cloned().unwrap_or(json!("?"))}),
        };
        assert_eq!(text(&got), text(want), "{name}: result of {a}");
    }
    let got = fake.requests.lock().unwrap().clone();
    let want: Vec<Value> = s["requests"].as_array().unwrap().iter().cloned().map(intended).collect();
    for (i, (g, w)) in got.iter().zip(&want).enumerate() {
        assert_eq!(text(g), text(w), "{name}: request {i}");
    }
    assert_eq!(got.len(), want.len(), "{name}: request count");
}

pub(super) fn check_db(s: &Value, coll: &str, docs: Vec<Document>) {
    let name = s["name"].as_str().unwrap();
    let got: Vec<Value> = docs.into_iter().map(|d| Bson::Document(d).into_canonical_extjson()).collect();
    let want = s["db"][coll].as_array().unwrap();
    for (i, (g, w)) in got.iter().zip(want).enumerate() {
        assert_eq!(text(g), text(w), "{name}: {coll}[{i}]");
    }
    assert_eq!(got.len(), want.len(), "{name}: {coll} count");
}

#[tokio::test]
async fn scenarios_match_python() {
    for s in golden()["scenarios"].as_array().unwrap() {
        let bot = make_bot(Fake::scenario(s), s);
        replay(&bot, &bot.b, s).await;
        for coll in s["db"].as_object().unwrap().keys() {
            check_db(s, coll, bot.b.coll(coll));
        }
    }
}

async fn build(fake: &Fake, builder: &str, lang: &str) -> Result<embeds::Obj> {
    let tr = bot_catalogs().tr(Some(lang));
    let asset = golden()["asset"].as_str().unwrap();
    let now = fake.now_us.div_euclid(1_000_000);
    match builder {
        "status" => embeds::status_embed(&tr, &fake.status().await),
        "activity" => embeds::activity_embed(
            &tr,
            &sources::activity_current(fake).await,
            &sources::activity_series(fake).await,
            asset,
        ),
        "chaos" => embeds::chaos_embed(&tr, &fake.chaos().await?),
        "bonuses" => embeds::bonuses_embed(&tr, &fake.daily_buff(), &fake.weekly_buff()),
        "challenge" => embeds::challenge_embed(&tr, &fake.challenge().await?),
        "longshade" => embeds::longshade_embed(&tr, &fake.biome_rotation()?),
        "wild_mana" => embeds::wild_mana_embed(&tr, &fake.wild_mana()?),
        "stampy" => embeds::stampy_embed(&tr, &fake.stampy()?, now),
        "corruxion" => embeds::corruxion_embed(&tr, &fake.corruxion()),
        "fluxion" => embeds::fluxion_embed(&tr, &fake.fluxion()),
        "luxion" => embeds::luxion_embed(&tr, &fake.luxion().await?, now, asset),
        "luxion_caption" => embeds::luxion_caption(&tr, &fake.luxion().await?, now),
        "giveaways" => {
            let [a, b, c] = fake.giveaways().await?;
            embeds::giveaways_embed(&tr, &a, &b, &c)
        }
        "trove_news" => embeds::trove_news_embed(&tr, &fake.news(5).await?),
        "game_update" => embeds::game_update_embed(&tr, fake.latest_update().await?.as_ref(), asset),
        "live_board" => {
            let overall = fake.status().await.get("overall").cloned().unwrap_or(json!("unknown"));
            Ok(embeds::live_board_embed(&tr, &overall, fake.now_us, asset))
        }
        other => panic!("unknown builder {other}"),
    }
}

#[tokio::test]
async fn embeds_match_python() {
    let g = golden();
    for c in g["embeds"].as_array().unwrap() {
        let fake = Fake::new(&g["variants"][c["variant"].as_str().unwrap()]);
        let got = match build(&fake, c["builder"].as_str().unwrap(), c["lang"].as_str().unwrap()).await {
            Ok(e) => json!({"ok": e}),
            Err(_) => json!({"raised": c.get("raised")}),
        };
        let want = json!({"ok": c.get("ok"), "raised": c.get("raised")});
        let got = json!({"ok": got.get("ok"), "raised": got.get("raised")});
        assert_eq!(text(&got), text(&want), "{} {} {}", c["builder"], c["variant"], c["lang"]);
    }
    for c in g["market_embeds"].as_array().unwrap() {
        let tr = bot_catalogs().tr(c["lang"].as_str());
        let got = embeds::market_watch_embed(&tr, c["name"].as_str().unwrap(), &c["summary"], c["max"].as_f64());
        assert_eq!(text(&Value::Object(got)), text(&c["ok"]), "market {} {}", c["name"], c["lang"]);
    }
}

#[tokio::test]
async fn contexts_match_python() {
    let g = golden();
    let asset = g["asset"].as_str().unwrap();
    for c in g["contexts"].as_array().unwrap() {
        let fake = Fake::new(&g["variants"][c["variant"].as_str().unwrap()]);
        let tr = bot_catalogs().tr(c["lang"].as_str());
        let key = c["key"].as_str().unwrap();
        match contexts::context(&fake, &tr, key, asset).await {
            Ok(ctx) => assert_eq!(text(&Value::Object(ctx)), text(&c["ok"]), "{key} {} {}", c["variant"], c["lang"]),
            Err(e) => assert!(c.get("raised").is_some(), "{key} {} {}: {e:#}", c["variant"], c["lang"]),
        }
    }
}

#[test]
fn tokens_and_market_signatures() {
    let g = golden();
    for c in g["tokens"].as_array().unwrap() {
        assert_eq!(refresh_token(c["expires"].as_i64(), c["now"].as_i64().unwrap()), c["token"], "{c}");
    }
    for c in g["match_item"].as_array().unwrap() {
        let summary = Some(&c["summary"]).filter(|s| !s.is_null());
        let got = match_item(summary, c["max"].as_f64()).unwrap();
        assert_eq!(got.map_or(Value::Null, Value::String), c["sig"], "{c}");
    }
}

#[test]
fn embed_round_trip() {
    for c in golden()["embed_to_dict"].as_array().unwrap() {
        let got = message::embed_to_dict(c["in"].as_object().unwrap());
        assert_eq!(text(&Value::Object(got)), text(&c["out"]), "{}", c["in"]);
    }
}

#[test]
fn translations() {
    let site = Catalogs::with_dirs("../../site/static/locales", "../../assets/locales");
    for (cases, cats) in [("t_bot", bot_catalogs()), ("t_site", &site)] {
        for c in golden()[cases].as_array().unwrap() {
            let tr = cats.tr(c["lang"].as_str());
            let fmt: Vec<(String, String)> = c["fmt"]
                .as_object()
                .unwrap()
                .iter()
                .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_owned()))
                .collect();
            let args: Vec<(&str, &str)> = fmt.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
            assert_eq!(tr.tf(c["text"].as_str().unwrap(), &args), c["out"].as_str().unwrap(), "{cases}: {c}");
        }
    }
    assert_eq!(LANGS.len(), 10);
}

#[test]
fn python_format_rules() {
    use crate::discord_common::pyfmt::format;
    let a = [("when", "soon"), ("n", "3")];
    assert_eq!(format("Ends {when} ({n})", &a).unwrap(), "Ends soon (3)");
    assert_eq!(format("{{lit}} {when!s} {when!r}", &a).unwrap(), "{lit} soon 'soon'");
    assert!(format("{missing}", &a).is_err());
    assert!(format("{0}", &a).is_err());
    assert!(format("{}", &a).is_err());
    assert!(format("oops }", &a).is_err());
    assert!(format("oops {when", &a).is_err());
    assert!(format("{when:>9}", &a).is_err());
}

#[test]
fn minute_alignment() {
    assert_eq!(runner::until_next_minute(60_000_000).as_secs(), 60);
    assert_eq!(runner::until_next_minute(61_500_000).as_micros(), 58_500_000);
}

#[test]
fn settings_carry_the_bot_fields() {
    let s = crate::config::Settings::from_vars([("ASSET_BASE_URL", "https://api.aallyn.net/")]).unwrap();
    assert_eq!(s.asset_url(), "https://api.aallyn.net");
    assert!(s.bot_members_intent);
    assert_eq!(s.events_channel, "kiwi:events");
}

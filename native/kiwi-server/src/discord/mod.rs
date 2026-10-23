//! Discord: the signed interactions endpoint (`app/discord/router.py`), the
//! slash commands and their embeds, the announcement customization contexts,
//! and the site user's bot dashboard (`app/bot/router.py`) with its REST
//! helper, models and reconcile.

pub mod announcements;
pub mod commands;
pub mod contexts;
pub mod dashboard;
pub mod embeds;
pub mod models;
pub mod rest;

#[cfg(test)]
pub(crate) mod tests;

use std::time::Instant;

use axum::{
    Router,
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
};
use futures::TryStreamExt;
use kiwi_core::gems::{GemError, evaluator::StatInput};
use mongodb::bson::{Bson, Document, doc};
use serde_json::{Value, json};

use self::{commands::Ping, embeds::PlatformRelease};
use crate::{
    error::{ApiError, method_not_allowed},
    feeds::store::News,
    giveaways::{service as giveaways, store::Mongo as GiveawayStore},
    og::data,
    pyjson::{self, LoadsError, PyJson},
    request_id::latin1,
    routes::rotations::{bodies, gamedata::Gamedata},
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/discord/interactions", post(interactions).fallback(|| async { method_not_allowed("POST") }))
        .merge(dashboard::router())
}

/// `bytes.fromhex`: hex pairs, ASCII whitespace allowed between them.
pub fn fromhex(s: &str) -> Option<Vec<u8>> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len() / 2);
    let mut i = 0;
    let digit = |c: u8| (c as char).to_digit(16).map(|d| d as u8);
    while i < b.len() {
        if matches!(b[i], b' ' | b'\t' | b'\n' | b'\r' | b'\x0b' | b'\x0c') {
            i += 1;
            continue;
        }
        let hi = digit(b[i])?;
        let lo = digit(*b.get(i + 1)?)?;
        out.push(hi << 4 | lo);
        i += 2;
    }
    Some(out)
}

/// `_verify`: an Ed25519 signature of `timestamp + body` under the app key.
pub fn verify(public_key: Option<&str>, signature_hex: &str, timestamp: &str, body: &[u8]) -> bool {
    let Some(key) = public_key.filter(|k| !k.is_empty()) else { return false };
    let (Some(key), Some(sig)) = (fromhex(key), fromhex(signature_hex)) else { return false };
    if key.len() != 32 {
        return false;
    }
    let mut msg = timestamp.as_bytes().to_vec();
    msg.extend_from_slice(body);
    ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, &key).verify(&msg, &sig).is_ok()
}

fn header(headers: &HeaderMap, name: &str) -> Option<String> {
    headers.get(name).map(|v| latin1(v.as_bytes())).filter(|v| !v.is_empty())
}

/// `POST /discord/interactions`.
async fn interactions(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    let sig = header(&headers, "x-signature-ed25519");
    let ts = header(&headers, "x-signature-timestamp");
    let ok = match (&sig, &ts) {
        (Some(s), Some(t)) => verify(state.settings().discord_public_key.as_deref(), s, t, &body),
        _ => false,
    };
    if !ok {
        return (StatusCode::UNAUTHORIZED, PyJson(json!({"error": "invalid request signature"}))).into_response();
    }
    let raw: &[u8] = if body.is_empty() { b"{}" } else { &body };
    let data = match pyjson::loads(raw) {
        Ok(Value::Object(m)) => Value::Object(m),
        Ok(_) | Err(LoadsError::NonFinite) => return ApiError::internal().into_response(),
        Err(LoadsError::Syntax { .. }) => {
            return (StatusCode::BAD_REQUEST, PyJson(json!({"error": "invalid payload"}))).into_response();
        }
        Err(_) => return ApiError::internal().into_response(),
    };
    if commands::py_eq_int(data.get("type"), 1) {
        return PyJson(json!({"type": 1})).into_response();
    }
    match commands::handle(&LiveDomain(&state), &data).await {
        Ok(v) => PyJson(v).into_response(),
        Err(()) => ApiError::internal().into_response(),
    }
}

/// The domain reads over the real stores.
pub struct LiveDomain<'a>(pub &'a AppState);

impl LiveDomain<'_> {
    fn now(&self) -> i64 {
        crate::dt::now().timestamp_micros()
    }

    fn giveaway_store(&self) -> GiveawayStore<'_> {
        GiveawayStore { db: self.0.mongo(), email_enabled: false }
    }
}

fn bson_value(d: Document) -> Value {
    Bson::Document(d).into_relaxed_extjson()
}

fn btt_platforms(docs: &[Document]) -> Vec<(&'static str, PlatformRelease)> {
    let mut out = Vec::new();
    for p in kiwi_core::btt::PLATFORMS {
        let mut found: PlatformRelease = None;
        for d in docs {
            let assets: Vec<&Document> = match d.get("assets") {
                Some(Bson::Array(a)) => a.iter().filter_map(Bson::as_document).collect(),
                _ => Vec::new(),
            };
            let names: Vec<&str> = assets.iter().map(|a| a.get_str("name").unwrap_or("")).collect();
            let order = kiwi_core::btt::platform_order(&names, p);
            if let Some(first) = order.first() {
                let val = |k: &str| d.get(k).cloned().map_or(Value::Null, |b| b.into_relaxed_extjson());
                let url = assets[*first].get("url").cloned().map(|b| b.into_relaxed_extjson());
                found = Some((val("tag_name"), val("html_url"), url));
                break;
            }
        }
        out.push((p, found));
    }
    out
}

impl commands::Domain for LiveDomain<'_> {
    fn now_us(&self) -> i64 {
        self.now()
    }

    fn asset_url(&self) -> String {
        self.0.settings().asset_url()
    }

    async fn guild_language(&self, guild_id: &Value, locale: Option<&str>) -> &'static str {
        crate::i18n::guild_language(self.0, guild_id, locale).await
    }

    async fn record_command(&self, name: &str) {
        let coll = self.0.mongo().collection::<Document>("bot_command_usage");
        let now = crate::dt::to_bson(crate::dt::now());
        let r = coll
            .update_one(doc! {"_id": name}, doc! {"$inc": {"count": 1}, "$set": {"last_used_at": now}})
            .upsert(true)
            .await;
        if let Err(e) = r {
            tracing::warn!("stats: command write failed: {e}");
        }
    }

    async fn status(&self) -> Value {
        data::status(self.0).await
    }

    async fn activity(&self, period: &str) -> anyhow::Result<(Value, Value)> {
        let live = crate::activity::player::estimate_active_players(self.0, false).await?;
        let series = crate::activity::player::series(self.0, period).await?;
        Ok((live, series))
    }

    async fn chaos(&self) -> anyhow::Result<Value> {
        data::chaos_chest(self.0, self.now()).await
    }

    fn server_time(&self) -> Value {
        let st = kiwi_core::rotations::server_time(self.now());
        json!({"now_unix": st.now_unix, "daily_reset_at": st.daily_reset_at, "weekly_reset_at": st.weekly_reset_at})
    }

    fn daily_buffs(&self) -> Value {
        data::daily_buffs(self.now())
    }

    fn weekly_buffs(&self) -> Value {
        let idx = kiwi_core::rotations::weekly_buff_index(self.now());
        json!({"current": Gamedata::get().weekly.get(&idx.to_string()).cloned().unwrap_or_else(|| json!({}))})
    }

    async fn challenge(&self) -> anyhow::Result<Value> {
        data::challenge(self.0, self.now()).await
    }

    fn biome_rotation(&self) -> anyhow::Result<Value> {
        data::biome_rotation(self.now())
    }

    fn wild_mana(&self) -> anyhow::Result<Value> {
        data::wild_mana(self.now())
    }

    fn stampy(&self) -> anyhow::Result<Value> {
        data::stampy(self.now())
    }

    fn corruxion(&self) -> Value {
        bodies::corruxion(self.now())
    }

    fn fluxion(&self) -> Value {
        bodies::fluxion(self.now())
    }

    async fn giveaways(&self) -> anyhow::Result<(Vec<Value>, Vec<Value>, Vec<Value>)> {
        let s = self.giveaway_store();
        let err = |e: ApiError| anyhow::anyhow!("{}", e.body());
        let o = giveaways::list_ongoing(&s).await.map_err(err)?;
        let u = giveaways::list_upcoming(&s).await.map_err(err)?;
        let e = giveaways::list_ended(&s, 7).await.map_err(err)?;
        Ok((o, u, e))
    }

    async fn news(&self, limit: usize) -> Vec<News> {
        let docs = crate::feeds::store::news_page(self.0.mongo(), None, (limit * 3) as i64).await;
        crate::feeds::routes::dedupe_news(docs, limit)
    }

    async fn item_summary(&self, name: &str) -> anyhow::Result<Option<Value>> {
        if !self.0.settings().postgres_enabled() {
            anyhow::bail!("market summary needs Postgres");
        }
        let now = crate::dt::now().timestamp();
        crate::market::store::item_summary(
            &crate::market::store::Live(self.0),
            name,
            now - kiwi_core::market::LISTING_STALE_SECONDS,
            now - kiwi_core::market::LISTING_LIFETIME_SECONDS,
        )
        .await
        .map_err(|e| anyhow::anyhow!("{}", e.body()))
    }

    async fn history(&self, env: &str, days: i64) -> anyhow::Result<Value> {
        let env = kiwi_core::status::history_env(env);
        let rows = crate::routes::trove_status::load_segments(self.0, env).await?;
        let now = crate::routes::trove_status::unix_now();
        Ok(crate::routes::trove_status::history_value(&kiwi_core::status::history(env, days, now, &rows)))
    }

    fn evaluate_gem(&self, tier: i64, gem_type: i64, level: i64, stats: &[StatInput]) -> Result<Value, GemError> {
        crate::gems::evaluate_json(
            crate::gamedata::Gamedata::global(self.0.settings()),
            tier,
            gem_type,
            level,
            stats,
            true,
        )
    }

    async fn btt_latest(&self) -> anyhow::Result<Vec<(&'static str, PlatformRelease)>> {
        let coll = self.0.mongo().collection::<Document>("btt_releases");
        let docs: Vec<Document> =
            coll.find(doc! {"prerelease": false}).sort(doc! {"published_at": -1}).await?.try_collect().await?;
        Ok(btt_platforms(&docs))
    }

    async fn changelog(&self) -> anyhow::Result<Option<Value>> {
        let coll = self.0.mongo().collection::<Document>("btt_changelogs");
        let repo = &self.0.settings().btt_releases_repo;
        Ok(coll.find_one(doc! {"repo": repo}).await?.map(bson_value))
    }

    async fn ping(&self) -> (Ping, Option<Ping>) {
        let t0 = Instant::now();
        let mongo = match self.0.mongo().run_command(doc! {"ping": 1}).await {
            Ok(_) => Ok(t0.elapsed().as_secs_f64() * 1000.0),
            Err(_) => Err(()),
        };
        let redis = match self.0.inner.db.redis.clone() {
            None => None,
            Some(mut conn) => {
                let t0 = Instant::now();
                Some(match redis::cmd("PING").query_async::<String>(&mut conn).await {
                    Ok(_) => Ok(t0.elapsed().as_secs_f64() * 1000.0),
                    Err(_) => Err(()),
                })
            }
        };
        (mongo, redis)
    }

    async fn luxion(&self) -> anyhow::Result<Value> {
        data::luxion(self.0, self.now()).await
    }

    async fn latest_update(&self) -> anyhow::Result<Option<Value>> {
        announcements::latest_live_us(self.0).await
    }
}

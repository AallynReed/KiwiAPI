//! `app/discord/commands.py`: dispatch of an APPLICATION_COMMAND interaction.
//! Domain reads go through [`Domain`] so tests answer from fixtures.

use kiwi_core::gems::{GemError, evaluator::StatInput};
use serde_json::{Value, json};

use super::embeds::{self, PlatformRelease};
use crate::{
    feeds::{pyval, store::News},
    i18n::Lang,
};

const TYPE_APPLICATION_COMMAND: i64 = 2;
const RESP_MESSAGE: i64 = 4;
const FLAG_EPHEMERAL: i64 = 64;

/// A store's ping round-trip in ms, `Err` when unreachable.
pub type Ping = Result<f64, ()>;

/// The domain reads the commands make.
pub trait Domain: Sync {
    fn now_us(&self) -> i64;
    fn asset_url(&self) -> String;
    fn guild_language(&self, guild_id: &Value, locale: Option<&str>) -> impl Future<Output = &'static str> + Send;
    fn record_command(&self, name: &str) -> impl Future<Output = ()> + Send;
    fn status(&self) -> impl Future<Output = Value> + Send;
    fn activity(&self, period: &str) -> impl Future<Output = anyhow::Result<(Value, Value)>> + Send;
    fn chaos(&self) -> impl Future<Output = anyhow::Result<Value>> + Send;
    fn server_time(&self) -> Value;
    fn daily_buffs(&self) -> Value;
    fn weekly_buffs(&self) -> Value;
    fn challenge(&self) -> impl Future<Output = anyhow::Result<Value>> + Send;
    fn biome_rotation(&self) -> anyhow::Result<Value>;
    fn wild_mana(&self) -> anyhow::Result<Value>;
    fn stampy(&self) -> anyhow::Result<Value>;
    fn corruxion(&self) -> Value;
    fn fluxion(&self) -> Value;
    fn giveaways(&self) -> impl Future<Output = anyhow::Result<(Vec<Value>, Vec<Value>, Vec<Value>)>> + Send;
    fn news(&self, limit: usize) -> impl Future<Output = Vec<News>> + Send;
    fn item_summary(&self, name: &str) -> impl Future<Output = anyhow::Result<Option<Value>>> + Send;
    fn history(&self, env: &str, days: i64) -> impl Future<Output = anyhow::Result<Value>> + Send;
    fn evaluate_gem(&self, tier: i64, gem_type: i64, level: i64, stats: &[StatInput]) -> Result<Value, GemError>;
    fn btt_latest(&self) -> impl Future<Output = anyhow::Result<Vec<(&'static str, PlatformRelease)>>> + Send;
    fn changelog(&self) -> impl Future<Output = anyhow::Result<Option<Value>>> + Send;
    fn ping(&self) -> impl Future<Output = (Ping, Option<Ping>)> + Send;
    /// `luxion.get_luxion()`.
    fn luxion(&self) -> impl Future<Output = anyhow::Result<Value>> + Send;
    /// `updates.read.latest_version("live-us")`.
    fn latest_update(&self) -> impl Future<Output = anyhow::Result<Option<Value>>> + Send;
}

fn embed(e: Value, ephemeral: bool) -> Value {
    let mut data = json!({"embeds": [e]});
    if ephemeral {
        data["flags"] = json!(FLAG_EPHEMERAL);
    }
    json!({"type": RESP_MESSAGE, "data": data})
}

fn message(content: String) -> Value {
    json!({"type": RESP_MESSAGE, "data": {"content": content}})
}

/// `v == n` with Python's numeric equality (`True == 1`, `2.0 == 2`).
pub fn py_eq_int(v: Option<&Value>, n: i64) -> bool {
    match v {
        Some(Value::Number(x)) => x.as_f64() == Some(n as f64),
        Some(Value::Bool(b)) => i64::from(*b) == n,
        _ => false,
    }
}

/// `(d.get(k) or {})` that must be a dict; `Err` is the `AttributeError`.
fn dict_or_empty(v: Option<&Value>) -> Result<Option<&Value>, ()> {
    match v {
        Some(x) if pyval::truthy(x) => {
            if x.is_object() {
                Ok(Some(x))
            } else {
                Err(())
            }
        }
        _ => Ok(None),
    }
}

/// `_option`.
fn option<'a>(i: &'a Value, name: &str) -> Option<&'a Value> {
    let data = i.get("data").filter(|d| pyval::truthy(d))?;
    let opts = data.get("options").filter(|o| pyval::truthy(o))?;
    opts.as_array()?
        .iter()
        .find(|o| o.get("name").and_then(Value::as_str) == Some(name))
        .map(|o| o.get("value").unwrap_or(&Value::Null))
}

fn name_str(v: &Value) -> String {
    pyval::py_str(v)
}

/// The value `int(v)` gives, or `None` (an exception: the snag message).
fn opt_int(v: Option<&Value>) -> Option<i64> {
    pyval::py_int(v?).and_then(|i| i64::try_from(i).ok())
}

fn opt_float(v: Option<&Value>) -> Option<f64> {
    match v? {
        Value::Number(n) => n.as_f64(),
        Value::Bool(b) => Some(f64::from(u8::from(*b))),
        Value::String(s) => kiwi_core::pynum::float(s),
        _ => None,
    }
}

const GEM_TIERS: [&str; 4] = ["Radiant", "Stellar", "Crystal", "Mystic"];
const GEM_TYPES: [&str; 2] = ["Lesser", "Empowered"];

async fn gem<D: Domain>(d: &D, l: Lang, i: &Value) -> anyhow::Result<Value> {
    let err = || anyhow::anyhow!("TypeError");
    let tier = opt_int(option(i, "tier")).ok_or_else(err)?;
    if let Err(e) = kiwi_core::gems::catalog::tier(tier) {
        return Ok(embeds::gem_error_embed(l, e.message()));
    }
    let gem_type = opt_int(option(i, "type")).ok_or_else(err)?;
    if let Err(e) = kiwi_core::gems::catalog::gem_type(gem_type) {
        return Ok(embeds::gem_error_embed(l, e.message()));
    }
    let level = opt_int(option(i, "level")).ok_or_else(err)?;
    let mut stats = Vec::new();
    let mut values_missing = false;
    for n in 1..=3 {
        let value = opt_float(option(i, &format!("value{n}")));
        values_missing |= value.is_none();
        stats.push(StatInput {
            stat: opt_int(option(i, &format!("stat{n}"))).unwrap_or(i64::MIN),
            value: value.unwrap_or(0.0),
            extra_containers: 0,
        });
    }
    let sentinel = format!("{} is not a valid GemStatType", i64::MIN);
    match d.evaluate_gem(tier, gem_type, level, &stats) {
        Err(GemError::Evaluator(m) | GemError::Value(m)) if m == sentinel => Err(err()),
        Err(GemError::Evaluator(m) | GemError::Value(m)) => Ok(embeds::gem_error_embed(l, &m)),
        Err(e) => Err(anyhow::anyhow!("{}", e.message())),
        Ok(_) if values_missing => Err(err()),
        Ok(r) => {
            let tier_name = GEM_TIERS[(tier - 1) as usize];
            let type_name = GEM_TYPES[(gem_type - 1) as usize];
            embeds::gem_embed(l, tier_name, type_name, level, &r)
        }
    }
}

async fn dispatch<D: Domain>(d: &D, l: Lang, name: &str, i: &Value) -> anyhow::Result<Option<Value>> {
    let now = d.now_us().div_euclid(1_000_000);
    Ok(Some(match name {
        "status" => embed(embeds::status_embed(l, &d.status().await)?, false),
        "activity" => {
            let period = option(i, "period").cloned().unwrap_or_else(|| json!("7d"));
            let period = match period.as_str() {
                Some(p) if embeds::activity_label(p).is_some() => p.to_owned(),
                _ => "7d".to_owned(),
            };
            let (live, series) = d.activity(&period).await?;
            embed(embeds::activity_embed(l, &d.asset_url(), &period, &live, &series)?, false)
        }
        "chaos" => embed(embeds::chaos_embed(l, &d.chaos().await?)?, false),
        "servertime" => embed(embeds::servertime_embed(l, &d.server_time(), &embeds::trove_clock(d.now_us()))?, false),
        "bonuses" => embed(embeds::bonuses_embed(l, &d.daily_buffs(), &d.weekly_buffs())?, false),
        "hourly_challenge" => embed(embeds::challenge_embed(l, &d.challenge().await?)?, false),
        "longshade" => embed(embeds::longshade_embed(l, &d.biome_rotation()?)?, false),
        "giveaways" => {
            let (o, u, e) = d.giveaways().await?;
            embed(embeds::giveaways_embed(l, &o, &u, &e)?, false)
        }
        "corruxion" => embed(embeds::corruxion_embed(l, &d.corruxion())?, false),
        "fluxion" => embed(embeds::fluxion_embed(l, &d.fluxion())?, false),
        "stampy" => embed(embeds::stampy_embed(l, &d.stampy()?, now)?, false),
        "wild_mana" => embed(embeds::wild_mana_embed(l, &d.wild_mana()?)?, false),
        "trove_news" => embed(embeds::trove_news_embed(l, &d.news(5).await), false),
        "price" => {
            let item = match option(i, "item") {
                None | Some(Value::Null) => String::new(),
                Some(Value::String(s)) => kiwi_core::pystr::strip(s).to_owned(),
                Some(v) if !pyval::truthy(v) => String::new(),
                Some(_) => anyhow::bail!("AttributeError: strip"),
            };
            if item.is_empty() {
                message(l.t("🥝 Please provide an item name."))
            } else {
                let summary = d.item_summary(&item).await?;
                embed(embeds::price_embed(l, &item, summary.as_ref())?, false)
            }
        }
        "trove_uptime" => {
            let mut hists = Vec::new();
            for (env, _) in embeds::REGIONS {
                hists.push(d.history(env, 7).await?);
            }
            embed(embeds::uptime_embed(l, &hists, 7), false)
        }
        "gem" => embed(gem(d, l, i).await?, false),
        "download" => embed(embeds::download_embed(l, &d.btt_latest().await?), false),
        "web" => embed(embeds::web_embed(l), false),
        "change_log" => embed(embeds::changelog_embed(l, d.changelog().await?.as_ref())?, true),
        "ping" => {
            let (mongo, redis) = d.ping().await;
            embed(embeds::ping_embed(l, mongo, redis), false)
        }
        _ => return Ok(None),
    }))
}

/// `commands.handle`: the response for a non-PING interaction. `Err` is an
/// exception outside the command guard (a 500).
pub async fn handle<D: Domain>(d: &D, i: &Value) -> Result<Value, ()> {
    if !py_eq_int(i.get("type"), TYPE_APPLICATION_COMMAND) {
        return Ok(message(Lang::default().t("🥝 Unsupported interaction.")));
    }
    let locale = [i.get("guild_locale"), i.get("locale")]
        .into_iter()
        .flatten()
        .find(|v| pyval::truthy(v))
        .and_then(Value::as_str);
    let l = Lang(d.guild_language(i.get("guild_id").unwrap_or(&Value::Null), locale).await);
    let data = dict_or_empty(i.get("data"))?;
    let name = data.and_then(|d| d.get("name")).cloned().unwrap_or(Value::Null);
    if let Some(n) = name.as_str().filter(|n| !n.is_empty()) {
        d.record_command(n).await;
    }
    let key = name.as_str().unwrap_or("\u{0}");
    match dispatch(d, l, key, i).await {
        Ok(Some(v)) => Ok(v),
        Ok(None) => Ok(message(l.f("🥝 Unknown command: {name}", &[("name", &name_str(&name))]))),
        Err(e) => {
            tracing::error!("discord command /{} failed: {e:#}", name_str(&name));
            Ok(message(l.f("🥝 Sorry, `/{name}` hit a snag. Try again in a moment.", &[("name", &name_str(&name))])))
        }
    }
}

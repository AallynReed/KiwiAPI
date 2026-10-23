//! `app/bot/market_watch.py`: alert a guild when a watched item is listed
//! at or under its target, once per new cheapest price.

use anyhow::Result;
use serde_json::Value;

use super::{Backend, Bot, announcer::Target};
use crate::{
    discord_common::{
        embeds::{get, market_watch_embed},
        i18n::normalize_lang,
        models::GuildConfig,
        pyfmt::py_float,
        sources,
    },
    features::py_truthy,
};

/// `match_item`: the alert signature (`str(round(cheapest))`, ties to even)
/// when the item qualifies.
pub fn match_item(summary: Option<&Value>, max_price_each: Option<f64>) -> Result<Option<String>> {
    let Some(summary) = summary.filter(|s| py_truthy(s)) else { return Ok(None) };
    if !summary.is_object() {
        anyhow::bail!("AttributeError: summary is not a dict");
    }
    if !py_truthy(get(summary, "count")) {
        return Ok(None);
    }
    let cheapest = get(summary, "min_each");
    if cheapest.is_null() {
        return Ok(None);
    }
    let c = py_float(cheapest).ok_or_else(|| anyhow::anyhow!("float() of {cheapest}"))?;
    if max_price_each.is_some_and(|m| c > m) {
        return Ok(None);
    }
    if !c.is_finite() {
        anyhow::bail!("round() of {c}");
    }
    Ok(Some((c.round_ties_even() as i128).to_string()))
}

impl<B: Backend> Bot<B> {
    pub async fn run_market_watch(&self) -> Result<()> {
        let _g = self.market_lock.lock().await;
        let now = self.now_bson();
        let mut configs = Vec::new();
        for d in self.b.guild_configs().await? {
            configs.push(GuildConfig::from_doc(&d, now)?);
        }
        for cfg in &mut configs {
            let mw = &cfg.market_watch;
            if !(mw.enabled && mw.channel_id.is_some_and(|c| c != 0) && !mw.channel_missing && !mw.items.is_empty()) {
                continue;
            }
            let lang = normalize_lang(Some(cfg.language.as_str()));
            let mut dirty = false;
            for idx in 0..cfg.market_watch.items.len() {
                let item = cfg.market_watch.items[idx].clone();
                let summary = sources::market_summary(&self.b, &item.name).await;
                let Some(sig) = match_item(summary.as_ref(), item.max_price_each)? else {
                    if item.last_alert_sig.is_some() {
                        cfg.market_watch.items[idx].last_alert_sig = None;
                        dirty = true;
                    }
                    continue;
                };
                if item.last_alert_sig.as_deref() == Some(sig.as_str()) {
                    continue;
                }
                let summary = summary.unwrap_or_default();
                let embed = market_watch_embed(&self.i18n.tr(Some(lang)), &item.name, &summary, item.max_price_each);
                let mw = &cfg.market_watch;
                let target = Target { channel_id: mw.channel_id, ping_role_ids: &mw.ping_role_ids };
                if self.post(cfg.guild_id, target, &embed, "market_watch", None, None).await?.is_some() {
                    cfg.market_watch.items[idx].last_alert_sig = Some(sig);
                    dirty = true;
                }
            }
            if dirty {
                cfg.updated_at = self.now_bson();
                self.save_config(cfg).await?;
            }
        }
        Ok(())
    }
}

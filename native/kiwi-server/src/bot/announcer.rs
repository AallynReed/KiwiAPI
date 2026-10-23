//! `app/bot/announcer.py`: post each enabled type once per new anchor to
//! every guild that wants it, track the auto-managed posts, delete them when
//! they end and re-edit their banners when the shown countdown changes.

use anyhow::{Context, Result};
use indexmap::IndexSet;
use serde_json::{Map, Value, json};

use super::{
    Backend, Bot, RestError, TrackedQuery,
    announcements::{self, AnnouncementType},
    message,
};
use crate::{
    discord_common::{
        contexts,
        embeds::Obj,
        i18n::normalize_lang,
        models::{AnnouncementSetting, GUILD_CONFIGS, GuildConfig, TRACKED, Tracked},
    },
    embed_templates::{EmbedTemplate, render_template},
};

/// `countdown_bucket(target, now)` as the `?v` token: `m<min>` under an hour,
/// `h<hours>` under a day, `d<days>`, `now0`, `none0`.
pub fn refresh_token(expires_at: Option<i64>, now: i64) -> String {
    let Some(target) = expires_at.filter(|t| *t != 0) else { return "none0".into() };
    let rem = target - now;
    if rem <= 0 {
        "now0".into()
    } else if rem < 3600 {
        format!("m{}", rem / 60)
    } else if rem < 86_400 {
        format!("h{}", rem / 3600)
    } else {
        format!("d{}", rem / 86_400)
    }
}

/// `_image_embed_dict`: the bare banner embed (`&lang=` only off English).
pub fn image_embed(asset: &str, kind: &str, token: &str, lang: &str) -> Obj {
    let suffix = if lang == "en" { String::new() } else { format!("&lang={lang}") };
    let mut m = Map::new();
    m.insert("image".into(), json!({"url": format!("{asset}/announce.png?kind={kind}&v={token}{suffix}")}));
    m
}

/// What `_post` needs from an `AnnouncementSetting` or `MarketWatch`.
pub struct Target<'a> {
    pub channel_id: Option<i64>,
    pub ping_role_ids: &'a [i64],
}

impl<'a> From<&'a AnnouncementSetting> for Target<'a> {
    fn from(s: &'a AnnouncementSetting) -> Self {
        Target { channel_id: s.channel_id, ping_role_ids: &s.ping_role_ids }
    }
}

fn py_expiry(v: Option<Value>) -> Result<Option<i64>> {
    match v {
        None | Some(Value::Null) => Ok(None),
        Some(v) => match (v.as_i64(), v.as_f64()) {
            (Some(i), _) => Ok(Some(i)),
            (None, Some(f)) if f.fract() == 0.0 => Ok(Some(f as i64)),
            _ => anyhow::bail!("expires_at: not an int: {v}"),
        },
    }
}

impl<B: Backend> Bot<B> {
    /// `bot.get_channel(id)`, else `fetch_channel`; `Ok(false)` when it is
    /// unreachable over HTTP.
    async fn reachable(&self, channel: i64) -> Result<Result<(), RestError>> {
        if self.cache.has_channel(channel) {
            return Ok(Ok(()));
        }
        match self.b.fetch_channel(channel).await {
            Ok(()) => Ok(Ok(())),
            Err(e) => Ok(Err(e.http()?)),
        }
    }

    /// `_post`: send one message (role pings, custom text, design attachment).
    pub async fn post(
        &self,
        guild_id: i64,
        target: Target<'_>,
        embed: &Obj,
        key: &str,
        extra_content: Option<&str>,
        design_id: Option<&str>,
    ) -> Result<Option<i64>> {
        let channel = target.channel_id.unwrap_or_default();
        if self.reachable(channel).await?.is_err() {
            tracing::warn!("announce {key}: channel {channel} unreachable (guild {guild_id})");
            return Ok(None);
        }
        let mut parts = Vec::new();
        if !target.ping_role_ids.is_empty() {
            parts.push(target.ping_role_ids.iter().map(|r| format!("<@&{r}>")).collect::<Vec<_>>().join(" "));
        }
        if let Some(extra) = extra_content.filter(|e| !e.is_empty()) {
            parts.push(extra.to_owned());
        }
        let joined = parts.join("\n");
        let content = kiwi_core::pystr::prefix(&joined, 2000);
        let content = (!content.is_empty()).then_some(content);
        let png = match design_id.filter(|d| !d.is_empty()) {
            Some(id) => {
                let png = self.b.render_design(id).await;
                if png.is_none() {
                    tracing::warn!("announce image render failed ({id})");
                }
                png
            }
            None => None,
        };
        let payload = message::send_payload(content, embed, Some(target.ping_role_ids), png.is_some());
        match self.b.send_message(channel, payload, png).await {
            Ok(id) => {
                tracing::info!("announced {key} to guild {guild_id}");
                Ok(Some(id))
            }
            Err(e) => {
                match e.http()? {
                    RestError::Forbidden => {
                        tracing::warn!("announce {key}: missing send perms in guild {guild_id} channel {channel}")
                    }
                    other => tracing::error!("announce {key}: send failed in guild {guild_id}: {other:?}"),
                }
                Ok(None)
            }
        }
    }

    /// `_delete_message`: true when the record should be dropped.
    pub async fn delete_message(&self, channel: i64, message: i64) -> Result<bool> {
        match self.reachable(channel).await? {
            Ok(()) => {}
            Err(RestError::NotFound) => return Ok(true),
            Err(_) => return Ok(false),
        }
        match self.b.delete_message(channel, message).await {
            Ok(()) => Ok(true),
            Err(e) => Ok(match e.http()? {
                RestError::NotFound => true,
                RestError::Forbidden => {
                    tracing::warn!("cleanup: can't delete message {message} in channel {channel}");
                    true
                }
                _ => {
                    tracing::warn!("cleanup: delete failed for {message}");
                    false
                }
            }),
        }
    }

    /// `_edit_message`: "ok", "gone" or "retry".
    async fn edit_message(&self, channel: i64, message: i64, embed: &Obj) -> Result<&'static str> {
        match self.reachable(channel).await? {
            Ok(()) => {}
            Err(RestError::NotFound) => return Ok("gone"),
            Err(_) => return Ok("retry"),
        }
        match self.b.edit_message(channel, message, message::edit_payload(embed)).await {
            Ok(()) => Ok("ok"),
            Err(e) => Ok(match e.http()? {
                RestError::NotFound | RestError::Forbidden => "gone",
                _ => {
                    tracing::warn!("refresh: edit failed for {message}");
                    "retry"
                }
            }),
        }
    }

    pub async fn save_config(&self, cfg: &GuildConfig) -> Result<()> {
        self.b.save(GUILD_CONFIGS, cfg.id, cfg.to_doc()).await
    }

    /// `_track`: supersede the guild's earlier message of this kind, then
    /// record the new one.
    #[allow(clippy::too_many_arguments)]
    async fn track(
        &self,
        guild_id: i64,
        channel_id: i64,
        message_id: i64,
        atype: &AnnouncementType,
        anchor: &str,
        expires: Option<i64>,
        token: &str,
        custom: bool,
    ) -> Result<()> {
        let now = self.now_bson();
        for prior in self.load_tracked(TrackedQuery::GuildKind(guild_id, atype.key.into())).await? {
            self.delete_message(prior.channel_id, prior.message_id).await?;
            self.b.delete(TRACKED, prior.id).await?;
        }
        let rec = Tracked {
            id: Default::default(),
            guild_id,
            channel_id,
            message_id,
            kind: atype.key.into(),
            anchor: anchor.into(),
            expires_at: expires,
            refresh: atype.expires && !custom,
            refresh_v: Some(token.into()),
            created_at: now,
        };
        self.b.insert(TRACKED, rec.fields()).await?;
        Ok(())
    }

    /// `cleanup_tracked`: delete ended announcements, then their records.
    pub async fn cleanup_tracked(&self) {
        let now = self.now_secs();
        let rows = match self.load_tracked(TrackedQuery::Expired(now)).await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("cleanup: query failed: {e:#}");
                return;
            }
        };
        for t in rows {
            let step = async {
                if t.expires_at.is_none() {
                    return Ok(());
                }
                if self.delete_message(t.channel_id, t.message_id).await? {
                    self.b.delete(TRACKED, t.id).await?;
                }
                anyhow::Ok(())
            };
            if let Err(e) = step.await {
                tracing::warn!("cleanup: failed for a tracked message: {e:#}");
            }
        }
    }

    /// A tracked query as Beanie's `to_list()`: every row validated, or none.
    async fn load_tracked(&self, q: TrackedQuery) -> Result<Vec<Tracked>> {
        let at = self.now_bson();
        self.b.tracked(q).await?.iter().map(|d| Tracked::from_doc(d, at)).collect()
    }

    /// `_load_configs`: every guild config, legacy shapes folded in and saved.
    async fn load_configs(&self) -> Result<Vec<GuildConfig>> {
        let now = self.now_bson();
        let mut out = Vec::new();
        for d in self.b.guild_configs().await? {
            out.push(GuildConfig::from_doc(&d, now)?);
        }
        for cfg in &mut out {
            if cfg.migrate_legacy() {
                self.save_config(cfg).await?;
            }
        }
        Ok(out)
    }

    /// `_announce_one`.
    async fn announce_one(&self, atype: &AnnouncementType, configs: &mut [GuildConfig]) -> Result<()> {
        let key = atype.key;
        let candidates: Vec<usize> = (0..configs.len())
            .filter(|&i| configs[i].announcements.get(key).is_some_and(AnnouncementSetting::live))
            .collect();
        if candidates.is_empty() {
            return Ok(());
        }
        let anchor = match announcements::anchor(&self.b, atype.kind).await {
            Ok(Some(a)) => a,
            Ok(None) => return Ok(()),
            Err(e) => {
                tracing::warn!("anchor failed for {key}: {e:#}");
                return Ok(());
            }
        };
        let pending: Vec<usize> = candidates
            .into_iter()
            .filter(|&i| configs[i].announcements[key].last_anchor.as_deref() != Some(anchor.as_str()))
            .collect();
        if pending.is_empty() {
            return Ok(());
        }
        let langs: IndexSet<&'static str> =
            pending.iter().map(|&i| normalize_lang(Some(configs[i].language.as_str()))).collect();
        let mut expires = None;
        let mut token = String::new();
        let mut embeds: Vec<(&'static str, Obj)> = Vec::new();
        let built: Result<()> = async {
            if atype.auto_manage {
                if atype.expires {
                    expires = py_expiry(announcements::expiry(&self.b, atype.kind).await?)?;
                }
                token = refresh_token(expires, self.now_secs());
                for &lang in &langs {
                    let mut d = image_embed(&self.asset, key, &token, lang);
                    if let Some(cap) = announcements::caption(&self.b, atype.kind, &self.i18n.tr(Some(lang))).await? {
                        d.extend(cap);
                    }
                    embeds.push((lang, d));
                }
            } else {
                for &lang in &langs {
                    let tr = self.i18n.tr(Some(lang));
                    embeds.push((lang, announcements::build_embed(&self.b, atype.kind, &tr, &self.asset).await?));
                }
            }
            Ok(())
        }
        .await;
        if let Err(e) = built {
            tracing::warn!("embed build failed for {key}: {e:#}");
            return Ok(());
        }
        let mut ctxs: Vec<(&'static str, crate::embed_templates::Ctx)> = Vec::new();
        let wants_custom = pending
            .iter()
            .any(|&i| configs[i].announcements[key].template.as_ref().is_some_and(|t: &EmbedTemplate| t.enabled));
        if contexts::has_customization(key) && wants_custom {
            for &lang in &langs {
                match contexts::context(&self.b, &self.i18n.tr(Some(lang)), key, &self.asset).await {
                    Ok(c) => ctxs.push((lang, c)),
                    Err(e) => tracing::warn!("context build failed for {key} ({lang}): {e:#}"),
                }
            }
        }
        for i in pending {
            let cfg = &mut configs[i];
            let lang = normalize_lang(Some(cfg.language.as_str()));
            let setting = cfg.announcements[key].clone();
            let ctx = ctxs.iter().find(|(l, _)| *l == lang).map(|(_, c)| c);
            let tmpl = setting.template.as_ref().filter(|t| t.enabled);
            let (embed, extra, design) = match (tmpl, ctx) {
                (Some(t), Some(ctx)) => {
                    let (ed, content) = render_template(t, ctx, ctx.get("image_url"));
                    let design = if t.show_image { t.image_design_id.clone() } else { None };
                    (ed, content, design)
                }
                _ => (embeds.iter().find(|(l, _)| *l == lang).map(|(_, e)| e.clone()).context("embed")?, None, None),
            };
            let custom = tmpl.is_some() && ctx.is_some();
            let mid =
                self.post(cfg.guild_id, (&setting).into(), &embed, key, extra.as_deref(), design.as_deref()).await?;
            if let Some(mid) = mid {
                if let Some(s) = cfg.announcements.get_mut(key) {
                    s.last_anchor = Some(anchor.clone());
                }
                cfg.updated_at = self.now_bson();
                self.save_config(cfg).await?;
                if atype.auto_manage {
                    let channel = setting.channel_id.unwrap_or_default();
                    self.track(cfg.guild_id, channel, mid, atype, &anchor, expires, &token, custom).await?;
                }
            }
        }
        Ok(())
    }

    /// `run_announcement_type`: one type, from an event.
    pub async fn run_announcement_type(&self, key: &str) -> Result<()> {
        let Some(atype) = announcements::by_key(key) else { return Ok(()) };
        let _g = self.announce_lock.lock().await;
        let mut configs = self.load_configs().await?;
        self.announce_one(atype, &mut configs).await
    }

    /// `run_all_announcements`: every type (the minute backstop).
    pub async fn run_all_announcements(&self) -> Result<()> {
        let _g = self.announce_lock.lock().await;
        let mut configs = self.load_configs().await?;
        if !configs.iter().any(|c| c.announcements.values().any(AnnouncementSetting::live)) {
            return Ok(());
        }
        for atype in &announcements::TYPES {
            self.announce_one(atype, &mut configs).await?;
        }
        Ok(())
    }

    /// `i18n.guild_language(guild_id)`.
    async fn guild_language(&self, guild_id: i64) -> &'static str {
        if guild_id != 0 {
            match self.b.guild_config(guild_id).await {
                Ok(Some(d)) => {
                    if let Ok(cfg) = GuildConfig::from_doc(&d, self.now_bson()) {
                        let lang = normalize_lang(Some(cfg.language.as_str()));
                        if lang == cfg.language {
                            return lang;
                        }
                    }
                }
                Ok(None) => {}
                Err(e) => tracing::warn!("i18n: guild_language lookup failed for {guild_id}: {e:#}"),
            }
        }
        "en"
    }

    /// `refresh_tracked`: re-edit banners whose shown countdown changed.
    pub async fn refresh_tracked(&self) -> Result<()> {
        let now = self.now_secs();
        let rows = match self.load_tracked(TrackedQuery::Refreshing).await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("refresh: query failed: {e:#}");
                return Ok(());
            }
        };
        for mut t in rows {
            if t.expires_at.is_some_and(|e| e != 0 && e <= now) {
                continue;
            }
            let token = refresh_token(t.expires_at, now);
            if t.refresh_v.as_deref() == Some(token.as_str()) {
                continue;
            }
            let lang = self.guild_language(t.guild_id).await;
            let embed = image_embed(&self.asset, &t.kind, &token, lang);
            match self.edit_message(t.channel_id, t.message_id, &embed).await? {
                "ok" => {
                    t.refresh_v = Some(token);
                    self.b.save(TRACKED, t.id, t.to_doc()).await?;
                }
                "gone" => self.b.delete(TRACKED, t.id).await?,
                _ => {}
            }
        }
        Ok(())
    }
}

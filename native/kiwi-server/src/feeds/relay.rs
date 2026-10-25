//! `app/trove/feeds.py`: Twitch, YouTube and Bilibili fetched at source into
//! `feed_cache` (the `feeds` loop). Items keep the exact values (and so the
//! BSON types) Python would store.

use std::{collections::HashSet, sync::LazyLock};

use anyhow::{Context, anyhow, bail};
use chrono::{DateTime, Datelike, TimeDelta, Utc};
use mongodb::bson::doc;
use regex::Regex;
use serde_json::{Value, json};

use super::{
    http::{self, check_status, clients},
    pyval::{self, NONE, empty_dict, get, or, truthy},
    store,
};
use crate::{dt, runtime_config::get_setting, state::AppState};

const TWITCH_TOKEN_URL: &str = "https://id.twitch.tv/oauth2/token";
const TWITCH_STREAMS_URL: &str = "https://api.twitch.tv/helix/streams";
const YOUTUBE_SEARCH_URL: &str = "https://www.googleapis.com/youtube/v3/search";
const YOUTUBE_VIDEOS_URL: &str = "https://www.googleapis.com/youtube/v3/videos";
const BILIBILI_SPI_URL: &str = "https://api.bilibili.com/x/frontend/finger/spi";
const BILIBILI_SEARCH_URL: &str = "https://api.bilibili.com/x/web-interface/search/type";

pub const FEEDS: [&str; 3] = ["twitch", "youtube", "bilibili"];

/// Insertion-ordered map with Python dict update semantics.
struct OrderedMap<V> {
    entries: Vec<(String, V)>,
}

impl<V> OrderedMap<V> {
    fn new() -> Self {
        OrderedMap { entries: Vec::new() }
    }

    fn insert(&mut self, key: String, value: V) {
        match self.entries.iter_mut().find(|(k, _)| *k == key) {
            Some(slot) => slot.1 = value,
            None => self.entries.push((key, value)),
        }
    }

    fn get(&self, key: &str) -> Option<&V> {
        self.entries.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    fn get_mut(&mut self, key: &str) -> Option<&mut V> {
        self.entries.iter_mut().find(|(k, _)| k == key).map(|(_, v)| v)
    }
}

/// `for x in v` over a JSON value (a dict yields its keys, a str its chars).
pub fn py_iter(v: &Value) -> anyhow::Result<Vec<Value>> {
    Ok(match v {
        Value::Array(a) => a.clone(),
        Value::Object(o) => o.keys().map(|k| Value::String(k.clone())).collect(),
        Value::String(s) => s.chars().map(|c| Value::String(c.to_string())).collect(),
        other => bail!("TypeError: '{}' object is not iterable", pyval::type_name(other)),
    })
}

fn as_str<'a>(v: &'a Value, what: &str) -> anyhow::Result<&'a str> {
    v.as_str().ok_or_else(|| anyhow!("AttributeError: {what} is a {}", pyval::type_name(v)))
}

/// `_normalize_twitch(raw)`.
pub fn normalize_twitch(raw: &Value) -> anyhow::Result<Vec<Value>> {
    let streams = match raw {
        Value::Array(_) => raw.clone(),
        _ => get(or(Some(raw), empty_dict()), "data")?.cloned().unwrap_or(Value::Array(Vec::new())),
    };
    let mut out = Vec::new();
    for s in py_iter(&streams)? {
        if !s.is_object() {
            continue;
        }
        let empty = Value::String(String::new());
        let login = or(get(&s, "user_login")?, or(get(&s, "user_name")?, &empty)).clone();
        let thumb = as_str(or(get(&s, "thumbnail_url")?, &empty), "thumbnail_url")?
            .replace("{width}", "440")
            .replace("{height}", "248");
        let url = if truthy(&login) {
            format!("https://twitch.tv/{}", pyval::py_str(&login))
        } else {
            "https://twitch.tv".into()
        };
        let viewers = pyval::py_int(or(get(&s, "viewer_count")?, &json!(0)))
            .ok_or_else(|| anyhow!("ValueError: viewer_count"))?;
        out.push(json!({
            "channel": or(get(&s, "user_name")?, &login),
            "login": login,
            "url": url,
            "title": or(get(&s, "title")?, &empty),
            "viewers": viewers_value(viewers)?,
            "game": get(&s, "game_name")?.unwrap_or(&NONE),
            "started_at": get(&s, "started_at")?.unwrap_or(&NONE),
            "thumbnail": if thumb.is_empty() { Value::Null } else { Value::String(thumb) },
        }));
    }
    Ok(out)
}

fn viewers_value(i: i128) -> anyhow::Result<Value> {
    if let Ok(v) = i64::try_from(i) {
        Ok(Value::from(v))
    } else {
        bail!("OverflowError: MongoDB can only handle up to 8-byte ints")
    }
}

/// `_normalize_videos(raw)`.
pub fn normalize_videos(raw: &Value) -> Vec<Value> {
    let Value::Array(videos) = raw else { return Vec::new() };
    let empty = Value::String(String::new());
    videos
        .iter()
        .filter(|v| v.is_object() && v.get("url").is_some_and(truthy))
        .map(|v| {
            let field = |k: &str| v.get(k).unwrap_or(&NONE);
            json!({
                "title": or(v.get("title"), &empty),
                "url": or(v.get("url"), &empty),
                "channel": or(v.get("channel"), &empty),
                "video_id": field("video_id"),
                "published_at": field("published_at"),
                "thumbnail_url": field("thumbnail_url"),
            })
        })
        .collect()
}

/// `_csv_set(raw)`.
pub fn csv_set(raw: &Value) -> anyhow::Result<Vec<String>> {
    let empty = Value::String(String::new());
    let s = as_str(or(Some(raw), &empty), "csv setting")?;
    let mut out: Vec<String> =
        s.split(',').map(|p| kiwi_core::feeds::py::strip(p).to_lowercase()).filter(|p| !p.is_empty()).collect();
    out.sort();
    out.dedup();
    Ok(out)
}

/// Python's Unicode `\w`: letters, numerics and `_`.
fn is_word(c: char) -> bool {
    static WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[\p{L}\p{N}_]$").expect("word class"));
    c == '_' || c.is_ascii_alphanumeric() || (!c.is_ascii() && WORD.is_match(c.encode_utf8(&mut [0; 4])))
}

fn boundary(text: &str, at: usize) -> bool {
    let before = text[..at].chars().next_back().is_some_and(is_word);
    let after = text[at..].chars().next().is_some_and(is_word);
    before != after
}

/// `_has_term(text, term)`: `re.search(r"\b" + re.escape(term) + r"\b", text)`.
pub fn has_term(text: &str, term: &str) -> bool {
    let mut from = 0;
    while let Some(off) = text[from..].find(term) {
        let start = from + off;
        if boundary(text, start) && boundary(text, start + term.len()) {
            return true;
        }
        match text[start..].chars().next() {
            Some(c) => from = start + c.len_utf8(),
            None => return false,
        }
    }
    false
}

/// `_iso8601_seconds(value)`.
pub fn iso8601_seconds(value: Option<&Value>) -> anyhow::Result<Option<i128>> {
    let Some(v) = value.filter(|v| truthy(v)) else { return Ok(None) };
    let s = v.as_str().ok_or_else(|| anyhow!("TypeError: expected string"))?;
    let Some(mut rest) = s.strip_prefix("PT") else { return Ok(None) };
    let mut parts = [0i128; 3];
    for (slot, unit) in parts.iter_mut().zip(['H', 'M', 'S']) {
        let digits: usize =
            rest.chars().take_while(|c| kiwi_core::feeds::py::decimal(*c).is_some()).map(char::len_utf8).sum();
        if digits > 0 && rest[digits..].starts_with(unit) {
            *slot = kiwi_core::feeds::py::int(&rest[..digits], 10).unwrap_or(0);
            rest = &rest[digits + 1..];
        }
    }
    if !rest.is_empty() && rest != "\n" {
        return Ok(None);
    }
    Ok(Some(parts[0] * 3600 + parts[1] * 60 + parts[2]))
}

/// `seq[:n]` length.
fn head(len: usize, n: i128) -> usize {
    if n >= 0 { usize::try_from(n).unwrap_or(usize::MAX).min(len) } else { (len as i128 + n).max(0) as usize }
}

/// `x["published_at"] or ""` as a sort key.
fn date_key(v: &Value) -> anyhow::Result<String> {
    match v.get("published_at") {
        Some(Value::String(s)) => Ok(s.clone()),
        Some(p) if truthy(p) => bail!("TypeError: '<' not supported for published_at"),
        _ => Ok(String::new()),
    }
}

/// Python's stable `list.sort(key=..., reverse=True)`.
fn sort_desc(v: &mut [Value]) -> anyhow::Result<()> {
    if v.len() < 2 {
        return Ok(());
    }
    let mut keyed: Vec<(String, Value)> =
        v.iter().map(|x| Ok((date_key(x)?, x.clone()))).collect::<anyhow::Result<_>>()?;
    keyed.sort_by(|a, b| b.0.cmp(&a.0));
    for (slot, (_, x)) in v.iter_mut().zip(keyed) {
        *slot = x;
    }
    Ok(())
}

/// The YouTube curation knobs (runtime_config `community_feeds`).
#[derive(Clone, Debug, Default)]
pub struct YtConfig {
    pub query: Value,
    pub excluded_channels: Vec<String>,
    pub exclude_terms: Vec<String>,
    pub require_terms: Vec<String>,
    pub relevance_terms: Vec<String>,
    pub gaming_cat: String,
    pub cutoff_days: i128,
    pub per_channel_max: i128,
    pub max_items: i128,
}

/// `(it.get("id") or {}).get("videoId")` -> snippet, in first-seen order,
/// later duplicates winning, the `None` key dropped.
pub fn search_snippets(search: &Value) -> anyhow::Result<Vec<(String, Value)>> {
    let empty = Value::Array(Vec::new());
    let items = match get(search, "items")? {
        Some(v) => v,
        None => &empty,
    };
    let mut map = OrderedMap::new();
    for it in py_iter(items)? {
        let id = or(get(&it, "id")?, empty_dict());
        let vid = get(id, "videoId")?;
        let snip = or(get(&it, "snippet")?, empty_dict()).clone();
        match vid {
            None | Some(Value::Null) => {}
            Some(Value::String(s)) => map.insert(s.clone(), snip),
            Some(other) => bail!("TypeError: videoId {}", pyval::type_name(other)),
        }
    }
    Ok(map.entries)
}

/// The curation after `videos.list` (`details` is its JSON).
pub fn curate_youtube(snippets: &[(String, Value)], details: &Value, cfg: &YtConfig) -> anyhow::Result<Vec<Value>> {
    let empty_list = Value::Array(Vec::new());
    let mut by_id: OrderedMap<Value> = OrderedMap::new();
    for v in py_iter(get(details, "items")?.unwrap_or(&empty_list))? {
        let id = match &v {
            Value::Object(m) => m.get("id").ok_or_else(|| anyhow!("KeyError: 'id'"))?,
            other => bail!("TypeError: {} indices", pyval::type_name(other)),
        };
        match id {
            Value::String(s) => by_id.insert(s.clone(), v.clone()),
            Value::Array(_) | Value::Object(_) => bail!("TypeError: unhashable type"),
            _ => {}
        }
    }
    let excluded: HashSet<&str> = cfg.excluded_channels.iter().map(String::as_str).collect();
    let empty = Value::String(String::new());
    let mut temp: Vec<Value> = Vec::new();
    for (vid, ssnip) in snippets {
        let full = by_id.get(vid).unwrap_or(empty_dict());
        let d = or(get(full, "snippet")?, empty_dict());
        let pick = |key: &str| -> anyhow::Result<Value> {
            Ok(match get(d, key)?.filter(|v| truthy(v)) {
                Some(v) => v.clone(),
                None => get(ssnip, key)?.cloned().unwrap_or_else(|| empty.clone()),
            })
        };
        let channel = pick("channelTitle")?;
        let title = pick("title")?;
        if excluded.contains(as_str(&channel, "channel")?.to_lowercase().as_str()) {
            continue;
        }
        if matches!(get(ssnip, "liveBroadcastContent")?, Some(Value::String(s)) if s == "live" || s == "upcoming") {
            continue;
        }
        let description = get(d, "description")?.unwrap_or(&empty);
        let tags: Vec<String> = py_iter(or(get(d, "tags")?, &empty_list))?
            .iter()
            .map(|t| as_str(t, "tag").map(str::to_owned))
            .collect::<anyhow::Result<_>>()?;
        let blob = [
            as_str(&title, "title").map_err(|_| anyhow!("TypeError: sequence item 0"))?,
            as_str(description, "description").map_err(|_| anyhow!("TypeError: sequence item 1"))?,
            &tags.join(" "),
        ]
        .join(" ")
        .to_lowercase();
        let details_of = or(get(full, "contentDetails")?, empty_dict());
        let dur = iso8601_seconds(get(details_of, "duration")?)?;
        if dur.is_some_and(|d| d <= 60) || has_term(&blob, "shorts") {
            continue;
        }
        if cfg.exclude_terms.iter().any(|t| has_term(&blob, t)) {
            continue;
        }
        if !cfg.require_terms.iter().all(|t| has_term(&blob, t)) {
            continue;
        }
        let in_gaming =
            !cfg.gaming_cat.is_empty() && get(d, "categoryId")? == Some(&Value::String(cfg.gaming_cat.clone()));
        let has_signal = cfg.relevance_terms.iter().any(|t| has_term(&blob, t));
        if !cfg.relevance_terms.is_empty() && !(has_signal || in_gaming) {
            continue;
        }
        let thumbs = or(get(d, "thumbnails")?, or(get(ssnip, "thumbnails")?, empty_dict()));
        let thumb = get(or(get(thumbs, "high")?, empty_dict()), "url")?.cloned().unwrap_or(Value::Null);
        let published = match get(ssnip, "publishedAt")?.filter(|v| truthy(v)) {
            Some(v) => v.clone(),
            None => get(d, "publishedAt")?.cloned().unwrap_or(Value::Null),
        };
        temp.push(json!({
            "title": title,
            "video_id": vid,
            "url": format!("https://www.youtube.com/watch?v={vid}"),
            "channel": channel,
            "published_at": published,
            "thumbnail_url": thumb,
        }));
    }
    let mut by_channel: OrderedMap<Vec<Value>> = OrderedMap::new();
    for v in temp {
        let ch = as_str(&v["channel"], "channel")?.to_owned();
        match by_channel.get_mut(&ch) {
            Some(list) => list.push(v),
            None => by_channel.insert(ch, vec![v]),
        }
    }
    let mut pool = Vec::new();
    for (_, mut vids) in by_channel.entries {
        sort_desc(&mut vids)?;
        let n = head(vids.len(), cfg.per_channel_max);
        pool.extend(vids.into_iter().take(n));
    }
    sort_desc(&mut pool)?;
    let n = head(pool.len(), cfg.max_items);
    pool.truncate(n);
    Ok(normalize_videos(&Value::Array(pool)))
}

/// `now - timedelta(**{unit: amount})`, with Python's range errors.
fn minus(now: DateTime<Utc>, amount: i128, unit_secs: i128) -> anyhow::Result<DateTime<Utc>> {
    let secs = amount * unit_secs;
    if secs.div_euclid(86_400).abs() > 999_999_999 {
        bail!("OverflowError: days out of range");
    }
    let out = TimeDelta::try_seconds(secs as i64).and_then(|d| now.checked_sub_signed(d));
    match out {
        Some(t) if (1..=9999).contains(&t.year()) => Ok(t),
        _ => bail!("OverflowError: date value out of range"),
    }
}

/// `date.isoformat().replace("+00:00", "Z")`.
pub fn iso_z(t: &DateTime<Utc>) -> String {
    dt::isoformat(t).replace("+00:00", "Z")
}

/// The Bilibili knobs.
#[derive(Clone, Copy, Debug)]
pub struct BiliConfig {
    pub cutoff_days: i128,
    pub per_channel_max: i128,
    pub max_items: i128,
}

/// Bilibili's `search/type` video results, newest first, as feed items.
pub fn bilibili_videos(body: &Value, now: DateTime<Utc>, cfg: BiliConfig) -> anyhow::Result<Vec<Value>> {
    if body["code"].as_i64() != Some(0) {
        bail!("bilibili search rejected: code {} {}", body["code"], body["message"]);
    }
    let cutoff = minus(now, cfg.cutoff_days, 86_400)?.timestamp();
    let mut counts: Vec<(String, i128)> = Vec::new();
    let mut videos: Vec<Value> = Vec::new();
    for v in body["data"]["result"].as_array().map(Vec::as_slice).unwrap_or_default() {
        let bvid = v["bvid"].as_str().unwrap_or_default();
        let Some(pubdate) = v["pubdate"].as_i64() else { continue };
        if bvid.is_empty() || pubdate < cutoff {
            continue;
        }
        let author = v["author"].as_str().unwrap_or_default().trim().to_owned();
        let count = counts.iter().find(|(a, _)| *a == author).map_or(0, |(_, n)| *n);
        if count >= cfg.per_channel_max {
            continue;
        }
        match counts.iter_mut().find(|(a, _)| *a == author) {
            Some((_, n)) => *n += 1,
            None => counts.push((author.clone(), 1)),
        }
        let title = TAG_RE.replace_all(v["title"].as_str().unwrap_or_default(), "");
        let mut pic = v["pic"].as_str().unwrap_or_default().to_owned();
        if pic.starts_with("//") {
            pic = format!("https:{pic}");
        }
        let published = DateTime::from_timestamp(pubdate, 0).context("pubdate out of range")?;
        videos.push(json!({
            "title": kiwi_core::feeds::unescape::unescape(&title),
            "video_id": bvid,
            "url": format!("https://www.bilibili.com/video/{bvid}"),
            "channel": author,
            "published_at": iso_z(&published),
            "thumbnail_url": pic,
        }));
        if videos.len() as i128 >= cfg.max_items {
            break;
        }
    }
    sort_desc(&mut videos)?;
    Ok(normalize_videos(&Value::Array(videos)))
}

static TAG_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<[^>]*>").expect("tag regex"));

// --- fetching ----------------------------------------------------------------

static TWITCH_TOKEN: tokio::sync::Mutex<Option<String>> = tokio::sync::Mutex::const_new(None);

fn twitch_creds(state: &AppState) -> anyhow::Result<(String, String)> {
    let s = state.settings();
    match (s.twitch_client_id.as_deref(), s.twitch_client_secret.as_deref()) {
        (Some(id), Some(secret)) if !id.is_empty() && !secret.is_empty() => Ok((id.to_owned(), secret.to_owned())),
        _ => bail!("TWITCH_CLIENT_ID / TWITCH_CLIENT_SECRET not configured"),
    }
}

/// `_twitch_app_token`: cached, re-minted when `force`.
async fn twitch_token(state: &AppState, force: bool) -> anyhow::Result<String> {
    let mut cached = TWITCH_TOKEN.lock().await;
    if let Some(t) = cached.as_ref().filter(|t| !t.is_empty() && !force) {
        return Ok(t.clone());
    }
    let (id, secret) = twitch_creds(state)?;
    let resp = clients()
        .api
        .post(TWITCH_TOKEN_URL)
        .query(&[("client_id", id.as_str()), ("client_secret", &secret), ("grant_type", "client_credentials")])
        .send()
        .await?;
    let body = http::json(check_status(resp)?).await?;
    let token = match &body {
        Value::Object(m) => pyval::py_str(m.get("access_token").ok_or_else(|| anyhow!("KeyError: 'access_token'"))?),
        other => bail!("TypeError: token response is a {}", pyval::type_name(other)),
    };
    *cached = Some(token.clone());
    Ok(token)
}

async fn fetch_twitch(state: &AppState) -> anyhow::Result<Vec<Value>> {
    let (id, _) = twitch_creds(state)?;
    let game = state.settings().twitch_game_id.clone();
    let get = |tok: String| {
        clients()
            .api
            .get(TWITCH_STREAMS_URL)
            .query(&[("game_id", game.as_str()), ("first", "100")])
            .header("Client-Id", id.as_str())
            .header("Authorization", format!("Bearer {tok}"))
            .send()
    };
    let mut resp = get(twitch_token(state, false).await?).await?;
    if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
        resp = get(twitch_token(state, true).await?).await?;
    }
    normalize_twitch(&http::json(check_status(resp)?).await?)
}

async fn setting(state: &AppState, key: &str) -> anyhow::Result<Value> {
    get_setting(state, key).await.with_context(|| format!("runtime_config {key}"))
}

async fn setting_int(state: &AppState, key: &str) -> anyhow::Result<i128> {
    pyval::py_int(&setting(state, key).await?).ok_or_else(|| anyhow!("ValueError: {key}"))
}

async fn fetch_youtube(state: &AppState) -> anyhow::Result<Vec<Value>> {
    let key = state.settings().yt_api_key.clone().filter(|k| !k.is_empty());
    let Some(key) = key else { bail!("YT_API_KEY not configured") };
    let gaming = setting(state, "feeds_youtube_video_category_id").await?;
    let cfg = YtConfig {
        query: setting(state, "feeds_youtube_query").await?,
        excluded_channels: csv_set(&setting(state, "feeds_youtube_excluded_channels").await?)?,
        exclude_terms: csv_set(&setting(state, "feeds_youtube_excluded_title_terms").await?)?,
        require_terms: csv_set(&setting(state, "feeds_youtube_require_terms").await?)?,
        relevance_terms: csv_set(&setting(state, "feeds_youtube_relevance_terms").await?)?,
        gaming_cat: kiwi_core::feeds::py::strip(as_str(or(Some(&gaming), &Value::String(String::new())), "category")?)
            .to_owned(),
        cutoff_days: setting_int(state, "feeds_video_cutoff_days").await?,
        per_channel_max: setting_int(state, "feeds_per_channel_max").await?,
        max_items: setting_int(state, "feeds_max_items").await?,
    };
    let published_after = iso_z(&minus(dt::now(), cfg.cutoff_days, 86_400)?);
    let query = pyval::py_str(&cfg.query);
    let resp = clients()
        .api
        .get(YOUTUBE_SEARCH_URL)
        .query(&[
            ("part", "snippet"),
            ("q", query.as_str()),
            ("type", "video"),
            ("order", "relevance"),
            ("publishedAfter", published_after.as_str()),
            ("maxResults", "50"),
            ("key", key.as_str()),
        ])
        .send()
        .await?;
    let snippets = search_snippets(&http::json(check_status(resp)?).await?)?;
    if snippets.is_empty() {
        return Ok(Vec::new());
    }
    let ids: Vec<&str> = snippets.iter().map(|(k, _)| k.as_str()).collect();
    let resp = clients()
        .api
        .get(YOUTUBE_VIDEOS_URL)
        .query(&[
            ("part", "snippet,contentDetails"),
            ("id", ids.join(",").as_str()),
            ("maxResults", "50"),
            ("key", key.as_str()),
        ])
        .send()
        .await?;
    let details = http::json(check_status(resp)?).await?;
    curate_youtube(&snippets, &details, &cfg)
}

async fn fetch_bilibili(state: &AppState) -> anyhow::Result<Vec<Value>> {
    let keyword = pyval::py_str(&setting(state, "feeds_bilibili_keyword").await?);
    let cfg = BiliConfig {
        cutoff_days: setting_int(state, "feeds_video_cutoff_days").await?,
        per_channel_max: setting_int(state, "feeds_per_channel_max").await?,
        max_items: setting_int(state, "feeds_max_items").await?,
    };
    // The search API answers anonymous callers only with a buvid3 cookie.
    let spi = http::json(check_status(clients().bilibili.get(BILIBILI_SPI_URL).send().await?)?).await?;
    let buvid3 = spi["data"]["b_3"].as_str().context("bilibili spi without b_3")?;
    let resp = clients()
        .bilibili
        .get(BILIBILI_SEARCH_URL)
        .header("Cookie", format!("buvid3={buvid3}"))
        .header("Referer", "https://search.bilibili.com/")
        .query(&[("search_type", "video"), ("order", "pubdate"), ("keyword", keyword.as_str())])
        .send()
        .await?;
    let body = http::json(check_status(resp)?).await?;
    bilibili_videos(&body, dt::now(), cfg)
}

/// `refresh_feed(feed)`: fetch at source, then upsert the `feed_cache` doc.
pub async fn refresh_feed(state: &AppState, feed: &str) -> anyhow::Result<usize> {
    let items = match feed {
        "twitch" => fetch_twitch(state).await?,
        "youtube" => fetch_youtube(state).await?,
        _ => fetch_bilibili(state).await?,
    };
    let fields = doc! { "items": store::items_bson(&items)?, "fetched_at": dt::to_bson(dt::now()) };
    store::upsert_by(state.mongo(), &store::FEED_MODEL, "feed", feed, fields).await?;
    Ok(items.len())
}

/// `refresh_all_feeds`: one failing feed keeps its last good payload.
pub async fn refresh_all(state: AppState) -> anyhow::Result<()> {
    for feed in FEEDS {
        match refresh_feed(&state, feed).await {
            Ok(n) => tracing::info!("Refreshed feed {feed}: {n} item(s)"),
            Err(err) => tracing::warn!("Feed '{feed}' refresh failed: {err:#}"),
        }
    }
    Ok(())
}

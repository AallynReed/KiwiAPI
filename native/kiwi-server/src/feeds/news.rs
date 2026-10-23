//! `app/trove/news.py`'s refresher (loop `news`): fetch the official RSS
//! feed, upsert every item into `trove_news` by url, then publish
//! `trove_news` on the events bus.

use anyhow::anyhow;
use kiwi_core::feeds::news::{FeedError, NewsItem, parse_feed};
use mongodb::bson::{Bson, Document, doc};

use super::{
    http::{check_status, clients},
    store,
};
use crate::{dt, state::AppState};

/// The upserted fields (`it` minus `url`), in model order.
pub fn fields(it: &NewsItem) -> anyhow::Result<Document> {
    let published = match it.published_at {
        None => Bson::Null,
        Some(ts) => {
            let t = chrono::DateTime::from_timestamp(ts, 0).ok_or_else(|| anyhow!("published_at out of range"))?;
            Bson::DateTime(dt::to_bson(t))
        }
    };
    Ok(doc! {
        "title": &it.title,
        "author": &it.author,
        "summary": &it.summary,
        "category": &it.category,
        "categories": it.categories.iter().map(|c| Bson::String(c.clone())).collect::<Vec<_>>(),
        "image": it.image.as_ref().map_or(Bson::Null, |s| Bson::String(s.clone())),
        "published_at": published,
    })
}

/// `resp.text`: decoded with the charset the response declares (UTF-8
/// otherwise), bad bytes replaced.
async fn fetch_text(url: &str) -> anyhow::Result<String> {
    let resp = check_status(clients().fetch.get(url).send().await?)?;
    Ok(resp.text().await?)
}

/// `refresh_news`: the number of items processed.
pub async fn refresh(state: &AppState) -> anyhow::Result<usize> {
    let text = fetch_text(&state.settings().trove_news_feed_url).await?;
    let items = parse_feed(&text).map_err(|e| match e {
        FeedError::Xml(msg) => anyhow!("ParseError: {msg}"),
        FeedError::Raised(msg) => anyhow!("{msg}"),
    })?;
    let db = state.mongo();
    for it in &items {
        store::upsert_by(db, &store::NEWS_MODEL, "url", &it.url, fields(it)?).await?;
    }
    Ok(items.len())
}

/// One cycle of the `news` loop (`_refresh_news_cycle`).
pub async fn refresh_cycle(state: AppState) -> anyhow::Result<()> {
    let n = refresh(&state).await?;
    crate::events::publish_type(&state, "trove_news").await?;
    tracing::info!("Trove news refresh: {n} item(s)");
    Ok(())
}

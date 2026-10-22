//! The fields `_fetch_bilibili` reads off each `.bili-video-card` of the
//! search page, raw (the caller does the stripping, dates and filtering).

use super::soup::{ParseError, Soup};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Card {
    /// `card.select_one("a").get("href", "")`; `None` without an `<a>`.
    pub href: Option<String>,
    /// `h3.bili-video-card__info--tit` `title` attribute.
    pub title: Option<String>,
    /// `.bili-video-card__info--author` text.
    pub author: Option<String>,
    /// `.bili-video-card__info--date` text.
    pub date: Option<String>,
    /// `picture source` `srcset` attribute.
    pub srcset: Option<String>,
}

/// Every `soup.select(".bili-video-card")`, in document order.
pub fn cards(html: &str) -> Result<Vec<Card>, ParseError> {
    let soup = Soup::parse(html)?;
    let cards = soup.select(super::soup::ROOT, |t| t.has_class("bili-video-card"));
    Ok(cards
        .into_iter()
        .map(|card| {
            let desc = soup.descendants(card);
            let one = |pred: &dyn Fn(usize) -> bool| desc.iter().copied().find(|&t| pred(t));
            Card {
                href: one(&|t| soup.tag(t).name == "a").map(|t| soup.tag(t).get_str("href")),
                title: one(&|t| soup.tag(t).name == "h3" && soup.tag(t).has_class("bili-video-card__info--tit"))
                    .map(|t| soup.tag(t).get_str("title")),
                author: one(&|t| soup.tag(t).has_class("bili-video-card__info--author")).map(|t| soup.text(t)),
                date: one(&|t| soup.tag(t).has_class("bili-video-card__info--date")).map(|t| soup.text(t)),
                srcset: one(&|t| soup.tag(t).name == "source" && soup.has_ancestor(t, "picture"))
                    .map(|t| soup.tag(t).get_str("srcset")),
            }
        })
        .collect())
}

//! Pure pieces of the community-feed relays (`app/trove/feeds.py`,
//! `app/trove/news.py`): Python's `html.unescape`, the `html.parser` +
//! BeautifulSoup tree the Bilibili scrape reads, and the scrape's card fields.

pub mod bilibili;
pub mod py;
pub mod soup;
pub mod tables;
pub mod unescape;

#[cfg(test)]
mod tests;

//! Pure pieces of the community-feed relays (`app/trove/feeds.py`,
//! `app/trove/news.py`): Python's `html.unescape` and the `html.parser` +
//! BeautifulSoup tree.

pub mod news;
pub mod py;
pub mod soup;
pub mod tables;
pub mod unescape;

#[cfg(test)]
mod tests;

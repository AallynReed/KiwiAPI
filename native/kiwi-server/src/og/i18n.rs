//! `app/i18n.py`'s `t()` for the cards: the site catalog
//! (`site/static/locales/<lang>.json`) under the bot overlay
//! (`assets/locales/<lang>.json`), keyed by whitespace-normalised English, then
//! `str.format` with the English fallback.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, LazyLock, Mutex},
};

use kiwi_core::pystr;
use kiwi_pil::ImageError;
use serde_json::Value;

pub use super::pyformat::Arg;
use super::{
    pyformat::{self, FormatError},
    text::{Fonts, LangFonts, Text},
};

pub const SITE_DIR: &str = "site/static/locales";
pub const BOT_DIR: &str = "assets/locales";

type Catalog = Arc<HashMap<String, String>>;
type CatalogKey = (PathBuf, PathBuf, String);

static CACHE: LazyLock<Mutex<HashMap<CatalogKey, Catalog>>> = LazyLock::new(Default::default);

/// `_norm`: whitespace runs collapsed to one space, then stripped.
pub fn norm(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_ws = false;
    for c in s.chars() {
        if pystr::is_space(c) {
            in_ws = true;
        } else {
            if in_ws && !out.is_empty() {
                out.push(' ');
            }
            in_ws = false;
            out.push(c);
        }
    }
    out
}

/// `_load`: a missing or unreadable file is empty; a JSON document that is not
/// an object raises (`data.items()`), which the render turns into a failure.
fn load(path: &Path) -> Result<HashMap<String, String>, ImageError> {
    let Ok(bytes) = std::fs::read(path) else { return Ok(HashMap::new()) };
    let Ok(text) = std::str::from_utf8(&bytes) else {
        tracing::warn!("i18n: could not read locale file {}", path.display());
        return Ok(HashMap::new());
    };
    let parsed = if text.starts_with('\u{feff}') { None } else { crate::pyjson::loads(bytes.as_slice()).ok() };
    let Some(data) = parsed else {
        tracing::warn!("i18n: could not read locale file {}", path.display());
        return Ok(HashMap::new());
    };
    let Value::Object(map) = data else { return Err(ImageError::Value("locale file is not a JSON object")) };
    let mut out = HashMap::with_capacity(map.len());
    for (k, v) in map {
        if let Value::String(v) = v
            && !v.is_empty()
        {
            out.insert(norm(&k), v);
        }
    }
    Ok(out)
}

/// What a card in one language needs: `t()` and the faces `_font` tries.
pub struct Locale {
    pub tr: Tr,
    pub fonts: LangFonts,
}

impl Locale {
    pub fn en() -> Locale {
        Locale { tr: Tr::en(), fonts: LangFonts::default() }
    }

    pub fn load(lang: &str) -> Result<Locale, ImageError> {
        let tr = Tr::load(lang)?;
        Ok(Locale { fonts: LangFonts::for_lang(tr.lang()), tr })
    }

    pub fn text<'a>(&'a self, lib: &'a kiwi_pil::Library, fonts: &'a Fonts) -> Text<'a> {
        Text::localized(lib, fonts, &self.fonts)
    }
}

/// One language's `t()`.
#[derive(Clone)]
pub struct Tr {
    lang: &'static str,
    catalog: Option<Catalog>,
}

impl Tr {
    pub fn en() -> Tr {
        Tr { lang: "en", catalog: None }
    }

    /// `_catalog(lang)` from the repo's catalogs (relative to the working directory).
    pub fn load(lang: &str) -> Result<Tr, ImageError> {
        Self::load_from(lang, Path::new(SITE_DIR), Path::new(BOT_DIR))
    }

    /// Cached per process like `lru_cache`; a failed load is retried next time.
    pub fn load_from(lang: &str, site: &Path, bot: &Path) -> Result<Tr, ImageError> {
        let lang = super::normalize_lang(lang);
        let lang: &'static str = super::SUPPORTED.iter().find(|l| **l == lang).copied().unwrap_or("en");
        if lang == "en" {
            return Ok(Tr::en());
        }
        let key = (site.to_path_buf(), bot.to_path_buf(), lang.to_owned());
        if let Some(c) = CACHE.lock().ok().and_then(|c| c.get(&key).cloned()) {
            return Ok(Tr { lang, catalog: Some(c) });
        }
        let mut merged = load(&site.join(format!("{lang}.json")))?;
        merged.extend(load(&bot.join(format!("{lang}.json")))?);
        let catalog = Arc::new(merged);
        if let Ok(mut c) = CACHE.lock() {
            c.insert(key, catalog.clone());
        }
        Ok(Tr { lang, catalog: Some(catalog) })
    }

    pub fn lang(&self) -> &'static str {
        self.lang
    }

    fn lookup<'s>(&'s self, text: &'s str) -> &'s str {
        match &self.catalog {
            Some(c) => c.get(&norm(text)).map_or(text, String::as_str),
            None => text,
        }
    }

    /// `t(text)`.
    pub fn t(&self, text: &str) -> String {
        self.lookup(text).to_owned()
    }

    /// `t(text, **fmt)`: a translation `str.format` rejects falls back to the
    /// English source, then to the unformatted source; anything else raises.
    pub fn tf(&self, text: &str, args: &[(&str, Arg<'_>)]) -> Result<String, ImageError> {
        let other = |_| ImageError::Value("str.format raised");
        match pyformat::format(self.lookup(text), args) {
            Ok(s) => Ok(s),
            Err(FormatError::Lookup) => match pyformat::format(text, args) {
                Ok(s) => Ok(s),
                Err(FormatError::Lookup) => Ok(text.to_owned()),
                Err(e) => Err(other(e)),
            },
            Err(e) => Err(other(e)),
        }
    }
}

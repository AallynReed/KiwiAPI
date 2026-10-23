//! `app/i18n.py`: English is the key; each language merges the website's
//! catalog with the bot overlay (overlay wins), keys whitespace-normalized.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
};

use kiwi_core::pystr::is_space;
use serde_json::Value;

use super::pyfmt;

pub const LANGS: [(&str, &str); 10] = [
    ("en", "English"),
    ("fr", "Français"),
    ("de", "Deutsch"),
    ("pt-PT", "Português"),
    ("es", "Español"),
    ("ru", "Русский"),
    ("ja", "日本語"),
    ("ko", "한국어"),
    ("zh-CN", "简体中文"),
    ("th", "ไทย"),
];

pub const DEFAULT_LANG: &str = "en";

/// `normalize_lang`.
pub fn normalize_lang(lang: Option<&str>) -> &'static str {
    LANGS.iter().find(|(c, _)| Some(*c) == lang).map_or(DEFAULT_LANG, |(c, _)| c)
}

/// `_DISCORD_LOCALE` + `discord_locale_to_lang`.
pub fn discord_locale_to_lang(loc: Option<&str>) -> &'static str {
    let Some(loc) = loc.filter(|l| !l.is_empty()) else { return DEFAULT_LANG };
    if let Some((c, _)) = LANGS.iter().find(|(c, _)| *c == loc) {
        return c;
    }
    match loc {
        "en-US" | "en-GB" => "en",
        "pt-BR" => "pt-PT",
        "zh-TW" => "zh-CN",
        "es-ES" | "es-419" => "es",
        _ => DEFAULT_LANG,
    }
}

/// `_norm`: whitespace runs collapse to one space, then strip.
pub fn norm(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut gap = false;
    for c in s.chars() {
        if is_space(c) {
            gap = true;
        } else {
            if gap && !out.is_empty() {
                out.push(' ');
            }
            gap = false;
            out.push(c);
        }
    }
    out
}

pub type Catalog = HashMap<String, String>;

fn load(path: &Path) -> Catalog {
    let raw = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Catalog::new(),
        Err(e) => {
            tracing::warn!("i18n: could not read locale file {}: {e}", path.display());
            return Catalog::new();
        }
    };
    match crate::pyjson::loads(&raw) {
        Ok(Value::Object(m)) => m
            .into_iter()
            .filter_map(|(k, v)| match v {
                Value::String(s) if !s.is_empty() => Some((norm(&k), s)),
                _ => None,
            })
            .collect(),
        Ok(_) => Catalog::new(),
        Err(e) => {
            tracing::warn!("i18n: could not read locale file {}: {e:?}", path.display());
            Catalog::new()
        }
    }
}

/// The per-language catalogs under one repo root (`site/static/locales`, then
/// the `app/locales` overlay), each read once.
pub struct Catalogs {
    site: PathBuf,
    overlay: PathBuf,
    cache: Mutex<HashMap<&'static str, Arc<Catalog>>>,
}

impl Catalogs {
    pub fn new(root: impl AsRef<Path>) -> Self {
        let root = root.as_ref();
        Self::with_dirs(root.join("site/static/locales"), root.join("app/locales"))
    }

    pub fn with_dirs(site: impl Into<PathBuf>, overlay: impl Into<PathBuf>) -> Self {
        Catalogs { site: site.into(), overlay: overlay.into(), cache: Mutex::new(HashMap::new()) }
    }

    /// The process catalogs, relative to `KIWI_I18N_ROOT` (default: the
    /// working directory, `/app` in the container).
    pub fn global() -> &'static Catalogs {
        static G: OnceLock<Catalogs> = OnceLock::new();
        G.get_or_init(|| Catalogs::new(std::env::var("KIWI_I18N_ROOT").unwrap_or_else(|_| ".".into())))
    }

    pub fn catalog(&self, lang: &'static str) -> Arc<Catalog> {
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        cache
            .entry(lang)
            .or_insert_with(|| {
                if lang == DEFAULT_LANG {
                    return Arc::new(Catalog::new());
                }
                let mut merged = load(&self.site.join(format!("{lang}.json")));
                merged.extend(load(&self.overlay.join(format!("{lang}.json"))));
                Arc::new(merged)
            })
            .clone()
    }

    pub fn tr(&self, lang: Option<&str>) -> Tr {
        let lang = normalize_lang(lang);
        Tr { lang, cat: self.catalog(lang) }
    }
}

/// `t()` bound to one language (Python's context-variable language).
#[derive(Clone)]
pub struct Tr {
    pub lang: &'static str,
    cat: Arc<Catalog>,
}

impl Tr {
    pub fn english() -> Tr {
        Tr { lang: DEFAULT_LANG, cat: Arc::new(Catalog::new()) }
    }

    fn lookup<'a>(&'a self, text: &'a str) -> &'a str {
        if self.lang == DEFAULT_LANG {
            return text;
        }
        self.cat.get(&norm(text)).map_or(text, String::as_str)
    }

    /// `t(text)`.
    pub fn t(&self, text: &str) -> String {
        self.lookup(text).to_owned()
    }

    /// `t(text, **fmt)`: a translation that does not format falls back to the
    /// English source, and that to the unformatted source.
    pub fn tf(&self, text: &str, fmt: &[(&str, &str)]) -> String {
        if fmt.is_empty() {
            return self.t(text);
        }
        pyfmt::format(self.lookup(text), fmt).or_else(|_| pyfmt::format(text, fmt)).unwrap_or_else(|_| text.to_owned())
    }
}

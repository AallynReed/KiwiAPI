//! `app/i18n.py`: the bot's server-side translations. English is the key; a
//! language's catalog is the site's `site/static/locales/<lang>.json` with the
//! bot overlay `assets/locales/<lang>.json` on top, keyed by whitespace-collapsed
//! English. The language is passed explicitly ([`Lang`]) instead of a context var.

#[cfg(test)]
mod tests;

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::OnceLock,
};

use mongodb::bson::{Bson, Document, doc};
use serde_json::Value;

use kiwi_core::pystr::{is_space, strip};

use crate::state::AppState;

/// `LANGS`: (code, endonym), the site's language picker order.
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

pub const SITE_LOCALES: &str = "site/static/locales";
pub const BOT_LOCALES: &str = "assets/locales";

/// The supported code equal to `code`, if any.
pub fn supported(code: &str) -> Option<&'static str> {
    LANGS.iter().map(|(c, _)| *c).find(|c| *c == code)
}

/// `normalize_lang`.
pub fn normalize_lang(lang: Option<&str>) -> &'static str {
    lang.and_then(supported).unwrap_or("en")
}

/// `_norm`: runs of whitespace become one space, then stripped.
pub fn norm(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_ws = false;
    for c in s.chars() {
        if is_space(c) {
            if !in_ws {
                out.push(' ');
            }
            in_ws = true;
        } else {
            out.push(c);
            in_ws = false;
        }
    }
    strip(&out).to_owned()
}

fn load(path: &Path, into: &mut HashMap<String, String>) {
    let Ok(bytes) = std::fs::read(path) else { return };
    let Ok(Value::Object(m)) = serde_json::from_slice::<Value>(&bytes) else {
        tracing::warn!("i18n: could not read locale file {}", path.display());
        return;
    };
    for (k, v) in m {
        if let Value::String(v) = v
            && !v.is_empty()
        {
            into.insert(norm(&k), v);
        }
    }
}

/// The merged catalog for `lang` (overlay wins), from `site` and `bot` dirs.
pub fn load_catalog(site: &Path, bot: &Path, lang: &str) -> HashMap<String, String> {
    let mut merged = HashMap::new();
    if lang == "en" || supported(lang).is_none() {
        return merged;
    }
    load(&site.join(format!("{lang}.json")), &mut merged);
    load(&bot.join(format!("{lang}.json")), &mut merged);
    merged
}

type Catalog = HashMap<String, String>;

fn catalog(lang: &'static str) -> &'static Catalog {
    static CATALOGS: OnceLock<Vec<(&'static str, OnceLock<Catalog>)>> = OnceLock::new();
    let all = CATALOGS.get_or_init(|| LANGS.iter().map(|(c, _)| (*c, OnceLock::new())).collect());
    let slot = &all.iter().find(|(c, _)| *c == lang).expect("supported language").1;
    let root = if cfg!(test) { PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..") } else { PathBuf::new() };
    slot.get_or_init(|| load_catalog(&root.join(SITE_LOCALES), &root.join(BOT_LOCALES), lang))
}

/// `str.format` raised (`KeyError`, `IndexError` or `ValueError`).
#[derive(Debug, PartialEq, Eq)]
pub struct FormatError;

/// Python `str.format(**kwargs)` for plain `{name}` fields; `Err` where
/// Python raises (missing key, positional field, malformed braces).
pub fn py_format(s: &str, args: &[(&str, &str)]) -> Result<String, FormatError> {
    let mut out = String::with_capacity(s.len() + 16);
    let mut it = s.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        match c {
            '{' => {
                if it.peek().map(|&(_, n)| n) == Some('{') {
                    it.next();
                    out.push('{');
                    continue;
                }
                let start = i + 1;
                let mut depth = 1;
                let mut end = None;
                for (j, d) in it.by_ref() {
                    match d {
                        '{' => depth += 1,
                        '}' => {
                            depth -= 1;
                            if depth == 0 {
                                end = Some(j);
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                let field = &s[start..end.ok_or(FormatError)?];
                let (head, spec) = match field.split_once(':') {
                    Some((h, sp)) => (h, Some(sp)),
                    None => (field, None),
                };
                let (name, conv) = match head.split_once('!') {
                    Some((n, c)) => (n, Some(c)),
                    None => (head, None),
                };
                if name.is_empty() || name.contains(['.', '[', '{', '}']) || name.chars().all(|c| c.is_ascii_digit()) {
                    return Err(FormatError);
                }
                let (_, v) = args.iter().find(|(k, _)| *k == name).ok_or(FormatError)?;
                let v = match conv {
                    None | Some("s") => (*v).to_owned(),
                    Some("r") => kiwi_core::pystr::repr(v),
                    Some(_) => return Err(FormatError),
                };
                match spec {
                    None => out.push_str(&v),
                    Some(sp) => out.push_str(&format_str_spec(&v, sp)?),
                }
            }
            '}' => {
                if it.peek().map(|&(_, n)| n) == Some('}') {
                    it.next();
                    out.push('}');
                } else {
                    return Err(FormatError);
                }
            }
            _ => out.push(c),
        }
    }
    Ok(out)
}

/// `format(s, spec)` for a `str`: fill, align, width, precision, type `s`.
fn format_str_spec(v: &str, spec: &str) -> Result<String, FormatError> {
    if spec.contains(['{', '}']) {
        return Err(FormatError);
    }
    let chars: Vec<char> = spec.chars().collect();
    let is_align = |c: char| matches!(c, '<' | '>' | '^' | '=');
    let (mut fill, mut align, mut i) = (' ', '<', 0);
    if chars.len() >= 2 && is_align(chars[1]) {
        (fill, align, i) = (chars[0], chars[1], 2);
    } else if chars.first().is_some_and(|c| is_align(*c)) {
        (align, i) = (chars[0], 1);
    }
    if align == '=' || chars.get(i).is_some_and(|c| matches!(c, '+' | '-' | ' ' | 'z' | '#')) {
        return Err(FormatError);
    }
    if chars.get(i) == Some(&'0') {
        if i == 0 {
            fill = '0';
        }
        i += 1;
    }
    let digits = |i: &mut usize| {
        let mut n = 0usize;
        while let Some(d) = chars.get(*i).and_then(|c| c.to_digit(10)) {
            n = n.saturating_mul(10).saturating_add(d as usize);
            *i += 1;
        }
        n
    };
    let width = digits(&mut i);
    if chars.get(i).is_some_and(|c| matches!(c, ',' | '_')) {
        return Err(FormatError);
    }
    let mut precision = None;
    if chars.get(i) == Some(&'.') {
        i += 1;
        let st = i;
        let p = digits(&mut i);
        if i == st {
            return Err(FormatError);
        }
        precision = Some(p);
    }
    if !matches!(&chars[i..], [] | ['s']) {
        return Err(FormatError);
    }
    let body: String = match precision {
        Some(p) => v.chars().take(p).collect(),
        None => v.to_owned(),
    };
    let pad = width.saturating_sub(body.chars().count());
    let (left, right) = match align {
        '>' => (pad, 0),
        '^' => (pad / 2, pad - pad / 2),
        _ => (0, pad),
    };
    let f = |k: usize| fill.to_string().repeat(k);
    Ok(format!("{}{body}{}", f(left), f(right)))
}

/// A language the text is rendered in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Lang(pub &'static str);

impl Default for Lang {
    fn default() -> Self {
        Lang("en")
    }
}

impl Lang {
    pub fn new(code: Option<&str>) -> Lang {
        Lang(normalize_lang(code))
    }

    pub fn code(self) -> &'static str {
        self.0
    }

    fn lookup(self, text: &str) -> Option<&'static str> {
        if self.0 == "en" {
            return None;
        }
        catalog(self.0).get(&norm(text)).map(String::as_str)
    }

    /// `t(text)` without kwargs: no formatting at all.
    pub fn t(self, text: &str) -> String {
        self.lookup(text).unwrap_or(text).to_owned()
    }

    /// `t(text, **fmt)`: the translation formatted, else the English source
    /// formatted, else the raw source.
    pub fn f(self, text: &str, args: &[(&str, &str)]) -> String {
        if args.is_empty() {
            return self.t(text);
        }
        let out = self.lookup(text).unwrap_or(text);
        py_format(out, args).or_else(|_| py_format(text, args)).unwrap_or_else(|_| text.to_owned())
    }
}

/// `_DISCORD_LOCALE`.
const DISCORD_LOCALE: [(&str, &str); 14] = [
    ("en-US", "en"),
    ("en-GB", "en"),
    ("fr", "fr"),
    ("de", "de"),
    ("pt-BR", "pt-PT"),
    ("pt-PT", "pt-PT"),
    ("ru", "ru"),
    ("ja", "ja"),
    ("zh-CN", "zh-CN"),
    ("zh-TW", "zh-CN"),
    ("ko", "ko"),
    ("es-ES", "es"),
    ("es-419", "es"),
    ("th", "th"),
];

/// `discord_locale_to_lang`.
pub fn discord_locale_to_lang(loc: Option<&str>) -> &'static str {
    let Some(loc) = loc.filter(|l| !l.is_empty()) else { return "en" };
    if let Some(c) = supported(loc) {
        return c;
    }
    DISCORD_LOCALE.iter().find(|(k, _)| *k == loc).map_or("en", |(_, v)| v)
}

/// `int(guild_id)` as the lookup does it; `None` where Python raises.
fn guild_int(v: &Value) -> Option<i64> {
    let i = crate::feeds::pyval::py_int(v)?;
    i64::try_from(i).ok()
}

/// The stored `language` of a `guild_configs` document, if it validates.
pub fn stored_language(d: &Document) -> Option<&'static str> {
    match d.get("language") {
        Some(Bson::String(s)) => supported(s),
        None => Some("en"),
        _ => None,
    }
}

/// `guild_language`: the guild's configured language, else the Discord
/// locale mapped, else English. Never fails.
pub async fn guild_language(state: &AppState, guild_id: &Value, fallback: Option<&str>) -> &'static str {
    if crate::features::py_truthy(guild_id) {
        match guild_int(guild_id) {
            Some(gid) => {
                let coll = state.mongo().collection::<Document>("guild_configs");
                match coll.find_one(doc! {"guild_id": gid}).await {
                    Ok(Some(d)) => {
                        if let Some(code) = stored_language(&d) {
                            return code;
                        }
                    }
                    Ok(None) => {}
                    Err(e) => tracing::warn!("i18n: guild_language lookup failed for {guild_id}: {e}"),
                }
            }
            None => tracing::warn!("i18n: guild_language lookup failed for {guild_id}"),
        }
    }
    discord_locale_to_lang(fallback)
}

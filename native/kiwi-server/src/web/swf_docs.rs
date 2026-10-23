//! `app/web/swf_docs.py`: one page per decompiled Flash UI doc under
//! `site/static/swf-docs/`, markdown rendered as markdown-it-py's commonmark
//! preset with tables and raw HTML escaped (comrak, plus markdown-it's
//! `style="text-align:..."` cells), sibling `.md` links rewritten to pages.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::SystemTime;

use minijinja::Value;
use regex::Regex;
use serde_json::{Map, Value as Json, json};

pub const OVERVIEW: &str = "README";
const TITLE: &str = "Trove UI (Flash) Reference";
const DESC: &str = "Decompiled reference for Trove's Flash UI (.swf) files: document classes, \
game-logic classes, and timeline symbols for every screen.";

static DOC_LINK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"href="(?:\./)?([A-Za-z0-9_.-]+)\.md(#[^"]*)?""#).expect("static regex"));
static ALIGN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"<(th|td) align="(left|right|center)">"#).expect("static regex"));

fn path_of(slug: &str) -> String {
    if slug == OVERVIEW { "/swf-docs".to_owned() } else { format!("/swf-docs/{slug}") }
}

/// Python's text-mode read: universal newlines.
fn read_text(p: &Path) -> Option<String> {
    let s = std::fs::read_to_string(p).ok()?;
    Some(if s.contains('\r') { s.replace("\r\n", "\n").replace('\r', "\n") } else { s })
}

/// markdown-it-py `MarkdownIt("commonmark", {"html": False}).enable("table").render(text)`.
pub fn render_markdown(text: &str) -> String {
    let mut opts = comrak::Options::default();
    opts.extension.table = true;
    opts.render.escape = true;
    let html = comrak::markdown_to_html(text, &opts);
    ALIGN.replace_all(&html, r#"<$1 style="text-align:$2">"#).into_owned()
}

#[derive(Clone)]
struct Manifest {
    groups: Vec<Json>,
    by_slug: HashMap<String, Map<String, Json>>,
    checked: Option<String>,
}

pub struct SwfDocs {
    dir: PathBuf,
    manifest: Mutex<Option<(SystemTime, Manifest)>>,
    rendered: Mutex<HashMap<(String, SystemTime), String>>,
}

fn label_of(item: &Map<String, Json>, file: &str) -> String {
    let swf = match item.get("swf") {
        Some(Json::String(s)) if !s.is_empty() => s.clone(),
        _ => file.to_owned(),
    };
    swf.strip_suffix(".swf").map(str::to_owned).unwrap_or(swf)
}

fn truthy_str(v: Option<&Json>) -> Option<String> {
    match v {
        Some(Json::String(s)) if !s.is_empty() => Some(s.clone()),
        _ => None,
    }
}

impl SwfDocs {
    pub fn new(site_root: &Path) -> Self {
        SwfDocs {
            dir: site_root.join("static/swf-docs"),
            manifest: Mutex::new(None),
            rendered: Mutex::new(HashMap::new()),
        }
    }

    fn manifest(&self) -> Option<Manifest> {
        let index = self.dir.join("index.json");
        let mtime = std::fs::metadata(&index).and_then(|m| m.modified()).ok()?;
        let mut memo = self.manifest.lock().ok()?;
        if let Some((at, m)) = memo.as_ref()
            && *at == mtime
        {
            return Some(m.clone());
        }
        let data: Json = serde_json::from_str(&read_text(&index)?).ok()?;
        let mut groups = Vec::new();
        let mut by_slug = HashMap::new();
        let empty = Vec::new();
        let gs = data.get("groups").and_then(Json::as_array).unwrap_or(&empty);
        for g in gs {
            let mut items = Vec::new();
            for it in g.get("items").and_then(Json::as_array).unwrap_or(&empty) {
                let Some(obj) = it.as_object() else { continue };
                let file = truthy_str(obj.get("file")).unwrap_or_default();
                let Some(slug) = file.strip_suffix(".md") else { continue };
                let mut entry = obj.clone();
                entry.insert("slug".into(), json!(slug));
                entry.insert("url".into(), json!(path_of(slug)));
                entry.insert("label".into(), json!(label_of(obj, &file)));
                by_slug.insert(slug.to_lowercase(), entry.clone());
                items.push(Json::Object(entry));
            }
            let title = truthy_str(g.get("title")).unwrap_or_default();
            let shared = g.get("kind").and_then(Json::as_str) == Some("shared");
            groups.push(json!({"title": title, "shared": shared, "docs": items}));
        }
        let checked = truthy_str(data.get("checked"));
        let m = Manifest { groups, by_slug, checked };
        *memo = Some((mtime, m.clone()));
        Some(m)
    }

    fn render(&self, slug: &str, mtime: SystemTime) -> Option<String> {
        let key = (slug.to_owned(), mtime);
        if let Some(html) = self.rendered.lock().ok()?.get(&key) {
            return Some(html.clone());
        }
        let html = render_markdown(&read_text(&self.dir.join(format!("{slug}.md")))?);
        let html = DOC_LINK
            .replace_all(&html, |c: &regex::Captures| {
                format!("href=\"{}{}\"", path_of(&c[1]), c.get(2).map_or("", |m| m.as_str()))
            })
            .into_owned();
        let mut cache = self.rendered.lock().ok()?;
        if cache.len() >= 256 {
            cache.clear();
        }
        cache.insert(key, html.clone());
        Some(html)
    }

    /// `doc_paths()`: every doc page, for the sitemap.
    pub fn doc_paths(&self) -> Vec<String> {
        let Some(m) = self.manifest() else { return Vec::new() };
        m.groups
            .iter()
            .flat_map(|g| g["docs"].as_array().cloned().unwrap_or_default())
            .filter_map(|e| e.get("url").and_then(Json::as_str).map(str::to_owned))
            .collect()
    }

    /// `page_context(slug)`; `None` when the slug is not a published doc.
    pub fn page_context(&self, slug: Option<&str>, app_url: &str) -> Option<Vec<(&'static str, Value)>> {
        let m = self.manifest()?;
        let (entry, real) = match slug {
            None => (None, OVERVIEW.to_owned()),
            Some(s) => {
                let e = m.by_slug.get(&s.to_lowercase())?;
                (Some(e), e.get("slug").and_then(Json::as_str).unwrap_or("").to_owned())
            }
        };
        let md = self.dir.join(format!("{real}.md"));
        let meta = std::fs::metadata(&md).ok().filter(|m| m.is_file())?;
        let base = app_url.trim_end_matches('/');
        let url = format!("{base}{}", path_of(&real));
        let (title, desc) = match entry {
            Some(e) => {
                let cls = truthy_str(e.get("mainClass"));
                let name = truthy_str(e.get("swf")).unwrap_or_else(|| real.clone());
                let title = match cls {
                    Some(c) if c != "\u{2014}" => format!("{name} ({c}) - {TITLE}"),
                    _ => format!("{name} - {TITLE}"),
                };
                let summary = truthy_str(e.get("summary")).unwrap_or_else(|| DESC.to_owned());
                (title, kiwi_core::pystr::prefix(&summary, 300).to_owned())
            }
            None => (format!("{TITLE} - every Trove .swf screen decompiled"), DESC.to_owned()),
        };
        let mut crumbs =
            vec![json!({"@type": "ListItem", "position": 1, "name": TITLE, "item": format!("{base}/swf-docs")})];
        if let Some(e) = entry {
            let name = truthy_str(e.get("swf")).unwrap_or_else(|| real.clone());
            crumbs.push(json!({"@type": "ListItem", "position": 2, "name": name, "item": url}));
        }
        let ld = json!({"@context": "https://schema.org", "@graph": [
            {"@type": "TechArticle", "headline": title, "description": desc, "url": url,
             "inLanguage": "en", "isPartOf": {"@id": format!("{base}/#website")}},
            {"@type": "BreadcrumbList", "itemListElement": crumbs},
        ]});
        let html = self.render(&real, meta.modified().ok()?)?;
        let count: usize = m.groups.iter().map(|g| g["docs"].as_array().map_or(0, Vec::len)).sum();
        let updated = m.checked.as_deref().and_then(|stamp| {
            let w = kiwi_core::pydt::fromisoformat(&stamp.replace('Z', "+00:00")).ok()?;
            const MONTHS: [&str; 12] =
                ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
            let readable = format!(
                "{:02} {} {}, {:02}:{:02} UTC",
                w.day,
                MONTHS[(w.month - 1).clamp(0, 11) as usize],
                w.year,
                w.hour,
                w.minute
            );
            Some(Value::from(vec![Value::from(stamp), Value::from(readable)]))
        });
        Some(vec![
            ("doc_slug", Value::from(real.clone())),
            ("doc_file", Value::from(format!("{real}.md"))),
            ("doc_title", Value::from(title)),
            ("doc_desc", Value::from(desc)),
            ("doc_url", Value::from(url)),
            ("doc_html", Value::from(html)),
            ("doc_groups", Value::from_serialize(&m.groups)),
            ("doc_ld", Value::from(crate::pyjson::dumps_spaced_unicode(&ld).replace('<', "\\u003c"))),
            ("doc_count", Value::from(count)),
            ("doc_updated", updated.unwrap_or(Value::from(()))),
        ])
    }
}

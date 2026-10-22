//! `app/embed_templates.py`: a user-editable embed template plus a flat
//! variable context renders a Discord embed. Shared by the outbound webhooks
//! and the bot's per-guild announcements.

#[cfg(test)]
pub(crate) mod tests;

use std::sync::LazyLock;

use kiwi_core::pystr::{prefix, strip};
use mongodb::bson::{Bson, Document, doc};
use regex::{Captures, Regex};
use serde_json::{Map, Value, json};

use crate::{
    features::py_truthy,
    feeds::pyval::{py_int, py_str},
    validation::Field,
};

/// The filename a template's Image Studio render is uploaded under.
pub const EMBED_IMAGE_ATTACHMENT: &str = "embed-image.png";

const MAX_FIELDS: usize = 25;
const TITLE: usize = 256;
const DESCRIPTION: usize = 4096;
const FIELD_NAME: usize = 256;
const FIELD_VALUE: usize = 1024;
const FOOTER: usize = 2048;
const CONTENT: usize = 2000;
const ZWSP: &str = "\u{200b}";

static VAR_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\{([a-zA-Z0-9_]+)(?::([a-zA-Z]))?\}").expect("placeholder regex"));
static HEX6: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[0-9a-fA-F]{6}$").expect("hex regex"));

pub type Ctx = Map<String, Value>;

#[derive(Clone, Debug, PartialEq)]
pub struct EmbedField {
    pub name: String,
    pub value: String,
    pub inline: bool,
}

impl Default for EmbedField {
    fn default() -> Self {
        EmbedField { name: String::new(), value: String::new(), inline: true }
    }
}

impl EmbedField {
    pub fn new(name: &str, value: &str, inline: bool) -> Self {
        EmbedField { name: name.into(), value: value.into(), inline }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct EmbedTemplate {
    pub enabled: bool,
    pub content: Option<String>,
    pub title: Option<String>,
    pub url: Option<String>,
    pub description: Option<String>,
    pub color: Option<String>,
    pub fields: Vec<EmbedField>,
    pub footer: Option<String>,
    pub show_image: bool,
    pub image_design_id: Option<String>,
    pub image_url: Option<String>,
}

impl Default for EmbedTemplate {
    fn default() -> Self {
        EmbedTemplate {
            enabled: false,
            content: None,
            title: None,
            url: None,
            description: None,
            color: None,
            fields: Vec::new(),
            footer: None,
            show_image: true,
            image_design_id: None,
            image_url: None,
        }
    }
}

fn opt_bson(v: &Option<String>) -> Bson {
    v.as_ref().map_or(Bson::Null, |s| Bson::String(s.clone()))
}

fn bson_str(d: &Document, key: &str) -> Option<String> {
    match d.get(key) {
        Some(Bson::String(s)) => Some(s.clone()),
        _ => None,
    }
}

fn bson_bool(d: &Document, key: &str, default: bool) -> bool {
    match d.get(key) {
        Some(Bson::Boolean(b)) => *b,
        Some(Bson::Int32(i)) => *i != 0,
        Some(Bson::Int64(i)) => *i != 0,
        _ => default,
    }
}

impl EmbedTemplate {
    /// `template_to_dict` (`model_dump`).
    pub fn to_value(&self) -> Value {
        let fields: Vec<Value> =
            self.fields.iter().map(|f| json!({"name": f.name, "value": f.value, "inline": f.inline})).collect();
        json!({
            "enabled": self.enabled,
            "content": self.content,
            "title": self.title,
            "url": self.url,
            "description": self.description,
            "color": self.color,
            "fields": fields,
            "footer": self.footer,
            "show_image": self.show_image,
            "image_design_id": self.image_design_id,
            "image_url": self.image_url,
        })
    }

    /// The stored sub-document, in model field order.
    pub fn to_bson(&self) -> Document {
        let fields: Vec<Bson> = self
            .fields
            .iter()
            .map(|f| Bson::Document(doc! {"name": &f.name, "value": &f.value, "inline": f.inline}))
            .collect();
        doc! {
            "enabled": self.enabled,
            "content": opt_bson(&self.content),
            "title": opt_bson(&self.title),
            "url": opt_bson(&self.url),
            "description": opt_bson(&self.description),
            "color": opt_bson(&self.color),
            "fields": fields,
            "footer": opt_bson(&self.footer),
            "show_image": self.show_image,
            "image_design_id": opt_bson(&self.image_design_id),
            "image_url": opt_bson(&self.image_url),
        }
    }

    /// A stored template (missing keys take the model defaults).
    pub fn from_bson(d: &Document) -> Self {
        let fields = match d.get("fields") {
            Some(Bson::Array(a)) => a
                .iter()
                .filter_map(Bson::as_document)
                .map(|f| EmbedField {
                    name: bson_str(f, "name").unwrap_or_default(),
                    value: bson_str(f, "value").unwrap_or_default(),
                    inline: bson_bool(f, "inline", true),
                })
                .collect(),
            _ => Vec::new(),
        };
        EmbedTemplate {
            enabled: bson_bool(d, "enabled", false),
            content: bson_str(d, "content"),
            title: bson_str(d, "title"),
            url: bson_str(d, "url"),
            description: bson_str(d, "description"),
            color: bson_str(d, "color"),
            fields,
            footer: bson_str(d, "footer"),
            show_image: bson_bool(d, "show_image", true),
            image_design_id: bson_str(d, "image_design_id"),
            image_url: bson_str(d, "image_url"),
        }
    }

    /// pydantic validation of an `EmbedTemplate` in a request body.
    pub fn validate(field: Field<'_>) -> Self {
        field
            .model(|m| EmbedTemplate {
                enabled: m.field("enabled").bool().default(false),
                content: m.field("content").str().optional(),
                title: m.field("title").str().optional(),
                url: m.field("url").str().optional(),
                description: m.field("description").str().optional(),
                color: m.field("color").str().optional(),
                fields: m
                    .field("fields")
                    .list(|f| {
                        f.model(|m| EmbedField {
                            name: m.field("name").str().default(String::new()),
                            value: m.field("value").str().default(String::new()),
                            inline: m.field("inline").bool().default(true),
                        })
                        .required()
                    })
                    .default(Vec::new()),
                footer: m.field("footer").str().optional(),
                show_image: m.field("show_image").bool().default(true),
                image_design_id: m.field("image_design_id").str().optional(),
                image_url: m.field("image_url").str().optional(),
            })
            .required()
    }

    /// The Image Studio design delivery renders and attaches, if any.
    pub fn attached_design(tmpl: Option<&EmbedTemplate>) -> Option<String> {
        tmpl.filter(|t| t.enabled && t.show_image).and_then(|t| t.image_design_id.clone())
    }
}

/// `substitute`: `{var}` / `{unix_var:R}` placeholders; unknown vars are ''.
pub fn substitute(text: Option<&str>, ctx: &Ctx) -> String {
    let Some(text) = text.filter(|t| !t.is_empty()) else {
        return String::new();
    };
    VAR_RE
        .replace_all(text, |c: &Captures<'_>| {
            let Some(val) = ctx.get(&c[1]) else {
                return String::new();
            };
            if let Some(style) = c.get(2).map(|m| m.as_str()).filter(|s| "tTdDfFR".contains(s))
                && let Some(i) = py_int(val)
            {
                return format!("<t:{i}:{style}>");
            }
            if val.is_null() { String::new() } else { py_str(val) }
        })
        .into_owned()
}

/// `parse_color`: `#RRGGBB`, `RRGGBB` or a decimal string.
pub fn parse_color(c: Option<&str>) -> Option<i128> {
    let c = c.filter(|c| !c.is_empty())?;
    let s = strip(c).trim_start_matches('#');
    if HEX6.is_match(s) {
        return i128::from_str_radix(s, 16).ok();
    }
    kiwi_core::feeds::py::int(s, 10)
}

fn int_value(i: i128) -> Value {
    if let Ok(v) = i64::try_from(i) {
        Value::from(v)
    } else if let Ok(v) = u64::try_from(i) {
        Value::from(v)
    } else {
        Value::from(i as f64)
    }
}

/// `render_template`: the embed object and the optional message content.
pub fn render_template(tmpl: &EmbedTemplate, ctx: &Ctx, default_image_url: Option<&Value>) -> (Ctx, Option<String>) {
    let mut embed = Map::new();
    let title = substitute(tmpl.title.as_deref(), ctx);
    let title = strip(&title);
    if !title.is_empty() {
        embed.insert("title".into(), prefix(title, TITLE).into());
    }
    let url = substitute(tmpl.url.as_deref(), ctx);
    let url = strip(&url);
    if url.starts_with("http") {
        embed.insert("url".into(), url.into());
    }
    let desc = substitute(tmpl.description.as_deref(), ctx);
    let desc = strip(&desc);
    if !desc.is_empty() {
        embed.insert("description".into(), prefix(desc, DESCRIPTION).into());
    }
    if let Some(color) = parse_color(tmpl.color.as_deref()) {
        embed.insert("color".into(), int_value(color));
    }
    let mut fields = Vec::new();
    for f in tmpl.fields.iter().take(MAX_FIELDS) {
        let n = substitute(Some(&f.name), ctx);
        let n = prefix(strip(&n), FIELD_NAME);
        let v = substitute(Some(&f.value), ctx);
        let v = prefix(strip(&v), FIELD_VALUE);
        if !n.is_empty() || !v.is_empty() {
            fields.push(json!({
                "name": if n.is_empty() { ZWSP } else { n },
                "value": if v.is_empty() { ZWSP } else { v },
                "inline": f.inline,
            }));
        }
    }
    if !fields.is_empty() {
        embed.insert("fields".into(), Value::Array(fields));
    }
    let footer = substitute(tmpl.footer.as_deref(), ctx);
    let footer = strip(&footer);
    if !footer.is_empty() {
        embed.insert("footer".into(), json!({"text": prefix(footer, FOOTER)}));
    }
    if tmpl.show_image {
        if tmpl.image_design_id.as_deref().is_some_and(|d| !d.is_empty()) {
            embed.insert("image".into(), json!({"url": format!("attachment://{EMBED_IMAGE_ATTACHMENT}")}));
        } else {
            let own = tmpl.image_url.as_deref().map(strip).filter(|s| !s.is_empty());
            let img = match own {
                Some(s) => Some(Value::String(s.to_owned())),
                None => default_image_url.cloned(),
            };
            if let Some(img) = img.filter(py_truthy) {
                embed.insert("image".into(), json!({"url": img}));
            }
        }
    }
    let content = substitute(tmpl.content.as_deref(), ctx);
    let content = strip(&content);
    let content = (!content.is_empty()).then(|| prefix(content, CONTENT).to_owned());
    (embed, content)
}

/// `{"embeds": [embed]}` plus `content` when there is any.
pub fn message_body(embed: Ctx, content: Option<String>) -> Value {
    let mut body = Map::new();
    body.insert("embeds".into(), Value::Array(vec![Value::Object(embed)]));
    if let Some(c) = content {
        body.insert("content".into(), Value::String(c));
    }
    Value::Object(body)
}

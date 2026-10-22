//! `app/webhooks/embeds.py`: per-event variable contexts, the default
//! templates and the editor's sample contexts. Every context mirrors the
//! Python dict built from the bus payload; an `Err` is the exception Python
//! raises on a payload of the wrong shape.

use serde_json::{Map, Value, json};

use super::model::EVENT_TYPES;
use crate::{
    embed_templates::{Ctx, EmbedField, EmbedTemplate, message_body, render_template},
    outbound::py::{commas, fixed, get, get_or, iter, join, or, py_float, py_int, py_str, title, truthy, type_name},
};

pub const SITE: &str = "https://trove.aallyn.net";

const COLOR_CHALLENGE: &str = "#F2A33C";
const COLOR_MOD_RELEASE: &str = "#46D39A";
const COLOR_GAME_UPDATE: &str = "#5EC6FF";

fn s(v: &str) -> Value {
    Value::String(v.to_owned())
}

/// `_num`: `f"{int(v):,}"`, `"0"` when `int()` fails.
fn num(v: Option<&Value>) -> String {
    v.and_then(py_int).map_or_else(|| "0".to_owned(), commas)
}

/// `_trunc`.
fn trunc(v: &Value, n: usize) -> String {
    let text = if truthy(v) { py_str(v) } else { String::new() };
    let text = kiwi_core::pystr::strip(&text);
    if text.chars().count() <= n {
        return text.to_owned();
    }
    let head = kiwi_core::pystr::prefix(text, n - 1).trim_end_matches(kiwi_core::pystr::is_space);
    format!("{head}\u{2026}")
}

/// `_bytes`.
fn bytes_label(v: Option<&Value>) -> anyhow::Result<String> {
    let Some(b) = v.and_then(py_float) else {
        return Ok(String::new());
    };
    if b < 1024.0 {
        anyhow::ensure!(b.is_finite(), "OverflowError: cannot convert float infinity to integer");
        return Ok(format!("{} B", b.trunc() as i128));
    }
    if b < 1024.0 * 1024.0 {
        return Ok(format!("{} KB", fixed(b / 1024.0, 0)));
    }
    Ok(format!("{} MB", fixed(b / (1024.0 * 1024.0), 2)))
}

/// `_signed_bytes`.
fn signed_bytes(v: Option<&Value>) -> anyhow::Result<String> {
    let Some(b) = v.and_then(py_int) else {
        return Ok(String::new());
    };
    if b == 0 {
        return Ok("no change".into());
    }
    let sign = if b > 0 { "+" } else { "-" };
    Ok(format!("{sign}{}", bytes_label(Some(&json!(b.unsigned_abs() as f64)))?))
}

fn empty_dict() -> Value {
    Value::Object(Map::new())
}

fn challenge_context(d: &Value) -> anyhow::Result<Ctx> {
    let name = get_or(d, "name", &s("Challenge"))?.clone();
    let ctype = get_or(d, "type", &s(""))?.clone();
    let label = match &ctype {
        Value::String(t) => title(&t.replace('_', " ")),
        other => anyhow::bail!("AttributeError: '{}' object has no attribute 'replace'", type_name(other)),
    };
    let mut c = Map::new();
    c.insert("name".into(), name);
    c.insert("type".into(), ctype);
    c.insert("type_label".into(), s(if label.is_empty() { "Challenge" } else { &label }));
    c.insert("starts_at".into(), get(d, "starts_at")?.cloned().unwrap_or(Value::Null));
    c.insert("ends_at".into(), get(d, "ends_at")?.cloned().unwrap_or(Value::Null));
    c.insert("status".into(), s(if get(d, "active")?.is_some_and(truthy) { "live" } else { "upcoming" }));
    c.insert("is_friday".into(), Value::Bool(get(d, "is_friday_window")?.is_some_and(truthy)));
    Ok(c)
}

fn challenge_default() -> EmbedTemplate {
    EmbedTemplate {
        title: Some("\u{2694}\u{fe0f} New challenge \u{2014} {name}".into()),
        url: Some(format!("{SITE}/server-time")),
        description: Some("A new hourly challenge is live in Trove.".into()),
        color: Some(COLOR_CHALLENGE.into()),
        fields: vec![EmbedField::new("Type", "{type_label}", true), EmbedField::new("Ends", "{ends_at:R}", true)],
        footer: Some("trove.aallyn.net".into()),
        show_image: false,
        ..Default::default()
    }
}

fn challenge_sample(_asset: &str) -> Ctx {
    challenge_context(&json!({
        "name": "Racing Challenge", "type": "racing",
        "starts_at": 1718200000, "ends_at": 1718201200,
        "active": true, "is_friday_window": false,
    }))
    .expect("sample")
}

/// `(v or "")[:n]` on a str or list.
fn slice(v: &Value, n: usize) -> anyhow::Result<Value> {
    Ok(match v {
        _ if !truthy(v) => s(""),
        Value::String(t) => s(kiwi_core::pystr::prefix(t, n)),
        Value::Array(a) => Value::Array(a.iter().take(n).cloned().collect()),
        other => anyhow::bail!("TypeError: '{}' object is not subscriptable", type_name(other)),
    })
}

fn mod_release_context(d: &Value) -> anyhow::Result<Ctx> {
    let empty = empty_dict();
    let project = get_or(d, "project", &empty)?;
    let release = get_or(d, "release", &empty)?;
    let previous = get_or(d, "previous", &empty)?;
    let change = get_or(d, "change", &empty)?;
    let fmt = py_str(get_or(release, "format", &s(""))?);
    let prev_tag = get_or(previous, "tag", &s(""))?.clone();
    let tag = get_or(release, "tag", &s(""))?.clone();
    let none = Value::Null;

    let title_v = or(get(project, "title")?, or(get(project, "slug")?, &s("Mod"))).clone();
    let owner = get_or(project, "owner", &s("unknown"))?.clone();
    let author = or(get(project, "author")?, or(get(project, "owner")?, &s("unknown"))).clone();
    let tags: Vec<Value> = iter(get_or(project, "tags", &json!([]))?)?.into_iter().filter(truthy).collect();
    let sha = get_or(release, "sha256", &s(""))?.clone();

    let mut c = Map::new();
    c.insert("project_title".into(), title_v);
    c.insert("project_slug".into(), get_or(project, "slug", &s(""))?.clone());
    c.insert("handle".into(), get_or(project, "handle", &s(""))?.clone());
    c.insert("owner".into(), owner);
    c.insert("author".into(), author);
    c.insert("summary".into(), s(&trunc(get_or(project, "summary", &s(""))?, 200)));
    c.insert("tags".into(), s(&join(&tags, ", ")?));
    c.insert("downloads".into(), s(&num(get(project, "download_count")?)));
    c.insert("stars".into(), s(&num(get(project, "star_count")?)));
    c.insert("tag".into(), tag.clone());
    c.insert("release_title".into(), or(get(release, "title")?, or(Some(&tag), &s("New release"))).clone());
    c.insert("branch".into(), get_or(release, "branch", &s(""))?.clone());
    c.insert("format".into(), s(&fmt));
    c.insert("format_upper".into(), s(&fmt.to_uppercase()));
    c.insert("filename".into(), get_or(release, "filename", &s(""))?.clone());
    c.insert("mod_version".into(), get_or(release, "mod_version", &s(""))?.clone());
    c.insert("sha256".into(), sha.clone());
    c.insert("sha_short".into(), slice(&sha, 12)?);
    c.insert("size".into(), get(release, "size")?.unwrap_or(&none).clone());
    c.insert("size_label".into(), s(&bytes_label(get(release, "size")?)?));
    c.insert("changelog".into(), s(&trunc(get_or(release, "changelog", &s(""))?, 1000)));
    c.insert("previous_tag".into(), prev_tag.clone());
    let version_change =
        if truthy(&prev_tag) { s(&format!("{} \u{2192} {}", py_str(&prev_tag), py_str(&tag))) } else { tag.clone() };
    c.insert("version_change".into(), version_change);
    c.insert("size_delta".into(), s(&signed_bytes(get(change, "size_delta")?)?));
    c.insert("release_count".into(), s(&num(get(change, "release_count")?)));
    let first = get(change, "is_first_release")?.is_some_and(truthy);
    c.insert("update_kind".into(), s(if first { "First release" } else { "Update" }));
    c.insert("download_url".into(), get_or(d, "download_url", &s(""))?.clone());
    c.insert("page_url".into(), get_or(d, "page_url", &s(SITE))?.clone());
    c.insert("image_url".into(), get_or(d, "image_url", &s(""))?.clone());
    Ok(c)
}

fn mod_release_default() -> EmbedTemplate {
    EmbedTemplate {
        title: Some("\u{1f4e6} {project_title} \u{2014} {release_title}".into()),
        url: Some("{page_url}".into()),
        description: Some("{changelog}".into()),
        color: Some(COLOR_MOD_RELEASE.into()),
        fields: vec![
            EmbedField::new("Version", "{version_change}", true),
            EmbedField::new("Variant", "{branch}", true),
            EmbedField::new("Format", "{format_upper}", true),
            EmbedField::new("Size", "{size_label}", true),
            EmbedField::new("Download", "[Get it]({download_url})", true),
        ],
        footer: Some("Mods Hub \u{b7} by {author}".into()),
        show_image: true,
        ..Default::default()
    }
}

fn mod_release_sample(asset: &str) -> Ctx {
    mod_release_context(&json!({
        "project": {"title": "Neon HUD", "slug": "neon-hud", "handle": "aallyn",
                    "owner": "Aallyn", "author": "Aallyn",
                    "summary": "A cleaner, higher-contrast HUD.",
                    "tags": ["ui", "hud"], "download_count": 4820, "star_count": 96},
        "release": {"tag": "v1.3.0", "title": "Stable", "branch": "main",
                    "format": "tmod", "changelog": "Cleaner icons + bug fixes.",
                    "filename": "Neon HUD.tmod", "size": 1_482_240,
                    "mod_version": "1.3.0",
                    "sha256": "9f2c1ab4d5e6f708192a3b4c5d6e7f8091a2b3c4d5e6f708192a3b4c5d6e7f80"},
        "previous": {"tag": "v1.2.0"},
        "change": {"size_delta": 40_960, "release_count": 7, "is_first_release": false},
        "image_url": format!("{asset}/static/assets/mod_manager.png"),
        "download_url": format!("{SITE}/mods/aallyn/neon-hud"), "page_url": format!("{SITE}/mods/aallyn/neon-hud"),
    }))
    .expect("sample")
}

fn game_update_context(d: &Value, asset: &str) -> anyhow::Result<Ctx> {
    let empty = empty_dict();
    let v = get_or(d, "version", &empty)?;
    let ordinal = get(v, "ordinal")?.cloned().unwrap_or(Value::Null);
    let mut c = Map::new();
    c.insert("version_tag".into(), get_or(v, "version_tag", &s("new build"))?.clone());
    c.insert("ordinal".into(), ordinal.clone());
    c.insert("branch".into(), get_or(v, "branch", &s(""))?.clone());
    c.insert("files_added".into(), s(&num(get(v, "files_added")?)));
    c.insert("files_modified".into(), s(&num(get(v, "files_modified")?)));
    c.insert("files_removed".into(), s(&num(get(v, "files_removed")?)));
    let image = if ordinal.is_null() {
        Value::Null
    } else {
        s(&format!("{asset}/announce.png?kind=game_update&v={}", py_str(&ordinal)))
    };
    c.insert("image_url".into(), image);
    Ok(c)
}

fn game_update_default() -> EmbedTemplate {
    EmbedTemplate {
        title: Some("\u{1f9e9} New Trove update \u{2014} {version_tag}".into()),
        url: Some(format!("{SITE}/updates")),
        description: Some("A new build is live on the US servers. See what changed:".into()),
        color: Some(COLOR_GAME_UPDATE.into()),
        fields: vec![
            EmbedField::new("Added", "{files_added}", true),
            EmbedField::new("Modified", "{files_modified}", true),
            EmbedField::new("Removed", "{files_removed}", true),
        ],
        footer: Some("Browse the changed files at trove.aallyn.net/updates".into()),
        show_image: true,
        ..Default::default()
    }
}

fn game_update_sample(asset: &str) -> Ctx {
    game_update_context(
        &json!({"version": {
            "version_tag": "2024.06.01", "ordinal": 142, "branch": "live-us",
            "files_added": 12, "files_modified": 8, "files_removed": 1,
        }}),
        asset,
    )
    .expect("sample")
}

pub fn is_event(event_type: &str) -> bool {
    EVENT_TYPES.contains(&event_type)
}

pub fn default_template(event_type: &str) -> Option<EmbedTemplate> {
    Some(match event_type {
        "challenge" => challenge_default(),
        "mod_release" => mod_release_default(),
        "game_update" => game_update_default(),
        _ => return None,
    })
}

pub fn sample_context(event_type: &str, asset: &str) -> Ctx {
    match event_type {
        "challenge" => challenge_sample(asset),
        "mod_release" => mod_release_sample(asset),
        "game_update" => game_update_sample(asset),
        _ => Map::new(),
    }
}

pub fn context(event_type: &str, data: &Value, asset: &str) -> anyhow::Result<Option<Ctx>> {
    Ok(Some(match event_type {
        "challenge" => challenge_context(data)?,
        "mod_release" => mod_release_context(data)?,
        "game_update" => game_update_context(data, asset)?,
        _ => return Ok(None),
    }))
}

/// `variables`: the sample's keys, minus `image_url`.
pub fn variables(event_type: &str, asset: &str) -> Vec<String> {
    sample_context(event_type, asset).keys().filter(|k| *k != "image_url").cloned().collect()
}

fn render_ctx(event_type: &str, ctx: &Ctx, template: Option<&EmbedTemplate>) -> Option<Value> {
    let default = default_template(event_type)?;
    let tmpl = template.filter(|t| t.enabled).unwrap_or(&default);
    let (embed, content) = render_template(tmpl, ctx, ctx.get("image_url"));
    Some(message_body(embed, content))
}

/// `render`: the webhook body for one event (`data or {}` as Python passes it).
pub fn render(
    event_type: &str,
    data: &Value,
    template: Option<&EmbedTemplate>,
    asset: &str,
) -> anyhow::Result<Option<Value>> {
    let empty = empty_dict();
    let data = if truthy(data) { data } else { &empty };
    let Some(ctx) = context(event_type, data, asset)? else {
        return Ok(None);
    };
    Ok(render_ctx(event_type, &ctx, template))
}

pub fn render_sample(event_type: &str, template: Option<&EmbedTemplate>, asset: &str) -> Option<Value> {
    render_ctx(event_type, &sample_context(event_type, asset), template)
}

pub fn test_body() -> Value {
    json!({
        "embeds": [{
            "title": "\u{2705} Kiwi webhook connected",
            "color": 0x46D39A,
            "description": "This Discord webhook is wired up. You'll get a message here when your subscribed Trove events fire.",
            "footer": {"text": "trove.aallyn.net"},
        }]
    })
}

/// `event_meta`: the editor's per-event variables, default and sample.
pub fn event_meta(asset: &str) -> Value {
    let items: Vec<Value> = EVENT_TYPES
        .iter()
        .map(|key| {
            json!({
                "key": key,
                "variables": variables(key, asset),
                "default_template": default_template(key).map(|t| t.to_value()),
                "sample": Value::Object(sample_context(key, asset)),
            })
        })
        .collect();
    Value::Array(items)
}

//! The JSON discord.py sends: `Embed.from_dict(d).to_dict()` and the
//! message bodies `channel.send` / `message.edit` build.

use serde_json::{Map, Value, json};

use crate::{embed_templates::EMBED_IMAGE_ATTACHMENT, feeds::pyval::py_str};

type Obj = Map<String, Value>;

/// `Embed.from_dict(d).to_dict()`: the raw sections in slot order, then
/// `color`, `timestamp`, `description`, `url`, `title` (falsy text dropped).
pub fn embed_to_dict(d: &Obj) -> Obj {
    let mut out = Map::new();
    for k in ["footer", "image", "thumbnail", "video", "provider", "author", "fields"] {
        if let Some(v) = d.get(k) {
            out.insert(k.into(), v.clone());
        }
    }
    if let Some(c) = d.get("color") {
        out.insert("color".into(), c.clone());
    }
    if let Some(Value::String(t)) = d.get("timestamp")
        && !t.is_empty()
    {
        let ts =
            crate::dt::parse(t).map_or_else(|| t.clone(), |p| crate::dt::isoformat(&p.with_timezone(&chrono::Utc)));
        out.insert("timestamp".into(), ts.into());
    }
    for k in ["type", "description", "url", "title"] {
        match d.get(k) {
            None | Some(Value::Null) => {}
            Some(v) => {
                let s = py_str(v);
                if !s.is_empty() {
                    out.insert(k.into(), s.into());
                }
            }
        }
    }
    out
}

/// `allowed_mentions`: none, or exactly the ping roles. (discord.py's
/// `AllowedMentions(roles=...)` also re-enabled @everyone and user pings,
/// against the announcer's intent; this sends what it meant.)
pub fn allowed_mentions(roles: &[i64]) -> Value {
    if roles.is_empty() { json!({"parse": []}) } else { json!({"parse": [], "roles": roles}) }
}

/// `channel.send(content=..., embed=..., file=..., allowed_mentions=...)`.
pub fn send_payload(content: Option<&str>, embed: &Obj, roles: Option<&[i64]>, with_file: bool) -> Value {
    let mut p = Map::new();
    p.insert("embeds".into(), json!([embed_to_dict(embed)]));
    p.insert("content".into(), content.map_or(Value::Null, |c| Value::String(c.to_owned())));
    p.insert("components".into(), json!([]));
    p.insert("tts".into(), Value::Bool(false));
    if let Some(roles) = roles {
        p.insert("allowed_mentions".into(), allowed_mentions(roles));
    }
    if with_file {
        p.insert("attachments".into(), json!([{"id": 0, "filename": EMBED_IMAGE_ATTACHMENT}]));
    }
    Value::Object(p)
}

/// `message.edit(embed=...)`.
pub fn edit_payload(embed: &Obj) -> Value {
    json!({"embeds": [embed_to_dict(embed)], "tts": false})
}

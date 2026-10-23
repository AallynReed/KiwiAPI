//! Custom art for Zakros UI (`app/custom_art`): the public request form's data
//! plane, the master's review queue, and the release worker the `kiwi-builder`
//! binary runs.

pub mod admin;
pub mod builder;
pub mod intake;
pub mod measure;
pub mod model;

#[cfg(test)]
mod tests;

use std::collections::HashMap;

use axum::{
    Router,
    routing::{get, post},
};
use kiwi_core::pystr;
use serde_json::{Map, Value, json};

pub use model::{ArtPicture, ArtRequest};

use crate::{
    error::{ApiError, ErrorCode},
    outbound,
    portal::account::html_escape,
    state::AppState,
};

pub const COLLECTION: &str = "art_requests";
pub const CHAT: &str = "Zakros UI - Chat";
pub const NAMEPLATE: &str = "Zakros UI - Nameplate";
pub const CLUBS: &str = "Zakros UI - Clubs";
pub const MODS: [&str; 3] = [CHAT, NAMEPLATE, CLUBS];
pub const WIDEST: i64 = 6;
pub const CHANGELOG_MAX: usize = 240;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/site/custom-art/config", get(intake::form_config))
        .route("/site/custom-art", post(intake::submit_request))
        .route("/admin/custom-art", get(admin::list))
        .route("/admin/custom-art/{request_id}/image/{slot}", get(admin::image))
        .route("/admin/custom-art/release", post(admin::release))
        .route("/admin/custom-art/steam-done", post(admin::steam_done))
        .route("/admin/custom-art/{request_id}/approve", post(admin::approve))
        .route("/admin/custom-art/{request_id}/deny", post(admin::deny))
        .route("/admin/custom-art/{request_id}/retry", post(admin::retry))
}

/// `SLOTS[kind]`.
pub fn slots(kind: &str) -> &'static [&'static str] {
    match kind {
        "player" => &["pfp"],
        "club" => &["pfp", "banner"],
        _ => &[],
    }
}

/// `LANES[(kind, slot)]`.
pub fn lane_of(kind: &str, slot: &str) -> Option<&'static str> {
    match (kind, slot) {
        ("player", "pfp") => Some("pfp"),
        ("club", "pfp") => Some("club"),
        ("club", "banner") => Some("banner"),
        _ => None,
    }
}

/// `LANE_MODS[lane]`.
pub fn lane_mods(lane: &str) -> &'static [&'static str] {
    match lane {
        "pfp" => &[CHAT],
        "club" => &[CHAT, CLUBS],
        "banner" => &[CHAT, NAMEPLATE, CLUBS],
        _ => &[],
    }
}

/// `LANE_LABELS[lane]`.
pub fn lane_label(lane: &str) -> &'static str {
    match lane {
        "pfp" => "profile picture",
        "club" => "club picture",
        "banner" => "club banner",
        _ => "",
    }
}

const BANNED: &[char] = &['\t', '\n', '\r', '"', ',', '\\', '/', ':', '*', '?', '<', '>', '|'];

/// `clean_name`.
pub fn clean_name(name: &str) -> Result<String, ApiError> {
    let name = pystr::strip(name);
    if name.is_empty() || name.chars().count() > 64 || name.ends_with('.') || name.contains(BANNED) {
        return Err(validation(
            "That name can't be used. Names are up to 64 characters, can't end in a full stop, \
             and can't contain , \" \\ / : * ? < > or |.",
        ));
    }
    Ok(name.to_owned())
}

pub fn validation(message: impl Into<String>) -> ApiError {
    ApiError::new(axum::http::StatusCode::BAD_REQUEST, ErrorCode::ValidationError, message)
}

/// `check_shape`.
pub fn check_shape(kind: &str, slot: &str, width: i64, height: i64) -> Result<(), ApiError> {
    let label = lane_of(kind, slot).map(lane_label).unwrap_or_default();
    if slot == "pfp" && width != height {
        return Err(validation(format!("A {label} has to be square - that one is {width}\u{d7}{height}.")));
    }
    if slot == "banner" && !(height <= width && width <= WIDEST * height) {
        return Err(validation(format!(
            "A {label} has to be from square up to {WIDEST} times as wide as it is tall - that one is \
             {width}\u{d7}{height}."
        )));
    }
    Ok(())
}

/// `lanes(request)`.
pub fn lanes(r: &ArtRequest) -> Vec<&'static str> {
    slots(&r.kind)
        .iter()
        .filter(|s| r.pictures.iter().any(|(k, _)| k == *s))
        .filter_map(|s| lane_of(&r.kind, s))
        .collect()
}

/// `mods_of(request)`.
pub fn mods_of(r: &ArtRequest) -> Vec<&'static str> {
    let lanes = lanes(r);
    MODS.iter().copied().filter(|m| lanes.iter().any(|l| lane_mods(l).contains(m))).collect()
}

/// `what(request, mod)`; None where Python raises (no picture to name).
pub fn what(r: &ArtRequest, mod_: Option<&str>) -> Option<String> {
    let shown: Vec<&str> = lanes(r).into_iter().filter(|l| mod_.is_none_or(|m| lane_mods(l).contains(&m))).collect();
    if shown == ["club", "banner"] {
        return Some("club picture and banner".into());
    }
    shown.first().map(|l| lane_label(l).to_owned())
}

fn join(parts: &[String]) -> String {
    match parts {
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
        [] => String::new(),
    }
}

/// `changelog(requests, mod)`.
pub fn changelog(requests: &[ArtRequest], mod_: &str) -> String {
    let mine: Vec<&ArtRequest> = requests.iter().filter(|r| mods_of(r).contains(&mod_)).collect();
    let parts: Vec<String> =
        mine.iter().map(|r| format!("a {} for {}", what(r, Some(mod_)).unwrap_or_default(), r.name)).collect();
    let text = format!("Added {}.", join(&parts));
    if text.chars().count() <= CHANGELOG_MAX {
        return text;
    }
    let mut counts: Vec<(&str, usize)> = Vec::new();
    for r in &mine {
        for lane in lanes(r).into_iter().filter(|l| lane_mods(l).contains(&mod_)) {
            match counts.iter_mut().find(|(l, _)| *l == lane) {
                Some((_, n)) => *n += 1,
                None => counts.push((lane, 1)),
            }
        }
    }
    let parts: Vec<String> =
        counts.iter().map(|(lane, n)| format!("{n} {}{}", lane_label(lane), if *n == 1 { "" } else { "s" })).collect();
    format!("Added {}.", join(&parts))
}

/// `view(r)`; None where Python raises.
pub fn view(r: &ArtRequest) -> Option<Value> {
    let mut pictures = Map::new();
    for (slot, p) in &r.pictures {
        pictures.insert(slot.clone(), json!({"width": p.width, "height": p.height}));
    }
    let versions: Map<String, Value> = r.versions.iter().map(|(k, v)| (k.clone(), json!(v))).collect();
    Some(json!({
        "id": r.id.to_hex(),
        "kind": r.kind,
        "label": what(r, None)?,
        "name": r.name,
        "email": r.email,
        "note": r.note,
        "mods": mods_of(r),
        "pictures": pictures,
        "status": r.status,
        "reason": r.reason,
        "log": r.log,
        "versions": versions,
        "steam_pending": r.steam_pending,
        "created_at": outbound::iso(&Some(r.created_at)),
        "decided_at": outbound::iso(&r.decided_at),
        "released_at": outbound::iso(&r.released_at),
    }))
}

// -- Email --------------------------------------------------------------------

/// What `_send` hands `queue_email`: subject, text and HTML.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mail {
    pub to: String,
    pub subject: String,
    pub text: String,
    pub html: String,
}

fn wrap(title: &str, body: &str) -> String {
    format!(
        "<div style=\"font-family:system-ui,sans-serif;max-width:480px;margin:0 auto;padding:24px;color:#e8ecf3;\
         background:#0a0e14\"><h1 style='font-size:1.3rem;margin:0 0 12px'>{}</h1>{body}<p style='color:#9aa4b2'>\
         - Better Trove Tools</p></div>",
        html_escape(title)
    )
}

/// `_notify_denied`'s message.
pub fn denied_mail(r: &ArtRequest, app_url: &str) -> Mail {
    let asked = format!("{} for {}", what(r, None).unwrap_or_default(), r.name);
    let subject = "Your custom art request wasn't added";
    let reason = r.reason.clone().unwrap_or_default();
    let text = [
        format!("Your request for a {asked} in Zakros UI wasn't added."),
        String::new(),
        format!("Reason: {}", r.reason.as_deref().unwrap_or("None")),
        String::new(),
        "You're welcome to send a new request with that sorted out.".into(),
        format!("{}/zakros-ui-requests", app_url.trim_end_matches('/')),
        String::new(),
        "- Better Trove Tools".into(),
    ]
    .join("\n");
    let body = format!(
        "<p>Your request for a {} in Zakros UI wasn't added.</p><p style='margin:16px 0 4px;color:#9aa4b2'>Reason</p>\
         <p style='background:#161b22;border:1px solid #232a33;border-radius:8px;padding:12px 14px'>{}</p>\
         <p>You're welcome to send a new request with that sorted out.</p>",
        html_escape(&asked),
        html_escape(&reason)
    );
    Mail { to: r.email.clone(), subject: subject.into(), text, html: wrap(subject, &body) }
}

/// `notify_released`'s message; None where Python raises (a mod without a
/// recorded version).
pub fn released_mail(r: &ArtRequest, pages: &HashMap<String, String>) -> Option<Mail> {
    let asked = format!("{} for {}", what(r, None).unwrap_or_default(), r.name);
    let subject = "Your custom art is in Zakros UI";
    let mods = mods_of(r);
    let version = |m: &str| r.versions.iter().find(|(k, _)| k == m).map(|(_, v)| v.clone());
    let page = |m: &str| pages.get(m).filter(|p| !p.is_empty());
    let mut shipped = Vec::new();
    let mut items = String::new();
    for m in &mods {
        let v = version(m)?;
        shipped.push(format!("{m} {v}") + &page(m).map(|p| format!(": {p}")).unwrap_or_default());
        items.push_str(&format!("<li>{} {}", html_escape(m), html_escape(&v)));
        if let Some(p) = page(m) {
            items.push_str(&format!(" - <a href='{}' style='color:#569cff'>mod page</a>", html_escape(p)));
        }
        items.push_str("</li>");
    }
    let mut lines = vec![format!("Your {asked} has been added. It's in:"), String::new()];
    lines.extend(shipped);
    lines.extend([String::new(), "Update the mod to see it.".into(), String::new(), "- Better Trove Tools".into()]);
    let body = format!(
        "<p>Your {} has been added. It's in:</p><ul>{items}</ul><p>Update the mod to see it.</p>",
        html_escape(&asked)
    );
    Some(Mail { to: r.email.clone(), subject: subject.into(), text: lines.join("\n"), html: wrap(subject, &body) })
}

/// `_send`: a failed queue is a warning, never the caller's error.
pub async fn send(state: &AppState, mail: &Mail) {
    if let Err(e) = crate::email::queue_email(state, &mail.to, &mail.subject, &mail.text, Some(&mail.html)).await {
        tracing::warn!("custom art email to {} failed: {e:#}", mail.to);
    }
}

//! `app/auth/emails.py` + `app/core/email_render.py` + `queue_email` from
//! `app/core/email_outbox.py`: the account mails, their branded HTML (the
//! Jinja2 template rendered with MarkupSafe escaping) and the outbox row.

use chrono::{DateTime, Duration, Utc};
use mongodb::bson::{Bson, Document, doc, oid::ObjectId};
use serde_json::Value;

use crate::{
    auth::{Rejection, jwt, models::User, tokens::password_fingerprint},
    config::Settings,
    dt,
    state::AppState,
};

pub const VERIFY_PURPOSE: &str = "email_verify";
pub const RESET_PURPOSE: &str = "password_reset";
pub const EMAIL_CHANGE_PURPOSE: &str = "email_change";
pub const OUTBOX: &str = "email_outbox";

const BASE: &str = r#"<!doctype html>
<html lang="en">
<body style="margin:0;padding:0;background:#0d1117;
  font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',Roboto,Helvetica,Arial,sans-serif;">
  <table role="presentation" width="100%" cellpadding="0" cellspacing="0"
    style="background:#0d1117;padding:28px 0;">
    <tr><td align="center">
      <table role="presentation" width="100%" cellpadding="0" cellspacing="0"
        style="max-width:520px;background:#161b22;border:1px solid #232a33;border-radius:14px;">
        <tr><td style="padding:26px 30px 8px;">
          <div style="font-size:1.25rem;font-weight:700;color:#e6edf3;">
            <span style="color:#58a6ff;">&#9670;</span> {{ app_name }}
          </div>
        </td></tr>
        <tr><td style="padding:8px 30px 4px;">
          <h1 style="margin:0 0 6px;font-size:1.3rem;color:#e6edf3;">{{ heading }}</h1>
        </td></tr>
        <tr><td style="padding:4px 30px 8px;color:#c2ccd6;line-height:1.6;font-size:.97rem;">
          {% for p in paragraphs %}<p style="margin:0 0 14px;">{{ p }}</p>{% endfor %}
          {% if button %}
          <p style="margin:22px 0;">
            <a href="{{ button.url }}" style="display:inline-block;background:#238636;color:#fff;
              text-decoration:none;padding:11px 20px;border-radius:8px;font-weight:600;">{{ button.label }}</a>
          </p>
          <p style="margin:0 0 14px;color:#8b949e;font-size:.85rem;word-break:break-all;">
            Or paste this link into your browser:<br>{{ button.url }}</p>
          {% endif %}
          {% if note %}<p style="margin:14px 0 0;color:#8b949e;font-size:.85rem;">{{ note }}</p>{% endif %}
        </td></tr>
        <tr><td style="padding:18px 30px 26px;border-top:1px solid #232a33;color:#6e7681;font-size:.8rem;">
          {{ app_name }} · <a href="{{ dev_url }}" style="color:#58a6ff;">Developer portal</a>
          · <a href="{{ docs_url }}" style="color:#58a6ff;">Docs</a>
        </td></tr>
      </table>
    </td></tr>
  </table>
</body>
</html>"#;

/// `markupsafe.escape(str)`.
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\'' => out.push_str("&#39;"),
            '"' => out.push_str("&#34;"),
            c => out.push(c),
        }
    }
    out
}

#[derive(Debug, Clone)]
pub struct Button {
    pub label: String,
    pub url: String,
}

pub struct Mail<'a> {
    pub app_name: &'a str,
    pub dev_url: &'a str,
    pub docs_url: &'a str,
    pub heading: &'a str,
    pub paragraphs: &'a [String],
    pub button: Option<&'a Button>,
    pub note: Option<&'a str>,
}

enum Tok<'a> {
    Text(&'a str),
    Var(&'a str),
    Tag(&'a str),
}

fn tokenize(t: &str) -> Vec<Tok<'_>> {
    let mut out = Vec::new();
    let mut rest = t;
    while let Some(i) = rest.find(['{']) {
        let (open, close) = match rest[i..].get(..2) {
            Some("{{") => ("{{", "}}"),
            Some("{%") => ("{%", "%}"),
            _ => {
                out.push(Tok::Text(&rest[..=i]));
                rest = &rest[i + 1..];
                continue;
            }
        };
        out.push(Tok::Text(&rest[..i]));
        let end = rest[i..].find(close).expect("closed template tag") + i;
        let inner = rest[i + 2..end].trim();
        out.push(if open == "{{" { Tok::Var(inner) } else { Tok::Tag(inner) });
        rest = &rest[end + 2..];
    }
    out.push(Tok::Text(rest));
    out
}

/// `render_email(heading, paragraphs, button, note)`: the template above as
/// Jinja2 renders it (autoescape on, the final newline dropped).
pub fn render_email(m: &Mail<'_>) -> String {
    let toks = tokenize(BASE);
    let mut out = String::new();
    render(&toks, m, None, &mut out);
    out
}

fn block_end(toks: &[Tok<'_>], start: usize, end_tag: &str) -> usize {
    toks[start..].iter().position(|t| matches!(t, Tok::Tag(x) if *x == end_tag)).expect("template block") + start
}

fn render(toks: &[Tok<'_>], m: &Mail<'_>, p: Option<&str>, out: &mut String) {
    let mut i = 0;
    while i < toks.len() {
        match &toks[i] {
            Tok::Text(t) => out.push_str(t),
            Tok::Var(name) => {
                let v = match *name {
                    "app_name" => m.app_name,
                    "dev_url" => m.dev_url,
                    "docs_url" => m.docs_url,
                    "heading" => m.heading,
                    "p" => p.unwrap_or_default(),
                    "note" => m.note.unwrap_or_default(),
                    "button.url" => m.button.map_or("", |b| &b.url),
                    "button.label" => m.button.map_or("", |b| &b.label),
                    other => panic!("unknown template variable {other}"),
                };
                out.push_str(&escape(v));
            }
            Tok::Tag(tag) => {
                let (end_tag, show) = match *tag {
                    "for p in paragraphs" => ("endfor", true),
                    "if button" => ("endif", m.button.is_some()),
                    "if note" => ("endif", m.note.is_some_and(|n| !n.is_empty())),
                    other => panic!("unknown template tag {other}"),
                };
                let end = block_end(toks, i + 1, end_tag);
                let body = &toks[i + 1..end];
                if *tag == "for p in paragraphs" {
                    for para in m.paragraphs {
                        render(body, m, Some(para), out);
                    }
                } else if show {
                    render(body, m, p, out);
                }
                i = end;
            }
        }
        i += 1;
    }
}

/// The plain-text part `_queue` builds.
pub fn render_text(paragraphs: &[String], button: Option<&Button>, note: Option<&str>) -> String {
    let mut text = paragraphs.join("\n\n");
    if let Some(b) = button {
        text.push_str(&format!("\n\n{}: {}", b.label, b.url));
    }
    if let Some(n) = note.filter(|n| !n.is_empty()) {
        text.push_str(&format!("\n\n{n}"));
    }
    text
}

/// A rendered message, ready for the outbox.
#[derive(Debug, Clone)]
pub struct Message {
    pub to: String,
    pub subject: String,
    pub text: String,
    pub html: String,
}

pub fn compose(
    s: &Settings,
    to: &str,
    subject: String,
    heading: &str,
    paragraphs: Vec<String>,
    button: Option<Button>,
    note: Option<String>,
) -> Message {
    let html = render_email(&Mail {
        app_name: &s.app_name,
        dev_url: &s.dev_url,
        docs_url: &s.docs_url,
        heading,
        paragraphs: &paragraphs,
        button: button.as_ref(),
        note: note.as_deref(),
    });
    let text = render_text(&paragraphs, button.as_ref(), note.as_deref());
    Message { to: to.to_owned(), subject, text, html }
}

/// The `OutboxEmail` row `queue_email` inserts, fields in model order.
pub fn outbox_doc(m: &Message, next_attempt_at: DateTime<Utc>, created_at: DateTime<Utc>) -> Document {
    doc! {
        "_id": ObjectId::new(),
        "to": &m.to,
        "subject": &m.subject,
        "text": &m.text,
        "html": &m.html,
        "status": "pending",
        "attempts": 0_i32,
        "last_error": Bson::Null,
        "next_attempt_at": dt::to_bson(next_attempt_at),
        "claimed_at": Bson::Null,
        "created_at": dt::to_bson(created_at),
        "sent_at": Bson::Null,
    }
}

/// `queue_email`: a no-op without SMTP, skipped for a bounced address.
pub async fn queue_email(state: &AppState, m: &Message) -> Result<(), Rejection> {
    if !state.settings().email_enabled() {
        tracing::warn!("SMTP not configured - not queueing email to {} ({})", m.to, m.subject);
        return Ok(());
    }
    let db = super::store::db(state)?;
    let bounced = db
        .collection::<Document>(crate::auth::models::USERS)
        .find_one(doc! { "email": m.to.to_lowercase(), "email_bounced": true })
        .await
        .map_err(Rejection::internal)?;
    if bounced.is_some() {
        tracing::warn!("Skipping email to bounced address {} ({})", m.to, m.subject);
        return Ok(());
    }
    let next = dt::now();
    let created = dt::now();
    db.collection::<Document>(OUTBOX).insert_one(outbox_doc(m, next, created)).await.map_err(Rejection::internal)?;
    Ok(())
}

/// Run mail work after the response, like a Starlette `BackgroundTask`.
pub fn background<F>(state: &AppState, build: F)
where
    F: FnOnce(&AppState) -> Option<Message> + Send + 'static,
{
    let state = state.clone();
    tokio::spawn(async move {
        if let Some(m) = build(&state)
            && let Err(err) = queue_email(&state, &m).await
        {
            tracing::error!("background email to {} failed: {}", m.to, err.error);
        }
    });
}

fn secret() -> &'static str {
    &crate::auth::auth_settings().secret_key
}

/// `create_email_token` at the current time.
pub fn email_token(subject: &str, purpose: &str, ttl: Duration, extra: Option<&jwt::Claims>) -> String {
    jwt::create_email_token_at(secret(), subject, purpose, ttl, extra, Utc::now())
}

pub fn verification_link(s: &Settings, user: &User) -> String {
    let token =
        email_token(&user.id.to_hex(), VERIFY_PURPOSE, Duration::hours(s.email_verification_expire_hours), None);
    format!("{}/auth/verify-email?token={token}", s.api_url)
}

pub fn reset_link(s: &Settings, user: &User) -> String {
    let mut extra = jwt::Claims::new();
    extra.insert("fp".into(), Value::String(password_fingerprint(&user.hashed_password)));
    let token =
        email_token(&user.id.to_hex(), RESET_PURPOSE, Duration::hours(s.password_reset_expire_hours), Some(&extra));
    format!("{}/auth/reset-password?token={token}", s.api_url)
}

fn button(label: &str, url: String) -> Option<Button> {
    Some(Button { label: label.into(), url })
}

pub fn verification_email(s: &Settings, user: &User) -> Message {
    let hours = s.email_verification_expire_hours;
    compose(
        s,
        &user.email,
        format!("Verify your {} email", s.app_name),
        "Confirm your email",
        vec![format!("Welcome to {}.", s.app_name), "Confirm your email address to start creating API tokens.".into()],
        button("Verify my email", verification_link(s, user)),
        Some(format!("This link expires in {hours} hours. If you didn't sign up, ignore this email.")),
    )
}

pub fn password_reset_email(s: &Settings, user: &User) -> Message {
    let hours = s.password_reset_expire_hours;
    compose(
        s,
        &user.email,
        format!("Reset your {} password", s.app_name),
        "Reset your password",
        vec![format!("A password reset was requested for your {} account.", s.app_name)],
        button("Set a new password", reset_link(s, user)),
        Some(format!("This link expires in {hours} hour(s). If you didn't request this, ignore this email.")),
    )
}

pub fn email_change_verification(s: &Settings, user: &User, new_email: &str) -> Message {
    let mut extra = jwt::Claims::new();
    extra.insert("new".into(), Value::String(new_email.to_owned()));
    extra.insert("fp".into(), Value::String(password_fingerprint(&user.email)));
    let token = email_token(
        &user.id.to_hex(),
        EMAIL_CHANGE_PURPOSE,
        Duration::hours(s.email_verification_expire_hours),
        Some(&extra),
    );
    let link = format!("{}/auth/verify-email-change?token={token}", s.api_url);
    let hours = s.email_verification_expire_hours;
    compose(
        s,
        new_email,
        format!("Confirm your new {} email", s.app_name),
        "Confirm your new email",
        vec![format!("Confirm your new email address for {}.", s.app_name)],
        button("Confirm new email", link),
        Some(format!("This link expires in {hours} hours. If you didn't request this, ignore it.")),
    )
}

fn security_notice(s: &Settings, to: &str, subject: String, heading: &str, paragraphs: Vec<String>) -> Message {
    let note = format!(
        "If this wasn't you, secure your {} account immediately - change your password and review your active sessions.",
        s.app_name
    );
    compose(s, to, subject, heading, paragraphs, None, Some(note))
}

pub fn new_login_email(s: &Settings, user: &User, ip: Option<&str>, device: Option<&str>) -> Message {
    security_notice(
        s,
        &user.email,
        format!("New sign-in to your {} account", s.app_name),
        "New sign-in detected",
        vec![
            "A new sign-in to your account was detected from a device we hadn't seen before.".into(),
            format!("IP address: {}", ip.filter(|v| !v.is_empty()).unwrap_or("unknown")),
            format!("Device: {}", device.filter(|v| !v.is_empty()).unwrap_or("unknown")),
        ],
    )
}

pub fn password_changed_email(s: &Settings, user: &User) -> Message {
    security_notice(
        s,
        &user.email,
        format!("Your {} password was changed", s.app_name),
        "Password changed",
        vec!["Your account password was just changed, and all other sessions were signed out.".into()],
    )
}

pub fn token_created_email(s: &Settings, user: &User, token_name: &str, prefix: &str) -> Message {
    security_notice(
        s,
        &user.email,
        format!("A new API token was created on your {} account", s.app_name),
        "New API token created",
        vec![format!("A new API token \"{token_name}\" ({prefix}\u{2026}) was just created.")],
    )
}

pub fn token_expiring_email(s: &Settings, user: &User, token_name: &str, prefix: &str, days_left: i64) -> Message {
    let when = if days_left <= 0 { "today".to_owned() } else { format!("in {days_left} day(s)") };
    compose(
        s,
        &user.email,
        format!("Your {} API token expires {when}", s.app_name),
        "API token expiring",
        vec![
            format!("Your {} API token \"{token_name}\" ({prefix}\u{2026}) expires {when}.", s.app_name),
            "Once it expires it will stop authenticating requests. Rotate or replace it from the developer portal."
                .into(),
        ],
        button("Open the portal", s.dev_url.clone()),
        Some("If you no longer need this token, you can ignore this message.".into()),
    )
}

pub fn github_linked_email(s: &Settings, user: &User) -> Message {
    security_notice(
        s,
        &user.email,
        format!("GitHub was linked to your {} account", s.app_name),
        "GitHub account linked",
        vec!["A GitHub account was just linked to your account and can now be used to sign in.".into()],
    )
}

pub fn token_compromised_email(s: &Settings, user: &User, token_name: &str, prefix: &str) -> Message {
    security_notice(
        s,
        &user.email,
        format!("An API token was auto-revoked on your {} account", s.app_name),
        "API token exposed - auto-revoked",
        vec![
            format!(
                "Your API token \"{token_name}\" ({prefix}\u{2026}) was found exposed publicly (detected by secret scanning) and has been automatically revoked."
            ),
            "Create a replacement token and update wherever the old one was used. Never commit a token to source control."
                .into(),
        ],
    )
}

pub fn email_changed_notice_to_old(s: &Settings, old_email: &str, new_email: &str) -> Message {
    security_notice(
        s,
        old_email,
        format!("Your {} email address was changed", s.app_name),
        "Email address changed",
        vec!["The email address on your account was changed to a new one.".into(), format!("New address: {new_email}")],
    )
}

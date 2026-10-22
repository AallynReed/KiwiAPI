//! `app/giveaways/schemas.py` response models (field order is the wire
//! order) and the winner email of `service._email_winner`.

use chrono::{DateTime, Utc};
use mongodb::bson::oid::ObjectId;
use serde_json::{Value, json};

use super::model::{CodeStatus, Giveaway, PrizeCode, VaultItem};
use crate::dt;

fn t(d: DateTime<Utc>) -> Value {
    Value::String(dt::pydantic(&d))
}

fn opt_t(d: Option<DateTime<Utc>>) -> Value {
    d.map_or(Value::Null, t)
}

fn opt_id(o: Option<ObjectId>) -> Value {
    o.map_or(Value::Null, |o| Value::String(o.to_hex()))
}

/// Code tallies of one drawer (`_code_counts`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub available: i64,
    pub reserved: i64,
    pub awarded: i64,
    pub total: i64,
}

impl Counts {
    pub fn add(&mut self, status: CodeStatus) {
        self.total += 1;
        match status {
            CodeStatus::Available => self.available += 1,
            CodeStatus::Reserved => self.reserved += 1,
            CodeStatus::Awarded => self.awarded += 1,
        }
    }
}

pub fn item_view(item: &VaultItem, c: Counts) -> Value {
    json!({
        "id": item.id.to_hex(),
        "name": item.name,
        "description": item.description,
        "available": c.available,
        "reserved": c.reserved,
        "awarded": c.awarded,
        "total": c.total,
        "created_at": t(item.created_at),
    })
}

pub fn code_view(c: &PrizeCode) -> Value {
    json!({
        "id": c.id.to_hex(),
        "code": c.code,
        "status": c.status.as_str(),
        "giveaway_id": opt_id(c.giveaway_id),
        "awarded_at": opt_t(c.awarded_at),
        "created_at": t(c.created_at),
    })
}

pub fn admin_view(g: &Giveaway, item_name: Option<&str>) -> Value {
    json!({
        "id": g.id.to_hex(),
        "title": g.title,
        "description": g.description,
        "prize_name": g.prize_name,
        "status": g.status.as_str(),
        "starts_at": t(g.starts_at),
        "ends_at": t(g.ends_at),
        "entry_count": g.entry_count,
        "vault_item_id": opt_id(g.vault_item_id),
        "vault_item_name": item_name,
        "prize_code_id": opt_id(g.prize_code_id),
        "winner_user_id": opt_id(g.winner_user_id),
        "winner_username": g.winner_username,
        "drawn_at": opt_t(g.drawn_at),
        "created_at": t(g.created_at),
    })
}

pub fn public_view(g: &Giveaway) -> Value {
    json!({
        "id": g.id.to_hex(),
        "title": g.title,
        "description": g.description,
        "prize_name": g.prize_name,
        "status": g.status.as_str(),
        "starts_at": t(g.starts_at),
        "ends_at": t(g.ends_at),
        "entry_count": g.entry_count,
        "winner_username": g.winner_username,
    })
}

pub fn enter_view(giveaway_id: &str, entry_count: i64) -> Value {
    json!({"giveaway_id": giveaway_id, "entered": true, "entry_count": entry_count})
}

/// One `MyGiveawayView`, with the keys `/me` sorts by.
#[derive(Clone, Debug)]
pub struct Mine {
    pub won: bool,
    pub ends_at: DateTime<Utc>,
    pub view: Value,
}

pub fn mine(g: &Giveaway, entered_at: DateTime<Utc>, won: bool, code: Option<&str>) -> Mine {
    let view = json!({
        "giveaway_id": g.id.to_hex(),
        "title": g.title,
        "prize_name": g.prize_name,
        "status": g.status.as_str(),
        "starts_at": t(g.starts_at),
        "ends_at": t(g.ends_at),
        "entered_at": t(entered_at),
        "won": won,
        "code": code,
    });
    Mine { won, ends_at: g.ends_at, view }
}

/// `out.sort(key=lambda x: (x.won, x.ends_at), reverse=True)`: stable, so
/// equal keys keep their entry order.
pub fn sort_mine(out: &mut [Mine]) {
    out.sort_by_key(|m| std::cmp::Reverse((m.won, m.ends_at)));
}

/// `(subject, text, html)` of the winner email.
pub fn winner_email(g: &Giveaway, code: &str) -> (String, String, String) {
    let subject = format!("You won: {}!", g.prize_name);
    let mut lines = vec![
        format!("Congratulations - you won \"{}\" on Better Trove Tools!", g.title),
        String::new(),
        format!("Prize: {}", g.prize_name),
        format!("Your code: {code}"),
    ];
    let desc = g.description.as_deref().filter(|d| !d.is_empty());
    if let Some(d) = desc {
        lines.push(String::new());
        lines.push(d.to_owned());
    }
    for l in ["", "Your code is also always on your dashboard.", "Thanks for playing.", "- Better Trove Tools"] {
        lines.push(l.to_owned());
    }
    let text = lines.join("\n");
    let desc_html = desc.map(|d| format!("<p style='color:#9aa4b2'>{d}</p>")).unwrap_or_default();
    let html = format!(
        "<div style=\"font-family:system-ui,sans-serif;max-width:480px;margin:0 auto;padding:24px;color:#e8ecf3;background:#0a0e14\">\
<h1 style='font-size:1.4rem;margin:0 0 12px'>You won {prize}! \u{1f389}</h1>\
<p>Congratulations - you won <strong>{title}</strong> on Better Trove Tools.</p>\
<p style='margin:18px 0 6px'>Your code:</p>\
<p style='font-family:monospace;font-size:1.1rem;background:#161b22;border:1px solid #232a33;border-radius:8px;padding:12px 14px'>{code}</p>\
{desc_html}\
<p style='margin-top:20px;color:#9aa4b2'>Your code is also always on your dashboard.<br>Thanks for playing.<br>- Better Trove Tools</p>\
</div>",
        prize = g.prize_name,
        title = g.title,
    );
    (subject, text, html)
}

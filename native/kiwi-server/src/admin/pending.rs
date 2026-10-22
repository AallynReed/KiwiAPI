//! `GET /admin/pending` (`app/admin/pending.py`): how many items each portal
//! tab has waiting on a master decision, for the sidebar badges.

use axum::extract::State;
use chrono::{Duration, Utc};
use mongodb::bson::{DateTime, Document, doc};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use super::{Res, coll};
use crate::{auth::Superuser, dt, pyjson::PydanticJson, state::AppState};

pub enum Count {
    /// `count_documents(filter)` on a collection.
    Docs(&'static str, fn() -> Document),
    /// `decode.runner.failing_count()`.
    FailingDecoders,
}

pub struct Source {
    pub tab: &'static str,
    pub one: &'static str,
    pub many: &'static str,
    pub count: Count,
}

/// `_SOURCES`, in order.
pub const SOURCES: &[Source] = &[
    Source {
        tab: "customart",
        one: "request to review",
        many: "requests to review",
        count: Count::Docs("art_requests", || doc! {"status": "pending"}),
    },
    Source {
        tab: "customart",
        one: "approved request waiting on a release",
        many: "approved requests waiting on a release",
        count: Count::Docs("art_requests", || doc! {"status": "approved"}),
    },
    Source {
        tab: "customart",
        one: "failed build to retry",
        many: "failed builds to retry",
        count: Count::Docs("art_requests", || doc! {"status": "failed"}),
    },
    Source {
        tab: "customart",
        one: "request waiting on a Steam push",
        many: "requests waiting on a Steam push",
        count: Count::Docs("art_requests", || doc! {"steam_pending": true}),
    },
    Source {
        tab: "claims",
        one: "name claim to verify",
        many: "name claims to verify",
        count: Count::Docs("site_users", || doc! {"claimed_trove_name": {"$ne": null}, "claim_verified": false}),
    },
    Source {
        tab: "claims",
        one: "username change request",
        many: "username change requests",
        count: Count::Docs(super::docs::USERNAME_REQUESTS, || doc! {"status": "pending"}),
    },
    Source {
        tab: "mods",
        one: "open content report",
        many: "open content reports",
        count: Count::Docs("content_reports", || doc! {"resolved": false}),
    },
    Source {
        tab: "mods",
        one: "stray mod awaiting approval",
        many: "stray mods awaiting approval",
        count: Count::Docs(super::docs::MOD_PROJECTS, || {
            doc! {"$and": [{"is_stray": true}, {"stray_status": "pending"}]}
        }),
    },
    Source {
        tab: "mods",
        one: "mod claim request",
        many: "mod claim requests",
        count: Count::Docs("mod_claim_requests", || doc! {"status": "pending"}),
    },
    Source {
        tab: "gamedata",
        one: "game-data decoder failing",
        many: "game-data decoders failing",
        count: Count::FailingDecoders,
    },
];

/// `app.trove.decode.registry.DECODERS`, in order.
pub const DECODERS: [&str; 33] = [
    "class_levels",
    "class_abilities",
    "ally_abilities",
    "gem_abilities",
    "ring_abilities",
    "mount_abilities",
    "pvp_stat_ranges",
    "delve_modifiers",
    "companions",
    "badges",
    "fish",
    "mementos",
    "recipes",
    "collectibles",
    "cosmetics",
    "titles",
    "npcs",
    "placeables",
    "quests",
    "worlds",
    "subclass_abilities",
    "daily_bonuses",
    "leaderboards",
    "gem_upgrades",
    "upgrade_trees",
    "items",
    "mastery",
    "class_rewards",
    "lootbox_odds",
    "pvp_powerups",
    "chat_commands",
    "insignia",
    "gear",
];

const LOCK_TTL_SECONDS: i64 = 1800;

/// `DecoderRun` as Beanie validates it; a row it can't validate skips the badge.
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct DecoderRun {
    name: String,
    trigger: String,
    ordinal: i64,
    #[serde(default)]
    version_tag: Option<String>,
    code_version: String,
    pub started_at: DateTime,
    #[serde(default)]
    pub finished_at: Option<DateTime>,
    #[serde(default)]
    pub ok: Option<bool>,
    #[serde(default)]
    entries: Option<i64>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    traceback: Option<String>,
}

/// `last.ok is False or _interrupted(last)`.
pub fn is_failing(run: &DecoderRun, now: chrono::DateTime<Utc>) -> bool {
    let interrupted =
        run.finished_at.is_none() && now - dt::from_bson(run.started_at) > Duration::seconds(LOCK_TTL_SECONDS);
    run.ok == Some(false) || interrupted
}

async fn failing_decoders(state: &AppState) -> anyhow::Result<i64> {
    let c = coll(state, "decoder_runs");
    let mut total = 0;
    for name in DECODERS {
        let last = c.find_one(doc! {"name": name}).sort(doc! {"started_at": -1}).await?;
        if let Some(d) = last {
            let run: DecoderRun = mongodb::bson::from_document(d)?;
            if is_failing(&run, dt::now()) {
                total += 1;
            }
        }
    }
    Ok(total)
}

async fn count(state: &AppState, c: &Count) -> anyhow::Result<i64> {
    match c {
        Count::Docs(name, filter) => Ok(coll(state, name).count_documents(filter()).await? as i64),
        Count::FailingDecoders => failing_decoders(state).await,
    }
}

/// `pending_counts()` from each source's count (`None`: the count raised).
pub fn fold(counts: &[Option<i64>]) -> Value {
    let mut totals: Map<String, Value> = Map::new();
    let mut detail: Map<String, Value> = Map::new();
    for (source, n) in SOURCES.iter().zip(counts) {
        let Some(n) = *n else { continue };
        if n <= 0 {
            continue;
        }
        let prev = totals.get(source.tab).and_then(Value::as_i64).unwrap_or(0);
        totals.insert(source.tab.to_owned(), json!(prev + n));
        let label = if n == 1 { source.one } else { source.many };
        if let Some(Value::Array(list)) = detail.get_mut(source.tab) {
            list.push(json!({"label": label, "count": n}));
        } else {
            detail.insert(source.tab.to_owned(), json!([{"label": label, "count": n}]));
        }
    }
    json!({"counts": totals, "detail": detail})
}

pub async fn pending_counts(State(state): State<AppState>, _: Superuser) -> Res {
    let mut counts = Vec::with_capacity(SOURCES.len());
    for s in SOURCES {
        counts.push(match count(&state, &s.count).await {
            Ok(n) => Some(n),
            Err(e) => {
                tracing::debug!("pending {}: {e:#}", s.tab);
                None
            }
        });
    }
    Ok(PydanticJson(fold(&counts)))
}

//! `GET /site/clubs` (`app/site/clubs_page.py`): public clubs ordered by their
//! rank on the club leaderboard (board 1100), unranked clubs after.

use std::collections::HashMap;

use axum::{extract::State, response::Response};
use futures::TryStreamExt;
use mongodb::bson::{Bson, Document, doc};
use serde_json::{Value, json};

use super::{cached, flags::SiteGate};
use crate::{
    error::ApiError,
    lbread::{Live, store::Source},
    state::AppState,
};

const CLUB_BOARD: i32 = 1100;

fn internal(e: impl std::fmt::Display) -> ApiError {
    tracing::error!("site clubs: {e}");
    ApiError::internal()
}

/// A public `Club` as the directory needs it.
#[derive(Clone, Debug, PartialEq)]
pub struct Club {
    pub name: String,
    pub description: Option<String>,
    pub banner_url: Option<String>,
    pub avatar_url: Option<String>,
    pub discord_url: Option<String>,
    pub website_url: Option<String>,
}

fn opt_str(d: &Document, k: &str) -> Result<Option<String>, ApiError> {
    match d.get(k) {
        None | Some(Bson::Null) => Ok(None),
        Some(Bson::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(internal(format!("club {k} is not a string"))),
    }
}

impl Club {
    pub fn from_doc(d: &Document) -> Result<Club, ApiError> {
        let name = match d.get("name") {
            Some(Bson::String(s)) => s.clone(),
            _ => return Err(internal("club name is not a string")),
        };
        Ok(Club {
            name,
            description: opt_str(d, "description")?,
            banner_url: opt_str(d, "banner_url")?,
            avatar_url: opt_str(d, "avatar_url")?,
            discord_url: opt_str(d, "discord_url")?,
            website_url: opt_str(d, "website_url")?,
        })
    }
}

fn norm(s: &str) -> String {
    kiwi_core::pystr::strip(s).to_lowercase()
}

/// `public_clubs_ordered()` over the clubs and the board's `(player_name, rank)` rows.
pub fn ordered(mut clubs: Vec<Club>, entries: &[(String, i64)]) -> Vec<Value> {
    let mut ranks: HashMap<String, i64> = HashMap::new();
    for (name, rank) in entries {
        let k = norm(name);
        if !k.is_empty() {
            ranks.entry(k).or_insert(*rank);
        }
    }
    let rank = |c: &Club| ranks.get(&norm(&c.name)).copied();
    clubs.sort_by_cached_key(|c| match rank(c) {
        Some(r) => (0, r, String::new()),
        None => (1, 0, c.name.to_lowercase()),
    });
    clubs
        .iter()
        .map(|c| {
            json!({
                "name": c.name,
                "rank": rank(c),
                "description": c.description,
                "banner_url": c.banner_url,
                "avatar_url": c.avatar_url,
                "discord_url": c.discord_url,
                "website_url": c.website_url,
            })
        })
        .collect()
}

async fn board_entries(state: &AppState) -> anyhow::Result<Vec<(String, i64)>> {
    let live = Live(state);
    let Some(anchor) = live.latest_anchor_for_board(CLUB_BOARD).await? else { return Ok(Vec::new()) };
    let (rows, _) = live.list_entries(CLUB_BOARD, anchor, 2000, 0).await?;
    Ok(rows.into_iter().map(|r| (r.player_name, r.rank)).collect())
}

pub async fn route(_: SiteGate, State(state): State<AppState>) -> Result<Response, ApiError> {
    let docs: Vec<Document> = state
        .mongo()
        .collection::<Document>("clubs")
        .find(doc! {"public": true})
        .await
        .map_err(internal)?
        .try_collect()
        .await
        .map_err(internal)?;
    let clubs = docs.iter().map(Club::from_doc).collect::<Result<Vec<_>, _>>()?;
    let items = if clubs.is_empty() {
        Vec::new()
    } else {
        let entries = board_entries(&state).await.unwrap_or_else(|e| {
            tracing::warn!("clubs: leaderboard {CLUB_BOARD} unavailable: {e:#}");
            Vec::new()
        });
        ordered(clubs, &entries)
    };
    let count = items.len();
    Ok(cached(json!({"items": items, "count": count}), "public, max-age=60"))
}

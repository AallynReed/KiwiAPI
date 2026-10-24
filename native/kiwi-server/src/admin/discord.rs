//! `/admin/discord/*` (`app/discord/registration.py`): preview and push the
//! slash commands, clear a guild's; `/admin/bot/stats` (`app/bot/stats.py`);
//! and `discord_rest.fetch_user` for the site-user Discord refresh.

use std::{sync::LazyLock, time::Duration};

use axum::extract::State;
use futures::TryStreamExt;
use mongodb::bson::{Bson, Document, doc};
use serde_json::{Value, json};

use super::{Args, Res, coll, internal};
use crate::{
    auth::Superuser,
    config::Settings,
    error::ApiError,
    lb_ingest::admin::bson_json,
    pyjson::{self, PydanticJson},
    state::AppState,
    validation::{FromParams, Params},
};

const API: &str = "https://discord.com/api/v10";
/// The `User-Agent` httpx sends by default.
const HTTPX_UA: &str = "python-httpx/0.28.1";

/// `app.discord.commands.COMMAND_DEFS` (generated from Python by
/// `native/port/parity/gen_admin_fixtures.py`).
pub static COMMAND_DEFS: LazyLock<Vec<Value>> = LazyLock::new(|| {
    serde_json::from_str(include_str!("discord_commands.json")).expect("discord_commands.json parses")
});

fn text(v: Option<&Value>) -> Value {
    v.cloned().unwrap_or(Value::Null)
}

/// `{name, description}` of a command (`description` defaults to "").
pub fn brief(c: &Value) -> Value {
    json!({"name": text(c.get("name")), "description": c.get("description").cloned().unwrap_or_else(|| json!(""))})
}

pub fn commands_body() -> Value {
    let defs = &*COMMAND_DEFS;
    json!({"count": defs.len(), "commands": defs.iter().map(brief).collect::<Vec<_>>()})
}

pub async fn commands_preview(_: Superuser) -> PydanticJson {
    PydanticJson(commands_body())
}

/// `guild_id: str | None = Query(default=None)`.
pub struct OptGuild(pub Option<String>);

impl FromParams for OptGuild {
    fn from_params(p: &mut Params<'_>) -> Self {
        OptGuild(p.query("guild_id").str().optional())
    }
}

/// `guild_id: str = Query(...)`.
pub struct Guild(pub String);

impl FromParams for Guild {
    fn from_params(p: &mut Params<'_>) -> Self {
        Guild(p.query("guild_id").str().required())
    }
}

fn client(timeout: u64) -> Result<reqwest::Client, ApiError> {
    crate::http::client_builder().timeout(Duration::from_secs(timeout)).build().map_err(internal)
}

fn truthy(v: &Option<String>) -> Option<&str> {
    v.as_deref().filter(|s| !s.is_empty())
}

/// `_require_creds`.
fn creds(s: &Settings) -> Result<(&str, &str), ApiError> {
    match (truthy(&s.discord_client_id), truthy(&s.discord_bot_token)) {
        (Some(app), Some(token)) => Ok((app, token)),
        _ => Err(ApiError::bad_request("discord_client_id and discord_bot_token must be set in the environment.")),
    }
}

/// `_put_commands`: bulk-overwrite the set at `url`, Discord's echo back.
async fn put_commands(url: &str, payload: &Value, token: &str) -> Result<Value, ApiError> {
    let resp = client(30)?
        .put(url)
        .header("Authorization", format!("Bot {token}"))
        .header("User-Agent", HTTPX_UA)
        .header("Content-Type", "application/json")
        .body(pyjson::dumps(payload))
        .send()
        .await
        .map_err(|e| ApiError::bad_request(format!("Could not reach Discord: {e}")))?;
    let status = resp.status().as_u16();
    let body = resp.text().await.map_err(|e| ApiError::bad_request(format!("Could not reach Discord: {e}")))?;
    if status >= 300 {
        let head: String = body.chars().take(400).collect();
        return Err(ApiError::bad_request(format!("Discord rejected the request ({status}): {head}")));
    }
    pyjson::loads(body.as_bytes()).map_err(|e| internal(format!("discord answer: {e:?}")))
}

/// The bulk-overwrite URL; a guild id must be a snowflake so it stays one path segment.
pub fn commands_url(app: &str, guild: Option<&str>) -> Result<String, ApiError> {
    match guild {
        None => Ok(format!("{API}/applications/{app}/commands")),
        Some(g) if g.bytes().all(|b| b.is_ascii_digit()) && !g.is_empty() => {
            Ok(format!("{API}/applications/{app}/guilds/{g}/commands"))
        }
        Some(_) => Err(ApiError::bad_request("guild_id must be a Discord server ID (digits only).")),
    }
}

fn echo(cmds: &Value) -> Result<(usize, Vec<Value>), ApiError> {
    let list = cmds.as_array().ok_or_else(|| internal("discord answer is not a list"))?;
    Ok((list.len(), list.iter().map(brief).collect()))
}

pub async fn register_commands(
    State(state): State<AppState>,
    _: Superuser,
    Args(OptGuild(guild)): Args<OptGuild>,
) -> Res {
    let (app, token) = creds(state.settings())?;
    let url = commands_url(app, truthy(&guild))?;
    let cmds = put_commands(&url, &Value::Array(COMMAND_DEFS.clone()), token).await?;
    let (count, commands) = echo(&cmds)?;
    tracing::info!(
        "Pushed {count} Discord command(s){}",
        truthy(&guild).map_or_else(|| " globally".to_owned(), |g| format!(" to guild {g}"))
    );
    Ok(PydanticJson(json!({
        "scope": if truthy(&guild).is_some() { "guild" } else { "global" },
        "guild_id": guild,
        "count": count,
        "commands": commands,
    })))
}

pub async fn clear_guild_commands(State(state): State<AppState>, _: Superuser, Args(Guild(guild)): Args<Guild>) -> Res {
    let (app, token) = creds(state.settings())?;
    let url = commands_url(app, Some(&guild))?;
    put_commands(&url, &json!([]), token).await?;
    tracing::info!("Cleared guild-scoped Discord commands for guild {guild}");
    Ok(PydanticJson(json!({"cleared": true, "guild_id": guild})))
}

// --- bot stats -----------------------------------------------------------------------

/// `int(c.get("count", 0))`.
fn count_of(c: &Document) -> Result<i64, ApiError> {
    match c.get("count") {
        None => Ok(0),
        Some(Bson::Int32(n)) => Ok(i64::from(*n)),
        Some(Bson::Int64(n)) => Ok(*n),
        Some(Bson::Double(f)) if f.is_finite() => Ok(f.trunc() as i64),
        Some(Bson::Boolean(b)) => Ok(i64::from(*b)),
        Some(other) => Err(internal(format!("bot_command_usage count: {other}"))),
    }
}

/// `stats.get_stats()` from the presence singleton and the command rows.
pub fn bot_stats_body(presence: Option<&Document>, commands: &[Document]) -> Result<Value, ApiError> {
    let field = |k: &str| presence.and_then(|p| p.get(k)).map_or(Value::Null, bson_json);
    let mut rows = Vec::with_capacity(commands.len());
    let mut total = 0i64;
    for c in commands {
        let n = count_of(c)?;
        total += n;
        let name = c.get("_id").ok_or_else(|| internal("bot_command_usage row without _id"))?;
        rows.push(json!({
            "name": bson_json(name),
            "count": n,
            "last_used_at": c.get("last_used_at").map_or(Value::Null, bson_json),
        }));
    }
    Ok(json!({
        "guild_count": field("guild_count"),
        "member_count": field("member_count"),
        "updated_at": field("updated_at"),
        "total_commands": total,
        "commands": rows,
    }))
}

pub async fn bot_stats(State(state): State<AppState>, _: Superuser) -> Res {
    let presence = coll(&state, "bot_presence").find_one(doc! {"_id": "current"}).await.map_err(internal)?;
    let commands: Vec<Document> = coll(&state, "bot_command_usage")
        .find(doc! {})
        .sort(doc! {"count": -1})
        .await
        .map_err(internal)?
        .try_collect()
        .await
        .map_err(internal)?;
    Ok(PydanticJson(bot_stats_body(presence.as_ref(), &commands)?))
}

// --- Discord REST ----------------------------------------------------------------------

/// `discord_rest.fetch_user`: Discord's user object, `None` on a 404. `Err`
/// carries `DiscordRestError`'s message; a body that isn't JSON is a 500.
pub async fn fetch_user(s: &Settings, discord_id: i64) -> Result<Option<Value>, FetchError> {
    let Some(token) = truthy(&s.discord_bot_token) else {
        return Err(FetchError::Rest("DISCORD_BOT_TOKEN is not configured.".into()));
    };
    let path = format!("/users/{discord_id}");
    let client = client(15).map_err(|_| FetchError::Other)?;
    let resp = client
        .get(format!("{API}{path}"))
        .header("Authorization", format!("Bot {token}"))
        .header("User-Agent", HTTPX_UA)
        .send()
        .await
        .map_err(|e| FetchError::Rest(format!("Could not reach Discord: {e}")))?;
    let status = resp.status().as_u16();
    if status == 404 {
        return Ok(None);
    }
    let body = resp.text().await.map_err(|e| FetchError::Rest(format!("Could not reach Discord: {e}")))?;
    if status >= 300 {
        let head: String = body.chars().take(200).collect();
        return Err(FetchError::Rest(format!("GET {path} -> {status}: {head}")));
    }
    pyjson::loads(body.as_bytes()).map(Some).map_err(|_| FetchError::Other)
}

#[derive(Debug)]
pub enum FetchError {
    /// `DiscordRestError(message)`.
    Rest(String),
    /// Anything else (Python's unhandled exception).
    Other,
}

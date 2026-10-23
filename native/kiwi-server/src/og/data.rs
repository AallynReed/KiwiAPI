//! The service dicts the board and the banners read, from the ported
//! rotation, chaos, luxion and status code.

use kiwi_core::rotations as rot;
use serde_json::{Map, Value, json};

use crate::{
    dt,
    routes::{
        rotations::{bodies, chaos, gamedata::Gamedata, store},
        trove_status,
    },
    state::AppState,
};

fn invalid(_: impl std::fmt::Debug) -> anyhow::Error {
    anyhow::anyhow!("rotation data failed validation")
}

/// `captures.get_current_challenge()`.
pub async fn challenge(state: &AppState, now_us: i64) -> anyhow::Result<Value> {
    let window = rot::challenge_window(now_us);
    let doc = store::challenge_for_window(state.mongo(), window.starts_at).await.map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(bodies::challenge_current(now_us, doc.as_ref().map(|d| (d.name.as_str(), d.captured_at))))
}

/// `chaos.get_chaos_chest()` (the item's codex blueprint is not looked up:
/// nothing drawn reads it).
pub async fn chaos_chest(state: &AppState, now_us: i64) -> anyhow::Result<Value> {
    let week = rot::chaos_chest_window(now_us);
    let db = state.mongo();
    let err = |e: store::StoreError| anyhow::anyhow!("{e}");
    let chest = match store::chaos_for_week(db, week.0).await.map_err(err)? {
        Some(capture) => {
            chaos::build(Some(&chaos::capture_item(&capture.name, week)), Some(capture.captured_at), now_us)
        }
        None => match store::chaos_feed(db).await.map_err(err)? {
            Some((cached, fetched_at)) => chaos::build(cached.as_ref(), Some(fetched_at), now_us),
            None => chaos::build(None, None, now_us),
        },
    }
    .map_err(invalid)?;
    Ok(chaos::body(&chest, None, &state.settings().api_url))
}

/// `rotations.biome_rotation()`.
pub fn biome_rotation(now_us: i64) -> anyhow::Result<Value> {
    bodies::biomes(Gamedata::get(), now_us).map_err(invalid)
}

pub fn wild_mana(now_us: i64) -> anyhow::Result<Value> {
    bodies::wild_mana(Gamedata::get(), now_us).map_err(invalid)
}

pub fn stampy(now_us: i64) -> anyhow::Result<Value> {
    bodies::stampy(Gamedata::get(), now_us).map_err(invalid)
}

/// `server_time.daily_buffs()`: the raw entry of today's buff.
pub fn daily_buffs(now_us: i64) -> Value {
    let current = Gamedata::get().daily.get(&rot::daily_buff_index(now_us).to_string()).cloned();
    json!({"current": current.unwrap_or_else(|| Value::Object(Map::new()))})
}

pub fn server_time() -> Value {
    bodies::server_time(dt::now())
}

/// `luxion.get_luxion()`.
pub async fn luxion(state: &AppState, now_us: i64) -> anyhow::Result<Value> {
    let doc = store::latest_luxion(state.mongo()).await.map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(bodies::luxion(doc.as_ref(), now_us))
}

/// `trove_status.get_status()`.
pub async fn status(state: &AppState) -> Value {
    trove_status::shared_snapshot(state).await.unwrap_or_else(trove_status::unknown_snapshot)
}

/// `updates_read.latest_version("live-us")`: what the `game_update` banner draws.
pub async fn game_update(state: &AppState) -> anyhow::Result<Value> {
    use crate::events::sources;
    Ok(match sources::latest_version(sources::mongo(state)?).await? {
        None => Value::Null,
        Some(v) => json!({
            "version_tag": v.version_tag,
            "files_added": v.files_added,
            "files_modified": v.files_modified,
            "files_removed": v.files_removed,
        }),
    })
}

/// Everything `render_board_image` gathers.
pub async fn board(state: &AppState, now_us: i64) -> anyhow::Result<Value> {
    let challenge = challenge(state, now_us).await?;
    let chaos = chaos_chest(state, now_us).await?;
    let biomes = match biome_rotation(now_us)?.get("current") {
        Some(v) if crate::features::py_truthy(v) => v.clone(),
        _ => Value::Object(Map::new()),
    };
    Ok(json!({
        "challenge": challenge,
        "chaos": chaos,
        "biomes": biomes,
        "corruxion": bodies::corruxion(now_us),
        "fluxion": bodies::fluxion(now_us),
        "server": server_time(),
        "status": status(state).await,
    }))
}

/// The sources `_announcement_content(kind)` reads (see `board::sources`).
pub async fn announcement(state: &AppState, kind: &str, now_us: i64) -> anyhow::Result<Value> {
    let mut out = Map::new();
    for src in super::board::sources(kind) {
        let v = match *src {
            "challenge" => challenge(state, now_us).await?,
            "chaos" => chaos_chest(state, now_us).await?,
            "daily" => daily_buffs(now_us),
            "server" => server_time(),
            "biomes" => biome_rotation(now_us)?,
            "wild_mana" => wild_mana(now_us)?,
            "stampy" => stampy(now_us)?,
            "corruxion" => bodies::corruxion(now_us),
            "fluxion" => bodies::fluxion(now_us),
            "luxion" => luxion(state, now_us).await?,
            "status" => status(state).await,
            "game_update" => game_update(state).await?,
            _ => Value::Null,
        };
        out.insert((*src).to_owned(), v);
    }
    Ok(Value::Object(out))
}

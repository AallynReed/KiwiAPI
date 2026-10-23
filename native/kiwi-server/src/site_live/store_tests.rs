//! The live Postgres and Redis paths, opt-in: point POSTGRES_DSN at a scratch
//! database and REDIS_URL at a throwaway server, then run
//! `cargo test -p kiwi-server site_live_store -- --ignored`.

use redis::AsyncCommands;
use serde_json::json;

use crate::{config::Settings, pyjson, state::AppState};

const SEED: &str = include_str!("../../../port/parity/site_live.seed.sql");

#[tokio::test]
#[ignore = "writes to POSTGRES_DSN / REDIS_URL"]
async fn site_live_store_reads() {
    if std::env::var("PY_UPSTREAM").is_err() {
        // SAFETY: only this ignored test runs in its process invocation.
        unsafe { std::env::set_var("PY_UPSTREAM", "http://127.0.0.1:9") };
    }
    let state = AppState::new(Settings::from_env().unwrap()).await.unwrap();
    state.pg().get().await.unwrap().batch_execute(SEED).await.unwrap();

    let (names, total) = super::search::search_players(&state, " AL ", 10).await.unwrap();
    assert_eq!(names.first().map(String::as_str), Some("Alpha"));
    assert!(names.iter().all(|n| n != "Alphaclub"), "{names:?}");
    assert_eq!(total, names.len() as i64);
    let (escaped, _) = super::search::search_players(&state, "al_pha%", 10).await.unwrap();
    assert_eq!(escaped, ["Al_pha%"]);
    assert_eq!(super::search::search_players(&state, "a", 10).await.unwrap(), (Vec::new(), 0));

    let cached = json!({"repo": "r", "repo_url": "u", "groups": [], "rate_limited": false});
    let mut r = state.redis();
    let _: () = r.set_ex("site:changelog", pyjson::dumps_spaced(&cached), 60).await.unwrap();
    assert_eq!(crate::routes::btt::site::get_changelog(&state).await.unwrap(), cached);
    let _: () = r.del("site:changelog").await.unwrap();
}

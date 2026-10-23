//! `app/bot/runner.py`: the Redis event subscriber, the minute loop aligned
//! to HH:MM:00 UTC, and the gateway session (retried once without the members
//! intent when Discord refuses it).

use std::{future::Future, panic::AssertUnwindSafe, sync::Arc, time::Duration};

use futures::{FutureExt, StreamExt};
use serde_json::Value;
use tokio::sync::watch;

use super::{Backend, Bot, announcements, cache::Cache, gateway, live::Live, liveboard};
use crate::{discord_common::i18n::Catalogs, state::AppState};

/// Runs `fut`, logging an error or panic the way Python's `except Exception`
/// around each step does.
pub async fn guard<F: Future<Output = anyhow::Result<()>>>(what: &str, fut: F) {
    match AssertUnwindSafe(fut).catch_unwind().await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => tracing::error!("{what}: {e:#}"),
        Err(_) => tracing::error!("{what}: panicked"),
    }
}

/// What one `kiwi:events` message triggers, in Python's order.
pub async fn handle_event<B: Backend>(bot: &Bot<B>, payload: &Value) {
    let Some(event) = payload.get("type").and_then(Value::as_str) else { return };
    for key in announcements::for_event(event) {
        guard(&format!("event-driven announce failed for {key}"), bot.run_announcement_type(key)).await;
    }
    if event == "market" {
        guard("market_watch run failed", bot.run_market_watch()).await;
    }
    if liveboard::BOARD_EVENTS.contains(&event) {
        guard(&format!("live board refresh failed for {event}"), bot.refresh_boards()).await;
    }
}

/// One minute-loop pass.
pub async fn minute<B: Backend>(bot: &Bot<B>) {
    guard("announcement cleanup round failed", async {
        bot.cleanup_tracked().await;
        Ok(())
    })
    .await;
    guard("announcement refresh round failed", bot.refresh_tracked()).await;
    guard("live board refresh round failed", bot.refresh_boards()).await;
    guard("announcement backstop failed", bot.run_all_announcements()).await;
    bot.record_presence().await;
}

/// `_seconds_until(0)`: to the next HH:MM:00 (a full minute when on it).
pub fn until_next_minute(now_us: i64) -> Duration {
    Duration::from_micros((60_000_000 - now_us.rem_euclid(60_000_000)) as u64)
}

async fn wait_ready(ready: &mut watch::Receiver<bool>) {
    let _ = ready.wait_for(|r| *r).await;
}

async fn minute_loop<B: Backend>(bot: Arc<Bot<B>>, mut ready: watch::Receiver<bool>) {
    wait_ready(&mut ready).await;
    loop {
        tokio::time::sleep(until_next_minute(bot.b.now_us())).await;
        minute(&bot).await;
    }
}

async fn listen<B: Backend>(bot: &Bot<B>, url: &str, channel: &str) -> redis::RedisResult<()> {
    let client = redis::Client::open(url)?;
    let mut pubsub = client.get_async_pubsub().await?;
    pubsub.subscribe(channel).await?;
    tracing::info!("subscribed to events channel {channel:?}");
    let mut messages = pubsub.on_message();
    while let Some(msg) = messages.next().await {
        if let Ok(payload @ Value::Object(_)) = crate::pyjson::loads(msg.get_payload_bytes()) {
            handle_event(bot, &payload).await;
        }
    }
    Ok(())
}

async fn subscribe<B: Backend>(
    bot: Arc<Bot<B>>,
    url: Option<String>,
    channel: String,
    mut ready: watch::Receiver<bool>,
) {
    wait_ready(&mut ready).await;
    let Some(url) = url.filter(|u| !u.is_empty()) else {
        tracing::warn!("Redis not configured - announcements use the poll backstop only");
        return;
    };
    loop {
        if let Err(e) = listen(&bot, &url, &channel).await {
            tracing::warn!("events subscription: {e}");
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut term = match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(s) => s,
            Err(_) => return std::future::pending().await,
        };
        tokio::select! {
            _ = term.recv() => {}
            _ = tokio::signal::ctrl_c() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

/// The bot process: until SIGTERM/SIGINT, or an error on a fatal close.
pub async fn run(state: AppState) -> anyhow::Result<()> {
    let settings = state.settings().clone();
    let Some(token) = settings.discord_bot_token.clone().filter(|t| !t.is_empty()) else {
        anyhow::bail!("DISCORD_BOT_TOKEN is required to run the Kiwi bot.");
    };
    let _ = crate::http::tls_config();
    let bot = Arc::new(Bot::new(
        Live::new(state, &token),
        Arc::new(Cache::default()),
        Catalogs::global(),
        settings.asset_url(),
    ));
    let (ready_tx, ready_rx) = watch::channel(false);
    let (stop_tx, stop_rx) = watch::channel(false);
    let tasks = [
        tokio::spawn(subscribe(
            bot.clone(),
            settings.redis_url.clone(),
            settings.events_channel.clone(),
            ready_rx.clone(),
        )),
        tokio::spawn(minute_loop(bot.clone(), ready_rx)),
    ];
    tokio::spawn(async move {
        shutdown_signal().await;
        tracing::info!("shutdown signal received - closing the gateway connection");
        let _ = stop_tx.send(true);
    });
    let members = settings.bot_members_intent;
    let mut exit = gateway::run(bot.clone(), token.clone(), members, ready_tx.clone(), stop_rx.clone()).await;
    if matches!(exit, gateway::Exit::DisallowedIntents) && members {
        tracing::warn!(
            "Server Members Intent is not enabled in the Discord Developer Portal - starting the bot without it \
             (Discord avatar auto-sync is off; avatars still refresh on the user's next login). Enable the intent \
             to turn it on, or set BOT_MEMBERS_INTENT=false to silence this."
        );
        exit = gateway::run(bot.clone(), token, false, ready_tx, stop_rx).await;
    }
    for t in tasks {
        t.abort();
    }
    match exit {
        gateway::Exit::Stopped => Ok(()),
        gateway::Exit::DisallowedIntents => anyhow::bail!("the gateway refused the requested intents (4014)"),
        gateway::Exit::Closed(code) => anyhow::bail!("the gateway closed for good (code {code:?})"),
    }
}

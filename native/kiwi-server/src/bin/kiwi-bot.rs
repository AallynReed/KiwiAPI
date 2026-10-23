//! The Discord gateway bot (the `bot` container). Never run two against one
//! bot token.

use kiwi_server::{config::Settings, state::AppState};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = dotenvy::dotenv();
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stderr()))
        .init();
    let state = AppState::new(Settings::from_env()?).await?;
    kiwi_server::bot::runner::run(state).await
}

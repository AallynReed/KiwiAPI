//! The custom art release worker (the `builder` container), in place of
//! `python -m app.custom_art.builder`.

use kiwi_server::{config::Settings, custom_art::builder::Builder, state::AppState};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = dotenvy::dotenv();
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stderr()))
        .init();
    let mut vars: Vec<(String, String)> =
        std::env::vars_os().filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?))).collect();
    if !vars.iter().any(|(k, v)| k.eq_ignore_ascii_case("PY_UPSTREAM") && !v.is_empty()) {
        vars.push(("PY_UPSTREAM".into(), "http://unused".into()));
    }
    let state = AppState::new(Settings::from_vars(vars)?).await?;
    Builder::new(state).run_forever().await
}

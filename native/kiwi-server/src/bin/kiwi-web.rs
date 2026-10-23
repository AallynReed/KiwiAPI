//! The website container (`web`), in front of the Python website (`web-py`,
//! `PY_WEB_UPSTREAM`) the way kiwi-server fronts api-py. No database, no
//! loops but the page-view beacon and the GeoIP refresh.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use kiwi_server::{config::Settings, state::AppState, web};

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = dotenvy::dotenv();
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stderr()))
        .init();
    let mut vars: Vec<(String, String)> =
        std::env::vars_os().filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?))).collect();
    if let Some(up) = vars.iter().find(|(k, v)| k.eq_ignore_ascii_case("PY_WEB_UPSTREAM") && !v.is_empty()).cloned() {
        vars.retain(|(k, _)| !k.eq_ignore_ascii_case("PY_UPSTREAM"));
        vars.push(("PY_UPSTREAM".into(), up.1));
    }
    let settings = Settings::from_vars(vars)?;
    let bind: SocketAddr = settings.bind.parse()?;
    let site = Arc::new(web::Web::new(&settings, kiwi_server::middleware::MwConfig::from_env()));
    let state = AppState::new(settings).await?;
    site.start();
    let app = web::app(state, site.clone());

    let listener = tokio::net::TcpListener::bind(bind).await?;
    tracing::info!("kiwi-web listening on {bind}");
    axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
        .with_graceful_shutdown(shutdown())
        .await?;
    let _ = tokio::time::timeout(Duration::from_secs(3), site.beacon.flush(&site)).await;
    Ok(())
}

async fn shutdown() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let term = async {
        if let Ok(mut s) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            s.recv().await;
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! { _ = ctrl_c => {}, _ = term => {} }
}

use std::net::SocketAddr;

use std::time::Duration;

use kiwi_server::{config::Settings, routes, state::AppState, usage};

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = dotenvy::dotenv();
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let settings = Settings::from_env()?;
    let bind: SocketAddr = settings.bind.parse()?;
    let state = AppState::new(settings).await?;
    let app = routes::app(state);

    let listener = tokio::net::TcpListener::bind(bind).await?;
    tracing::info!("kiwi-server listening on {bind}");
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let server = axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()).with_graceful_shutdown(
        async move {
            shutdown().await;
            let _ = tx.send(());
        },
    );
    // Endless SSE streams would hold a graceful shutdown open; cut them off
    // after 3s like uvicorn's --timeout-graceful-shutdown.
    tokio::select! {
        res = server => res?,
        _ = async {
            let _ = rx.await;
            tokio::time::sleep(Duration::from_secs(3)).await;
        } => tracing::warn!("graceful shutdown timed out"),
    }
    usage::shutdown().await;
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

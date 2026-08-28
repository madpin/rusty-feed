//! RustSS HTTP server entry point.
//!
//! Loads and validates configuration, initializes telemetry, and serves the
//! current HTTP surface (health + metrics). Auth, the reading UI, the
//! native/GReader APIs, and the optional embedded worker are wired in later
//! milestones (see `docs/DEVELOPMENT_PLAN.md`).

use std::net::SocketAddr;

use anyhow::Context;
use rustss_core::AppConfig;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Fail fast on bad config (before we have logging).
    let config = AppConfig::load().context("failed to load configuration")?;

    rustss_core::telemetry::init_tracing(&config.log_level);
    let metrics = rustss_core::telemetry::install_metrics();

    let addr: SocketAddr = config
        .bind_addr
        .parse()
        .with_context(|| format!("invalid BIND_ADDR: {}", config.bind_addr))?;

    let state = rustss_web::AppState::new(move || metrics.render());
    let app = rustss_web::router(state);

    tracing::info!("{} starting on http://{addr}", rustss_core::banner());

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("failed to bind {addr}"))?;

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("server error")?;

    tracing::info!("shutdown complete");
    Ok(())
}

/// Wait for Ctrl-C or SIGTERM for graceful shutdown.
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl-C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    tracing::info!("shutdown signal received");
}

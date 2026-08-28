//! RustSS standalone AGENTS worker entry point.
//!
//! Loads configuration and initializes telemetry, then idles until shutdown.
//! The scheduler, durable queue, and concrete agents are implemented in
//! milestone M7 (see `docs/DEVELOPMENT_PLAN.md` and `docs/ARCHITECTURE.md` §8).

use anyhow::Context;
use rustss_core::AppConfig;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = AppConfig::load().context("failed to load configuration")?;
    rustss_core::telemetry::init_tracing(&config.log_level);

    tracing::info!("{} worker starting", rustss_core::banner());
    tracing::warn!("no agents registered yet (implemented in milestone M7)");

    shutdown_signal().await;
    tracing::info!("worker shutdown complete");
    Ok(())
}

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
}

//! RustSS HTTP server entry point.
//!
//! Milestone M0/M1: initializes tracing and serves the health surface. Auth,
//! the reading UI, the native/GReader APIs, and the optional embedded worker are
//! wired in later milestones (see `docs/DEVELOPMENT_PLAN.md`).

use std::net::SocketAddr;

use anyhow::Context;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing();

    // M1 will replace this with the typed, validated `AppConfig` (figment).
    let bind_addr = std::env::var("BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".to_string());
    let addr: SocketAddr = bind_addr
        .parse()
        .with_context(|| format!("invalid BIND_ADDR: {bind_addr}"))?;

    let app = rustss_web::router();

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

/// Minimal tracing setup. M1 centralizes this in `rustss_core::telemetry`
/// (JSON/pretty output, request IDs, metrics).
fn init_tracing() {
    use tracing_subscriber::{fmt, prelude::*, EnvFilter};
    let filter = EnvFilter::try_from_env("LOG_LEVEL")
        .or_else(|_| EnvFilter::try_from_default_env())
        .unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer())
        .init();
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

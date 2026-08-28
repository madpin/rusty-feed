//! RustSS standalone AGENTS worker entry point.
//!
//! Milestone M0: initializes tracing and idles until shutdown. The scheduler,
//! durable queue, and concrete agents are implemented in milestone M7 (see
//! `docs/DEVELOPMENT_PLAN.md` and `docs/ARCHITECTURE.md` §8).

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing();

    tracing::info!("{} worker starting", rustss_core::banner());
    tracing::warn!("no agents registered yet (implemented in milestone M7)");

    shutdown_signal().await;
    tracing::info!("worker shutdown complete");
    Ok(())
}

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

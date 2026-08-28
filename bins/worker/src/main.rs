//! RustSS standalone AGENTS worker: runs the scheduler + durable-queue consumers
//! (feed refresh/fetch today; more agents in later milestones). Runnable
//! separately from the web server, or embedded in it via `RUN_EMBEDDED_WORKER`.

use std::time::Duration;

use anyhow::Context;
use rustss_agents::{Worker, WorkerConfig};
use rustss_core::AppConfig;
use rustss_db::Db;
use rustss_feeds::SafeFetchClient;
use tokio::sync::watch;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = AppConfig::load().context("failed to load configuration")?;
    rustss_core::telemetry::init_tracing(&config.log_level);

    let db = Db::connect(&config.database_url)
        .await
        .context("failed to connect to the database")?;
    db.migrate().await.context("failed to apply migrations")?;

    let fetch =
        SafeFetchClient::new(Duration::from_secs(30)).context("failed to build HTTP client")?;
    let worker = Worker::new(
        db,
        fetch,
        WorkerConfig {
            concurrency: config.feed_refresh_concurrency,
            ..WorkerConfig::default()
        },
    );

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let handle = tokio::spawn(async move { worker.run(shutdown_rx).await });

    tracing::info!("{} worker started", rustss_core::banner());
    shutdown_signal().await;
    tracing::info!("worker shutting down");
    let _ = shutdown_tx.send(true);
    let _ = handle.await;
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

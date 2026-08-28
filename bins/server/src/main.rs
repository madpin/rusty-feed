//! RustSS HTTP server entry point.
//!
//! Loads and validates configuration, initializes telemetry, and serves the
//! current HTTP surface (health + metrics). Auth, the reading UI, the
//! native/GReader APIs, and the optional embedded worker are wired in later
//! milestones (see `docs/DEVELOPMENT_PLAN.md`).

use std::net::SocketAddr;

use anyhow::Context;
use rustss_cache::{Cache, CacheConfig};
use rustss_core::AppConfig;
use rustss_db::Db;
use rustss_web::{AppSettings, AppState};

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

    // Connect the database and apply migrations (idempotent) so a fresh
    // `cargo run` / container boot is ready to serve.
    let db = Db::connect(&config.database_url)
        .await
        .context("failed to connect to the database")?;
    db.migrate().await.context("failed to apply migrations")?;

    // Best-effort cache (degrades to L1-only if Redis is unavailable).
    let cache = Cache::new(CacheConfig {
        enabled: config.cache_enabled,
        redis_url: config.redis_url.clone(),
        redis_password: config
            .redis_password
            .as_ref()
            .map(|s| s.expose().to_string()),
        l1_max_capacity: 10_000,
    })
    .await;

    let base_url = config.base_url();
    let settings = AppSettings {
        registration_mode: config.registration_mode,
        cookie_secure: base_url.starts_with("https://"),
        base_url,
    };

    let state = AppState::new(db, cache, settings, move || metrics.render());
    let app = rustss_web::router(state);

    tracing::info!("{} starting on http://{addr}", rustss_core::banner());

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("failed to bind {addr}"))?;

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
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

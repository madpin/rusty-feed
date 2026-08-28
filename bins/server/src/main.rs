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
use rustss_web::{AppSettings, AppState, OAuthProvider, ProviderKind};

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
    let oauth_providers = build_oauth_providers(&config);
    if !oauth_providers.is_empty() {
        let names: Vec<&str> = oauth_providers.iter().map(|p| p.slug.as_str()).collect();
        tracing::info!("OAuth providers enabled: {}", names.join(", "));
    }
    // One embedding model per instance, shared by the search path and worker.
    let embedder = rustss_embed::build_embedder(rustss_embed::EmbedConfig {
        provider: config.embedding_provider,
        model: config.embedding_model.clone(),
        openai_api_key: config
            .openai_api_key
            .as_ref()
            .map(|s| s.expose().to_string()),
        openai_base_url: config.openai_base_url.clone(),
    })
    .context("failed to build embedder")?;

    // Optional LLM client for summaries (None ⇒ summarization disabled).
    let llm = rustss_llm::build_client(rustss_llm::LlmConfig {
        provider: config.llm_provider,
        model: config.llm_summary_model.clone(),
        openai_api_key: config
            .openai_api_key
            .as_ref()
            .map(|s| s.expose().to_string()),
        openai_base_url: config.openai_base_url.clone(),
        ollama_base_url: config.ollama_base_url.clone(),
    });

    let settings = AppSettings {
        registration_mode: config.registration_mode,
        cookie_secure: base_url.starts_with("https://"),
        base_url,
        oauth_providers,
        summarization_enabled: llm.is_some(),
    };

    let state = AppState::new(db, cache, settings, move || metrics.render())
        .with_embedder(embedder.clone());

    // Embedded AGENTS worker (default): refreshes feeds in the background.
    // Disable with RUN_EMBEDDED_WORKER=false to run `rustss-worker` separately.
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let worker_handle = if config.run_embedded_worker {
        let worker = rustss_agents::Worker::new(
            state.db.clone(),
            state.fetch.clone(),
            embedder,
            llm,
            rustss_agents::WorkerConfig {
                concurrency: config.feed_refresh_concurrency,
                embed_on_ingest: config.embedding_auto_generate,
                ..rustss_agents::WorkerConfig::default()
            },
        );
        tracing::info!("embedded AGENTS worker enabled");
        Some(tokio::spawn(async move { worker.run(shutdown_rx).await }))
    } else {
        None
    };

    let app = rustss_web::router(state);
    tracing::info!("{} starting on http://{addr}", rustss_core::banner());

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("failed to bind {addr}"))?;

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        shutdown_signal().await;
        let _ = shutdown_tx.send(true); // stop the embedded worker too
    })
    .await
    .context("server error")?;

    if let Some(handle) = worker_handle {
        let _ = handle.await;
    }
    tracing::info!("shutdown complete");
    Ok(())
}

/// Assemble the enabled OAuth providers from configuration.
fn build_oauth_providers(config: &AppConfig) -> Vec<OAuthProvider> {
    use rustss_core::Secret;
    let mut providers = Vec::new();

    if let (Some(id), Some(secret)) = (&config.google_client_id, &config.google_client_secret) {
        providers.push(OAuthProvider {
            slug: "google".to_string(),
            display_name: "Google".to_string(),
            client_id: id.clone(),
            client_secret: Secret::new(secret.expose()),
            auth_url: "https://accounts.google.com/o/oauth2/v2/auth".to_string(),
            token_url: "https://oauth2.googleapis.com/token".to_string(),
            userinfo_url: "https://openidconnect.googleapis.com/v1/userinfo".to_string(),
            scopes: "openid email profile".to_string(),
            kind: ProviderKind::Oidc,
        });
    }

    if let (Some(id), Some(secret)) = (&config.github_client_id, &config.github_client_secret) {
        providers.push(OAuthProvider {
            slug: "github".to_string(),
            display_name: "GitHub".to_string(),
            client_id: id.clone(),
            client_secret: Secret::new(secret.expose()),
            auth_url: "https://github.com/login/oauth/authorize".to_string(),
            token_url: "https://github.com/login/oauth/access_token".to_string(),
            userinfo_url: "https://api.github.com/user".to_string(),
            scopes: "read:user user:email".to_string(),
            kind: ProviderKind::GitHub,
        });
    }

    if let (Some(id), Some(secret), Some(auth), Some(token), Some(userinfo)) = (
        &config.oauth_client_id,
        &config.oauth_client_secret,
        &config.oauth_auth_url,
        &config.oauth_token_url,
        &config.oauth_userinfo_url,
    ) {
        providers.push(OAuthProvider {
            slug: "oauth".to_string(),
            display_name: config
                .oauth_provider_name
                .clone()
                .unwrap_or_else(|| "SSO".to_string()),
            client_id: id.clone(),
            client_secret: Secret::new(secret.expose()),
            auth_url: auth.clone(),
            token_url: token.clone(),
            userinfo_url: userinfo.clone(),
            scopes: "openid email profile".to_string(),
            kind: ProviderKind::Oidc,
        });
    }

    providers
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

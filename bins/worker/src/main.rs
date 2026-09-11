//! Rusty Feed standalone AGENTS worker: runs the scheduler + durable-queue consumers
//! (feed refresh/fetch today; more agents in later milestones). Runnable
//! separately from the web server, or embedded in it via `RUN_EMBEDDED_WORKER`.

use std::time::Duration;

use anyhow::Context;
use rusty_feed_agents::{Worker, WorkerConfig};
use rusty_feed_core::AppConfig;
use rusty_feed_db::Db;
use rusty_feed_feeds::SafeFetchClient;
use tokio::sync::watch;

#[path = "../../systemd.rs"]
mod systemd;

// glibc's default allocator fragments badly across the per-thread arenas this
// process creates under concurrent HTML parsing/sanitization; jemalloc
// decays and returns freed arena memory to the OS instead. See docs/FINDINGS.md.
#[cfg(not(target_env = "msvc"))]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = AppConfig::load().context("failed to load configuration")?;
    let systemd = systemd::Notifier::detect(config.systemd_notify_enabled);
    let tracking_url_policy = config
        .tracking_url_policy()
        .map_err(|error| anyhow::anyhow!("invalid tracking URL policy: {error}"))?;
    let tracking_pixel_policy = config
        .tracking_pixel_policy()
        .map_err(|error| anyhow::anyhow!("invalid tracking pixel policy: {error}"))?;
    rusty_feed_core::telemetry::init_tracing(&config.log_level);

    let db = Db::connect(&config.database_url)
        .await
        .context("failed to connect to the database")?;
    db.migrate().await.context("failed to apply migrations")?;
    let embedding_model =
        rusty_feed_embed::canonical_model(config.embedding_provider, &config.embedding_model)
            .context("invalid embedding model")?;
    rusty_feed_db::embedding_space::ensure(
        db.pool(),
        &embedding_model,
        rusty_feed_embed::EMBEDDING_DIM as i32,
    )
    .await
    .context("embedding-space configuration is incompatible")?;

    if config.feeds_allow_private_network {
        tracing::warn!("FEEDS_ALLOW_PRIVATE_NETWORK is on — feeds may reach private addresses");
    }
    let fetch = SafeFetchClient::with_options(rusty_feed_feeds::SafeFetchOptions {
        timeout: Duration::from_millis(config.extraction_timeout_ms),
        allow_private: config.feeds_allow_private_network,
        user_agent: config.outbound_user_agent.clone(),
        http1_only: config.outbound_http1_only,
    })
    .context("failed to build HTTP client")?;
    // EMBEDDINGS_ENABLED=false skips building a real embedder entirely (no
    // model download, no OpenAI credential requirement); embed_article stays
    // unscheduled and saved_search_match falls back to keyword-only matching.
    let embedder: std::sync::Arc<dyn rusty_feed_embed::Embedder> = if config.embeddings_enabled {
        rusty_feed_embed::build_embedder(rusty_feed_embed::EmbedConfig {
            provider: config.embedding_provider,
            model: embedding_model,
            openai_api_key: config
                .openai_api_key
                .as_ref()
                .map(|s| s.expose().to_string()),
            openai_base_url: config.openai_base_url.clone(),
            model_cache_dir: config.model_cache_dir.clone(),
        })
        .context("failed to build embedder")?
    } else {
        tracing::info!("embeddings disabled (EMBEDDINGS_ENABLED=false): worker skips model load");
        std::sync::Arc::new(rusty_feed_embed::DisabledEmbedder)
    };
    let llm = rusty_feed_llm::build_client(rusty_feed_llm::LlmConfig {
        provider: config.llm_provider,
        model: config.llm_summary_model.clone(),
        openai_api_key: config
            .openai_api_key
            .as_ref()
            .map(|s| s.expose().to_string()),
        openai_base_url: config.openai_base_url.clone(),
        ollama_base_url: config.ollama_base_url.clone(),
    });
    let llm_cascade = std::sync::Arc::new(rusty_feed_llm::ProviderCascade::new(
        db.clone(),
        config.encryption_secret.expose().as_bytes(),
        llm.clone(),
    ));
    let worker = Worker::new(
        db,
        fetch,
        embedder,
        llm,
        Some(llm_cascade),
        WorkerConfig {
            concurrency: config.feed_refresh_concurrency,
            interactive_concurrency: config.interactive_job_concurrency,
            scheduler_enabled: config.enable_cron_jobs,
            feed_refresh_schedule: rusty_feed_core::cron::CronSchedule::parse(
                &config.feed_refresh_schedule,
            )
            .map_err(|error| anyhow::anyhow!("invalid FEED_REFRESH_SCHEDULE: {error}"))?,
            cleanup_schedule: rusty_feed_core::cron::CronSchedule::parse(&config.cleanup_schedule)
                .map_err(|error| anyhow::anyhow!("invalid CLEANUP_SCHEDULE: {error}"))?,
            extraction_config: extraction_config(&config),
            embed_on_ingest: config.embedding_auto_generate,
            embeddings_enabled: config.embeddings_enabled,
            auto_summarize: config.llm_auto_summarize,
            auto_summarize_min_chars: config.llm_auto_summarize_min_chars,
            eager_extraction: config.article_page_eager_extraction,
            tracking_url_policy,
            tracking_pixel_policy,
            article_retention_days: config.article_retention_days,
            retention_age_basis: config.retention_age_basis,
            integration_credentials: rusty_feed_core::crypto::Cryptor::new_with_context(
                config.encryption_secret.expose().as_bytes(),
                b"rusty-feed/integration-credentials/v1",
            ),
            acquisition_credentials: rusty_feed_core::crypto::Cryptor::new_with_context(
                config.encryption_secret.expose().as_bytes(),
                b"rusty-feed/acquisition-profile-secrets/v1",
            ),
            youtube_duration_enabled: config.youtube_duration_enabled,
            youtube_duration_daily_budget: config.youtube_duration_daily_budget,
            youtube_duration_cache_days: i64::from(config.youtube_duration_cache_days),
            ai_budget: config.ai_budget(),
            feed_max_items_per_refresh: config.feed_max_items_per_refresh,
            demo_mode: config.demo_mode,
            demo_reset_schedule: rusty_feed_core::cron::CronSchedule::parse(
                &config.demo_reset_schedule,
            )
            .map_err(|error| anyhow::anyhow!("invalid DEMO_RESET_SCHEDULE: {error}"))?,
            ..WorkerConfig::default()
        },
    );

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let watchdog_handle = systemd.spawn_watchdog(shutdown_tx.subscribe());
    let handle = tokio::spawn(async move { worker.run(shutdown_rx).await });

    tracing::info!("{} worker started", rusty_feed_core::banner());
    systemd.ready("Rusty Feed worker is ready to process jobs");
    shutdown_signal().await;
    tracing::info!("worker shutting down");
    systemd.stopping("Rusty Feed worker is draining jobs");
    let _ = shutdown_tx.send(true);
    let _ = handle.await;
    if let Some(handle) = watchdog_handle {
        let _ = handle.await;
    }
    tracing::info!("worker shutdown complete");
    Ok(())
}

fn extraction_config(config: &AppConfig) -> rusty_feed_extract::ExtractConfig {
    let chromium = if config.chromium_enabled {
        config.chromium_cdp_url.as_ref().map(|endpoint| {
            rusty_feed_extract::chromium::ChromiumConfig {
                endpoint: endpoint.clone(),
                max_concurrency: config.feed_refresh_concurrency.min(4),
                render_wait: Duration::from_millis(750),
                timeout: Duration::from_millis(config.extraction_timeout_ms),
            }
        })
    } else {
        None
    };
    rusty_feed_extract::ExtractConfig {
        base_delay: Duration::from_millis(config.extraction_delay_ms),
        chromium,
        ..rusty_feed_extract::ExtractConfig::default()
    }
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

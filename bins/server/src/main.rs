//! Rusty Feed HTTP server entry point.
//!
//! Loads and validates configuration, initializes telemetry, and serves the
//! current HTTP surface (health + metrics). Auth, the reading UI, the
//! native/GReader APIs, and the optional embedded worker are wired in later
//! milestones (see `docs/DEVELOPMENT_PLAN.md`).

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Context;
use rusty_feed_cache::{Cache, CacheConfig};
use rusty_feed_core::AppConfig;
use rusty_feed_db::Db;
use rusty_feed_embed::Embedder;
use rusty_feed_web::{
    AppSettings, AppState, CaptchaSettings, OAuthProvider, ProviderKind, RateLimitSettings,
    ReverseProxyAuthSettings,
};

#[path = "../../systemd.rs"]
mod systemd;

// glibc's default allocator fragments badly across the per-thread arenas this
// process creates under concurrent HTML parsing/sanitization (embedded
// worker + HTTP server in one process); jemalloc decays and returns freed
// arena memory to the OS instead. See docs/FINDINGS.md.
#[cfg(not(target_env = "msvc"))]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Fail fast on bad config (before we have logging).
    let config = AppConfig::load().context("failed to load configuration")?;
    let systemd = systemd::Notifier::detect(config.systemd_notify_enabled);

    rusty_feed_core::telemetry::init_tracing(&config.log_level);
    let metrics = rusty_feed_core::telemetry::install_metrics();

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

    // Best-effort cache (degrades to L1-only if Redis is unavailable).
    let cache = Cache::new(CacheConfig {
        enabled: config.cache_enabled,
        redis_url: config.redis_url.clone(),
        redis_password: config
            .redis_password
            .as_ref()
            .map(|s| s.expose().to_string()),
        l1_max_capacity: 10_000,
        namespace: config.cache_namespace.clone(),
    })
    .await;

    let base_url = config.base_url();
    let tracking_url_policy = config
        .tracking_url_policy()
        .map_err(|error| anyhow::anyhow!("invalid tracking URL policy: {error}"))?;
    let tracking_pixel_policy = config
        .tracking_pixel_policy()
        .map_err(|error| anyhow::anyhow!("invalid tracking pixel policy: {error}"))?;
    let oauth_providers = build_oauth_providers(&config);
    if !oauth_providers.is_empty() {
        let names: Vec<&str> = oauth_providers.iter().map(|p| p.slug.as_str()).collect();
        tracing::info!("OAuth providers enabled: {}", names.join(", "));
    }
    // One embedding model per instance, shared by the search path and worker.
    // EMBEDDINGS_ENABLED=false skips building a real embedder entirely (no
    // model download, no OpenAI credential requirement); every caller treats
    // the resulting refusal as "semantic search unavailable".
    let embedder: Arc<dyn Embedder> = if config.embeddings_enabled {
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
        tracing::info!("embeddings disabled (EMBEDDINGS_ENABLED=false): search is keyword-only");
        Arc::new(rusty_feed_embed::DisabledEmbedder)
    };

    // Optional LLM client for summaries (None ⇒ summarization disabled).
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
    // Surface the environment-configured provider (the cascade fallback) on the
    // AI providers screen when one is actually usable.
    let env_llm = llm.is_some().then(|| rusty_feed_web::EnvLlmProvider {
        provider: config.llm_provider.as_str().to_string(),
        model: config.llm_summary_model.clone(),
        base_url: match config.llm_provider {
            rusty_feed_core::config::LlmProvider::Openai => config
                .openai_base_url
                .clone()
                .unwrap_or_else(|| "https://api.openai.com".to_string()),
            rusty_feed_core::config::LlmProvider::Ollama => config.ollama_base_url.clone(),
        },
        has_key: config.openai_api_key.is_some(),
    });

    let settings = AppSettings {
        registration_mode: config.registration_mode,
        reverse_proxy_auth: ReverseProxyAuthSettings {
            enabled: config.reverse_proxy_auth_enabled,
            trusted_cidrs: comma_values(&config.reverse_proxy_auth_trusted_cidrs)
                .map(str::parse)
                .collect::<Result<Vec<_>, _>>()
                .context("invalid reverse-proxy trusted CIDR")?,
            user_header: config.reverse_proxy_auth_user_header.clone(),
            name_header: config.reverse_proxy_auth_name_header.clone(),
            auto_provision: config.reverse_proxy_auth_auto_provision,
            allowed_domains: comma_values(&config.reverse_proxy_auth_allowed_domains)
                .map(|value| value.to_ascii_lowercase())
                .collect(),
            admin_emails: comma_values(&config.reverse_proxy_auth_admin_emails)
                .map(|value| value.to_ascii_lowercase())
                .collect(),
            logout_url: config.reverse_proxy_auth_logout_url.clone(),
        },
        cookie_secure: base_url.starts_with("https://"),
        base_url,
        pwa_name: config.pwa_name.clone(),
        pwa_short_name: config.pwa_short_name.clone(),
        pwa_theme_color: config.pwa_theme_color.clone(),
        pwa_background_color: config.pwa_background_color.clone(),
        external_links_new_tab_required: config.external_links_new_tab_required,
        public_article_sharing_enabled: config.public_article_sharing_enabled,
        public_share_default_lifetime_days: config.public_share_default_lifetime_days,
        public_share_max_lifetime_days: config.public_share_max_lifetime_days,
        public_share_allow_infinite: config.public_share_allow_infinite,
        api_enabled: config.api_enabled,
        greader_api_enabled: config.greader_api_enabled,
        fever_api_enabled: config.fever_api_enabled,
        custom_css_enabled: config.custom_css_enabled,
        custom_javascript_enabled: config.custom_javascript_enabled,
        ui_customization_max_bytes: config.ui_customization_max_bytes,
        passkeys_enabled: config.passkeys_enabled,
        passkey_rp_name: config.passkey_rp_name.clone(),
        oauth_providers,
        // The managed-provider screen can enable summarization without a restart.
        summarization_enabled: true,
        auto_summarize_default: config.llm_auto_summarize,
        auto_summarize_min_chars_default: config.llm_auto_summarize_min_chars,
        env_llm,
        auto_embed: config.embedding_auto_generate,
        embeddings_enabled: config.embeddings_enabled,
        search_mode: config.search_mode,
        search_text_config: config.search_text_config,
        search_semantic_weight_percent: config.search_semantic_weight_percent,
        media_proxy_enabled: config.media_proxy_enabled,
        media_proxy_max_bytes: config.media_proxy_max_bytes,
        media_proxy_cache_budget_bytes: config.media_proxy_cache_budget_bytes,
        media_proxy_cache_ttl: std::time::Duration::from_secs(config.media_proxy_cache_ttl_seconds),
        media_proxy_negative_ttl: std::time::Duration::from_secs(
            config.media_proxy_negative_ttl_seconds,
        ),
        media_proxy_extra_inline_mime_types: config.media_proxy_extra_inline_mime_types(),
        tracking_url_policy: tracking_url_policy.clone(),
        tracking_pixel_policy: tracking_pixel_policy.clone(),
        youtube_policy: config
            .youtube_policy()
            .map_err(|error| anyhow::anyhow!("invalid YouTube player policy: {error}"))?,
        youtube_duration_enabled: config.youtube_duration_enabled,
        youtube_duration_cache_days: config.youtube_duration_cache_days,
        outbound_user_agent: config.outbound_user_agent.clone(),
        outbound_http1_only: config.outbound_http1_only,
        trusted_proxy_cidrs: comma_values(&config.trusted_proxy_cidrs)
            .map(str::parse)
            .collect::<Result<Vec<_>, _>>()
            .context("invalid TRUSTED_PROXY_CIDRS")?,
        ai_budget: config.ai_budget(),
        rate_limits: RateLimitSettings {
            auth_per_min: config.rate_limit_auth_per_min,
            api_per_min: config.rate_limit_api_per_min,
            feed_add_per_min: config.rate_limit_feed_add_per_min,
            search_per_min: config.rate_limit_search_per_min,
            summarize_per_min: config.rate_limit_summarize_per_min,
            opml_per_min: config.rate_limit_opml_per_min,
            media_per_min: config.rate_limit_media_per_min,
        },
        quotas: config.quotas(),
        email_verification_required: config.email_verification_required,
        min_password_length: config.min_password_length,
        login_max_failures: config.login_max_failures,
        login_lockout_seconds: config.login_lockout_seconds,
        captcha: config
            .active_captcha_provider()
            .map(|provider| CaptchaSettings {
                provider,
                site_key: config.captcha_site_key.clone().unwrap_or_default(),
                secret: config
                    .captcha_secret
                    .as_ref()
                    .map(|secret| secret.expose().to_string())
                    .unwrap_or_default(),
            }),
        legal_contact_email: config.legal_contact_email.clone(),
        legal_entity_name: config
            .legal_entity_name
            .clone()
            .unwrap_or_else(|| config.pwa_name.clone()),
        require_tos_acceptance: config.require_tos_acceptance,
        demo_mode: config.demo_mode,
        demo_max_subscriptions: config.demo_max_subscriptions,
        demo_admin_readonly: config.demo_admin_readonly,
        demo_session_ttl_hours: config.demo_session_ttl_hours,
    };

    // SSRF-safe fetch client (opt into private/loopback for trusted intranet
    // feeds or local testing).
    if config.feeds_allow_private_network {
        tracing::warn!("FEEDS_ALLOW_PRIVATE_NETWORK is on — feeds may reach private addresses");
    }
    let fetch =
        rusty_feed_feeds::SafeFetchClient::with_options(rusty_feed_feeds::SafeFetchOptions {
            timeout: std::time::Duration::from_millis(config.extraction_timeout_ms),
            allow_private: config.feeds_allow_private_network,
            user_agent: config.outbound_user_agent.clone(),
            http1_only: config.outbound_http1_only,
        })
        .context("failed to build HTTP client")?;

    // Outbound mailer for email verification. A missing/broken SMTP config
    // degrades to a Noop mailer so signup never breaks (FND-191).
    let mailer = match config.smtp_url.as_ref() {
        Some(url) => {
            let from = config
                .smtp_from
                .clone()
                .unwrap_or_else(|| "Rusty Feed <no-reply@localhost>".to_string());
            match rusty_feed_web::mailer::Mailer::from_smtp_url(url.expose(), &from) {
                Ok(mailer) => mailer,
                Err(error) => {
                    tracing::error!(%error, "invalid SMTP configuration; email verification disabled");
                    rusty_feed_web::mailer::Mailer::Noop
                }
            }
        }
        None => rusty_feed_web::mailer::Mailer::Noop,
    };

    let state = AppState::new(db, cache, settings, move || metrics.render())
        .with_embedder(embedder.clone())
        .with_live_view_secret(config.session_secret.expose().as_bytes())
        .with_public_share_secret(config.encryption_secret.expose().as_bytes())
        .with_llm_secret(config.encryption_secret.expose().as_bytes())
        .with_integration_secret(config.encryption_secret.expose().as_bytes())
        .with_acquisition_secret(config.encryption_secret.expose().as_bytes())
        .with_mailer(mailer)
        .with_fetch(fetch);

    // Embedded AGENTS worker (default): refreshes feeds in the background.
    // Disable with RUN_EMBEDDED_WORKER=false to run `rusty-feed-worker` separately.
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let watchdog_handle = systemd.spawn_watchdog(shutdown_tx.subscribe());
    let worker_handle = if config.run_embedded_worker {
        let worker = rusty_feed_agents::Worker::new(
            state.db.clone(),
            state.fetch.clone(),
            embedder,
            llm,
            Some(llm_cascade),
            rusty_feed_agents::WorkerConfig {
                concurrency: config.feed_refresh_concurrency,
                interactive_concurrency: config.interactive_job_concurrency,
                scheduler_enabled: config.enable_cron_jobs,
                feed_refresh_schedule: rusty_feed_core::cron::CronSchedule::parse(
                    &config.feed_refresh_schedule,
                )
                .map_err(|error| anyhow::anyhow!("invalid FEED_REFRESH_SCHEDULE: {error}"))?,
                cleanup_schedule: rusty_feed_core::cron::CronSchedule::parse(
                    &config.cleanup_schedule,
                )
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
                ..rusty_feed_agents::WorkerConfig::default()
            },
        );
        tracing::info!("embedded AGENTS worker enabled");
        Some(tokio::spawn(async move { worker.run(shutdown_rx).await }))
    } else {
        None
    };

    let app = rusty_feed_web::router(state);
    tracing::info!("{} starting on http://{addr}", rusty_feed_core::banner());

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("failed to bind {addr}"))?;
    systemd.ready("Rusty Feed is ready to accept HTTP requests");

    let shutdown_notifier = systemd;
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        shutdown_signal().await;
        shutdown_notifier.stopping("Rusty Feed is draining HTTP and worker activity");
        let _ = shutdown_tx.send(true); // stop the embedded worker too
    })
    .await
    .context("server error")?;

    if let Some(handle) = worker_handle {
        let _ = handle.await;
    }
    if let Some(handle) = watchdog_handle {
        let _ = handle.await;
    }
    tracing::info!("shutdown complete");
    Ok(())
}

fn comma_values(value: &str) -> impl Iterator<Item = &str> {
    value
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
}

fn extraction_config(config: &AppConfig) -> rusty_feed_extract::ExtractConfig {
    let chromium = if config.chromium_enabled {
        config.chromium_cdp_url.as_ref().map(|endpoint| {
            rusty_feed_extract::chromium::ChromiumConfig {
                endpoint: endpoint.clone(),
                max_concurrency: config.feed_refresh_concurrency.min(4),
                render_wait: std::time::Duration::from_millis(750),
                timeout: std::time::Duration::from_millis(config.extraction_timeout_ms),
            }
        })
    } else {
        None
    };
    rusty_feed_extract::ExtractConfig {
        base_delay: std::time::Duration::from_millis(config.extraction_delay_ms),
        chromium,
        ..rusty_feed_extract::ExtractConfig::default()
    }
}

/// Assemble the enabled OAuth providers from configuration.
fn build_oauth_providers(config: &AppConfig) -> Vec<OAuthProvider> {
    use rusty_feed_core::Secret;
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

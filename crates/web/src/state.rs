//! Shared application state passed to Axum handlers.

use std::sync::Arc;
use std::time::Duration;

use rustss_cache::Cache;
use rustss_core::config::RegistrationMode;
use rustss_core::Secret;
use rustss_db::Db;
use rustss_feeds::SafeFetchClient;

use crate::ratelimit::RateLimiter;

/// How a provider's userinfo response is shaped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    /// OIDC-style: userinfo returns `sub` + `email` (Google, generic OIDC).
    Oidc,
    /// GitHub: numeric `id`, and email may require the `/user/emails` endpoint.
    GitHub,
}

/// A configured OAuth provider.
#[derive(Debug, Clone)]
pub struct OAuthProvider {
    pub slug: String,
    pub display_name: String,
    pub client_id: String,
    pub client_secret: Secret,
    pub auth_url: String,
    pub token_url: String,
    pub userinfo_url: String,
    pub scopes: String,
    pub kind: ProviderKind,
}

/// Instance-level settings the web layer needs at request time.
#[derive(Debug, Clone)]
pub struct AppSettings {
    /// How new-user registration is gated.
    pub registration_mode: RegistrationMode,
    /// Whether to mark cookies `Secure` (true when served over HTTPS).
    pub cookie_secure: bool,
    /// Public base URL (used for absolute links and OAuth redirect URIs).
    pub base_url: String,
    /// Configured OAuth providers (empty if none).
    pub oauth_providers: Vec<OAuthProvider>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            registration_mode: RegistrationMode::Open,
            cookie_secure: false,
            base_url: "http://localhost:8080".to_string(),
            oauth_providers: Vec::new(),
        }
    }
}

/// Cloneable handler state (cheap `Arc`/pool clones per request).
#[derive(Clone)]
pub struct AppState {
    pub db: Db,
    pub cache: Cache,
    pub settings: Arc<AppSettings>,
    /// SSRF-guarded HTTP client for fetching/refreshing feeds.
    pub fetch: SafeFetchClient,
    /// Per-IP limiter for authentication endpoints.
    pub auth_limiter: RateLimiter,
    metrics: Arc<dyn Fn() -> String + Send + Sync>,
}

impl AppState {
    /// Build application state (with the default SSRF-safe fetch client).
    pub fn new(
        db: Db,
        cache: Cache,
        settings: AppSettings,
        metrics_renderer: impl Fn() -> String + Send + Sync + 'static,
    ) -> Self {
        Self {
            db,
            cache,
            settings: Arc::new(settings),
            fetch: SafeFetchClient::default(),
            // 10 auth attempts per minute per client IP.
            auth_limiter: RateLimiter::new(10, Duration::from_secs(60)),
            metrics: Arc::new(metrics_renderer),
        }
    }

    /// Override the fetch client (e.g. tests using an allow-private client).
    pub fn with_fetch(mut self, fetch: SafeFetchClient) -> Self {
        self.fetch = fetch;
        self
    }

    /// Render the current metrics exposition text.
    pub fn render_metrics(&self) -> String {
        (self.metrics)()
    }
}

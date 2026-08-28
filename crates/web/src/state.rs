//! Shared application state passed to Axum handlers.

use std::sync::Arc;

use rustss_cache::Cache;
use rustss_core::config::RegistrationMode;
use rustss_db::Db;

/// Instance-level settings the web layer needs at request time.
#[derive(Debug, Clone)]
pub struct AppSettings {
    /// How new-user registration is gated.
    pub registration_mode: RegistrationMode,
    /// Whether to mark cookies `Secure` (true when served over HTTPS).
    pub cookie_secure: bool,
    /// Public base URL (used for absolute links).
    pub base_url: String,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            registration_mode: RegistrationMode::Open,
            cookie_secure: false,
            base_url: "http://localhost:8080".to_string(),
        }
    }
}

/// Cloneable handler state (cheap `Arc`/pool clones per request).
#[derive(Clone)]
pub struct AppState {
    pub db: Db,
    pub cache: Cache,
    pub settings: Arc<AppSettings>,
    metrics: Arc<dyn Fn() -> String + Send + Sync>,
}

impl AppState {
    /// Build application state.
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
            metrics: Arc::new(metrics_renderer),
        }
    }

    /// Render the current metrics exposition text.
    pub fn render_metrics(&self) -> String {
        (self.metrics)()
    }
}

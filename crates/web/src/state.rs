//! Shared application state passed to Axum handlers.
//!
//! Kept intentionally small and decoupled: instead of depending on the concrete
//! metrics exporter, the state holds a renderer closure the binary wires up.
//! Repositories, cache, and service handles are added here in later milestones.

use std::sync::Arc;

/// Cloneable handler state (cheap `Arc` clones per request).
#[derive(Clone)]
pub struct AppState {
    metrics: Arc<dyn Fn() -> String + Send + Sync>,
}

impl AppState {
    /// Build state with a metrics renderer (e.g. a Prometheus handle's
    /// `render`). Use [`AppState::default`] when metrics aren't installed.
    pub fn new(metrics_renderer: impl Fn() -> String + Send + Sync + 'static) -> Self {
        Self {
            metrics: Arc::new(metrics_renderer),
        }
    }

    /// Render the current metrics exposition text.
    pub fn render_metrics(&self) -> String {
        (self.metrics)()
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new(String::new)
    }
}

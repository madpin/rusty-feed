//! Process-wide observability setup (behind the `telemetry` feature).
//!
//! Binaries call [`init_tracing`] once at startup and, for the HTTP server,
//! [`install_metrics`] to obtain a Prometheus handle for the `/metrics` route.
//! Domain crates use the `tracing` facade directly and don't depend on the
//! subscriber/exporter.

use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

/// Initialize the global tracing subscriber.
///
/// Filtering precedence: `RUST_LOG`/`LOG_LEVEL` env (if set) → the provided
/// `log_level` → `info`. Uses `try_init` so repeated calls (e.g. in tests) are
/// harmless rather than panicking.
pub fn init_tracing(log_level: &str) {
    let filter = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new(log_level))
        .unwrap_or_else(|_| EnvFilter::new("info"));

    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer())
        .try_init();
}

/// Install the global Prometheus recorder and return a handle whose
/// [`PrometheusHandle::render`] produces the exposition text for `/metrics`.
///
/// # Panics
/// Panics if a global recorder is already installed (should be called once).
pub fn install_metrics() -> PrometheusHandle {
    PrometheusBuilder::new()
        .install_recorder()
        .expect("failed to install Prometheus recorder")
}

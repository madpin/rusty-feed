//! # rustss-core
//!
//! Foundational, mostly I/O-free building blocks shared across every RustSS
//! crate: configuration, the error taxonomy, secret handling, telemetry setup,
//! and shared policy/value types. Narrow substitution seams live beside the
//! feature types they operate on instead of in a catch-all port module.
//!
//! See `docs/ARCHITECTURE.md` §5 (module structure), §16 (config), §17
//! (observability), and §19 (errors).
//!
//! ## The one invariant
//! *Content is shared; state is personal.* Types and traits here must never
//! encode per-user duplication of shared content. See `docs/ARCHITECTURE.md` §7.

pub mod acquisition;
pub mod ai_budget;
pub mod blocklist;
pub mod charset;
pub mod config;
pub mod content_rules;
pub mod cron;
pub mod crypto;
pub mod error;
pub mod feed_content;
pub mod hash;
pub mod html;
pub mod i18n;
pub mod integrations;
pub mod quotas;
pub mod secret;
pub mod tracking_pixel;
pub mod tracking_url;
pub mod youtube;

#[cfg(feature = "telemetry")]
pub mod telemetry;

pub use config::AppConfig;
pub use error::ConfigError;
pub use secret::Secret;

/// Human-facing application name.
pub const APP_NAME: &str = "RustSS";

/// The crate/package version (from Cargo at build time).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// A short build/version banner, e.g. `"RustSS 0.1.0"`.
pub fn banner() -> String {
    format!("{APP_NAME} {VERSION}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn banner_contains_name_and_version() {
        let b = banner();
        assert!(b.contains(APP_NAME));
        assert!(b.contains(VERSION));
    }
}

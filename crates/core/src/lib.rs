//! # rustss-core
//!
//! Foundational, mostly I/O-free building blocks shared across every RustSS
//! crate: configuration, the error taxonomy, secret handling, telemetry setup,
//! and (from M2) the port traits that adapter crates implement.
//!
//! See `docs/ARCHITECTURE.md` §5 (module structure), §16 (config), §17
//! (observability), and §19 (errors).
//!
//! ## The one invariant
//! *Content is shared; state is personal.* Types and traits here must never
//! encode per-user duplication of shared content. See `docs/ARCHITECTURE.md` §7.

pub mod config;
pub mod error;
pub mod hash;
pub mod secret;

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

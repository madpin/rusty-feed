//! Core error taxonomy.
//!
//! Libraries return precise `thiserror` enums; binaries add `anyhow` context at
//! the edges; the web layer maps errors to HTTP via its own `AppError`
//! (`docs/ARCHITECTURE.md` §19). This module holds the errors that belong to
//! `rustss-core` itself (currently: configuration).

/// Errors raised while loading or validating [`crate::config::AppConfig`].
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// The configuration could not be read/parsed from the environment.
    #[error("failed to load configuration: {0}")]
    Load(String),

    /// The configuration parsed but failed a validation rule.
    #[error("invalid configuration: {0}")]
    Invalid(String),
}

//! # rustss-core
//!
//! Foundational, I/O-free building blocks shared across every RustSS crate:
//! domain types, configuration, the error taxonomy, and the port traits
//! (`Repositories`, `Cache`, `VectorStore`, `Embedder`, `LlmClient`,
//! `FetchClient`, `Queue`, `Clock`) that adapter crates implement.
//!
//! See `docs/ARCHITECTURE.md` §5 (module structure) and §19 (errors).
//!
//! ## The one invariant
//! *Content is shared; state is personal.* Types and traits here must never
//! encode per-user duplication of shared content. See `docs/ARCHITECTURE.md` §7.

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

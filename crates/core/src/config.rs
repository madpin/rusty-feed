//! Typed, validated application configuration.
//!
//! Loaded from the process environment (12-factor) with a `.env` fallback in
//! development, via `figment`. The process should call [`AppConfig::load`] once
//! at startup and fail fast on invalid/missing required values
//! (`docs/ARCHITECTURE.md` §16).

use std::path::PathBuf;

use figment::{providers::Env, Figment};
use serde::Deserialize;

use crate::error::ConfigError;
use crate::secret::Secret;

/// How new-user registration is gated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum RegistrationMode {
    /// Anyone may register.
    #[default]
    Open,
    /// Reserved for invitation-based registration; currently behaves as closed.
    Invite,
    /// Registration is disabled.
    Closed,
}

/// Which embedding backend the instance uses (one per instance).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum EmbeddingProvider {
    /// Deterministic local hashing embedder (default; on-device model pending).
    #[default]
    Local,
    /// OpenAI-compatible embeddings API.
    Openai,
}

/// Which LLM backend is used for summarization by default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum LlmProvider {
    /// OpenAI-compatible chat completions API.
    #[default]
    Openai,
    /// Ollama (native or its OpenAI-compatible endpoint).
    Ollama,
}

/// The complete, validated runtime configuration.
///
/// Field names map directly to upper-cased environment variables
/// (e.g. `database_url` <- `DATABASE_URL`). See `.env.example`.
#[derive(Debug, Clone, Deserialize)]
pub struct AppConfig {
    // ── Server ──
    #[serde(default = "defaults::bind_addr")]
    pub bind_addr: String,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default = "defaults::log_level")]
    pub log_level: String,

    // ── Database (required) ──
    pub database_url: String,

    // ── Cache / Redis ──
    #[serde(default)]
    pub redis_url: Option<String>,
    #[serde(default)]
    pub redis_password: Option<Secret>,
    #[serde(default = "defaults::bool_true")]
    pub cache_enabled: bool,

    // ── Auth & secrets (required) ──
    pub session_secret: Secret,
    pub encryption_secret: Secret,
    #[serde(default)]
    pub registration_mode: RegistrationMode,

    // ── OAuth providers (all optional) ──
    #[serde(default)]
    pub google_client_id: Option<String>,
    #[serde(default)]
    pub google_client_secret: Option<Secret>,
    #[serde(default)]
    pub github_client_id: Option<String>,
    #[serde(default)]
    pub github_client_secret: Option<Secret>,
    #[serde(default)]
    pub oauth_client_id: Option<String>,
    #[serde(default)]
    pub oauth_client_secret: Option<Secret>,
    #[serde(default)]
    pub oauth_auth_url: Option<String>,
    #[serde(default)]
    pub oauth_token_url: Option<String>,
    #[serde(default)]
    pub oauth_userinfo_url: Option<String>,
    #[serde(default)]
    pub oauth_provider_name: Option<String>,

    // ── Embeddings (one model per instance) ──
    #[serde(default)]
    pub embedding_provider: EmbeddingProvider,
    #[serde(default = "defaults::embedding_model")]
    pub embedding_model: String,
    #[serde(default = "defaults::embedding_batch_size")]
    pub embedding_batch_size: usize,
    #[serde(default)]
    pub embedding_auto_generate: bool,
    #[serde(default = "defaults::model_cache_dir")]
    pub model_cache_dir: PathBuf,
    #[serde(default)]
    pub openai_api_key: Option<Secret>,
    #[serde(default)]
    pub openai_base_url: Option<String>,

    // ── LLM ──
    #[serde(default)]
    pub llm_provider: LlmProvider,
    #[serde(default = "defaults::llm_model")]
    pub llm_summary_model: String,
    #[serde(default = "defaults::llm_model")]
    pub llm_digest_model: String,
    #[serde(default = "defaults::ollama_base_url")]
    pub ollama_base_url: String,

    // ── Content extraction ──
    #[serde(default = "defaults::bool_true")]
    pub chromium_enabled: bool,
    #[serde(default)]
    pub chromium_cdp_url: Option<String>,
    #[serde(default = "defaults::extraction_timeout_ms")]
    pub extraction_timeout_ms: u64,
    #[serde(default = "defaults::extraction_delay_ms")]
    pub extraction_delay_ms: u64,
    #[serde(default = "defaults::bool_true")]
    pub respect_robots_txt: bool,
    /// Allow fetching feeds/content from private/loopback addresses. **Off by
    /// default** (SSRF-safe); enable only for trusted intranet feeds or local
    /// testing.
    #[serde(default)]
    pub feeds_allow_private_network: bool,

    // ── Background jobs / AGENTS ──
    #[serde(default = "defaults::bool_true")]
    pub enable_cron_jobs: bool,
    #[serde(default = "defaults::bool_true")]
    pub run_embedded_worker: bool,
    #[serde(default = "defaults::feed_refresh_schedule")]
    pub feed_refresh_schedule: String,
    #[serde(default = "defaults::cleanup_schedule")]
    pub cleanup_schedule: String,
    #[serde(default = "defaults::feed_refresh_concurrency")]
    pub feed_refresh_concurrency: usize,
    #[serde(default = "defaults::saved_search_concurrency")]
    pub saved_search_concurrency: usize,
    #[serde(default = "defaults::job_lock_timeout_ms")]
    pub job_lock_timeout_ms: u64,
}

impl AppConfig {
    /// Load configuration from `.env` (if present) + the environment, then
    /// validate. Returns [`ConfigError`] on failure — callers should treat this
    /// as fatal at startup.
    pub fn load() -> Result<Self, ConfigError> {
        // Best-effort: load `.env` for local development. Missing file is fine.
        let _ = dotenvy::dotenv();
        Self::from_figment(Figment::new().merge(Env::raw()))
    }

    /// Build from an arbitrary figment (used by tests to inject values).
    pub fn from_figment(figment: Figment) -> Result<Self, ConfigError> {
        let cfg: AppConfig = figment
            .extract()
            .map_err(|e| ConfigError::Load(e.to_string()))?;
        cfg.validate()?;
        Ok(cfg)
    }

    /// Enforce cross-field/semantic rules not expressible in the type system.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.database_url.trim().is_empty() {
            return Err(ConfigError::Invalid(
                "DATABASE_URL must not be empty".into(),
            ));
        }
        if self.session_secret.len() < 32 {
            return Err(ConfigError::Invalid(
                "SESSION_SECRET must be at least 32 bytes".into(),
            ));
        }
        if self.encryption_secret.len() < 32 {
            return Err(ConfigError::Invalid(
                "ENCRYPTION_SECRET must be at least 32 bytes".into(),
            ));
        }
        if self.embedding_batch_size == 0 {
            return Err(ConfigError::Invalid(
                "EMBEDDING_BATCH_SIZE must be greater than 0".into(),
            ));
        }
        if self.embedding_provider == EmbeddingProvider::Openai && self.openai_api_key.is_none() {
            return Err(ConfigError::Invalid(
                "OPENAI_API_KEY is required when EMBEDDING_PROVIDER=openai".into(),
            ));
        }
        Ok(())
    }

    /// The externally-visible base URL, falling back to the bind address.
    pub fn base_url(&self) -> String {
        self.base_url
            .clone()
            .unwrap_or_else(|| format!("http://{}", self.bind_addr))
    }
}

mod defaults {
    use std::path::PathBuf;

    pub fn bind_addr() -> String {
        "0.0.0.0:8080".to_string()
    }
    pub fn log_level() -> String {
        "info".to_string()
    }
    pub fn bool_true() -> bool {
        true
    }
    pub fn embedding_model() -> String {
        "hash-placeholder-v1".to_string()
    }
    pub fn embedding_batch_size() -> usize {
        10
    }
    pub fn model_cache_dir() -> PathBuf {
        PathBuf::from("/var/lib/rustss/models")
    }
    pub fn llm_model() -> String {
        "gpt-4o-mini".to_string()
    }
    pub fn ollama_base_url() -> String {
        "http://localhost:11434".to_string()
    }
    pub fn extraction_timeout_ms() -> u64 {
        30_000
    }
    pub fn extraction_delay_ms() -> u64 {
        3_000
    }
    pub fn feed_refresh_schedule() -> String {
        "*/5 * * * *".to_string()
    }
    pub fn cleanup_schedule() -> String {
        "0 3 * * *".to_string()
    }
    pub fn feed_refresh_concurrency() -> usize {
        3
    }
    pub fn saved_search_concurrency() -> usize {
        5
    }
    pub fn job_lock_timeout_ms() -> u64 {
        600_000
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use figment::{providers::Serialized, Figment};

    /// A figment pre-seeded with the required fields, so tests can override.
    fn base() -> Figment {
        Figment::from(Serialized::defaults(serde_json::json!({
            "database_url": "postgres://u:p@localhost/db",
            "session_secret": "0123456789012345678901234567890123",
            "encryption_secret": "0123456789012345678901234567890123",
        })))
    }

    #[test]
    fn loads_with_defaults() {
        let cfg = AppConfig::from_figment(base()).expect("should load");
        assert_eq!(cfg.bind_addr, "0.0.0.0:8080");
        assert_eq!(cfg.embedding_model, "hash-placeholder-v1");
        assert_eq!(cfg.embedding_batch_size, 10);
        assert!(cfg.cache_enabled);
        assert_eq!(cfg.registration_mode, RegistrationMode::Open);
        assert_eq!(cfg.embedding_provider, EmbeddingProvider::Local);
        assert_eq!(cfg.base_url(), "http://0.0.0.0:8080");
    }

    #[test]
    fn supports_l1_only_cache_without_redis() {
        let cfg = AppConfig::from_figment(base().merge(Serialized::defaults(serde_json::json!({
            "cache_enabled": false,
        }))))
        .expect("Redis-free configuration should load");
        assert!(!cfg.cache_enabled);
        assert!(cfg.redis_url.is_none());
    }

    #[test]
    fn rejects_short_secret() {
        let f = base().merge(Serialized::defaults(serde_json::json!({
            "session_secret": "too-short",
        })));
        let err = AppConfig::from_figment(f).unwrap_err();
        assert!(matches!(err, ConfigError::Invalid(_)));
    }

    #[test]
    fn missing_required_is_error() {
        // No database_url/secrets provided.
        let err = AppConfig::from_figment(Figment::new()).unwrap_err();
        assert!(matches!(err, ConfigError::Load(_)));
    }

    #[test]
    fn openai_requires_api_key() {
        let f = base().merge(Serialized::defaults(serde_json::json!({
            "embedding_provider": "openai",
        })));
        let err = AppConfig::from_figment(f).unwrap_err();
        assert!(matches!(err, ConfigError::Invalid(_)));
    }
}

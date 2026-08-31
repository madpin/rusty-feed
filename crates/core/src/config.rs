//! Typed, validated application configuration.
//!
//! Loaded from an optional TOML file plus the process environment (with a
//! `.env` fallback in development) via `figment`. The process should call
//! [`AppConfig::load`] once at startup and fail fast on invalid/missing values
//! (`docs/ARCHITECTURE.md` §16).

use std::path::{Path, PathBuf};

use figment::{
    providers::{Env, Format, Toml},
    Figment,
};
use serde::Deserialize;

use crate::cron::CronSchedule;
use crate::error::ConfigError;
use crate::secret::Secret;
use crate::tracking_pixel::{TrackingPixelPolicy, TrackingPixelStrictness};
use crate::tracking_url::TrackingUrlPolicy;
use crate::youtube::YouTubePolicy;

const SAFE_CONFIGURABLE_MEDIA_MIMES: &[&str] = &[
    "audio/3gpp",
    "audio/3gpp2",
    "audio/x-m4a",
    "image/heic",
    "image/heif",
    "image/jxl",
    "video/3gpp",
    "video/3gpp2",
    "video/x-m4v",
];

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

impl RegistrationMode {
    /// Stable value used by diagnostics and forms.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Invite => "invite",
            Self::Closed => "closed",
        }
    }
}

/// Which embedding backend the instance uses (one per instance).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum EmbeddingProvider {
    /// Local fastembed/ONNX model (BGE-small by default).
    #[default]
    Local,
    /// OpenAI-compatible embeddings API.
    Openai,
}

impl EmbeddingProvider {
    /// Stable configuration value.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Openai => "openai",
        }
    }
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

impl LlmProvider {
    /// Stable configuration value.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Openai => "openai",
            Self::Ollama => "ollama",
        }
    }
}

/// Default query strategy for interactive article search.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SearchMode {
    /// Blend PostgreSQL keyword relevance with semantic similarity.
    #[default]
    Hybrid,
    /// Use PostgreSQL full-text search without requiring an embedding provider.
    Keyword,
    /// Use only semantic similarity from the active embedding space.
    Semantic,
}

impl SearchMode {
    /// Stable value used by forms and persisted user preferences.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Hybrid => "hybrid",
            Self::Keyword => "keyword",
            Self::Semantic => "semantic",
        }
    }

    /// Parse a persisted or request-level value without accepting aliases.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "hybrid" => Some(Self::Hybrid),
            "keyword" => Some(Self::Keyword),
            "semantic" => Some(Self::Semantic),
            _ => None,
        }
    }
}

/// PostgreSQL text-search dictionary used by the indexed search path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SearchTextConfig {
    /// Language-neutral token matching, suitable for multilingual libraries.
    #[default]
    Simple,
    /// English stop words and stemming.
    English,
}

impl SearchTextConfig {
    /// Stable value used by forms and persisted user preferences.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Simple => "simple",
            Self::English => "english",
        }
    }

    /// Parse a persisted value without allowing a SQL configuration name.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "simple" => Some(Self::Simple),
            "english" => Some(Self::English),
            _ => None,
        }
    }
}

/// Timestamp used by shared article-retention cleanup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum RetentionAgeBasis {
    /// Age from the publisher date, falling back to ingestion time.
    #[default]
    Published,
    /// Age only from the time RustSS first stored the article.
    Created,
}

impl RetentionAgeBasis {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Published => "published",
            Self::Created => "created",
        }
    }
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
    /// Emit READY/STOPPING/WATCHDOG messages when systemd provides a notify socket.
    #[serde(default = "defaults::bool_true")]
    pub systemd_notify_enabled: bool,
    #[serde(default = "defaults::pwa_name")]
    pub pwa_name: String,
    #[serde(default = "defaults::pwa_short_name")]
    pub pwa_short_name: String,
    #[serde(default = "defaults::pwa_theme_color")]
    pub pwa_theme_color: String,
    #[serde(default = "defaults::pwa_background_color")]
    pub pwa_background_color: String,
    /// Force publisher links to open in a separate tab for every account.
    /// Privacy attributes remain mandatory regardless of this navigation mode.
    #[serde(default)]
    pub external_links_new_tab_required: bool,
    #[serde(default = "defaults::bool_true")]
    pub public_article_sharing_enabled: bool,
    #[serde(default = "defaults::public_share_default_lifetime_days")]
    pub public_share_default_lifetime_days: u16,
    #[serde(default = "defaults::public_share_max_lifetime_days")]
    pub public_share_max_lifetime_days: u16,
    #[serde(default = "defaults::bool_true")]
    pub public_share_allow_infinite: bool,
    /// Native automation API policy. Health/metrics remain separately available.
    #[serde(default = "defaults::bool_true")]
    pub api_enabled: bool,
    #[serde(default = "defaults::bool_true")]
    pub greader_api_enabled: bool,
    #[serde(default = "defaults::bool_true")]
    pub fever_api_enabled: bool,
    /// Allow personal versioned CSS assets.
    #[serde(default = "defaults::bool_true")]
    pub custom_css_enabled: bool,
    /// Allow administrator-managed JavaScript assets. Disabled by default.
    #[serde(default)]
    pub custom_javascript_enabled: bool,
    #[serde(default = "defaults::ui_customization_max_bytes")]
    pub ui_customization_max_bytes: usize,
    /// Enable passwordless WebAuthn credentials. Requires a secure public origin
    /// except for localhost development.
    #[serde(default)]
    pub passkeys_enabled: bool,
    #[serde(default = "defaults::passkey_rp_name")]
    pub passkey_rp_name: String,

    // ── Database (required) ──
    pub database_url: String,

    // ── Cache / Redis ──
    #[serde(default)]
    pub redis_url: Option<String>,
    #[serde(default)]
    pub redis_password: Option<Secret>,
    #[serde(default = "defaults::bool_true")]
    pub cache_enabled: bool,
    #[serde(default = "defaults::cache_namespace")]
    pub cache_namespace: String,

    // ── Auth & secrets (required) ──
    pub session_secret: Secret,
    pub encryption_secret: Secret,
    #[serde(default)]
    pub registration_mode: RegistrationMode,
    #[serde(default)]
    pub reverse_proxy_auth_enabled: bool,
    #[serde(default = "defaults::reverse_proxy_auth_trusted_cidrs")]
    pub reverse_proxy_auth_trusted_cidrs: String,
    #[serde(default = "defaults::reverse_proxy_auth_user_header")]
    pub reverse_proxy_auth_user_header: String,
    #[serde(default = "defaults::reverse_proxy_auth_name_header")]
    pub reverse_proxy_auth_name_header: String,
    #[serde(default = "defaults::bool_true")]
    pub reverse_proxy_auth_auto_provision: bool,
    #[serde(default)]
    pub reverse_proxy_auth_allowed_domains: String,
    #[serde(default)]
    pub reverse_proxy_auth_admin_emails: String,
    #[serde(default)]
    pub reverse_proxy_auth_logout_url: Option<String>,

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

    // ── Search ──
    #[serde(default)]
    pub search_mode: SearchMode,
    #[serde(default)]
    pub search_text_config: SearchTextConfig,
    #[serde(default = "defaults::search_semantic_weight_percent")]
    pub search_semantic_weight_percent: u8,

    // ── LLM ──
    #[serde(default)]
    pub llm_provider: LlmProvider,
    #[serde(default = "defaults::llm_model")]
    pub llm_summary_model: String,
    #[serde(default = "defaults::llm_model")]
    pub llm_digest_model: String,
    /// Admin-owned instance policy. On-demand summaries always use the
    /// interactive lane independently of this setting.
    #[serde(default)]
    pub llm_auto_summarize: bool,
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
    /// Instance-wide identity sent to public feed, page, robots, icon, and
    /// media origins. Per-source overrides are intentionally resolved inside
    /// the shared acquisition layer rather than from personal settings.
    #[serde(default = "defaults::outbound_user_agent")]
    pub outbound_user_agent: String,
    /// Force HTTP/1.1 for publisher traffic when operators need to reduce
    /// protocol fingerprinting or work around broken HTTP/2 origins.
    #[serde(default)]
    pub outbound_http1_only: bool,
    /// Allow fetching feeds/content from private/loopback addresses. **Off by
    /// default** (SSRF-safe); enable only for trusted intranet feeds or local
    /// testing.
    #[serde(default)]
    pub feeds_allow_private_network: bool,
    #[serde(default = "defaults::bool_true")]
    pub tracking_url_cleanup_enabled: bool,
    #[serde(default)]
    pub tracking_url_extra_parameters: String,
    #[serde(default)]
    pub tracking_url_domain_exceptions: String,
    #[serde(default)]
    pub tracking_pixel_strictness: TrackingPixelStrictness,
    #[serde(default)]
    pub tracking_pixel_blocked_hosts: String,
    #[serde(default)]
    pub tracking_pixel_allowed_hosts: String,

    // ── YouTube privacy player ──
    #[serde(default = "defaults::bool_true")]
    pub youtube_embeds_enabled: bool,
    #[serde(default)]
    pub youtube_source_hosts: String,
    #[serde(default)]
    pub youtube_alternative_player_name: String,
    #[serde(default)]
    pub youtube_alternative_player_url: String,
    #[serde(default)]
    pub youtube_duration_enabled: bool,
    #[serde(default = "defaults::youtube_duration_daily_budget")]
    pub youtube_duration_daily_budget: u32,
    #[serde(default = "defaults::youtube_duration_cache_days")]
    pub youtube_duration_cache_days: u16,

    // ── Shared media proxy ──
    #[serde(default = "defaults::bool_true")]
    pub media_proxy_enabled: bool,
    #[serde(default = "defaults::media_proxy_max_bytes")]
    pub media_proxy_max_bytes: usize,
    #[serde(default = "defaults::media_proxy_cache_budget_bytes")]
    pub media_proxy_cache_budget_bytes: usize,
    #[serde(default = "defaults::media_proxy_cache_ttl_seconds")]
    pub media_proxy_cache_ttl_seconds: u64,
    #[serde(default = "defaults::media_proxy_negative_ttl_seconds")]
    pub media_proxy_negative_ttl_seconds: u64,
    /// Optional comma-separated passive media types from the bounded built-in
    /// extension allow-list. Active document types can never be enabled.
    #[serde(default)]
    pub media_proxy_extra_inline_mime_types: String,

    // ── Shared article retention ──
    /// Zero keeps article rows indefinitely. Public-page copies retain their
    /// separate per-user aggregated lifetime policy.
    #[serde(default)]
    pub article_retention_days: u32,
    #[serde(default)]
    pub retention_age_basis: RetentionAgeBasis,

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
    #[serde(default = "defaults::interactive_job_concurrency")]
    pub interactive_job_concurrency: usize,
    #[serde(default = "defaults::saved_search_concurrency")]
    pub saved_search_concurrency: usize,
    #[serde(default = "defaults::job_lock_timeout_ms")]
    pub job_lock_timeout_ms: u64,
}

impl AppConfig {
    /// Load configuration from an optional TOML file, `.env` (if present), and
    /// the process environment, then validate. Environment values have the
    /// highest precedence. Set `RUSTSS_CONFIG_FILE` to opt into TOML.
    pub fn load() -> Result<Self, ConfigError> {
        // Best-effort: load `.env` for local development. Missing file is fine.
        let _ = dotenvy::dotenv();
        let config_file = std::env::var_os("RUSTSS_CONFIG_FILE").map(PathBuf::from);
        Self::from_figment(figment_for_sources(config_file.as_deref())?)
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
        if !valid_cache_namespace(&self.cache_namespace) {
            return Err(ConfigError::Invalid(
                "CACHE_NAMESPACE must contain 1 to 64 ASCII letters, digits, '.', '-', or '_'"
                    .into(),
            ));
        }
        if !valid_header_name(&self.reverse_proxy_auth_user_header)
            || !valid_header_name(&self.reverse_proxy_auth_name_header)
        {
            return Err(ConfigError::Invalid(
                "REVERSE_PROXY_AUTH_USER_HEADER and REVERSE_PROXY_AUTH_NAME_HEADER must be valid ASCII HTTP header names"
                    .into(),
            ));
        }
        let trusted_cidrs: Vec<_> = comma_values(&self.reverse_proxy_auth_trusted_cidrs).collect();
        if self.reverse_proxy_auth_enabled && trusted_cidrs.is_empty() {
            return Err(ConfigError::Invalid(
                "REVERSE_PROXY_AUTH_TRUSTED_CIDRS must not be empty when reverse-proxy authentication is enabled"
                    .into(),
            ));
        }
        for cidr in trusted_cidrs {
            cidr.parse::<ipnet::IpNet>().map_err(|_| {
                ConfigError::Invalid(format!(
                    "REVERSE_PROXY_AUTH_TRUSTED_CIDRS contains invalid network '{cidr}'"
                ))
            })?;
        }
        for domain in comma_values(&self.reverse_proxy_auth_allowed_domains) {
            if !valid_domain(domain) {
                return Err(ConfigError::Invalid(format!(
                    "REVERSE_PROXY_AUTH_ALLOWED_DOMAINS contains invalid domain '{domain}'"
                )));
            }
        }
        for email in comma_values(&self.reverse_proxy_auth_admin_emails) {
            if !valid_email_shape(email) {
                return Err(ConfigError::Invalid(format!(
                    "REVERSE_PROXY_AUTH_ADMIN_EMAILS contains invalid email '{email}'"
                )));
            }
        }
        if let Some(logout_url) = self.reverse_proxy_auth_logout_url.as_deref() {
            if !valid_logout_url(logout_url) {
                return Err(ConfigError::Invalid(
                    "REVERSE_PROXY_AUTH_LOGOUT_URL must be an absolute application path or credential-free HTTPS URL"
                        .into(),
                ));
            }
        }
        if !valid_brand_name(&self.pwa_name, 64) {
            return Err(ConfigError::Invalid(
                "PWA_NAME must contain 1 to 64 printable characters".into(),
            ));
        }
        if !valid_brand_name(&self.pwa_short_name, 32) {
            return Err(ConfigError::Invalid(
                "PWA_SHORT_NAME must contain 1 to 32 printable characters".into(),
            ));
        }
        for (name, value) in [
            ("PWA_THEME_COLOR", &self.pwa_theme_color),
            ("PWA_BACKGROUND_COLOR", &self.pwa_background_color),
        ] {
            if !valid_hex_color(value) {
                return Err(ConfigError::Invalid(format!(
                    "{name} must use the #RRGGBB hexadecimal color format"
                )));
            }
        }
        if !(1..=3_650).contains(&self.public_share_max_lifetime_days) {
            return Err(ConfigError::Invalid(
                "PUBLIC_SHARE_MAX_LIFETIME_DAYS must be between 1 and 3650".into(),
            ));
        }
        if self.public_share_default_lifetime_days == 0
            || self.public_share_default_lifetime_days > self.public_share_max_lifetime_days
        {
            return Err(ConfigError::Invalid(
                "PUBLIC_SHARE_DEFAULT_LIFETIME_DAYS must be between 1 and PUBLIC_SHARE_MAX_LIFETIME_DAYS"
                    .into(),
            ));
        }
        if self.embedding_batch_size == 0 {
            return Err(ConfigError::Invalid(
                "EMBEDDING_BATCH_SIZE must be greater than 0".into(),
            ));
        }
        if !(1_024..=262_144).contains(&self.ui_customization_max_bytes) {
            return Err(ConfigError::Invalid(
                "UI_CUSTOMIZATION_MAX_BYTES must be between 1024 and 262144".into(),
            ));
        }
        if !valid_brand_name(&self.passkey_rp_name, 64) {
            return Err(ConfigError::Invalid(
                "PASSKEY_RP_NAME must contain 1 to 64 printable characters".into(),
            ));
        }
        if self.passkeys_enabled {
            let base = url::Url::parse(&self.base_url()).map_err(|_| {
                ConfigError::Invalid("BASE_URL must be valid when PASSKEYS_ENABLED=true".into())
            })?;
            let host = base.host().ok_or_else(|| {
                ConfigError::Invalid("PASSKEYS_ENABLED requires BASE_URL to contain a host".into())
            })?;
            if !base.username().is_empty()
                || base.password().is_some()
                || base.path() != "/"
                || base.query().is_some()
                || base.fragment().is_some()
            {
                return Err(ConfigError::Invalid(
                    "PASSKEYS_ENABLED requires BASE_URL to be an origin without credentials, path, query, or fragment"
                        .into(),
                ));
            }
            let is_local_development = match host {
                url::Host::Domain(name) => name.eq_ignore_ascii_case("localhost"),
                url::Host::Ipv4(address) => address.is_loopback(),
                url::Host::Ipv6(address) => address.is_loopback(),
            };
            if base.scheme() != "https" && !is_local_development {
                return Err(ConfigError::Invalid(
                    "PASSKEYS_ENABLED requires an HTTPS BASE_URL (localhost is allowed for development)"
                        .into(),
                ));
            }
        }
        if self.search_semantic_weight_percent > 100 {
            return Err(ConfigError::Invalid(
                "SEARCH_SEMANTIC_WEIGHT_PERCENT must be between 0 and 100".into(),
            ));
        }
        if !(1..=10_000).contains(&self.youtube_duration_daily_budget) {
            return Err(ConfigError::Invalid(
                "YOUTUBE_DURATION_DAILY_BUDGET must be between 1 and 10000".into(),
            ));
        }
        if !(1..=365).contains(&self.youtube_duration_cache_days) {
            return Err(ConfigError::Invalid(
                "YOUTUBE_DURATION_CACHE_DAYS must be between 1 and 365".into(),
            ));
        }
        if self.feed_refresh_concurrency == 0 {
            return Err(ConfigError::Invalid(
                "FEED_REFRESH_CONCURRENCY must be greater than 0".into(),
            ));
        }
        if self.interactive_job_concurrency == 0 {
            return Err(ConfigError::Invalid(
                "INTERACTIVE_JOB_CONCURRENCY must be greater than 0".into(),
            ));
        }
        CronSchedule::parse(&self.feed_refresh_schedule)
            .map_err(|error| ConfigError::Invalid(format!("FEED_REFRESH_SCHEDULE: {error}")))?;
        CronSchedule::parse(&self.cleanup_schedule)
            .map_err(|error| ConfigError::Invalid(format!("CLEANUP_SCHEDULE: {error}")))?;
        if self.embedding_provider == EmbeddingProvider::Openai && self.openai_api_key.is_none() {
            return Err(ConfigError::Invalid(
                "OPENAI_API_KEY is required when EMBEDDING_PROVIDER=openai".into(),
            ));
        }
        if !(1_048_576..=536_870_912).contains(&self.media_proxy_max_bytes) {
            return Err(ConfigError::Invalid(
                "MEDIA_PROXY_MAX_BYTES must be between 1048576 and 536870912".into(),
            ));
        }
        if self.media_proxy_cache_budget_bytes < self.media_proxy_max_bytes
            || self.media_proxy_cache_budget_bytes > 107_374_182_400
        {
            return Err(ConfigError::Invalid(
                "MEDIA_PROXY_CACHE_BUDGET_BYTES must be at least MEDIA_PROXY_MAX_BYTES and at most 107374182400".into(),
            ));
        }
        if !(60..=2_592_000).contains(&self.media_proxy_cache_ttl_seconds) {
            return Err(ConfigError::Invalid(
                "MEDIA_PROXY_CACHE_TTL_SECONDS must be between 60 and 2592000".into(),
            ));
        }
        if !(10..=3_600).contains(&self.media_proxy_negative_ttl_seconds) {
            return Err(ConfigError::Invalid(
                "MEDIA_PROXY_NEGATIVE_TTL_SECONDS must be between 10 and 3600".into(),
            ));
        }
        let extra_media_types: Vec<_> = comma_values(&self.media_proxy_extra_inline_mime_types)
            .map(str::to_ascii_lowercase)
            .collect();
        if extra_media_types.len() > 32
            || extra_media_types
                .iter()
                .any(|value| !SAFE_CONFIGURABLE_MEDIA_MIMES.contains(&value.as_str()))
        {
            return Err(ConfigError::Invalid(
                "MEDIA_PROXY_EXTRA_INLINE_MIME_TYPES accepts at most 32 values from the documented passive media allow-list"
                    .into(),
            ));
        }
        if self.article_retention_days > 36_500 {
            return Err(ConfigError::Invalid(
                "ARTICLE_RETENTION_DAYS must be 0 (infinite) or at most 36500".into(),
            ));
        }
        if !valid_user_agent(&self.outbound_user_agent) {
            return Err(ConfigError::Invalid(
                "OUTBOUND_USER_AGENT must contain 1 to 256 printable ASCII characters without surrounding whitespace"
                    .into(),
            ));
        }
        self.tracking_url_policy().map_err(ConfigError::Invalid)?;
        self.tracking_pixel_policy().map_err(ConfigError::Invalid)?;
        self.youtube_policy().map_err(ConfigError::Invalid)?;
        Ok(())
    }

    /// Build the validated instance-wide URL privacy policy.
    pub fn tracking_url_policy(&self) -> Result<TrackingUrlPolicy, String> {
        TrackingUrlPolicy::try_new(
            self.tracking_url_cleanup_enabled,
            &self.tracking_url_extra_parameters,
            &self.tracking_url_domain_exceptions,
        )
    }

    /// Build the validated instance-wide tracking-image policy.
    pub fn tracking_pixel_policy(&self) -> Result<TrackingPixelPolicy, String> {
        TrackingPixelPolicy::try_new(
            self.tracking_pixel_strictness,
            &self.tracking_pixel_blocked_hosts,
            &self.tracking_pixel_allowed_hosts,
        )
    }

    /// Build the validated instance-wide YouTube privacy-player boundary.
    pub fn youtube_policy(&self) -> Result<YouTubePolicy, String> {
        YouTubePolicy::try_new_with_alternative(
            self.youtube_embeds_enabled,
            &self.youtube_source_hosts,
            &self.youtube_alternative_player_name,
            &self.youtube_alternative_player_url,
        )
    }

    /// Validated additional passive MIME types accepted by the media proxy.
    pub fn media_proxy_extra_inline_mime_types(&self) -> Vec<String> {
        comma_values(&self.media_proxy_extra_inline_mime_types)
            .map(str::to_ascii_lowercase)
            .collect()
    }

    /// The externally-visible base URL, falling back to the bind address.
    pub fn base_url(&self) -> String {
        self.base_url
            .clone()
            .unwrap_or_else(|| format!("http://{}", self.bind_addr))
    }

    /// Return a support-friendly configuration summary that never includes
    /// URLs, credentials, tokens, or secret values.
    pub fn diagnostic_summary(&self) -> serde_json::Value {
        serde_json::json!({
            "server": {
                "bind_addr": self.bind_addr,
                "base_url_configured": self.base_url.is_some(),
                "log_level": self.log_level,
                "systemd_notify_enabled": self.systemd_notify_enabled,
                "pwa_name": self.pwa_name,
                "public_article_sharing_enabled": self.public_article_sharing_enabled,
                "external_links_new_tab_required": self.external_links_new_tab_required,
                "api_enabled": self.api_enabled,
                "greader_api_enabled": self.greader_api_enabled,
                "fever_api_enabled": self.fever_api_enabled,
                "custom_css_enabled": self.custom_css_enabled,
                "custom_javascript_enabled": self.custom_javascript_enabled,
                "ui_customization_max_bytes": self.ui_customization_max_bytes,
                "passkeys_enabled": self.passkeys_enabled,
                "passkey_rp_name": self.passkey_rp_name,
            },
            "database": { "configured": !self.database_url.is_empty() },
            "cache": {
                "enabled": self.cache_enabled,
                "redis_configured": self.redis_url.is_some(),
                "namespace": self.cache_namespace,
            },
            "authentication": {
                "registration_mode": self.registration_mode.as_str(),
                "google_enabled": self.google_client_id.is_some() && self.google_client_secret.is_some(),
                "github_enabled": self.github_client_id.is_some() && self.github_client_secret.is_some(),
                "generic_oauth_enabled": self.oauth_client_id.is_some()
                    && self.oauth_client_secret.is_some()
                    && self.oauth_auth_url.is_some()
                    && self.oauth_token_url.is_some()
                    && self.oauth_userinfo_url.is_some(),
                "reverse_proxy_enabled": self.reverse_proxy_auth_enabled,
                "reverse_proxy_trusted_networks": comma_values(&self.reverse_proxy_auth_trusted_cidrs).count(),
                "reverse_proxy_auto_provision": self.reverse_proxy_auth_auto_provision,
                "reverse_proxy_allowed_domains": comma_values(&self.reverse_proxy_auth_allowed_domains).count(),
                "reverse_proxy_admin_mappings": comma_values(&self.reverse_proxy_auth_admin_emails).count(),
                "reverse_proxy_logout_configured": self.reverse_proxy_auth_logout_url.is_some(),
            },
            "embeddings": {
                "provider": self.embedding_provider.as_str(),
                "model": self.embedding_model,
                "batch_size": self.embedding_batch_size,
                "auto_generate": self.embedding_auto_generate,
                "provider_credential_configured": self.openai_api_key.is_some(),
            },
            "search": {
                "mode": self.search_mode.as_str(),
                "text_config": self.search_text_config.as_str(),
                "semantic_weight_percent": self.search_semantic_weight_percent,
            },
            "summarization": {
                "provider": self.llm_provider.as_str(),
                "summary_model": self.llm_summary_model,
                "digest_model": self.llm_digest_model,
                "provider_credential_configured": self.openai_api_key.is_some(),
                "automatic": self.llm_auto_summarize,
            },
            "extraction": {
                "chromium_enabled": self.chromium_enabled,
                "chromium_endpoint_configured": self.chromium_cdp_url.is_some(),
                "respect_robots_txt": self.respect_robots_txt,
                "private_network_enabled": self.feeds_allow_private_network,
                "outbound_user_agent": self.outbound_user_agent,
                "outbound_http1_only": self.outbound_http1_only,
            },
            "youtube": {
                "embeds_enabled": self.youtube_embeds_enabled,
                "alternative_player_configured": !self.youtube_alternative_player_url.is_empty(),
                "duration_enabled": self.youtube_duration_enabled,
                "duration_daily_budget": self.youtube_duration_daily_budget,
                "duration_cache_days": self.youtube_duration_cache_days,
            },
            "media_proxy": {
                "enabled": self.media_proxy_enabled,
                "extra_passive_mime_types": comma_values(&self.media_proxy_extra_inline_mime_types).count(),
            },
            "retention": {
                "article_days": self.article_retention_days,
                "age_basis": self.retention_age_basis.as_str(),
            },
            "jobs": {
                "scheduler_enabled": self.enable_cron_jobs,
                "embedded_worker": self.run_embedded_worker,
                "background_concurrency": self.feed_refresh_concurrency,
                "interactive_concurrency": self.interactive_job_concurrency,
                "saved_search_concurrency": self.saved_search_concurrency,
            }
        })
    }
}

fn figment_for_sources(config_file: Option<&Path>) -> Result<Figment, ConfigError> {
    let figment = match config_file {
        Some(path) => {
            if !path.is_file() {
                return Err(ConfigError::Load(format!(
                    "RUSTSS_CONFIG_FILE does not name a readable file: {}",
                    path.display()
                )));
            }
            Figment::new().merge(Toml::file(path))
        }
        None => Figment::new(),
    };
    Ok(figment.merge(Env::raw()))
}

fn valid_brand_name(value: &str, max_chars: usize) -> bool {
    let trimmed = value.trim();
    value == trimmed
        && !trimmed.is_empty()
        && trimmed.chars().count() <= max_chars
        && !trimmed.chars().any(char::is_control)
}

fn valid_hex_color(value: &str) -> bool {
    value.len() == 7
        && value.starts_with('#')
        && value[1..].bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn valid_user_agent(value: &str) -> bool {
    value == value.trim()
        && (1..=256).contains(&value.len())
        && value.bytes().all(|byte| (0x20..=0x7e).contains(&byte))
}

fn comma_values(value: &str) -> impl Iterator<Item = &str> {
    value
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
}

fn valid_header_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'!' | b'#'
                        | b'$'
                        | b'%'
                        | b'&'
                        | b'\''
                        | b'*'
                        | b'+'
                        | b'-'
                        | b'.'
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'|'
                        | b'~'
                )
        })
}

fn valid_domain(value: &str) -> bool {
    let value = value.trim_matches('.');
    !value.is_empty()
        && value.len() <= 253
        && value.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
}

fn valid_email_shape(value: &str) -> bool {
    value.len() <= 320
        && value == value.trim()
        && !value.chars().any(char::is_control)
        && value
            .rsplit_once('@')
            .is_some_and(|(local, domain)| !local.is_empty() && valid_domain(domain))
}

fn valid_logout_url(value: &str) -> bool {
    if value.starts_with('/') && !value.starts_with("//") && !value.contains(['\r', '\n']) {
        return true;
    }
    url::Url::parse(value).is_ok_and(|url| {
        url.scheme() == "https"
            && url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none()
    })
}

fn valid_cache_namespace(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
}

mod defaults {
    use std::path::PathBuf;

    pub fn bind_addr() -> String {
        "0.0.0.0:8080".to_string()
    }
    pub fn log_level() -> String {
        "info".to_string()
    }
    pub fn pwa_name() -> String {
        "RustSS".to_string()
    }
    pub fn pwa_short_name() -> String {
        "RustSS".to_string()
    }
    pub fn pwa_theme_color() -> String {
        "#2563eb".to_string()
    }
    pub fn pwa_background_color() -> String {
        "#f8fafc".to_string()
    }
    pub fn public_share_default_lifetime_days() -> u16 {
        30
    }
    pub fn public_share_max_lifetime_days() -> u16 {
        3_650
    }
    pub fn ui_customization_max_bytes() -> usize {
        65_536
    }
    pub fn passkey_rp_name() -> String {
        "RustSS".to_string()
    }
    pub fn youtube_duration_daily_budget() -> u32 {
        100
    }
    pub fn youtube_duration_cache_days() -> u16 {
        30
    }
    pub fn cache_namespace() -> String {
        "rustss".to_string()
    }
    pub fn reverse_proxy_auth_trusted_cidrs() -> String {
        "127.0.0.1/32,::1/128".to_string()
    }
    pub fn reverse_proxy_auth_user_header() -> String {
        "x-remote-user".to_string()
    }
    pub fn reverse_proxy_auth_name_header() -> String {
        "x-remote-name".to_string()
    }
    pub fn bool_true() -> bool {
        true
    }
    pub fn embedding_model() -> String {
        "BAAI/bge-small-en-v1.5".to_string()
    }
    pub fn embedding_batch_size() -> usize {
        10
    }
    pub fn search_semantic_weight_percent() -> u8 {
        60
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
    pub fn outbound_user_agent() -> String {
        "RustSS/0.1 (+https://github.com/madpin/rustss)".to_string()
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
    pub fn interactive_job_concurrency() -> usize {
        1
    }
    pub fn saved_search_concurrency() -> usize {
        5
    }
    pub fn job_lock_timeout_ms() -> u64 {
        600_000
    }
    pub fn media_proxy_max_bytes() -> usize {
        64 * 1_024 * 1_024
    }
    pub fn media_proxy_cache_ttl_seconds() -> u64 {
        7 * 24 * 60 * 60
    }
    pub fn media_proxy_cache_budget_bytes() -> usize {
        2 * 1_024 * 1_024 * 1_024
    }
    pub fn media_proxy_negative_ttl_seconds() -> u64 {
        5 * 60
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
        assert_eq!(cfg.pwa_name, "RustSS");
        assert!(cfg.systemd_notify_enabled);
        assert_eq!(cfg.pwa_theme_color, "#2563eb");
        assert!(cfg.public_article_sharing_enabled);
        assert_eq!(cfg.public_share_default_lifetime_days, 30);
        assert_eq!(cfg.public_share_max_lifetime_days, 3_650);
        assert!(cfg.public_share_allow_infinite);
        assert_eq!(cfg.embedding_model, "BAAI/bge-small-en-v1.5");
        assert_eq!(cfg.embedding_batch_size, 10);
        assert_eq!(cfg.search_mode, SearchMode::Hybrid);
        assert_eq!(cfg.search_text_config, SearchTextConfig::Simple);
        assert_eq!(cfg.search_semantic_weight_percent, 60);
        assert_eq!(cfg.interactive_job_concurrency, 1);
        assert!(cfg.cache_enabled);
        assert_eq!(cfg.cache_namespace, "rustss");
        assert_eq!(cfg.registration_mode, RegistrationMode::Open);
        assert_eq!(cfg.embedding_provider, EmbeddingProvider::Local);
        assert_eq!(cfg.base_url(), "http://0.0.0.0:8080");
        assert!(cfg.media_proxy_enabled);
        assert!(cfg.tracking_url_cleanup_enabled);
        assert_eq!(
            cfg.tracking_pixel_strictness,
            TrackingPixelStrictness::Balanced
        );
        assert!(cfg.youtube_embeds_enabled);
        assert_eq!(
            cfg.outbound_user_agent,
            "RustSS/0.1 (+https://github.com/madpin/rustss)"
        );
        assert!(!cfg.outbound_http1_only);
        assert_eq!(cfg.youtube_policy().unwrap().source_host_count(), 3);
        assert_eq!(cfg.media_proxy_max_bytes, 64 * 1_024 * 1_024);
        assert_eq!(
            cfg.media_proxy_cache_budget_bytes,
            2 * 1_024 * 1_024 * 1_024
        );
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
    fn validates_reverse_proxy_auth_boundary() {
        let valid = base().merge(Serialized::defaults(serde_json::json!({
            "reverse_proxy_auth_enabled": true,
            "reverse_proxy_auth_trusted_cidrs": "127.0.0.1/32,10.20.0.0/16",
            "reverse_proxy_auth_user_header": "x-auth-email",
            "reverse_proxy_auth_name_header": "x-auth-name",
            "reverse_proxy_auth_allowed_domains": "example.test,company.example",
            "reverse_proxy_auth_admin_emails": "admin@example.test",
            "reverse_proxy_auth_logout_url": "https://auth.example.test/logout"
        })));
        assert!(AppConfig::from_figment(valid).is_ok());

        for (field, value) in [
            ("reverse_proxy_auth_trusted_cidrs", "not-a-network"),
            ("reverse_proxy_auth_user_header", "bad header"),
            ("reverse_proxy_auth_allowed_domains", "https://example.test"),
            ("reverse_proxy_auth_admin_emails", "not-an-email"),
            (
                "reverse_proxy_auth_logout_url",
                "http://auth.example.test/logout",
            ),
        ] {
            let invalid = base().merge(Serialized::defaults(serde_json::json!({
                "reverse_proxy_auth_enabled": true,
                (field): value,
            })));
            assert!(
                AppConfig::from_figment(invalid).is_err(),
                "accepted {field}"
            );
        }
    }

    #[test]
    fn validates_cache_namespace() {
        for value in ["", "two installations", "bad:separator", "💾"] {
            let invalid = base().merge(Serialized::defaults(serde_json::json!({
                "cache_namespace": value,
            })));
            assert!(AppConfig::from_figment(invalid).is_err());
        }

        let valid = base().merge(Serialized::defaults(serde_json::json!({
            "cache_namespace": "home-reader_1.prod",
        })));
        assert!(AppConfig::from_figment(valid).is_ok());
    }

    #[test]
    fn validates_media_proxy_storage_bounds() {
        let too_small = base().merge(Serialized::defaults(serde_json::json!({
            "media_proxy_max_bytes": 1024,
        })));
        assert!(AppConfig::from_figment(too_small).is_err());

        let budget_below_object = base().merge(Serialized::defaults(serde_json::json!({
            "media_proxy_max_bytes": 2_097_152,
            "media_proxy_cache_budget_bytes": 1_048_576,
        })));
        assert!(AppConfig::from_figment(budget_below_object).is_err());

        let passive = base().merge(Serialized::defaults(serde_json::json!({
            "media_proxy_extra_inline_mime_types": "image/jxl,audio/x-m4a",
        })));
        assert_eq!(
            AppConfig::from_figment(passive)
                .unwrap()
                .media_proxy_extra_inline_mime_types(),
            vec!["image/jxl", "audio/x-m4a"]
        );
        for active in ["image/svg+xml", "text/html", "application/javascript"] {
            let invalid = base().merge(Serialized::defaults(serde_json::json!({
                "media_proxy_extra_inline_mime_types": active,
            })));
            assert!(AppConfig::from_figment(invalid).is_err());
        }
    }

    #[test]
    fn validates_youtube_source_hosts() {
        let invalid = base().merge(Serialized::defaults(serde_json::json!({
            "youtube_source_hosts": "https://youtube.com",
        })));
        assert!(AppConfig::from_figment(invalid).is_err());

        let alternative = base().merge(Serialized::defaults(serde_json::json!({
            "youtube_alternative_player_name": "Home Invidious",
            "youtube_alternative_player_url": "https://video.example.org",
        })));
        let cfg = AppConfig::from_figment(alternative).unwrap();
        assert_eq!(
            cfg.youtube_policy().unwrap().alternative_name(),
            Some("Home Invidious")
        );
    }

    #[test]
    fn validates_outbound_user_agent() {
        for value in ["", " leading", "trailing ", "bad\r\nheader", "💥"] {
            let invalid = base().merge(Serialized::defaults(serde_json::json!({
                "outbound_user_agent": value,
            })));
            assert!(
                AppConfig::from_figment(invalid).is_err(),
                "accepted {value:?}"
            );
        }

        let valid = base().merge(Serialized::defaults(serde_json::json!({
            "outbound_user_agent": "MyReader/4.2 (+https://reader.example)",
            "outbound_http1_only": true,
        })));
        let cfg = AppConfig::from_figment(valid).unwrap();
        assert!(cfg.outbound_http1_only);
        assert_eq!(
            cfg.outbound_user_agent,
            "MyReader/4.2 (+https://reader.example)"
        );
    }

    #[test]
    fn validates_installable_app_branding() {
        for values in [
            serde_json::json!({ "pwa_name": "" }),
            serde_json::json!({ "pwa_name": " RustSS " }),
            serde_json::json!({ "pwa_short_name": "x".repeat(33) }),
            serde_json::json!({ "pwa_theme_color": "blue" }),
            serde_json::json!({ "pwa_background_color": "#12345g" }),
        ] {
            assert!(AppConfig::from_figment(base().merge(Serialized::defaults(values))).is_err());
        }
    }

    #[test]
    fn validates_public_share_lifetime_policy() {
        for values in [
            serde_json::json!({ "public_share_max_lifetime_days": 0 }),
            serde_json::json!({ "public_share_max_lifetime_days": 3651 }),
            serde_json::json!({ "public_share_default_lifetime_days": 0 }),
            serde_json::json!({
                "public_share_default_lifetime_days": 31,
                "public_share_max_lifetime_days": 30
            }),
        ] {
            assert!(AppConfig::from_figment(base().merge(Serialized::defaults(values))).is_err());
        }
    }

    #[test]
    fn validates_passkey_relying_party_origin() {
        for base_url in [
            "https://reader.example",
            "https://reader.example:8443",
            "http://localhost:8080",
            "http://127.0.0.1:8080",
            "http://[::1]:8080",
        ] {
            AppConfig::from_figment(base().merge(Serialized::defaults(serde_json::json!({
                "passkeys_enabled": true,
                "base_url": base_url,
            }))))
            .unwrap_or_else(|error| panic!("{base_url} should be a valid passkey origin: {error}"));
        }

        for base_url in [
            "http://reader.example",
            "https://reader.example/app",
            "https://reader.example/?tenant=one",
            "https://reader.example/#login",
            "https://user:secret@reader.example",
            "file:///tmp/rustss",
        ] {
            assert!(
                AppConfig::from_figment(base().merge(Serialized::defaults(serde_json::json!({
                    "passkeys_enabled": true,
                    "base_url": base_url,
                }))))
                .is_err(),
                "{base_url} must not be accepted as a passkey origin"
            );
        }
    }

    #[test]
    fn rejects_zero_worker_capacity() {
        for key in ["feed_refresh_concurrency", "interactive_job_concurrency"] {
            let mut values = serde_json::Map::new();
            values.insert(key.to_string(), serde_json::json!(0));
            let invalid = base().merge(Serialized::defaults(serde_json::Value::Object(values)));
            assert!(AppConfig::from_figment(invalid).is_err());
        }
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

    #[test]
    fn validates_search_weight() {
        let invalid = base().merge(Serialized::defaults(serde_json::json!({
            "search_semantic_weight_percent": 101,
        })));
        let err = AppConfig::from_figment(invalid).unwrap_err();
        assert!(matches!(err, ConfigError::Invalid(_)));
    }

    #[test]
    fn validates_tracking_pixel_host_lists() {
        let invalid = base().merge(Serialized::defaults(serde_json::json!({
            "tracking_pixel_blocked_hosts": "https://metrics.example.com",
        })));
        assert!(AppConfig::from_figment(invalid).is_err());
    }

    #[test]
    fn explicit_missing_config_file_fails_clearly() {
        let path =
            std::env::temp_dir().join(format!("rustss-missing-config-{}.toml", std::process::id()));
        let error = figment_for_sources(Some(&path)).unwrap_err();
        assert!(error.to_string().contains("RUSTSS_CONFIG_FILE"));
        assert!(error.to_string().contains(&path.display().to_string()));
    }

    #[test]
    fn loads_optional_toml_configuration() {
        let path = std::env::temp_dir().join(format!("rustss-config-{}.toml", std::process::id()));
        std::fs::write(
            &path,
            r#"
database_url = "postgres://from-file/rustss"
session_secret = "file-session-secret-that-is-at-least-32-bytes"
encryption_secret = "file-encryption-secret-that-is-at-least-32-bytes"
pwa_name = "RustSS from TOML"
cache_enabled = false
"#,
        )
        .expect("temporary configuration should be writable");

        let result = figment_for_sources(Some(&path)).and_then(AppConfig::from_figment);
        let _ = std::fs::remove_file(&path);
        let cfg = result.expect("TOML configuration should load");
        assert_eq!(cfg.pwa_name, "RustSS from TOML");
        assert!(!cfg.cache_enabled);
    }

    #[test]
    fn diagnostic_summary_never_contains_connection_or_secret_values() {
        let database_url = "postgres://diagnostic-user:diagnostic-password@db/rustss";
        let redis_url = "redis://:diagnostic-redis-secret@cache:6379";
        let openai_key = "sk-diagnostic-openai-secret";
        let cfg = AppConfig::from_figment(base().merge(Serialized::defaults(serde_json::json!({
            "database_url": database_url,
            "redis_url": redis_url,
            "openai_api_key": openai_key,
        }))))
        .expect("configuration should validate");

        let summary = cfg.diagnostic_summary().to_string();
        for forbidden in [
            database_url,
            redis_url,
            openai_key,
            "a-session-secret-that-is-at-least-32-bytes-long",
            "an-encryption-secret-that-is-at-least-32-bytes",
        ] {
            assert!(!summary.contains(forbidden));
        }
        assert!(summary.contains("provider_credential_configured"));
        assert!(summary.contains("redis_configured"));
    }
}

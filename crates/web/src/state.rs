//! Shared application state passed to Axum handlers.

use std::sync::Arc;
use std::time::Duration;

use ipnet::IpNet;
use moka::future::Cache as MokaCache;
use rand::rngs::OsRng;
use rand::RngCore;
use rusty_feed_cache::Cache;
use rusty_feed_core::ai_budget::AiBudget;
use rusty_feed_core::config::{RegistrationMode, SearchMode, SearchTextConfig};
use rusty_feed_core::crypto::Cryptor;
use rusty_feed_core::quotas::Quotas;
use rusty_feed_core::tracking_pixel::TrackingPixelPolicy;
use rusty_feed_core::tracking_url::TrackingUrlPolicy;
use rusty_feed_core::youtube::YouTubePolicy;
use rusty_feed_core::Secret;
use rusty_feed_db::Db;
use rusty_feed_embed::{Embedder, HashEmbedder};
use rusty_feed_feeds::SafeFetchClient;
use tokio::sync::Mutex;
use webauthn_rs::prelude::{Url as WebauthnUrl, Webauthn, WebauthnBuilder};

use crate::ratelimit::RateLimiter;

/// Validated trusted reverse-proxy identity boundary.
#[derive(Debug, Clone)]
pub struct ReverseProxyAuthSettings {
    pub enabled: bool,
    pub trusted_cidrs: Vec<IpNet>,
    pub user_header: String,
    pub name_header: String,
    pub auto_provision: bool,
    pub allowed_domains: Vec<String>,
    pub admin_emails: Vec<String>,
    pub logout_url: Option<String>,
}

impl Default for ReverseProxyAuthSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            trusted_cidrs: vec!["127.0.0.1/32".parse().expect("static CIDR")],
            user_header: "x-remote-user".to_string(),
            name_header: "x-remote-name".to_string(),
            auto_provision: true,
            allowed_domains: Vec::new(),
            admin_emails: Vec::new(),
            logout_url: None,
        }
    }
}

/// Per-key request limits (requests/min). `0` means unlimited.
#[derive(Debug, Clone, Copy)]
pub struct RateLimitSettings {
    pub auth_per_min: u32,
    pub api_per_min: u32,
    pub feed_add_per_min: u32,
    pub search_per_min: u32,
    pub summarize_per_min: u32,
    pub opml_per_min: u32,
    pub media_per_min: u32,
}

impl Default for RateLimitSettings {
    fn default() -> Self {
        Self {
            auth_per_min: 10,
            api_per_min: 600,
            feed_add_per_min: 60,
            search_per_min: 120,
            summarize_per_min: 20,
            opml_per_min: 10,
            media_per_min: 1_800,
        }
    }
}

/// Server-side CAPTCHA configuration (registration anti-abuse, FND-191).
#[derive(Debug, Clone)]
pub struct CaptchaSettings {
    /// `turnstile` (Cloudflare) or `hcaptcha`.
    pub provider: String,
    /// Public site key rendered into the page.
    pub site_key: String,
    /// Secret used only for server-side verification (never rendered).
    pub secret: String,
}

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

/// Summary of the environment-configured summarization provider — the final
/// fallback in the user → admin → environment cascade. Surfaced read-only on
/// the AI providers screen so operators can see the built-in fallback.
#[derive(Debug, Clone)]
pub struct EnvLlmProvider {
    pub provider: String,
    pub model: String,
    pub base_url: String,
    pub has_key: bool,
}

/// Instance-level settings the web layer needs at request time.
#[derive(Debug, Clone)]
pub struct AppSettings {
    /// How new-user registration is gated.
    pub registration_mode: RegistrationMode,
    /// Trusted upstream identity policy; disabled by default.
    pub reverse_proxy_auth: ReverseProxyAuthSettings,
    /// Whether to mark cookies `Secure` (true when served over HTTPS).
    pub cookie_secure: bool,
    /// Public base URL (used for absolute links and OAuth redirect URIs).
    pub base_url: String,
    /// Installed-app display name exposed through the web manifest.
    pub pwa_name: String,
    /// Compact installed-app label exposed through the web manifest.
    pub pwa_short_name: String,
    /// Validated installed-app theme color.
    pub pwa_theme_color: String,
    /// Validated installed-app launch background color.
    pub pwa_background_color: String,
    /// Instance policy that overrides personal same-tab navigation.
    pub external_links_new_tab_required: bool,
    /// Whether users may publish revocable article links.
    pub public_article_sharing_enabled: bool,
    /// Default finite lifetime presented by the share form.
    pub public_share_default_lifetime_days: u16,
    /// Longest finite lifetime accepted from a user.
    pub public_share_max_lifetime_days: u16,
    /// Whether a user may explicitly create a non-expiring share.
    pub public_share_allow_infinite: bool,
    pub api_enabled: bool,
    /// Compress eligible responses in-process (`HTTP_COMPRESSION`).
    pub http_compression: bool,
    pub greader_api_enabled: bool,
    pub fever_api_enabled: bool,
    pub custom_css_enabled: bool,
    pub custom_javascript_enabled: bool,
    pub ui_customization_max_bytes: usize,
    pub passkeys_enabled: bool,
    pub passkey_rp_name: String,
    /// Configured OAuth providers (empty if none).
    pub oauth_providers: Vec<OAuthProvider>,
    /// Whether an LLM is configured (enables the "Summarize" action).
    pub summarization_enabled: bool,
    /// First-start enablement for the persisted automatic-summary policy.
    pub auto_summarize_default: bool,
    /// First-start readable-text minimum for the persisted policy.
    pub auto_summarize_min_chars_default: u32,
    /// Environment-configured summarization provider (the cascade fallback), if any.
    pub env_llm: Option<EnvLlmProvider>,
    /// Whether newly-ingested articles should be enqueued for embedding.
    pub auto_embed: bool,
    /// Master switch for every embedding use case (`EMBEDDINGS_ENABLED`).
    /// `false` forces `search_mode` to keyword-only regardless of instance or
    /// personal preference, and hides the semantic/hybrid options in the UI.
    pub embeddings_enabled: bool,
    /// Instance default for keyword, semantic, or blended interactive search.
    pub search_mode: SearchMode,
    /// Instance default PostgreSQL text dictionary.
    pub search_text_config: SearchTextConfig,
    /// Instance default contribution of semantic similarity to hybrid ranking.
    pub search_semantic_weight_percent: u8,
    /// Whether same-origin enclosure delivery is available.
    pub media_proxy_enabled: bool,
    /// Maximum media body cached and served by the proxy.
    pub media_proxy_max_bytes: usize,
    /// Aggregate shared media-cache body budget.
    pub media_proxy_cache_budget_bytes: usize,
    /// Lifetime of a successful shared media cache entry.
    pub media_proxy_cache_ttl: Duration,
    /// Lifetime of a shared media fetch failure.
    pub media_proxy_negative_ttl: Duration,
    /// Additional passive MIME types selected from the core safety allow-list.
    pub media_proxy_extra_inline_mime_types: Vec<String>,
    /// Immutable URL cleanup policy shared with the worker.
    pub tracking_url_policy: TrackingUrlPolicy,
    /// Immutable tracking-image policy shared with background processing.
    pub tracking_pixel_policy: TrackingPixelPolicy,
    /// Validated source-host boundary and enablement for privacy YouTube players.
    pub youtube_policy: YouTubePolicy,
    pub youtube_duration_enabled: bool,
    pub youtube_duration_cache_days: u16,
    /// Effective publisher-facing HTTP user agent.
    pub outbound_user_agent: String,
    /// Whether publisher traffic is restricted to HTTP/1.1.
    pub outbound_http1_only: bool,
    /// Reverse-proxy CIDRs allowed to set the client IP via forwarded headers
    /// (otherwise the socket peer is used). Guards IP rate limits (FND-190).
    pub trusted_proxy_cidrs: Vec<IpNet>,
    /// Immutable AI cost/compute budget (FND-187).
    pub ai_budget: AiBudget,
    /// Per-key request rate limits (FND-190).
    pub rate_limits: RateLimitSettings,
    /// Immutable per-user resource quotas (FND-188).
    pub quotas: Quotas,
    /// Require local accounts to verify their email before privileged actions.
    pub email_verification_required: bool,
    /// Minimum accepted length for new/changed passwords.
    pub min_password_length: usize,
    /// Failed local logins before a temporary account lock (`0` disables).
    pub login_max_failures: u32,
    /// How long an account stays locked after too many failures (seconds).
    pub login_lockout_seconds: u64,
    /// Registration CAPTCHA, if configured.
    pub captcha: Option<CaptchaSettings>,
    /// Contact address for the Abuse/DMCA and Privacy pages (FND-195).
    pub legal_contact_email: Option<String>,
    /// Operator/entity name shown in the legal pages.
    pub legal_entity_name: String,
    /// Require accepting Terms + Privacy at registration.
    pub require_tos_acceptance: bool,
    /// Demo sandbox enabled: "Try the demo" provisions ephemeral accounts (§7).
    pub demo_mode: bool,
    /// Tighter subscription cap for demo accounts.
    pub demo_max_subscriptions: u32,
    /// Expose the read-only admin demo ("Try the admin demo").
    pub demo_admin_readonly: bool,
    /// Demo session cookie lifetime, in hours (§7.5).
    pub demo_session_ttl_hours: u32,
    /// How long a user's Library unread counts stay cached; zero disables the
    /// cache (`UNREAD_COUNT_CACHE_SECONDS`, FND-237).
    pub unread_count_cache_ttl: Duration,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            registration_mode: RegistrationMode::Open,
            reverse_proxy_auth: ReverseProxyAuthSettings::default(),
            cookie_secure: false,
            base_url: "http://localhost:8080".to_string(),
            pwa_name: "Rusty Feed".to_string(),
            pwa_short_name: "Rusty Feed".to_string(),
            pwa_theme_color: "#2563eb".to_string(),
            pwa_background_color: "#f8fafc".to_string(),
            external_links_new_tab_required: false,
            public_article_sharing_enabled: true,
            public_share_default_lifetime_days: 30,
            public_share_max_lifetime_days: 3_650,
            public_share_allow_infinite: true,
            api_enabled: true,
            http_compression: true,
            greader_api_enabled: true,
            fever_api_enabled: true,
            custom_css_enabled: true,
            custom_javascript_enabled: false,
            ui_customization_max_bytes: 65_536,
            passkeys_enabled: false,
            passkey_rp_name: "Rusty Feed".to_string(),
            oauth_providers: Vec::new(),
            summarization_enabled: false,
            auto_summarize_default: false,
            auto_summarize_min_chars_default:
                rusty_feed_db::summary_policy::DEFAULT_MIN_CONTENT_CHARS,
            env_llm: None,
            auto_embed: false,
            embeddings_enabled: true,
            search_mode: SearchMode::Hybrid,
            search_text_config: SearchTextConfig::Simple,
            search_semantic_weight_percent: 60,
            media_proxy_enabled: true,
            media_proxy_max_bytes: 64 * 1_024 * 1_024,
            media_proxy_cache_budget_bytes: 2 * 1_024 * 1_024 * 1_024,
            media_proxy_cache_ttl: Duration::from_secs(7 * 24 * 60 * 60),
            media_proxy_negative_ttl: Duration::from_secs(5 * 60),
            media_proxy_extra_inline_mime_types: Vec::new(),
            tracking_url_policy: TrackingUrlPolicy::default(),
            tracking_pixel_policy: TrackingPixelPolicy::default(),
            youtube_policy: YouTubePolicy::default(),
            youtube_duration_enabled: false,
            youtube_duration_cache_days: 30,
            outbound_user_agent: rusty_feed_feeds::DEFAULT_USER_AGENT.to_string(),
            outbound_http1_only: false,
            trusted_proxy_cidrs: vec!["127.0.0.1/32".parse().expect("static CIDR")],
            ai_budget: AiBudget::default(),
            rate_limits: RateLimitSettings::default(),
            quotas: Quotas::default(),
            email_verification_required: false,
            min_password_length: 10,
            login_max_failures: 10,
            login_lockout_seconds: 900,
            captcha: None,
            legal_contact_email: None,
            legal_entity_name: "Rusty Feed".to_string(),
            require_tos_acceptance: false,
            demo_mode: false,
            demo_max_subscriptions: 20,
            demo_admin_readonly: true,
            demo_session_ttl_hours: 24,
            unread_count_cache_ttl: rusty_feed_cache::keys::ttl::UNREAD_COUNTS,
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
    /// The instance's single embedding model (for semantic search).
    pub embedder: Arc<dyn Embedder>,
    /// Per-IP limiter for authentication endpoints.
    pub auth_limiter: RateLimiter,
    /// Best-effort per-token limit for native API automation.
    pub api_limiter: RateLimiter,
    /// Per-user limiter for feed subscription/discovery.
    pub feed_add_limiter: RateLimiter,
    /// Per-user limiter for (potentially expensive) search.
    pub search_limiter: RateLimiter,
    /// Per-user limiter for on-demand AI summaries.
    pub summarize_limiter: RateLimiter,
    /// Per-user limiter for OPML import/preview.
    pub opml_limiter: RateLimiter,
    /// Per-user limiter for the media proxy.
    pub media_limiter: RateLimiter,
    /// Outbound transactional mail (email verification); Noop when SMTP is off.
    pub mailer: crate::mailer::Mailer,
    pub(crate) live_view_tokens: Cryptor,
    pub(crate) public_share_tokens: Cryptor,
    pub(crate) email_verification_tokens: Cryptor,
    pub(crate) llm_credentials: Cryptor,
    pub(crate) integration_credentials: Cryptor,
    pub(crate) acquisition_credentials: Cryptor,
    pub(crate) webauthn: Option<Arc<Webauthn>>,
    live_view_fetch_locks: MokaCache<String, Arc<Mutex<()>>>,
    media_fetch_locks: MokaCache<String, Arc<Mutex<()>>>,
    /// Cached compiled feed blocklist (shared; short TTL + invalidate on edit).
    blocklist_cache: MokaCache<(), Arc<rusty_feed_core::blocklist::Blocklist>>,
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
        let mut live_view_key = [0_u8; 32];
        OsRng.fill_bytes(&mut live_view_key);
        let webauthn = if settings.passkeys_enabled {
            WebauthnUrl::parse(&settings.base_url)
                .ok()
                .and_then(|origin| {
                    let rp_id = origin.host_str()?.to_string();
                    WebauthnBuilder::new(&rp_id, &origin)
                        .ok()?
                        .rp_name(&settings.passkey_rp_name)
                        .build()
                        .ok()
                })
                .map(Arc::new)
        } else {
            None
        };
        let limits = settings.rate_limits;
        Self {
            db,
            cache,
            settings: Arc::new(settings),
            fetch: SafeFetchClient::default(),
            embedder: Arc::new(HashEmbedder::default()),
            // Per-key limiters built from instance config (0 == unlimited).
            auth_limiter: RateLimiter::per_minute(limits.auth_per_min),
            api_limiter: RateLimiter::per_minute(limits.api_per_min),
            feed_add_limiter: RateLimiter::per_minute(limits.feed_add_per_min),
            search_limiter: RateLimiter::per_minute(limits.search_per_min),
            summarize_limiter: RateLimiter::per_minute(limits.summarize_per_min),
            opml_limiter: RateLimiter::per_minute(limits.opml_per_min),
            media_limiter: RateLimiter::per_minute(limits.media_per_min),
            mailer: crate::mailer::Mailer::Noop,
            live_view_tokens: Cryptor::new(&live_view_key),
            public_share_tokens: Cryptor::new_with_context(
                &live_view_key,
                b"rusty-feed/public-share-tokens/v1",
            ),
            email_verification_tokens: Cryptor::new_with_context(
                &live_view_key,
                b"rusty-feed/email-verification/v1",
            ),
            llm_credentials: Cryptor::new_with_context(
                &live_view_key,
                b"rusty-feed/llm-provider-credentials/v1",
            ),
            integration_credentials: Cryptor::new_with_context(
                &live_view_key,
                b"rusty-feed/integration-credentials/v1",
            ),
            acquisition_credentials: Cryptor::new_with_context(
                &live_view_key,
                b"rusty-feed/acquisition-profile-secrets/v1",
            ),
            webauthn,
            live_view_fetch_locks: MokaCache::builder()
                .max_capacity(10_000)
                .time_to_idle(Duration::from_secs(30 * 60))
                .build(),
            media_fetch_locks: MokaCache::builder()
                .max_capacity(10_000)
                .time_to_idle(Duration::from_secs(30 * 60))
                .build(),
            blocklist_cache: MokaCache::builder()
                .max_capacity(1)
                .time_to_live(Duration::from_secs(30))
                .build(),
            metrics: Arc::new(metrics_renderer),
        }
    }

    /// The compiled feed blocklist, cached with a short TTL. Errors surface as
    /// an internal error so a feed-add fails cleanly rather than silently
    /// bypassing moderation.
    pub async fn blocklist(
        &self,
    ) -> Result<Arc<rusty_feed_core::blocklist::Blocklist>, crate::error::AppError> {
        let pool = self.db.pool().clone();
        self.blocklist_cache
            .try_get_with((), async move {
                rusty_feed_db::feed_blocklist::compiled(&pool)
                    .await
                    .map(Arc::new)
            })
            .await
            .map_err(|error: Arc<rusty_feed_db::DbError>| {
                crate::error::AppError::Internal(anyhow::anyhow!("blocklist load failed: {error}"))
            })
    }

    /// Drop the cached blocklist after an admin edit so the next check reloads.
    pub fn invalidate_blocklist(&self) {
        self.blocklist_cache.invalidate_all();
    }

    /// A user's per-feed unread counts for the Library sidebar, cached per user
    /// for `unread_count_cache_ttl` (FND-237). New arrivals may lag by up to
    /// that TTL; the user's own actions call [`Self::invalidate_unread_counts`].
    pub async fn unread_counts(
        &self,
        user_id: uuid::Uuid,
    ) -> rusty_feed_db::Result<Vec<(uuid::Uuid, i64)>> {
        let ttl = self.settings.unread_count_cache_ttl;
        let load = || rusty_feed_db::articles::unread_counts_by_feed(self.db.pool(), user_id);
        if ttl.is_zero() {
            return load().await;
        }
        self.cache
            .get_or_set(&rusty_feed_cache::keys::unread(user_id), ttl, load)
            .await
    }

    /// Forget a user's cached unread counts. Call after every action of theirs
    /// that changes read state or subscriptions.
    pub async fn invalidate_unread_counts(&self, user_id: uuid::Uuid) {
        if !self.settings.unread_count_cache_ttl.is_zero() {
            self.cache
                .delete(&rusty_feed_cache::keys::unread(user_id))
                .await;
        }
    }

    /// Override the fetch client (e.g. tests using an allow-private client).
    pub fn with_fetch(mut self, fetch: SafeFetchClient) -> Self {
        self.fetch = fetch;
        self
    }

    /// Derive short-lived Live View bearer tickets from an instance secret.
    pub fn with_live_view_secret(mut self, secret: &[u8]) -> Self {
        self.live_view_tokens = Cryptor::new(secret);
        self
    }

    /// Derive encrypted public-share token storage from the instance secret.
    pub fn with_public_share_secret(mut self, secret: &[u8]) -> Self {
        self.public_share_tokens =
            Cryptor::new_with_context(secret, b"rusty-feed/public-share-tokens/v1");
        self.email_verification_tokens =
            Cryptor::new_with_context(secret, b"rusty-feed/email-verification/v1");
        self
    }

    /// Configure the outbound mailer used for email verification.
    pub fn with_mailer(mut self, mailer: crate::mailer::Mailer) -> Self {
        self.mailer = mailer;
        self
    }

    /// Derive the managed-AI credential key from the instance encryption secret.
    pub fn with_llm_secret(mut self, secret: &[u8]) -> Self {
        self.llm_credentials =
            Cryptor::new_with_context(secret, b"rusty-feed/llm-provider-credentials/v1");
        self
    }

    pub fn with_integration_secret(mut self, secret: &[u8]) -> Self {
        self.integration_credentials =
            Cryptor::new_with_context(secret, b"rusty-feed/integration-credentials/v1");
        self
    }

    pub fn with_acquisition_secret(mut self, secret: &[u8]) -> Self {
        self.acquisition_credentials =
            Cryptor::new_with_context(secret, b"rusty-feed/acquisition-profile-secrets/v1");
        self
    }

    pub(crate) async fn fetch_for_feed(
        &self,
        feed_id: uuid::Uuid,
    ) -> anyhow::Result<SafeFetchClient> {
        let Some(profile) =
            rusty_feed_db::acquisition_profiles::for_feed(self.db.pool(), feed_id).await?
        else {
            return Ok(self.fetch.clone());
        };
        let secrets = profile
            .secrets_ciphertext
            .as_deref()
            .map(|encrypted| self.acquisition_credentials.decrypt_str(encrypted))
            .transpose()
            .map_err(|_| anyhow::anyhow!("acquisition profile secrets could not be decrypted"))?
            .map(|json| {
                serde_json::from_str::<rusty_feed_core::acquisition::AcquisitionSecrets>(&json)
            })
            .transpose()?
            .unwrap_or_default();
        Ok(self
            .fetch
            .for_acquisition(rusty_feed_feeds::AcquisitionPolicy {
                allowed_hosts: profile.allowed_hosts,
                allowed_path_prefix: profile.allowed_path_prefix,
                cookie_header: secrets.cookie_header,
                proxy_url: secrets.proxy_url,
                allow_invalid_tls: profile.allow_invalid_tls,
            })?)
    }

    /// Set the instance embedding model.
    pub fn with_embedder(mut self, embedder: Arc<dyn Embedder>) -> Self {
        self.embedder = embedder;
        self
    }

    /// Return the shared per-resource lock used to collapse concurrent Live View misses.
    pub(crate) async fn live_view_fetch_lock(&self, key: &str) -> Arc<Mutex<()>> {
        self.live_view_fetch_locks
            .get_with(key.to_string(), async { Arc::new(Mutex::new(())) })
            .await
    }

    /// Return the shared per-URL lock used to collapse concurrent media misses.
    pub(crate) async fn media_fetch_lock(&self, key: &str) -> Arc<Mutex<()>> {
        self.media_fetch_locks
            .get_with(key.to_string(), async { Arc::new(Mutex::new(())) })
            .await
    }

    /// Render the current metrics exposition text.
    pub fn render_metrics(&self) -> String {
        (self.metrics)()
    }
}

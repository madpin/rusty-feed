//! # rusty-feed-agents
//!
//! The AGENTS runtime (M7): a durable Postgres job queue (`SKIP LOCKED`
//! consumers), a periodic scheduler, singleton leases, and per-run records —
//! plus the concrete agents. This is what guarantees **O(1) shared work**: a
//! feed with N subscribers is fetched once (see `docs/ARCHITECTURE.md` §8).
//!
//! Concrete agents so far: [`FeedRefreshAgent`] (enqueue due feeds, deduped) and
//! [`FetchFeedAgent`] (fetch + ingest one feed, via `rusty-feed-feeds`).

mod catalog_preview;
mod content_cleanup;
mod demo_reset;
mod embed_article;
mod extract_content;
mod feed_refresh;
mod fetch_feed;
mod integration_delivery;
mod pattern_decay;
mod saved_search;
mod summarize;
mod worker;
mod youtube_duration;

pub use catalog_preview::CatalogPreviewAgent;
pub use content_cleanup::ContentCleanupAgent;
pub use demo_reset::DemoResetAgent;
pub use embed_article::EmbedArticleAgent;
pub use extract_content::ExtractContentAgent;
pub use feed_refresh::FeedRefreshAgent;
pub use fetch_feed::FetchFeedAgent;
pub use integration_delivery::IntegrationDeliveryAgent;
pub use pattern_decay::PatternDecayAgent;
pub use saved_search::{SavedSearchMatchAgent, SavedSearchScanAgent};
pub use summarize::SummarizeArticleAgent;
pub use worker::{enqueue_feed_refresh, process, Registry, Worker, WorkerConfig};
pub use youtube_duration::YouTubeDurationAgent;

use async_trait::async_trait;

use rusty_feed_core::ai_budget::AiBudget;
use rusty_feed_core::crypto::Cryptor;
use rusty_feed_core::{tracking_pixel::TrackingPixelPolicy, tracking_url::TrackingUrlPolicy};
use rusty_feed_db::Db;
use rusty_feed_feeds::SafeFetchClient;
use uuid::Uuid;

/// Ports injected into every agent.
#[derive(Clone)]
pub struct AgentContext {
    pub db: Db,
    pub fetch: SafeFetchClient,
    /// Whether new/extracted articles should be enqueued for embedding.
    pub embed_on_ingest: bool,
    /// Admin-owned automatic shared summarization policy.
    pub auto_summarize: bool,
    /// Whether feed ingest proactively backfills public-page extraction for
    /// article-page subscribers. When false, extraction happens lazily on the
    /// first user access (via the interactive lane) instead.
    pub eager_extraction: bool,
    /// Immutable instance-wide URL privacy policy used by shared processing.
    pub tracking_url_policy: TrackingUrlPolicy,
    /// Immutable tracking-image policy used by shared processing.
    pub tracking_pixel_policy: TrackingPixelPolicy,
    pub integration_credentials: Cryptor,
    pub acquisition_credentials: Cryptor,
    pub youtube_duration_enabled: bool,
    pub youtube_duration_daily_budget: u32,
    pub youtube_duration_cache_days: i64,
    /// Immutable AI budget enforced before background LLM spend (FND-187).
    pub ai_budget: AiBudget,
    /// Cap on articles ingested per feed refresh; `0` == unlimited (FND-189).
    pub feed_max_items_per_refresh: usize,
}

impl AgentContext {
    pub fn new(db: Db, fetch: SafeFetchClient) -> Self {
        Self {
            db,
            fetch,
            embed_on_ingest: false,
            auto_summarize: false,
            eager_extraction: true,
            tracking_url_policy: TrackingUrlPolicy::default(),
            tracking_pixel_policy: TrackingPixelPolicy::default(),
            integration_credentials: Cryptor::new(b"rusty-feed-agent-test-integration-key"),
            acquisition_credentials: Cryptor::new_with_context(
                b"rusty-feed-agent-test-acquisition-key",
                b"rusty-feed/acquisition-profile-secrets/v1",
            ),
            youtube_duration_enabled: false,
            youtube_duration_daily_budget: 100,
            youtube_duration_cache_days: 30,
            ai_budget: AiBudget::default(),
            feed_max_items_per_refresh: 0,
        }
    }

    pub fn with_ai_budget(mut self, budget: AiBudget) -> Self {
        self.ai_budget = budget;
        self
    }

    pub fn with_feed_max_items_per_refresh(mut self, max_items: usize) -> Self {
        self.feed_max_items_per_refresh = max_items;
        self
    }

    pub fn with_embed_on_ingest(mut self, enabled: bool) -> Self {
        self.embed_on_ingest = enabled;
        self
    }

    pub fn with_auto_summarize(mut self, enabled: bool) -> Self {
        self.auto_summarize = enabled;
        self
    }

    pub fn with_eager_extraction(mut self, enabled: bool) -> Self {
        self.eager_extraction = enabled;
        self
    }

    pub fn with_tracking_url_policy(mut self, policy: TrackingUrlPolicy) -> Self {
        self.tracking_url_policy = policy;
        self
    }

    pub fn with_tracking_pixel_policy(mut self, policy: TrackingPixelPolicy) -> Self {
        self.tracking_pixel_policy = policy;
        self
    }

    pub fn with_integration_credentials(mut self, cryptor: Cryptor) -> Self {
        self.integration_credentials = cryptor;
        self
    }

    pub fn with_acquisition_credentials(mut self, cryptor: Cryptor) -> Self {
        self.acquisition_credentials = cryptor;
        self
    }

    pub async fn fetch_for_feed(&self, feed_id: Uuid) -> anyhow::Result<SafeFetchClient> {
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

    pub fn with_youtube_duration_policy(
        mut self,
        enabled: bool,
        daily_budget: u32,
        cache_days: i64,
    ) -> Self {
        self.youtube_duration_enabled = enabled;
        self.youtube_duration_daily_budget = daily_budget;
        self.youtube_duration_cache_days = cache_days;
        self
    }
}

/// A unit of background behavior. `name()` doubles as the job `kind`.
#[async_trait]
pub trait Agent: Send + Sync {
    /// Stable identifier, used as the job `kind` and the `agent_runs.agent_name`.
    fn name(&self) -> &'static str;

    /// Execute for the given JSON payload, returning stats to record.
    async fn run(
        &self,
        ctx: &AgentContext,
        payload: serde_json::Value,
    ) -> anyhow::Result<serde_json::Value>;
}

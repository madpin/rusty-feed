//! # rustss-agents
//!
//! The AGENTS runtime (M7): a durable Postgres job queue (`SKIP LOCKED`
//! consumers), a periodic scheduler, singleton leases, and per-run records —
//! plus the concrete agents. This is what guarantees **O(1) shared work**: a
//! feed with N subscribers is fetched once (see `docs/ARCHITECTURE.md` §8).
//!
//! Concrete agents so far: [`FeedRefreshAgent`] (enqueue due feeds, deduped) and
//! [`FetchFeedAgent`] (fetch + ingest one feed, via `rustss-feeds`).

mod embed_article;
mod extract_content;
mod feed_refresh;
mod fetch_feed;
mod pattern_decay;
mod worker;

pub use embed_article::EmbedArticleAgent;
pub use extract_content::ExtractContentAgent;
pub use feed_refresh::FeedRefreshAgent;
pub use fetch_feed::FetchFeedAgent;
pub use pattern_decay::PatternDecayAgent;
pub use worker::{enqueue_feed_refresh, process, Registry, Worker, WorkerConfig};

use async_trait::async_trait;

use rustss_db::Db;
use rustss_feeds::SafeFetchClient;

/// Ports injected into every agent.
#[derive(Clone)]
pub struct AgentContext {
    pub db: Db,
    pub fetch: SafeFetchClient,
    /// Whether new/extracted articles should be enqueued for embedding.
    pub embed_on_ingest: bool,
}

impl AgentContext {
    pub fn new(db: Db, fetch: SafeFetchClient) -> Self {
        Self {
            db,
            fetch,
            embed_on_ingest: false,
        }
    }

    pub fn with_embed_on_ingest(mut self, enabled: bool) -> Self {
        self.embed_on_ingest = enabled;
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

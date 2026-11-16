//! # rusty-feed-db
//!
//! PostgreSQL + pgvector persistence via SQLx: the embedded migrations and the
//! repositories that enforce the shared/per-user data model
//! (`docs/ARCHITECTURE.md` §6, §7).
//!
//! Repositories are plain modules of functions taking `&PgPool`; shared writes
//! (feeds, articles) are idempotent so concurrent workers never duplicate
//! content. The [`Db`] handle owns the pool and runs migrations.

pub mod abuse_reports;
pub mod acquisition_profiles;
pub mod agent_locks;
pub mod agent_runs;
pub mod api_audit;
pub mod api_tokens;
pub mod articles;
pub mod bookmarks;
pub mod catalog;
pub mod categories;
pub mod cost_ledger;
pub mod digests;
pub mod domain_state;
pub mod embedding_space;
pub mod embeddings;
pub mod error;
pub mod estimates;
pub mod feed_blocklist;
pub mod feed_icons;
pub mod feedback;
pub mod feeds;
pub mod greader;
pub mod integrations;
pub mod invitations;
pub mod jobs;
pub mod llm_profiles;
pub mod media_cache;
pub mod models;
pub mod moderation;
pub mod notifications;
pub mod oauth_accounts;
pub mod operations;
pub mod outbox;
pub mod passkeys;
pub mod password_resets;
pub mod patterns;
pub mod public_shares;
pub mod read_articles;
pub mod rewrite_rules;
pub mod saved_search_matches;
pub mod saved_searches;
pub mod search;
pub mod seen_articles;
pub mod sessions;
pub mod stories;
pub mod subscriptions;
pub mod summary_policy;
pub mod ui_customizations;
pub mod url_backfill;
pub mod user_preferences;
pub mod users;
pub mod youtube_metadata;

pub use error::{DbError, Result};
pub use models::{
    ApiToken, Article, ArticleEnclosure, DomainState, Feed, Job, MediaCacheEntry, NewArticle,
    NewArticleEnclosure, OAuthAccount, SavedSearch, Session, UpsertAction, User, UserFeed,
};

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;

/// Embedded migrations (from `crates/db/migrations`).
pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// Connection-pool sizing, normally built from `DATABASE_MAX_CONNECTIONS` and
/// `DATABASE_ACQUIRE_TIMEOUT_SECONDS`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DbPoolSettings {
    /// Maximum open connections in the pool.
    pub max_connections: u32,
    /// How long `acquire` waits for a free connection before failing.
    pub acquire_timeout: Duration,
}

impl Default for DbPoolSettings {
    fn default() -> Self {
        Self {
            max_connections: 10,
            acquire_timeout: Duration::from_secs(30),
        }
    }
}

impl DbPoolSettings {
    /// Settings from the configured connection cap and acquire timeout (seconds).
    pub fn new(max_connections: u32, acquire_timeout_seconds: u64) -> Self {
        Self {
            max_connections,
            acquire_timeout: Duration::from_secs(acquire_timeout_seconds),
        }
    }

    fn options(&self) -> PgPoolOptions {
        PgPoolOptions::new()
            .max_connections(self.max_connections)
            .acquire_timeout(self.acquire_timeout)
    }
}

/// A cloneable database handle (the inner `PgPool` is reference-counted).
#[derive(Clone)]
pub struct Db {
    pool: PgPool,
    media_lock_pool: Arc<OnceLock<PgPool>>,
}

impl Db {
    /// Connect to Postgres using the given URL.
    pub async fn connect(database_url: &str) -> Result<Self> {
        Self::connect_with(database_url, &DbPoolSettings::default()).await
    }

    /// Connect to Postgres with explicit pool sizing.
    pub async fn connect_with(database_url: &str, settings: &DbPoolSettings) -> Result<Self> {
        let pool = settings.options().connect(database_url).await?;
        Ok(Self::from_pool(pool))
    }

    /// Create a lazily-connecting pool (no connection is opened until first use).
    /// Useful for constructing app state in tests that never touch the database.
    pub fn connect_lazy(database_url: &str) -> Result<Self> {
        Self::connect_lazy_with(database_url, &DbPoolSettings::default())
    }

    /// Lazily-connecting pool with explicit pool sizing.
    pub fn connect_lazy_with(database_url: &str, settings: &DbPoolSettings) -> Result<Self> {
        let pool = settings.options().connect_lazy(database_url)?;
        Ok(Self::from_pool(pool))
    }

    /// Wrap an existing pool (useful in tests).
    pub fn from_pool(pool: PgPool) -> Self {
        Self {
            pool,
            media_lock_pool: Arc::new(OnceLock::new()),
        }
    }

    /// Borrow the underlying connection pool.
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Small dedicated pool for media-fetch advisory locks (see
    /// [`media_cache::FetchLock`]), built lazily from the main pool's connect
    /// options so lock holders never consume main-pool connections.
    pub fn media_lock_pool(&self) -> &PgPool {
        self.media_lock_pool.get_or_init(|| {
            PgPoolOptions::new()
                .max_connections(media_cache::FETCH_LOCK_CONNECTIONS)
                .acquire_timeout(media_cache::FETCH_LOCK_WAIT)
                .connect_lazy_with((*self.pool.connect_options()).clone())
        })
    }

    /// Apply all pending migrations. Idempotent (already-applied ones are
    /// skipped), so it is safe to call on every startup.
    pub async fn migrate(&self) -> Result<()> {
        MIGRATOR.run(&self.pool).await?;
        Ok(())
    }

    /// Cheap connectivity check for readiness probes.
    pub async fn ping(&self) -> Result<()> {
        sqlx::query("SELECT 1").execute(&self.pool).await?;
        Ok(())
    }
}

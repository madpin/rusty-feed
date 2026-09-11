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
pub mod domain_state;
pub mod embedding_space;
pub mod embeddings;
pub mod error;
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
pub mod patterns;
pub mod public_shares;
pub mod read_articles;
pub mod rewrite_rules;
pub mod saved_search_matches;
pub mod saved_searches;
pub mod search;
pub mod sessions;
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

use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;

/// Embedded migrations (from `crates/db/migrations`).
pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// A cloneable database handle (the inner `PgPool` is reference-counted).
#[derive(Clone)]
pub struct Db {
    pool: PgPool,
}

impl Db {
    /// Connect to Postgres using the given URL.
    pub async fn connect(database_url: &str) -> Result<Self> {
        let pool = PgPoolOptions::new()
            .max_connections(10)
            .connect(database_url)
            .await?;
        Ok(Self { pool })
    }

    /// Create a lazily-connecting pool (no connection is opened until first use).
    /// Useful for constructing app state in tests that never touch the database.
    pub fn connect_lazy(database_url: &str) -> Result<Self> {
        let pool = PgPoolOptions::new()
            .max_connections(10)
            .connect_lazy(database_url)?;
        Ok(Self { pool })
    }

    /// Wrap an existing pool (useful in tests).
    pub fn from_pool(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Borrow the underlying connection pool.
    pub fn pool(&self) -> &PgPool {
        &self.pool
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

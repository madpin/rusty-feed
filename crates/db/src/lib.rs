//! # rustss-db
//!
//! PostgreSQL + pgvector persistence via SQLx: the embedded migrations and the
//! repositories that enforce the shared/per-user data model
//! (`docs/ARCHITECTURE.md` §6, §7).
//!
//! Repositories are plain modules of functions taking `&PgPool`; shared writes
//! (feeds, articles) are idempotent so concurrent workers never duplicate
//! content. The [`Db`] handle owns the pool and runs migrations.

pub mod api_tokens;
pub mod articles;
pub mod error;
pub mod feeds;
pub mod models;
pub mod sessions;
pub mod subscriptions;
pub mod users;

pub use error::{DbError, Result};
pub use models::{ApiToken, Article, Feed, NewArticle, Session, UpsertAction, User, UserFeed};

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

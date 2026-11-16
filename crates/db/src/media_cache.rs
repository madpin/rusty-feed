//! Shared, bounded-lifetime cache for proxied enclosure media.

use std::time::Duration;

use sqlx::pool::PoolConnection;
use sqlx::{PgPool, Postgres};
use time::OffsetDateTime;

use crate::models::MediaCacheEntry;
use crate::Result;

const MEDIA_CACHE_COLUMNS: &str =
    "cache_key, source_url, status, content_type, body, failure_kind, fetched_at, expires_at";

/// Connections in the dedicated media-fetch lock pool (see
/// `Db::media_lock_pool`). These are separate from, and never taken out of, the
/// main pool.
pub const FETCH_LOCK_CONNECTIONS: u32 = 8;

/// How long a miss waits for a free lock connection before fetching without the
/// cross-process lock.
pub const FETCH_LOCK_WAIT: Duration = Duration::from_millis(250);

/// A PostgreSQL session advisory lock for one source URL hash, held on a
/// connection from the dedicated lock pool for the duration of an upstream
/// fetch so concurrent misses across Rusty Feed processes download once.
///
/// The lock lives on the connection, so the connection is never returned to
/// the pool still locked: [`FetchLock::release`] unlocks first, and dropping
/// the guard without releasing (a cancelled request, including one still
/// waiting for the lock) closes the connection, which makes PostgreSQL release
/// the lock.
pub struct FetchLock {
    connection: Option<PoolConnection<Postgres>>,
    cache_key: String,
}

impl FetchLock {
    /// Block until the advisory lock for `cache_key` is held. Fails (without
    /// waiting on the fetch) when no lock connection frees up within
    /// [`FETCH_LOCK_WAIT`]; callers then proceed without cross-process
    /// coalescing, which is safe because cache writes are idempotent upserts.
    pub async fn acquire(lock_pool: &PgPool, cache_key: &str) -> Result<Self> {
        // The guard exists before the (possibly long) lock wait so that a
        // cancelled request closes the connection instead of returning it to
        // the pool with a lock request still pending on the server.
        let mut lock = Self {
            connection: Some(lock_pool.acquire().await?),
            cache_key: cache_key.to_string(),
        };
        if let Some(connection) = lock.connection.as_mut() {
            sqlx::query("SELECT pg_advisory_lock(hashtextextended($1, 0))")
                .bind(cache_key)
                .execute(&mut **connection)
                .await?;
        }
        Ok(lock)
    }

    /// Unlock and return the connection to the lock pool.
    pub async fn release(mut self) -> Result<()> {
        let Some(mut connection) = self.connection.take() else {
            return Ok(());
        };
        let unlocked: std::result::Result<bool, sqlx::Error> =
            sqlx::query_scalar("SELECT pg_advisory_unlock(hashtextextended($1, 0))")
                .bind(&self.cache_key)
                .fetch_one(&mut *connection)
                .await;
        if !matches!(unlocked, Ok(true)) {
            connection.close_on_drop();
        }
        unlocked?;
        Ok(())
    }
}

impl Drop for FetchLock {
    fn drop(&mut self) {
        if let Some(mut connection) = self.connection.take() {
            connection.close_on_drop();
        }
    }
}

/// Return a non-expired cache entry.
pub async fn find_fresh(pool: &PgPool, cache_key: &str) -> Result<Option<MediaCacheEntry>> {
    let sql = format!(
        "SELECT {MEDIA_CACHE_COLUMNS} FROM media_cache \
         WHERE cache_key = $1 AND expires_at > now()"
    );
    Ok(sqlx::query_as(&sql)
        .bind(cache_key)
        .fetch_optional(pool)
        .await?)
}

/// Replace a URL's entry with a successfully fetched body.
pub async fn store_success(
    pool: &PgPool,
    cache_key: &str,
    source_url: &str,
    content_type: Option<&str>,
    body: &[u8],
    expires_at: OffsetDateTime,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO media_cache \
         (cache_key, source_url, status, content_type, body, failure_kind, expires_at) \
         VALUES ($1, $2, 'ready', $3, $4, NULL, $5) \
         ON CONFLICT (cache_key) DO UPDATE SET \
         source_url = EXCLUDED.source_url, status = 'ready', \
         content_type = EXCLUDED.content_type, body = EXCLUDED.body, failure_kind = NULL, \
         fetched_at = now(), expires_at = EXCLUDED.expires_at, updated_at = now()",
    )
    .bind(cache_key)
    .bind(source_url)
    .bind(content_type)
    .bind(body)
    .bind(expires_at)
    .execute(pool)
    .await?;
    Ok(())
}

/// Replace a URL's entry with a short-lived, client-safe failure category.
pub async fn store_failure(
    pool: &PgPool,
    cache_key: &str,
    source_url: &str,
    failure_kind: &str,
    expires_at: OffsetDateTime,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO media_cache \
         (cache_key, source_url, status, content_type, body, failure_kind, expires_at) \
         VALUES ($1, $2, 'failed', NULL, NULL, $3, $4) \
         ON CONFLICT (cache_key) DO UPDATE SET \
         source_url = EXCLUDED.source_url, status = 'failed', content_type = NULL, body = NULL, \
         failure_kind = EXCLUDED.failure_kind, fetched_at = now(), \
         expires_at = EXCLUDED.expires_at, updated_at = now()",
    )
    .bind(cache_key)
    .bind(source_url)
    .bind(failure_kind)
    .bind(expires_at)
    .execute(pool)
    .await?;
    Ok(())
}

/// Delete expired positive and negative media entries.
pub async fn delete_expired(pool: &PgPool) -> Result<u64> {
    Ok(
        sqlx::query("DELETE FROM media_cache WHERE expires_at <= now()")
            .execute(pool)
            .await?
            .rows_affected(),
    )
}

/// Evict oldest successful bodies until the aggregate byte budget is met.
pub async fn trim_to_budget(pool: &PgPool, max_bytes: i64) -> Result<u64> {
    let removed: i64 = sqlx::query_scalar(
        "WITH ranked AS ( \
             SELECT cache_key, \
                    SUM(octet_length(body)) OVER (ORDER BY updated_at DESC, cache_key) AS used \
             FROM media_cache WHERE status = 'ready' \
         ), deleted AS ( \
             DELETE FROM media_cache WHERE cache_key IN ( \
                 SELECT cache_key FROM ranked WHERE used > $1 \
             ) RETURNING 1 \
         ) SELECT COUNT(*) FROM deleted",
    )
    .bind(max_bytes)
    .fetch_one(pool)
    .await?;
    Ok(removed as u64)
}

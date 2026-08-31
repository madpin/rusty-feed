//! Shared, bounded-lifetime cache for proxied enclosure media.

use sqlx::pool::PoolConnection;
use sqlx::{PgPool, Postgres};
use time::OffsetDateTime;

use crate::models::MediaCacheEntry;
use crate::Result;

const MEDIA_CACHE_COLUMNS: &str =
    "cache_key, source_url, status, content_type, body, failure_kind, fetched_at, expires_at";

/// Acquire a PostgreSQL-session advisory lock for one source URL hash. Holding
/// the returned connection holds the lock across all RustSS processes.
pub async fn acquire_fetch_lock(
    pool: &PgPool,
    cache_key: &str,
) -> Result<PoolConnection<Postgres>> {
    let mut connection = pool.acquire().await?;
    sqlx::query("SELECT pg_advisory_lock(hashtextextended($1, 0))")
        .bind(cache_key)
        .execute(&mut *connection)
        .await?;
    Ok(connection)
}

/// Release a media fetch lock before returning its connection to the pool.
pub async fn release_fetch_lock(
    connection: &mut PoolConnection<Postgres>,
    cache_key: &str,
) -> Result<bool> {
    Ok(
        sqlx::query_scalar("SELECT pg_advisory_unlock(hashtextextended($1, 0))")
            .bind(cache_key)
            .fetch_one(&mut **connection)
            .await?,
    )
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

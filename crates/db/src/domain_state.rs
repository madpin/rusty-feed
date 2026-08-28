//! Per-domain extraction politeness state (M8): spacing, robots crawl-delay
//! cache, and adaptive backoff on rate limits/failures.

use sqlx::PgPool;

use crate::error::Result;
use crate::models::{DomainState, DOMAIN_STATE_COLUMNS};

/// Fetch a domain's state, if any.
pub async fn get(pool: &PgPool, domain: &str) -> Result<Option<DomainState>> {
    let sql =
        format!("SELECT {DOMAIN_STATE_COLUMNS} FROM domain_extraction_state WHERE domain = $1");
    Ok(sqlx::query_as::<_, DomainState>(&sql)
        .bind(domain)
        .fetch_optional(pool)
        .await?)
}

/// Record a successful extraction: bump `last_extracted_at`, reset failures, and
/// (optionally) cache the robots.txt crawl-delay.
pub async fn record_success(
    pool: &PgPool,
    domain: &str,
    crawl_delay_ms: Option<i32>,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO domain_extraction_state (domain, last_extracted_at, crawl_delay_ms, updated_at) \
         VALUES ($1, now(), $2, now()) \
         ON CONFLICT (domain) DO UPDATE SET last_extracted_at = now(), failure_count = 0, \
             rate_limit_count = 0, crawl_delay_ms = COALESCE($2, domain_extraction_state.crawl_delay_ms), \
             last_error = NULL, updated_at = now()",
    )
    .bind(domain)
    .bind(crawl_delay_ms)
    .execute(pool)
    .await?;
    Ok(())
}

/// Record a failed extraction attempt.
pub async fn record_failure(pool: &PgPool, domain: &str, error: &str) -> Result<()> {
    sqlx::query(
        "INSERT INTO domain_extraction_state (domain, last_extracted_at, failure_count, last_error, updated_at) \
         VALUES ($1, now(), 1, $2, now()) \
         ON CONFLICT (domain) DO UPDATE SET last_extracted_at = now(), \
             failure_count = domain_extraction_state.failure_count + 1, last_error = $2, updated_at = now()",
    )
    .bind(domain)
    .bind(error)
    .execute(pool)
    .await?;
    Ok(())
}

/// Record that the domain rate-limited us (HTTP 429).
pub async fn record_rate_limit(pool: &PgPool, domain: &str) -> Result<()> {
    sqlx::query(
        "INSERT INTO domain_extraction_state (domain, last_extracted_at, rate_limit_count, updated_at) \
         VALUES ($1, now(), 1, now()) \
         ON CONFLICT (domain) DO UPDATE SET last_extracted_at = now(), \
             rate_limit_count = domain_extraction_state.rate_limit_count + 1, updated_at = now()",
    )
    .bind(domain)
    .execute(pool)
    .await?;
    Ok(())
}

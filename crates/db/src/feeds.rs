//! Feed repository — shared feeds, deduplicated by normalized URL.
//!
//! [`upsert_by_url`] is the write path that guarantees a feed URL maps to
//! exactly one row no matter how many users subscribe (FR-DEDUP-1).

use serde_json::Value as Json;
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::error::{DbError, Result};
use crate::models::{Feed, FEED_COLUMNS};

/// Find a feed by its (already normalized) URL.
pub async fn find_by_url(pool: &PgPool, url: &str) -> Result<Option<Feed>> {
    let sql = format!("SELECT {FEED_COLUMNS} FROM feeds WHERE url = $1");
    Ok(sqlx::query_as::<_, Feed>(&sql)
        .bind(url)
        .fetch_optional(pool)
        .await?)
}

/// Find a feed by id.
pub async fn find_by_id(pool: &PgPool, id: Uuid) -> Result<Option<Feed>> {
    let sql = format!("SELECT {FEED_COLUMNS} FROM feeds WHERE id = $1");
    Ok(sqlx::query_as::<_, Feed>(&sql)
        .bind(id)
        .fetch_optional(pool)
        .await?)
}

/// Get-or-create a shared feed by URL. Returns the feed and whether it was newly
/// created. Concurrent callers are handled via `ON CONFLICT DO NOTHING` + reread.
pub async fn upsert_by_url(
    pool: &PgPool,
    url: &str,
    name: &str,
    site_url: Option<&str>,
) -> Result<(Feed, bool)> {
    if let Some(existing) = find_by_url(pool, url).await? {
        return Ok((existing, false));
    }

    let sql = format!(
        "INSERT INTO feeds (id, url, name, site_url) VALUES ($1, $2, $3, $4) \
         ON CONFLICT (url) DO NOTHING RETURNING {FEED_COLUMNS}"
    );
    let inserted = sqlx::query_as::<_, Feed>(&sql)
        .bind(Uuid::now_v7())
        .bind(url)
        .bind(name)
        .bind(site_url)
        .fetch_optional(pool)
        .await?;

    match inserted {
        Some(feed) => Ok((feed, true)),
        // Lost the race with a concurrent insert: the row now exists.
        None => find_by_url(pool, url)
            .await?
            .map(|f| (f, false))
            .ok_or(DbError::NotFound),
    }
}

/// List feeds due for a refresh, system-wide (not per-user): either never fetched,
/// past the health-aware cadence, or due for a weekly icon attempt. Ordered oldest
/// first. This is what makes refresh O(1) per shared feed.
pub async fn list_due(pool: &PgPool, limit: i64) -> Result<Vec<Feed>> {
    let sql = format!(
        "SELECT {FEED_COLUMNS} FROM feeds \
         WHERE health_status <> 'disabled' AND blocked_at IS NULL \
           AND (EXISTS (SELECT 1 FROM user_feeds uf WHERE uf.feed_id = feeds.id) \
                OR EXISTS (SELECT 1 FROM feed_catalog_entries ce \
                           WHERE ce.feed_id = feeds.id AND ce.published)) \
           AND (last_fetched_at IS NULL \
                OR last_fetched_at + make_interval(mins => CASE \
                    WHEN consecutive_failures >= 3 AND failure_started_at <= now() - interval '14 days' \
                        THEN greatest(fetch_interval_minutes, 10080) \
                    WHEN consecutive_failures >= 3 THEN greatest(fetch_interval_minutes, 1440) \
                    ELSE fetch_interval_minutes END) <= now() \
                OR (consecutive_failures = 0 AND NOT EXISTS (SELECT 1 FROM feed_icons fi \
                    WHERE fi.feed_id = feeds.id AND fi.last_attempt_at > now() - interval '7 days'))) \
         ORDER BY last_fetched_at ASC NULLS FIRST \
         LIMIT $1"
    );
    Ok(sqlx::query_as::<_, Feed>(&sql)
        .bind(limit)
        .fetch_all(pool)
        .await?)
}

/// List the distinct shared feeds a user subscribes to.
pub async fn list_for_user(pool: &PgPool, user_id: Uuid) -> Result<Vec<Feed>> {
    let sql = format!(
        "SELECT {} FROM feeds f \
         JOIN user_feeds uf ON uf.feed_id = f.id AND uf.user_id = $1 \
         ORDER BY f.name ASC",
        FEED_COLUMNS
            .split(", ")
            .map(|c| format!("f.{c}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    Ok(sqlx::query_as::<_, Feed>(&sql)
        .bind(user_id)
        .fetch_all(pool)
        .await?)
}

/// List non-paused shared feeds eligible for a forced refresh.
pub async fn list_active_ids(pool: &PgPool) -> Result<Vec<Uuid>> {
    Ok(sqlx::query_scalar(
        "SELECT id FROM feeds WHERE health_status <> 'disabled' AND blocked_at IS NULL",
    )
    .fetch_all(pool)
    .await?)
}

/// Mark selected non-paused shared feeds due immediately without clearing health history.
pub async fn reset_refresh_schedule(pool: &PgPool, feed_ids: &[Uuid]) -> Result<Vec<Uuid>> {
    if feed_ids.is_empty() {
        return Ok(Vec::new());
    }
    Ok(sqlx::query_scalar(
        "UPDATE feeds SET last_fetched_at = NULL, updated_at = now() \
         WHERE id = ANY($1) AND health_status <> 'disabled' AND blocked_at IS NULL RETURNING id",
    )
    .bind(feed_ids)
    .fetch_all(pool)
    .await?)
}

/// Blocklist a shared feed (FND-192): set the dedicated block markers so all
/// refresh-suppression sites and the fetch agent skip it. Idempotent.
pub async fn set_blocked(pool: &PgPool, feed_id: Uuid, reason: Option<&str>) -> Result<bool> {
    Ok(sqlx::query(
        "UPDATE feeds SET blocked_at = COALESCE(blocked_at, now()), blocked_reason = $2, \
                updated_at = now() WHERE id = $1",
    )
    .bind(feed_id)
    .bind(reason)
    .execute(pool)
    .await?
    .rows_affected()
        > 0)
}

/// Lift a feed block.
pub async fn clear_blocked(pool: &PgPool, feed_id: Uuid) -> Result<bool> {
    Ok(sqlx::query(
        "UPDATE feeds SET blocked_at = NULL, blocked_reason = NULL, updated_at = now() \
         WHERE id = $1 AND blocked_at IS NOT NULL",
    )
    .bind(feed_id)
    .execute(pool)
    .await?
    .rows_affected()
        > 0)
}

/// Explicit administrator pause/resume. Resume derives health from preserved
/// failure history instead of falsely declaring a failing source healthy.
pub async fn set_paused(pool: &PgPool, feed_id: Uuid, paused: bool) -> Result<bool> {
    let result = sqlx::query(
        "UPDATE feeds SET health_status = CASE WHEN $2 THEN 'disabled' \
              WHEN consecutive_failures = 0 THEN 'healthy' \
              WHEN consecutive_failures < 3 THEN 'warning' ELSE 'error' END, \
         last_fetched_at = CASE WHEN $2 THEN last_fetched_at ELSE NULL END, updated_at = now() \
         WHERE id = $1 RETURNING id",
    )
    .bind(feed_id)
    .bind(paused)
    .fetch_optional(pool)
    .await?;
    Ok(result.is_some())
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct AdminFeed {
    pub id: Uuid,
    pub name: String,
    pub url: String,
    pub health_status: String,
    pub fetch_interval_minutes: i32,
    pub consecutive_failures: i32,
    pub failure_started_at: Option<time::OffsetDateTime>,
    pub extraction_settings: Option<Json>,
    pub subscriber_count: i64,
    pub article_count: i64,
    pub last_post_at: Option<time::OffsetDateTime>,
    pub last_fetched_at: Option<time::OffsetDateTime>,
}

/// Aggregate counters for the shared-feed admin overview.
#[derive(Debug, Clone, Default, sqlx::FromRow)]
pub struct AdminFeedSummary {
    pub total: i64,
    pub healthy: i64,
    pub warning: i64,
    pub error: i64,
    pub disabled: i64,
    pub subscribers: i64,
    pub articles: i64,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct FeedErrorEvent {
    pub feed_id: Uuid,
    pub feed_name: String,
    pub occurred_at: time::OffsetDateTime,
    pub error_type: String,
    pub http_status: Option<i32>,
    pub health_status: String,
    pub consecutive_failures: i32,
    pub resolved: bool,
}

#[derive(Debug, Clone, Copy)]
pub enum FetchFailure {
    Network,
    InvalidUrl,
    UnsupportedScheme,
    ResponseTooLarge,
    Http(i32),
    Parse,
    Database,
}

impl FetchFailure {
    fn error_type(self) -> &'static str {
        match self {
            Self::Network => "network",
            Self::InvalidUrl => "invalid_url",
            Self::UnsupportedScheme => "unsupported_scheme",
            Self::ResponseTooLarge => "response_too_large",
            Self::Http(_) => "http",
            Self::Parse => "parse",
            Self::Database => "database",
        }
    }

    fn summary(self) -> String {
        match self {
            Self::Network => "Network request failed".to_string(),
            Self::InvalidUrl => "Feed URL was rejected".to_string(),
            Self::UnsupportedScheme => "Feed URL scheme is unsupported".to_string(),
            Self::ResponseTooLarge => "Feed response exceeded the size limit".to_string(),
            Self::Http(status) => format!("HTTP {status}"),
            Self::Parse => "Feed response could not be parsed".to_string(),
            Self::Database => "Feed state update failed".to_string(),
        }
    }

    fn http_status(self) -> Option<i32> {
        match self {
            Self::Http(status) => Some(status),
            _ => None,
        }
    }
}

pub async fn count(pool: &PgPool) -> Result<i64> {
    Ok(sqlx::query_scalar("SELECT count(*) FROM feeds")
        .fetch_one(pool)
        .await?)
}

pub async fn list_admin(pool: &PgPool, limit: i64) -> Result<Vec<AdminFeed>> {
    list_admin_filtered(pool, None, None, limit, 0).await
}

/// Paginated, filterable shared-feed listing for the admin console. `health`
/// filters by health status (`healthy`/`warning`/`error`/`disabled`); `search`
/// matches feed name or URL case-insensitively.
pub async fn list_admin_filtered(
    pool: &PgPool,
    health: Option<&str>,
    search: Option<&str>,
    limit: i64,
    offset: i64,
) -> Result<Vec<AdminFeed>> {
    Ok(sqlx::query_as(
        "SELECT f.id, f.name, f.url, f.health_status, f.fetch_interval_minutes, \
           f.consecutive_failures, f.failure_started_at, f.extraction_settings, \
           s.subscriber_count, a.article_count, a.last_post_at, f.last_fetched_at \
         FROM feeds f \
         LEFT JOIN LATERAL (SELECT count(*)::bigint AS subscriber_count \
                            FROM user_feeds WHERE feed_id = f.id) s ON true \
         LEFT JOIN LATERAL (SELECT count(*)::bigint AS article_count, max(published_at) AS last_post_at \
                            FROM articles WHERE feed_id = f.id) a ON true \
         WHERE ($1::text IS NULL OR f.health_status = $1) \
           AND ($2::text IS NULL OR f.name ILIKE $2 OR f.url ILIKE $2) \
         ORDER BY f.updated_at DESC LIMIT $3 OFFSET $4",
    )
    .bind(health)
    .bind(search.map(|term| format!("%{term}%")))
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?)
}

/// Count feeds matching the same admin filters, for pagination.
pub async fn count_admin_filtered(
    pool: &PgPool,
    health: Option<&str>,
    search: Option<&str>,
) -> Result<i64> {
    Ok(sqlx::query_scalar(
        "SELECT count(*)::bigint FROM feeds f \
         WHERE ($1::text IS NULL OR f.health_status = $1) \
           AND ($2::text IS NULL OR f.name ILIKE $2 OR f.url ILIKE $2)",
    )
    .bind(health)
    .bind(search.map(|term| format!("%{term}%")))
    .fetch_one(pool)
    .await?)
}

/// Aggregate counters for the shared-feed admin overview.
pub async fn admin_summary(pool: &PgPool) -> Result<AdminFeedSummary> {
    Ok(sqlx::query_as(
        "SELECT count(*) AS total, \
           count(*) FILTER (WHERE health_status = 'healthy') AS healthy, \
           count(*) FILTER (WHERE health_status = 'warning') AS warning, \
           count(*) FILTER (WHERE health_status = 'error') AS error, \
           count(*) FILTER (WHERE health_status = 'disabled') AS disabled, \
           (SELECT count(*) FROM user_feeds) AS subscribers, \
           (SELECT count(*) FROM articles) AS articles \
         FROM feeds",
    )
    .fetch_one(pool)
    .await?)
}

pub async fn list_recent_errors(pool: &PgPool, limit: i64) -> Result<Vec<FeedErrorEvent>> {
    Ok(sqlx::query_as(
        "SELECT e.feed_id, f.name AS feed_name, e.occurred_at, e.error_type, e.http_status, \
         f.health_status, f.consecutive_failures, e.resolved \
         FROM feed_error_log e JOIN feeds f ON f.id = e.feed_id \
         ORDER BY e.occurred_at DESC LIMIT $1",
    )
    .bind(limit)
    .fetch_all(pool)
    .await?)
}

/// Update a feed's canonical name/site URL from parsed feed metadata. (Per-user
/// display names live in `user_feeds.custom_name` and are unaffected.)
pub async fn update_metadata(
    pool: &PgPool,
    feed_id: Uuid,
    name: &str,
    site_url: Option<&str>,
) -> Result<()> {
    sqlx::query("UPDATE feeds SET name = $2, site_url = COALESCE($3, site_url) WHERE id = $1")
        .bind(feed_id)
        .bind(name)
        .bind(site_url)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn update_image_url(pool: &PgPool, feed_id: Uuid, image_url: &str) -> Result<()> {
    sqlx::query(
        "UPDATE feeds SET image_url = $2, updated_at = now() WHERE id = $1 \
         AND image_url IS DISTINCT FROM $2",
    )
    .bind(feed_id)
    .bind(image_url)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn update_content_settings(
    pool: &PgPool,
    feed_id: Uuid,
    settings: &Json,
    respect_robots_txt: bool,
    extraction_delay_ms: Option<i32>,
    fetch_interval_minutes: i32,
) -> Result<()> {
    sqlx::query(
        "UPDATE feeds SET extraction_settings = $2, respect_robots_txt = $3, \
         extraction_delay_ms = $4, fetch_interval_minutes = $5, updated_at = now() \
         WHERE id = $1",
    )
    .bind(feed_id)
    .bind(settings)
    .bind(respect_robots_txt)
    .bind(extraction_delay_ms)
    .bind(fetch_interval_minutes)
    .execute(pool)
    .await?;
    Ok(())
}

/// Force the next shared refresh to retrieve a full representation instead of
/// accepting a conditional 304. Used when administrator-owned processing rules
/// change and the stored feed body must be rebuilt from publisher input.
pub async fn clear_fetch_validators(pool: &PgPool, feed_id: Uuid) -> Result<()> {
    sqlx::query(
        "UPDATE feeds SET etag = NULL, last_modified = NULL, updated_at = now() WHERE id = $1",
    )
    .bind(feed_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Record a successful fetch: refresh validators + reset health.
pub async fn record_fetch_success(
    pool: &PgPool,
    feed_id: Uuid,
    etag: Option<&str>,
    last_modified: Option<&str>,
) -> Result<()> {
    let mut transaction = pool.begin().await?;
    let previous: Option<(String, String, Option<OffsetDateTime>)> = sqlx::query_as(
        "SELECT name, health_status, failure_started_at FROM feeds WHERE id = $1 FOR UPDATE",
    )
    .bind(feed_id)
    .fetch_optional(&mut *transaction)
    .await?;
    sqlx::query(
        "UPDATE feeds SET last_fetched_at = now(), last_successful_fetch_at = now(), \
         etag = $2, last_modified = $3, error_count = 0, consecutive_failures = 0, \
         failure_started_at = NULL, health_status = 'healthy', last_error = NULL, http_status = NULL WHERE id = $1",
    )
    .bind(feed_id)
    .bind(etag)
    .bind(last_modified)
    .execute(&mut *transaction)
    .await?;
    sqlx::query("UPDATE feed_error_log SET resolved = true WHERE feed_id = $1 AND NOT resolved")
        .bind(feed_id)
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await?;
    if let Some((name, previous_status, failure_started_at)) = previous {
        if previous_status == "error" {
            let event_key = format!(
                "feed:{feed_id}:recovered:{}",
                failure_started_at
                    .map(|started| started.unix_timestamp())
                    .unwrap_or_default()
            );
            crate::notifications::create_feed_health_batch(
                pool,
                feed_id,
                &name,
                "recovered",
                &event_key,
            )
            .await?;
        }
    }
    Ok(())
}

/// Record a typed failed fetch and preserve the failure window used for retry cadence.
pub async fn record_fetch_failure(
    pool: &PgPool,
    feed_id: Uuid,
    failure: FetchFailure,
) -> Result<()> {
    let error_type = failure.error_type();
    let error = failure.summary();
    let http_status = failure.http_status();
    let mut transaction = pool.begin().await?;
    let transition: Option<(String, i32, Option<OffsetDateTime>)> = sqlx::query_as(
        "UPDATE feeds SET last_fetched_at = now(), error_count = error_count + 1, \
         consecutive_failures = consecutive_failures + 1, \
         failure_started_at = coalesce(failure_started_at, now()), last_error = $2, http_status = $3, \
         health_status = CASE WHEN consecutive_failures + 1 >= 3 THEN 'error' ELSE 'warning' END \
         WHERE id = $1 RETURNING name, consecutive_failures, failure_started_at",
    )
    .bind(feed_id)
    .bind(&error)
    .bind(http_status)
    .fetch_optional(&mut *transaction)
    .await?;
    sqlx::query(
        "INSERT INTO feed_error_log (id, feed_id, error_type, error_message, http_status) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(Uuid::now_v7())
    .bind(feed_id)
    .bind(error_type)
    .bind(&error)
    .bind(http_status)
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    if let Some((name, consecutive_failures, failure_started_at)) = transition {
        if consecutive_failures == 3 {
            let event_key = format!(
                "feed:{feed_id}:failing:{}",
                failure_started_at
                    .map(|started| started.unix_timestamp())
                    .unwrap_or_default()
            );
            crate::notifications::create_feed_health_batch(
                pool, feed_id, &name, "failing", &event_key,
            )
            .await?;
        }
    }
    Ok(())
}

/// Count feeds with a given URL (0 or 1). Handy for dedup assertions/tests.
pub async fn count_by_url(pool: &PgPool, url: &str) -> Result<i64> {
    Ok(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM feeds WHERE url = $1")
            .bind(url)
            .fetch_one(pool)
            .await?,
    )
}

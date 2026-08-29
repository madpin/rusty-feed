//! Feed repository — shared feeds, deduplicated by normalized URL.
//!
//! [`upsert_by_url`] is the write path that guarantees a feed URL maps to
//! exactly one row no matter how many users subscribe (FR-DEDUP-1).

use serde_json::Value as Json;
use sqlx::PgPool;
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

/// List feeds due for a refresh, system-wide (not per-user): not disabled, and
/// either never fetched, past its `fetch_interval_minutes`, or due for a weekly
/// icon attempt. Ordered oldest first. This is what makes refresh O(1) per shared feed.
pub async fn list_due(pool: &PgPool, limit: i64) -> Result<Vec<Feed>> {
    let sql = format!(
        "SELECT {FEED_COLUMNS} FROM feeds \
         WHERE health_status <> 'disabled' \
           AND (EXISTS (SELECT 1 FROM user_feeds uf WHERE uf.feed_id = feeds.id) \
                OR EXISTS (SELECT 1 FROM feed_catalog_entries ce \
                           WHERE ce.feed_id = feeds.id AND ce.published)) \
           AND (last_fetched_at IS NULL \
                OR last_fetched_at + make_interval(mins => fetch_interval_minutes) <= now() \
                OR NOT EXISTS (SELECT 1 FROM feed_icons fi WHERE fi.feed_id = feeds.id \
                    AND fi.last_attempt_at > now() - interval '7 days')) \
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

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct AdminFeed {
    pub id: Uuid,
    pub name: String,
    pub url: String,
    pub health_status: String,
    pub fetch_interval_minutes: i32,
    pub extraction_settings: Option<Json>,
    pub subscriber_count: i64,
    pub article_count: i64,
    pub last_post_at: Option<time::OffsetDateTime>,
}

pub async fn count(pool: &PgPool) -> Result<i64> {
    Ok(sqlx::query_scalar("SELECT count(*) FROM feeds")
        .fetch_one(pool)
        .await?)
}

pub async fn list_admin(pool: &PgPool, limit: i64) -> Result<Vec<AdminFeed>> {
    Ok(sqlx::query_as(
        "SELECT f.id, f.name, f.url, f.health_status, f.fetch_interval_minutes, \
           f.extraction_settings, s.subscriber_count, a.article_count, a.last_post_at \
         FROM feeds f \
         LEFT JOIN LATERAL (SELECT count(*)::bigint AS subscriber_count \
                            FROM user_feeds WHERE feed_id = f.id) s ON true \
         LEFT JOIN LATERAL (SELECT count(*)::bigint AS article_count, max(published_at) AS last_post_at \
                            FROM articles WHERE feed_id = f.id) a ON true \
         ORDER BY f.updated_at DESC LIMIT $1",
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

/// Record a successful fetch: refresh validators + reset health.
pub async fn record_fetch_success(
    pool: &PgPool,
    feed_id: Uuid,
    etag: Option<&str>,
    last_modified: Option<&str>,
) -> Result<()> {
    sqlx::query(
        "UPDATE feeds SET last_fetched_at = now(), last_successful_fetch_at = now(), \
         etag = $2, last_modified = $3, error_count = 0, consecutive_failures = 0, \
         health_status = 'healthy', last_error = NULL WHERE id = $1",
    )
    .bind(feed_id)
    .bind(etag)
    .bind(last_modified)
    .execute(pool)
    .await?;
    Ok(())
}

/// Record a failed fetch: bump counters and downgrade health (auto-disable at the
/// configured threshold).
pub async fn record_fetch_failure(pool: &PgPool, feed_id: Uuid, error: &str) -> Result<()> {
    sqlx::query(
        "UPDATE feeds SET last_fetched_at = now(), error_count = error_count + 1, \
         consecutive_failures = consecutive_failures + 1, last_error = $2, \
         health_status = CASE \
            WHEN consecutive_failures + 1 >= auto_disable_threshold THEN 'disabled' \
            WHEN consecutive_failures + 1 >= 3 THEN 'error' \
            ELSE 'warning' END \
         WHERE id = $1",
    )
    .bind(feed_id)
    .bind(error)
    .execute(pool)
    .await?;
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

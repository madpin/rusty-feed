use std::collections::HashSet;

use sqlx::PgPool;
use uuid::Uuid;

use crate::error::Result;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct FeedIcon {
    pub content_type: String,
    pub content: Vec<u8>,
}

pub async fn should_attempt(pool: &PgPool, feed_id: Uuid, source_url: &str) -> Result<bool> {
    Ok(sqlx::query_scalar(
        "SELECT NOT EXISTS (SELECT 1 FROM feed_icons WHERE feed_id = $1 AND source_url = $2 \
         AND last_attempt_at > now() - interval '7 days')",
    )
    .bind(feed_id)
    .bind(source_url)
    .fetch_one(pool)
    .await?)
}

pub async fn record_success(
    pool: &PgPool,
    feed_id: Uuid,
    source_url: &str,
    content_type: &str,
    content: &[u8],
) -> Result<()> {
    sqlx::query(
        "INSERT INTO feed_icons (feed_id, source_url, content_type, content, last_attempt_at, fetched_at, last_error) \
         VALUES ($1, $2, $3, $4, now(), now(), NULL) \
         ON CONFLICT (feed_id) DO UPDATE SET source_url = EXCLUDED.source_url, \
         content_type = EXCLUDED.content_type, content = EXCLUDED.content, last_attempt_at = now(), \
         fetched_at = now(), last_error = NULL",
    )
    .bind(feed_id)
    .bind(source_url)
    .bind(content_type)
    .bind(content)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn record_failure(
    pool: &PgPool,
    feed_id: Uuid,
    source_url: &str,
    error: &str,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO feed_icons (feed_id, source_url, content_type, content, last_attempt_at, fetched_at, last_error) \
         VALUES ($1, $2, NULL, NULL, now(), NULL, $3) \
         ON CONFLICT (feed_id) DO UPDATE SET source_url = EXCLUDED.source_url, \
         content_type = CASE WHEN feed_icons.source_url IS DISTINCT FROM EXCLUDED.source_url \
           THEN NULL ELSE feed_icons.content_type END, \
         content = CASE WHEN feed_icons.source_url IS DISTINCT FROM EXCLUDED.source_url \
           THEN NULL ELSE feed_icons.content END, last_attempt_at = now(), \
         fetched_at = CASE WHEN feed_icons.source_url IS DISTINCT FROM EXCLUDED.source_url \
           THEN NULL ELSE feed_icons.fetched_at END, last_error = EXCLUDED.last_error",
    )
    .bind(feed_id)
    .bind(source_url)
    .bind(error)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn get(pool: &PgPool, feed_id: Uuid) -> Result<Option<FeedIcon>> {
    Ok(sqlx::query_as(
        "SELECT content_type, content FROM feed_icons WHERE feed_id = $1 AND content IS NOT NULL",
    )
    .bind(feed_id)
    .fetch_optional(pool)
    .await?)
}

pub async fn available_for_feeds(pool: &PgPool, feed_ids: &[Uuid]) -> Result<HashSet<Uuid>> {
    if feed_ids.is_empty() {
        return Ok(HashSet::new());
    }
    Ok(sqlx::query_scalar(
        "SELECT feed_id FROM feed_icons WHERE feed_id = ANY($1) AND content IS NOT NULL",
    )
    .bind(feed_ids)
    .fetch_all(pool)
    .await?
    .into_iter()
    .collect())
}

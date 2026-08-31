//! Shared YouTube duration cache and bounded enrichment budget.

use sqlx::PgPool;
use time::{Duration, OffsetDateTime};

use crate::Result;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct YouTubeMetadata {
    pub video_id: String,
    pub duration_secs: Option<i32>,
    pub status: String,
    pub error_summary: Option<String>,
    pub attempted_at: Option<OffsetDateTime>,
    pub expires_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

const COLUMNS: &str =
    "video_id, duration_secs, status, error_summary, attempted_at, expires_at, updated_at";

pub async fn list(pool: &PgPool, video_ids: &[String]) -> Result<Vec<YouTubeMetadata>> {
    if video_ids.is_empty() {
        return Ok(Vec::new());
    }
    let sql = format!("SELECT {COLUMNS} FROM youtube_video_metadata WHERE video_id = ANY($1)");
    Ok(sqlx::query_as(&sql).bind(video_ids).fetch_all(pool).await?)
}

pub async fn reserve(pool: &PgPool, video_id: &str) -> Result<()> {
    sqlx::query(
        "INSERT INTO youtube_video_metadata (video_id) VALUES ($1) \
         ON CONFLICT (video_id) DO NOTHING",
    )
    .bind(video_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn attempts_today(pool: &PgPool) -> Result<i64> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM youtube_video_metadata WHERE attempted_at >= date_trunc('day', now())",
    )
    .fetch_one(pool)
    .await?)
}

pub async fn mark_ready(
    pool: &PgPool,
    video_id: &str,
    duration_secs: i32,
    ttl_days: i64,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO youtube_video_metadata \
         (video_id, duration_secs, status, attempted_at, expires_at) \
         VALUES ($1,$2,'ready',now(),$3) \
         ON CONFLICT (video_id) DO UPDATE SET duration_secs = excluded.duration_secs, \
         status = 'ready', error_summary = NULL, attempted_at = now(), \
         expires_at = excluded.expires_at, updated_at = now()",
    )
    .bind(video_id)
    .bind(duration_secs)
    .bind(OffsetDateTime::now_utc() + Duration::days(ttl_days.clamp(1, 365)))
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn mark_failed(pool: &PgPool, video_id: &str, error: &str) -> Result<()> {
    sqlx::query(
        "INSERT INTO youtube_video_metadata \
         (video_id, status, error_summary, attempted_at, expires_at) \
         VALUES ($1,'failed',left($2,300),now(),now() + interval '1 day') \
         ON CONFLICT (video_id) DO UPDATE SET duration_secs = NULL, status = 'failed', \
         error_summary = left(excluded.error_summary,300), attempted_at = now(), \
         expires_at = now() + interval '1 day', updated_at = now()",
    )
    .bind(video_id)
    .bind(error)
    .execute(pool)
    .await?;
    Ok(())
}

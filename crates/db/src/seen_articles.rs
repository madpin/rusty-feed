//! Per-user seen state. Opening an article records a glance without marking it
//! read, so "seen" and "read" stay independent.

use sqlx::PgPool;
use uuid::Uuid;

use crate::error::Result;

/// Record that a user has opened an article (idempotent).
pub async fn mark_seen(pool: &PgPool, user_id: Uuid, article_id: Uuid) -> Result<()> {
    sqlx::query(
        "INSERT INTO seen_articles (id, user_id, article_id) VALUES ($1, $2, $3) \
         ON CONFLICT (user_id, article_id) DO NOTHING",
    )
    .bind(Uuid::now_v7())
    .bind(user_id)
    .bind(article_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Whether the user has opened the article.
pub async fn is_seen(pool: &PgPool, user_id: Uuid, article_id: Uuid) -> Result<bool> {
    let found: Option<i32> =
        sqlx::query_scalar("SELECT 1 FROM seen_articles WHERE user_id = $1 AND article_id = $2")
            .bind(user_id)
            .bind(article_id)
            .fetch_optional(pool)
            .await?;
    Ok(found.is_some())
}

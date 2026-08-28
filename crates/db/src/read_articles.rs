//! Per-user read-state repository.

use sqlx::PgPool;
use uuid::Uuid;

use crate::error::Result;

/// Mark an article read for a user (idempotent).
pub async fn mark_read(pool: &PgPool, user_id: Uuid, article_id: Uuid) -> Result<()> {
    sqlx::query(
        "INSERT INTO read_articles (id, user_id, article_id) VALUES ($1, $2, $3) \
         ON CONFLICT (user_id, article_id) DO NOTHING",
    )
    .bind(Uuid::now_v7())
    .bind(user_id)
    .bind(article_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Mark an article unread for a user.
pub async fn mark_unread(pool: &PgPool, user_id: Uuid, article_id: Uuid) -> Result<()> {
    sqlx::query("DELETE FROM read_articles WHERE user_id = $1 AND article_id = $2")
        .bind(user_id)
        .bind(article_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Whether an article is read by a user.
pub async fn is_read(pool: &PgPool, user_id: Uuid, article_id: Uuid) -> Result<bool> {
    let found: Option<i32> =
        sqlx::query_scalar("SELECT 1 FROM read_articles WHERE user_id = $1 AND article_id = $2")
            .bind(user_id)
            .bind(article_id)
            .fetch_optional(pool)
            .await?;
    Ok(found.is_some())
}

/// Mark every article in the user's subscribed feeds as read. Returns the number
/// newly marked.
pub async fn mark_all_read(pool: &PgPool, user_id: Uuid) -> Result<u64> {
    let res = sqlx::query(
        "INSERT INTO read_articles (id, user_id, article_id) \
         SELECT gen_random_uuid(), $1, a.id \
         FROM articles a \
         JOIN user_feeds uf ON uf.feed_id = a.feed_id AND uf.user_id = $1 \
         ON CONFLICT (user_id, article_id) DO NOTHING",
    )
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(res.rows_affected())
}

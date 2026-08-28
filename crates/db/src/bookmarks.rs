//! Per-user bookmark (favorite) repository.

use sqlx::PgPool;
use uuid::Uuid;

use crate::error::Result;

/// Bookmark an article for a user (idempotent).
pub async fn add(pool: &PgPool, user_id: Uuid, article_id: Uuid) -> Result<()> {
    sqlx::query(
        "INSERT INTO bookmarks (id, user_id, article_id) VALUES ($1, $2, $3) \
         ON CONFLICT (user_id, article_id) DO NOTHING",
    )
    .bind(Uuid::now_v7())
    .bind(user_id)
    .bind(article_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Remove a bookmark.
pub async fn remove(pool: &PgPool, user_id: Uuid, article_id: Uuid) -> Result<()> {
    sqlx::query("DELETE FROM bookmarks WHERE user_id = $1 AND article_id = $2")
        .bind(user_id)
        .bind(article_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Whether an article is bookmarked by a user.
pub async fn is_bookmarked(pool: &PgPool, user_id: Uuid, article_id: Uuid) -> Result<bool> {
    let found: Option<i32> =
        sqlx::query_scalar("SELECT 1 FROM bookmarks WHERE user_id = $1 AND article_id = $2")
            .bind(user_id)
            .bind(article_id)
            .fetch_optional(pool)
            .await?;
    Ok(found.is_some())
}

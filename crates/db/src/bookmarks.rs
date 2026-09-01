//! Per-user bookmark (favorite) repository.

use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::error::Result;

/// One bookmarked article, for the GDPR data export (§9).
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct BookmarkExport {
    pub url: String,
    pub title: String,
    pub bookmarked_at: OffsetDateTime,
}

/// A user's bookmarked articles (url + title + when), newest first, capped.
pub async fn list_for_export(
    pool: &PgPool,
    user_id: Uuid,
    limit: i64,
) -> Result<Vec<BookmarkExport>> {
    let limit = limit.clamp(1, 100_000);
    Ok(sqlx::query_as::<_, BookmarkExport>(
        "SELECT a.url AS url, a.title AS title, b.created_at AS bookmarked_at \
         FROM bookmarks b JOIN articles a ON a.id = b.article_id \
         WHERE b.user_id = $1 ORDER BY b.created_at DESC LIMIT $2",
    )
    .bind(user_id)
    .bind(limit)
    .fetch_all(pool)
    .await?)
}

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

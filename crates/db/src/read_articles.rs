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

/// Mark articles in the selected personal Library scope as read. Returns the number
/// newly marked.
pub async fn mark_all_read(
    pool: &PgPool,
    user_id: Uuid,
    feed_id: Option<Uuid>,
    category_id: Option<Uuid>,
    unfiled: bool,
    tag: Option<&str>,
) -> Result<u64> {
    let res = sqlx::query(
        "WITH RECURSIVE category_scope(id) AS ( \
             SELECT id FROM user_categories WHERE id = $3::uuid AND user_id = $1 \
             UNION \
             SELECT child.id FROM user_categories child \
             JOIN category_scope scope ON child.parent_id = scope.id \
             WHERE child.user_id = $1 \
         ) \
         INSERT INTO read_articles (id, user_id, article_id) \
         SELECT gen_random_uuid(), $1, a.id \
         FROM articles a \
         JOIN user_feeds uf ON uf.feed_id = a.feed_id AND uf.user_id = $1 \
         WHERE ($2::uuid IS NULL OR a.feed_id = $2) \
           AND ($3::uuid IS NULL OR EXISTS (SELECT 1 FROM user_feed_categories ufc \
                JOIN category_scope scope ON scope.id = ufc.user_category_id \
                WHERE ufc.user_feed_id = uf.id)) \
           AND (NOT $4 OR NOT EXISTS (SELECT 1 FROM user_feed_categories ufc \
                WHERE ufc.user_feed_id = uf.id)) \
           AND ($5::text IS NULL OR uf.tags @> ARRAY[$5]::text[]) \
         ON CONFLICT (user_id, article_id) DO NOTHING",
    )
    .bind(user_id)
    .bind(feed_id)
    .bind(category_id)
    .bind(unfiled)
    .bind(tag)
    .execute(pool)
    .await?;
    Ok(res.rows_affected())
}

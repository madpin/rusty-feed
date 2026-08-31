//! Per-user public article-share authorization.
//!
//! Rows reference shared articles and contain no article body, preserving the
//! system-wide content deduplication invariant.

use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::error::Result;
use crate::models::{PublicArticleShare, PUBLIC_ARTICLE_SHARE_COLUMNS};

/// Validated mutable fields for creating or rotating a share.
pub struct PublicShareDraft<'a> {
    pub token_hash: &'a str,
    pub token_ciphertext: &'a str,
    pub slug: &'a str,
    pub content_source: &'a str,
    pub show_title: bool,
    pub show_metadata: bool,
    pub show_source_link: bool,
    pub include_images: bool,
    pub expires_at: Option<OffsetDateTime>,
}

/// Create or rotate the one share owned by a user for an article.
pub async fn upsert(
    pool: &PgPool,
    user_id: Uuid,
    article_id: Uuid,
    draft: PublicShareDraft<'_>,
) -> Result<PublicArticleShare> {
    let mut transaction = pool.begin().await?;
    let sql = format!(
        "INSERT INTO public_article_shares (id, user_id, article_id, token_hash, \
         token_ciphertext, slug, content_source, show_title, show_metadata, show_source_link, \
         include_images, expires_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12) \
         ON CONFLICT (user_id, article_id) DO UPDATE SET token_hash = EXCLUDED.token_hash, \
         token_ciphertext = EXCLUDED.token_ciphertext, slug = EXCLUDED.slug, \
         content_source = EXCLUDED.content_source, show_title = EXCLUDED.show_title, \
         show_metadata = EXCLUDED.show_metadata, show_source_link = EXCLUDED.show_source_link, \
         include_images = EXCLUDED.include_images, expires_at = EXCLUDED.expires_at, \
         revoked_at = NULL, created_at = now(), updated_at = now() \
         RETURNING {PUBLIC_ARTICLE_SHARE_COLUMNS}"
    );
    let share = sqlx::query_as::<_, PublicArticleShare>(&sql)
        .bind(Uuid::now_v7())
        .bind(user_id)
        .bind(article_id)
        .bind(draft.token_hash)
        .bind(draft.token_ciphertext)
        .bind(draft.slug)
        .bind(draft.content_source)
        .bind(draft.show_title)
        .bind(draft.show_metadata)
        .bind(draft.show_source_link)
        .bind(draft.include_images)
        .bind(draft.expires_at)
        .fetch_one(&mut *transaction)
        .await?;
    sqlx::query(
        "INSERT INTO public_article_share_events (id, share_id, user_id, article_id, action, \
         content_source, expires_at, show_title, show_metadata, show_source_link, include_images) \
         VALUES ($1, $2, $3, $4, 'updated', $5, $6, $7, $8, $9, $10)",
    )
    .bind(Uuid::now_v7())
    .bind(share.id)
    .bind(user_id)
    .bind(article_id)
    .bind(draft.content_source)
    .bind(draft.expires_at)
    .bind(draft.show_title)
    .bind(draft.show_metadata)
    .bind(draft.show_source_link)
    .bind(draft.include_images)
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(share)
}

/// Load the share configuration owned by one user for one article.
pub async fn find_for_user(
    pool: &PgPool,
    user_id: Uuid,
    article_id: Uuid,
) -> Result<Option<PublicArticleShare>> {
    let sql = format!(
        "SELECT {PUBLIC_ARTICLE_SHARE_COLUMNS} FROM public_article_shares \
         WHERE user_id = $1 AND article_id = $2"
    );
    Ok(sqlx::query_as::<_, PublicArticleShare>(&sql)
        .bind(user_id)
        .bind(article_id)
        .fetch_optional(pool)
        .await?)
}

/// Resolve a non-revoked, non-expired public share by its token hash.
pub async fn find_active_by_hash(
    pool: &PgPool,
    token_hash: &str,
) -> Result<Option<PublicArticleShare>> {
    let sql = format!(
        "SELECT {PUBLIC_ARTICLE_SHARE_COLUMNS} FROM public_article_shares \
         WHERE token_hash = $1 AND revoked_at IS NULL \
           AND (expires_at IS NULL OR expires_at > now())"
    );
    Ok(sqlx::query_as::<_, PublicArticleShare>(&sql)
        .bind(token_hash)
        .fetch_optional(pool)
        .await?)
}

/// Revoke the caller-owned share. Returns whether an active row changed.
pub async fn revoke(pool: &PgPool, user_id: Uuid, article_id: Uuid) -> Result<bool> {
    let mut transaction = pool.begin().await?;
    let share_id: Option<Uuid> = sqlx::query_scalar(
        "UPDATE public_article_shares SET revoked_at = now(), updated_at = now() \
         WHERE user_id = $1 AND article_id = $2 AND revoked_at IS NULL RETURNING id",
    )
    .bind(user_id)
    .bind(article_id)
    .fetch_optional(&mut *transaction)
    .await?;
    if let Some(share_id) = share_id {
        sqlx::query(
            "INSERT INTO public_article_share_events \
             (id, share_id, user_id, article_id, action) VALUES ($1, $2, $3, $4, 'revoked')",
        )
        .bind(Uuid::now_v7())
        .bind(share_id)
        .bind(user_id)
        .bind(article_id)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(true)
    } else {
        transaction.rollback().await?;
        Ok(false)
    }
}

/// Safe administrator projection of a public-share policy change.
#[derive(Debug, sqlx::FromRow)]
pub struct AdminShareEvent {
    pub action: String,
    pub user_email: String,
    pub article_title: String,
    pub content_source: Option<String>,
    pub expires_at: Option<OffsetDateTime>,
    pub include_images: Option<bool>,
    pub created_at: OffsetDateTime,
}

/// Count links that are usable at the current time.
pub async fn active_count(pool: &PgPool) -> Result<i64> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM public_article_shares WHERE revoked_at IS NULL \
         AND (expires_at IS NULL OR expires_at > now())",
    )
    .fetch_one(pool)
    .await?)
}

/// Count one user's active shares (for quota enforcement, FND-188).
pub async fn count_for_user(pool: &PgPool, user_id: Uuid) -> Result<i64> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM public_article_shares \
         WHERE user_id = $1 AND revoked_at IS NULL \
         AND (expires_at IS NULL OR expires_at > now())",
    )
    .bind(user_id)
    .fetch_one(pool)
    .await?)
}

/// Return recent token-free policy events for the administration dashboard.
pub async fn recent_events(pool: &PgPool, limit: i64) -> Result<Vec<AdminShareEvent>> {
    Ok(sqlx::query_as(
        "SELECT event.action, users.email AS user_email, articles.title AS article_title, \
         event.content_source, event.expires_at, event.include_images, event.created_at \
         FROM public_article_share_events event \
         JOIN users ON users.id = event.user_id \
         JOIN articles ON articles.id = event.article_id \
         ORDER BY event.created_at DESC LIMIT $1",
    )
    .bind(limit.clamp(1, 100))
    .fetch_all(pool)
    .await?)
}

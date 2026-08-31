//! Saved-search match rows + result listing (M11).

use serde_json::Value as Json;
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::error::Result;

/// A match joined with its article, for the results UI.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct MatchedArticle {
    pub id: Uuid,
    pub title: String,
    pub url: String,
    pub excerpt: Option<String>,
    pub feed_name: String,
    pub published_at: Option<OffsetDateTime>,
    pub relevance_score: f64,
}

/// Upsert a match; returns whether it was newly created.
pub async fn upsert(
    pool: &PgPool,
    saved_search_id: Uuid,
    article_id: Uuid,
    score: f64,
    matched_terms: &Json,
    reason: &str,
) -> Result<bool> {
    let created: bool = sqlx::query_scalar(
        "INSERT INTO saved_search_matches \
           (id, saved_search_id, article_id, relevance_score, matched_terms, match_reason) \
         VALUES ($1, $2, $3, $4, $5, $6) \
         ON CONFLICT (saved_search_id, article_id) DO UPDATE SET \
           relevance_score = EXCLUDED.relevance_score, matched_terms = EXCLUDED.matched_terms, \
           match_reason = EXCLUDED.match_reason \
         RETURNING (xmax = 0) AS created",
    )
    .bind(Uuid::now_v7())
    .bind(saved_search_id)
    .bind(article_id)
    .bind(score)
    .bind(matched_terms)
    .bind(reason)
    .fetch_one(pool)
    .await?;
    Ok(created)
}

/// Atomically mark a match notified; returns `true` the first time only.
pub async fn claim_notify(pool: &PgPool, saved_search_id: Uuid, article_id: Uuid) -> Result<bool> {
    let updated: Option<Uuid> = sqlx::query_scalar(
        "UPDATE saved_search_matches SET notified = true \
         WHERE saved_search_id = $1 AND article_id = $2 AND NOT notified RETURNING id",
    )
    .bind(saved_search_id)
    .bind(article_id)
    .fetch_optional(pool)
    .await?;
    Ok(updated.is_some())
}

/// Matches for a saved search, highest score first.
pub async fn list_for_search(
    pool: &PgPool,
    saved_search_id: Uuid,
    limit: i64,
    offset: i64,
) -> Result<Vec<MatchedArticle>> {
    Ok(sqlx::query_as::<_, MatchedArticle>(
        "SELECT a.id, a.title, a.url, a.excerpt, f.name AS feed_name, a.published_at, \
                m.relevance_score \
         FROM saved_search_matches m \
         JOIN articles a ON a.id = m.article_id \
         JOIN feeds f ON f.id = a.feed_id \
         WHERE m.saved_search_id = $1 \
         ORDER BY m.relevance_score DESC LIMIT $2 OFFSET $3",
    )
    .bind(saved_search_id)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?)
}

/// Clear matches for an explicit full rematch. The parent ownership check is
/// performed by the caller before this shared helper is invoked.
pub async fn clear_for_search(pool: &PgPool, saved_search_id: Uuid) -> Result<u64> {
    Ok(
        sqlx::query("DELETE FROM saved_search_matches WHERE saved_search_id = $1")
            .bind(saved_search_id)
            .execute(pool)
            .await?
            .rows_affected(),
    )
}

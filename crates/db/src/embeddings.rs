//! Vector store for article embeddings (M9). pgvector isn't natively supported
//! by sqlx, so vectors are bound as text and cast with `::vector`. Queries are
//! **model-guarded** (only vectors from the same model are compared) to avoid
//! mixing embedding spaces (FND-002), and **user-scoped** to subscribed feeds.

use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::error::Result;

/// A semantic search / related hit.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct SearchResult {
    pub id: Uuid,
    pub title: String,
    pub url: String,
    pub excerpt: Option<String>,
    pub image_url: Option<String>,
    pub published_at: Option<OffsetDateTime>,
    pub feed_name: String,
    pub similarity: f64,
}

/// Format a vector as a pgvector literal: `[0.1,0.2,...]`.
fn to_pg_vector(v: &[f32]) -> String {
    let mut s = String::with_capacity(v.len() * 8 + 2);
    s.push('[');
    for (i, x) in v.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&x.to_string());
    }
    s.push(']');
    s
}

/// Store an article's embedding (once per article; ignores duplicates).
pub async fn upsert(pool: &PgPool, article_id: Uuid, embedding: &[f32], model: &str) -> Result<()> {
    sqlx::query(
        "INSERT INTO article_embeddings (article_id, embedding, model) \
         VALUES ($1, $2::vector, $3) ON CONFLICT (article_id) DO NOTHING",
    )
    .bind(article_id)
    .bind(to_pg_vector(embedding))
    .bind(model)
    .execute(pool)
    .await?;
    Ok(())
}

/// Whether an article already has an embedding.
pub async fn exists(pool: &PgPool, article_id: Uuid) -> Result<bool> {
    let found: Option<i32> =
        sqlx::query_scalar("SELECT 1 FROM article_embeddings WHERE article_id = $1")
            .bind(article_id)
            .fetch_optional(pool)
            .await?;
    Ok(found.is_some())
}

/// KNN semantic search within the user's subscribed feeds, comparing only
/// vectors from `model`. Returns hits ordered by descending similarity.
pub async fn search_for_user(
    pool: &PgPool,
    user_id: Uuid,
    query: &[f32],
    model: &str,
    limit: i64,
) -> Result<Vec<SearchResult>> {
    let sql = "SELECT a.id, a.title, a.url, a.excerpt, a.image_url, a.published_at, \
                      f.name AS feed_name, \
                      1 - (e.embedding <=> $2::vector) AS similarity \
               FROM article_embeddings e \
               JOIN articles a ON a.id = e.article_id \
               JOIN feeds f ON f.id = a.feed_id \
               JOIN user_feeds uf ON uf.feed_id = a.feed_id AND uf.user_id = $1 \
               WHERE e.model = $3 \
               ORDER BY e.embedding <=> $2::vector \
               LIMIT $4";
    Ok(sqlx::query_as::<_, SearchResult>(sql)
        .bind(user_id)
        .bind(to_pg_vector(query))
        .bind(model)
        .bind(limit)
        .fetch_all(pool)
        .await?)
}

/// Articles similar to `article_id` (using its stored embedding), within the
/// user's subscribed feeds and the same model.
pub async fn related_for_user(
    pool: &PgPool,
    user_id: Uuid,
    article_id: Uuid,
    model: &str,
    limit: i64,
) -> Result<Vec<SearchResult>> {
    let sql = "SELECT a.id, a.title, a.url, a.excerpt, a.image_url, a.published_at, \
                      f.name AS feed_name, \
                      1 - (e.embedding <=> q.embedding) AS similarity \
               FROM article_embeddings e \
               JOIN articles a ON a.id = e.article_id \
               JOIN feeds f ON f.id = a.feed_id \
               JOIN user_feeds uf ON uf.feed_id = a.feed_id AND uf.user_id = $1 \
               CROSS JOIN (SELECT embedding FROM article_embeddings WHERE article_id = $2) q \
               WHERE e.model = $3 AND e.article_id <> $2 \
               ORDER BY e.embedding <=> q.embedding \
               LIMIT $4";
    Ok(sqlx::query_as::<_, SearchResult>(sql)
        .bind(user_id)
        .bind(article_id)
        .bind(model)
        .bind(limit)
        .fetch_all(pool)
        .await?)
}

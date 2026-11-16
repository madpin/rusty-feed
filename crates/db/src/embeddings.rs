//! Vector store for article embeddings (M9). pgvector isn't natively supported
//! by sqlx, so vectors are bound as text and cast with `::vector`. Queries are
//! **model-guarded** (only vectors from the same model are compared) to avoid
//! mixing embedding spaces (FND-002), and **user-scoped** to subscribed feeds.

use sqlx::{PgPool, Postgres, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::error::Result;

/// Begin the transaction one per-user vector query runs in (FND-232).
///
/// An HNSW index scan yields only `hnsw.ef_search` nearest tuples from the
/// whole shared corpus, and the `user_feeds` join filters them afterwards, so
/// a reader who follows a small share of the corpus could get fewer rows than
/// the limit, or none. `hnsw.iterative_scan = relaxed_order` keeps scanning
/// (up to `hnsw.max_scan_tuples`) until the filtered query is satisfied. The
/// order can be slightly relaxed, so every caller re-sorts on the exact
/// distance or score; a re-sort of a materialized CTE uses `distance + 0`
/// because PostgreSQL 17 would otherwise trust the CTE's `ORDER BY` and skip
/// the outer sort. The setting is transaction-local (`SET LOCAL`), so it never
/// leaks to other users of a pooled connection.
pub async fn begin_vector_scan(pool: &PgPool) -> Result<Transaction<'static, Postgres>> {
    let mut transaction = pool.begin().await?;
    sqlx::query("SELECT set_config('hnsw.iterative_scan', 'relaxed_order', true)")
        .execute(&mut *transaction)
        .await?;
    Ok(transaction)
}

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

/// Store an article's embedding for an effective content revision.
pub async fn upsert(
    pool: &PgPool,
    article_id: Uuid,
    embedding: &[f32],
    model: &str,
    content_revision: i64,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO article_embeddings (article_id, embedding, model, content_revision) \
         VALUES ($1, $2::vector, $3, $4) \
         ON CONFLICT (article_id) DO UPDATE SET \
           embedding = EXCLUDED.embedding, model = EXCLUDED.model, \
           content_revision = EXCLUDED.content_revision \
         WHERE article_embeddings.content_revision <= EXCLUDED.content_revision",
    )
    .bind(article_id)
    .bind(to_pg_vector(embedding))
    .bind(model)
    .bind(content_revision)
    .execute(pool)
    .await?;
    Ok(())
}

/// Whether an article already has an embedding for the requested model/revision.
pub async fn exists(
    pool: &PgPool,
    article_id: Uuid,
    model: &str,
    content_revision: i64,
) -> Result<bool> {
    let found: Option<i32> = sqlx::query_scalar(
        "SELECT 1 FROM article_embeddings \
         WHERE article_id = $1 AND model = $2 AND content_revision = $3",
    )
    .bind(article_id)
    .bind(model)
    .bind(content_revision)
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
    let sql = "WITH nearest AS MATERIALIZED ( \
                 SELECT a.id, a.title, a.url, a.excerpt, a.image_url, a.published_at, \
                        f.name AS feed_name, \
                        1 - ((e.embedding <=> $2::vector) / 2) AS similarity, \
                        e.embedding <=> $2::vector AS distance \
                 FROM article_embeddings e \
                 JOIN articles a ON a.id = e.article_id \
                 JOIN feeds f ON f.id = a.feed_id \
                 JOIN user_feeds uf ON uf.feed_id = a.feed_id AND uf.user_id = $1 \
                 WHERE e.model = $3 AND e.content_revision = a.content_revision \
                 ORDER BY e.embedding <=> $2::vector \
                 LIMIT $4 \
               ) \
               SELECT * FROM nearest ORDER BY distance + 0";
    let mut transaction = begin_vector_scan(pool).await?;
    let hits = sqlx::query_as::<_, SearchResult>(sql)
        .bind(user_id)
        .bind(to_pg_vector(query))
        .bind(model)
        .bind(limit)
        .fetch_all(&mut *transaction)
        .await?;
    transaction.commit().await?;
    Ok(hits)
}

/// A candidate article for saved-search matching: content + semantic similarity.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Candidate {
    pub id: Uuid,
    pub title: String,
    pub content: String,
    pub similarity: f64,
}

/// Recent embedded articles in the user's feeds, ranked by similarity to a query
/// vector (for saved-search matching). `since_days` bounds recency.
pub async fn candidates_for_query(
    pool: &PgPool,
    user_id: Uuid,
    query: &[f32],
    model: &str,
    since_days: i64,
    limit: i64,
) -> Result<Vec<Candidate>> {
    let sql = "WITH nearest AS MATERIALIZED ( \
                 SELECT a.id, a.title, a.content, \
                        1 - ((e.embedding <=> $2::vector) / 2) AS similarity, \
                        e.embedding <=> $2::vector AS distance \
                 FROM article_embeddings e \
                 JOIN articles a ON a.id = e.article_id \
                 JOIN user_feeds uf ON uf.feed_id = a.feed_id AND uf.user_id = $1 \
                 WHERE e.model = $3 AND e.content_revision = a.content_revision \
                   AND a.created_at > now() - make_interval(days => $4::int) \
                 ORDER BY e.embedding <=> $2::vector \
                 LIMIT $5 \
               ) \
               SELECT * FROM nearest ORDER BY distance + 0";
    let mut transaction = begin_vector_scan(pool).await?;
    let candidates = sqlx::query_as::<_, Candidate>(sql)
        .bind(user_id)
        .bind(to_pg_vector(query))
        .bind(model)
        .bind(since_days)
        .bind(limit)
        .fetch_all(&mut *transaction)
        .await?;
    transaction.commit().await?;
    Ok(candidates)
}

/// The embeddings-disabled counterpart to [`candidates_for_query`]: the same
/// recent, per-user candidate window, but ranked by recency instead of vector
/// similarity (`similarity` is always `0.0`) since there is no query vector to
/// compare against. Lets saved-search matching keep working, keyword-only,
/// on an instance with `EMBEDDINGS_ENABLED=false`.
pub async fn recent_candidates_for_user(
    pool: &PgPool,
    user_id: Uuid,
    since_days: i64,
    limit: i64,
) -> Result<Vec<Candidate>> {
    let sql = "SELECT a.id, a.title, a.content, 0.0::float8 AS similarity \
               FROM articles a \
               JOIN user_feeds uf ON uf.feed_id = a.feed_id AND uf.user_id = $1 \
               WHERE a.created_at > now() - make_interval(days => $2::int) \
               ORDER BY a.created_at DESC \
               LIMIT $3";
    Ok(sqlx::query_as::<_, Candidate>(sql)
        .bind(user_id)
        .bind(since_days)
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
    let sql = "WITH nearest AS MATERIALIZED ( \
                 SELECT a.id, a.title, a.url, a.excerpt, a.image_url, a.published_at, \
                        f.name AS feed_name, \
                        1 - ((e.embedding <=> q.embedding) / 2) AS similarity, \
                        e.embedding <=> q.embedding AS distance \
                 FROM article_embeddings e \
                 JOIN articles a ON a.id = e.article_id \
                 JOIN feeds f ON f.id = a.feed_id \
                 JOIN user_feeds uf ON uf.feed_id = a.feed_id AND uf.user_id = $1 \
                 CROSS JOIN (SELECT e0.embedding FROM article_embeddings e0 \
                             JOIN articles a0 ON a0.id = e0.article_id \
                             WHERE e0.article_id = $2 AND e0.model = $3 \
                               AND e0.content_revision = a0.content_revision) q \
                 WHERE e.model = $3 AND e.content_revision = a.content_revision AND e.article_id <> $2 \
                 ORDER BY e.embedding <=> q.embedding \
                 LIMIT $4 \
               ) \
               SELECT * FROM nearest ORDER BY distance + 0";
    let mut transaction = begin_vector_scan(pool).await?;
    let hits = sqlx::query_as::<_, SearchResult>(sql)
        .bind(user_id)
        .bind(article_id)
        .bind(model)
        .bind(limit)
        .fetch_all(&mut *transaction)
        .await?;
    transaction.commit().await?;
    Ok(hits)
}

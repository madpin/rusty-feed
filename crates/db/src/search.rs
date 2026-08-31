//! Indexed PostgreSQL keyword and hybrid search across a user's subscriptions.
//!
//! The SQL chooses between two migration-owned `tsvector` columns. Callers
//! cannot provide arbitrary PostgreSQL configuration or column names.

use rustss_core::config::SearchTextConfig;
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::error::Result;

/// One ranked interactive-search result with its independently explainable
/// keyword and semantic components.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct SearchHit {
    pub id: Uuid,
    pub title: String,
    pub url: String,
    pub excerpt: Option<String>,
    pub image_url: Option<String>,
    pub published_at: Option<OffsetDateTime>,
    pub feed_name: String,
    pub semantic_score: Option<f64>,
    pub keyword_score: Option<f64>,
    pub final_score: f64,
}

/// Inputs for a hybrid query. Grouping these keeps the repository call explicit
/// as ranking controls evolve.
pub struct HybridSearch<'a> {
    pub user_id: Uuid,
    pub query: &'a str,
    pub query_vector: &'a [f32],
    pub model: &'a str,
    pub text_config: SearchTextConfig,
    pub semantic_weight: f64,
    pub limit: i64,
}

fn text_search_spec(config: SearchTextConfig) -> (&'static str, &'static str) {
    match config {
        SearchTextConfig::Simple => ("search_vector_simple", "simple"),
        SearchTextConfig::English => ("search_vector", "english"),
    }
}

fn to_pg_vector(vector: &[f32]) -> String {
    let mut value = String::with_capacity(vector.len() * 8 + 2);
    value.push('[');
    for (index, component) in vector.iter().enumerate() {
        if index > 0 {
            value.push(',');
        }
        value.push_str(&component.to_string());
    }
    value.push(']');
    value
}

/// Search only with PostgreSQL `websearch_to_tsquery` syntax. Quoted phrases,
/// `OR`, and `-excluded` terms are supported without constructing SQL from the
/// user's query.
pub async fn keyword_for_user(
    pool: &PgPool,
    user_id: Uuid,
    query: &str,
    config: SearchTextConfig,
    limit: i64,
) -> Result<Vec<SearchHit>> {
    let (column, dictionary) = text_search_spec(config);
    let sql = format!(
        "WITH q AS (SELECT websearch_to_tsquery('{dictionary}', $2) AS value), \
         ranked AS ( \
           SELECT a.id, ts_rank_cd(a.{column}, q.value, 32)::float8 AS keyword_score \
           FROM articles a \
           JOIN user_feeds uf ON uf.feed_id = a.feed_id AND uf.user_id = $1 \
           CROSS JOIN q \
           WHERE a.{column} @@ q.value \
           ORDER BY keyword_score DESC, a.published_at DESC NULLS LAST, a.id \
           LIMIT $3 \
         ) \
         SELECT a.id, a.title, a.url, a.excerpt, a.image_url, a.published_at, \
                COALESCE(uf.custom_name, f.name) AS feed_name, \
                NULL::float8 AS semantic_score, ranked.keyword_score, \
                ranked.keyword_score AS final_score \
         FROM ranked \
         JOIN articles a ON a.id = ranked.id \
         JOIN feeds f ON f.id = a.feed_id \
         JOIN user_feeds uf ON uf.feed_id = a.feed_id AND uf.user_id = $1 \
         ORDER BY final_score DESC, a.published_at DESC NULLS LAST, a.id"
    );
    Ok(sqlx::query_as::<_, SearchHit>(&sql)
        .bind(user_id)
        .bind(query)
        .bind(limit.clamp(1, 100))
        .fetch_all(pool)
        .await?)
}

/// Blend bounded keyword relevance with cosine similarity. Keyword-only rows
/// remain eligible, so missing/stale embeddings do not make exact matches
/// disappear. Semantic candidates stay guarded by model and content revision.
pub async fn hybrid_for_user(pool: &PgPool, request: &HybridSearch<'_>) -> Result<Vec<SearchHit>> {
    let (column, dictionary) = text_search_spec(request.text_config);
    let result_limit = request.limit.clamp(1, 100);
    let candidate_limit = result_limit.saturating_mul(4).clamp(result_limit, 400);
    let semantic_weight = request.semantic_weight.clamp(0.0, 1.0);
    let sql = format!(
        "WITH q AS (SELECT websearch_to_tsquery('{dictionary}', $2) AS value), \
         keyword AS ( \
           SELECT a.id, ts_rank_cd(a.{column}, q.value, 32)::float8 AS keyword_score \
           FROM articles a \
           JOIN user_feeds uf ON uf.feed_id = a.feed_id AND uf.user_id = $1 \
           CROSS JOIN q \
           WHERE a.{column} @@ q.value \
           ORDER BY keyword_score DESC, a.published_at DESC NULLS LAST, a.id \
           LIMIT $5 \
         ), \
         semantic AS ( \
           SELECT e.article_id AS id, \
                  GREATEST(0::float8, LEAST(1::float8, \
                    1 - ((e.embedding <=> $3::vector) / 2))) AS semantic_score \
           FROM article_embeddings e \
           JOIN articles a ON a.id = e.article_id \
           JOIN user_feeds uf ON uf.feed_id = a.feed_id AND uf.user_id = $1 \
           WHERE e.model = $4 AND e.content_revision = a.content_revision \
           ORDER BY e.embedding <=> $3::vector \
           LIMIT $5 \
         ), \
         candidates AS (SELECT id FROM keyword UNION SELECT id FROM semantic) \
         SELECT a.id, a.title, a.url, a.excerpt, a.image_url, a.published_at, \
                COALESCE(uf.custom_name, f.name) AS feed_name, \
                semantic.semantic_score, keyword.keyword_score, \
                ($6::float8 * COALESCE(semantic.semantic_score, 0) + \
                 (1 - $6::float8) * COALESCE(keyword.keyword_score, 0)) AS final_score \
         FROM candidates \
         JOIN articles a ON a.id = candidates.id \
         JOIN feeds f ON f.id = a.feed_id \
         JOIN user_feeds uf ON uf.feed_id = a.feed_id AND uf.user_id = $1 \
         LEFT JOIN keyword ON keyword.id = candidates.id \
         LEFT JOIN semantic ON semantic.id = candidates.id \
         ORDER BY final_score DESC, a.published_at DESC NULLS LAST, a.id \
         LIMIT $7"
    );
    Ok(sqlx::query_as::<_, SearchHit>(&sql)
        .bind(request.user_id)
        .bind(request.query)
        .bind(to_pg_vector(request.query_vector))
        .bind(request.model)
        .bind(candidate_limit)
        .bind(semantic_weight)
        .bind(result_limit)
        .fetch_all(pool)
        .await?)
}

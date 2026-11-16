//! Indexed PostgreSQL keyword and hybrid search across a user's subscriptions.
//!
//! The SQL chooses between two migration-owned `tsvector` columns. Callers
//! cannot provide arbitrary PostgreSQL configuration or column names.

use rusty_feed_core::config::SearchTextConfig;
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::embeddings::begin_vector_scan;
use crate::error::Result;

/// How many of the newest keyword matches are relevance-ranked (FND-231).
/// Ranking reads each candidate's whole `tsvector`, so a query matching most of
/// the corpus (a stopword under `simple`) would otherwise rank every article.
/// Queries with at most this many matches are ranked exactly as before; above
/// it, only the newest matches are ranked and the page reports `truncated`.
pub const KEYWORD_CANDIDATE_CAP: i64 = 5_000;

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

/// Ranked hits plus whether the keyword candidate set hit the cap, meaning
/// older matches beyond [`KEYWORD_CANDIDATE_CAP`] were not ranked.
#[derive(Debug, Clone)]
pub struct SearchPage {
    pub hits: Vec<SearchHit>,
    pub truncated: bool,
}

#[derive(sqlx::FromRow)]
struct PagedHit {
    #[sqlx(flatten)]
    hit: SearchHit,
    truncated: bool,
}

impl SearchPage {
    fn from_rows(rows: Vec<PagedHit>) -> Self {
        let truncated = rows.iter().any(|row| row.truncated);
        Self {
            hits: rows.into_iter().map(|row| row.hit).collect(),
            truncated,
        }
    }
}

fn candidate_cap(cap: i64) -> i64 {
    cap.clamp(1, i64::MAX - 1)
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
    Ok(keyword_page_for_user(pool, user_id, query, config, limit)
        .await?
        .hits)
}

/// [`keyword_for_user`] plus whether the match set exceeded
/// [`KEYWORD_CANDIDATE_CAP`] so the caller can suggest a narrower query.
pub async fn keyword_page_for_user(
    pool: &PgPool,
    user_id: Uuid,
    query: &str,
    config: SearchTextConfig,
    limit: i64,
) -> Result<SearchPage> {
    keyword_page_with_cap(pool, user_id, query, config, limit, KEYWORD_CANDIDATE_CAP).await
}

/// Keyword search that relevance-ranks only the newest `cap` matches. The
/// candidate set is chosen by recency alone (no time window), so old articles
/// stay findable whenever a query has at most `cap` matches. Exposed for tests
/// and tuning; product code uses [`keyword_page_for_user`].
pub async fn keyword_page_with_cap(
    pool: &PgPool,
    user_id: Uuid,
    query: &str,
    config: SearchTextConfig,
    limit: i64,
    cap: i64,
) -> Result<SearchPage> {
    let (column, dictionary) = text_search_spec(config);
    // `q` is referenced twice; NOT MATERIALIZED keeps the tsquery a planner
    // constant so the GIN index stays usable. The candidates carry the
    // tsvector so ranking does not re-read the article rows.
    let sql = format!(
        "WITH q AS NOT MATERIALIZED (SELECT websearch_to_tsquery('{dictionary}', $2) AS value), \
         recent AS MATERIALIZED ( \
           SELECT a.id, a.published_at, a.{column} AS document \
           FROM articles a \
           JOIN user_feeds uf ON uf.feed_id = a.feed_id AND uf.user_id = $1 \
           CROSS JOIN q \
           WHERE a.{column} @@ q.value \
           ORDER BY a.published_at DESC NULLS LAST, a.id \
           LIMIT $4::int8 + 1 \
         ), \
         ranked AS ( \
           SELECT bounded.id, ts_rank_cd(bounded.document, q.value, 32)::float8 AS keyword_score \
           FROM (SELECT * FROM recent ORDER BY published_at DESC NULLS LAST, id LIMIT $4) bounded \
           CROSS JOIN q \
           ORDER BY keyword_score DESC, bounded.published_at DESC NULLS LAST, bounded.id \
           LIMIT $3 \
         ) \
         SELECT a.id, a.title, a.url, a.excerpt, a.image_url, a.published_at, \
                COALESCE(uf.custom_name, f.name) AS feed_name, \
                NULL::float8 AS semantic_score, ranked.keyword_score, \
                ranked.keyword_score AS final_score, \
                (SELECT count(*) FROM recent) > $4 AS truncated \
         FROM ranked \
         JOIN articles a ON a.id = ranked.id \
         JOIN feeds f ON f.id = a.feed_id \
         JOIN user_feeds uf ON uf.feed_id = a.feed_id AND uf.user_id = $1 \
         ORDER BY final_score DESC, a.published_at DESC NULLS LAST, a.id"
    );
    let rows = sqlx::query_as::<_, PagedHit>(&sql)
        .bind(user_id)
        .bind(query)
        .bind(limit.clamp(1, 100))
        .bind(candidate_cap(cap))
        .fetch_all(pool)
        .await?;
    Ok(SearchPage::from_rows(rows))
}

/// Blend bounded keyword relevance with cosine similarity. Keyword-only rows
/// remain eligible, so missing/stale embeddings do not make exact matches
/// disappear. Semantic candidates stay guarded by model and content revision.
pub async fn hybrid_for_user(pool: &PgPool, request: &HybridSearch<'_>) -> Result<Vec<SearchHit>> {
    Ok(hybrid_page_for_user(pool, request).await?.hits)
}

/// [`hybrid_for_user`] plus whether the keyword side exceeded
/// [`KEYWORD_CANDIDATE_CAP`].
pub async fn hybrid_page_for_user(pool: &PgPool, request: &HybridSearch<'_>) -> Result<SearchPage> {
    hybrid_page_with_cap(pool, request, KEYWORD_CANDIDATE_CAP).await
}

/// Hybrid search whose keyword side ranks only the newest `cap` matches (see
/// [`keyword_page_with_cap`]). Exposed for tests and tuning.
pub async fn hybrid_page_with_cap(
    pool: &PgPool,
    request: &HybridSearch<'_>,
    cap: i64,
) -> Result<SearchPage> {
    let (column, dictionary) = text_search_spec(request.text_config);
    let result_limit = request.limit.clamp(1, 100);
    let candidate_limit = result_limit.saturating_mul(4).clamp(result_limit, 400);
    let semantic_weight = request.semantic_weight.clamp(0.0, 1.0);
    let sql = format!(
        "WITH q AS NOT MATERIALIZED (SELECT websearch_to_tsquery('{dictionary}', $2) AS value), \
         recent AS MATERIALIZED ( \
           SELECT a.id, a.published_at, a.{column} AS document \
           FROM articles a \
           JOIN user_feeds uf ON uf.feed_id = a.feed_id AND uf.user_id = $1 \
           CROSS JOIN q \
           WHERE a.{column} @@ q.value \
           ORDER BY a.published_at DESC NULLS LAST, a.id \
           LIMIT $8::int8 + 1 \
         ), \
         keyword AS ( \
           SELECT bounded.id, ts_rank_cd(bounded.document, q.value, 32)::float8 AS keyword_score \
           FROM (SELECT * FROM recent ORDER BY published_at DESC NULLS LAST, id LIMIT $8) bounded \
           CROSS JOIN q \
           ORDER BY keyword_score DESC, bounded.published_at DESC NULLS LAST, bounded.id \
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
                 (1 - $6::float8) * COALESCE(keyword.keyword_score, 0)) AS final_score, \
                (SELECT count(*) FROM recent) > $8 AS truncated \
         FROM candidates \
         JOIN articles a ON a.id = candidates.id \
         JOIN feeds f ON f.id = a.feed_id \
         JOIN user_feeds uf ON uf.feed_id = a.feed_id AND uf.user_id = $1 \
         LEFT JOIN keyword ON keyword.id = candidates.id \
         LEFT JOIN semantic ON semantic.id = candidates.id \
         ORDER BY final_score DESC, a.published_at DESC NULLS LAST, a.id \
         LIMIT $7"
    );
    // The semantic CTE goes through HNSW; the outer ORDER BY re-sorts exactly.
    let mut transaction = begin_vector_scan(pool).await?;
    let rows = sqlx::query_as::<_, PagedHit>(&sql)
        .bind(request.user_id)
        .bind(request.query)
        .bind(to_pg_vector(request.query_vector))
        .bind(request.model)
        .bind(candidate_limit)
        .bind(semantic_weight)
        .bind(result_limit)
        .bind(candidate_cap(cap))
        .fetch_all(&mut *transaction)
        .await?;
    transaction.commit().await?;
    Ok(SearchPage::from_rows(rows))
}
